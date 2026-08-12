//! Bringing one window up, and everything it owns while it is open.
//!
//! # What this file is
//!
//! The client used to be a document mounted in a webview, and the order of
//! operations was the renderer's. It is now a GTK toplevel with panels in it,
//! and the order is here: create the window, install the pane in it, mount the
//! three panels, start the two pumps, show it. Every mark on the boot timeline
//! is taken at the point in this function that produced it.
//!
//! # Why there is a `Ctx`
//!
//! Every reducer function needs the same five things: the window's state, the
//! socket, the command line, which session the daemon believes is attached,
//! and the requests this window has sent and not seen confirmed. Passing them
//! as five parameters is what the previous shell did, and adding a sixth meant
//! editing forty call sites. They are one value now, held behind an `Rc` so a
//! callback that outlives the call can keep it.
//!
//! `Cell` and `RefCell` rather than a lock: a window and everything in it is
//! one thread, the GTK main thread, and a lock would buy nothing for a hazard
//! that cannot arise.
//!
//! # Why the pumps are `spawn_local` and not tasks
//!
//! Both receivers hand out values that move the window's state, and the state
//! is not `Send`. The futures therefore run on the GTK main loop, driven by
//! glib, while the socket's own I/O runs on the tokio runtime's workers. The
//! runtime is entered for the life of the process, which is what lets a
//! `tokio::time::sleep` inside one of these futures resolve at all.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use vitrum_os::deeplink::DeepLink;
use vitrum_proto::{ClientMsg, SessionId};

use crate::actions::PendingLaunch;
use crate::cli::Options;
use crate::geometry::{
    claim_ordinal, fresh_geometry, live_window_count, load_geometry, measure, monitor_rects,
    monitors, release_ordinal, remember, remember_sidebar, remembered, save_geometry,
    save_window_state, seed_book, window_ui_scale,
};
use crate::shell::{Ident, Shell, Slot};
use crate::state::{Flash, UiState};
use crate::wire::ClientEvent;
use crate::{Tick, boot, fixture, instance, keys, launch, pane, socket, splash, state, sync, tray};

/// Handle on the client's outside world.
///
/// One handle, two directions. Everything with a session id or a byte offset
/// in it goes to the daemon through [`socket::Net`]; everything the window
/// itself observed goes back to the reducer through `ui`, which is the same
/// queue the pane's keystrokes arrive on. Handlers should not have to know
/// which, and a second handle threaded beside this one would be a second thing
/// every call site could forget to pass.
#[derive(Clone)]
pub(crate) struct Bridge {
    net: Rc<RefCell<socket::Net>>,
    ui: UnboundedSender<ClientEvent>,
}

impl Bridge {
    /// Encode and send a control-plane message to the daemon.
    pub(crate) fn msg(&self, m: &ClientMsg) {
        self.net.borrow_mut().send(crate::wire::encode(m));
    }

    /// Open, or reopen, the session socket.
    pub(crate) fn connect(&self, url: String) {
        self.net.borrow_mut().connect(url);
    }

    /// Close the session socket without opening another.
    pub(crate) fn hang_up(&self) {
        self.net.borrow_mut().hang_up();
    }

    /// Which socket generation is current, so a dying one's events can be
    /// discarded rather than allowed to overwrite the new one's state.
    pub(crate) fn epoch(&self) -> u64 {
        self.net.borrow().epoch()
    }

    /// Give the socket the pane it feeds.
    ///
    /// Called once per window, at bring-up. Ops the socket produced before
    /// this are held rather than dropped, so a session whose first bytes
    /// arrived during startup paints them.
    pub(crate) fn attach_pane(&self, sink: Rc<RefCell<dyn socket::PaneSink>>) {
        self.net.borrow_mut().attach_pane(sink);
    }

    /// Point the pane at `session`, or clear it.
    pub(crate) fn focus(&self, session: Option<SessionId>) {
        self.net
            .borrow_mut()
            .drive(|stream, ops| stream.focus(session, ops));
    }

    /// Claim the pane for a page-back, once the request has actually gone out.
    pub(crate) fn arm_page_back(&self) {
        self.net.borrow_mut().stream.arm_page_back();
    }

    /// Paint history, then the live frames buffered behind it.
    pub(crate) fn backfill(
        &self,
        session: SessionId,
        from_seq: u64,
        resume_seq: u64,
        bytes: Vec<u8>,
        jump_seq: Option<u64>,
        keep_view: bool,
    ) {
        boot::mark("scrollback.restored");
        self.net.borrow_mut().drive(move |stream, ops| {
            stream.backfill(session, from_seq, resume_seq, bytes, jump_seq, keep_view, ops);
        });
    }

