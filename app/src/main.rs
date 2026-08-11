//! The vitrum desktop client: sidebar, workspace bar, and a native pane.
//!
//! Shape of the process:
//!
//! - Rust owns all UI state, in exactly one [`UiState`] signal per window,
//!   encodes every control-plane message, and owns the session WebSocket:
//!   connect, reconnect, sequence continuity, the backlog splice and the
//!   reassembly of a character split across two frames all live in [`socket`].
//! - The pane is a GTK drawing area with its own X window and a wgpu
//!   swapchain on it. PTY bytes go from the socket task to libghostty's VT in
//!   `vitrum-vt`, into a `vitrum-grid` cell grid, and onto that surface. They
//!   are never copied into a document, never re-parsed, and never encoded.
//! - Scrollback lives on the server. This process holds one grid per window
//!   and nothing else, so its memory is flat whether the operator runs one
//!   agent or twenty.
//!
//! Idle cost is a design constraint, not a nice-to-have. There is no timer, no
//! polling loop and no animation anywhere in this program. Every wakeup at rest
//! traces back to a socket message, an input event, or a keypress.
//!
//! Two things schedule work, both one-shot and both bounded, and neither runs
//! while the window is doing its job: a transient notice retires itself after
//! the operator's configured life, and a window whose socket closed reconnects
//! on the schedule in [`reconnect_delay_ms`]. A connected window has neither
//! outstanding, which is what keeps the claim above true where it matters.

mod actions;
mod agent;
mod badge;
mod boot;
mod chrome;
mod cli;
mod clock;
mod fixture;
mod geometry;
mod hint;
mod icons;
mod inbox;
mod instance;
mod keymap;
mod keys;
mod launch;
// The pane. Not behind a feature and not behind a target: its geometry,
// selection, scrolling, search, palette and pacing are plain Rust that
// compiles and is tested everywhere, and only the GTK host inside it is
// Linux-only. Gating the module would take the pure logic and its tests off
// every other platform to no purpose.
mod pane;
// The window. The toplevel, the widget tree in it, and the contracts every
// panel is mounted through.
mod shell;
mod socket;
mod splash;
mod state;
mod sync;
mod termpalette;
#[cfg(test)]
mod testkit;
mod tray;
mod ui;
mod update;
mod wire;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use tokio::sync::mpsc::UnboundedSender;
use vitrum_fmt::TimeFormat;
use vitrum_model::Direction;
use vitrum_os::AppPaths;
use vitrum_os::deeplink::DeepLink;
use vitrum_os::single_instance::{self, Acquisition, Activation, InstanceGuard};
// `WindowGeometry` because `crate::state::WindowState` is a different thing
// entirely: this one is a rectangle on a desktop, that one is what a window is
// showing. Importing both under one name is a bug waiting for a hurried edit.
use vitrum_os::window_state::{self, Monitor, WindowState as WindowGeometry};
use vitrum_proto::{ClientMsg, PROTOCOL_VERSION, ProjectId, SessionId};

use actions::*;
use chrome::*;
use cli::*;
use geometry::*;
use instance::*;
use keymap::KeyAction;
use keys::*;
use shell::run::Ctx;
use state::{ConnState, Flash, Layer, NewSessionSeed, Reaction, RenameSeed, UiState};
#[cfg(test)]
use state::{SIDEBAR_MAX_PX, SIDEBAR_MIN_PX};
use sync::*;
use wire::{BEFORE_SEQ_HEAD, ClientEvent, ConnEvent, backfill_max_bytes};

