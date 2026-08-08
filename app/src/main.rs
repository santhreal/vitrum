//! The vitrum desktop client: sidebar, tab strip, and a real terminal pane.
//!
//! Shape of the process:
//!
//! - Rust owns all UI state, in exactly one [`UiState`] signal, and encodes
//!   every control-plane message.
//! - JavaScript (`bootstrap.js`) owns the WebSocket and the xterm.js instance.
//!   PTY bytes never cross into Rust: they go from the socket into the terminal
//!   with one header strip and no trip through an IPC channel.
//! - Scrollback lives on the server. This process holds one terminal grid and
//!   nothing else, so its memory is flat whether the user runs one agent or
//!   twenty.
//!
//! Idle cost is a design constraint, not a nice-to-have. There is no timer, no
//! polling loop and no animation anywhere in this program. Every wakeup at rest
//! traces back to a socket message, a DOM event, or a keypress.
//!
//! Two things schedule work, both one-shot and both bounded, and neither runs
//! while the window is doing its job: a transient notice retires itself after
//! [`NOTICE_MS`], and a window whose socket closed reconnects on the schedule
//! in [`reconnect_delay_ms`]. A CONNECTED window has neither outstanding, which
//! is what keeps the claim above true where it matters.

mod actions;
mod agent;
mod badge;
mod chrome;
mod cli;
mod clock;
mod fixture;
mod geometry;
mod hint;
mod inbox;
mod instance;
mod keymap;
mod keys;
mod launch;
mod state;
mod sync;
mod termpalette;
#[cfg(test)]
mod testkit;
mod tray;
mod ui;
mod update;
mod wire;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll, Waker};

use dioxus::document::Eval;
use dioxus::prelude::*;
use vitrum_dioxus_desktop::tao::dpi::{PhysicalPosition, PhysicalSize};
use vitrum_dioxus_desktop::tao::event::{Event as WryEvent, WindowEvent};
use vitrum_dioxus_desktop::tao::event_loop::EventLoopBuilder;
use vitrum_dioxus_desktop::tao::monitor::MonitorHandle;
use vitrum_dioxus_desktop::tao::window::{Window, WindowBuilder};
use vitrum_dioxus_desktop::{Config, DesktopContext, WindowCloseBehaviour, use_wry_event_handler};
use vitrum_fmt::TimeFormat;
use vitrum_os::AppPaths;
use vitrum_os::deeplink::DeepLink;
use vitrum_os::single_instance::{self, Acquisition, Activation, InstanceGuard};
// `WindowGeometry` because `crate::state::WindowState` is a different thing
// entirely: this one is a rectangle on a desktop, that one is what a window is
// showing. Importing both under one name is a bug waiting for a hurried edit.
use vitrum_model::{Direction, Section};
use vitrum_os::window_state::{self, Monitor, WindowState as WindowGeometry};
use vitrum_proto::{ClientMsg, PROTOCOL_VERSION, ProjectId, SessionId};

use actions::*;
use chrome::*;
use cli::*;
use geometry::*;
use instance::*;
use keymap::KeyAction;
use keys::*;
use state::{
    ConnState, Flash, GroupKey, Layer, MenuAction, MenuState, NewSessionSeed, Reaction, RenameSeed,
    SIDEBAR_MAX_PX, SIDEBAR_MIN_PX, UiState,
};
use sync::*;
use wire::{BEFORE_SEQ_HEAD, BridgeCmd, BridgeEvent, ConnEvent, backfill_max_bytes};

/// How long a transient notice stays on screen, in milliseconds.
///
/// Long enough to read a sentence and its shortcut without hurrying, short
/// enough that it is gone before it becomes furniture. Errors ignore this
/// entirely: they are not transient.
const NOTICE_MS: u64 = 6_000;

/// First reconnect delay, in milliseconds. Doubles per attempt.
const RECONNECT_BASE_MS: u64 = 250;
/// Ceiling on the reconnect delay. A machine asleep for a week must not spend
/// the night dialling.
const RECONNECT_MAX_MS: u64 = 30_000;
/// How many times to try before the window goes back to saying "failed" and
/// waiting for Retry. Roughly ten minutes of trying at the ceiling.
const RECONNECT_ATTEMPTS: u32 = 25;

/// The JS half. Sent through one long-lived `eval` that never returns.
const BOOTSTRAP_JS: &str = include_str!("bootstrap.js");

/// Vendored xterm.js and its addons, inlined into the document head so they are
/// parsed before the app's first render. No CDN, no network at startup.
const XTERM_JS: &str = include_str!("vendor/xterm.js");
const XTERM_CSS: &str = include_str!("vendor/xterm.css");
const ADDON_WEBGL_JS: &str = include_str!("vendor/addon-webgl.js");
const ADDON_FIT_JS: &str = include_str!("vendor/addon-fit.js");

/// Sidebar styling and the shared `--rg-*` design tokens.
const SIDEBAR_CSS: &str = include_str!("../assets/sidebar.css");
/// Settings sheet and workspace bar styling. Loaded after the sidebar so it
/// can lean on the tokens that file declares.
const SETTINGS_CSS: &str = include_str!("../assets/settings.css");
/// Window frame, tab strip, and terminal pane styling.
const APP_CSS: &str = include_str!("app.css");