    /// One data frame off the socket, still in the buffer it arrived in.
    pub(crate) fn output(&self, frame: socket::Frame) {
        self.net
            .borrow_mut()
            .drive(move |stream, ops| stream.output(frame, ops));
    }

    /// Fixture mode's substitute for a session: literal lines, no socket.
    pub(crate) fn banner(&self, lines: &[String]) {
        self.net
            .borrow_mut()
            .drive(|stream, ops| stream.banner(lines, ops));
    }

    /// Anything the pane needs the operator told, drained.
    pub(crate) fn notices(&self) -> Vec<String> {
        self.net.borrow_mut().stream.take_notices()
    }

    /// Report something the window observed to the reducer.
    pub(crate) fn raise(&self, event: ClientEvent) {
        if self.ui.send(event).is_err() {
            tracing::debug!("client event dropped: the window is closing");
        }
    }

    /// Put `text` on the system clipboard, and say whether it landed.
    ///
    /// Reported rather than assumed: a copy that did not happen and a "Copied"
    /// notice that says it did is a lie the operator only discovers when they
    /// paste.
    pub(crate) fn clipboard(&self, text: String) {
        let ok = crate::set_clipboard(&text);
        self.raise(ClientEvent::Copied { ok, text });
    }
}

/// Everything one window's handlers need, in one value.
///
/// Held behind an `Rc` by every callback and every pump. The four cells are
/// the window's own bookkeeping and deliberately not part of [`UiState`]: none
/// of them is on screen, none is persisted, and putting them in the model
/// would make every panel repaint when a reconnect counter moved.
pub(crate) struct Ctx {
    pub(crate) shell: Shell,
    pub(crate) bridge: Bridge,
    pub(crate) opts: Options,
    /// Session the daemon currently believes this window is attached to.
    ///
    /// Kept separate from `window.focused` so one reconciler drives every
    /// attach and detach, including the ones a reconnect causes rather than a
    /// click.
    pub(crate) attached: Cell<Option<SessionId>>,
    /// Reconnects tried since the last accepted handshake. Zero while
    /// connected, so a window at rest schedules nothing.
    pub(crate) reconnect: Cell<u32>,
    /// The launch THIS window asked for, held until the daemon says it exists.
    /// `SessionCreated` is broadcast to every window, so without a record of
    /// who asked, a window cannot tell its own launch from another's.
    pub(crate) pending_open: RefCell<Option<PendingLaunch>>,
    /// A session a `vitrum://session/N` handoff asked this window to open.
    pub(crate) pending_link: Cell<Option<SessionId>>,
}

impl Ctx {
    /// Read the window's state without holding a borrow past the call.
    pub(crate) fn peek<R>(&self, f: impl FnOnce(&UiState) -> R) -> R {
        self.shell.peek(f)
    }

    /// Change the window's state and repaint.
    pub(crate) fn edit<R>(&self, f: impl FnOnce(&mut UiState) -> R) -> R {
        self.shell.edit(f)
    }

    /// Put one sentence on the notice.
    pub(crate) fn flash(&self, flash: Flash) {
        self.edit(|st| st.window.flash = Some(flash));
    }
}