fn main() {
    // First, before anything else can take time: every later mark is a delta
    // from this one, and a zero taken after the logging subscriber is built
    // hides the subscriber.
    boot::arm();
    boot::mark("process.start");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("VITRUM_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,vitrum=info")),
        )
        .init();
    boot::mark("logging.ready");

    // An update staged by an earlier run is applied here: before the window,
    // before the daemon is dialled, and before any subcommand, because this is
    // the only moment at which nothing yet depends on which build is on disk.
    // It also sweeps the image a previous update on Windows could not delete,
    // the process holding it open having been the one doing the replacing.
    //
    // The daemon is not restarted. It keeps running the old code until the
    // operator restarts it, which ends every session it holds.
    update::apply_on_start();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // Before the option parser, because these are subcommands and not flags:
    // each does its work and exits without ever opening a window, and the
    // parser below exists to configure a window.
    if args.first().map(String::as_str) == Some("update") {
        std::process::exit(update::run_update(&args[1..]));
    }
    // `hint` writes one escape sequence to stdout. An agent wrapper or a shell
    // prompt calls it, so it has to stay cheap and silent: nothing is read,
    // nothing is logged, and no window is touched.
    if args.first().map(String::as_str) == Some("hint") {
        std::process::exit(hint::run_hint(&args[1..]));
    }
    // `icons` writes the platform icon set from the mark's geometry. The
    // installer runs it after moving the binary into place, because the
    // release archive carries two executables and nothing else: an installed
    // copy has no icon files to unpack and no toolchain to build them with.
    if args.first().map(String::as_str) == Some("icons") {
        std::process::exit(icons::run_icons(&args[1..]));
    }
    // A command line this program cannot act on is a failure, and it exits
    // like one. `--help` and `--version` travel the same channel and are not:
    // `CliExit` carries which, and it is the only thing here that chooses a
    // stream or a code.
    let opts = match Options::parse(args.iter().cloned()) {
        Ok(o) => o,
        Err(told) => std::process::exit(told.report()),
    };

    // One process, N windows. A second launch is a request for another
    // window, not another copy of the program: it hands its intent to the
    // instance holding the lock and exits, and that instance opens the window.
    // Twenty windows share one engine and one set of mapped pages; twenty
    // processes would each pay for their own.
    //
    // The guard is bound for the length of `main`. `launch` never returns, so
    // this is a statement of lifetime rather than a drop that will run.
    let activation = Activation::from_args(&args);
    let _instance = if opts.standalone {
        tracing::info!("running standalone; a second launch will not reach this process");
        Instance::Alone
    } else {
        match claim_instance(&activation) {
            Instance::Second => {
                tracing::info!("handed off to the running instance: {activation:?}");
                return;
            }
            held => held,
        }
    };
    boot::mark("instance.claimed");

    // A profile that has never had a launch store gets one, with a preset per
    // agent vitrum knows. Before the prewarm below, which READS that store:
    // seeding after it would leave the first launcher of a new profile empty
    // and only fill it on the second start.
    //
    // One `stat` on every other start, which is what "has this profile been
    // used" costs. The `PATH` walk behind the roster happens only when the
    // file is absent, so it is paid once in the life of a profile and never
    // on the path this program is measured on.
    launch::seed_launch_store_once();

    // The startup profile and the window mark, read and rasterised on a
    // thread of its own while this one brings the toolkit up.
    //
    // Neither needs a window, a display server or the main loop, and the
    // main thread is busy with all three. Both are one-shot caches, so
    // whichever thread arrives second takes the finished value and a prewarm
    // that never ran costs nothing but the overlap.
    //
    // Safe with respect to the environment the parser read above: nothing in
    // the process writes the environment after this point.
    if let Err(e) = std::thread::Builder::new()
        .name("vitrum-prewarm".to_string())
        .spawn(move || {
            let _ = state::startup_prefs();
            warm_window_icon();
        })
    {
        tracing::warn!("no prewarm thread, reading the profile inline: {e}");
    }

    // Never returns. The toolkit, the monitor probe, the window and the main
    // loop are all on the other side of this call.
    shell::run::launch(opts, activation.link());
}

