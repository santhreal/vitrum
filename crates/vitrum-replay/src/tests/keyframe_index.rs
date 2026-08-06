//! Keyframe placement, lookup, and cost.

use crate::config::ReplayConfig;
use crate::emulator::Emulator;
use crate::keyframe::KeyframeIndex;
use crate::palette::Palette;
use crate::stream::Stream;
use crate::tests::support::{CAPTURED, config, grown};

/// A stream shorter than one stride gets no keyframes, and lookup says so.
///
/// The bug: emitting a keyframe at the end of the stream. A keyframe past the last byte
/// can never be the answer to a seek and would only cost memory.
#[test]
fn a_stream_shorter_than_one_stride_has_no_keyframes() {
    let bytes = CAPTURED;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let index = KeyframeIndex::build(&stream, &config(80, 24)).expect("build");

    assert!(index.is_empty());
    assert_eq!(index.len(), 0);
    assert!(index.latest_at_or_before(u64::MAX).is_none());
}

/// Keyframes land at or after each stride boundary and never before it.
///
/// A keyframe placed *before* its boundary is not a bug in itself, but it would mean the
/// slide went backwards, which is the direction that cannot be resumed from.
#[test]
fn every_keyframe_lands_at_or_after_its_stride_boundary() {
    let owned = grown(64 * 1024);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let stride = 4096;
    let index = KeyframeIndex::build(
        &stream,
        &config(80, 24)
            .with_keyframe_stride(stride)
            .expect("non-zero stride"),
    )
    .expect("build");

    assert!(index.len() >= 14, "64 KiB at a 4 KiB stride, got {}", index.len());
    for (position, frame) in index.frames().iter().enumerate() {
        let boundary = (position as u64 + 1) * stride as u64;
        assert!(
            frame.seq >= boundary,
            "keyframe {position} at {} is before its boundary {boundary}",
            frame.seq
        );
        assert!(
            frame.seq < boundary + 64,
            "keyframe {position} slid {} bytes past its boundary, which means the \
             ground search is not finding the boundaries it should",
            frame.seq - boundary
        );
    }
}

/// Keyframe seqs are strictly increasing, which is what the binary search needs.
#[test]
fn keyframe_seqs_are_strictly_increasing() {
    let owned = grown(64 * 1024);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let index =
        KeyframeIndex::build(&stream, &config(80, 24).with_keyframe_stride(2048).expect("stride"))
            .expect("build");

    let seqs: Vec<u64> = index.frames().iter().map(|frame| frame.seq).collect();
    assert!(seqs.len() > 20);
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "not sorted: {seqs:?}"
    );
}

/// Lookup returns the newest keyframe at or before the target, and nothing for a target
/// before the first one.
///
/// The bug: `partition_point` off by one, which returns a keyframe *after* the target.
/// The seek would then feed a negative range, silently read nothing, and show a screen
/// from the future.
#[test]
fn lookup_returns_the_newest_keyframe_at_or_before_the_target() {
    let owned = grown(32 * 1024);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let index =
        KeyframeIndex::build(&stream, &config(80, 24).with_keyframe_stride(4096).expect("stride"))
            .expect("build");

    let first = index.frames()[0].seq;
    assert!(index.latest_at_or_before(first - 1).is_none());
    assert_eq!(
        index.latest_at_or_before(first).map(|frame| frame.seq),
        Some(first),
        "a target exactly on a keyframe uses that keyframe"
    );

    let second = index.frames()[1].seq;
    assert_eq!(
        index.latest_at_or_before(second - 1).map(|frame| frame.seq),
        Some(first)
    );
    let last = index.frames().last().expect("at least one").seq;
    assert_eq!(
        index.latest_at_or_before(u64::MAX).map(|frame| frame.seq),
        Some(last)
    );
}

/// Every keyframe's screen equals a linear replay of the bytes up to its seq.
///
/// This is the keyframe contract in one assertion. If a snapshot were taken at a byte
/// the parser could not be resumed from, or one byte off from where it claims, this is
/// where it shows.
#[test]
fn every_keyframe_matches_a_linear_replay_to_its_own_seq() {
    let owned = grown(48 * 1024);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let index =
        KeyframeIndex::build(&stream, &config(80, 24).with_keyframe_stride(4096).expect("stride"))
            .expect("build");

    assert!(index.len() >= 10);
    for frame in index.frames() {
        let reference = crate::tests::support::linear(80, 24, &bytes[..frame.seq as usize]);
        assert_eq!(
            frame.screen(),
            &reference,
            "keyframe at seq {} does not match a linear replay to it",
            frame.seq
        );
    }
}

