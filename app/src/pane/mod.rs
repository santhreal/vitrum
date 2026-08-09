//! A terminal pane drawn by the GPU instead of by xterm.js.
//!
//! Today a session's output reaches the screen as decoded pane operations
//! handed to xterm.js inside the WebKit view. This module is the other end of
//! that road: a `GtkDrawingArea` with its own X window, a `wgpu` swapchain on
//! that window, and `vitrum-grid` painting a [`CellGrid`] into it, hosted in
//! the same GTK toplevel the shell already has. Keystrokes on the widget are
//! encoded by [`key`] and handed to whatever the caller passes as the input
//! sink, which in the app is the same `ClientMsg::Input` frame the webview's
//! keyboard path already sends.
//!
//! The mechanism was proved and measured in `crates/vitrum-pane-lab` before
//! any of it was written here. On an RTX 4090 the native Vulkan path sustained
//! 24.7 MB/s with frame times of 0.79 ms at p50, 1.22 ms at p95 and 1.28 ms at
//! p99; xterm.js in the DOM sustained a higher 44.9 MB/s but with a p99 of
//! 46 ms and a worst frame of 147 ms. Throughput is not the argument, and this
//! module is not a performance change. The argument is that going native
//! leaves one parser in the product instead of two, and puts OSC 7 and OSC 133
//! semantics — working directory, prompt and command boundaries, exit status —
//! in Rust where the sidebar can read them, rather than in a JavaScript
//! addon's private state.
//!
//! # What is not here yet
//!
//! The pane cannot replace xterm.js until all of the following exist. None of
//! them is started, and each is named because a half-built pane that silently
//! lacks one of them is worse than no pane:
//!
//! - **Input method composition.** [`key`] reads a committed character out of
//!   a key event. That is wrong for anyone typing through an IME: the widget
//!   has to own a `GtkIMContext`, show preedit text in the grid, and send only
//!   what the context commits. Until then Chinese, Japanese and Korean input
//!   do not work in this pane at all.
//! - **Selection and clipboard.** There is no pointer handling, so no
//!   click-drag selection, no word or line selection, no rectangular
//!   selection, no autoscroll at the edges, and nothing wired to
//!   `GtkClipboard` for either the CLIPBOARD or the PRIMARY selection. Paste
//!   also needs bracketed-paste framing, which the pane cannot do without
//!   knowing the emulator's mode state.
//! - **Search.** The in-pane find bar, its match highlighting and its
//!   scroll-to-match all read xterm.js's search addon today. A native pane
//!   needs the equivalent over the scrollback the emulator holds, and it has
//!   to agree with the daemon's cross-session search on what a match is.
//! - **Scrollback paging.** [`PaneSurface`] renders exactly the live grid.
//!   There is no viewport offset, no wheel or Page Up handling, no scrollbar,
//!   and no path to the retained history the socket can already backfill.
//! - **Theme.** The renderer is constructed with `RendererConfig::default()`
//!   and `Style::DEFAULT`. Nothing reads the palette the rest of the client
//!   uses, so colours, font family, font size and cell metrics do not follow
//!   the operator's settings and do not change when those settings change.
//! - **Wayland.** [`PaneSurface`] is X11 only: it presents to an XID obtained
//!   from `gdk_x11_window_get_xid`. Under a Wayland GDK backend that call
//!   returns nothing usable and attaching fails with a diagnostic. A Wayland
//!   pane needs a subsurface and a `wl_surface` handle instead, which is a
//!   different attach path with different sizing and scaling rules.
//!
//! Two more gaps are worth naming even though they are smaller: there is no
//! mouse reporting to the child (SGR 1006 and friends), and nothing here
//! resizes the pty when the widget resizes — [`PaneSurface::resize`] reports
//! the new cell count and expects its caller to pass that on.
//!
//! [`CellGrid`]: vitrum_grid::CellGrid

pub(crate) mod key;
mod surface;

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::Result;
use gtk::prelude::*;

pub(crate) use surface::PaneSurface;

