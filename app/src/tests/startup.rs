//! Guards on the path from process start to a usable window.
//!
//! # Why none of these is a timer
//!
//! What a start costs is a property of the machine: the disk it read the
//! profile off, whether the toolkit's caches are warm, what else is compiling
//! at the time. A test that asserted "under 250 ms" would pass on an idle
//! desktop, fail on a loaded builder, and teach everyone to rerun it until it
//! went green, which is the same as deleting it.
//!
//! What these assert instead is the shape of the work: how many times the
//! profile is parsed, how many times the mark is drawn, and whether the
//! document is assembled once or per window. Those numbers do not move under
//! load, they go red the moment somebody puts the work back, and they are the
//! numbers the measured improvement was made of.
//!
//! The timings themselves live in the trace `VITRUM_BOOT_TRACE=1` emits, which
//! also ends a run by printing these same counters, so the claim can be
//! checked on a real machine and not only here.

use super::*;

/// The profile is parsed once, however many times the startup path asks.
///
/// The defect: `ui.json` was read three times before the first frame. Once by
/// `TRANSLUCENT`, deciding whether to create the window see-through. Once by
/// `document_head`, to inline the operator's keymap. Once again by the first
/// window's mount, to restore the workspaces. Two of those ran on the main
/// thread with no window on screen yet, and all three parsed the same bytes,
/// because nothing writes that file between process start and the first paint.
///
/// Absolute and not a delta. Every test in this binary shares one process, so
/// a before-and-after around `startup_prefs` would race whatever else is
/// running; the count of times the SNAPSHOT went to disk can only ever be
/// zero or one, whoever asks and in what order.
#[test]
fn the_startup_profile_is_parsed_once_however_often_it_is_asked_for() {
    for _ in 0..64 {
        let _ = state::startup_prefs();
    }
    assert_eq!(
        state::startup_prefs_loads(),
        1,
        "the startup profile snapshot went to disk more than once"
    );
}

/// `load_prefs` is still the uncached door, and the counter still sees it.
///
/// Without this the guard above would also pass if `load_prefs` had quietly
/// become a cache, which would break every caller that has to observe a
/// setting the operator just saved.
#[test]
fn a_direct_load_still_reads_the_file() {
    let before = state::prefs_loads();
    let _ = state::load_prefs();
    let _ = state::load_prefs();
    assert!(
        state::prefs_loads() >= before + 2,
        "load_prefs stopped reading the file; a saved setting would not be seen"
    );
}

/// The mark is rasterised once for the process, not once per window.
///
/// Twenty windows drew twenty identical 128x128 rasters, each one on the main
/// thread in the middle of building that window. The raster is cached rather
/// than the toolkit's icon object because the raster is plain bytes that can
/// cross to the prewarm thread.
#[test]
fn the_window_mark_is_rasterised_once_for_the_process() {
    for _ in 0..8 {
        chrome::warm_window_icon();
    }
    assert_eq!(
        chrome::mark_rasterisations(),
        1,
        "the window mark was drawn more than once"
    );
}

/// Every phase of a start names the phase that must precede it.
///
/// Derived from `PHASES` at run time rather than listed here, so a phase
/// added with no prerequisite turns this red instead of silently opting out
/// of the ordering rule. `process.start` is the only row allowed to have
/// none, because it is the first thing the process does.
#[test]
fn every_boot_phase_but_the_first_states_what_precedes_it() {
    let mut placed = vec![];
    for (name, needs) in boot::PHASES {
        match needs {
            None => assert_eq!(
                name, "process.start",
                "{name} is traced with no prerequisite, so nothing states when \
                 it is allowed to happen"
            ),
            Some(needs) => assert!(
                placed.contains(&needs),
                "{name} requires {needs}, which PHASES lists after it"
            ),
        }
        placed.push(name);
    }
    assert!(
        placed.len() > 1,
        "PHASES is empty, so the ordering guard checks nothing"
    );
}