/// The design-system layer, loaded LAST so it overrides by cascade order
/// rather than by specificity, and so no part needs `!important`.
///
/// Each file is owned by exactly one author. Two agents editing one stylesheet
/// is what produced the composition this layer exists to repair, so ownership
/// is enforced by the file boundary and not by convention. The numeric prefix
/// IS the cascade: a later part may override an earlier one, and each author
/// wrote against that guarantee.
const PART_SPACING_CSS: &str = include_str!("../assets/parts/10-spacing.css");
const PART_TYPE_CSS: &str = include_str!("../assets/parts/11-type.css");
const PART_COLOR_CSS: &str = include_str!("../assets/parts/12-color.css");
const PART_EMPTY_CSS: &str = include_str!("../assets/parts/13-empty.css");
const PART_CHROME_CSS: &str = include_str!("../assets/parts/14-chrome.css");
const PART_ROWS_CSS: &str = include_str!("../assets/parts/15-rows.css");
const PART_CONTROLS_CSS: &str = include_str!("../assets/parts/16-controls.css");
const PART_MOTION_CSS: &str = include_str!("../assets/parts/17-motion.css");
const PART_DIALOG_CSS: &str = include_str!("../assets/parts/18-dialog.css");
const PART_SETTINGS_CSS: &str = include_str!("../assets/parts/19-settings.css");
const PART_AGENT_MARKS_CSS: &str = include_str!("../assets/parts/20-agent-marks.css");
const PART_SEARCH_CSS: &str = include_str!("../assets/parts/21-search.css");
const PART_LAUNCHER_CSS: &str = include_str!("../assets/parts/22-launcher.css");
const PART_BACKDROP_CSS: &str = include_str!("../assets/parts/23-backdrop.css");

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("VITRUM_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,vitrum=info")),
        )
        .init();

    // WebKitGTK's MemoryPressureMonitor parks a thread reading
    // `/proc/self/cgroup` one byte at a time, 178 read syscalls a second, for
    // as long as the process lives. Measured on this machine at idle with
    // nothing on screen changing. This program's whole claim is that an idle
    // window does no work, and it has to be set before the first webview is
    // built because WebKit reads it once at process init.
    //
    // Safety: single-threaded here. `main` has not started the event loop, so
    // no other thread exists to observe the environment mid-write.
    #[cfg(target_os = "linux")]
    unsafe {
        std::env::set_var("WEBKIT_DISABLE_MEMORY_PRESSURE_MONITOR", "1")
    };

    // A previous update on Windows could not delete the image it replaced,
    // because that image was the process doing the replacing. It has exited by
    // now, so this is the first moment the file can go.
    if let Ok(dir) = update::install_dir() {
        update::sweep_displaced(&dir);
    }

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
    let opts = match Options::parse(args.iter().cloned()) {
        Ok(o) => o,
        Err(msg) => {
            println!("{msg}");
            return;
        }
    };

    // One process, N windows. A second launch is a request for another
    // window, not another copy of the program: it hands its intent to the
    // instance holding the lock and exits, and that instance opens the window.
    // Twenty windows share one WebKit engine and one set of mapped pages;
    // twenty processes would each pay for their own.
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

    // The event loop is built here rather than left to the launcher because
    // the first window's size and scale depend on the monitor it will open on,
    // and there is no monitor list until the loop exists. Opening at a nominal
    // 1280x800 and correcting after the first paint is a visible flash of
    // half-size UI on exactly the machine this feature is for.
    let event_loop = EventLoopBuilder::with_user_event().build();
    let monitors: Vec<MonitorHandle> = event_loop.available_monitors().collect();
    let primary = event_loop
        .primary_monitor()
        .or_else(|| monitors.first().cloned());
    seed_book(load_geometry(&monitor_rects(primary.as_ref(), &monitors)));

    let density = primary.as_ref().map(density_of);
    let scale = opts
        .ui_scale
        .unwrap_or_else(|| density.map_or(MIN_UI_SCALE, Density::ui_scale));
    let os_scale = density.map_or(1.0, |d| d.os_scale).max(1.0);
    for monitor in &monitors {
        let d = density_of(monitor);
        tracing::info!(
            "monitor {:?}: {}x{} device px, {}x{} mm, {} dpi, toolkit scale {}, ui scale {}",
            monitor.name().unwrap_or_default(),
            d.width_px,
            d.height_px,
            d.width_mm,
            d.height_mm,
            d.dpi().map_or(-1.0, |v| (v * 10.0).round() / 10.0),
            d.os_scale,
            d.ui_scale()
        );
    }

    let ordinal = claim_ordinal();
    let state =
        remembered(ordinal).unwrap_or_else(|| fresh_geometry(primary.as_ref(), scale, ordinal));
    remember(ordinal, state);

    let config = window_config(opts, &state, scale, os_scale).with_event_loop(event_loop);

    let seed = WindowSeed {
        ordinal,
        link: activation.link(),
    };

    // `LaunchBuilder::desktop()` would go through the dioxus facade, which
    // resolves the registry renderer. This is the same call one level down,
    // against the fork.
    vitrum_dioxus_desktop::launch::launch(
        App,
        vec![
            Box::new(move || Box::new(opts) as Box<dyn std::any::Any>),
            Box::new(move || Box::new(seed) as Box<dyn std::any::Any>),
        ],
        vec![Box::new(config)],
    );
}

/// Apply this platform's frameless-window configuration.
///
/// Linux and Windows drop the decoration entirely and
/// [`ui::titlebar::TitleBar`] draws the replacement, controls included.
#[cfg(not(target_os = "macos"))]
fn decorate(window: WindowBuilder) -> WindowBuilder {
    window.with_decorations(false)
}

/// macOS keeps its decoration, with the titlebar made transparent and the
/// content view extended under it.
///
/// Dropping decorations on macOS takes the traffic lights with them, and
/// reimplementing those in HTML gives you three circles that look almost right
/// and do not respond to Mission Control, window tabbing, or a long press.
/// Extending the content view instead keeps the real buttons where macOS puts
/// them; [`ui::titlebar::MACOS_TRAFFIC_LIGHT_INSET`] reserves their space.
#[cfg(target_os = "macos")]
fn decorate(window: WindowBuilder) -> WindowBuilder {
    use vitrum_dioxus_desktop::tao::platform::macos::WindowBuilderExtMacOS;
    window
        .with_titlebar_transparent(true)
        .with_title_hidden(true)
        .with_fullsize_content_view(true)
}

/// Handle on the JavaScript bridge.
///
/// `Eval` is `Copy`, so this is a plain value that event handlers can capture
/// without cloning or reference counting.
#[derive(Clone, Copy)]
struct Bridge {
    eval: Eval,
}

impl Bridge {
    fn cmd(&self, c: BridgeCmd) {
        if let Err(e) = self.eval.send(&c) {
            tracing::error!("bridge command dropped: {e}");
        }
    }

