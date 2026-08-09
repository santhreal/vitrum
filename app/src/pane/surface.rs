//! The native GPU surface behind a pane, and the gdk half of key input.
//!
//! A `GtkDrawingArea` normally shares its toplevel's X window and has no
//! drawable of its own. [`gdk_window_ensure_native`] gives it one, and that
//! window's XID is something `wgpu` can create a surface on. That is the whole
//! mechanism: it is what lets a Vulkan swapchain live inside the same GTK
//! toplevel as the WebKit view the shell is drawn with, with no offscreen
//! copy and no compositing pass in between.
//!
//! The prototype in `crates/vitrum-pane-lab` proved this works and measured
//! it. What is here is the same sequence, with the lab's benchmark harness,
//! pty, argument parsing and side-by-side webview experiment left behind.
//!
//! X11 only, deliberately: see the module doc of [`super`] for what Wayland
//! needs and why it is not attempted here.

use std::ffi::c_void;

use anyhow::{Context, Result, anyhow};
use glib::translate::ToGlibPtr;
use gtk::prelude::*;
use vitrum_grid::{CellGrid, GridRenderer, RendererConfig, Style};

use super::key::{Key, Mods, Named, encode};

// Functions that live in `libgdk-3` but have no gtk-rs binding.
//
// Declaring them beats pulling in `gdkx11` for two symbols: they are in the
// library gtk-rs already links, and the pointers come from gtk-rs types.
unsafe extern "C" {
    fn gdk_x11_window_get_xid(window: *mut gdk::ffi::GdkWindow) -> core::ffi::c_ulong;
    fn gdk_x11_display_get_xdisplay(display: *mut gdk::ffi::GdkDisplay) -> *mut c_void;
}

/// An X11 display connection wgpu can hold onto.
///
/// `wgpu` wants the display handle to be `Send + Sync + 'static` because an
/// instance can be shared across threads. This one never leaves the GTK main
/// thread; the unsafe impls record that, rather than pretending Xlib is
/// thread-safe in general.
#[derive(Debug)]
struct XDisplay {
    ptr: *mut c_void,
    screen: i32,
}

// SAFETY: the pointer is only ever dereferenced by wgpu on the thread that
// created it, because the pane drives wgpu solely from the GTK main loop.
unsafe impl Send for XDisplay {}
// SAFETY: as above.
unsafe impl Sync for XDisplay {}

impl wgpu::rwh::HasDisplayHandle for XDisplay {
    fn display_handle(&self) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
        let raw = wgpu::rwh::XlibDisplayHandle::new(core::ptr::NonNull::new(self.ptr), self.screen);
        // SAFETY: the handle borrows `self`, which owns a connection GTK keeps
        // open for the life of the process.
        Ok(unsafe { wgpu::rwh::DisplayHandle::borrow_raw(wgpu::rwh::RawDisplayHandle::Xlib(raw)) })
    }
}

/// A swapchain on a widget's own X window, plus the grid drawn into it.
pub(crate) struct PaneSurface {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: GridRenderer,
    grid: CellGrid,
    /// Pixel size of the drawable, which is not derivable from the grid: the
    /// last partial column and row are padding the renderer still has to clear.
    size: (u32, u32),
}

