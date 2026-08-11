//! The native GPU surface behind a pane, and the gdk half of key input.
//!
//! A `GtkDrawingArea` normally shares its toplevel's X window and has no
//! drawable of its own. `gdk_window_ensure_native` gives it one, and that
//! window's XID is something `wgpu` can create a surface on. That is the whole
//! mechanism: it is what lets a Vulkan swapchain live inside the same GTK
//! toplevel as the shell, with no offscreen copy and no compositing pass in
//! between.
//!
//! The surface owns the GPU and nothing else. The cell grid it paints belongs
//! to [`super::session::PaneSession`], because the same grid is what the
//! selection, the search overlay and the emulator all write into, and two
//! copies of it would disagree for one frame every time either changed.
//!
//! X11 only. [`Backend::detect`] says so in a diagnostic that names the
//! remedy rather than leaving a blank widget behind.

use std::ffi::c_void;

use anyhow::{Context, Result, anyhow};
use glib::translate::ToGlibPtr;
use gtk::prelude::*;
use vitrum_grid::font::FontConfig;
use vitrum_grid::{CellGrid, FontStack, GridRenderer, RendererConfig, Rgba};

use super::geometry::PaneRect;
use super::key::{Key, Mods, Named, encode};
use super::theme::Present;

// Functions that live in `libgdk-3` but have no gtk-rs binding.
//
// Declaring them beats pulling in `gdkx11` for two symbols: they are in the
// library gtk-rs already links, and the pointers come from gtk-rs types.
unsafe extern "C" {
    fn gdk_x11_window_get_xid(window: *mut gdk::ffi::GdkWindow) -> core::ffi::c_ulong;
    fn gdk_x11_display_get_xdisplay(display: *mut gdk::ffi::GdkDisplay) -> *mut c_void;
}

/// Which GDK backend the process ended up on.
///
/// GDK picks this at startup from the session, and a pane that assumes X11
/// under Wayland does not fail: it produces a widget that never paints, which
/// is defect six on the operator's list wearing a different hat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// An X11 display. The XID path works.
    X11,
    /// A Wayland display. Named separately from any other backend because the
    /// remedy is different and the operator can apply it.
    Wayland,
    /// Something else, named by its GDK type so a bug report can say which.
    Other(String),
}

impl Backend {
    /// Read the backend off a live display.
    pub(crate) fn detect(display: &gdk::Display) -> Self {
        Self::from_type_name(&display.type_().name())
    }

    /// The classification, split out so it is testable with no display.
    pub(crate) fn from_type_name(name: &str) -> Self {
        if name.contains("X11") {
            Self::X11
        } else if name.contains("Wayland") {
            Self::Wayland
        } else {
            Self::Other(name.to_owned())
        }
    }

    /// What to tell the operator when the pane cannot be created.
    ///
    /// Every branch names an action. A message that says only what failed
    /// leaves someone staring at an empty rectangle, which is the failure mode
    /// this whole check exists to replace.
    pub(crate) fn unsupported(&self) -> Option<String> {
        match self {
            Self::X11 => None,
            Self::Wayland => Some(
                "the pane needs the X11 GDK backend and this session is Wayland. \
                 Start vitrum with GDK_BACKEND=x11 to run it through XWayland. \
                 A native Wayland pane needs the drawing area's surface exposed \
                 as a wl_subsurface and that is not built."
                    .to_owned(),
            ),
            Self::Other(name) => Some(format!(
                "the pane needs the X11 GDK backend and this session reports {name}. \
                 Start vitrum with GDK_BACKEND=x11."
            )),
        }
    }
}