    /// Encode and send a control-plane message.
    fn msg(&self, m: &ClientMsg) {
        self.cmd(BridgeCmd::Send {
            text: wire::encode(m),
        });
    }
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
struct Tick {
    /// For rendering relative times.
    fmt: TimeFormat,
    /// For every derivation in `vitrum-model`.
    model: vitrum_model::Clock,
    /// For stamping visits and snoozes.
    now_ms: u64,
}

fn tick() -> Tick {
    let fmt = clock::now();
    Tick {
        model: inbox::model_clock(fmt),
        // Derived from the same reading rather than a second syscall, so the
        // three forms can never disagree by the time between them.
        now_ms: fmt.now().as_millis().max(0) as u64,
        fmt,
    }
}

#[allow(non_snake_case)]
fn App() -> Element {
    let opts: Options = use_context();
    let seed: WindowSeed = use_context();
    let window = vitrum_dioxus_desktop::use_window();

    // Physical size, applied to the document as page zoom rather than as a
    // root font-size. Zoom is the stronger of the two because it scales every
    // length the page has, including the pixel values still left in `app.css`,
    // the xterm.js cell metrics, and one-pixel borders. A root font-size would
    // scale the sidebar's type and leave the tab strip, the terminal grid and
    // every rule at half size, which is a worse bug than the one it fixes
    // because it looks deliberate.
    //
    // Applied in the signal's initialiser so it lands before the first paint.
    let mut scale = use_signal({
        let window = window.clone();
        move || {
            let scale = window_ui_scale(&window, opts.ui_scale);
            window.set_zoom_level(scale);
            scale
        }
    });

    let mut st = use_signal({
        let window = window.clone();
        move || {
            let mut ui = UiState::default();
            // The slot this window persists into. Set at construction because
            // nothing else can know it, and every window defaulting to zero
            // means they all overwrite one entry.
            ui.window.index = seed.ordinal;
            // The sidebar's default is a fraction of the document, not a fixed
            // 256 px: a fixed default is a fifth of a 1280 px window and a
            // sixteenth of a 3840 px one. A width the user dragged in a
            // previous run wins over both, and `set_sidebar_width_in` caps
            // either at `state::SIDEBAR_MAX_FRACTION` of the window so a
            // remembered width from a maximised 4K session does not swallow a
            // laptop screen.
            //
            // The measurement here is provisional and known to be. tao has
            // been asked for the window's size but the platform may not have
            // applied it yet, so `inner_size` can still report the toolkit's
            // placeholder; measured on X11 with no window manager it reports
            // under 700 CSS pixels for a window that is about to be 2560,
            // and the fraction of that is below the legibility floor. The
            // first `Resized` re-derives it, which is where the real number
            // arrives.
            let css_width = css_viewport_width(&window, *scale.peek());
            let want = match remembered(seed.ordinal) {
                Some(state) if state.sidebar_width > 0 => f64::from(state.sidebar_width),
                _ => default_sidebar_width(css_width),
            };
            ui.window.set_sidebar_width_in(want, css_width);
            ui
        }
    });
    // Has the operator chosen this window's sidebar width, by dragging the
    // edge or nudging it with the keyboard?
    //
    // Until they have, the width is a FRACTION of the document and is
    // re-derived whenever the document changes size. After they have, it is
    // theirs and resizing the window only ever clamps it. Without the flag the
    // two rules fight: either a dragged width is silently thrown away on the
    // next resize, or a window that was measured before the platform applied
    // its geometry keeps a sidebar at the 224px floor forever, which is what
    // it did.
    let mut sidebar_pinned =
        use_signal(|| remembered(seed.ordinal).is_some_and(|state| state.sidebar_width > 0));
    // Session the server currently believes we are attached to. Kept separate
    // from `focused` so one reconciler drives every attach and detach,
    // including the ones caused by a reconnect rather than by a click.
    let mut attached = use_signal(|| None::<SessionId>);
    // (pointer x where the drag began, sidebar width at that moment)
    let mut drag = use_signal(|| None::<(f64, f64)>);
    // Rows armed for termination by a first press, waiting for the second.
    // Empty means nothing is armed, which is also what every dismissal
    // restores it to.
    let mut pending_terminate = use_signal(Vec::<SessionId>::new);
    // A session a `vitrum://session/N` handoff asked this window to open. Held
    // until the daemon confirms the session exists, because the link arrives
    // before the first snapshot does and acting on the first miss would drop
    // the request the user actually made.
    let pending_link = use_signal(|| match seed.link {
        Some(DeepLink::Session(id)) => Some(id),
        _ => None,
    });
    // The launch THIS window asked for, held until the daemon says it exists.
    // `SessionCreated` is broadcast to every window, so without a record of
    // who asked, a window cannot tell its own launch from another window's.
    let pending_open = use_signal(|| None::<PendingLaunch>);
    // How many reconnects have been tried since the last successful open.
    // Zero while connected, so a window at rest schedules nothing.
    let reconnect = use_signal(|| 0u32);
    // What the first-run sheet found on PATH. Empty until the sheet is about
    // to open, because resolving it walks every PATH entry and no window that
    // never shows onboarding should pay for that.
    let mut detected = use_signal(|| None::<Vec<launch::Detected>>);

    // Quiet update check for the titlebar chip. Separate from About's button
    // so a release can surface without the operator opening Settings first.
    //
    // A forced offer seeds the signal before the first paint so demos and
    // screenshots do not depend on a future tick racing the capture.
    let mut update_offer = use_signal(|| {
        if std::env::var_os("VITRUM_UPDATE_OFFER").is_none() {
            return None;
        }
        let seeded = match update::quiet_check() {
            Ok(status) => update::chrome_offer(&status, ""),
            Err(e) => {
                tracing::warn!("forced update offer failed: {e:#}");
                None
            }
        };
        tracing::info!(
            "forced update offer seed: {}",
            seeded
                .as_ref()
                .map(|a| a.version.to_string())
                .unwrap_or_else(|| "none".into())
        );
        seeded
    });

    let bridge = use_hook(|| Bridge {
        eval: document::eval(BOOTSTRAP_JS),
    });

    // Everything the window itself has to say. There is no timer behind any of
    // it: `Moved` arrives while the user drags, `Focused` when they switch
    // away, and nothing at all while the window sits still.
    use_wry_event_handler({
        let window = window.clone();
        move |event, _target| {
            // The window id is checked, and that is not tidiness. Every window
            // installs one of these handlers and every handler is called for
            // every window's events, so without the check each window measures
            // itself whenever any other window is moved, resized or closed.
            // With twenty windows open that is twenty geometry round trips to
            // the display server for one drag, nineteen of them answering for
            // a window that did not move.
            //
            // It also narrows the window in which a handler can ask X about a
            // drawable that is being destroyed. That is NOT known to fix the
            // open BadDrawable crash on closing one window of several: the
            // crash still reproduces with this guard in place, so do not read
            // it as the cure.
            let WryEvent::WindowEvent {
                window_id, event, ..
            } = event
            else {
                return;
            };
            if *window_id != window.id() {
                return;
            }
            match event {
                // A window dragged from the 82 dpi panel onto the 163 dpi one
                // has to be redrawn at the other panel's scale, or half the
                // point of measuring the scale at all is lost the first time
                // somebody moves a window.
                WindowEvent::Moved(_)
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. } => {
                    let next = window_ui_scale(&window, opts.ui_scale);
                    if next != *scale.peek() {
                        tracing::info!("window {} moved to a {next}x monitor", seed.ordinal);
                        window.set_zoom_level(next);
                        scale.set(next);
                    }
                    // The sidebar is a fraction of the document until the
                    // operator says otherwise, so a resize re-derives it. This
                    // is also the ONLY place the first honest measurement of
                    // the window arrives: `inner_size` at construction can
                    // still be the toolkit's placeholder, and a fraction of
                    // that lands on the 224px legibility floor and stays
                    // there for the life of the window.
                    //
                    // A width the operator chose is never re-derived, only
                    // clamped, which is what keeps a drag from being undone by
                    // the next resize.
                    let css = css_viewport_width(&window, next);
                    if *sidebar_pinned.peek() {
                        let have = st.peek().window.sidebar_width;
                        st.write().window.set_sidebar_width_in(have, css);
                    } else {
                        st.write()
                            .window
                            .set_sidebar_width_in(default_sidebar_width(css), css);
                    }
                    remember(seed.ordinal, measure(&window, seed.ordinal));
                }
                // Focus loss and a close request are the two moments a user has
                // finished moving a window. Writing on a pointer sample instead
                // would be a filesystem write per frame of a drag.
                //
                // `ui.json` is written here too, and that is not housekeeping.
                // `sidebar_collapsed` and the whole tab strip live in
                // `WindowSnapshot`, which `save_prefs` writes and `restore_window`
                // reads, but no collapse toggle and no tab operation ever called
                // `commit`. They survived only when some unrelated control
                // committed afterwards and happened to carry them along. Collapse
                // the sidebar, change nothing else, quit, and the collapse was
                // lost. These are the same two moments geometry already uses, so
                // the cost is one small write next to the one being made anyway.
                WindowEvent::Focused(false) | WindowEvent::CloseRequested => {
                    remember(seed.ordinal, measure(&window, seed.ordinal));
                    save_geometry();
                    save_window_state(st);
                }
                _ => {}
            }
        }
    });

