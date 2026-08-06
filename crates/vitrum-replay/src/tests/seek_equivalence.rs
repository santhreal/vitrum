//! Every seek, against a linear replay.
//!
//! The contract is one sentence: `seek(n)` must produce the same screen as feeding
//! bytes `base..n` to a fresh emulator. Everything else in this crate is an
//! optimisation of that sentence, and every optimisation is a chance to break it.
//!
//! The reference implementation is [`crate::tests::support::linear`], which has no
//! keyframes, no restore, and no cleverness at all.

use crate::config::ReplayConfig;
use crate::replay::Replay;
use crate::stream::Stream;
use crate::tests::support::{CAPTURED, config, grown, linear};

/// Build a replay over `bytes` and assert `seek(n)` matches a linear replay for every n
/// in `targets`, visiting them in the order given.
fn assert_seeks_match(bytes: &[u8], targets: &[u64], config: &ReplayConfig) {
    let chunks = [bytes];
    let stream = Stream::new(0, &chunks);
    let mut replay = Replay::build(stream, config).expect("build");

    for &target in targets {
        let screen = replay.seek(target).expect("in range");
        let reference = linear(config.cols, config.rows, &bytes[..target as usize]);
        assert_eq!(
            screen, &reference,
            "seek to {target} diverged from a linear replay of the same prefix"
        );
        assert_eq!(replay.position(), target);
    }
}

/// Seeking to every single byte offset of the capture matches a linear replay.
///
/// This is the exhaustive form of the contract. It covers every boundary the fixture
/// contains, which includes the interior of every escape sequence, every OSC, every
/// multi-byte character and every invalid byte, without anybody having to enumerate
/// them.
#[test]
fn seeking_to_every_byte_of_the_capture_matches_a_linear_replay() {
    let targets: Vec<u64> = (0..=CAPTURED.len() as u64).collect();
    assert_seeks_match(CAPTURED, &targets, &config(80, 24));
}

/// The same, in descending order, which forces the keyframe rewind path on every seek.
///
/// Ascending seeks never rewind, so a suite that only walked forwards would never
/// execute the restore at all.
#[test]
fn seeking_backwards_through_the_capture_matches_a_linear_replay() {
    let targets: Vec<u64> = (0..=CAPTURED.len() as u64).rev().collect();
    assert_seeks_match(
        CAPTURED,
        &targets,
        &config(80, 24).with_keyframe_stride(64).expect("stride"),
    );
}

/// A seq inside a multi-byte UTF-8 character replays as though the character had not
/// arrived, which is what the session's own terminal showed at that instant.
///
/// The bug: a keyframe or a resume that treats the partial character as garbage and
/// prints a replacement character. The fixture contains Japanese text, so this is the
/// real byte layout and not a constructed one.
#[test]
fn a_seek_inside_a_multi_byte_character_shows_the_character_as_not_yet_arrived() {
    // The first byte of 日 (0xe6 0x97 0xa5) in the fixture's `utf8:` line.
    let lead = CAPTURED
        .windows(3)
        .position(|window| window == [0xe6, 0x97, 0xa5])
        .expect("the fixture contains Japanese text");

    let chunks = [CAPTURED];
    let stream = Stream::new(0, &chunks);
    let mut replay = Replay::build(stream, &config(80, 24)).expect("build");

    for inside in 1..=2u64 {
        let target = lead as u64 + inside;
        let screen = replay.seek(target).expect("in range");
        let reference = linear(80, 24, &CAPTURED[..target as usize]);
        assert_eq!(
            screen, &reference,
            "seek {inside} byte(s) into a three-byte character diverged"
        );
    }

    // And the whole character appears once its last byte is included.
    let complete = replay.seek(lead as u64 + 3).expect("in range");
    assert!(
        complete.text().contains('\u{65e5}'),
        "the character should be on screen once all three bytes are in"
    );
}