/// Where a pane's keystrokes go.
///
/// A boxed closure rather than a session id, because the pane must not know
/// what a session is. The app hands it one that sends `ClientMsg::Input` for
/// the focused session; a test or a lab harness hands it one that writes to a
/// pty directly, and the widget cannot tell the difference.
pub(crate) type InputSink = Box<dyn Fn(Vec<u8>)>;

/// A terminal pane widget: a native GPU surface plus a keyboard.
///
/// The surface is created on realize rather than in [`TerminalPane::new`],
/// because there is no X window to present to before then and asking for one
/// early is how the prototype first failed.
pub(crate) struct TerminalPane {
    area: gtk::DrawingArea,
    surface: Rc<RefCell<Option<PaneSurface>>>,
}

impl TerminalPane {
    /// Build the widget and wire its keyboard to `sink`.
    ///
    /// Nothing is drawn and no GPU resource is created until the widget is
    /// realized and [`TerminalPane::surface`] is first taken.
    pub(crate) fn new(sink: InputSink) -> Self {
        let area = gtk::DrawingArea::new();
        area.set_can_focus(true);
        area.set_app_paintable(true);
        // GTK's own draw handler must not run: the pixels under this widget
        // belong to the swapchain, and a themed background painted on expose
        // would race it.
        area.connect_draw(|_, _| glib::Propagation::Stop);
        area.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::KEY_PRESS_MASK
                | gdk::EventMask::STRUCTURE_MASK,
        );

        // Click to focus. Without this the pane never takes the keyboard from
        // the webview, because a `GtkDrawingArea` has no focus behaviour of
        // its own.
        area.connect_button_press_event(|area, _| {
            area.grab_focus();
            glib::Propagation::Stop
        });

        // The handler is on the widget, not the toplevel. A pane that hooked
        // the window would eat keystrokes aimed at the sidebar's filter field
        // whenever focus was anywhere else.
        area.connect_key_press_event(move |_, ev| match surface::encode_event(ev) {
            Some(bytes) => {
                sink(bytes);
                glib::Propagation::Stop
            }
            // Not a keystroke the terminal sends: let the shell's own keymap
            // have it.
            None => glib::Propagation::Proceed,
        });

        let surface: Rc<RefCell<Option<PaneSurface>>> = Rc::new(RefCell::new(None));
        {
            // Following the widget's size is the pane's job, not its host's:
            // the swapchain must be reconfigured before the next frame or the
            // driver reports the surface as outdated on every acquire.
            let surface = Rc::clone(&surface);
            area.connect_size_allocate(move |_, alloc| {
                let mut slot = surface.borrow_mut();
                let Some(pane) = slot.as_mut() else {
                    return;
                };
                if let Some((cols, rows)) =
                    pane.resize(alloc.width().max(1) as u32, alloc.height().max(1) as u32)
                {
                    tracing::debug!("pane resized to {cols}x{rows} cells");
                }
            });
        }

        Self { area, surface }
    }

    /// The widget to pack into a container.
    pub(crate) fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Create the GPU surface, once the widget is realized.
    ///
    /// Idempotent: a second call with a surface already attached is a no-op,
    /// so a host that calls this from both `realize` and its first frame gets
    /// one swapchain.
    pub(crate) fn attach(&self) -> Result<()> {
        if self.surface.borrow().is_some() {
            return Ok(());
        }
        let pane = PaneSurface::attach(&self.area)?;
        *self.surface.borrow_mut() = Some(pane);
        Ok(())
    }

    /// Run `f` against the attached surface, if there is one.
    ///
    /// The surface is behind a `RefCell` because GTK callbacks and the host
    /// both reach it, and this is the only way in: handing out the
    /// [`PaneSurface`] would let a caller hold it across a callback that also
    /// wants it.
    pub(crate) fn with_surface<T>(&self, f: impl FnOnce(&mut PaneSurface) -> T) -> Option<T> {
        self.surface.borrow_mut().as_mut().map(f)
    }
}