    // Give the ordinal back when the window goes away, so the next window
    // opens where this one was rather than cascading past it forever. The UI
    // state goes with it: a window torn down without a `CloseRequested`, which
    // is what happens when the process is asked to quit, would otherwise lose
    // its strip and its collapse.
    use_drop(move || {
        release_ordinal(seed.ordinal);
        save_geometry();
        save_window_state(st);
        // The launcher entry is driven by a signal and has nothing to re-read,
        // so a count left behind by the last window stays on the launcher
        // after the process is gone and is wrong from that moment on.
        if live_window_count() == 0 {
            badge::clear();
        }
    });

    // Handoffs from later launches. Every window parks here; the queue hands
    // each activation to exactly one of them, and if that window closes the
    // next one along takes over without losing anything already queued.
    use_future({
        let window = window.clone();
        move || {
            let window = window.clone();
            async move {
                loop {
                    let activation = ACTIVATIONS.next().await;
                    tracing::info!("second launch asked for a window: {activation:?}");
                    open_window(&window, opts, activation.link());
                }
            }
        }
    });

    // The Windows taskbar overlay belongs to a window, not to a process, so
    // the badge backend cannot be built until one exists. `badge::publish`
    // runs from a static with no window in hand; registering the handle here
    // is what lets it find one. Idempotent, and the first window's button is
    // the one the operator sees.
    #[cfg(target_os = "windows")]
    use_hook(|| {
        use vitrum_dioxus_desktop::wry::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = window.window_handle()
            && let RawWindowHandle::Win32(win32) = handle.as_raw()
        {
            vitrum_os::badge::register_main_window(vitrum_os::badge::WindowHandle(isize::from(
                win32.hwnd,
            )
                as u64));
        }
    });

    // One tray for the process, owned by the first window because the handle
    // is thread-affine: a macOS status item is main-thread only and the
    // Windows tray is pumped by the thread that made it.
    let tray = use_hook(|| {
        if seed.ordinal == 0 {
            tray::install()
        } else {
            None
        }
    });

    // The icon follows the same number the OS badge does. An effect and not a
    // poll: it re-runs when the state it read actually changes, and
    // `set_attention` returns immediately on an unchanged count, which matters
    // on Linux where a push re-emits the whole menu.
    {
        let tray = tray.clone();
        use_effect(move || {
            if let Some(t) = tray.as_ref() {
                t.set_attention(st.read().daemon.attention_total(tick().model));
            }
        });
    }