/// Present modes the pane is allowed to configure.
///
/// Both hand the compositor a complete image. `Fifo` queues finished frames
/// and releases one per vertical blank; `Mailbox` replaces the queued frame
/// with the newest finished one and never waits for the compositor, which is
/// the lowest latency a composited desktop can offer.
///
/// `wgpu::PresentMode::Immediate` is the third mode a driver offers and it is
/// deliberately not representable here. It hands the image straight to the
/// scanout that is already reading, so a present that lands part way down the
/// panel shows the top of one frame and the bottom of the next. The renderer
/// clears its attachment and redraws every instance each frame, so the two
/// halves are two different states of the whole grid rather than two states of
/// one changed row: a line of text that moved appears in both places at once.
/// Naming the safe set as a type is what makes the tearing mode unreachable
/// instead of merely unchosen, because there is no value of this enum that
/// produces it and every path to a swapchain configuration goes through one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SafeMode {
    Fifo,
    Mailbox,
}

impl SafeMode {
    const fn wgpu(self) -> wgpu::PresentMode {
        match self {
            Self::Fifo => wgpu::PresentMode::Fifo,
            Self::Mailbox => wgpu::PresentMode::Mailbox,
        }
    }
}

/// The tear-free mode a theme's present choice asks for.
///
/// Exhaustive on purpose: a choice added to [`Present`] stops compiling here
/// until someone says which tear-free mode it means.
const fn wanted(present: Present) -> SafeMode {
    match present {
        Present::Vsync => SafeMode::Fifo,
        // Both of these ask for "do not wait for the compositor". Mailbox is
        // the answer to that which does not tear, so it serves both.
        Present::Newest | Present::Immediate => SafeMode::Mailbox,
    }
}