/// Open the first window and run the main loop. Never returns.
///
/// # Ordering, which is the whole of this function
///
/// The toolkit comes up first because the monitor list does not exist until it
/// has. The window is created from that list, so a restored window opens at
/// the size and scale of the panel it will actually appear on rather than at a
/// nominal 1280x800 corrected after the first paint. The pane is installed
/// before the panels are mounted, so it is parsing and holding a grid for the
/// whole interval the shell is still being built. The window is shown last,
/// because a window shown first presents an empty frame and then fills it,
/// which is one visible reflow on every launch.
pub(crate) fn launch(opts: Options, link: Option<DeepLink>) -> ! {
    {
        let _span = boot::span("gtk.init");
        if let Err(e) = gtk::init() {
            // Nothing this program does is possible without a display, and a
            // message naming the variable is what an operator can act on.
            eprintln!("vitrum: cannot reach a display server: {e}");
            std::process::exit(1);
        }
    }

    // The runtime the socket does its I/O on, entered for the life of the
    // process. The guard is what makes `Handle::current` resolve on the GTK
    // thread, which `socket::Net` captures and every `tokio::time::sleep` in a
    // pump needs.
    let runtime = {
        let _span = boot::span("tokio.runtime");
        match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(e) => {
                eprintln!("vitrum: no async runtime: {e}");
                std::process::exit(1);
            }
        }
    };
    let _entered = runtime.enter();

    let (all, primary) = {
        let _span = boot::span("monitors.probe");
        monitors()
    };

    // The pane's font stack: a font-directory scan and four face parses, tens
    // of milliseconds, needing no window and no GPU. It is built here on its
    // own thread, for the configuration the pane will ask for, so that by the
    // time the window is on screen and the pane attaches, the answer is
    // already sitting in the slot. The toolkit scale is the primary monitor's,
    // which is the scale the widget reports on every machine with one display
    // and the overwhelmingly common case; a window that lands somewhere else
    // misses and pays what it paid before.
    {
        let _span = boot::span("fonts.prewarm");
        let scale = primary
            .as_ref()
            .map_or(1.0, |m| f64::from(m.scale_factor().max(1)));
        let theme = crate::pane::theme_from(&crate::state::live::pane_settings());
        vitrum_grid::prewarm_font_stack(theme.font_config(scale));
    }

    // The same trade for the GPU: an instance, an adapter and a device, none
    // of which needs a window, a surface or a size, and all three of which
    // the pane used to build after the window existed and on the thread that
    // paints it. Ninety milliseconds of the ninety-odd between the shell
    // appearing and the pane's first glyph was this. A pane that attaches
    // before the thread finishes builds its own.
    #[cfg(target_os = "linux")]
    {
        let _span = boot::span("gpu.prewarm");
        crate::pane::surface::prewarm_gpu();
    }
    {
        let _span = boot::span("geometry.load");
        seed_book(load_geometry(&monitor_rects(primary.as_ref(), &all)));
    }
    for monitor in &all {
        let d = crate::geometry::density_of(monitor);
        tracing::info!(
            "monitor {}x{} device px, {}x{} mm, {} dpi, toolkit scale {}, ui scale {}",
            d.width_px,
            d.height_px,
            d.width_mm,
            d.height_mm,
            d.dpi().map_or(-1.0, |v| (v * 10.0).round() / 10.0),
            d.os_scale,
            d.ui_scale()
        );
    }

    #[cfg(target_os = "linux")]
    if let Some(display) = gtk::gdk::Display::default() {
        let _span = boot::span("style.install");
        super::style::install(&display);
    }
    boot::mark("styles.built");

    {
        let _span = boot::span("open_on");
        open_on(opts, link, primary.as_ref());
    }

    gtk::main();
    // `gtk::main` returns when the last window asked the loop to quit. Nothing
    // is left to do, and the runtime is dropped by the exit rather than joined:
    // its only remaining work is a socket whose window is gone.
    std::process::exit(0);
}

/// Open another window in this process.
///
/// Called by the second-launch handoff and by nothing else. The toolkit, the
/// runtime and the geometry book are already up, so this is the tail of
/// [`launch`] and shares it.
pub(crate) fn open(opts: Options, link: Option<DeepLink>) {
    let (_all, primary) = monitors();
    open_on(opts, link, primary.as_ref());
}