/// A seq inside an escape sequence replays as though the sequence had not arrived.
///
/// The bug: keyframing mid-sequence, which loses the whole sequence and every effect it
/// would have had. The fixture's SGR runs are the real thing produced by `git log`.
#[test]
fn a_seek_inside_an_escape_sequence_shows_the_sequence_as_not_yet_arrived() {
    // `ESC [ 1 ; 3 1 m`, the bold-red run in the fixture.
    let start = CAPTURED
        .windows(8)
        .position(|window| window == b"\x1b[1;31mE")
        .expect("the fixture contains a bold red run");

    let chunks = [CAPTURED];
    let stream = Stream::new(0, &chunks);
    let mut replay = Replay::build(stream, &config(80, 24)).expect("build");

    for inside in 1..=6u64 {
        let target = start as u64 + inside;
        let screen = replay.seek(target).expect("in range");
        let reference = linear(80, 24, &CAPTURED[..target as usize]);
        assert_eq!(
            screen, &reference,
            "seek {inside} byte(s) into `ESC [ 1 ; 3 1 m` diverged"
        );
    }
}

/// A seq inside an `ESC \` terminated OSC replays correctly.
///
/// The interesting byte is the `ESC` of the terminator, where vte has already dispatched
/// the OSC but is not yet in its ground state. This is the exact boundary the ground
/// probe deliberately calls unsafe, and a seek landing there must still be right.
#[test]
fn a_seek_inside_an_esc_terminated_osc_matches_a_linear_replay() {
    let start = CAPTURED
        .windows(6)
        .position(|window| window == b"\x1b]7373")
        .expect("the fixture contains OSC 7373 hints");
    let terminator = CAPTURED[start..]
        .windows(2)
        .position(|window| window == b"\x1b\\" || window == [0x07, 0x00])
        .map_or(start + 20, |offset| start + offset);

    let chunks = [CAPTURED];
    let stream = Stream::new(0, &chunks);
    let mut replay = Replay::build(stream, &config(80, 24)).expect("build");

    for target in (start as u64)..=(terminator as u64 + 2) {
        let screen = replay.seek(target).expect("in range");
        let reference = linear(80, 24, &CAPTURED[..target as usize]);
        assert_eq!(screen, &reference, "seek to {target} inside an OSC diverged");
    }
}

/// A seek across a ring's join matches a linear replay.
///
/// The whole point of not stitching the halves is that nothing downstream should notice
/// them. If the join were mishandled, this would fail for exactly one split value and
/// nothing else in the suite would catch it.
#[test]
fn seeking_over_a_ring_join_matches_a_linear_replay() {
    for split in [1usize, 7, 400, CAPTURED.len() / 2, CAPTURED.len() - 1] {
        let (older, newer) = CAPTURED.split_at(split);
        let chunks = [older, newer];
        let stream = Stream::new(0, &chunks);
        let mut replay = Replay::build(stream, &config(80, 24)).expect("build");

        for target in [
            0u64,
            split as u64 - 1,
            split as u64,
            split as u64 + 1,
            CAPTURED.len() as u64,
        ] {
            let screen = replay.seek(target).expect("in range");
            let reference = linear(80, 24, &CAPTURED[..target as usize]);
            assert_eq!(
                screen, &reference,
                "join at {split}, seek to {target} diverged"
            );
        }
    }
}

/// A stream that has evicted bytes seeks by absolute seq and starts from the first byte
/// it still holds.
///
/// The bug: replaying from byte zero of the retained buffer while numbering by absolute
/// seq, which puts every screen at the wrong offset by exactly the amount evicted.
#[test]
fn a_stream_with_an_evicted_prefix_seeks_by_absolute_seq() {
    let base = 9_999_000u64;
    let chunks = [CAPTURED];
    let stream = Stream::new(base, &chunks);
    let mut replay = Replay::build(stream, &config(80, 24)).expect("build");

    for offset in [0u64, 1, 100, CAPTURED.len() as u64] {
        let screen = replay.seek(base + offset).expect("in range");
        let reference = linear(80, 24, &CAPTURED[..offset as usize]);
        assert_eq!(screen, &reference, "seek to base + {offset} diverged");
    }

    assert!(replay.seek(base - 1).is_err(), "before the retained window");
    assert!(
        replay.seek(base + CAPTURED.len() as u64 + 1).is_err(),
        "past the head"
    );
}