/// Pick a present mode the adapter actually offers.
///
/// A configure with an unsupported mode is a panic inside wgpu, so an
/// operator choosing the newest-frame mode on a driver without it would crash
/// the window. Fifo is guaranteed by the specification everywhere, which is
/// why it is the fallback and not a second guess.
pub(crate) fn clamp_present(want: Present, offered: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    let want = wanted(want).wgpu();
    if offered.contains(&want) {
        return want;
    }
    SafeMode::Fifo.wgpu()
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

/// A swapchain on a widget's own X window.
pub(crate) struct PaneSurface {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: GridRenderer,
    /// Present modes this adapter offers on this surface, for clamping a
    /// setting change without asking the adapter again.
    offered: Vec<wgpu::PresentMode>,
    /// Pixel size of the drawable, which is not derivable from the grid: the
    /// last partial column and row are padding the renderer still has to clear.
    size: (u32, u32),
}

impl PaneSurface {
    /// Take over `area`'s drawing, present `grid` once, and return the surface.
    ///
    /// `area` must already be realized: the X window only exists after that,
    /// and there is no XID to present to before it does.
    ///
    /// The first frame is painted here rather than left to the first tick.
    /// A freshly created X window shows whatever was in that framebuffer,
    /// which on this stack is black, and a pane that waits for its first
    /// output before painting shows black until the child says something.
    /// That is one of the two causes of a pane the operator sees as black.
    ///
    /// # Errors
    ///
    /// The backend is not X11, the widget has no window, no adapter can
    /// present to it, or the first frame could not be drawn.
    pub(crate) fn attach(
        area: &gtk::DrawingArea,
        font: FontConfig,
        present: Present,
        grid: &mut CellGrid,
    ) -> Result<Self> {
        // GTK must not paint a background into this widget's window: the X11
        // window under it belongs to the GPU, and a themed background drawn on
        // every expose would race the swapchain and flicker.
        area.set_app_paintable(true);

        let gdk_window = area
            .window()
            .ok_or_else(|| anyhow!("pane widget has no GdkWindow; realize it before attaching"))?;
        let display = gdk_window.display();
        if let Some(why) = Backend::detect(&display).unsupported() {
            return Err(anyhow!("{why}"));
        }
        // Without this the widget shares the toplevel's X window and there is
        // no XID to present to. This is the whole trick behind a native pane
        // inside a GTK window.
        {
            let _span = crate::boot::span("pane.ensure_native");
            gdk_window.ensure_native();
        }

        let scale = area.scale_factor().max(1) as u32;
        let alloc = area.allocation();
        let size = (
            (alloc.width().max(1) as u32) * scale,
            (alloc.height().max(1) as u32) * scale,
        );

        // SAFETY: both pointers come from live gtk-rs objects the widget holds.
        let (xid, xdisplay) = unsafe {
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

        let instance = {
            let _span = crate::boot::span("wgpu.instance");
            wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(XDisplay {
                    ptr: xdisplay,
                    screen: 0,
                })),
            )
        };

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
        let surface = {
            let _span = crate::boot::span("wgpu.surface");
            // SAFETY: `xid` names a window GTK keeps alive for as long as the
            // widget lives, and the display pointer is GTK's own connection.
            unsafe { instance.create_surface_unsafe(target) }
        }
        .with_context(|| format!("create wgpu surface on XID {xid:#x}"))?;

        let adapter = {
            let _span = crate::boot::span("wgpu.adapter");
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            }))
        }
        .map_err(|e| anyhow!("no GPU adapter can present to the pane's window: {e}"))?;

        let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        let (device, queue) = {
            let _span = crate::boot::span("wgpu.device");
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("vitrum.pane.device"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))
        }
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
        let offered = caps.present_modes.clone();
        let chosen = clamp_present(present, &offered);
        if chosen != wanted(present).wgpu() {
            tracing::info!(
                wanted = ?present,
                using = ?chosen,
                "the GPU does not offer the requested present mode"
            );
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0,
            height: size.1,
            present_mode: chosen,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        {
            let _span = crate::boot::span("wgpu.configure");
            surface.configure(&device, &config);
        }

        let fonts = {
            let _span = crate::boot::span("font.stack");
            FontStack::system(&font)
        }
        .map_err(|e| anyhow!("discover the pane's monospace face: {e}"))?;
        let renderer = {
            let _span = crate::boot::span("grid.pipeline");
            GridRenderer::with_fonts(
                &device,
                &RendererConfig { format, font, ..RendererConfig::default() },
                fonts,
            )
        };

        let mut this = Self {
            device,
            queue,
            surface,
            config,
            renderer,
            offered,
            size,
        };

        let (cols, rows) = this.cells_for(size.0, size.1);
        if (cols, rows) != (grid.cols(), grid.rows()) {
            grid.resize(cols, rows)
                .map_err(|e| anyhow!("size the pane's cell grid to {cols}x{rows}: {e}"))?;
        }
        grid.mark_all_damaged();
        {
            let _span = crate::boot::span("pane.present");
            this.present(grid).context("paint the pane's first frame")?;
        }
        Ok(this)
    }

    /// Columns and rows that fit a pixel size.
    ///
    /// Through [`super::geometry`], which is the only place in the pane that
    /// divides a box into cells. The renderer's own division floors the same
    /// way but floors to one, and a one-column grid is not a terminal: a
    /// child told it has one column wraps every line into a vertical stripe.
    pub(crate) fn cells_for(&self, width: u32, height: u32) -> (u16, u16) {
        PaneRect {
            x: 0,
            y: 0,
            width,
            height,
        }
        .grid(self.cell_size())
    }

    /// One cell in pixels, for the winsize's pixel fields.
    pub(crate) fn cell_size(&self) -> (u32, u32) {
        self.renderer.cell_size()
    }

    /// The drawable's pixel size.
    pub(crate) const fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Change the present mode while the window is open.
    ///
    /// Returns the mode actually in force, which is the requested one unless
    /// the adapter does not offer it.
    pub(crate) fn set_present(&mut self, want: Present) -> wgpu::PresentMode {
        let chosen = clamp_present(want, &self.offered);
        if chosen == self.config.present_mode {
            return chosen;
        }
        self.config.present_mode = chosen;
        self.surface.configure(&self.device, &self.config);
        // The swapchain images are new, so nothing the renderer believes is on
        // screen is on screen.
        self.renderer.invalidate();
        chosen
    }

    /// Rebuild the glyph renderer for a new font or size.
    ///
    /// Returns the new cell count for the current pixel size, so the caller
    /// resizes the emulator and the pty in the same breath.
    ///
    /// # Errors
    ///
    /// No usable face was found for the requested families.
    pub(crate) fn set_font(&mut self, font: FontConfig) -> Result<(u16, u16)> {
        let renderer = GridRenderer::new(
            &self.device,
            &RendererConfig {
                format: self.config.format,
                font,
                ..RendererConfig::default()
            },
        )
        .map_err(|e| anyhow!("rebuild the pane's glyph renderer: {e}"))?;
        self.renderer = renderer;
        Ok(self.cells_for(self.size.0, self.size.1))
    }

    /// Follow the widget to a new pixel size.
    ///
    /// Returns the cell count for the new size, whether or not it changed, so
    /// a caller never has to remember what it was.
    pub(crate) fn resize(&mut self, width: u32, height: u32) -> (u16, u16) {
        let size = (width.max(1), height.max(1));
        if size != self.size {
            self.size = size;
            self.config.width = size.0;
            self.config.height = size.1;
            self.surface.configure(&self.device, &self.config);
            // The swapchain is new, so the previous frame's contents are gone
            // and the renderer's idea of what is on screen is stale.
            self.renderer.invalidate();
        }
        self.cells_for(size.0, size.1)
    }

    /// Say that what the server is showing is no longer the frame this
    /// renderer drew.
    ///
    /// The only caller is the widget's expose handler. Presenting is skipped
    /// while nothing changed, and "nothing changed" is a statement about the
    /// grid, not about the window: an X server with no backing store drops the
    /// pixels of a window that was covered, and the renderer goes on believing
    /// its last frame is still up. This is how it is told otherwise, and the
    /// next tick redraws the frame it already has rather than waiting for the
    /// agent to write something.
    pub(crate) fn forget_what_is_on_screen(&mut self) {
        self.renderer.invalidate();
    }

    /// Dim everything this surface draws, or stop dimming it.
    ///
    /// A modal sheet dims what is behind it. The pane cannot be dimmed from
    /// behind a toolkit scrim: it draws into a native child window of the
    /// shell's, and a translucent widget laid over that is a window of its
    /// own, which paints its background opaque and turns the terminal into a
    /// black rectangle. So the sheet tells the pane, and the pane dims itself.
    pub(crate) fn set_veil(&mut self, color: Rgba, strength: f32) {
        self.renderer.set_veil(color, strength);
    }

    /// Draw the grid, if anything changed, and present.
    ///
    /// Returns whether a frame was actually put on screen. A clean grid over a
    /// swapchain the renderer still owns submits no GPU command at all, which
    /// is what makes an idle pane free.
    ///
    /// The skip needs both halves of the question answered. `is_dirty` says
    /// whether a cell changed; `needs_rebuild` says whether what is on screen
    /// is still the frame this renderer drew. Reconfiguring a swapchain hands
    /// back a new set of images with undefined contents, and a resize inside
    /// one cell, a present-mode change and a font rebuild all reconfigure
    /// without changing a single cell. Skipping those frames leaves the
    /// operator looking at whatever was in that memory until the child writes
    /// something, which is a pane that goes to garbage on a drag and stays
    /// there.
    ///
    /// # Errors
    ///
    /// The swapchain could not produce an image twice running, or the renderer
    /// failed.
    pub(crate) fn present(&mut self, grid: &mut CellGrid) -> Result<bool> {
        if !grid.is_dirty() && !self.renderer.needs_rebuild() {
            return Ok(false);
        }
        match self.draw(grid) {
            Ok(drawn) => Ok(drawn),
            Err(first) => {
                // An outdated or lost swapchain is the normal consequence of a
                // resize the widget has not told us about yet. Reconfiguring
                // and drawing again costs one frame; giving up costs the
                // operator a pane that stops updating until they type.
                self.surface.configure(&self.device, &self.config);
                self.renderer.invalidate();
                grid.mark_all_damaged();
                self.draw(grid)
                    .with_context(|| format!("after reconfiguring the pane's swapchain: {first}"))
            }
        }
    }

    /// One attempt at a frame.
    ///
    /// An acquired image is presented only when this call drew the whole of it.
    /// A swapchain image is recycled, so it holds an older frame's pixels until
    /// something writes it; the render pass clears the attachment and redraws
    /// every instance, so once it has run the image is a complete frame, and
    /// until it has run the image is whatever was there two frames ago.
    /// Presenting one of those is the stale-frame flash the operator sees as a
    /// flicker, and it is why the caller's decision to draw is not enough on
    /// its own: the render can still decline, and a declined render must take
    /// the image with it rather than put it on screen.
    fn draw(&mut self, grid: &mut CellGrid) -> Result<bool> {
        use wgpu::CurrentSurfaceTexture as Acquired;
        let frame = match self.surface.get_current_texture() {
            Acquired::Success(frame) | Acquired::Suboptimal(frame) => frame,
            other => return Err(anyhow!("pane swapchain unavailable: {other:?}")),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let stats = match self
            .renderer
            .render(&self.device, &self.queue, grid, &view, self.size)
        {
            Ok(stats) => stats,
            Err(e) => {
                // Nothing was written into this image. Returning it to the
                // swapchain unpresented is what the caller's reconfigure then
                // recovers from, and it is the only option that does not put
                // an unpainted image in front of the operator.
                drop(view);
                drop(frame);
                return Err(anyhow!("render the pane: {e}"));
            }
        };
        if !stats.gpu_work {
            drop(view);
            drop(frame);
            return Ok(false);
        }
        // Nothing waits on the GPU here, so the UI thread returns before the
        // frame is scanned out. The present is ordered behind the submit, so
        // the compositor never reads an image the queue is still writing.
        frame.present();
        Ok(true)
    }
}

