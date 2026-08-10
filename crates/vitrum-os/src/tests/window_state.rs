//! Window geometry: clamping onto the live monitor layout, and persistence.
//!
//! Every clamping assertion here names exact coordinates. "The window is
//! on screen" is not a testable property; "x is 640 and not 3200" is.

use crate::tests::support::TempDir;
use crate::window_state::{
    self, DEFAULT_HEIGHT, DEFAULT_SIDEBAR_WIDTH, DEFAULT_WIDTH, MAX_SIDEBAR_WIDTH, MIN_HEIGHT,
    MIN_SIDEBAR_WIDTH, MIN_WIDTH, Monitor, STATE_FORMAT_VERSION, StateLoad, WindowState,
    clamp_sidebar, clamp_to_monitors,
};

fn primary() -> Monitor {
    Monitor::new(0, 0, 1920, 1080)
}

/// A saved rectangle that still fits must come back untouched.
///
/// The clamp runs on every launch. If it nudged a perfectly valid window the
/// user's layout would drift a few pixels every restart, which is worse than
/// not persisting at all.
#[test]
fn a_valid_rectangle_is_returned_unchanged() {
    let saved = WindowState { x: 100, y: 80, width: 1280, height: 800, maximized: false, sidebar_width: 300 };
    assert_eq!(clamp_to_monitors(&saved, &[primary()]), saved);
}

/// A window saved on a monitor that no longer exists must land on the primary,
/// at exact coordinates.
///
/// This is the feature. Undock a laptop that had a screen at x=1920 and the
/// saved x=2400 is off the right edge of the only remaining display: invisible,
/// unreachable, and impossible to drag back because there is no title bar on
/// screen to grab.
#[test]
fn a_window_from_a_vanished_monitor_lands_on_the_primary() {
    let saved =
        WindowState { x: 2400, y: 200, width: 1280, height: 800, maximized: false, sidebar_width: 280 };
    let clamped = clamp_to_monitors(&saved, &[primary()]);
    assert_eq!(
        clamped,
        WindowState {
            // 1920 - 1280 is the rightmost position that keeps the window whole.
            x: 640,
            y: 200,
            width: 1280,
            height: 800,
            maximized: false,
            sidebar_width: 280,
        }
    );
}

/// A window saved above and left of every monitor must be pulled to the origin
/// of the primary.
///
/// Negative coordinates are legitimate in a multi-monitor layout with a screen
/// to the left, so they must be preserved when that screen exists and corrected
/// when it does not.
#[test]
fn a_window_saved_off_the_top_left_is_pulled_to_the_origin() {
    let saved = WindowState {
        x: -3000,
        y: -2000,
        width: 1000,
        height: 700,
        maximized: false,
        sidebar_width: 280,
    };
    let clamped = clamp_to_monitors(&saved, &[primary()]);
    assert_eq!(clamped.x, 0);
    assert_eq!(clamped.y, 0);
    assert_eq!(clamped.width, 1000);
    assert_eq!(clamped.height, 700);
}

/// Negative coordinates must survive when the monitor at those coordinates is
/// still there.
///
/// A second display placed to the left of the primary has negative x in every
/// window system. Clamping everything to non-negative would move the window off
/// that display on every launch.
#[test]
fn negative_coordinates_survive_when_their_monitor_exists() {
    let left = Monitor::new(-1920, 0, 1920, 1080);
    let saved =
        WindowState { x: -1800, y: 50, width: 1000, height: 700, maximized: false, sidebar_width: 280 };
    assert_eq!(clamp_to_monitors(&saved, &[primary(), left]), saved);
}

/// The monitor with the largest shared area wins, not the first one that
/// touches.
///
/// A window straddling two displays belongs to the one showing most of it. The
/// naive "first intersecting monitor" rule would yank a window that is 90% on
/// the second display back onto the first.
#[test]
fn the_monitor_with_the_most_overlap_is_chosen() {
    let right = Monitor::new(1920, 0, 1920, 1080);
    // 200 columns on the primary, 1080 on the right monitor.
    let saved =
        WindowState { x: 1720, y: 0, width: 1280, height: 800, maximized: false, sidebar_width: 280 };
    let clamped = clamp_to_monitors(&saved, &[primary(), right]);
    assert_eq!(clamped.x, 1920, "the window belongs to the right monitor and is pushed onto it");
    assert_eq!(clamped.y, 0);
    assert_eq!(clamped.width, 1280);
}

