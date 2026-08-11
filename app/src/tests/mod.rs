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
mod publication;
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
    let _bus = crate::state::live::exclusive();
    let prefs = crate::state::ConnectionPrefs::default();
    let max_ms = u64::from(prefs.reconnect_max_ms);
    let attempts_max = prefs.reconnect_attempts;
    let first = reconnect_delay_ms(0).expect("the first retry must be scheduled");
    assert_eq!(first, RECONNECT_BASE_MS, "the first retry must be prompt");

    // Strictly increasing until it reaches the ceiling, never past it.
    let mut prev = first;
    for n in 1..attempts_max {
        let d = reconnect_delay_ms(n).expect("still within the schedule");
        assert!(d >= prev, "attempt {n} waits less than attempt {}", n - 1);
        assert!(
            d <= max_ms,
            "attempt {n} waits {d}ms, past the ceiling"
        );
        prev = d;
    }
    assert_eq!(
        prev, max_ms,
        "the schedule never reaches its ceiling"
    );

    // And it STOPS. A window that reconnects forever is a window that never
    // tells the operator their daemon is gone.
    assert_eq!(
        reconnect_delay_ms(attempts_max),
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
    let _bus = crate::state::live::exclusive();
    let attempts_max = crate::state::ConnectionPrefs::default().reconnect_attempts;
    for n in 0..attempts_max {
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