impl PaneSurface {
    /// Take over `area`'s drawing and return a surface that presents to it.
    ///
    /// `area` must already be realized: the X window only exists after that,
    /// and there is no XID to present to before it does.
    pub(crate) fn attach(area: &gtk::DrawingArea) -> Result<Self> {
        // GTK must not paint a background into this widget's window: the X11
        // window under it belongs to the GPU, and a themed background drawn on
        // every expose would race the swapchain and flicker.
        area.set_app_paintable(true);

        let gdk_window = area
            .window()
            .ok_or_else(|| anyhow!("pane widget has no GdkWindow; realize it before attaching"))?;
        // Without this the widget shares the toplevel's X window and there is
        // no XID to present to. This is the whole trick behind a native pane
        // inside a GTK window.
        gdk_window.ensure_native();

        let alloc = area.allocation();
        let size = (alloc.width().max(1) as u32, alloc.height().max(1) as u32);

        // SAFETY: both pointers come from live gtk-rs objects the widget holds.
        let (xid, xdisplay) = unsafe {
            let display = gdk_window.display();
            (
                gdk_x11_window_get_xid(gdk_window.to_glib_none().0),
                gdk_x11_display_get_xdisplay(display.to_glib_none().0),
            )
        };
        if xid == 0 {
            return Err(anyhow!(
                "gdk_x11_window_get_xid returned 0: the pane needs the X11 GDK backend"
            ));
        }

        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(XDisplay {
                ptr: xdisplay,
                screen: 0,
            })),
        );

        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(wgpu::rwh::RawDisplayHandle::Xlib(
                wgpu::rwh::XlibDisplayHandle::new(core::ptr::NonNull::new(xdisplay), 0),
            )),
            raw_window_handle: wgpu::rwh::RawWindowHandle::Xlib(wgpu::rwh::XlibWindowHandle::new(
                xid,
            )),
        };
        // SAFETY: `xid` names a window GTK keeps alive for as long as the
        // widget lives, and the display pointer is GTK's own connection.
        let surface = unsafe { instance.create_surface_unsafe(target) }
            .with_context(|| format!("create wgpu surface on XID {xid:#x}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|e| anyhow!("no GPU adapter can present to the pane's window: {e}"))?;

        let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vitrum.pane.device"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| anyhow!("request GPU device for the pane: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        // Terminal colours are already sRGB byte values. An `*_srgb` swapchain
        // format would run them through a second encode and every colour on
        // screen would be wrong, so a linear format is required, not preferred.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        // Fifo, not Immediate. The lab used Immediate to measure how fast the
        // path can go; a pane in a desktop shell wants the frame the
        // compositor is going to show and no tearing, and Fifo is the only
        // present mode every driver guarantees.
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0,
            height: size.1,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let renderer = GridRenderer::new(
            &device,
            &RendererConfig {
                format,
                ..RendererConfig::default()
            },
        )
        .map_err(|e| anyhow!("build the pane's glyph renderer: {e}"))?;

        let (cols, rows) = renderer.grid_size_for(size.0, size.1);
        let grid = CellGrid::new(cols.max(2), rows.max(2), Style::DEFAULT)
            .map_err(|e| anyhow!("allocate the pane's cell grid: {e}"))?;

        Ok(Self {
            device,
            queue,
            surface,
            config,
            renderer,
            grid,
            size,
        })
    }

    /// The grid the pane paints. Whatever owns the parser writes into this.
    pub(crate) fn grid_mut(&mut self) -> &mut CellGrid {
        &mut self.grid
    }

    /// Columns and rows currently allocated, which is what the pty's winsize
    /// has to be told.
    pub(crate) fn grid_size(&self) -> (u16, u16) {
        (self.grid.cols(), self.grid.rows())
    }

    /// One cell in pixels, for the winsize's pixel fields.
    pub(crate) fn cell_size(&self) -> (u32, u32) {
        self.renderer.cell_size()
    }

    /// Follow the widget to a new pixel size.
    ///
    /// Returns the new grid size when the cell count changed, so the caller
    /// can resize the emulator and the pty in the same breath. A resize that
    /// only changes the leftover padding returns `None` and costs a
    /// reconfigure.
    pub(crate) fn resize(&mut self, width: u32, height: u32) -> Option<(u16, u16)> {
        let size = (width.max(1), height.max(1));
        if size == self.size {
            return None;
        }
        self.size = size;
        self.config.width = size.0;
        self.config.height = size.1;
        self.surface.configure(&self.device, &self.config);

        let (cols, rows) = self.renderer.grid_size_for(size.0, size.1);
        let (cols, rows) = (cols.max(2), rows.max(2));
        if (cols, rows) == self.grid_size() {
            // The swapchain is new, so the previous frame's contents are gone
            // and the renderer's idea of what is on screen is stale.
            self.renderer.invalidate();
            self.grid.mark_all_damaged();
            return None;
        }
        if let Err(err) = self.grid.resize(cols, rows) {
            tracing::error!("pane grid resize to {cols}x{rows} failed: {err}");
            return None;
        }
        self.renderer.invalidate();
        self.grid.mark_all_damaged();
        Some((cols, rows))
    }

    /// Draw the grid, if anything changed, and present.
    ///
    /// Returns whether a frame was actually put on screen. A clean grid
    /// submits no GPU command at all, which is what makes an idle pane free.
    pub(crate) fn present(&mut self) -> Result<bool> {
        if !self.grid.is_dirty() {
            return Ok(false);
        }

        use wgpu::CurrentSurfaceTexture as Acquired;
        let frame = match self.surface.get_current_texture() {
            Acquired::Success(frame) | Acquired::Suboptimal(frame) => frame,
            // Nothing to draw into this time. A full rebuild is queued so the
            // recreated swapchain does not inherit a half-drawn frame's
            // damage state, and the next write to the grid brings the pane
            // back.
            other => {
                self.renderer.invalidate();
                self.grid.mark_all_damaged();
                return Err(anyhow!("pane swapchain unavailable: {other:?}"));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let stats = self
            .renderer
            .render(&self.device, &self.queue, &mut self.grid, &view, self.size)
            .map_err(|e| anyhow!("render the pane: {e}"))?;
        // Present either way: the texture was acquired, and dropping it
        // unpresented wedges the swapchain.
        frame.present();
        Ok(stats.gpu_work)
    }
}

/// Translate a GTK key event into the bytes the child expects.
///
/// Returns `None` for a keystroke that sends nothing: a bare modifier press,
/// or a keyval with no character and no named sequence. That decision lives
/// here rather than in [`super::key`] so the encoder never has to represent
/// "no keystroke".
pub(crate) fn encode_event(ev: &gdk::EventKey) -> Option<Vec<u8>> {
    let state = ev.state();
    let mods = Mods {
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        // MOD1 is Alt on every desktop this ships to. Super is not read: a
        // Super chord belongs to the window manager or the shell keymap, and
        // the pane must not swallow it.
        alt: state.contains(gdk::ModifierType::MOD1_MASK),
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
    };
    let key = classify(ev.keyval())?;
    Some(encode(key, mods))
}

/// A gdk keyval, as the encoder sees it.
///
/// The named table is the only place gdk constants appear. Everything past it
/// is toolkit-free, which is why the encoding is testable without a display.
fn classify(kv: gdk::keys::Key) -> Option<Key> {
    use gdk::keys::constants as k;

    let named = match kv {
        k::Return => Named::Enter,
        k::KP_Enter => Named::KeypadEnter,
        k::Tab | k::ISO_Left_Tab => Named::Tab,
        k::BackSpace => Named::Backspace,
        k::Escape => Named::Escape,
        k::Up | k::KP_Up => Named::Up,
        k::Down | k::KP_Down => Named::Down,
        k::Right | k::KP_Right => Named::Right,
        k::Left | k::KP_Left => Named::Left,
        k::Home | k::KP_Home => Named::Home,
        k::End | k::KP_End => Named::End,
        k::Page_Up | k::KP_Page_Up => Named::PageUp,
        k::Page_Down | k::KP_Page_Down => Named::PageDown,
        k::Insert | k::KP_Insert => Named::Insert,
        k::Delete | k::KP_Delete => Named::Delete,
        k::F1 => Named::F1,
        k::F2 => Named::F2,
        k::F3 => Named::F3,
        k::F4 => Named::F4,
        k::F5 => Named::F5,
        k::F6 => Named::F6,
        k::F7 => Named::F7,
        k::F8 => Named::F8,
        k::F9 => Named::F9,
        k::F10 => Named::F10,
        k::F11 => Named::F11,
        k::F12 => Named::F12,
        // Not a named key. If the layout produced a character, that character
        // is the keystroke; otherwise this was a modifier or a dead key and
        // there is nothing to send.
        _ => return kv.to_unicode().map(Key::Char),
    };
    Some(Key::Named(named))
}
