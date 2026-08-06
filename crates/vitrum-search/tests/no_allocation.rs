//! Proof that a scan does not allocate per line.
//!
//! This is a hard requirement rather than a nicety: a live search box reissues
//! the query on every keystroke, over twenty sessions of ten megabytes. At
//! roughly eighty bytes per line that is 2.6 million lines per session and 52
//! million lines per query. One allocation per line would be 52 million
//! malloc/free pairs per keystroke, which is not slow — it is a different
//! product.
//!
//! # How this is measured
//!
//! A counting [`GlobalAlloc`] wraps the system allocator and increments on
//! every `alloc`, `alloc_zeroed` and `realloc`. Counting is gated behind an
//! atomic flag so only the measured window contributes.
//!
//! **This file deliberately contains exactly one `#[test]`.** The counter is
//! process-global, so a second test running concurrently in the same binary
//! would allocate into it and make the measurement noise. The rest of the
//! suite lives in `scrollback.rs`.
//!
//! Two assertions, because either alone is weak:
//!
//! - An absolute bound. Scanning hundreds of thousands of lines must cost a
//!   two-digit number of allocations, which no per-line implementation can
//!   reach.
//! - A doubling comparison. Twice the lines must cost the same, within a small
//!   constant. This is the one that survives any fixed overhead the harness
//!   contributes and pins the *slope* at zero rather than just the intercept.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use vitrum_search::matcher::Matcher;
use vitrum_search::{Haystack, Query, Sweep, search_with};

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Run `body` with allocation counting on, and report the count.
fn measure(body: impl FnOnce()) -> usize {
    ALLOCATIONS.store(0, Ordering::SeqCst);
    COUNTING.store(true, Ordering::SeqCst);
    body();
    COUNTING.store(false, Ordering::SeqCst);
    ALLOCATIONS.load(Ordering::SeqCst)
}

/// Scrollback with the awkward shapes mixed in: plain lines, SGR-coloured
/// lines, an OSC title, a long line and an empty one.
///
/// Colour matters here — a coloured line takes the stripping path, which has
/// its own scratch buffers, and those are exactly what a naive implementation
/// would reallocate every line.
fn scrollback(lines: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(lines * 72);
    for index in 0..lines {
        match index % 8 {
            0 => {
                out.extend_from_slice(b"   Compiling vitrum-search v0.1.0 (crates/vitrum-search)\n")
            }
            1 => out.extend_from_slice(
                b"\x1b[1;32m    Finished\x1b[0m dev profile in 1.79s, no diagnostics\n",
            ),
            2 => {
                out.extend_from_slice(b"\x1b[2mdebug\x1b[0m ring wrote 4096 bytes at seq 918273\n")
            }
            3 => out.extend_from_slice(b"\n"),
            4 => out.extend_from_slice(b"\x1b]0;vitrum - session 7\x07plain line after a title\n"),
            5 => out.extend_from_slice(
                b"a much longer line than the others, carrying a stack frame: \
                  at core::iter::adapters::map::Map<I,F> as core::iter::traits\n",
            ),
            6 => out.extend_from_slice(b"warning: unused variable `index`\n"),
            _ => out.extend_from_slice(b"test chunks::tests::empty_chunks_are_skipped ... ok\n"),
        }
    }
    out
}

/// Locks out any per-line allocation in the scan. A regression here is
/// invisible in a unit test and fatal in a live search box: 52 million
/// allocations per keystroke across twenty sessions.
#[test]
fn scanning_does_not_allocate_per_line() {
    const SMALL: usize = 100_000;
    const LARGE: usize = 200_000;

    let small = scrollback(SMALL);
    let large = scrollback(LARGE);
    let small_slice: &[u8] = &small;
    let large_slice: &[u8] = &large;

    // A pattern that is present but rare, so hit allocations are a handful
    // rather than the measurement. `Finished` appears on one line in eight.
    let query = Query::literal("this-string-does-not-occur").context(0);
    let matcher = Matcher::compile(&query).expect("valid pattern");

    // Warm up: the regex crate keeps a thread-local cache pool whose first use
    // allocates, and both `Vec`s inside the scan grow to their working size on
    // the first few lines. Neither is per-line, but both would land in the
    // measurement if it started cold.
    for _ in 0..3 {
        let warm = search_with(
            &matcher,
            &query,
            &[Haystack {
                session: 0,
                base_seq: 0,
                chunks: std::slice::from_ref(&small_slice),
            }],
        )
        .expect("search");
        assert!(warm.is_empty());
    }

    let scan = |bytes: &&[u8]| {
        let results = search_with(
            &matcher,
            &query,
            &[Haystack {
                session: 0,
                base_seq: 0,
                chunks: std::slice::from_ref(bytes),
            }],
        )
        .expect("search");
        assert!(results.is_empty());
        results.lines_scanned
    };

    let mut small_lines = 0;
    let small_allocations = measure(|| small_lines = scan(&small_slice));
    let mut large_lines = 0;
    let large_allocations = measure(|| large_lines = scan(&large_slice));

    assert_eq!(small_lines, SMALL as u64);
    assert_eq!(large_lines, LARGE as u64);

    // Absolute bound. Per-line allocation would be six figures here.
    assert!(
        large_allocations < 64,
        "scanning {LARGE} lines cost {large_allocations} allocations; \
         the scan must reuse its buffers rather than allocate per line"
    );

    // Slope. Doubling the input must not increase the count, whatever fixed
    // overhead the harness contributed to both measurements.
    let growth = large_allocations.saturating_sub(small_allocations);
    assert!(
        growth <= 8,
        "doubling the line count from {SMALL} to {LARGE} added {growth} \
         allocations ({small_allocations} then {large_allocations}); \
         allocation must not scale with lines"
    );

    // A Sweep takes its sessions one at a time, which is what the daemon must
    // do because it cannot hold every ring lock at once. Its scratch buffers
    // have to survive ACROSS sessions, not just across lines: a Sweep that
    // rebuilt them per session would allocate twenty times per query and the
    // per-line measurement above would never notice.
    let sessions: Vec<&[u8]> = vec![small_slice; 8];
    let mut sweep_lines = 0u64;
    let sweep_allocations = measure(|| {
        let mut sweep = Sweep::new(&query).expect("compile");
        for (index, body) in sessions.iter().enumerate() {
            let chunks = [*body];
            sweep.push(&Haystack {
                session: index as u64,
                base_seq: 0,
                chunks: &chunks,
            });
        }
        sweep_lines = sweep.finish().lines_scanned;
    });

    assert_eq!(sweep_lines, (SMALL * 8) as u64);
    // Compiling the matcher and building the state allocate a fixed handful;
    // eight sessions of 100k lines each must not add more than the one scan did.
    assert!(
        sweep_allocations < 64,
        "sweeping 8 sessions of {SMALL} lines cost {sweep_allocations} \
         allocations; the sweep must reuse its scratch across sessions"
    );

    println!(
        "scan allocations: {small_lines} lines -> {small_allocations}, \
         {large_lines} lines -> {large_allocations}; \
         sweep of 8 sessions ({sweep_lines} lines) -> {sweep_allocations}"
    );
}
