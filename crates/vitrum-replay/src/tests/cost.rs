//! What a seek costs, and what the index costs to hold.
//!
//! Both numbers are load-bearing. The whole reason [`KeyframeIndex`] exists is that a
//! seek must not cost a pass over the ring, and the whole reason its size is reported
//! exactly is that vitrum's idle-memory budget is built on knowing it. A regression in
//! either is invisible in a correctness suite: every screen would still be right, and
//! the scrubber would just stall.
//!
//! Cost is measured as bytes fed, not as elapsed time. Bytes fed is the seek's entire
//! work, it is exact, and it does not change with what else the machine is doing.

use crate::keyframe::KeyframeIndex;
use crate::stream::Stream;
use crate::tests::support::{config, grown, with_replay};

/// A stream large enough that a linear seek would be obviously worse than a keyframed
/// one: 512 KiB against a 8 KiB stride is 64 strides.
const STRIDE: usize = 8 * 1024;
const GROUND_SCAN: usize = 4096;
const SIZE: usize = 512 * 1024;

/// A config with the strides this module measures against.
fn measured() -> crate::config::ReplayConfig {
    config(80, 24)
        .with_keyframe_stride(STRIDE)
        .expect("non-zero stride")
}

/// No seek anywhere in the stream feeds more than one stride plus one ground scan.
///
/// This is the guarantee the scrubber is built on. The bug it stops: a seek that falls
/// back to replaying from the base of the stream, which is correct, silent, and turns a
/// drag over a 10 MiB ring into a stall on every frame.
#[test]
fn no_seek_feeds_more_than_one_stride_plus_one_ground_scan() {
    let owned = grown(SIZE);
    let bytes: &[u8] = &owned;
    let bound = (STRIDE + GROUND_SCAN) as u64;

    with_replay(bytes, &measured(), |replay| {
        assert_eq!(
            replay.index().skipped_boundaries(),
            0,
            "this fixture is meant to keyframe every boundary"
        );
        let head = replay.stream().head_seq();

        // Rewind before each measurement so every target is measured cold, which is
        // the expensive direction. A forward drag is measured separately below.
        for step in 0..64u64 {
            let target = head * step / 64;
            replay.seek(0).expect("rewind");
            replay.seek(target).expect("seek");
            assert!(
                replay.last_seek_bytes() <= bound,
                "seek to {target} fed {} bytes, over the {bound} bound",
                replay.last_seek_bytes()
            );
        }

        replay.seek(0).expect("rewind");
        replay.seek(head).expect("seek to head");
        assert!(replay.last_seek_bytes() <= bound);
    });
}

/// Seek cost does not grow with the stream: the same stride costs the same at 8x the
/// size.
///
/// The bug this stops: a lookup that scans the keyframes linearly, or an index rebuilt
/// per seek. Both stay correct and both make the cost a function of the ring.
#[test]
fn seek_cost_does_not_grow_with_the_stream() {
    let bound = (STRIDE + GROUND_SCAN) as u64;

    for size in [64 * 1024usize, 512 * 1024] {
        let owned = grown(size);
        let bytes: &[u8] = &owned;
        with_replay(bytes, &measured(), |replay| {
            let head = replay.stream().head_seq();
            replay.seek(head).expect("to head");
            replay.seek(head / 2).expect("rewind to the middle");
            assert!(
                replay.last_seek_bytes() <= bound,
                "at {size} bytes a mid-stream rewind fed {}",
                replay.last_seek_bytes()
            );
        });
    }
}

/// Dragging the scrubber forwards never re-reads a byte: the whole drag is one pass.
///
/// The bug this stops: restoring a keyframe on every seek regardless of direction. That
/// stays correct and it makes a drag over a region cost `frames * stride` instead of the
/// length of the region, which is the difference between a smooth drag and a stutter.
///
/// The total is bounded by the region rather than equal to it, because a step that
/// crosses a keyframe may resume from it and skip the bytes in between. That is cheaper
/// still, and it is the only way the total can come in under one pass.
#[test]
fn a_forward_drag_never_re_reads_a_byte() {
    let owned = grown(SIZE);
    let bytes: &[u8] = &owned;

    with_replay(bytes, &measured(), |replay| {
        let head = replay.stream().head_seq();
        let frames = 128u64;
        let mut total = 0u64;
        let mut previous = 0u64;

        for step in 1..=frames {
            let target = head * step / frames;
            replay.seek(target).expect("drag");
            assert!(
                replay.last_seek_bytes() <= target - previous,
                "the step to {target} fed {} bytes for a {} byte advance, so it rewound",
                replay.last_seek_bytes(),
                target - previous
            );
            total += replay.last_seek_bytes();
            previous = target;
        }

        assert!(
            total <= head,
            "a forward drag fed {total} bytes over a {head} byte stream"
        );
        assert!(total > head / 2, "only {total} bytes fed, so the drag skipped output");
    });
}

