//! What a seek costs, and what a replay costs to hold.
//!
//! Both numbers are load-bearing. A scrubber's smoothness is the seek number and
//! vitrum's idle-memory budget is the memory number, and a regression in either is
//! invisible in a correctness suite: every screen would still be right, and the
//! scrubber would just stall.
//!
//! Cost is measured as bytes fed, not as elapsed time. Bytes fed is the seek's
//! entire work, it is exact, and it does not change with what else the machine is
//! doing.
//!
//! # What changed when Ghostty became the engine
//!
//! There used to be a `KeyframeIndex` here, and a test asserting that no seek fed
//! more than one 256 KiB stride. That bound came from cloning a [`crate::Screen`]
//! every stride, which was possible only while this crate owned the parser and
//! terminal state was its own `Clone` struct. Ghostty owns terminal state now and
//! will not clone it, serialise it, or read it back, so the bound is gone and the
//! tests that asserted it are gone with it rather than being softened until they
//! passed. `a_rewind_replays_the_whole_prefix` below states the cost that replaced
//! it, so it is a measured contract rather than a silent regression.
//!
//! What survives intact is the forward direction, which is the one a drag actually
//! uses, and what improves is memory: a replay now holds one screen instead of one
//! per stride.

use crate::stream::Stream;
use crate::tests::support::{config, grown, with_replay};

const SIZE: usize = 512 * 1024;

/// Dragging the scrubber forwards never re-reads a byte: the whole drag is one pass.
///
/// The bug this stops: rebuilding the emulator on every seek regardless of
/// direction. That stays correct and it turns a drag over a region into one full
/// replay per frame, which is the difference between a smooth drag and a freeze.
///
/// The step total is now *exactly* the region dragged across. Under the keyframe
/// index it could come in under that, because a step crossing a keyframe resumed
/// from it and skipped the bytes between; with one live engine there is nothing to
/// resume from and nothing to skip, so every byte of the region is fed once and
/// none of it twice.
#[test]
fn a_forward_drag_never_re_reads_a_byte() {
    let owned = grown(SIZE);
    let bytes: &[u8] = &owned;

    with_replay(bytes, &config(80, 24), |replay| {
        let head = replay.stream().head_seq();
        let frames = 128u64;
        let mut total = 0u64;
        let mut previous = 0u64;

        for step in 1..=frames {
            let target = head * step / frames;
            replay.seek(target).expect("drag");
            assert_eq!(
                replay.last_seek_bytes(),
                target - previous,
                "the step to {target} fed a different number of bytes than it advanced"
            );
            total += replay.last_seek_bytes();
            previous = target;
        }

        assert_eq!(
            total, head,
            "a forward drag fed {total} bytes over a {head} byte stream"
        );
    });
}

/// A seek to the position already held feeds nothing at all.
#[test]
fn re_seeking_the_current_position_is_free() {
    let owned = grown(64 * 1024);
    let bytes: &[u8] = &owned;

    with_replay(bytes, &config(80, 24), |replay| {
        let target = replay.stream().head_seq() / 3;
        replay.seek(target).expect("seek");
        replay.seek(target).expect("seek again");

        assert_eq!(replay.last_seek_bytes(), 0);
    });
}