    // Menu picks arrive on the tray's own thread and are read here.
    use_future({
        let window = window.clone();
        let tray = tray.clone();
        move || {
            let window = window.clone();
            let tray = tray.clone();
            async move {
                // A window with no tray must not park in this loop: nothing
                // will ever post to it.
                let Some(handle) = tray else {
                    return;
                };
                let mut visible = true;
                loop {
                    match tray::next_command().await {
                        vitrum_os::tray::TrayCommand::ToggleWindow => {
                            visible = !visible;
                            window.set_visible(visible);
                            if visible {
                                window.set_focus();
                            }
                            handle.set_window_visible(visible);
                        }
                        vitrum_os::tray::TrayCommand::NewSession => {
                            if !visible {
                                visible = true;
                                window.set_visible(true);
                                handle.set_window_visible(true);
                            }
                            window.set_focus();
                            open_new_session(st, None);
                        }
                        vitrum_os::tray::TrayCommand::Quit => {
                            // Take the icon down first. A host keeps showing a
                            // StatusNotifierItem until its bus name goes away,
                            // so quitting without this leaves a dead icon.
                            handle.shutdown();
                            save_geometry();
                            save_window_state(st);
                            window.close();
                        }
                    }
                }
            }
        }
    });

    // One long-lived reader. Not a poll: it parks on the channel until the
    // bridge has something to say.
    // Every window runs the settings script the sheet broadcasts, not just the
    // window the sheet was open in. Subscribed here, at the top of the root
    // component, so the subscription lasts exactly as long as this window.
    ui::settings::use_live_settings();

    // The window, for the one measurement this future takes. Cloned rather
    // than captured by reference because the future outlives this render.
    let settle_window = window.clone();
    use_future(move || {
        let settle_window = settle_window.clone();
        async move {
            // Preferences before anything else, so the first connection uses the
            // daemon URL the user chose and the first paint uses their theme.
            //
            // This runs after the sidebar width was seeded from window geometry,
            // and deliberately overwrites it: the geometry file is the fallback for
            // a window nobody has expressed a preference about, and `ui.json` is
            // the preference. Two files can only disagree if one of them is not
            // authoritative, so this decides which.
            let (prefs, why) = state::load_prefs();
            {
                let mut guard = st.write();
                let w = &mut *guard;
                prefs.restore_daemon(&mut w.daemon);
                // Window N restores window N's strip. The slot is set here rather
                // than left at its default, because a window is the only thing
                // that knows which slot it is and every window defaulting to 0
                // makes them fight over one entry and lose the rest.
                w.window.index = seed.ordinal;
                prefs.restore_window(&mut w.window);
            }
            if let Some(detail) = why {
                st.write().window.flash = Some(Flash::error(format!(
                    "Settings not fully restored: {detail}"
                )));
            }
            // Pushes the restored text scale, terminal options and key bindings
            // into the webview that just mounted.
            ui::settings::apply_here(&st.peek().daemon.settings);

            // First run gets the walkthrough, an upgrade gets the release
            // notes, and a window that is neither gets neither. Only the first
            // window: a second window is not a second first run, and two
            // sheets over two windows is the same sheet twice.
            //
            // Agent detection walks every PATH entry. It used to run to
            // completion before the onboarding sheet opened AND before the
            // daemon was asked to start, so a first launch paid the scan and
            // the connect in series. The sheet already treats an empty agent
            // list as a first-class state, and the daemon row updates live
            // from `conn`, so the scan can fill in beside the connect rather
            // than in front of it. Wall clock becomes max(scan, connect)
            // instead of their sum.
            let agent_fill = if seed.ordinal == 0 {
                let onboarded = st.peek().daemon.settings.onboarded;
                if !onboarded {
                    st.write().window.layer = Layer::Onboarding;
                    Some(tokio::task::spawn_blocking(launch::detected_agents))
                } else {
                    let seen = st.peek().daemon.settings.last_seen_version();
                    if !ui::whatsnew::whats_new(seen.as_ref()).is_empty() {
                        st.write().window.layer = Layer::WhatsNew;
                    }
                    None
                }
            } else {
                None
            };
            let fill_agents = async {
                if let Some(handle) = agent_fill {
                    detected.set(Some(handle.await.unwrap_or_default()));
                }
            };

            if opts.fixture {
                let now = tick().now_ms;
                let first = {
                    let mut w = st.write();
                    w.daemon.conn = ConnState::Fixture;
                    w.daemon.projects = fixture::projects();
                    w.daemon.sessions = fixture::sessions(now);
                    // The same placement a live `Sessions` snapshot gets inside
                    // `DaemonState::apply`. Without it the fixture has a different
                    // state machine from the real thing: nothing is ever filed
                    // into a workspace, `workspace_of` falls back to `intake`, and
                    // every session follows the operator into whichever workspace
                    // they just created. A demo mode that diverges from the real
                    // path is worse than no demo mode.
                    let infos: Vec<vitrum_proto::SessionInfo> = w
                        .daemon
                        .sessions
                        .iter()
                        .map(|row| row.info.clone())
                        .collect();
                    w.daemon.workspaces.adopt(infos.iter());
                    w.daemon.sessions.first().map(|row| row.id())
                };
                if let Some(id) = first {
                    st.write().open(id, now);
                }
                reconcile(bridge, st, attached, opts);
                fill_agents.await;
            } else {
                // The Advanced tab may point somewhere other than the command
                // line. `resolved_daemon_url` falls back to `--server` when the
                // setting is blank, so the flag still wins on a fresh profile.
                let url = st
                    .peek()
                    .daemon
                    .settings
                    .resolved_daemon_url(opts.server)
                    .to_string();
                // First-run agent detection shares this await so a PATH walk
                // cannot push the first connect later than it has to.
                tokio::join!(
                    start_daemon_then_connect(bridge, st, &url, opts),
                    fill_agents
                );
            }

            // Re-derive the automatic sidebar width against the window's real
            // geometry, on every control-plane message.
            //
            // This is not belt-and-braces, it is the only thing that gets the
            // number right on a first launch. The signal's initialiser runs
            // before the window is mapped and `inner_size` there still reports
            // the toolkit's placeholder: measured on this 3840x2160 panel it
            // comes back under 700 CSS pixels, 22% of which is below the 224px
            // legibility floor, so the sidebar clamped to the floor and stayed
            // there for the life of the window however large the screen was.
            // Neither the `Resized` handler nor a single re-measure after the
            // connect covers it: a window created at its final size is never
            // resized, and the connect can complete without ever yielding to
            // the event loop.
            //
            // Doing it per message is affordable because control-plane
            // messages are rare by design — the daemon sends none at all while
            // sessions merely stream output — and it is one `inner_size` call
            // guarded by an equality check, so it writes the signal only when
            // the answer actually moved. A width the operator has chosen is
            // never touched.
            let mut resettle = move || {
                if *sidebar_pinned.peek() {
                    return;
                }
                let css = css_viewport_width(&settle_window, *scale.peek());
                let want = default_sidebar_width(css);
                if want != st.peek().window.sidebar_width {
                    tracing::info!(
                        "sidebar re-derived: viewport {css} css px, want {want}, had {}",
                        st.peek().window.sidebar_width
                    );
                    st.write().window.set_sidebar_width_in(want, css);
                }
            };
            resettle();

            let mut eval = bridge.eval;
            loop {
                match eval.recv::<BridgeEvent>().await {
                    Ok(ev) => {
                        resettle();
                        on_bridge_event(
                            ev,
                            bridge,
                            st,
                            attached,
                            opts,
                            pending_terminate,
                            pending_open,
                            reconnect,
                        );
                        claim_link(bridge, st, attached, pending_link, opts);
                        claim_launch(bridge, st, attached, pending_open, opts);
                    }
                    Err(e) => {
                        tracing::error!("bridge channel closed: {e}");
                        break;
                    }
                }
            }
        }
    });

