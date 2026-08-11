//! Tests for the crate root.
//!
//! Split out of `main.rs`, because shipped code and the guards that police it
//! are different reading tasks and a file you scroll past a thousand
//! assertions to reach the next function is a file nobody reads twice.
//!
//! What lives here is what `main.rs` itself owns: how a panel's density
//! becomes a magnification, how a window claims and gives back a slot, how a
//! second launch reaches the process that already has one, what the command
//! line accepts, when the client gives up reconnecting, and what the document
//! is allowed to contain. Everything narrower belongs to its own module's
//! test file.

mod actions;
mod assets;
mod integrations;
mod no_javascript;
mod platform_build;
mod readme;
mod startup;
pub(crate) mod tree;
mod upstream;
mod workflows;

use super::*;

/// The reported panel on this machine: 3840x2160 across 597x336 mm.
fn panel_4k() -> Density {
    Density {
        width_px: 3840,
        height_px: 2160,
        width_mm: 597,
        height_mm: 336,
        os_scale: 1.0,
    }
}

/// The other panel on this machine: same 27-inch glass, a quarter of the
/// pixels.
fn panel_1080p() -> Density {
    Density {
        width_px: 1920,
        height_px: 1080,
        width_mm: 597,
        height_mm: 336,
        os_scale: 1.0,
    }
}

/// The shipped bug: two monitors the toolkit cannot tell apart.
///
/// Both report a scale factor of 1.0, because `Xft.dpi` is 96 and
/// `GDK_SCALE` is unset, so anything reasoning from `scale_factor()` alone
/// draws the 163 dpi panel at exactly half the physical size of the 82 dpi
/// one. Physical size is the only input that separates them, and these are
/// the numbers RandR reports here.
#[test]
fn the_two_monitors_differ_only_in_physical_density() {
    assert_eq!(panel_4k().os_scale, panel_1080p().os_scale);

    let dense = panel_4k()
        .dpi()
        .expect("597 mm across is a real measurement");
    let sparse = panel_1080p().dpi().expect("same panel size, fewer pixels");
    assert!((dense - 163.4).abs() < 0.5, "4K panel measured {dense} dpi");
    assert!(
        (sparse - 81.7).abs() < 0.5,
        "1080p panel measured {sparse} dpi"
    );

    assert_eq!(panel_4k().ui_scale(), 1.5);
    assert_eq!(panel_1080p().ui_scale(), 1.0);
}

/// A dense panel must never be drawn at 1x, and a sparse one must never be
/// shrunk below the size it was authored at.
///
/// The second half is the one worth pinning: 82 dpi divided by 96 is 0.85,
/// and honouring that would make the low-density monitor physically
/// smaller than it is today to fix a problem it does not have.
#[test]
fn scale_never_shrinks_below_the_authored_size() {
    assert_eq!(quantize_ui_scale(0.85), MIN_UI_SCALE);
    assert_eq!(quantize_ui_scale(0.1), MIN_UI_SCALE);
    assert_eq!(quantize_ui_scale(f64::NAN), MIN_UI_SCALE);
    assert_eq!(quantize_ui_scale(f64::INFINITY), MIN_UI_SCALE);
    assert_eq!(quantize_ui_scale(99.0), MAX_UI_SCALE);
}

/// The ladder is 25% steps, and the boundaries land where a person would
/// expect rather than one step off.
#[test]
fn scale_snaps_to_quarter_steps() {
    assert_eq!(quantize_ui_scale(1.0), 1.0);
    assert_eq!(quantize_ui_scale(1.12), 1.0);
    assert_eq!(quantize_ui_scale(1.13), 1.25);
    assert_eq!(quantize_ui_scale(1.485), 1.5);
    assert_eq!(quantize_ui_scale(1.702), 1.75);
    assert_eq!(quantize_ui_scale(2.29), 2.25);
    assert_eq!(quantize_ui_scale(2.4), 2.5);
}

/// A panel that will not report its size gets 1.0, not a guess.
///
/// Two very different machines land here: a Wayland or macOS session where
/// the platform already applied the user's scale, and a virtual display
/// that reports zero millimetres. Inventing a factor from the pixel count
/// alone would double the UI on a 4K television across the room.
#[test]
fn a_panel_that_will_not_measure_itself_is_left_alone() {
    let unknown = Density {
        width_px: 3840,
        height_px: 2160,
        width_mm: 0,
        height_mm: 0,
        os_scale: 1.0,
    };
    assert_eq!(unknown.dpi(), None);
    assert_eq!(unknown.ui_scale(), 1.0);
}

/// An EDID claiming a 160x90 mm desktop monitor is lying, and believing it
/// would produce a scale of 6 and a window showing four words.
#[test]
fn implausible_physical_sizes_are_rejected_rather_than_believed() {
    let liar = Density {
        width_px: 3840,
        height_px: 2160,
        width_mm: 160,
        height_mm: 90,
        os_scale: 1.0,
    };
    assert!(
        liar.dpi().is_none(),
        "{:?} dpi should be out of band",
        liar.dpi()
    );
    assert_eq!(liar.ui_scale(), 1.0);

    let projector = Density {
        width_px: 1920,
        height_px: 1080,
        width_mm: 4000,
        height_mm: 2250,
        os_scale: 1.0,
    };
    assert_eq!(projector.dpi(), None);
}

/// A platform that already scaled must not be scaled again.
///
/// A Retina panel is 220 dpi with a backing scale factor of 2, so its CSS
/// pixels are already 110 to the inch. Multiplying by 220/96 would draw
/// everything at more than twice life size.
#[test]
fn a_platform_that_already_scaled_is_not_scaled_twice() {
    let retina = Density {
        width_px: 2880,
        height_px: 1800,
        width_mm: 331,
        height_mm: 207,
        os_scale: 2.0,
    };
    let physical = retina.dpi().expect("a real panel size");
    assert!(physical > 200.0, "{physical} dpi");
    let css = retina.css_dpi().expect("halved by the backing scale");
    assert!((css - physical / 2.0).abs() < 0.01);

    // The contract, stated as the thing that actually matters: total
    // magnification, toolkit and ours combined, lands within one step of
    // what the panel's density asks for. A second doubling would put it at
    // 4.6 against an ideal of 2.3 and blow this apart.
    let ideal = physical / REFERENCE_DPI;
    let applied = retina.os_scale * retina.ui_scale();
    assert!(
        (applied - ideal).abs() <= UI_SCALE_STEP,
        "panel wants {ideal:.2}x, we apply {applied:.2}x"
    );
    assert!(applied < 2.0 * 1.5, "the toolkit's 2x was applied twice");

    assert_eq!(
        retina.ui_scale(),
        1.0,
        "a Retina panel needs no correction at all"
    );

    // Same check for the two panels on this machine. The 1080p one is the
    // documented exception: its honest answer is below 1.0 and the floor
    // outranks it, which is the one place the ideal is deliberately missed.
    for panel in [panel_4k(), panel_1080p()] {
        let ideal = panel.dpi().unwrap() / REFERENCE_DPI;
        let applied = panel.os_scale * panel.ui_scale();
        assert!(
            (applied - ideal).abs() <= UI_SCALE_STEP || applied == MIN_UI_SCALE,
            "{panel:?} wants {ideal:.2}x, we apply {applied:.2}x"
        );
    }
}

/// The sidebar default is a fraction of the window, not a constant.
///
/// The shipped constant was 256 px: a fifth of a 1280 px window, and 6.7%
/// of a 3840 px one. The second number is a column of elided titles beside
/// an ocean of empty pane.
#[test]
fn the_sidebar_default_tracks_the_window_rather_than_a_constant() {
    let narrow = default_sidebar_width(1280.0);
    let wide = default_sidebar_width(3840.0);
    assert!(narrow < wide, "{narrow} should be narrower than {wide}");
    assert_eq!(narrow, 1280.0 * SIDEBAR_FRACTION);

    // Both stay inside the bounds the stylesheet enforces.
    for width in [400.0, 800.0, 1280.0, 1920.0, 2560.0, 3840.0, 7680.0] {
        let got = default_sidebar_width(width);
        assert!(
            (SIDEBAR_MIN_PX..=SIDEBAR_MAX_PX).contains(&got),
            "{width} px window produced a {got} px sidebar"
        );
    }

    // The old constant was 6.7% of a 4K window. The new default is not, and
    // that ratio is the whole defect.
    let share = wide / 3840.0;
    assert!(
        share > 0.10,
        "sidebar is still only {:.1}% of the window",
        share * 100.0
    );
}

/// The pane wins when the window is too narrow for both.
///
/// A sidebar beside a 30-column pane is a file manager. The floor is the
/// stylesheet's minimum, because below that the sidebar shows neither a
/// title nor a pill and is worse than useless.
#[test]
fn a_narrow_window_gives_the_room_to_the_pane() {
    assert_eq!(default_sidebar_width(500.0), SIDEBAR_MIN_PX);
    let cramped = default_sidebar_width(640.0);
    assert_eq!(cramped, SIDEBAR_MIN_PX);
    assert!(
        640.0 - cramped >= MIN_CONTENT_CSS_PX - 1.0,
        "pane left with {} px",
        640.0 - cramped
    );
}

