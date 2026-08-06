//! OSC 7373 marker positions.

use vitrum_proto::HintState;

use crate::hints::scan;
use crate::stream::Stream;
use crate::tests::support::CAPTURED;

fn markers_of(bytes: &[u8]) -> Vec<(u64, String, Option<HintState>)> {
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    scan(&stream)
        .into_iter()
        .map(|marker| (marker.seq, marker.label, marker.hint))
        .collect()
}

/// A marker sits one byte past the sequence's terminator, so seeking to it shows the
/// screen as it stood the instant the hint landed.
///
/// The bug: recording the seq of the introducer. Seeking there shows the screen *before*
/// the hint's own output, which for an approval prompt means the question is not on
/// screen yet, which is the one thing the user wanted to see.
#[test]
fn a_marker_sits_one_byte_past_the_terminator() {
    let bytes = b"abc\x1b]7373;approval;force push?\x07def";
    let markers = markers_of(bytes);
    assert_eq!(markers.len(), 1);
    let terminator = bytes.iter().position(|&byte| byte == 0x07).expect("BEL");
    assert_eq!(markers[0].0, terminator as u64 + 1);
    assert_eq!(markers[0].1, "force push?");
    assert_eq!(markers[0].2, Some(HintState::Approval));
}

/// Both terminators are recognised and both give the same position rule.
///
/// `ESC \` is two bytes and `BEL` is one, so a position rule based on the introducer plus
/// a constant would be wrong for one of them. The fixture contains both.
#[test]
fn both_terminators_are_recognised_with_the_same_position_rule() {
    let bel = markers_of(b"\x1b]7373;ready;done\x07");
    assert_eq!(bel[0].0, 18, "one past the BEL");

    let st = markers_of(b"\x1b]7373;ready;done\x1b\\");
    assert_eq!(st[0].0, 19, "one past the backslash");
}

/// A hint with no label gets the state's own name, not an empty one.
///
/// An unlabelled tick on a scrubber is a tick the user cannot identify.
#[test]
fn an_unlabelled_hint_gets_the_states_own_name() {
    assert_eq!(markers_of(b"\x1b]7373;working\x07")[0].1, "working");
    assert_eq!(markers_of(b"\x1b]7373;approval\x07")[0].1, "approval needed");
    assert_eq!(markers_of(b"\x1b]7373;input\x07")[0].1, "input needed");
    assert_eq!(markers_of(b"\x1b]7373;ready\x07")[0].1, "ready");
}

/// Other applications' OSC sequences produce no markers.
///
/// A shell sets a window title on every prompt and `OSC 777` notifications fly past
/// constantly. Treating any OSC as a chapter would bury the real ones under hundreds of
/// ticks.
#[test]
fn other_osc_sequences_produce_no_markers() {
    for noise in [
        b"\x1b]0;a shell title\x07".as_slice(),
        b"\x1b]2;another title\x1b\\".as_slice(),
        b"\x1b]777;notify;title;body\x07".as_slice(),
        b"\x1b]8;;https://example.com\x07link\x1b]8;;\x07".as_slice(),
        b"\x1b]737;ready\x07".as_slice(),
        b"\x1b]73730;ready\x07".as_slice(),
        b"\x1b]07373;ready\x07".as_slice(),
        b"\x1b]7373;Ready\x07".as_slice(),
        b"\x1b]7373;paused\x07".as_slice(),
        b"\x1b]7373\x07".as_slice(),
    ] {
        assert!(
            markers_of(noise).is_empty(),
            "{noise:?} should not be a chapter marker"
        );
    }
}