/// Translate a GTK key event into the key and modifiers it names.
///
/// Returns `None` for a keystroke that means nothing: a bare modifier press,
/// or a keyval with no character and no named sequence. That decision lives
/// here rather than in [`super::key`] so the encoder never has to represent
/// "no keystroke".
pub(crate) fn decode_event(ev: &gdk::EventKey) -> Option<(Key, Mods)> {
    let state = ev.state();
    let mods = Mods {
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        // MOD1 is Alt on every desktop this ships to. Super is not read: a
        // Super chord belongs to the window manager or the shell keymap, and
        // the pane must not swallow it.
        alt: state.contains(gdk::ModifierType::MOD1_MASK),
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
    };
    Some((classify(ev.keyval())?, mods))
}

/// The bytes a keystroke sends to the child.
pub(crate) fn encode_event(ev: &gdk::EventKey) -> Option<Vec<u8>> {
    let (key, mods) = decode_event(ev)?;
    Some(encode(key, mods))
}

/// The top-row digit the physical key carries, if it carries one.
///
/// The layout's name for Ctrl+Shift+1 is `!`, so a chord bound to a digit
/// would never match the keystroke it is named after. Asking the keymap what
/// the same hardware key produces with no modifiers is the only way to get
/// the digit back, and it is asked once per press rather than kept in a
/// table that would go stale when the layout changes.
pub(crate) fn digit_of(ev: &gdk::EventKey) -> Option<char> {
    let keymap = gdk::Keymap::for_display(&ev.window()?.display())?;
    let (keyval, ..) = keymap.translate_keyboard_state(
        ev.hardware_keycode().into(),
        gdk::ModifierType::empty(),
        ev.group().into(),
    )?;
    let ch = gdk::keys::Key::from(keyval).to_unicode()?;
    ch.is_ascii_digit().then_some(ch)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// WHY: a pane on the wrong backend used to produce a widget that never
    /// painted. Silence is the defect; the classification and the remedy text
    /// are the fix, so both are pinned here.
    ///
    /// Names are the real GDK type names, which is what the check reads.
    #[test]
    fn a_backend_that_cannot_work_says_so_and_says_what_to_do() {
        assert_eq!(Backend::from_type_name("GdkX11Display"), Backend::X11);
        assert_eq!(Backend::from_type_name("GdkX11Display").unsupported(), None);

        let wayland = Backend::from_type_name("GdkWaylandDisplay");
        assert_eq!(wayland, Backend::Wayland);
        let why = wayland.unsupported().expect("Wayland is not supported");
        assert!(why.contains("GDK_BACKEND=x11"), "no remedy named: {why}");

        let other = Backend::from_type_name("GdkBroadwayDisplay");
        assert_eq!(other, Backend::Other("GdkBroadwayDisplay".to_owned()));
        let why = other.unsupported().expect("Broadway is not supported");
        assert!(why.contains("GdkBroadwayDisplay"), "backend unnamed: {why}");
        assert!(why.contains("GDK_BACKEND=x11"), "no remedy named: {why}");
    }

    /// Every present choice the pane can be given, so a choice added to
    /// [`Present`] has to be added here as well. The list is short enough to
    /// hold in one place, and `wanted` is exhaustive, so a new variant stops
    /// the crate compiling before it reaches this test.
    const EVERY_CHOICE: [Present; 3] = [Present::Vsync, Present::Newest, Present::Immediate];

    /// WHY: configuring a surface with a present mode the adapter does not
    /// offer panics inside wgpu, so an operator picking one in Settings would
    /// take the window down. Fifo is guaranteed everywhere and is the floor.
    #[test]
    fn a_present_mode_the_gpu_lacks_falls_back_to_one_it_has() {
        use wgpu::PresentMode::{Fifo, Immediate, Mailbox};

        for (want, expect) in [
            (Present::Vsync, Fifo),
            (Present::Newest, Mailbox),
            (Present::Immediate, Mailbox),
        ] {
            assert_eq!(clamp_present(want, &[Fifo, Immediate, Mailbox]), expect);
        }

        // Only Fifo, which is the minimum any driver may offer.
        for want in EVERY_CHOICE {
            assert_eq!(clamp_present(want, &[Fifo]), Fifo, "{want:?}");
        }

        // An adapter offering nothing recognised still returns something
        // configurable rather than the caller's unsupported choice.
        assert_eq!(clamp_present(Present::Newest, &[]), Fifo);
    }

    /// WHY: `Immediate` hands a frame to the scanout that is already reading
    /// the panel, so a present that lands part way down shows the top of one
    /// frame and the bottom of the next. The renderer redraws the whole
    /// attachment every frame, so those halves are two states of the whole
    /// grid, and the operator sees a line of text in two places at once.
    ///
    /// The invariant this closes is not "the default does not tear", which
    /// only covers the reported case. It is that no present choice, over any
    /// set of modes an adapter can report, produces a tearing swapchain: not
    /// the choice named after it, and not a fallback taken because the mode
    /// that was asked for is missing. Falling back from Mailbox to Immediate
    /// is how the tearing mode used to be reached without anyone selecting it.
    #[test]
    fn no_present_choice_can_configure_a_tearing_swapchain() {
        use wgpu::PresentMode::{AutoNoVsync, AutoVsync, Fifo, FifoRelaxed, Immediate, Mailbox};

        // Every subset of what an adapter can report, so no combination of
        // offered modes routes a choice into the tearing one.
        let all = [Fifo, FifoRelaxed, Immediate, Mailbox, AutoVsync, AutoNoVsync];
        for bits in 0u32..(1 << all.len()) {
            let offered: Vec<wgpu::PresentMode> = all
                .iter()
                .enumerate()
                .filter(|(i, _)| bits & (1 << i) != 0)
                .map(|(_, m)| *m)
                .collect();
            for want in EVERY_CHOICE {
                let chosen = clamp_present(want, &offered);
                assert_ne!(
                    chosen, Immediate,
                    "{want:?} over {offered:?} configured a tearing swapchain"
                );
                assert!(
                    matches!(chosen, Fifo | Mailbox),
                    "{want:?} over {offered:?} chose {chosen:?}, which is neither \
                     of the two modes that present a complete frame"
                );
                assert!(
                    offered.contains(&chosen) || chosen == Fifo,
                    "{want:?} over {offered:?} chose {chosen:?}, which the adapter \
                     did not offer and which is not the guaranteed floor"
                );
            }
        }
    }
}