/// A tie in shared area goes to the earlier monitor, which callers pass primary
/// first.
///
/// Without a defined tie-break the chosen monitor depends on iteration order
/// and a window centred on a seam jumps between displays on alternating
/// launches.
#[test]
fn an_exact_tie_goes_to_the_earlier_monitor() {
    let right = Monitor::new(1920, 0, 1920, 1080);
    // Exactly 640 columns on each.
    let saved =
        WindowState { x: 1280, y: 0, width: 1280, height: 800, maximized: false, sidebar_width: 280 };
    let clamped = clamp_to_monitors(&saved, &[primary(), right]);
    assert_eq!(clamped.x, 640, "clamped into the primary, which was listed first");
}

/// A window larger than its monitor must be shrunk to the monitor and placed at
/// its origin.
///
/// Restoring a 3840-wide window saved on a 4K screen onto a 1366-wide laptop
/// panel would otherwise put three quarters of the UI past the right edge.
#[test]
fn a_window_larger_than_the_monitor_is_shrunk_to_fit() {
    let laptop = Monitor::new(0, 0, 1366, 768);
    let saved = WindowState {
        x: 0,
        y: 0,
        width: 3840,
        height: 2160,
        maximized: false,
        sidebar_width: 280,
    };
    let clamped = clamp_to_monitors(&saved, &[laptop]);
    assert_eq!(clamped.width, 1366);
    assert_eq!(clamped.height, 768);
    assert_eq!(clamped.x, 0);
    assert_eq!(clamped.y, 0);
}

/// A monitor smaller than the minimum window size must win over the minimum.
///
/// The minimum exists so the UI is usable, but a window wider than its screen
/// is not usable at all. Applying the minimum after the monitor clamp would
/// produce a 720-wide window on a 640-wide display.
#[test]
fn a_monitor_smaller_than_the_minimum_still_bounds_the_window() {
    let tiny = Monitor::new(0, 0, 640, 400);
    let saved = WindowState { x: 0, y: 0, width: 100, height: 100, maximized: false, sidebar_width: 280 };
    let clamped = clamp_to_monitors(&saved, &[tiny]);
    assert_eq!(clamped.width, 640);
    assert_eq!(clamped.height, 400);
}

/// A degenerate saved size must be raised to the minimum.
///
/// A zero comes from a corrupt file or a compositor that reported a hidden
/// window. Restoring it produces an invisible window and a support ticket.
#[test]
fn a_zero_size_is_raised_to_the_minimum() {
    let saved = WindowState { x: 10, y: 10, width: 0, height: 0, maximized: false, sidebar_width: 280 };
    let clamped = clamp_to_monitors(&saved, &[primary()]);
    assert_eq!(clamped.width, MIN_WIDTH);
    assert_eq!(clamped.height, MIN_HEIGHT);
}

/// With no monitors reported, geometry must be discarded for the default.
///
/// Zero outputs happens on a headless start and during a hotplug. There is
/// nothing to validate against, so trusting the saved rectangle is a coin flip;
/// the default at the origin is at least always reachable.
#[test]
fn no_monitors_yields_the_default_geometry_at_the_origin() {
    let saved = WindowState {
        x: 5000,
        y: 5000,
        width: 300,
        height: 200,
        maximized: true,
        sidebar_width: 420,
    };
    let clamped = clamp_to_monitors(&saved, &[]);
    assert_eq!(
        clamped,
        WindowState {
            x: 0,
            y: 0,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            // Neither of these depends on a monitor, so both survive.
            maximized: true,
            sidebar_width: 420,
        }
    );
}

