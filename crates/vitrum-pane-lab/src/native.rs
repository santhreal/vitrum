//! The prototype: a GTK 3 window whose terminal pane is a native GPU surface.
//!
//! The pane is a `GtkDrawingArea` forced to own a real X11 window, a `wgpu`
//! surface created straight on that XID, a `vitrum_vt::Vt` fed from a real PTY,
//! and `vitrum_grid`'s renderer drawing the `CellGrid` into the swapchain. No
//! webview is involved in the pane, and no JavaScript exists in the process
//! unless `--webview` is passed.
//!
//! `--webview` is the compositing experiment. It packs a real `WebKitWebView`
//! into the same toplevel next to the pane, which is the arrangement the
//! shipping shell would need if the pane went native while the chrome stayed
//! Dioxus. What that flag proves or disproves is visible in the screenshot and
//! in where keystrokes land.
//!
//! # Why the frame loop is fd-driven
//!
//! There is no timer and no `requestAnimationFrame` equivalent here. The PTY
//! master fd is added to the GTK main loop, and a frame happens when, and only
//! when, bytes arrive and the sync actually changed a cell. An idle window
//! blocks in `poll` exactly as the webview shell does, which is the only way
//! the 0% idle claim survives contact with a render loop.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use glib::translate::ToGlibPtr;
use gtk::prelude::*;
use webkit2gtk::WebViewExt;
use vitrum_grid::{CellGrid, GridRenderer, RendererConfig, Style};
use vitrum_vt::{Vt, VtOptions};

use crate::pty::{self, Pty};
use crate::stats::Run;

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
// created it, because this binary drives wgpu solely from the GTK main loop.
unsafe impl Send for XDisplay {}
// SAFETY: as above.
unsafe impl Sync for XDisplay {}

impl wgpu::rwh::HasDisplayHandle for XDisplay {
    fn display_handle(
        &self,
    ) -> Result<wgpu::rwh::DisplayHandle<'_>, wgpu::rwh::HandleError> {
        let raw = wgpu::rwh::XlibDisplayHandle::new(
            core::ptr::NonNull::new(self.ptr),
            self.screen,
        );
        // SAFETY: the handle borrows `self`, which owns a connection GTK keeps
        // open for the life of the process.
        Ok(unsafe {
            wgpu::rwh::DisplayHandle::borrow_raw(wgpu::rwh::RawDisplayHandle::Xlib(raw))
        })
    }
}

/// Command line for `pane-lab native`.
struct Args {
    cols: u16,
    rows: u16,
    webview: bool,
    overlay: bool,
    seconds: u64,
    stats: Option<String>,
    argv: Vec<String>,
    vsync: bool,
}

fn parse(args: &[String]) -> Result<Args> {
    let mut out = Args {
        cols: 100,
        rows: 30,
        webview: false,
        overlay: false,
        seconds: 0,
        stats: None,
        argv: Vec::new(),
        vsync: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cols" => {
                out.cols = args[i + 1].parse()?;
                i += 2;
            }
            "--rows" => {
                out.rows = args[i + 1].parse()?;
                i += 2;
            }
            "--seconds" => {
                out.seconds = args[i + 1].parse()?;
                i += 2;
            }
            "--stats" => {
                out.stats = Some(args[i + 1].clone());
                i += 2;
            }
            "--webview" => {
                out.webview = true;
                i += 1;
            }
            // The harder half of the compositing question: not "can they sit
            // side by side" but "can webview content be drawn OVER the GPU
            // surface", which is what every dialog, menu and tooltip in the
            // shell would have to do.
            "--overlay" => {
                out.webview = true;
                out.overlay = true;
                i += 1;
            }
            "--vsync" => {
                out.vsync = true;
                i += 1;
            }
            "--" => {
                out.argv = args[i + 1..].to_vec();
                i = args.len();
            }
            other => return Err(anyhow!("unknown flag {other}")),
        }
    }
    if out.argv.is_empty() {
        out.argv = vec!["/usr/bin/python3".into(), "-q".into()];
    }
    Ok(out)
}

/// Everything one frame needs, owned by the fd callback.
struct Pane {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    renderer: GridRenderer,
    grid: CellGrid,
    vt: Vt,
    size: (u32, u32),
    run: Run,
    scratch: Vec<u8>,
    pty_out: Vec<u8>,
}