/// The stylesheets are finished before the window exists.
///
/// The defect this closes: the document was assembled between the window
/// being created and its first frame, so the operator watched an empty
/// rectangle for the length of a several-hundred-kilobyte string build. The
/// build now happens on the prewarm thread and the window is made from the
/// finished string.
#[test]
fn a_window_is_not_created_before_its_stylesheets_are_built() {
    assert!(
        boot::out_of_order(&["styles.built", "window.created"]).is_empty(),
        "the documented order was rejected by the guard that enforces it"
    );
    let backwards = boot::out_of_order(&["window.created"]);
    assert_eq!(
        backwards.len(),
        1,
        "a window created before its stylesheets were built was accepted"
    );
    assert!(
        backwards[0].contains("styles.built"),
        "the violation does not name the phase that was skipped: {}",
        backwards[0]
    );
}

/// The pane paints after the shell is mounted, and neither one waits on a
/// frame that was never realized.
///
/// Two separate things used to be conflated under "the pane must not wait".
///
/// The pane is INSTALLED with the window, at `window.created`, and that is
/// what stops it dropping output: from that point it holds a grid and parses
/// every byte the socket hands it, whether or not anything is on screen. That
/// has not changed and is not what this phase measures.
///
/// The pane PAINTS later, and deliberately after `shell.mounted`. Its first
/// frame needs a GPU instance, an adapter, a device, a configured swapchain
/// and a compiled pipeline, which on a cold start is most of a quarter of a
/// second. Done in the realize handler, all of it landed inside `show_all`,
/// so the whole window — sidebar, titlebar, bar, none of which use a GPU —
/// waited on it, and the operator saw nothing at all until it finished. The
/// handshake now runs from an idle below the toolkit's redraw priority, so
/// this mark cannot arrive before `shell.mounted` unless it has been put back
/// in front of the first frame.
#[test]
fn the_pane_paints_after_the_window_it_paints_into_is_on_screen() {
    assert!(
        boot::out_of_order(&["window.created", "frame.realized", "shell.mounted"]).is_empty(),
        "the shell's own chain was rejected by the guard that enforces it"
    );
    assert!(
        boot::out_of_order(&[
            "window.created",
            "frame.realized",
            "shell.mounted",
            "pane.first-paint",
        ])
        .is_empty(),
        "the documented order was rejected by the guard that enforces it"
    );
    let early = boot::out_of_order(&["window.created", "frame.realized", "pane.first-paint"]);
    assert_eq!(
        early.len(),
        1,
        "a pane that painted before the shell mounted was accepted, which is \
         the GPU handshake back in front of the window's first frame"
    );
    assert!(
        early[0].contains("shell.mounted"),
        "the violation does not name the phase that was skipped: {}",
        early[0]
    );
    for phase in ["frame.realized", "pane.first-paint"] {
        assert_eq!(
            boot::out_of_order(&[phase]).len(),
            1,
            "{phase} was accepted before anything it depends on existed"
        );
    }
    assert_eq!(
        boot::out_of_order(&["window.created", "shell.mounted"]).len(),
        1,
        "a shell mounted into a frame that was never realized was accepted"
    );
}

/// A start fast enough not to need the mark does not show it.
///
/// The defect: a splash on a fast start is a flicker, and a flicker reads as
/// a fault. The threshold and the switch are both the operator's, so this
/// drives the same pure function the draw handler calls.
#[test]
fn the_mark_is_not_painted_on_a_start_that_beats_the_threshold() {
    let prefs = state::StartupPrefs {
        show_splash: true,
        splash_after_ms: 120,
    };
    assert!(
        !splash::should_paint(0, prefs),
        "the mark was painted at the instant of exec"
    );
    assert!(
        !splash::should_paint(119, prefs),
        "the mark was painted one millisecond before its own threshold"
    );
    assert!(
        splash::should_paint(120, prefs),
        "a start that reached the threshold showed no mark at all"
    );
    assert!(
        !splash::should_paint(10_000, state::StartupPrefs {
            show_splash: false,
            splash_after_ms: 120,
        }),
        "the mark was painted after the operator turned it off"
    );
}
