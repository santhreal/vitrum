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
/// than the `tao` `Icon` because the raster is plain bytes that can cross to
/// the prewarm thread.
#[test]
fn the_window_mark_is_rasterised_once_for_the_process() {
    for _ in 0..8 {
        assert!(
            chrome::window_icon().is_some(),
            "the rasteriser handed back a buffer tao rejected"
        );
    }
    assert_eq!(
        chrome::mark_rasterisations(),
        1,
        "the window mark was drawn more than once"
    );
}

/// The document head is one string, built once, whoever asks first.
///
/// It is now built on the prewarm thread while the main thread brings up the
/// toolkit. Both call the same one-shot cache, so the guarantee that matters
/// is that a second caller gets the same bytes and pays nothing: a per-window
/// rebuild would copy 800 KB of vendored script for every window that opens.
#[test]
fn the_document_head_is_built_once_and_shared() {
    let opts = Options::parse(std::iter::empty()).expect("no arguments is a valid command line");
    let first = chrome::document_head(opts);
    let second = chrome::document_head(opts);
    assert!(
        std::ptr::eq(first, second),
        "the document head was rebuilt for a second caller"
    );
    assert!(
        first.contains("window.__vitrum_keymap="),
        "the head lost the operator's keymap; every rebound chord would be dead \
         until the mount-time push landed"
    );
    assert!(
        first.contains("window.__vitrum_bootDelayMs="),
        "the head lost the boot splash's timer"
    );
}