/// Build one window on `primary`, wire it, and show it.
fn open_on(opts: Options, link: Option<DeepLink>, primary: Option<&gtk::gdk::Monitor>) {
    let scale = opts.ui_scale.unwrap_or_else(|| {
        primary
            .map(crate::geometry::density_of)
            .map_or(crate::geometry::MIN_UI_SCALE, |d| d.ui_scale())
    });

    let ordinal = claim_ordinal();
    let geometry = remembered(ordinal).unwrap_or_else(|| fresh_geometry(primary, scale, ordinal));
    remember(ordinal, geometry);

    let window = {
        let _span = boot::span("window.build");
        super::window::create(&geometry, scale)
    };
    boot::mark("window.created");
    {
        let _span = boot::span("splash.install");
        splash::install(&window);
    }

    // The queue the pane's keystrokes and every panel's request arrive on. It
    // exists before the pane does, because the pane's key handler is built
    // during installation and needs somewhere to put a chord.
    let (tx, pane_rx) = unbounded_channel();
    crate::hold_window_events(ordinal, tx.clone());

    let mut ui = UiState::default();
    // The slot this window persists into. Set at construction because nothing
    // else can know it, and every window defaulting to zero means they all
    // overwrite one entry.
    ui.window.index = ordinal;
    let server = ui.daemon.settings.resolved_daemon_url(opts.server).to_string();
    let shell = {
        let _span = boot::span("shell.new");
        Shell::new(
            &window,
            ui,
            tx.clone(),
            Ident {
                ordinal,
                server,
                home: crate::clock::home(),
            },
        )
    };

    // Fold the chord table when the profile changes, not per key press, and
    // follow the shell theme from the first settings document rather than from
    // the first edit of the session.
    {
        let _span = boot::span("settings.watchers");
        keys::watch_chords();
        crate::ui::settings::watch_shell();
    }

    let (net, socket_rx) = {
        let _span = boot::span("socket.new");
        socket::Net::new()
    };
    let bridge = Bridge {
        net: Rc::new(RefCell::new(net)),
        ui: tx,
    };

    // A borrowed slice, so a keystroke does not allocate on the way in. It
    // becomes an owned frame exactly once, here, because that is what the
    // socket will send.
    let sink: pane::InputSink = Box::new({
        let raise = bridge.clone();
        move |bytes: &[u8]| raise.raise(ClientEvent::Input { data: bytes.to_vec() })
    });
    let installed = {
        let _span = boot::span("pane.install");
        pane::install_in(&shell.pane_host(), ordinal, sink)
    };
    match installed {
        Ok(host) => {
            bridge.attach_pane(host.sink());
            // The pane's only way back that is not bytes. A resize is a
            // measurement the pane alone can make, a page-back is a scroll
            // that reached the top of what has been painted, and a copy is a
            // clipboard write that can be refused. All three are observations
            // about this window, so they go on the same queue the keystrokes
            // do.
            let raise = bridge.clone();
            host.on_report(Box::new(move |report| match report {
                pane::PaneReport::Resize { cols, rows } => {
                    raise.raise(ClientEvent::Resize { cols, rows });
                }
                pane::PaneReport::PageBack => raise.raise(ClientEvent::PageBack),
                pane::PaneReport::Copied { ok, text } => {
                    raise.raise(ClientEvent::Copied { ok, text });
                }
            }));
        }
        // A window with no pane still shows the sidebar, the bar and every
        // sheet, and says so through the pane's own empty state. Refusing to
        // open would take all of that away to report one of them.
        Err(e) => tracing::error!("no native pane on this window: {e:#}"),
    }

    let cx = Rc::new(Ctx {
        shell: shell.clone(),
        bridge,
        opts,
        attached: Cell::new(None),
        reconnect: Cell::new(0),
        pending_open: RefCell::new(None),
        pending_link: Cell::new(match link {
            Some(DeepLink::Session(id)) => Some(id),
            _ => None,
        }),
    });

    {
        let _span = boot::span("mount.titlebar");
        shell.mount(Slot::Titlebar, crate::ui::titlebar::native::panel(&shell));
    }
    {
        let _span = boot::span("mount.sidebar");
        shell.mount(Slot::Sidebar, crate::ui::sidebar::widgets::panel(&shell));
    }
    {
        let _span = boot::span("mount.panebar");
        shell.mount(Slot::PaneBar, crate::ui::panebar::panel(&shell));
    }
    {
        let _span = boot::span("mount.toast");
        crate::ui::toast::Toast::install(&shell);
        shell.observe(Layers::new(&shell));
    }

    {
        let _span = boot::span("wire.window");
        wire_sidebar_width(&shell, ordinal, scale);
        wire_window(&cx, &window, ordinal, scale);
        wire_keyboard(&cx, &window);
    }

    if ordinal == 0 {
        let _span = boot::span("wire.tray");
        wire_tray(&cx, &window);
    }

    {
        let _span = boot::span("window.show_all");
        window.show_all();
    }
    boot::mark("shell.mounted");

    pump_socket(&cx, socket_rx);
    pump_client(&cx, pane_rx);
    start_up(&cx);
    watch_saves(&cx);
    watch_updates(&cx);
    watch_activations(opts);
}

// ---------------------------------------------------------------------------
// The transient layer
// ---------------------------------------------------------------------------

/// Keeps the presented surface in step with `window.layer`.
///
/// An observer rather than a call at each site that sets the layer: the layer
/// is set from the menu, the keyboard, the sidebar, the titlebar and the
/// reducer, and a presentation done at each of those is five places for the
/// sixth to be forgotten.
struct Layers {
    shell: Shell,
    /// The layer the frame is currently showing, so an unrelated repaint does
    /// not tear down and rebuild the dialog the operator is typing in.
    showing: RefCell<crate::state::Layer>,
}

impl Layers {
    fn new(shell: &Shell) -> Rc<dyn super::Observer> {
        Rc::new(Self {
            shell: shell.clone(),
            showing: RefCell::new(crate::state::Layer::None),
        })
    }
}

impl super::Observer for Layers {
    fn state_changed(&self, state: &UiState, _at: Tick) {
        if *self.showing.borrow() == state.window.layer {
            return;
        }
        *self.showing.borrow_mut() = state.window.layer.clone();
        crate::ui::dialog::present_layer(&self.shell, &state.window.layer);
    }
}

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