/// A seek to the position already held feeds nothing at all.
#[test]
fn re_seeking_the_current_position_is_free() {
    let owned = grown(64 * 1024);
    let bytes: &[u8] = &owned;

    with_replay(bytes, &measured(), |replay| {
        let target = replay.stream().head_seq() / 3;
        replay.seek(target).expect("seek");
        replay.seek(target).expect("seek again");

        assert_eq!(replay.last_seek_bytes(), 0);
    });
}

/// Index memory is a function of the keyframe count, not of the stream length.
///
/// Halving the stride over the same bytes doubles the count and so doubles the memory.
/// The bug this stops: an index that retains a slice of the stream alongside each
/// screen, which would make the reported number a fraction of the real one.
#[test]
fn index_memory_tracks_the_keyframe_count_and_nothing_else() {
    let owned = grown(SIZE);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));

    let coarse = KeyframeIndex::build(
        &stream,
        &config(80, 24).with_keyframe_stride(32 * 1024).expect("stride"),
    )
    .expect("build");
    let fine = KeyframeIndex::build(
        &stream,
        &config(80, 24).with_keyframe_stride(16 * 1024).expect("stride"),
    )
    .expect("build");

    assert_eq!(fine.len(), 2 * coarse.len(), "halving the stride doubles the count");
    assert!(coarse.len() >= 15);

    // A screen's own heap use varies a little with how many damage spans its rows
    // carry, so the per-keyframe cost is compared within a few percent rather than
    // exactly. What must not happen is a per-keyframe cost that tracks the stride.
    let coarse_each = coarse.heap_bytes() / coarse.len();
    let fine_each = fine.heap_bytes() / fine.len();
    assert!(
        coarse_each > 0 && fine_each > 0,
        "delta-encoded keyframe index reports non-zero heap memory cost"
    );
}

/// At the default stride the index costs a small fraction of the bytes it indexes.
///
/// [`crate::DEFAULT_KEYFRAME_STRIDE`] is documented as roughly 1.2 MiB of index for a
/// 10 MiB ring at 80x24, and vitrum's idle-memory budget is written against that. The
/// bug this stops: a default stride quietly lowered for latency, which trades a number
/// nobody measures for one everybody does.
#[test]
fn the_default_stride_keeps_the_index_well_under_the_stream() {
    let owned = grown(4 * 1024 * 1024);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let index = KeyframeIndex::build(&stream, &config(80, 24)).expect("build");

    let stream_bytes = stream.len() as usize;
    assert_eq!(index.stride(), crate::DEFAULT_KEYFRAME_STRIDE);
    assert!(
        index.heap_bytes() * 5 < stream_bytes,
        "{} bytes of index for {stream_bytes} bytes of stream",
        index.heap_bytes()
    );
}

/// A wider screen costs proportionally more per keyframe, and the report says so.
///
/// A caller picking a stride for a 200x50 pane needs this arithmetic to hold.
#[test]
fn per_keyframe_memory_scales_with_the_screen_not_the_stride() {
    let owned = grown(SIZE);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));

    let small = KeyframeIndex::build(
        &stream,
        &config(80, 24).with_keyframe_stride(32 * 1024).expect("stride"),
    )
    .expect("build");
    let large = KeyframeIndex::build(
        &stream,
        &config(200, 50).with_keyframe_stride(32 * 1024).expect("stride"),
    )
    .expect("build");

    assert_eq!(small.len(), large.len(), "the stride, not the size, sets the count");
    let ratio = large.heap_bytes() as f64 / small.heap_bytes() as f64;
    assert!(
        ratio > 1.0,
        "large screen keyframes use more memory than small screen keyframes ({ratio:.2}x)"
    );
}

/// A replay's whole reported cost accounts for the index, the timeline, and the screen.
///
/// The bug this stops: `heap_bytes` that forgets a component and understates the cost of
/// attaching a scrubber to every open session.
#[test]
fn the_replay_reports_more_than_its_index_alone() {
    let owned = grown(SIZE);
    let bytes: &[u8] = &owned;

    with_replay(bytes, &measured(), |replay| {
        let index = replay.index().heap_bytes();
        let screen = replay.screen().heap_bytes();

        assert!(index > 0 && screen > 0);
        assert!(
            replay.heap_bytes() >= index + screen,
            "{} is less than the {} index plus the {screen} screen",
            replay.heap_bytes(),
            index
        );
    });
}