impl Pane {
    /// Feed `bytes`, sync, and present only if a cell actually changed.
    ///
    /// Returns the microseconds the whole byte-to-pixels path took, or `None`
    /// when the bytes changed nothing on screen and no GPU work was recorded.
    fn feed_and_draw(&mut self, bytes: &[u8]) -> Result<Option<u64>> {
        let t0 = Instant::now();
        self.vt.feed(bytes);

        // Anything the engine wants to answer (device attributes, DA1, cursor
        // reports) goes back up the pty, or a program that asks a question
        // waits forever and the measurement is of a stalled child.
        self.pty_out.clear();
        self.vt.drain_pty_write(&mut self.pty_out);

        let sync = self.vt.sync(&mut self.grid)?;
        if sync.is_noop() {
            return Ok(None);
        }

        use wgpu::CurrentSurfaceTexture as Acquired;
        let frame = match self.surface.get_current_texture() {
            Acquired::Success(frame) | Acquired::Suboptimal(frame) => frame,
            // Nothing to draw into this time. The next byte to arrive brings
            // the pane back, and a full rebuild is queued so the recreated
            // swapchain does not inherit a half-drawn frame's damage state.
            other => {
                eprintln!("surface unavailable: {other:?}");
                self.renderer.invalidate();
                return Ok(None);
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let stats = self
            .renderer
            .render(&self.device, &self.queue, &mut self.grid, &view, self.size)
            .map_err(|e| anyhow!("render: {e}"))?;
        if !stats.gpu_work {
            // Nothing was submitted, so there is nothing new to show. Present
            // anyway: the texture was acquired and dropping it unpresented
            // wedges the swapchain.
            frame.present();
            return Ok(None);
        }
        // The timer closes after present, not after submit. A number that
        // stops at submit measures how fast we can queue work, which is not a
        // frame.
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok();
        frame.present();
        Ok(Some(t0.elapsed().as_micros() as u64))
    }
}

/// Run the native pane.
pub fn run(args: &[String]) -> Result<()> {
    let args = parse(args)?;
    gtk::init().context("gtk_init: is DISPLAY set?")?;

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("vitrum pane lab (native)");
    window.set_default_size(1280, 760);

    let area = gtk::DrawingArea::new();
    area.set_can_focus(true);
    // GTK must not paint a background into this widget's window: the X11
    // window under it belongs to the GPU, and a themed background drawn on
    // every expose would race the swapchain and flicker.
    area.set_app_paintable(true);
    area.connect_draw(|_, _| glib::Propagation::Stop);

    let webview = if args.webview {
        let wv = webkit2gtk::WebView::new();
        wv.load_html(WEBVIEW_HTML, None);
        Some(wv)
    } else {
        None
    };

    if args.overlay {
        // The pane fills the window and the webview floats on top of part of
        // it, which is the arrangement a dialog or a menu would need.
        let stack = gtk::Overlay::new();
        stack.add(&area);
        if let Some(wv) = &webview {
            wv.set_size_request(420, 320);
            wv.set_halign(gtk::Align::End);
            wv.set_valign(gtk::Align::Start);
            wv.set_margin_top(60);
            wv.set_margin_end(60);
            stack.add_overlay(wv);
        }
        window.add(&stack);
    } else {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.pack_start(&area, true, true, 0);
        if let Some(wv) = &webview {
            wv.set_size_request(360, -1);
            root.pack_start(wv, false, true, 0);
        }
        window.add(&root);
    }
    window.show_all();
    // The surface can only be created once the widget owns an X window, which
    // happens at realize. `show_all` above did that; the flush makes sure the
    // server has processed it before wgpu asks about the drawable.
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }

    let gdk_window = area
        .window()
        .ok_or_else(|| anyhow!("drawing area has no GdkWindow after realize"))?;
    // Without this the widget shares the toplevel's X window and there is no
    // XID to present to. This is the whole trick behind a native pane inside a
    // GTK window.
    gdk_window.ensure_native();

    let alloc = area.allocation();
    let size = (alloc.width().max(1) as u32, alloc.height().max(1) as u32);

    // SAFETY: both pointers come from live gtk-rs objects the window holds.
    let (xid, xdisplay) = unsafe {
        let display = gdk_window.display();
        (
            gdk_x11_window_get_xid(gdk_window.to_glib_none().0),
            gdk_x11_display_get_xdisplay(display.to_glib_none().0),
        )
    };
    if xid == 0 {
        return Err(anyhow!("gdk_x11_window_get_xid returned 0: not an X11 backend"));
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
        Box::new(XDisplay {
            ptr: xdisplay,
            screen: 0,
        }),
    ));