/// Ordinals are reused, so closing the second window and opening another
/// puts it back where the old one was instead of cascading forever.
#[test]
fn a_closed_window_gives_its_slot_back() {
    // The book is process-global, so this test claims and releases exactly
    // what it takes and asserts on deltas rather than absolutes.
    let a = claim_ordinal();
    let b = claim_ordinal();
    let c = claim_ordinal();
    assert_ne!(a, b);
    assert_ne!(b, c);

    release_ordinal(b);
    let reused = claim_ordinal();
    assert_eq!(
        reused, b,
        "the freed slot should be the next one handed out"
    );

    for n in [a, b, c] {
        release_ordinal(n);
    }
}

/// Successive fresh windows must not land exactly on top of each other,
/// and must not walk off the bottom of the screen either.
#[test]
fn fresh_windows_cascade_and_stay_on_the_monitor() {
    // No monitor handle in a unit test, so this exercises the fallback
    // rectangle, which is the same arithmetic against a nominal screen.
    let first = fresh_geometry(None, 1.0, 0);
    let second = fresh_geometry(None, 1.0, 1);
    assert_ne!(
        (first.x, first.y),
        (second.x, second.y),
        "the second window is exactly under the first"
    );

    // Every step stays inside the nominal monitor the fallback describes.
    for ordinal in 0..CASCADE_STEPS * 2 {
        let state = fresh_geometry(None, 1.0, ordinal);
        assert!(state.x >= 0, "window {ordinal} at x={}", state.x);
        assert!(state.y >= 0, "window {ordinal} at y={}", state.y);
        assert!(state.width > 0 && state.height > 0);
    }
}

/// A window on a dense panel opens physically the same size as one on a
/// sparse panel, which means more device pixels.
#[test]
fn a_fresh_window_is_sized_in_physical_units() {
    let sparse = fresh_geometry(None, 1.0, 0);
    let dense = fresh_geometry(None, 1.75, 0);
    assert_eq!(dense.width, (f64::from(sparse.width) * 1.75).round() as u32);
    assert_eq!(
        dense.height,
        (f64::from(sparse.height) * 1.75).round() as u32
    );
}

/// Remembered geometry round-trips through the file format, and a version
/// this build does not know is discarded rather than half-read.
#[test]
fn window_geometry_survives_a_round_trip_and_rejects_a_future_format() {
    let windows = vec![
        WindowGeometry {
            x: 1920,
            y: 0,
            width: 2240,
            height: 1400,
            maximized: false,
            sidebar_width: 282,
        },
        WindowGeometry {
            x: 100,
            y: 100,
            width: 1280,
            height: 800,
            maximized: true,
            sidebar_width: 300,
        },
    ];
    let text = serde_json::to_string(&PersistedWindows {
        version: window_state::STATE_FORMAT_VERSION,
        windows: windows.clone(),
    })
    .expect("plain numbers always encode");
    let back: PersistedWindows = serde_json::from_str(&text).expect("what we just wrote");
    assert_eq!(back.windows, windows);

    let future = text.replace(
        &format!("\"version\":{}", window_state::STATE_FORMAT_VERSION),
        "\"version\":99",
    );
    let parsed: PersistedWindows = serde_json::from_str(&future).expect("still valid json");
    assert_ne!(
        parsed.version,
        window_state::STATE_FORMAT_VERSION,
        "the version guard has nothing to catch"
    );
}

/// A saved rectangle on a monitor that has since been unplugged comes back
/// somewhere the user can reach.
#[test]
fn geometry_from_a_vanished_monitor_lands_on_one_that_exists() {
    let only = Monitor::new(0, 0, 1920, 1080);
    let stranded = WindowGeometry {
        x: 5000,
        y: 0,
        width: 1280,
        height: 800,
        maximized: false,
        sidebar_width: 280,
    };
    let fixed = window_state::clamp_to_monitors(&stranded, &[only]);
    assert!(
        fixed.x >= 0 && fixed.x + fixed.width as i32 <= 1920,
        "{fixed:?}"
    );
    assert!(
        fixed.y >= 0 && fixed.y + fixed.height as i32 <= 1080,
        "{fixed:?}"
    );
}

/// The monitor list handed to the clamp puts the primary first, because
/// that function breaks ties by position and "the other screen is gone"
/// has to land on the primary.
#[test]
fn the_primary_monitor_is_first_in_the_clamp_list() {
    // Built from the same shape `monitor_rects` produces, without needing
    // a live event loop to hand out `MonitorHandle`s.
    let primary = Monitor::new(0, 0, 1920, 1080);
    let secondary = Monitor::new(1920, 0, 3840, 2160);
    let ordered = vec![primary, secondary];
    let nowhere = WindowGeometry {
        x: -9000,
        y: -9000,
        width: 800,
        height: 600,
        maximized: false,
        sidebar_width: 280,
    };
    let fixed = window_state::clamp_to_monitors(&nowhere, &ordered);
    assert!(
        fixed.x < 1920,
        "a rectangle touching nothing landed on the secondary at x={}",
        fixed.x
    );
}

/// Every activation goes to exactly one waiter, and nothing is lost when
/// there is no waiter at all.
///
/// This is the whole contract behind "launching again opens a new window":
/// N windows park on one queue, and a handoff must open one window, not N
/// and not zero.
#[test]
fn a_handoff_reaches_exactly_one_waiter() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mailbox: Mailbox<Activation> = Mailbox::new();
    // Posted before anyone is waiting: the queue has to hold it.
    mailbox.post(Activation::Focus);
    mailbox.post(Activation::Open(DeepLink::Home));

    let taken = AtomicUsize::new(0);
    let mut got = Vec::new();
    // Three consumers, two items. Drained by polling the future to
    // completion by hand, which is what the executor does.
    for _ in 0..3 {
        if let Some(item) = poll_once(mailbox.next()) {
            taken.fetch_add(1, Ordering::Relaxed);
            got.push(item);
        }
    }
    assert_eq!(taken.load(Ordering::Relaxed), 2, "got {got:?}");
    assert_eq!(
        got,
        vec![Activation::Focus, Activation::Open(DeepLink::Home)]
    );
}

/// A waiter that finds the queue empty parks, and the next post wakes it
/// exactly once rather than leaving a stale waker behind per poll.
#[test]
fn an_empty_mailbox_parks_one_waker_per_waiter() {
    let mailbox: Mailbox<Activation> = Mailbox::new();
    assert!(poll_once(mailbox.next()).is_none());
    assert_eq!(mailbox.lock().waiting.len(), 1);
    // Polled again by the same waker: still one, not two.
    assert!(poll_once(mailbox.next()).is_none());
    assert_eq!(
        mailbox.lock().waiting.len(),
        1,
        "a second poll left a stale waker behind"
    );
    mailbox.post(Activation::Focus);
    assert!(
        mailbox.lock().waiting.is_empty(),
        "post did not drain the wakers"
    );
}

/// Drive a future one step with a no-op waker.
fn poll_once<F: Future>(future: F) -> Option<F::Output> {
    use std::task::{RawWaker, RawWakerVTable, Waker};

    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(std::ptr::null(), &VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    // Safety: the vtable's functions ignore the data pointer entirely, so
    // a null one is never dereferenced.
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(v) => Some(v),
        Poll::Pending => None,
    }
}

/// `--ui-scale` overrides the measurement, and refuses a value that would
/// make the app unusable.
#[test]
fn the_scale_override_is_bounded_and_rejects_nonsense() {
    assert_eq!(Options::parse(Vec::<String>::new()).unwrap().ui_scale, None);
    assert_eq!(
        Options::parse(vec!["--ui-scale".into(), "auto".into()])
            .unwrap()
            .ui_scale,
        None
    );
    assert_eq!(
        Options::parse(vec!["--ui-scale".into(), "1.5".into()])
            .unwrap()
            .ui_scale,
        Some(1.5)
    );
    let err = Options::parse(vec!["--ui-scale".into(), "9".into()]).unwrap_err();
    assert!(err.message.contains("--ui-scale 9 is outside"), "{err}");
    let err = Options::parse(vec!["--ui-scale".into(), "huge".into()]).unwrap_err();
    assert!(
        err.message.contains("--ui-scale huge is not a number"),
        "{err}"
    );
}

/// A launch pointed somewhere specific must not be swallowed by the
/// running instance.
///
/// Without this, typing `--fixture` while the real app is running opens a
/// window onto the live daemon and silently discards the flag, which is
/// the worst possible answer: it looks like it worked.
#[test]
fn a_targeted_launch_refuses_to_join_the_running_instance() {
    assert!(!Options::parse(Vec::<String>::new()).unwrap().standalone);
    assert!(Options::parse(vec!["--fixture".into()]).unwrap().standalone);
    assert!(
        Options::parse(vec!["--server".into(), "ws://127.0.0.1:9999".into()])
            .unwrap()
            .standalone
    );
    assert!(
        Options::parse(vec!["--standalone".into()])
            .unwrap()
            .standalone
    );
    // The default URL spelled out explicitly is still the default.
    assert!(
        !Options::parse(vec!["--server".into(), wire::DEFAULT_WS_URL.into()])
            .unwrap()
            .standalone
    );
}