/// Where a window's own events wait for whatever will read them.
///
/// The pane is installed before the panels are mounted, so the closure that
/// receives a keystroke exists before the reducer that will act on it. The
/// window's own bring-up in [`shell::run`] holds the receiving half for the
/// life of the window; this map holds the sending half, so code with no
/// handle on the window can still reach its queue.
///
/// Keyed by window ordinal because this process opens more than one window,
/// and an unkeyed queue would let window two's keystrokes be read by window
/// one. Thread-local because every one of these is created and consumed on
/// the UI thread, so no lock is bought for a hazard that cannot arise.
///
/// Which window, for everything in this process that has to name one: the
/// ordinal the window was opened under, claimed by `claim_ordinal` and given
/// back when the window closes. It is the identity geometry is remembered
/// under and the identity the pane registry is keyed by.
pub(crate) type WindowId = usize;

thread_local! {
    static WINDOW_EVENTS: RefCell<HashMap<WindowId, UnboundedSender<ClientEvent>>> =
        RefCell::new(HashMap::new());
}

/// Keep `tx` reachable by ordinal for as long as the window is open.
pub(crate) fn hold_window_events(id: WindowId, tx: UnboundedSender<ClientEvent>) {
    WINDOW_EVENTS.with(|map| map.borrow_mut().insert(id, tx));
}

/// A window's event queue, for a caller that has only its ordinal.
///
/// The pane's key handler is that caller. It runs in a toolkit callback with
/// no shell in scope, and this is the one thing it needs from the window:
/// somewhere to put a chord it is not going to encode.
pub(crate) fn window_sender(id: WindowId) -> Option<UnboundedSender<ClientEvent>> {
    WINDOW_EVENTS.with(|map| map.borrow().get(&id).cloned())
}

/// Forget a window's channel when the window goes away.
pub(crate) fn drop_window_events(id: WindowId) {
    WINDOW_EVENTS.with(|map| map.borrow_mut().remove(&id));
}

/// Hand `text` to the desktop's clipboard.
///
/// GTK owns the selection for as long as this process lives, which is what
/// the operator expects from an application window: the text stays pasteable
/// until they copy something else. It does NOT survive the process, because
/// X11 has no clipboard daemon of its own and a manager, if one is running,
/// takes over at exit.
#[cfg(target_os = "linux")]
fn set_clipboard(text: &str) -> bool {
    let Some(display) = gtk::gdk::Display::default() else {
        tracing::warn!("no display, so nothing to copy to");
        return false;
    };
    let clipboard = gtk::Clipboard::default(&display);
    let Some(clipboard) = clipboard else {
        tracing::warn!("this display offers no clipboard selection");
        return false;
    };
    clipboard.set_text(text);
    true
}

/// Platforms whose clipboard is not wired yet say so rather than claiming a
/// copy that did not happen.
#[cfg(not(target_os = "linux"))]
fn set_clipboard(_text: &str) -> bool {
    false
}

/// One reading of the wall clock, in the three forms the shell needs.
///
/// The invariant this protects is that a single paint, or a single user
/// action, takes exactly ONE reading. Two readings inside a loop over twenty
/// rows would pay twenty syscalls per paint; two readings straddling a
/// threshold would disagree about whether the same instant is "59s ago" or
/// "1m ago", and would disagree about whether a snooze has elapsed.
///
/// This is the only place in the program that reads the system clock for the
/// UI, which is what `clock::tests::the_clock_has_exactly_one_literal_call_site`
/// checks. Handlers take a fresh `Tick` rather than closing over the render
/// tick, because a handler fires long after the paint that built it and a
/// stale clock would offer a snooze preset for a time already past.
#[derive(Clone, Copy)]
pub(crate) struct Tick {
    /// For rendering relative times.
    pub(crate) fmt: TimeFormat,
    /// For every derivation in `vitrum-model`.
    pub(crate) model: vitrum_model::Clock,
    /// For stamping visits and snoozes.
    pub(crate) now_ms: u64,
}

pub(crate) fn tick() -> Tick {
    let fmt = clock::now();
    Tick {
        model: inbox::model_clock(fmt),
        // Derived from the same reading rather than a second syscall, so the
        // three forms can never disagree by the time between them.
        now_ms: fmt.now().as_millis().max(0) as u64,
        fmt,
    }
}

#[cfg(test)]
mod tests;