/// The maximised flag must survive clamping.
///
/// The stored rectangle is the restore geometry. Losing the flag means a user
/// who quit maximised reopens windowed every time.
#[test]
fn the_maximized_flag_survives_clamping() {
    let saved =
        WindowState { x: 9000, y: 0, width: 1280, height: 800, maximized: true, sidebar_width: 280 };
    assert!(clamp_to_monitors(&saved, &[primary()]).maximized);
}

/// The sidebar must never grow past the maximum or shrink below the minimum.
#[test]
fn the_sidebar_is_clamped_to_its_range() {
    assert_eq!(clamp_sidebar(10, 1920), MIN_SIDEBAR_WIDTH);
    assert_eq!(clamp_sidebar(9999, 1920), MAX_SIDEBAR_WIDTH);
    assert_eq!(clamp_sidebar(300, 1920), 300);
}

/// The terminal keeps its minimum width even when that squeezes the sidebar
/// below its own minimum.
///
/// On a 640-wide window, 180 of sidebar plus 360 of content is 540, so the
/// sidebar can be 280. On a 500-wide window it cannot be 180 and 360 at once,
/// and a terminal too narrow to render a prompt is the worse failure.
#[test]
fn the_terminal_minimum_wins_over_the_sidebar_minimum() {
    assert_eq!(clamp_sidebar(280, 640), 280);
    assert_eq!(clamp_sidebar(280, 500), 140);
    assert_eq!(clamp_sidebar(280, 360), 0);
    assert_eq!(clamp_sidebar(280, 100), 0);
}

/// Clamping the window also clamps the sidebar against the clamped width.
///
/// Clamping the sidebar against the *saved* width would leave a 520-wide
/// sidebar in a window that just shrank to 640.
#[test]
fn the_sidebar_is_clamped_against_the_clamped_width() {
    let tiny = Monitor::new(0, 0, 800, 600);
    let saved = WindowState {
        x: 0,
        y: 0,
        width: 2000,
        height: 1500,
        maximized: false,
        sidebar_width: MAX_SIDEBAR_WIDTH,
    };
    let clamped = clamp_to_monitors(&saved, &[tiny]);
    assert_eq!(clamped.width, 800);
    assert_eq!(clamped.sidebar_width, 440, "800 minus the 360 the terminal keeps");
}

/// Extreme coordinates must not overflow the overlap arithmetic.
///
/// `i32::MAX + width` leaves `i32` range. A corrupt or hostile state file is
/// exactly where that arrives, and an overflow panic on launch is an app that
/// cannot start until someone finds and deletes a JSON file.
#[test]
fn extreme_coordinates_do_not_overflow() {
    let saved = WindowState {
        x: i32::MAX,
        y: i32::MIN,
        width: u32::MAX,
        height: u32::MAX,
        maximized: false,
        sidebar_width: u32::MAX,
    };
    let clamped = clamp_to_monitors(&saved, &[primary()]);
    assert_eq!(clamped.x, 0);
    assert_eq!(clamped.y, 0);
    assert_eq!(clamped.width, 1920);
    assert_eq!(clamped.height, 1080);
    assert_eq!(clamped.sidebar_width, MAX_SIDEBAR_WIDTH);
}

/// A missing file is a first launch, distinct from every failure.
#[test]
fn a_missing_file_reads_as_missing() {
    let dir = TempDir::new("ws-missing");
    assert_eq!(window_state::load(&dir.join("nope.json")), StateLoad::Missing);
}

/// A missing file must report no problem, because it is not one.
#[test]
fn a_missing_file_is_not_a_problem() {
    assert_eq!(StateLoad::Missing.problem(), None);
}