    // After first paint. A GitHub round trip must not lengthen the path to a
    // usable window, and a fixture has no network story to tell.
    use_future(move || {
        let mut update_offer = update_offer;
        async move {
            if opts.fixture {
                // Still honour an explicit demo override so screenshots and
                // review can paint the chip without a live release.
                if std::env::var_os("VITRUM_UPDATE_OFFER").is_none() {
                    return;
                }
            }
            // A forced offer is for screenshots and demos; paint it on the
            // first tick. A real check waits so it cannot compete with the
            // first paint or the daemon connect for the network.
            let delay = if std::env::var_os("VITRUM_UPDATE_OFFER").is_some() {
                std::time::Duration::from_millis(50)
            } else {
                std::time::Duration::from_secs(2)
            };
            tokio::time::sleep(delay).await;
            let got = off_thread(update::quiet_check).await;
            match got {
                Ok(status) => {
                    let ignored = st.peek().daemon.settings.ignored_update.clone();
                    let next = update::chrome_offer(&status, &ignored);
                    tracing::debug!(
                        "quiet update check: status={status:?} offer={}",
                        next.as_ref()
                            .map(|a| a.version.to_string())
                            .unwrap_or_else(|| "none".into())
                    );
                    update_offer.set(next);
                }
                Err(e) => {
                    tracing::debug!("quiet update check skipped: {e:#}");
                }
            }
        }
    });