/// A primary that cannot serve activations must run standalone, not sit on
/// the slot.
///
/// The defect: `listen` failing was logged and otherwise ignored, so the
/// process kept the lock. Every later `vitrum` then resolved to a handoff
/// into a process with nothing accepting on the socket, and exited having
/// drawn no window at all. Losing window sharing is a degradation; a
/// launch that produces nothing is a failure, so the guard goes.
#[cfg(unix)]
#[test]
fn a_primary_that_cannot_listen_runs_standalone_rather_than_holding_the_slot() {
    let dir = std::env::temp_dir().join(format!(
        "vitrum-primary-role-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let lock = dir.join("instance.lock");
    let socket = dir.join("instance.sock");

    let Ok(Acquisition::Primary(guard)) =
        single_instance::acquire(&lock, &socket, &Activation::Focus)
    else {
        panic!("the first claim must be the primary");
    };
    let role = primary_role(guard, |_| {
        Err(single_instance::SingleInstanceError::Io {
            context: "starting the activation listener".to_string(),
            detail: "forced for the test".to_string(),
        })
    });
    assert!(
        matches!(role, Instance::Alone),
        "a primary whose listener failed must fall back to standalone"
    );

    let next = single_instance::acquire(&lock, &socket, &Activation::Focus)
        .expect("the slot must be free");
    assert!(
        next.is_primary(),
        "the next launch must win the slot and get a window, instead of \
         handing off to a process that cannot answer"
    );
    drop(next);
    std::fs::remove_dir_all(&dir).ok();
}

/// Autostart is on unless it is turned off, and `usage` says so.
///
/// The default matters more than the flag: the shipped behaviour was a red
/// banner on a clean machine with no hint that a second binary had to be
/// started by hand.
#[test]
fn the_daemon_starts_itself_unless_told_not_to() {
    assert!(Options::parse(Vec::<String>::new()).unwrap().autostart);
    assert!(
        !Options::parse(vec!["--no-autostart".into()])
            .unwrap()
            .autostart
    );
    let text = usage();
    assert!(text.contains("--no-autostart"), "{text}");
    assert!(
        text.contains(launch::DAEMON_BIN),
        "usage never mentions the daemon binary"
    );
}

/// The default option set, pinned whole so a new field cannot be added
/// with a surprising default and go unnoticed.
#[test]
fn default_options_connect_for_real() {
    assert_eq!(
        Options::parse(Vec::<String>::new()).unwrap(),
        Options {
            fixture: false,
            server: wire::DEFAULT_WS_URL,
            ui_scale: None,
            standalone: false,
            autostart: true,
            token_file: None,
        }
    );
}

/// No flags means a real connection attempt. If this ever defaulted to
/// fixture mode, a broken server would look like a working app.
#[test]
fn default_options_are_not_fixture() {
    assert!(!Options::parse(Vec::<String>::new()).unwrap().fixture);
}

/// Fixture mode must require the explicit flag and nothing else.
#[test]
fn fixture_requires_its_flag() {
    assert_eq!(
        Options::parse(vec!["--fixture".to_string()]).unwrap(),
        Options {
            fixture: true,
            server: wire::DEFAULT_WS_URL,
            ui_scale: None,
            standalone: true,
            autostart: true,
            token_file: None,
        }
    );
}

/// The daemon URL must be overridable, and must default to loopback. A
/// second daemon on another port is how the disconnect and reconnect paths
/// get exercised without taking down the one everybody else is using.
#[test]
fn the_server_url_is_overridable_and_defaults_to_loopback() {
    assert_eq!(
        Options::parse(Vec::<String>::new()).unwrap().server,
        "ws://127.0.0.1:7737"
    );
    assert_eq!(
        Options::parse(vec!["--server".into(), "ws://127.0.0.1:7738".into()])
            .unwrap()
            .server,
        "ws://127.0.0.1:7738"
    );
    assert_eq!(
        Options::parse(vec!["--server".into(), "wss://box.local:9000".into()])
            .unwrap()
            .server,
        "wss://box.local:9000"
    );
}

/// A URL that is not a WebSocket URL must be rejected at startup, by name.
/// Accepting `http://...` would open a socket that never handshakes and
/// present it as a connection failure, which sends the user looking at the
/// daemon instead of at their own command line.
#[test]
fn a_non_websocket_server_url_is_rejected() {
    let err = Options::parse(vec!["--server".into(), "http://127.0.0.1:7737".into()]).unwrap_err();
    assert!(
        err.message
            .contains("--server http://127.0.0.1:7737 is not a WebSocket URL"),
        "{err}"
    );
    let err = Options::parse(vec!["--server".into()]).unwrap_err();
    assert!(
        err.message.contains("--server needs a ws:// or wss:// URL"),
        "{err}"
    );
}

/// An unknown argument must be rejected with the usage text, not silently
/// ignored. A typo'd `--fixtures` that starts a real connection against an
/// absent server, with no message, is the worst of both outcomes.
///
/// It must also exit NON-ZERO. Every one of these went to stdout with a
/// successful status, so a wrapper script could not tell a typo from a launch.
#[test]
fn unknown_arguments_are_rejected_loudly() {
    let err = Options::parse(vec!["--fixtures".to_string()]).unwrap_err();
    assert!(err.message.contains("unknown argument --fixtures"), "{err}");
    assert!(err.message.contains("usage: vitrum"), "{err}");
    assert_ne!(err.exit.code(), 0, "a typo exited successfully");
}

/// A `vitrum://` URL on the command line must not be rejected as an option.
///
/// This is how the desktop launches a registered handler: the app installs
/// itself for `x-scheme-handler/vitrum`, the OS runs
/// `vitrum vitrum://session/3`, and `Activation::from_args` turns that into
/// a window. The option parser runs FIRST, and it used to answer "unknown
/// argument" and exit, so the entire deep-link feature was unreachable
/// while every other layer of it worked and was tested. Nothing caught it
/// because nothing tested the entry point with the URL the OS actually
/// passes.
#[test]
fn a_deep_link_url_is_not_an_unknown_argument() {
    for url in [
        "vitrum://session/3",
        "vitrum://project/1",
        "vitrum://home",
        // Case-insensitive: registered handlers are not required to
        // preserve it, and a capitalised scheme must not fail to launch.
        "VITRUM://session/3",
    ] {
        let opts = Options::parse(vec![url.to_string()])
            .unwrap_or_else(|e| panic!("{url} was rejected: {e}"));
        assert_eq!(
            opts.server,
            wire::DEFAULT_WS_URL,
            "a deep link must not change which daemon the window talks to"
        );
    }

    // A malformed one still parses as an ARGUMENT, so the activation layer
    // can report what is wrong with the URL itself. Saying "unknown
    // argument" there would name the wrong problem.
    assert!(Options::parse(vec!["vitrum://nonsense/9".to_string()]).is_ok());

    // And something merely starting with the same letters is still an
    // error, because it is not a URL.
    let err = Options::parse(vec!["vitrumish".to_string()]).unwrap_err();
    assert!(err.message.contains("unknown argument vitrumish"), "{err}");
}

/// The reconnect schedule backs off, is capped, and ENDS.
///
/// This is the one automatic reconnect in a program whose header says there
/// is none, and it is here because a window can now be pointed at a daemon
/// across a network: a laptop that closes its lid must not need a click to
/// come back. What keeps that honest is that it is a schedule rather than a
/// loop: each attempt is one `sleep` that fires once, a connected window
/// has none outstanding, and the schedule terminates.
#[test]
fn the_reconnect_schedule_backs_off_and_terminates() {
    let first = reconnect_delay_ms(0).expect("the first retry must be scheduled");
    assert_eq!(first, RECONNECT_BASE_MS, "the first retry must be prompt");

    // Strictly increasing until it reaches the ceiling, never past it.
    let mut prev = first;
    for n in 1..RECONNECT_ATTEMPTS {
        let d = reconnect_delay_ms(n).expect("still within the schedule");
        assert!(d >= prev, "attempt {n} waits less than attempt {}", n - 1);
        assert!(
            d <= RECONNECT_MAX_MS,
            "attempt {n} waits {d}ms, past the ceiling"
        );
        prev = d;
    }
    assert_eq!(
        prev, RECONNECT_MAX_MS,
        "the schedule never reaches its ceiling"
    );

    // And it STOPS. A window that reconnects forever is a window that never
    // tells the operator their daemon is gone.
    assert_eq!(
        reconnect_delay_ms(RECONNECT_ATTEMPTS),
        None,
        "the schedule is unbounded; nothing ever reports the daemon as gone"
    );
    assert_eq!(reconnect_delay_ms(u32::MAX), None);
}

/// Doubling must not overflow into a shorter wait.
///
/// `base << attempt` overflows a u64 at attempt 64 and wraps to zero, which
/// would turn the far end of the schedule into a tight reconnect loop: the
/// exact failure this design exists to avoid, arriving only after a long
/// outage when nobody is watching.
#[test]
fn a_long_outage_never_wraps_into_a_tight_loop() {
    for n in 0..RECONNECT_ATTEMPTS {
        let d = reconnect_delay_ms(n).expect("within the schedule");
        assert!(
            d >= RECONNECT_BASE_MS,
            "attempt {n} waits {d}ms, less than the base"
        );
    }
}

/// A fixture window schedules nothing, because it has no daemon.
#[test]
fn a_fixture_window_never_schedules_a_reconnect() {
    let opts = Options::parse(vec!["--fixture".to_string()]).expect("parses");
    assert!(opts.fixture);
    assert!(
        opts.standalone,
        "a fixture window must not join a running instance either"
    );
}

/// `--version` reports the crate version, and reports it as an exit path.
///
/// Every shipped binary answers this: it is how an operator filing a
/// report says which build they are on, and how they tell an installed
/// copy from one they just rebuilt. Taken from `CARGO_PKG_VERSION` so it
/// cannot drift from the tag a release was cut at, which a hand-written
/// string always eventually does.
#[test]
fn version_reports_the_crate_version_and_does_not_start_a_window() {
    for flag in ["-V", "--version"] {
        let out = Options::parse(vec![flag.to_string()])
            .expect_err("version must short-circuit startup, like --help");
        assert_eq!(
            out.message,
            format!("vitrum {}", env!("CARGO_PKG_VERSION")),
            "{flag} printed something other than the crate version"
        );
        assert_eq!(
            out.exit.code(),
            0,
            "{flag} was asked for, so it is not a failure"
        );
    }
}

/// The version is advertised in `--help`, or nobody knows to ask for it.
#[test]
fn the_help_text_lists_the_version_flag() {
    let help = usage();
    assert!(
        help.contains("--version"),
        "--help does not mention --version"
    );
    assert!(
        help.contains("-V"),
        "--help does not mention the short form"
    );
}

/// Help must be an error path, so `main` prints and exits instead of
/// opening a window.
#[test]
fn help_short_circuits_startup() {
    for flag in ["-h", "--help"] {
        let err = Options::parse(vec![flag.to_string()]).unwrap_err();
        assert!(err.message.contains("usage: vitrum"), "{flag}: {err}");
        assert!(err.message.contains("--fixture"), "{flag}: {err}");
        assert_eq!(err.exit.code(), 0, "{flag} was asked for, not a mistake");
    }
}

/// Help text is printed verbatim, so a C-style escape in it reaches the
/// operator's terminal as-is.
///
/// `usage()` is a Rust `format!`, where `%` carries no meaning. A `%%`
/// written out of printf habit is therefore NOT an escape: it rendered
/// literally, and `vitrum --help` shipped the line "0.24%% idle CPU" for
/// as long as that string existed. Nothing caught it because every test
/// asserted on substrings that did not include the percent, and the defect
/// is only visible by running the binary.
///
/// This locks the whole block: no doubled percent anywhere in the help,
/// and no stray `{`/`}` from a mis-escaped brace either, which is the same
/// mistake in the other direction.
#[test]
fn help_text_contains_no_unrendered_escapes() {
    let help = usage();
    assert!(
        !help.contains("%%"),
        "help ships a literal %% to the operator; `format!` does not treat \
         % as special, so write one: {help}"
    );
    for brace in ['{', '}'] {
        assert!(
            !help.contains(brace),
            "help ships a literal {brace}, which means a format placeholder \
             was mis-escaped: {help}"
        );
    }
}

/// One WebKit context and one web process, however many windows are open.
///
/// The shell around the pane is still a webview, and the fork it is built
/// on is where the sharing lives. Upstream builds a fresh `WebContext` per
/// webview and each one starts its own network process; the fork shares
/// one, and relates each new view to a live one so they also share a web
/// process. Both are one-line changes in a vendored file that a routine
/// upstream merge overwrites without anybody noticing, and the failure is
/// silent: the application still works, it just quietly costs three times
/// the memory. So this reads the vendored source.
#[test]
fn the_vendored_webview_keeps_one_context_and_one_web_process() {
    let src = include_str!("../../../vendor/src/webview.rs");
    assert!(
        src.contains("fn shared_web_context("),
        "the vendored webview lost its shared WebContext: twenty windows \
         go back to twenty WebKitNetworkProcess copies"
    );
    assert!(
        src.contains("WebViewBuilder::new_with_web_context(web_context)"),
        "the vendored webview no longer builds from the shared WebContext"
    );
    assert!(
        src.contains("with_related_view(target)"),
        "the vendored webview no longer relates new windows to an existing \
         one: every window gets its own WebKitWebProcess, which measured \
         1101.0 MB at twenty windows against 395.6 MB shared"
    );
    assert!(
        src.contains("register_webkit_view(built.webview())"),
        "a built webview is no longer registered as a relation target, so \
         only the second window would ever share a process"
    );
    let target = src
        .find("fn relation_target()")
        .expect("relation_target is gone");
    let retain = src[target..]
        .find("views.retain(")
        .expect("relation_target no longer drops dead views");
    let first = src[target..]
        .find("views.first()")
        .expect("relation_target no longer picks a view");
    assert!(
        retain < first,
        "relation_target picks a view before dropping dead ones, so it can \
         hand back a destroyed view; WebKit then refuses to reuse its \
         process and starts another, which measured 539.6 MB against 399.9"
    );
}

/// Every stylesheet in [`stylesheets`] actually reaches the document, and
/// nothing reaches it that the guards do not cover.
///
/// The guards below iterate that list, so a sheet missing from it is a
/// sheet exempt from all of them, silently.
///
/// This used to count `<style>` elements and compare the number to the
/// list's length. That was a proxy, and it broke the moment the sheets were
/// concatenated into one element for the cascade's sake: the head was
/// correct and the guard failed. Counting the CONTENT is the real check
/// and is strictly stronger, because a sheet silently dropped from the
/// bundle fails it whether or not the element count still adds up.
#[test]
fn every_shipped_stylesheet_is_covered_by_the_css_guards() {
    /// A style element the design-system guards do not read, and the
    /// reason.
    ///
    /// The guards iterate `stylesheets()`, so the bundle built from that
    /// array is covered by construction. Everything else is outside their
    /// reach and says so here in writing.
    ///
    /// Empty. It held one row for a vendored stylesheet that came with a
    /// JavaScript terminal, and that stylesheet is gone with it: every
    /// declaration in the document is now this project's own and is held
    /// to this project's motion, colour and grid rules.
    const GUARD_EXEMPT: &[(&str, &str)] = &[];

    let head = chrome::document_head();

    for (name, sheet) in stylesheets() {
        let stripped = strip_css(sheet);
        // A sheet that is only comments strips to nothing and would match
        // trivially, so it has to carry declarations to be checked at all.
        assert!(
            stripped.contains('{'),
            "{name} contributes no declarations; it is dead weight in the head"
        );
        assert!(
            head.contains(&stripped),
            "{name} is in `stylesheets()` and its declarations are not in \
             the document head, so every guard that checks it is checking \
             CSS the operator never receives"
        );
    }

    // What the operator actually receives, read back out of the document.
    let emitted: Vec<&str> = head
        .match_indices("<style>")
        .map(|(at, tag)| {
            let rest = &head[at + tag.len()..];
            let end = rest
                .find("</style>")
                .expect("every style element the head opens is closed");
            &rest[..end]
        })
        .collect();
    let origins = style_origins();
    assert_eq!(
        emitted.len(),
        origins.len(),
        "the head ships {} stylesheets and `style_origins` names {}, so one \
         of them reaches the operator without a name and without a guard",
        emitted.len(),
        origins.len()
    );
    for ((name, css), got) in origins.iter().zip(emitted.iter()) {
        assert_eq!(
            css, got,
            "the {name} element in the head is not what `style_origins` \
             produced, so the head is no longer built from that list"
        );
    }

    // Covered means the design-system guards read it: they iterate
    // `stylesheets()`, so the sheet carrying all of those is the covered
    // one. Identified by content rather than by name, because a name is a
    // claim and this is the fact.
    let covered: Vec<&str> = origins
        .iter()
        .filter(|(_, css)| {
            stylesheets()
                .iter()
                .all(|(_, sheet)| css.contains(&strip_css(sheet)))
        })
        .map(|(name, _)| *name)
        .collect();
    assert_eq!(
        covered.len(),
        1,
        "exactly one element carries the whole design system and is therefore \
         the one the guards read; {covered:?} carry it"
    );

    for (name, _) in origins.iter().filter(|(n, _)| !covered.contains(n)) {
        assert!(
            GUARD_EXEMPT.iter().any(|(exempt, _)| exempt == name),
            "{name} reaches the document and no design-system guard reads \
             it. Add it to `stylesheets()` so the guards cover it, or add a \
             row to GUARD_EXEMPT saying why they must not."
        );
    }
    for (name, why) in GUARD_EXEMPT {
        assert!(
            origins.iter().any(|(origin, _)| origin == name),
            "GUARD_EXEMPT still excuses {name} ({why}) and the head no \
             longer ships it"
        );
    }
}

/// The document carries declarations and nothing else.
///
/// The head is the one place in this program that assembles a document, so
/// it is the one place a script could be reintroduced without touching a
/// file whose name gives it away. `tests::no_javascript` reads the tree;
/// this reads the bytes the window is actually built from, which is the
/// claim an operator cares about.
#[test]
fn the_document_head_is_stylesheets_and_nothing_else() {
    let head = chrome::document_head();
    let mut rest = head;
    let mut elements = 0usize;
    while let Some(open) = rest.find('<') {
        assert!(
            rest[open..].starts_with("<style>"),
            "the head opens an element that is not a stylesheet: {:?}",
            &rest[open..(open + 40).min(rest.len())]
        );
        let close = rest[open..]
            .find("</style>")
            .expect("every style element the head opens is closed");
        let body = &rest[open + "<style>".len()..open + close];
        assert!(
            !body.contains('<'),
            "a stylesheet body carries markup, so it is not a stylesheet"
        );
        elements += 1;
        rest = &rest[open + close + "</style>".len()..];
    }
    assert!(
        rest.trim().is_empty(),
        "the head carries text outside a stylesheet: {rest:?}"
    );
    assert_eq!(
        elements,
        style_origins().len(),
        "the head does not carry one element per named origin"
    );
}

/// No stylesheet may hide a comment delimiter inside a string.
///
/// `strip_css` runs over every sheet before it is inlined, and it is a
/// plain scanner: it does not know CSS strings. A `content: "/*"` would
/// open a comment it never closes, and the stripper would drop the entire
/// remainder of that file, silently, at runtime only. Nothing in the tree
/// does this today; this is what keeps it that way.
#[test]
fn no_css_string_hides_a_comment_delimiter() {
    for (name, css) in stylesheets() {
        for (n, line) in css.lines().enumerate() {
            let Some(quote) = line.find('"') else {
                continue;
            };
            let rest = &line[quote + 1..];
            let Some(end) = rest.find('"') else { continue };
            let inside = &rest[..end];
            assert!(
                !inside.contains("/*") && !inside.contains("*/"),
                "{name}:{} puts a comment delimiter inside a string, which \
                 strip_css would read as a comment: {line}",
                n + 1
            );
        }
    }
}

/// Stripping removes comments and keeps every declaration.
///
/// The saving is real and worthless if it also removes a rule. Asserted on
/// a shape that has caught the two mistakes a scanner like this makes: a
/// comment between declarations, and one that opens immediately after a
/// value with no space.
#[test]
fn stripping_keeps_the_declarations_and_drops_the_prose() {
    let src = ".a {\n  color: red; /* why red */\n  /* a whole line */\n  gap: 4px;/*tight*/\n}";
    let out = strip_css(src);
    assert!(out.contains("color: red;"), "{out}");
    assert!(out.contains("gap: 4px;"), "{out}");
    assert!(!out.contains("why red"), "{out}");
    assert!(!out.contains("a whole line"), "{out}");
    assert!(!out.contains("tight"), "{out}");

    // Unterminated: keep what is known good, drop the rest, never ship a
    // file whose structure cannot be established.
    assert_eq!(
        strip_css(".a { color: red; } /* oops"),
        ".a { color: red; } "
    );
}

/// The stylesheets are inlined into style elements, so none of them may
/// close one early and dump the remainder into the document as text.
#[test]
fn stylesheets_cannot_break_out_of_their_style_tags() {
    for (name, css) in stylesheets() {
        assert!(
            !css.to_ascii_lowercase().contains("</style"),
            "{name} contains a closing style tag"
        );
    }
}

/// Every stylesheet concatenated, for resolving a token declared in one
/// file and used in another.
///
/// The engine sees one document with one cascade; `--rg-t-fast` is
/// declared in sidebar.css and used in settings.css, and a per-file
/// resolver would find no declaration, leave the `var()` unresolved, and
/// report that settings.css declares no transitions at all. That is a
/// false negative on a guard whose whole job is to catch a duration
/// escaping the reduced-motion block.
fn all_css() -> String {
    stylesheets()
        .iter()
        .map(|(_, css)| *css)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip `/* ... */` blocks so a stylesheet can talk about animation in a
/// comment without tripping the check below.
fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Does any stylesheet carry a rule for exactly this class?
///
/// A plain `css.contains(".rg-foo")` is NOT this question, and the
/// difference is a real escape rather than a nicety: `.rg-foo` is a
/// substring of `.rg-foo2` and of `.rg-foobar`, so renaming a rule out
/// from under live markup, which is precisely the regression the guard
/// below exists to catch, left it green. Proven by mutation: renaming
/// `.rg-launch__branch` to `.rg-launch__branch2` kept the substring form
/// passing while the class rendered with no padding, no colour and no box.
///
/// The concrete exposure this closes, since it reads as academic until you
/// see one: rename `.rg-session__time` to `.rg-session__timestamp`, an
/// ordinary tidy-up. `show_time` still flips, still persists, is still
/// read, still emits its span, and the span renders unstyled. The setting
/// stays functionally wired and becomes visually inert, and nothing in the
/// suite says a word.
///
/// So the character after the name must be one that cannot continue a CSS
/// identifier. Modifiers still resolve, because `.rg-foo--on` contains
/// `.rg-foo` followed by `-`, which ends the base name. That is intended:
/// a modifier without its base is a different defect and the per-module
/// guards own it.
fn styled(css: &str, class: &str) -> bool {
    let needle = format!(".{class}");
    css.match_indices(&needle).any(|(at, _)| {
        css[at + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
    })
}

/// No UI module may emit a class that no stylesheet paints.
///
/// An unstyled class is not an error anywhere: the element renders, with
/// no padding, no colour and no box, and looks like a layout bug rather
/// than a missing rule. `sidebar.rs` has guarded itself against this for a
/// while. Every other module, the settings sheet, both dialogs, the
/// titlebar, the menu, the shortcuts overlay, had no such check at all,
/// which is most of the surfaces in the product.
///
/// The class names are read out of each module's own source rather than a
/// hand-kept list, because a list is exactly the thing that silently stops
/// matching the markup. Only the code above `#[cfg(test)]` is scanned: the
/// test modules below quote these same names as assertion data.
#[test]
fn no_ui_module_emits_an_unpainted_class() {
    let css = strip_css_comments(&all_css());
    // `main.rs` is in this list because the shell's own root element was
    // the one place in the product where an emitted class was never
    // checked against a stylesheet, and it was quietly emitting two
    // `rg-density--*` values that nothing painted. A guard that skips the
    // file it lives in is the complement of a guard that looks wrongly,
    // and both hide the same defect.
    let modules: [(&str, &str); 10] = [
        ("main.rs", include_str!("../main.rs")),
        ("dialog.rs", include_str!("../ui/dialog.rs")),
        ("search.rs", include_str!("../ui/search.rs")),
        ("menu.rs", include_str!("../ui/menu.rs")),
        ("settings.rs", include_str!("../ui/settings.rs")),
        ("shortcuts.rs", include_str!("../ui/shortcuts.rs")),
        ("sidebar.rs", include_str!("../ui/sidebar.rs")),
        ("terminal.rs", include_str!("../ui/terminal.rs")),
        ("titlebar.rs", include_str!("../ui/titlebar.rs")),
        ("workspaces.rs", include_str!("../ui/workspaces.rs")),
    ];

    let mut checked = 0usize;
    for (name, src) in modules {
        // Anchor on the test MODULE, not on the first `#[cfg(test)]`.
        //
        // `main.rs` carries `#[cfg(test)] mod testkit;` near the top, so
        // the short anchor truncates its scan to a couple of dozen lines
        // and every check below passes on an almost empty string. That is
        // how adding main.rs to this array in the first place verified
        // nothing: the unpainted density classes it was added to catch sat
        // far past the cut.
        //
        // The length assertion is the part that matters. An anchor that
        // stops matching degrades to scanning the WHOLE file, which is
        // noisy but safe; an anchor that matches too early degrades to
        // scanning nothing, which is silent. Only the second needs a
        // guard, and a guard against silent collapse does not get an
        // exemption for being that guard.
        let markup = src
            .split_once("\n#[cfg(test)]\nmod tests {")
            .map_or(src, |(before, _)| before);
        // The subject is "the scan still sees markup", not "the scan is
        // big". A byte-fraction threshold is the wrong shape: settings.rs
        // is legitimately more than half tests and would fail a
        // half-the-file rule while scanning perfectly well. Every module
        // in this list emits at least one class, so a scan that finds none
        // has collapsed, whatever its size.
        assert!(
            markup.contains("class: \""),
            "the {name} scan found no markup at all, so it is checking \
             nothing and would pass whatever the file emits"
        );
        for (at, _) in markup.match_indices("class: \"") {
            let rest = &markup[at + 8..];
            let Some(end) = rest.find('"') else { continue };
            for token in rest[..end].split_whitespace() {
                // Interpolated fragments are assembled at runtime and are
                // covered by the tests that own their builders.
                if !token.starts_with("rg-") || token.contains('{') {
                    continue;
                }
                checked += 1;
                assert!(
                    styled(&css, token),
                    "{name} emits .{token} and no stylesheet has a rule for it"
                );
            }
        }
    }
    assert!(
        checked > 100,
        "only {checked} classes scanned; the extraction broke, not the markup"
    );
}

/// A card is exactly `--rg-card-h` tall, whatever it carries.
///
/// This guards the defining layout defect of an earlier build. Row pitch
/// alternated 86 and 68 px down the sidebar, and the cause was not a stray
/// element: line three rendered only when a disposition or completion
/// badge earned it, growing that one card past the height its neighbours
/// had taught the eye to expect. Uneven pitch is a P0 here, because
/// proximity is the only signal saying what belongs to what.
///
/// The first fix MOVED the badge: the card became a two-row grid with line
/// three placed at row 2, column 2. That worked, and this test used to
/// assert exactly those placements. It was an approximation, though: it
/// pinned WHERE each line sits in order to conclude something about the
/// card's HEIGHT, so any restructure that kept the height honest still
/// failed it, and any restructure that broke the height while preserving
/// the placements would still have passed.
///
/// The card now says it directly. A hard `height`, not a `min-height`,
/// means nothing any line contains can grow the box, which makes the
/// original question unaskable rather than merely answered. That also
/// closes a defect this codebase has shipped once already: a `min-height`
/// below the natural height is not a constraint, it is a comment, and
/// `--rg-card-h` sat at 58px against a 66px natural height where it never
/// bound at any value. A hard height cannot be inert.
#[test]
fn a_card_is_one_height_whatever_it_carries() {
    let css = strip_css_comments(&all_css());
    // A selector is declared in more than one sheet here on purpose:
    // spacing owns padding, type owns the font, rows owns placement. The
    // cascade is the union of those blocks, so the test reads the union
    // rather than whichever one happens to appear first.
    let declarations = |selector: &str| -> String {
        let joined: String = css
            .match_indices(selector)
            .map(|(at, _)| {
                let rest = &css[at..];
                &rest[..rest.find('}').expect("unterminated rule")]
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !joined.is_empty(),
            "{selector} is gone; the session card was restructured"
        );
        joined
    };

    let card = declarations(".rg-session--card {");
    assert!(
        card.contains("height: var(--rg-card-h)"),
        "the card no longer declares a hard height from the token, so its \
         contents decide how tall it is and row pitch alternates again: \
         {card}"
    );
    // The distinction that matters. `min-height` is the version that
    // shipped inert, and it is what this assertion exists to reject.
    assert!(
        !card.contains("min-height: var(--rg-card-h)"),
        "the card is back on min-height, which a taller line simply \
         overrides: {card}"
    );
    // A hard height only holds if the box cannot spill. `.rg-session`
    // carries this and has for a while; assert it here because the height
    // above is worthless without it.
    assert!(
        declarations(".rg-session {").contains("overflow: hidden"),
        "the row can overflow, so a hard height clips nothing and a long \
         line escapes the card"
    );
    // Both lines are unconditional now, so no content decides how many
    // there are. That is the structural half of the same guarantee.
    for line in [".rg-session__line--title {", ".rg-session__line--tail {"] {
        let rule = declarations(line);
        assert!(
            !rule.is_empty(),
            "{line} is gone; a card line became conditional again"
        );
    }
}

/// A chord column must be bounded at BOTH ends, or one long chord takes the
/// description column's space or its left edge.
///
/// The rows that pair a chord with a description exist in two layouts and
/// this asserts the bound in each, because the defect below has already been
/// shipped once in the flex one.
///
/// FLEX, in `ui/settings.rs`'s Advanced diagnostics table and
/// `ui/keybinds.rs`: `.rg-keys__chord { flex: 0 0 11.5rem }`. That basis
/// alone did NOT hold the column, because a flex item defaults to
/// `min-width: auto`, which floors it at its own min-content width. The two
/// widest chords in the shortcuts sheet outgrew the basis and pushed their
/// descriptions right: measured on the running binary, one column had every
/// description at x=588 and one at x=638, a left-edge ladder in a design
/// whose stated rule is one left edge per column. The `min-width: 0` floor
/// is the fix and it is invisible until a chord grows, so a future chord
/// rename can reintroduce it silently.
///
/// GRID, in the shortcuts sheet: 18-dialog.css makes `.rg-keys__group` a
/// two-column grid and dissolves the row into it. Here `minmax(0, 11.5rem)`
/// carries both bounds in one track: the 0 is the same floor the flex
/// layout spells `min-width`, and the 11.5rem is a CAP rather than a spend,
/// so a group of short chords hands the slack to its descriptions instead of
/// wrapping them four lines deep. The dissolve is asserted too: without
/// `display: contents` on the row, each row is a single grid item and the
/// template governs nothing.
#[test]
fn the_chord_column_cannot_push_or_starve_the_description_column() {
    let css = strip_css_comments(&all_css());
    // Only where the selector BEGINS a rule. `.rg-keys__row {` is a
    // substring of `.rg-keys__group .rg-keys__row {`, and the last assertion
    // here is precisely that those two blocks say different things, so a
    // plain substring search would compare the scoped rule against itself.
    let block = |selector: &str| -> Vec<&str> {
        css.match_indices(selector)
            .filter(|(at, _)| {
                css[..*at]
                    .trim_end_matches([' ', '\t'])
                    .chars()
                    .next_back()
                    .is_none_or(|c| c == '\n' || c == '}')
            })
            .map(|(at, _)| {
                let rest = &css[at..];
                &rest[..rest.find('}').expect("unterminated rule")]
            })
            .collect()
    };

    // `.rg-keys__chord` is declared in more than one sheet (type sets its
    // font, spacing sets its padding). The block under test is the one that
    // establishes the column, so select by the basis it declares rather than
    // by taking the first match.
    let bodies = block(".rg-keys__chord {");
    assert!(
        !bodies.is_empty(),
        ".rg-keys__chord no longer exists; the chord rows were restructured"
    );
    let column = bodies
        .iter()
        .find(|b| b.contains("flex: 0 0 11.5rem"))
        .unwrap_or_else(|| panic!("no .rg-keys__chord block declares the fixed basis: {bodies:?}"));
    assert!(
        column.contains("min-width: 0"),
        "the chord column declares a fixed basis but no min-width floor, so a \
         chord wider than 11.5rem will shove its description off the column's \
         left edge again: {column}"
    );

    let group = block(".rg-keys__group {")
        .into_iter()
        .find(|b| b.contains("grid-template-columns"))
        .expect("no .rg-keys__group block declares a grid template");
    assert!(
        group.contains("grid-template-columns: minmax(0, 11.5rem) minmax(0, 1fr)"),
        "the shortcuts sheet's chord track lost a bound. minmax(0, 11.5rem) \
         floors it so a long chord wraps inside its own column, and caps it so \
         a group of short chords does not reserve 184px it cannot use: {group}"
    );

    let row = block(".rg-keys__group .rg-keys__row {")
        .into_iter()
        .next()
        .expect("the shortcuts row is not dissolved into its group's grid");
    assert!(
        row.contains("display: contents"),
        "without display: contents a row is one grid item and the chord \
         template governs nothing: {row}"
    );

    // The same class outside a group is still a flex line and must stay one:
    // ui/settings.rs builds the Advanced diagnostics table from bare
    // `.rg-keys__row` elements with no `.rg-keys__group` around them.
    for body in block(".rg-keys__row {") {
        assert!(
            !body.contains("display: contents"),
            "an unscoped .rg-keys__row dissolves the settings diagnostics \
             table into the sheet around it: {body}"
        );
    }
}

/// The comment stripper itself must work, or the animation check below
/// silently passes on a stylesheet that does animate.
#[test]
fn css_comment_stripper_removes_only_comments() {
    assert_eq!(strip_css_comments("a{}/* x */b{}"), "a{}b{}");
    assert_eq!(strip_css_comments("/* no animation: here */"), "");
    assert_eq!(strip_css_comments("a{}/* unterminated"), "a{}");
    assert_eq!(strip_css_comments("no comments"), "no comments");
    assert_eq!(strip_css_comments("/*1*/keep/*2*/this/*3*/"), "keepthis");
}

/// Every `transition:` duration in every stylesheet, in milliseconds.
///
/// Parsed rather than pattern-matched because the rule is about duration,
/// not about the word. A shorthand carries its durations inline
/// (`transition: color 90ms linear, opacity 120ms linear`), and the first
/// time value in each comma-separated part is the duration; a later one
/// would be the delay, which is not what this caps.
fn transition_durations(css: &str) -> Vec<(String, f64)> {
    // The sheet's own declarations first, so a caller passing a snippet
    // with a local token gets that value, then every shipped stylesheet,
    // because `--rg-t-fast` is declared in sidebar.css and used in
    // settings.css and the engine sees one cascade.
    let tokens = strip_css_comments(&format!("{css}\n{}", all_css()));
    let code = resolve_custom_properties_from(&strip_css_comments(css), &tokens);
    let mut out = Vec::new();
    for decl in code.split(';') {
        let decl = decl.rsplit(['{', '}']).next().unwrap_or("");
        let Some((prop, value)) = decl.split_once(':') else {
            continue;
        };
        let prop = prop.trim();
        if prop != "transition" && prop != "transition-duration" {
            continue;
        }
        for part in value.split(',') {
            let first = part.split_whitespace().find_map(parse_time);
            if let Some(ms) = first {
                out.push((part.trim().to_string(), ms));
            }
        }
    }
    out
}

/// Substitute `var(--name)` with the value `--name` was declared as,
/// reading the declarations from `sources` rather than from the text being
/// substituted into.
///
/// Every stylesheet keeps its durations in custom properties, which is the
/// right thing to do and makes a literal scan for "90ms" useless: the
/// declaration reads `transition: color var(--rg-t-fast) linear`. Without
/// this, the duration cap would silently pass on any stylesheet that used
/// a token, which is every stylesheet in this repo.
///
/// The two arguments are separate because a token is often declared in one
/// sheet and used in another, so resolving a sheet against only itself
/// finds no declaration and reports no durations at all.
fn resolve_custom_properties_from(css: &str, sources: &str) -> String {
    // The reduced-motion block redeclares every duration token as `0s`.
    // Those are the zeroed copies, never the live values, and a table
    // built from them reports that a stylesheet transitions in no time at
    // all: source that does not exist. Dropped before anything is read.
    let sources = without_reduced_motion(sources);
    let mut vars: Vec<(String, String)> = Vec::new();
    for decl in sources.split(';') {
        // Drop any selector that came before the last brace, so the ':' of
        // `:root {` is not mistaken for the one separating a declaration.
        let decl = decl.rsplit(['{', '}']).next().unwrap_or("");
        let Some((name, value)) = decl.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.starts_with("--") {
            vars.push((format!("var({name})"), value.trim().to_string()));
        }
    }
    let mut out = css.to_string();
    // Longest name first, so `var(--a-b)` is not partially eaten by a
    // shorter `var(--a)` that happens to be a prefix.
    vars.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    for (needle, value) in vars {
        if out.contains(&needle) {
            out = out.replace(&needle, &value);
        }
    }
    out
}

/// Everything outside `@media (prefers-reduced-motion: reduce)`.
fn without_reduced_motion(css: &str) -> String {
    const AT: &str = "@media (prefers-reduced-motion: reduce)";
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find(AT) {
        out.push_str(&rest[..at]);
        let after = &rest[at + AT.len()..];
        let Some(open) = after.find('{') else { break };
        let mut depth = 0usize;
        let mut end = None;
        for (i, ch) in after[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(end) => rest = &after[end..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// A stylesheet that reads a token it does not declare must resolve to the
/// live value, not to the zero its own reduced-motion block sets.
///
/// This is a false positive that already happened: settings.css uses
/// `var(--rg-t-fast)` declared in sidebar.css, and its only local
/// declaration of that name was the `0s` inside its reduced-motion block,
/// so the guard read every transition in the file as instantaneous and
/// reported source that does not exist.
#[test]
fn a_token_resolves_to_its_live_value_not_its_zeroed_one() {
    let declaring = ":root { --rg-t-fast: 100ms; }";
    let using = ".a { transition: color var(--rg-t-fast) linear; }\n\
                 @media (prefers-reduced-motion: reduce) { :root { --rg-t-fast: 0s; } }";
    let resolved = resolve_custom_properties_from(using, &format!("{using}\n{declaring}"));
    assert!(resolved.contains("color 100ms linear"), "{resolved}");

    assert!(
        !without_reduced_motion(using).contains("0s"),
        "the reduced-motion block was not removed"
    );
    // And the live stylesheets, which is what the guard actually reads.
    //
    // A sheet with no transitions is skipped rather than failed. Several
    // of the design parts own a single concern each and deliberately
    // declare no motion at all; demanding a duration from a file whose
    // job is spacing would only teach its author to add one.
    for (name, css) in stylesheets() {
        let durations = transition_durations(css);
        if durations.is_empty() {
            continue;
        }
        assert!(
            durations.iter().all(|(_, ms)| *ms > 0.0),
            "{name} resolved a live transition to zero: {durations:?}"
        );
    }
}

/// A CSS time token as milliseconds, or `None` when the token is not one.
fn parse_time(token: &str) -> Option<f64> {
    if let Some(n) = token.strip_suffix("ms") {
        return n.parse::<f64>().ok();
    }
    if let Some(n) = token.strip_suffix('s') {
        return n.parse::<f64>().ok().map(|v| v * 1000.0);
    }
    None
}

/// The parser itself must work, or the cap below silently passes on a
/// stylesheet full of one-second transitions.
#[test]
fn transition_parser_reads_durations_and_ignores_delays() {
    assert_eq!(parse_time("90ms"), Some(90.0));
    assert_eq!(parse_time("0.2s"), Some(200.0));
    assert_eq!(parse_time("linear"), None);
    let got =
        transition_durations(".a { transition: color 90ms linear, opacity 0.12s ease 300ms; }");
    let ms: Vec<f64> = got.iter().map(|(_, ms)| *ms).collect();
    assert_eq!(ms, vec![90.0, 120.0], "{got:?}");
    assert!(transition_durations(".a { color: red; }").is_empty());

    // The stylesheets hold their durations in custom properties, so the
    // parser has to resolve them or it measures nothing at all.
    let via_var =
        transition_durations(":root { --t: 120ms; }\n.a { transition: color var(--t) linear; }");
    assert_eq!(
        via_var.iter().map(|(_, ms)| *ms).collect::<Vec<_>>(),
        vec![120.0]
    );
}

/// Longest transition any stylesheet may declare.
///
/// 200ms rather than 150ms, for exactly one case: the status pill's colour
/// change. The pill's word and glyph swap instantly, so the fade is not
/// carrying the information, it is smoothing the surface behind it, and at
/// that job 200ms reads as settling where 90ms reads as a flicker. Anything
/// that IS the feedback still has to be under 150ms, which is what
/// [`MAX_LAYOUT_TRANSITION_MS`] and the individual declarations enforce.
const MAX_TRANSITION_MS: f64 = 200.0;

/// Longest transition allowed on a property that triggers layout.
///
/// Layout is the expensive kind of motion: the pane refits for the whole
/// duration. One property is allowed to do it at all, and it is capped
/// tighter than the paint-only ones.
const MAX_LAYOUT_TRANSITION_MS: f64 = 150.0;

/// The only layout property a stylesheet may transition.
///
/// The sidebar's collapse IS a width change and there is nothing else to
/// animate; translating instead would slide the pane out from under
/// itself. Every other geometric property is banned outright.
const ALLOWED_LAYOUT_PROPERTY: &str = "width";

/// Geometric properties a transition must never name, because animating
/// one reflows the document every frame it runs.
const BANNED_LAYOUT_PROPERTIES: [&str; 8] = [
    "height", "top", "left", "right", "bottom", "margin", "padding", "flex",
];

/// No stylesheet may loop, and no transition may outstay its welcome.
///
/// The two halves are different rules and only one of them is absolute. A
/// LOOPING animation repaints the window at the display's refresh rate for
/// as long as it is on screen, forever on an idle window, and its cost
/// grows with the number of lit rows: that is the specific bug this client
/// exists to avoid and it is banned outright. A ONE-SHOT animation or
/// transition repaints only while a state is changing, costs nothing at
/// rest, and is most of what makes a UI feel finished, so it is allowed
/// and capped.
///
/// Checked against declarations, not prose, so the stylesheets can still
/// document the rule in their own comments.
#[test]
fn stylesheets_never_loop_and_keep_transitions_brief() {
    for (name, css) in stylesheets() {
        let code = strip_css_comments(css);
        assert!(
            !code.contains("infinite"),
            "{name} declares an infinite animation, which repaints the window forever"
        );
        // Every `animation:` shorthand must be paired with an explicit
        // single iteration. Relying on the CSS default of 1 would leave
        // nothing to assert against, and a later edit adding `2` or
        // `infinite` to the shorthand would pass unnoticed.
        let shorthands = code.matches("animation:").count();
        let pinned = code.matches("animation-iteration-count: 1").count();
        assert_eq!(
            shorthands, pinned,
            "{name} has {shorthands} animation shorthands but pins only \
             {pinned} of them to a single iteration"
        );
        for (decl, ms) in transition_durations(css) {
            let names_layout = BANNED_LAYOUT_PROPERTIES
                .iter()
                .find(|prop| decl.split_whitespace().any(|tok| tok == **prop));
            assert_eq!(
                names_layout, None,
                "{name} transitions {names_layout:?} in {decl:?}, which \
                 reflows every frame it runs"
            );
            let cap = if decl
                .split_whitespace()
                .any(|tok| tok == ALLOWED_LAYOUT_PROPERTY)
            {
                MAX_LAYOUT_TRANSITION_MS
            } else {
                MAX_TRANSITION_MS
            };
            assert!(
                ms <= cap,
                "{name} declares a {ms}ms transition in {decl:?}, over the {cap}ms cap"
            );
            assert!(
                ms > 0.0,
                "{name} declares a zero-length transition in {decl:?}"
            );
        }
    }
}

/// A stylesheet that declares motion must honour `prefers-reduced-motion`
/// itself, for what it owns.
///
/// Each file answers for its own motion rather than leaning on another
/// file's `:root` override, which would silently stop honouring the
/// preference the day that file was reorganised.
///
/// The condition matters: this used to demand the block from EVERY sheet,
/// which is wrong twice over. A file with no motion has nothing to zero,
/// and requiring the block anyway teaches authors to paste an empty one,
/// which is worse than no rule at all. It also failed on 10-spacing.css,
/// whose only occurrence of the word "transition" is a comment saying the
/// file deliberately contains none.
#[test]
fn a_stylesheet_with_motion_honours_reduced_motion() {
    for (name, css) in stylesheets() {
        let code = strip_css_comments(css);
        let has_motion = code.contains("transition") || code.contains("animation");
        if !has_motion {
            continue;
        }
        assert!(
            code.contains("@media (prefers-reduced-motion: reduce)"),
            "{name} declares motion but never honours prefers-reduced-motion"
        );
    }
}

/// Every duration token app.css declares must be zeroed by its
/// reduced-motion block. A token added later and left out of that block is
/// motion that keeps running for a reader who asked for none, and it is
/// invisible until someone turns the preference on.
#[test]
fn every_app_duration_token_is_zeroed_under_reduced_motion() {
    let code = strip_css_comments(APP_CSS);
    let (_, reduced) = code
        .split_once("@media (prefers-reduced-motion: reduce)")
        .expect("app.css has a reduced-motion block");
    let declared: Vec<&str> = code
        .split(';')
        .filter_map(|decl| decl.split_once(':'))
        .map(|(name, _)| name.trim())
        .filter(|name| name.starts_with("--rg-t-"))
        .collect();
    assert!(
        !declared.is_empty(),
        "app.css declares no duration tokens, so the motion is hidden in literals"
    );
    for token in declared {
        assert!(
            reduced.contains(&format!("{token}: 0s")),
            "app.css declares {token} but its reduced-motion block does not zero it"
        );
    }
}

/// Every scroller in every shipped stylesheet paints its own background.
///
/// WHY: `overflow: auto` promotes an element to its own composited scrolling
/// layer. WebKit fills such a layer with the engine's base colour, white,
/// for the frames between promotion and the first content paint, so a
/// scroller that inherits its surface visually instead of declaring it
/// flashes white the first time it is shown. `.rg-search__results` did
/// exactly that: opening a search that returned hits flashed the whole
/// window white for roughly one frame at 25fps. `.rg-sheet__body` on the
/// What's New and onboarding sheets and `.rg-sidebar__body` were the same
/// defect waiting for a first paint someone had not happened to film.
///
/// The set is read from `stylesheets()`, the same array the document is
/// built from, so a stylesheet added later is covered without being listed
/// here and a new bare scroller turns this RED rather than shipping.
///
/// What it does NOT catch: a scroller whose declared background is
/// transparent or a colour that does not match the surface behind it. Both
/// are visible in review; neither produces the white frame this closes.
#[test]
fn every_scroller_paints_its_own_background() {
    let mut checked = 0usize;
    for (name, css) in stylesheets() {
        let code = strip_css_comments(css);
        let mut rest = code.as_str();
        while let Some(open) = rest.find('{') {
            let selector = rest[..open].rsplit(['}', ';']).next().unwrap_or("").trim();
            let Some(close) = rest[open + 1..].find('}') else {
                break;
            };
            let body = &rest[open + 1..open + 1 + close];
            rest = &rest[open + 1 + close + 1..];

            // Nested blocks, @media and @supports, hold rules rather than
            // declarations, and their own text carries no overflow.
            let scrolls = body.split(';').any(|decl| {
                let Some((prop, value)) = decl.split_once(':') else {
                    return false;
                };
                matches!(prop.trim(), "overflow" | "overflow-x" | "overflow-y")
                    && matches!(value.trim(), "auto" | "scroll")
            });
            if !scrolls {
                continue;
            }
            checked += 1;
            assert!(
                body.split(';').any(|decl| {
                    decl.split_once(':')
                        .is_some_and(|(prop, _)| prop.trim().starts_with("background"))
                }),
                "{name}: `{selector}` scrolls without painting a background, so its \
                 composited layer shows white until its content arrives"
            );
        }
    }
    assert!(
        checked >= 6,
        "only {checked} scrollers were found, so the parser stopped seeing them \
         and this guard is no longer reading the stylesheets"
    );
}

/// Every keyframe animation in the client is one-shot, and none of them
/// reflows.
///
/// The rule is zero INFINITE animation, not zero animation: an idle window
/// must cost nothing, and a one-shot transition under 150 ms costs one
/// paint. The assertion this replaces forbade `@keyframes` in sidebar.css
/// outright, which contradicted its own name and would have to be relaxed
/// the first time the sidebar wanted a row to announce itself. Checking
/// the property that actually matters is both stricter and stable.
#[test]
fn every_keyframe_animation_is_one_shot_and_composited() {
    let mut found = 0;
    for (name, css) in stylesheets() {
        let code = strip_css_comments(css);

        // Every `animation:` shorthand pins its iteration count. Without
        // the pin a keyframe set with no `to` still repeats forever if the
        // shorthand carries `infinite`, and the pin is what makes that
        // impossible to write by accident.
        assert_eq!(
            code.matches("animation:").count(),
            code.matches("animation-iteration-count: 1").count(),
            "{name} runs an animation without pinning its iteration count"
        );
        assert!(!code.contains("infinite"), "{name} loops an animation");

        // transform and opacity only: both are composited, so an animation
        // cannot trigger layout however many rows wear it at once.
        for (_, body) in keyframe_bodies(&code) {
            for banned in ["width", "height", "margin", "padding", "font-size"] {
                assert!(
                    !body.contains(banned),
                    "{name} animates {banned}, which reflows every frame"
                );
            }
            found += 1;
        }
    }

    // The Woke pulse earns its place because the inbox sort is
    // deliberately static: a woken row reappears exactly where it was, so
    // the badge is the only thing that can announce the return. If it goes
    // missing this test must fail rather than pass vacuously on a document
    // with no animation at all.
    assert!(
        strip_css_comments(APP_CSS).contains("@keyframes rg-woke-pulse"),
        "the Woke pulse is missing"
    );
    assert!(
        found > 0,
        "no keyframes found, so nothing above was checked"
    );
}

/// Each `@keyframes` block in a stylesheet, as (name, body).
///
/// A keyframe body has nested braces, so it cannot be found by splitting
/// on the first `}`; that is what the old single-animation check did, and
/// it read only the first stop.
fn keyframe_bodies(code: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = code;
    while let Some(at) = rest.find("@keyframes") {
        let after = &rest[at + "@keyframes".len()..];
        let Some(open) = after.find('{') else { break };
        let name = after[..open].trim().to_string();
        let mut depth = 0usize;
        let mut end = None;
        for (i, ch) in after[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        out.push((name, after[open + 1..end].to_string()));
        rest = &after[end..];
    }
    out
}

/// The keyframe reader must see past nested braces, or the check above
/// inspects only the first stop of every animation.
#[test]
fn the_keyframe_reader_reads_whole_blocks() {
    let css = "@keyframes a { from { opacity: 0 } to { opacity: 1 } } .x{}";
    let blocks = keyframe_bodies(css);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].0, "a");
    assert!(blocks[0].1.contains("to {"), "{:?}", blocks[0].1);
    assert!(blocks[0].1.contains("opacity: 1"), "{:?}", blocks[0].1);
    assert!(!blocks[0].1.contains(".x"), "read past the block");

    assert_eq!(keyframe_bodies("no animation here").len(), 0);
    assert_eq!(
        keyframe_bodies("@keyframes a{from{opacity:0}}@keyframes b{to{opacity:1}}").len(),
        2
    );
}

/// The product must actually use motion now that it is allowed.
///
/// A cap nobody is near is a rule with nothing behind it, and the point of
/// allowing one-shot transitions was that the shell felt unfinished
/// without them.
///
/// Asserted ACROSS the cascade rather than per file. Per file it demanded
/// a transition from every stylesheet, which is the opposite of what this
/// codebase wants: the design layer is split so each part owns one
/// concern, and the file that owns spacing should contain no motion at
/// all. The thing worth protecting is that the product as a whole has not
/// quietly reverted to being motionless.
#[test]
fn the_product_uses_the_motion_it_is_allowed() {
    let total: usize = stylesheets()
        .iter()
        .map(|(_, css)| transition_durations(css).len())
        .sum();
    assert!(
        total > 0,
        "no stylesheet in the cascade declares a transition"
    );
}

/// Frame headers are decoded once, by the crate that defines them.
///
/// Two decoders for one wire format drift, and the failure is a stray byte
/// at the head of a line, which corrupts an escape sequence and is very
/// hard to trace back here. The socket hands `vitrum_proto`'s decoder the
/// bytes, so this asserts the client keeps no second copy of that
/// arithmetic.
///
/// What this does not catch: a decoder written in another module. It reads
/// the file that carries the data plane, which is where one would land.
#[test]
fn the_client_decodes_frames_only_through_the_protocol_crate() {
    let socket = include_str!("../socket.rs");
    assert!(
        socket.contains("OUTPUT_HEADER_LEN"),
        "the socket no longer names the protocol crate's header length, so \
         it is finding the payload some other way"
    );
    for hand_rolled in [
        "OUTPUT_HEADER_LEN = 17".to_string(),
        "OUTPUT_HEADER_LEN: usize = 17".to_string(),
        format!("[{}..]", vitrum_proto::OUTPUT_HEADER_LEN),
    ] {
        assert!(
            !socket.contains(&hand_rolled),
            "socket.rs parses the output header itself with `{hand_rolled}`, \
             so the wire format now has two decoders that can disagree"
        );
    }
}