/// A rewind feeds the whole prefix, and the cost is a function of the target.
///
/// This is a regression against the keyframe index, and it is asserted rather than
/// merely admitted so that the number a UI has to budget for is written down. A
/// rewind to seq `t` parses `t - base` bytes, every time, because Ghostty's state
/// cannot be checkpointed and a rewind therefore has nowhere to start but the
/// beginning. See [`crate::replay`] for why neither live engines nor cell-grid
/// snapshots recover the old bound.
///
/// Measured, release build, a 10 MiB stream of agent output on a Ryzen 9 9950X:
/// build 5.7 ms, rewind 2.7 ms at a target one sixteenth in, 25.0 ms at the median
/// target, 34.6 ms at the far end. A forward drag across the whole stream in fifty
/// steps costs 42 ms in total. So the worst rewind a full ring can produce is about
/// two frames at 60 Hz, and a scrubber that coalesces to the operator's latest
/// target rather than queuing every intermediate one absorbs it.
///
/// The defect this closes is the opposite of the obvious one: a future change that
/// makes a rewind report *less* than the prefix without actually replaying it would
/// be a seek reading stale state, and it would fail here before it failed in the
/// seek-equivalence suite.
#[test]
fn a_rewind_replays_the_whole_prefix() {
    let owned = grown(64 * 1024);
    let bytes: &[u8] = &owned;

    with_replay(bytes, &config(80, 24), |replay| {
        let head = replay.stream().head_seq();
        replay.seek(head).expect("to head");

        for divisor in [2u64, 3, 4, 8] {
            let target = head / divisor;
            replay.seek(head).expect("back to head");
            replay.seek(target).expect("rewind");
            assert_eq!(
                replay.last_seek_bytes(),
                target,
                "a rewind to {target} fed a different prefix"
            );
        }
    });
}

/// A replay's memory does not grow with the stream it replays.
///
/// This is what the keyframe index cost and what removing it bought: memory used to
/// be one full-screen clone per stride, so a 10 MiB ring carried about 1.2 MiB of
/// snapshots. A replay now carries one screen, whatever the ring holds.
///
/// The chapter markers are the one component that does scale, because there are
/// genuinely more chapters in a longer session, so they are measured apart from the
/// part that must stay flat. The bug this stops: an index of screens or grids
/// reappearing under another name.
#[test]
fn replay_memory_does_not_grow_with_the_stream() {
    let mut sizes = Vec::new();

    for size in [64 * 1024usize, 4 * 1024 * 1024] {
        let owned = grown(size);
        let bytes: &[u8] = &owned;
        with_replay(bytes, &config(80, 24), |replay| {
            let head = replay.stream().head_seq();
            replay.seek(head).expect("to head");
            sizes.push((
                replay.heap_bytes() - replay.timeline().heap_bytes(),
                replay.screen().heap_bytes(),
            ));
        });
    }

    assert_eq!(
        sizes[0], sizes[1],
        "a 64x longer stream changed the non-marker memory"
    );
    assert!(sizes[0].1 > 0, "a screen has to cost something");
}

/// Screen memory scales with the geometry and with nothing else.
///
/// A caller sizing a scrubber for a 200x50 pane needs this arithmetic to hold.
#[test]
fn screen_memory_scales_with_the_geometry() {
    let owned = grown(SIZE);
    let bytes: &[u8] = &owned;

    let mut measured = Vec::new();
    for (cols, rows) in [(80u16, 24u16), (200, 50)] {
        with_replay(bytes, &config(cols, rows), |replay| {
            let head = replay.stream().head_seq();
            replay.seek(head).expect("to head");
            measured.push(replay.screen().heap_bytes() as f64);
        });
    }

    let ratio = measured[1] / measured[0];
    let cells = (200.0 * 50.0) / (80.0 * 24.0);
    assert!(
        (ratio - cells).abs() < 0.5,
        "{ratio:.2}x the memory for {cells:.2}x the cells"
    );
}

/// A replay's reported cost accounts for the timeline and the screen, and the
/// stream it borrows is excluded.
///
/// The bug this stops: `heap_bytes` that forgets a component and understates the
/// cost of attaching a scrubber to every open session, or one that counts the
/// daemon's own ring twice.
#[test]
fn the_replay_reports_its_timeline_and_its_screen() {
    let owned = grown(SIZE);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let stream_bytes = stream.len() as usize;

    with_replay(bytes, &config(80, 24), |replay| {
        let timeline = replay.timeline().heap_bytes();
        let screen = replay.screen().heap_bytes();

        assert!(timeline > 0 && screen > 0);
        assert_eq!(replay.heap_bytes(), timeline + screen);
        assert!(
            replay.heap_bytes() < stream_bytes,
            "{} bytes of replay for {stream_bytes} bytes of stream",
            replay.heap_bytes()
        );
    });
}