/// A state file larger than any layout is refused without being read whole.
///
/// WHY: `load` used `read_to_string`, so whatever could write this path chose
/// the client's first allocation of the launch. The path is under the user's
/// state directory, which makes it a log somebody redirected, a symlink, or a
/// file a crashed writer left growing — not an attacker on the network, and
/// not a reason to allocate a gigabyte before looking at a single field.
///
/// It does not check the peak allocation, which no test here can observe. It
/// checks the decision: over the bound is corrupt, and the file is still
/// judged rather than defaulted away.
#[test]
fn a_state_file_larger_than_the_bound_reads_as_corrupt() {
    let dir = TempDir::new("ws-huge");
    let path = dir.join("windows.json");
    let mut text = String::from("{\"version\":1,\"pad\":\"");
    text.push_str(&"p".repeat(2 * 1024 * 1024));
    text.push_str("\"}");
    std::fs::write(&path, &text).expect("a temp file is writable");

    match window_state::load(&path) {
        StateLoad::Corrupt { detail } => assert!(
            detail.contains("larger than"),
            "the refusal names the bound, got {detail}"
        ),
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

/// A file just under the bound is still parsed, so the bound is not a
/// second, stricter format rule.
#[test]
fn a_state_file_under_the_bound_is_still_read() {
    let dir = TempDir::new("ws-large-ok");
    let path = dir.join("windows.json");
    let saved =
        WindowState { x: 1, y: 2, width: 800, height: 600, maximized: false, sidebar_width: 200 };
    let encoded = window_state::encode(&saved);
    // Whitespace to within a few bytes of the bound: valid JSON, nearly the
    // largest file that is allowed to load.
    let padded = format!("{encoded}{}", " ".repeat((1 << 20) - encoded.len() - 16));
    std::fs::write(&path, padded).expect("a temp file is writable");

    assert_eq!(window_state::load(&path), StateLoad::Loaded(saved));
}

/// Truncated JSON must be reported as corrupt, not silently defaulted.
///
/// Silent defaulting is how a product loses a user's layout every launch
/// without ever telling them there is a broken file to delete.
#[test]
fn truncated_json_reads_as_corrupt() {
    let load = window_state::parse("{\"version\":1,\"x\":0,");
    match &load {
        StateLoad::Corrupt { detail } => assert!(
            detail.contains("EOF") || detail.contains("eof"),
            "detail should name the parse failure, got {detail}"
        ),
        other => panic!("expected Corrupt, got {other:?}"),
    }
    assert!(load.problem().is_some());
}

/// A JSON value that is not an object must be reported with what it was.
#[test]
fn a_non_object_document_reads_as_corrupt() {
    assert_eq!(
        window_state::parse("[]"),
        StateLoad::Corrupt { detail: "expected a JSON object, found an array".to_string() }
    );
    assert_eq!(
        window_state::parse("42"),
        StateLoad::Corrupt { detail: "expected a JSON object, found a number".to_string() }
    );
}

/// A document with no version must be corrupt, not assumed current.
///
/// Assuming the current version means a hand-written or foreign file is
/// deserialised into whatever fields happen to match.
#[test]
fn a_document_without_a_version_reads_as_corrupt() {
    assert_eq!(
        window_state::parse("{\"x\":0}"),
        StateLoad::Corrupt { detail: "missing `version` field".to_string() }
    );
}

/// A newer format version must be reported as such, not parsed hopefully.
#[test]
fn a_newer_format_version_is_reported() {
    let load = window_state::parse("{\"version\":99,\"x\":0,\"y\":0}");
    assert_eq!(load, StateLoad::UnsupportedVersion { found: 99 });
    assert_eq!(
        load.problem().unwrap(),
        "window state is format version 99, this build understands 1"
    );
}

/// A file with the right version but missing fields is corrupt.
#[test]
fn missing_fields_read_as_corrupt() {
    let load = window_state::parse("{\"version\":1,\"x\":0}");
    assert!(matches!(load, StateLoad::Corrupt { .. }), "got {load:?}");
}

/// Every load outcome must still produce a usable, clamped state.
///
/// The caller must never be handed an unclamped rectangle, whatever went wrong,
/// because the whole point of the module is that no path ends with a window
/// off screen.
#[test]
fn every_failure_resolves_to_a_clamped_default() {
    let tiny = Monitor::new(0, 0, 800, 600);
    for load in [
        StateLoad::Missing,
        StateLoad::Corrupt { detail: "x".to_string() },
        StateLoad::Unreadable { detail: "x".to_string() },
        StateLoad::UnsupportedVersion { found: 2 },
    ] {
        let state = load.resolve(&[tiny]);
        assert_eq!(state.width, 800, "{load:?} must be clamped to the monitor");
        assert_eq!(state.height, 600);
        assert_eq!(state.x, 0);
        assert_eq!(state.sidebar_width, DEFAULT_SIDEBAR_WIDTH);
    }
}

/// A successful load must resolve to the saved state, clamped.
#[test]
fn a_loaded_state_resolves_to_itself_when_it_fits() {
    let saved =
        WindowState { x: 40, y: 40, width: 1000, height: 700, maximized: false, sidebar_width: 250 };
    assert_eq!(StateLoad::Loaded(saved).resolve(&[primary()]), saved);
}

/// A save must round-trip exactly through a real file.
#[test]
fn a_saved_state_round_trips_through_the_filesystem() {
    let dir = TempDir::new("ws-round-trip");
    let path = dir.join("window.json");
    let saved =
        WindowState { x: -12, y: 34, width: 1111, height: 777, maximized: true, sidebar_width: 321 };
    window_state::save(&path, &saved).expect("save must succeed in a temp dir");
    assert_eq!(window_state::load(&path), StateLoad::Loaded(saved));
}

/// Saving must create the state directory rather than failing on a first run.
#[test]
fn saving_creates_the_state_directory() {
    let dir = TempDir::new("ws-mkdir");
    let path = dir.join("nested/deeper/window.json");
    window_state::save(&path, &WindowState::default()).expect("parents must be created");
    assert!(path.exists());
}

/// Saving must leave no temporary file behind.
///
/// The write is atomic via a temp file plus rename. A leftover `window.json.tmp`
/// means the rename did not happen and the next launch reads a stale file.
#[test]
fn saving_leaves_no_temporary_file() {
    let dir = TempDir::new("ws-atomic");
    let path = dir.join("window.json");
    window_state::save(&path, &WindowState::default()).expect("save must succeed");
    assert!(!dir.join("window.json.tmp").exists());
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .expect("temp dir is readable")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(entries, vec!["window.json".to_string()]);
}

/// Overwriting an existing state file must replace it, not fail.
///
/// `rename` over an existing destination is the case Windows historically
/// refused. This pins that saving twice works.
#[test]
fn saving_twice_replaces_the_file() {
    let dir = TempDir::new("ws-overwrite");
    let path = dir.join("window.json");
    window_state::save(&path, &WindowState::default()).expect("first save");
    let second =
        WindowState { x: 7, y: 8, width: 900, height: 600, maximized: false, sidebar_width: 200 };
    window_state::save(&path, &second).expect("second save must replace the first");
    assert_eq!(window_state::load(&path), StateLoad::Loaded(second));
}

/// The on-disk shape is a contract; pin it exactly.
///
/// A field rename is a silent data loss for everyone who upgrades, and the
/// version number exists precisely so that such a change is detected.
#[test]
fn the_encoded_form_is_exactly_this() {
    let state =
        WindowState { x: 1, y: 2, width: 3, height: 4, maximized: true, sidebar_width: 5 };
    assert_eq!(
        window_state::encode(&state),
        "{\n  \"version\": 1,\n  \"x\": 1,\n  \"y\": 2,\n  \"width\": 3,\n  \"height\": 4,\n  \"maximized\": true,\n  \"sidebarWidth\": 5\n}"
    );
    assert_eq!(STATE_FORMAT_VERSION, 1);
}

/// The default state must itself be valid under the clamp.
///
/// A default that the clamp would modify means the very first launch is already
/// inconsistent with what gets saved.
#[test]
fn the_default_survives_its_own_clamp() {
    let default = WindowState::default();
    assert_eq!(clamp_to_monitors(&default, &[primary()]), default);
    assert_eq!(default.sidebar_width, DEFAULT_SIDEBAR_WIDTH);
}