/// An out-of-range seek names the range that is available.
///
/// A scrubber holding a stale timeline asks for evicted seqs routinely, and the answer
/// has to be enough to redraw the scrubber's own bounds without another round trip.
#[test]
fn an_out_of_range_seek_reports_the_window_it_could_have_used() {
    let chunks = [CAPTURED];
    let stream = Stream::new(500, &chunks);
    let mut replay = Replay::build(stream, &config(80, 24)).expect("build");

    assert_eq!(
        replay.seek(499),
        Err(crate::error::Error::SeqOutOfRange {
            seq: 499,
            oldest: 500,
            head: 500 + CAPTURED.len() as u64,
        })
    );
}

/// Seeking over a long stream in a scattered order matches a linear replay every time.
///
/// Forward, backward, tiny hops and huge jumps in one run, which is what a user dragging
/// a scrubber actually produces. A seek that only worked from a clean start would pass
/// every other test in this file and fail here.
#[test]
fn scattered_seeks_over_a_long_stream_all_match() {
    let bytes = grown(300 * 1024);
    let length = bytes.len() as u64;
    let config = config(100, 30).with_keyframe_stride(16 * 1024).expect("stride");

    // A deterministic scatter: a multiplicative walk over the stream, so the order is
    // reproducible and covers forward jumps, backward jumps and near-neighbours.
    let mut targets = Vec::new();
    let mut cursor = 7u64;
    for step in 0..120u64 {
        cursor = (cursor * 48_271) % length;
        targets.push(cursor);
        if step % 5 == 0 {
            targets.push(cursor.saturating_add(1).min(length));
            targets.push(cursor.saturating_sub(1));
        }
    }
    targets.push(0);
    targets.push(length);

    assert_seeks_match(&bytes, &targets, &config);
}

/// Seeking forward does not rewind to a keyframe when the current position is already
/// past it.
///
/// This is the property that makes dragging a scrubber cheap rather than quadratic. It
/// is asserted through the emulator's own position, because the alternative, timing, is
/// not a contract.
#[test]
fn a_forward_seek_from_a_position_past_the_last_keyframe_does_not_rewind() {
    let bytes = grown(64 * 1024);
    let chunks = [bytes.as_slice()];
    let stream = Stream::new(0, &chunks);
    let config = config(80, 24).with_keyframe_stride(8192).expect("stride");
    let mut replay = Replay::build(stream, &config).expect("build");

    let head = replay.stream().head_seq();
    let last_keyframe = replay.index().frames().last().expect("keyframes").seq;
    let start = head - 20;
    assert!(start > last_keyframe, "the fixture must end past its last keyframe");
    replay.seek(start).expect("in range");
    assert_eq!(replay.position(), start);

    // Now step forward one byte at a time. Each step must land exactly where asked and
    // agree with a linear replay, which it cannot do if the restore fired and dropped
    // the bytes between the keyframe and here.
    for step in 1..=20u64 {
        let target = start + step;
        let screen = replay.seek(target).expect("in range");
        let reference = linear(80, 24, &bytes[..target as usize]);
        assert_eq!(screen, &reference);
    }
}

/// An empty stream is seekable, and its only position is its base.
#[test]
fn an_empty_stream_seeks_to_its_base_and_nowhere_else() {
    let empty: &[u8] = b"";
    let stream = Stream::new(42, core::slice::from_ref(&empty));
    let mut replay = Replay::build(stream, &config(10, 3)).expect("build");

    assert_eq!(replay.seek(42).map(|screen| screen.text()), Ok(String::from("\n\n")));
    assert!(replay.seek(43).is_err());
}