/// Restore the divider the operator dragged, and remember where they leave it.
///
/// The paned's handle position IS the sidebar width. Nothing derives it and
/// nothing else writes it, which is what stops the terminal moving when the
/// list repaints.
fn wire_sidebar_width(shell: &Shell, ordinal: crate::WindowId, scale: f64) {
    let paned = shell.paned();
    let css = shell.peek(|st| st.window.sidebar_width);
    let want = match remembered(ordinal) {
        Some(state) if state.sidebar_width > 0 => f64::from(state.sidebar_width),
        _ => css,
    };
    paned.set_position((want * scale).round() as i32);

    let shell = shell.clone();
    paned.connect_position_notify(move |paned| {
        let css = f64::from(paned.position().max(0)) / scale;
        // Guarded, because writing the value the state already holds would
        // repaint the sidebar on every frame of a drag.
        if shell.peek(|st| (st.window.sidebar_width - css).abs() < 0.5) {
            return;
        }
        let viewport = f64::from(paned.allocated_width().max(0)) / scale;
        shell.update(move |st| st.window.set_sidebar_width_in(css, viewport));
        remember_sidebar(ordinal, css);
    });
}

/// Everything the window itself observes.
///
/// There is no timer behind any of it: a configure event arrives while the
/// operator drags, a state event when they maximise, and nothing at all while
/// the window sits still.
fn wire_window(cx: &Rc<Ctx>, window: &gtk::Window, ordinal: crate::WindowId, scale: f64) {
    // A drag delivers one configure event per frame, so the geometry is folded
    // into the book in memory and written to disk only when the operator has
    // finished: on focus loss and on close.
    window.connect_configure_event({
        let cx = Rc::clone(cx);
        move |window, _| {
            remember(ordinal, measure(window, ordinal));
            // A window dragged from an 82 dpi panel onto a 163 dpi one has to
            // be redrawn at the other panel's scale, or half the point of
            // measuring the scale at all is lost the first time somebody moves
            // a window.
            let next = window_ui_scale(window, cx.opts.ui_scale);
            if (next - scale).abs() > f64::EPSILON {
                tracing::info!("window {ordinal} moved to a {next}x monitor");
            }
            // `configure-event` reports whether the handler consumed the
            // event, not whether propagation stops. This one only measures.
            false
        }
    });
    window.connect_window_state_event({
        move |window, _| {
            remember(ordinal, measure(window, ordinal));
            glib::Propagation::Proceed
        }
    });

    // Focus loss and a close request are the two moments an operator has
    // finished moving a window. The profile is written here too, and that is
    // not housekeeping: the sidebar collapse and the whole strip live in the
    // window snapshot, and no collapse toggle ever commits on its own.
    window.connect_focus_out_event({
        let cx = Rc::clone(cx);
        move |window, _| {
            remember(ordinal, measure(window, ordinal));
            save_geometry();
            save_window_state(&cx.shell);
            glib::Propagation::Proceed
        }
    });

    window.connect_delete_event({
        let cx = Rc::clone(cx);
        move |window, _| {
            remember(ordinal, measure(window, ordinal));
            close(&cx, ordinal);
            glib::Propagation::Proceed
        }
    });

    // A window torn down without a delete event, which is what happens when
    // the process is asked to quit, would otherwise lose its strip and its
    // collapse.
    window.connect_destroy({
        let cx = Rc::clone(cx);
        move |_| {
            close(&cx, ordinal);
            if live_window_count() == 0 {
                gtk::main_quit();
            }
        }
    });
}

/// Give an ordinal back and write everything this window owned.
///
/// Idempotent: a window that is closed by its own glyph reaches this from the
/// delete event and again from the destroy that follows.
fn close(cx: &Rc<Ctx>, ordinal: crate::WindowId) {
    save_geometry();
    save_window_state(&cx.shell);
    // A settings write is coalesced on a quiet timer, so a change made in the
    // last fraction of a second before a window goes away is still queued.
    // Nothing will run that timer once the process is gone.
    crate::ui::settings::flush();
    release_ordinal(ordinal);
    crate::drop_window_events(ordinal);
    // The ordinal is handed back above and the next window may claim it, so a
    // host left behind would be handed to that window.
    pane::PaneHost::forget(ordinal);
    // The launcher entry is driven from this process and has nothing to
    // re-read, so a count left behind by the last window stays on the launcher
    // after the process is gone and is wrong from that moment on.
    if live_window_count() == 0 {
        crate::badge::clear();
        boot::tally("prefs-loads", state::prefs_loads());
        boot::tally("mark-rasterisations", crate::chrome::mark_rasterisations());
    }
}