    let render_tick = tick();
    let clock = render_tick.fmt;
    let home = clock::home();
    let dragging = drag.read().is_some();
    let flash = st.read().window.flash.clone();
    // A notice retires itself; an error does not.
    //
    // `Flash` had no expiry of any kind, so every confirmation the product
    // ever raised stayed on screen until the operator clicked Dismiss. On a
    // running window "Started bash in tmp. Ctrl+Shift+X stops it." was still
    // occupying a full-width band above the terminal twenty-nine minutes
    // later. A transient confirmation that never leaves is not a
    // confirmation, it is permanent chrome that happens to be worded like
    // news, and it is the loudest thing on an otherwise quiet screen.
    //
    // Errors stay. They report something the operator has to act on, and a
    // failure that erases itself before it is read is worse than a banner.
    //
    // A memo, so the effect runs when the FLASH changes rather than on every
    // daemon message, and the one-shot is scoped to the exact notice it was
    // raised for: it re-reads on wake and clears nothing unless that same
    // notice is still there, so a later notice keeps its full life.
    let flash_now = use_memo(move || st.read().window.flash.clone());
    use_effect(move || {
        let Some(mine) = flash_now() else { return };
        if mine.kind != state::FlashKind::Notice {
            return;
        }
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(NOTICE_MS)).await;
            if st.peek().window.flash.as_ref() == Some(&mine) {
                st.write().window.flash = None;
            }
        });
    });
    let layer = st.read().window.layer.clone();
    let armed = pending_terminate.read().clone();
    // Document width in CSS pixels, for clamping the sidebar against the
    // window rather than against a constant. Read once per paint: it is two
    // syscall-free reads off the window handle, and taking it inside the drag
    // handler would take it on every pointer sample.
    let viewport_css = css_viewport_width(&window, *scale.read());

    // Two consumers (a sidebar row and a tab), so it cannot be a single
    // `FnMut` closure that the first `EventHandler` moves out of.
    let menu_from_sidebar = {
        let window = window.clone();
        move |args| open_menu(st, &window, args)
    };

    // Theme, density and text scale are token overrides on the shell root, so
    // every one of them cascades into the sidebar and the dialogs from a
    // single element. Without these three attributes the
    // appearance settings render controls that change nothing.
    let settings = st.read().daemon.settings.clone();
    let update_version = update_offer()
        .as_ref()
        .map(|a| a.version.to_string());

    rsx! {
        div {
            // Density is carried entirely by `root_style`'s custom properties.
            // A `rg-density--*` modifier used to ride along here "for
            // structural rules a custom property cannot express", and no such
            // rule was ever written, so it was two class values in shipped
            // markup that painted nothing and told nobody anything. The
            // shell's own root was also the one place in the product an
            // emitted class was never checked against a stylesheet, which is
            // why it survived; main.rs is now in that guard's scan.
            class: "rg-app",
            "data-theme": ui::settings::theme_attr(&settings),
            style: ui::settings::root_style(&settings),
            ui::titlebar::TitleBar {
                state: st,
                // Built here rather than inside the titlebar so that file
                // stays about the window frame and grows no second opinion
                // about workspaces.
                switcher: rsx! {
                    ui::workspaces::WorkspaceSwitcher {
                        state: st,
                        clock: render_tick.model,
                    }
                },
                on_retry: move |()| retry(bridge, st, attached, opts),
                server: opts.server,
                on_drag: {
                    let window = window.clone();
                    move |()| window.drag()
                },
                on_toggle_maximize: {
                    let window = window.clone();
                    move |()| window.set_maximized(!window.is_maximized())
                },
                on_minimize: {
                    let window = window.clone();
                    move |()| window.set_minimized(true)
                },
                on_close: {
                    let window = window.clone();
                    move |()| window.close()
                },
                on_shortcuts: move |()| toggle_layer(st, Layer::Shortcuts),
                update_version,
                on_update: move |()| {
                    st.write().window.layer = Layer::Settings(state::SettingsTab::About);
                },
                on_dismiss_update: move |()| {
                    let Some(offer) = update_offer.peek().clone() else {
                        return;
                    };
                    {
                        let mut w = st.write();
                        w.daemon.settings.ignore_update(&offer.version);
                    }
                    ui::settings::commit(&st.peek());
                    update_offer.set(None);
                },
            }

            ui::workspaces::WorkspaceBar {
                state: st,
                clock: render_tick.model,
                on_manage: move |()| {
                    toggle_layer(st, Layer::Settings(state::SettingsTab::Workspaces))
                },
            }

            div { class: "rg-body",
                ui::sidebar::Sidebar {
                    state: st,
                    clock,
                    home: home.clone(),
                    server: opts.server,
                    on_select: move |(id, click): (SessionId, state::Click)| {
                        let tick = crate::tick();
                        // Selection is a pure pointer gesture and must be
                        // instant. Only a plain click opens the session; a
                        // modifier click builds a set to act on and must not
                        // drag the terminal along with it.
                        st.write().click_row(id, click, tick.model);
                        if click == state::Click::Plain {
                            st.write().open(id, tick.now_ms);
                            reconcile(bridge, st, attached, opts);
                        }
                    },
                    on_close_session: move |id: SessionId| {
                        request_terminate(bridge, st, &[id], opts, pending_terminate);
                        reconcile(bridge, st, attached, opts);
                    },
                    on_toggle_project: move |key: GroupKey| {
                        let mut w = st.write();
                        if !w.window.collapsed.remove(&key) {
                            w.window.collapsed.insert(key);
                        }
                    },
                    on_toggle_section: move |(key, section): (GroupKey, Section)| {
                        st.write().toggle_section(key, section);
                    },
                    on_toggle_preview: move |key: GroupKey| {
                        st.write().toggle_preview(key);
                    },
                    // Required, not Option, and deliberately so: the "Show
                    // more" button is emitted only when rows sit behind it, so
                    // an absent handler would put a live-looking control on
                    // screen that does nothing. A required prop makes the
                    // compiler ask instead of the operator finding out.
                    on_toggle_settled_tail: move |key: GroupKey| {
                        st.write().window.toggle_settled_tail(key);
                    },
                    on_toggle_sidebar: move |()| {
                        let mut w = st.write();
                        w.window.sidebar_collapsed = !w.window.sidebar_collapsed;
                    },
                    on_retry: move |()| retry(bridge, st, attached, opts),
                    // The toolbar's "n waiting" chip is the pointer half of
                    // Ctrl+Shift+Down. Same function, so the two cannot drift.
                    on_jump: move |()| jump_to_attention(bridge, st, Direction::Next),
                    on_new_session: move |project: Option<ProjectId>| {
                        open_new_session(st, project)
                    },
                    // The footer control's primary half. No layer: this is the
                    // one-click path, and the caret beside it is the "something
                    // else" path. Every route to a new session used to cost
                    // two clicks, the first of which only opened a form asking
                    // three questions with known answers.
                    on_launch_now: move |()| launch_now(bridge, st, pending_open, None),
                    on_filter: move |q: String| {
                        st.write().window.filter = q;
                    },
                    on_menu: menu_from_sidebar,
                    on_settings: move |()| {
                        toggle_layer(st, Layer::Settings(state::SettingsTab::default()))
                    },
                    on_resize_start: move |x: f64| {
                        let w = st.peek().window.sidebar_width;
                        drag.set(Some((x, w)));
                    },
                    on_resize_nudge: move |dx: f64| {
                        let w = st.peek().window.sidebar_width;
                        st.write().window.set_sidebar_width_in(w + dx, viewport_css);
                        remember_sidebar(seed.ordinal, st.peek().window.sidebar_width);
                        // Two files hold this number and only one of them is
                        // authoritative. `windows.json` is the fallback for a
                        // window nobody has expressed a preference about;
                        // `ui.json` is the preference, and it is what the
                        // startup future reads back over the geometry. Writing
                        // only the first meant every keyboard resize was
                        // silently discarded on the next launch.
                        ui::settings::commit(&st.peek());
                        sidebar_pinned.set(true);
                    },
                }

                div { class: "rg-main",
                    if let Some(f) = flash {
                        div { class: "{f.class()}",
                            span { class: "rg-flash__text", "{f.text}" }
                            if !armed.is_empty() {
                                button {
                                    class: "rg-btn-inline rg-btn-inline--danger",
                                    r#type: "button",
                                    onclick: move |_| {
                                        let targets = pending_terminate.peek().clone();
                                        request_terminate(
                                            bridge, st, &targets, opts, pending_terminate,
                                        );
                                        reconcile(bridge, st, attached, opts);
                                    },
                                    "Terminate"
                                }
                            }
                            button {
                                class: "rg-btn-inline",
                                r#type: "button",
                                onclick: move |_| {
                                    // Dismissing the prompt disarms it. A
                                    // confirmation that survives the message
                                    // explaining it is a trap.
                                    pending_terminate.set(Vec::new());
                                    st.write().window.flash = None;
                                },
                                "Dismiss"
                            }
                        }
                    }

                    ui::terminal::TerminalPane {
                        state: st,
                        on_new_session: move |()| open_new_session(st, None),
                        on_close_tab: move |id: SessionId| {
                            st.write().close_tab(id);
                            reconcile(bridge, st, attached, opts);
                        },
                        on_retry: move |()| retry(bridge, st, attached, opts),
                    }
                }
            }

            // Exactly one transient layer, never a stack. Escape has one
            // meaning, focus has one home, and the bridge's `layerOnly` scope
            // is a single DOM query for `.rg-layer`.
            match layer {
                Layer::None => rsx! {},
                Layer::Shortcuts => rsx! {
                    ui::shortcuts::Shortcuts { state: st, on_dismiss: move |()| dismiss(st) }
                },
                Layer::Search => {
                    let search = st.peek().window.search.clone();
                    let titles: Vec<(SessionId, String)> = st
                        .peek()
                        .daemon
                        .sessions
                        .iter()
                        .map(|row| (row.id(), row.info.title.clone()))
                        .collect();
                    rsx! {
                        ui::search::Search {
                            query: search.query.clone(),
                            options: search.options,
                            answer: search.answer.clone(),
                            searching: search.searching,
                            scope: search.scope.len(),
                            titles,
                            on_query: move |q: String| st.write().window.search.query = q,
                            on_toggle: move |which| {
                                let mut w = st.write();
                                w.window.search.options = w.window.search.options.toggled(which);
                            },
                            on_submit: move |()| run_search(bridge, st),
                            // Focuses the session the hit came from AND lands
                            // on the line. `line_seq` used to be discarded:
                            // the tooltip promised "jump to this line" and the
                            // handler focused the session, painted the usual
                            // head-anchored history and left you wherever that
                            // stopped, which for a hit written an hour ago is
                            // nowhere near it.
                            //
                            // Recording the intent BEFORE `open` matters:
                            // `open` clears the history anchor when focus
                            // actually moves, and `reconcile` below reads the
                            // intent to anchor the request on the hit.
                            on_activate: move |(id, line_seq): (SessionId, u64)| {
                                {
                                    let mut w = st.write();
                                    w.open(id, tick().now_ms);
                                    w.window.history_intent =
                                        state::HistoryIntent::Jump(line_seq);
                                    w.window.layer = Layer::None;
                                }
                                // A hit in the session already focused issues
                                // no Attach, so `reconcile` would send no
                                // Scrollback and the jump would never happen.
                                // Detaching forces the round trip.
                                attached.set(None);
                                reconcile(bridge, st, attached, opts);
                            },
                            on_dismiss: move |()| dismiss(st),
                        }
                    }
                }
                Layer::Settings(tab) => rsx! {
                    ui::settings::SettingsSheet {
                        state: st,
                        tab,
                        on_tab: move |t| st.write().set_settings_tab(t),
                        on_reconnect: move |url: String| {
                            bridge.cmd(BridgeCmd::Connect { url });
                            attached.set(None);
                        },
                        on_dismiss: move |()| dismiss(st),
                        update_offer,
                    }
                },
                // Both sheets record that they were seen the moment they
                // close, however they close. A first-run sheet that comes back
                // because you dismissed it rather than finished it is a sheet
                // that punishes you for not reading it.
                Layer::Onboarding => rsx! {
                    ui::onboarding::Onboarding {
                        machine: ui::onboarding::Machine {
                            agents: detected(),
                            connected: st.peek().daemon.conn.is_live(),
                            any_session: !st.peek().daemon.sessions.is_empty(),
                        },
                        on_close: move |_outcome: ui::onboarding::Outcome| {
                            {
                                let mut w = st.write();
                                w.daemon.settings.finish_onboarding(&update::current_version());
                                w.window.layer = Layer::None;
                            }
                            ui::settings::commit(&st.peek());
                        },
                    }
                },
                Layer::WhatsNew => rsx! {
                    ui::whatsnew::WhatsNew {
                        releases: ui::whatsnew::whats_new(
                            st.peek().daemon.settings.last_seen_version().as_ref(),
                        ),
                        on_dismiss: move |()| {
                            {
                                let mut w = st.write();
                                w.daemon.settings.mark_seen(&update::current_version());
                                w.window.layer = Layer::None;
                            }
                            ui::settings::commit(&st.peek());
                        },
                    }
                },
                Layer::Menu(menu) => rsx! {
                    ui::menu::ContextMenu {
                        state: st,
                        menu,
                        clock,
                        on_pick: move |(action, m): (MenuAction, MenuState)| {
                            on_menu_action(action, m, bridge, st, attached, opts, pending_terminate);
                        },
                        on_dismiss: move |()| dismiss(st),
                    }
                },
                Layer::NewSession(seed) => rsx! {
                    ui::dialog::NewSessionDialog {
                        state: st,
                        seed,
                        on_launch: move |(project, l): (ProjectId, launch::Launch)| {
                            start_session(bridge, st, pending_open, project, l);
                            dismiss(st);
                        },
                        on_dismiss: move |()| dismiss(st),
                    }
                },
                Layer::Rename(seed) => rsx! {
                    ui::dialog::RenameDialog {
                        seed,
                        on_rename: move |(session, title): (SessionId, String)| {
                            bridge.msg(&ClientMsg::Rename { session, title });
                            dismiss(st);
                        },
                        on_dismiss: move |()| dismiss(st),
                    }
                },
            }

            // Exists only while the sidebar edge is being dragged, so there is
            // no permanent mousemove listener sending an IPC message on every
            // pointer motion over the window.
            if dragging {
                div {
                    class: "rg-drag-shield",
                    onmousemove: move |e| {
                        if let Some((x0, w0)) = *drag.peek() {
                            let dx = e.client_coordinates().x - x0;
                            st.write().window.set_sidebar_width_in(w0 + dx, viewport_css);
                        }
                    },
                    // The width goes into the book when the drag ends rather
                    // than on every pointer sample: the book is what gets
                    // written to disk, and a hundred writes across one drag is
                    // a hundred writes nobody asked for.
                    onmouseup: move |_| {
                        drag.set(None);
                        remember_sidebar(seed.ordinal, st.peek().window.sidebar_width);
                        save_geometry();
                        ui::settings::commit(&st.peek());
                        sidebar_pinned.set(true);
                    },
                    onmouseleave: move |_| {
                        drag.set(None);
                        remember_sidebar(seed.ordinal, st.peek().window.sidebar_width);
                        save_geometry();
                        ui::settings::commit(&st.peek());
                        sidebar_pinned.set(true);
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