/// A hint split across a ring's join is found, with the same seq as when it is whole.
///
/// The join lands wherever the write cursor was. A scanner that reset per chunk would
/// lose exactly the hints that straddle it, and only sometimes, which is the worst kind
/// of bug to chase.
#[test]
fn a_hint_split_at_every_byte_is_found_with_the_same_seq() {
    let whole: &[u8] = b"noise\x1b]7373;input;which file?\x1b\\more";
    let expected = markers_of(whole);
    assert_eq!(expected.len(), 1);

    for split in 1..whole.len() {
        let (older, newer) = whole.split_at(split);
        let chunks = [older, newer];
        let stream = Stream::new(0, &chunks);
        let markers = scan(&stream);
        assert_eq!(markers.len(), 1, "split at {split} lost the hint");
        assert_eq!(
            markers[0].seq, expected[0].0,
            "split at {split} moved the marker"
        );
        assert_eq!(markers[0].label, "which file?");
    }
}

/// A hint arriving one byte per chunk is found, which is the pathological ring shape.
#[test]
fn a_hint_arriving_one_byte_per_chunk_is_found() {
    let whole: &[u8] = b"\x1b]7373;ready;turn over\x1b\\";
    let chunks: Vec<&[u8]> = whole.chunks(1).collect();
    let stream = Stream::new(0, &chunks);
    let markers = scan(&stream);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].seq, whole.len() as u64);
    assert_eq!(markers[0].label, "turn over");
}

/// Several hints in one chunk come back in order with distinct seqs.
#[test]
fn several_hints_in_one_chunk_come_back_in_order() {
    let markers = markers_of(b"\x1b]7373;working\x07out\x1b]7373;ready;done\x1b\\");
    assert_eq!(markers.len(), 2);
    assert_eq!(markers[0].2, Some(HintState::Working));
    assert_eq!(markers[1].2, Some(HintState::Ready));
    assert!(markers[0].0 < markers[1].0);
}

/// Positions are absolute seq, so an evicted prefix does not shift them.
#[test]
fn positions_are_absolute_seq() {
    let bytes: &[u8] = b"x\x1b]7373;ready\x07";
    let stream = Stream::new(5_000_000, core::slice::from_ref(&bytes));
    let markers = scan(&stream);
    assert_eq!(markers[0].seq, 5_000_000 + bytes.len() as u64);
}

/// The three hints in the real capture are found, in order, with their real labels.
///
/// The fixture was produced by a real PTY, so this proves the scan works against bytes
/// nobody arranged for it.
#[test]
fn the_real_captures_three_hints_are_found_in_order() {
    let markers = markers_of(CAPTURED);
    let labels: Vec<&str> = markers.iter().map(|marker| marker.1.as_str()).collect();
    assert_eq!(
        labels,
        vec!["building vitrum-replay", "force push to main?", "done"]
    );
    let states: Vec<Option<HintState>> = markers.iter().map(|marker| marker.2).collect();
    assert_eq!(
        states,
        vec![
            Some(HintState::Working),
            Some(HintState::Approval),
            Some(HintState::Ready)
        ]
    );
    assert!(
        markers.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "positions must be strictly increasing"
    );
}

/// A stream with no escape bytes at all produces no markers and reads every byte once.
///
/// The scan skips non-`ESC` bytes rather than feeding them, and this pins that the skip
/// cannot swallow a hint that follows.
#[test]
fn a_long_run_with_no_escapes_does_not_hide_a_following_hint() {
    let mut bytes = vec![b'.'; 100_000];
    bytes.extend_from_slice(b"\x1b]7373;ready;after the flood\x07");
    let markers = markers_of(&bytes);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].0, bytes.len() as u64);
    assert_eq!(markers[0].1, "after the flood");
}

/// An unterminated hint produces no marker and does not swallow the next one.
///
/// The bug: a parser that stays inside an abandoned sequence forever, so a program that
/// emits a malformed hint loses every hint after it for the rest of the session.
#[test]
fn an_unterminated_hint_does_not_swallow_the_next_one() {
    let mut bytes = b"\x1b]7373;working;never terminated".to_vec();
    bytes.extend(core::iter::repeat_n(b'x', 400));
    bytes.extend_from_slice(b"\x1b]7373;ready;recovered\x07");
    let markers = markers_of(&bytes);
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].1, "recovered");
}