/// The shell's whole keyboard, on the toplevel.
///
/// One handler rather than one per surface, because a chord's scope is what
/// decides where it fires and the scope lives in the table. Handlers spread
/// over the sidebar, the notice and the dialogs is how a chord ends up live in
/// one of them and dead in the other two.
///
/// Connected to the toplevel's own key-press signal, which GTK emits before it
/// propagates the event to the focused widget. A claimed chord returns `Stop`,
/// so it never also reaches the surface underneath: Ctrl+Shift+N opening a
/// session and typing an N into the rename field it was pressed over is one
/// keystroke doing two things, and the second is never wanted.
fn wire_keyboard(cx: &Rc<Ctx>, window: &gtk::Window) {
    let cx = Rc::clone(cx);
    window.connect_key_press_event(move |window, event| {
        // A keyval with neither a character nor a name is not a chord. GTK
        // sends one for a bare modifier press, which happens before every
        // chord the table does hold.
        let Some(pressed) = keys::chord_from_gdk(event) else {
            return glib::Propagation::Proceed;
        };
        let focus = focus_of(window);
        let open = cx.peek(|st| st.window.layer.is_open());
        let Some(found) = keys::claim_live(&pressed, focus, open) else {
            return glib::Propagation::Proceed;
        };
        cx.shell.batch(|| match found {
            keys::Claim::Action(action) => keys::on_key(&cx, action),
            keys::Claim::Custom(chord) => keys::dispatch_custom(&cx, &chord),
        });
        glib::Propagation::Stop
    });
}

/// Which surface the keyboard is on.
///
/// Answered from the toolkit rather than from the model. A chord's scope is
/// resolved against this: bare arrows traverse the session list only once
/// focus is actually in the list, and Escape belongs to the agent unless
/// something is open over it. A handler that inferred focus from the state
/// would be guessing about something GTK already knows.
fn focus_of(window: &gtk::Window) -> keys::Focus {
    let Some(widget) = window.focused_widget() else {
        return keys::Focus::Shell;
    };
    if widget.is::<gtk::Entry>() || widget.is::<gtk::TextView>() {
        return keys::Focus::TextInput;
    }
    if widget.is::<gtk::DrawingArea>() {
        return keys::Focus::Terminal;
    }
    // A row wears `rg-session` on its root, so any ancestor carrying it means
    // focus is inside the list however deep the control is.
    let mut at = Some(widget);
    while let Some(w) = at {
        if w.style_context().has_class("rg-session") {
            return keys::Focus::SessionList;
        }
        at = w.parent();
    }
    keys::Focus::Shell
}

/// One tray for the process, owned by the first window.
///
/// The handle is thread-affine: a macOS status item is main-thread only and
/// the Windows tray is pumped by the thread that made it.
fn wire_tray(cx: &Rc<Ctx>, window: &gtk::Window) {
    let Some(handle) = tray::install() else {
        return;
    };
    // The icon follows the same number the OS badge does, and it is pushed
    // when the count changes rather than polled: `set_attention` returns
    // immediately on an unchanged count, which matters on Linux where a push
    // re-emits the whole menu.
    cx.shell.observe(Attention::new(handle.clone()));

    let cx = Rc::clone(cx);
    let window = window.clone();
    glib::MainContext::default().spawn_local(async move {
        let mut visible = true;
        loop {
            match tray::next_command().await {
                vitrum_os::tray::TrayCommand::ToggleWindow => {
                    visible = !visible;
                    window.set_visible(visible);
                    if visible {
                        window.present();
                    }
                    handle.set_window_visible(visible);
                }
                vitrum_os::tray::TrayCommand::NewSession => {
                    if !visible {
                        visible = true;
                        window.set_visible(true);
                        handle.set_window_visible(true);
                    }
                    window.present();
                    cx.shell.batch(|| crate::actions::open_new_session(&cx, None));
                }
                vitrum_os::tray::TrayCommand::Quit => {
                    // Take the icon down first. A host keeps showing a
                    // StatusNotifierItem until its bus name goes away, so
                    // quitting without this leaves a dead icon.
                    handle.shutdown();
                    save_geometry();
                    save_window_state(&cx.shell);
                    crate::ui::settings::flush();
                    window.close();
                }
            }
        }
    });
}

/// Keeps the tray icon's attention mark on the number the badge shows.
struct Attention {
    handle: tray::Handle,
    last: Cell<usize>,
}