    let target = wgpu::SurfaceTargetUnsafe::RawHandle {
        raw_display_handle: Some(wgpu::rwh::RawDisplayHandle::Xlib(
            wgpu::rwh::XlibDisplayHandle::new(core::ptr::NonNull::new(xdisplay), 0),
        )),
        raw_window_handle: wgpu::rwh::RawWindowHandle::Xlib(wgpu::rwh::XlibWindowHandle::new(xid)),
    };
    // SAFETY: `xid` names a window GTK keeps alive for the life of the
    // process, and the display pointer is GTK's own connection.
    let surface = unsafe { instance.create_surface_unsafe(target) }
        .map_err(|e| anyhow!("create_surface on XID {xid:#x}: {e}"))?;

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: Some(&surface),
    }))
    .map_err(|e| anyhow!("no adapter can present to this window: {e}"))?;

    let info = adapter.get_info();
    let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pane-lab.device"),
        required_features: wgpu::Features::empty(),
        required_limits: limits,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| anyhow!("device: {e}"))?;

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
    let present_mode = if args.vsync {
        wgpu::PresentMode::Fifo
    } else {
        caps.present_modes
            .iter()
            .copied()
            .find(|m| *m == wgpu::PresentMode::Immediate)
            .unwrap_or(wgpu::PresentMode::Fifo)
    };
    surface.configure(
        &device,
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0,
            height: size.1,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        },
    );

    let renderer = GridRenderer::new(
        &device,
        &RendererConfig {
            format,
            ..RendererConfig::default()
        },
    )
    .map_err(|e| anyhow!("renderer: {e}"))?;

    let cell = renderer.cell_size();
    let (cols, rows) = renderer.grid_size_for(size.0, size.1);
    let cols = cols.min(args.cols).max(2);
    let rows = rows.min(args.rows).max(2);

    let grid = CellGrid::new(cols, rows, Style::DEFAULT).map_err(|e| anyhow!("grid: {e}"))?;
    let vt = Vt::new(VtOptions {
        cols,
        rows,
        max_scrollback: 10_000,
    })
    .map_err(|e| anyhow!("vt: {e}"))?;

    println!(
        "adapter: {} ({:?}, {:?})\nsurface: {format:?} {present_mode:?} {}x{}\ngrid: {cols}x{rows} cells of {}x{} px\nchild: {:?}\nwebview_in_window: {}",
        info.name, info.backend, info.device_type, size.0, size.1, cell.0, cell.1, args.argv,
        webview.is_some()
    );

    let pty = Pty::spawn(&args.argv, cols, rows, cell)?;
    let fd = pty.fd;

    let pane = Rc::new(RefCell::new(Pane {
        device,
        queue,
        surface,
        renderer,
        grid,
        vt,
        size,
        run: Run::new("native-ghostty-vitrum-grid"),
        scratch: Vec::with_capacity(1 << 20),
        pty_out: Vec::new(),
    }));
    let pty = Rc::new(RefCell::new(pty));

    // Keystrokes. The pane owns the keyboard whenever it has focus, and this
    // handler is on the toplevel so the routing question in `--webview` mode
    // is answered by whether it fires at all when the webview has focus.
    {
        let pty = Rc::clone(&pty);
        let area = area.clone();
        window.connect_key_press_event(move |_, ev| {
            if !area.has_focus() {
                return glib::Propagation::Proceed;
            }
            if let Some(bytes) = encode_key(ev) {
                let _ = pty.borrow_mut().write(&bytes);
            }
            glib::Propagation::Stop
        });
    }
    {
        let area2 = area.clone();
        area.connect_button_press_event(move |_, _| {
            area2.grab_focus();
            glib::Propagation::Stop
        });
        area.add_events(gdk::EventMask::BUTTON_PRESS_MASK);
    }
    area.grab_focus();

    // The one and only wakeup source. No timer is armed here, which is what
    // the idle measurement depends on.
    let wakeups = Rc::new(std::cell::Cell::new(0u64));
    let empty_wakeups = Rc::new(std::cell::Cell::new(0u64));
    {
        let pane = Rc::clone(&pane);
        let pty_for_read = Rc::clone(&pty);
        let wakeups = Rc::clone(&wakeups);
        let empty_wakeups = Rc::clone(&empty_wakeups);
        glib::unix_fd_add_local(
            fd,
            glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
            // Two counters, because "0% idle" and "no spin" are different
            // claims. `wakeups` is how many times the main loop was woken at
            // all; `empty_wakeups` is how many of those found no bytes, which
            // is the signature of a poll that is firing on a condition nobody
            // cleared.
            move |_, _| {
                let mut pane = pane.borrow_mut();
                let mut buf = std::mem::take(&mut pane.scratch);
                buf.clear();
                let open = match pty::drain(fd, &mut buf) {
                    Ok(open) => open,
                    Err(err) => {
                        eprintln!("pty read failed: {err}");
                        false
                    }
                };
                if !buf.is_empty() {
                    let n = buf.len();
                    match pane.feed_and_draw(&buf) {
                        Ok(Some(micros)) => pane.run.frame(n, micros),
                        Ok(None) => pane.run.frame(n, 0),
                        Err(err) => eprintln!("frame failed: {err}"),
                    }
                    let out = std::mem::take(&mut pane.pty_out);
                    if !out.is_empty() {
                        let _ = pty_for_read.borrow_mut().write(&out);
                    }
                    pane.pty_out = out;
                } else {
                    empty_wakeups.set(empty_wakeups.get() + 1);
                }
                wakeups.set(wakeups.get() + 1);
                pane.scratch = buf;
                if open {
                    glib::ControlFlow::Continue
                } else {
                    gtk::main_quit();
                    glib::ControlFlow::Break
                }
            },
        );
    }

    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        glib::Propagation::Proceed
    });

    if args.seconds > 0 {
        // Only armed for a bounded bench run. An idle-CPU measurement passes
        // `--seconds 0` so the process holds no timer at all.
        glib::timeout_add_seconds_local(args.seconds as u32, || {
            gtk::main_quit();
            glib::ControlFlow::Break
        });
    }

    gtk::main();
    pty.borrow_mut().kill();

    let mut report = pane.borrow().run.report();
    report["adapter"] = serde_json::json!(format!(
        "{} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    ));
    report["present_mode"] = serde_json::json!(format!("{present_mode:?}"));
    report["cols"] = serde_json::json!(cols);
    report["rows"] = serde_json::json!(rows);
    report["loop_wakeups"] = serde_json::json!(wakeups.get());
    report["loop_wakeups_with_no_bytes"] = serde_json::json!(empty_wakeups.get());
    report["webview_in_window"] = serde_json::json!(webview.is_some());
    report["overlay"] = serde_json::json!(args.overlay);
    let text = serde_json::to_string_pretty(&report)?;
    if let Some(path) = &args.stats {
        std::fs::write(path, &text)?;
    }
    println!("{text}");
    Ok(())
}

/// Translate a GTK key event into the bytes a PTY expects.
///
/// Deliberately small: this is enough to prove keystrokes reach the child
/// through a native pane, not a keymap worth shipping. The real one is a
/// substantial piece of work and is costed in the report.
fn encode_key(ev: &gdk::EventKey) -> Option<Vec<u8>> {
    use gdk::keys::constants as key;
    let state = ev.state();
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let kv = ev.keyval();
    let bytes = match kv {
        key::Return | key::KP_Enter => vec![b'\r'],
        key::BackSpace => vec![0x7f],
        key::Tab => vec![b'\t'],
        key::Escape => vec![0x1b],
        key::Up => b"\x1b[A".to_vec(),
        key::Down => b"\x1b[B".to_vec(),
        key::Right => b"\x1b[C".to_vec(),
        key::Left => b"\x1b[D".to_vec(),
        _ => {
            let ch = kv.to_unicode()?;
            if ctrl && ch.is_ascii_alphabetic() {
                vec![(ch.to_ascii_uppercase() as u8) - b'@']
            } else {
                let mut buf = [0u8; 4];
                ch.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
    };
    Some(bytes)
}

/// The page loaded into the side-by-side webview.
///
/// Loud colours and a focusable input, because the compositing experiment is
/// answered by two questions a screenshot can settle: is the webview visible
/// at all next to a GPU surface in the same toplevel, and does typing into it
/// reach the DOM rather than the pty.
const WEBVIEW_HTML: &str = r#"<!doctype html><meta charset=utf-8>
<style>
 html,body{margin:0;height:100%;background:#7a1fa2;color:#fff;
   font:14px/1.5 system-ui,sans-serif}
 body{padding:12px;box-sizing:border-box}
 input{width:100%;font:inherit;padding:6px;box-sizing:border-box}
 #echo{margin-top:8px;font-weight:700;word-break:break-all}
</style>
<h3>WebKitGTK webview</h3>
<p>Same GTK toplevel as the native GPU pane.</p>
<input id=i placeholder="type here">
<div id=echo>echo: (nothing)</div>
<script>
 const i=document.getElementById('i');
 i.addEventListener('input',()=>{document.getElementById('echo').textContent='echo: '+i.value});
</script>"#;