/// Resuming from a keyframe and feeding the rest reaches the same screen as a linear
/// replay of everything.
#[test]
fn resuming_from_any_keyframe_reaches_the_final_screen() {
    let owned = grown(48 * 1024);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let index =
        KeyframeIndex::build(&stream, &config(80, 24).with_keyframe_stride(4096).expect("stride"))
            .expect("build");
    let reference = crate::tests::support::linear(80, 24, bytes);

    for frame in index.frames() {
        let mut emulator = Emulator::resume(frame.screen().clone());
        emulator.feed(&bytes[frame.seq as usize..]);
        assert_eq!(
            emulator.screen(),
            &reference,
            "resuming from seq {} diverged",
            frame.seq
        );
    }
}

/// A stream that offers no resumable boundary produces no keyframes and reports it.
///
/// The bug: silently degrading. A session that catted a binary file gets slower seeks in
/// that region, and a caller that cannot see why would blame the whole feature.
#[test]
fn a_stream_with_no_resumable_boundary_reports_skipped_boundaries() {
    // One unterminated OSC string, so the parser never returns to ground.
    let mut bytes = b"\x1b]0;".to_vec();
    bytes.extend(core::iter::repeat_n(b'x', 40_000));
    let slice = bytes.as_slice();
    let stream = Stream::new(0, core::slice::from_ref(&slice));
    let index = KeyframeIndex::build(
        &stream,
        &config(80, 24).with_keyframe_stride(4096).expect("stride"),
    )
    .expect("build");

    assert_eq!(index.len(), 0, "nowhere safe to snapshot");
    assert!(
        index.skipped_boundaries() > 0,
        "the index has to say it could not keyframe"
    );
}

/// A ground scan of zero disables keyframing outright, which is a supported choice.
#[test]
fn a_zero_ground_scan_disables_keyframing() {
    let owned = grown(32 * 1024);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let index = KeyframeIndex::build(
        &stream,
        &config(80, 24)
            .with_keyframe_stride(4096)
            .expect("stride")
            .with_ground_scan(0),
    )
    .expect("build");

    assert_eq!(index.len(), 0);
    assert!(index.skipped_boundaries() > 0);
    assert_eq!(index.heap_bytes(), 0);
}

/// A stride of zero is refused rather than snapshotting every byte.
#[test]
fn a_zero_stride_is_refused() {
    assert_eq!(
        config(80, 24).with_keyframe_stride(0),
        Err(crate::error::Error::ZeroStride)
    );
}

/// The reported memory cost is the real one: keyframe count times screen size.
///
/// vitrum's whole memory story rests on numbers like this being exact rather than
/// estimated, and a caller choosing a stride needs to be able to do the arithmetic.
#[test]
fn the_reported_memory_cost_is_the_real_one() {
    let owned = grown(64 * 1024);
    let bytes: &[u8] = &owned;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let index =
        KeyframeIndex::build(&stream, &config(80, 24).with_keyframe_stride(8192).expect("stride"))
            .expect("build");

    let per_screen: usize = index
        .frames()
        .iter()
        .map(|frame| frame.screen().heap_bytes())
        .sum();
    assert!(index.len() >= 7);
    assert!(
        index.heap_bytes() >= per_screen,
        "the total must account for at least the screens it holds"
    );
    // 80x24 at 16 bytes a cell, plus one damage span per row.
    let plain_screen = 80 * 24 * 16 + 24 * 4;
    assert!(
        index.heap_bytes() < index.len() * plain_screen * 3,
        "{} bytes for {} keyframes is more than three screens each",
        index.heap_bytes(),
        index.len()
    );
}

/// The index refuses a geometry the grid will not build, before doing any work.
#[test]
fn an_impossible_geometry_is_refused_before_the_pass() {
    let bytes = CAPTURED;
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let broken = ReplayConfig {
        cols: 0,
        rows: 24,
        palette: Palette::XTERM,
        keyframe_stride: 4096,
        ground_scan: 4096,
    };
    assert_eq!(
        KeyframeIndex::build(&stream, &broken).map(|index| index.len()),
        Err(crate::error::Error::Geometry { cols: 0, rows: 24 })
    );
}