impl Attention {
    fn new(handle: tray::Handle) -> Rc<dyn super::Observer> {
        Rc::new(Self {
            handle,
            last: Cell::new(usize::MAX),
        })
    }
}

impl super::Observer for Attention {
    fn state_changed(&self, state: &UiState, at: Tick) {
        let total = state.daemon.attention_total(at.model);
        if self.last.replace(total) == total {
            return;
        }
        self.handle.set_attention(total);
    }
}

// ---------------------------------------------------------------------------
// Pumps
// ---------------------------------------------------------------------------

/// Everything the socket has to say, on the UI thread.
///
/// A second pump beside the pane's rather than a select over both. They are
/// independent sources with independent lifetimes: the pane lives as long as
/// the window and a socket is replaced on every reconnect, and joining them
/// would make a dead socket able to stall a keystroke.
fn pump_socket(cx: &Rc<Ctx>, mut rx: UnboundedReceiver<(u64, socket::SocketEvent)>) {
    let cx = Rc::clone(cx);
    glib::MainContext::default().spawn_local(async move {
        while let Some((epoch, event)) = rx.recv().await {
            // A superseded socket's dying words must not overwrite the live
            // one's state.
            if epoch != cx.bridge.epoch() {
                continue;
            }
            cx.shell.batch(|| {
                sync::on_socket_event(&cx, event);
                sync::claim_link(&cx);
                sync::claim_launch(&cx);
            });
        }
    });
}

/// Everything the window itself observed: a keystroke the pane captured, a
/// chord, a resize, a page-back gesture, the result of a copy, and every
/// request a panel raised.
fn pump_client(cx: &Rc<Ctx>, mut rx: UnboundedReceiver<ClientEvent>) {
    let cx = Rc::clone(cx);
    glib::MainContext::default().spawn_local(async move {
        while let Some(event) = rx.recv().await {
            cx.shell.batch(|| {
                sync::on_client_event(&cx, event);
                sync::claim_link(&cx);
                sync::claim_launch(&cx);
            });
        }
    });
}

/// Restore the profile, then dial the daemon.
///
/// Preferences before anything else, so the first connection uses the daemon
/// URL the operator chose and the first paint uses their theme.
fn start_up(cx: &Rc<Ctx>) {
    let cx = Rc::clone(cx);
    glib::MainContext::default().spawn_local(async move {
        // The SNAPSHOT, not a fresh read. The prewarm thread already parsed
        // this file and the window was created from it; a second read here
        // would be a second parse of a file nothing has written since, on the
        // path to the first paint.
        let (prefs, why) = state::startup_prefs();
        let ordinal = cx.shell.ident().ordinal;
        cx.edit(|st| {
            prefs.restore_daemon(&mut st.daemon);
            // Window N restores window N's strip. A window is the only thing
            // that knows which slot it is, and every window defaulting to 0
            // makes them fight over one entry and lose the rest.
            st.window.index = ordinal;
            prefs.restore_window(&mut st.window);
        });
        // Two files, one strip, and the launch store goes second because the
        // settings one is the wider failure. Neither is silent: a state file
        // this build could not read is a thing the operator has to be told
        // about, or the first save writes defaults over it and the loss is
        // permanent.
        let problem = why
            .as_deref()
            .map(|detail| format!("Settings not fully restored: {detail}"))
            .or_else(|| launch::store_problem().map(str::to_string));
        if let Some(text) = problem {
            cx.flash(Flash::error(text));
        }
        // The pane reads its font, palette, cursor and scrollback off the live
        // bus rather than being pushed at. Published once here so a pane
        // created before the profile was restored picks up the operator's
        // values on its next frame.
        cx.peek(|st| state::live::publish(&st.daemon.settings));
        // The saved commands, for the same reason and in the same breath. The
        // chord table is folded from both halves, so publishing one without
        // the other leaves every preset shortcut dead until the operator
        // happens to edit the list.
        state::live::publish_presets(&launch::presets_saved());
        boot::mark("settings.restored");

        // First run gets the walkthrough, an upgrade gets the release notes,
        // and a window that is neither gets neither. Only the first window: a
        // second window is not a second first run.
        if ordinal == 0 {
            let onboarded = cx.peek(|st| st.daemon.settings.onboarded);
            if !onboarded {
                cx.edit(|st| st.window.layer = crate::state::Layer::Onboarding);
            } else {
                let seen = cx.peek(|st| st.daemon.settings.last_seen_version());
                if !crate::ui::whatsnew::whats_new(seen.as_ref()).is_empty() {
                    cx.edit(|st| st.window.layer = crate::state::Layer::WhatsNew);
                }
            }
        }

        if cx.opts.fixture {
            let now = crate::tick().now_ms;
            let first = cx.edit(|st| {
                st.daemon.conn = crate::state::ConnState::Fixture;
                st.daemon.projects = fixture::projects();
                st.daemon.sessions = fixture::sessions(now);
                // The same placement a live `Sessions` snapshot gets inside
                // `DaemonState::apply`. Without it the fixture has a different
                // state machine from the real thing: nothing is ever filed
                // into a workspace, and every session follows the operator
                // into whichever workspace they just created.
                let infos: Vec<vitrum_proto::SessionInfo> =
                    st.daemon.sessions.iter().map(|row| row.info.clone()).collect();
                st.daemon.workspaces.adopt(infos.iter());
                st.daemon.sessions.first().map(|row| row.id())
            });
            if let Some(id) = first {
                cx.edit(|st| st.open(id, now));
            }
            sync::reconcile(&cx);
        } else {
            // The Advanced tab may point somewhere other than the command
            // line. `resolved_daemon_url` falls back to `--server` when the
            // setting is blank, so the flag still wins on a fresh profile.
            let url = cx.peek(|st| {
                st.daemon
                    .settings
                    .resolved_daemon_url(cx.opts.server)
                    .to_string()
            });
            boot::mark("daemon.dialled");
            crate::actions::start_daemon_then_connect(&cx, &url).await;
        }
    });
}

/// Report what a background profile write did, once per outcome.
///
/// A background write has nowhere to return a failure to, so it records the
/// outcome and this reads it. Without this a profile that cannot be written is
/// lost in silence: the operator arranges a window, the write fails, and the
/// arrangement is gone at the next launch with nothing on screen having said
/// so.
fn watch_saves(cx: &Rc<Ctx>) {
    let cx = Rc::clone(cx);
    glib::MainContext::default().spawn_local(async move {
        const WATCH: std::time::Duration = std::time::Duration::from_secs(2);
        let mut seen = state::SaveReport::default();
        loop {
            tokio::time::sleep(WATCH).await;
            let now = state::save_report();
            if now == seen {
                continue;
            }
            if now.error != seen.error
                && let Some(why) = now.error.as_deref()
            {
                cx.flash(Flash::error(format!("Profile not saved: {why}")));
            } else if now.archived != seen.archived
                && let Some(path) = now.archived.as_deref()
            {
                cx.flash(Flash::notice(format!(
                    "The profile could not be read and was moved to {path}"
                )));
            }
            seen = now;
        }
    });
}

/// The quiet update check behind the titlebar chip.
///
/// After first paint, then every [`crate::update::check_interval`]. A round
/// trip to a release host must not lengthen the path to a usable window, and a
/// fixture has no network story to tell.
fn watch_updates(cx: &Rc<Ctx>) {
    let cx = Rc::clone(cx);
    glib::MainContext::default().spawn_local(async move {
        let forced = std::env::var_os("VITRUM_UPDATE_OFFER").is_some();
        if cx.opts.fixture && !forced {
            return;
        }
        // A forced offer is for screenshots and demos; paint it on the first
        // tick. A real check waits so it cannot compete with the first paint
        // or the daemon connect for the network.
        let delay = if forced {
            std::time::Duration::from_millis(50)
        } else {
            std::time::Duration::from_secs(2)
        };
        tokio::time::sleep(delay).await;
        loop {
            let channel = cx.peek(|st| st.daemon.settings.update_channel);
            // Silence, not an error, when the network is unreachable: the
            // operator did not ask for this check and must not be handed its
            // failure.
            let got = if forced {
                instance::off_thread(crate::update::quiet_check).await.ok()
            } else {
                instance::off_thread(move || crate::update::background_check(channel)).await
            };
            if let Some(status) = got {
                let ignored = cx.peek(|st| st.daemon.settings.ignored_update.clone());
                cx.shell
                    .set_update_offer(crate::update::chrome_offer(&status, &ignored));
            }
            if forced {
                return;
            }
            let hours = cx.peek(|st| st.daemon.settings.update_check_hours);
            tokio::time::sleep(crate::update::check_interval(hours)).await;
        }
    });
}

/// Handoffs from later launches.
///
/// Every window parks here; the queue hands each activation to exactly one of
/// them, and if that window closes the next one along takes over without
/// losing anything already queued.
fn watch_activations(opts: Options) {
    glib::MainContext::default().spawn_local(async move {
        loop {
            let activation = instance::ACTIVATIONS.next().await;
            tracing::info!("second launch asked for a window: {activation:?}");
            open(opts, activation.link());
        }
    });
}
