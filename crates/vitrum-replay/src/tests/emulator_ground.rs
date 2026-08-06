//! The ground-state probe keyframes depend on.
//!
//! Every one of these tests defends the same invariant: [`Emulator::feed_byte`] may
//! answer `false` when the parser is in fact resumable, which only costs a keyframe,
//! but it must never answer `true` when it is not, which would produce a keyframe that
//! silently replays into a different screen.

use crate::emulator::Emulator;
use crate::palette::Palette;

fn probe(bytes: &[u8]) -> Vec<bool> {
    let mut emulator = Emulator::new(20, 4, Palette::XTERM).expect("geometry");
    bytes.iter().map(|&byte| emulator.feed_byte(byte)).collect()
}

/// A printable ASCII byte reaches ground, so almost every byte of ordinary output is a
/// safe keyframe boundary.
///
/// If this were false, keyframing would degrade into "slide forward until the scan
/// gives up" and every seek would replay from the start of the stream.
#[test]
fn every_printable_byte_reaches_ground() {
    assert_eq!(probe(b"hello"), vec![true; 5]);
}

/// Only the last byte of a multi-byte UTF-8 character reaches ground.
///
/// A keyframe taken after the lead byte of `é` would be restored into a fresh parser
/// with no partial character buffered; the continuation byte would then be printed as a
/// replacement character and every column after it on that row would be one off.
#[test]
fn only_the_final_byte_of_a_utf8_character_reaches_ground() {
    assert_eq!(probe("é".as_bytes()), vec![false, true]);
    assert_eq!(probe("日".as_bytes()), vec![false, false, true]);
    assert_eq!(
        probe("\u{1f600}".as_bytes()),
        vec![false, false, false, true],
        "a four-byte character is unsafe for three of its bytes"
    );
}

/// No byte inside a CSI sequence reaches ground until the final byte.
///
/// A keyframe taken after `ESC [ 3 1` would lose the colour entirely: the restored
/// parser sees a bare `m` and prints it.
#[test]
fn no_byte_inside_a_csi_sequence_reaches_ground_until_the_final_one() {
    assert_eq!(
        probe(b"\x1b[31m"),
        vec![false, false, false, false, true],
        "only the `m` is safe"
    );
    assert_eq!(
        probe(b"\x1b[?1049h"),
        vec![false, false, false, false, false, false, false, true]
    );
}

/// A C0 byte arriving inside a CSI sequence does not reach ground.
///
/// This is why [`vte::Perform::execute`] cannot be used as the ground signal. vte calls
/// `execute` for a carriage return in the middle of `ESC [ 1 \r 2 m`, and the parser is
/// still collecting parameters. A probe that trusted `execute` would keyframe there and
/// lose the `2 m`.
#[test]
fn a_control_byte_inside_a_csi_sequence_does_not_reach_ground() {
    let answers = probe(b"\x1b[1\r2m");
    assert_eq!(
        answers,
        vec![false, false, false, false, false, true],
        "the CR at index 3 is executed but the parser is still mid-sequence"
    );
}

/// An `ESC \` terminated OSC does not reach ground on the `ESC`.
///
/// This is why [`vte::Perform::osc_dispatch`] cannot be used as the signal either: vte
/// fires it on the `ESC` of the `ESC \` terminator and then moves to its escape state,
/// one byte short of ground. Every OSC 7373 hint in the fixture with an `ESC \`
/// terminator would otherwise produce an unresumable keyframe.
#[test]
fn an_esc_terminated_osc_reaches_ground_only_on_the_backslash() {
    let answers = probe(b"\x1b]0;t\x1b\\");
    assert_eq!(answers.len(), 7);
    assert!(
        !answers[5],
        "the ESC of the terminator fires osc_dispatch but leaves the parser in escape"
    );
    assert!(answers[6], "the backslash is the byte that reaches ground");
    assert!(answers[..5].iter().all(|safe| !safe));
}

/// A `BEL` terminated OSC reaches ground on the `BEL` itself.
///
/// The probe does not report it, because `BEL` arrives through `execute`, which is not a
/// usable signal. Reporting `false` here is the safe direction: the next printable byte
/// is reported instead and the keyframe slides one byte forward.
#[test]
fn a_bel_terminated_osc_is_reported_conservatively() {
    let answers = probe(b"\x1b]0;t\x07X");
    assert!(
        !answers[5],
        "the BEL is not reported, which costs a byte of slide and never correctness"
    );
    assert!(answers[6], "the following printable byte is");
}

/// A two-byte escape reaches ground on its second byte.
#[test]
fn a_two_byte_escape_reaches_ground_on_its_second_byte() {
    assert_eq!(probe(b"\x1b7"), vec![false, true]);
    assert_eq!(probe(b"\x1bM"), vec![false, true]);
    assert_eq!(probe(b"\x1b(0"), vec![false, false, true], "with an intermediate");
}

/// A DCS payload never reaches ground until its terminator's final byte.
///
/// A keyframe inside a sixel image would be restored into a parser that reads the rest
/// of the image data as text and prints it across the screen.
#[test]
fn a_dcs_payload_does_not_reach_ground_until_its_terminator_completes() {
    let answers = probe(b"\x1bPq#0;2;0;0;0\x1b\\X");
    let last = answers.len() - 1;
    assert!(
        answers[..last - 1].iter().all(|safe| !safe),
        "nothing inside the device control string is safe"
    );
    assert!(answers[last - 1], "the backslash completes the terminator");
    assert!(answers[last]);
}

/// An unterminated sequence never reaches ground, however long the stream runs.
///
/// This is the case [`crate::config::DEFAULT_GROUND_SCAN`] bounds. A program that emits
/// `ESC ]` and never terminates it must not make the index build scan forever.
#[test]
fn an_unterminated_osc_never_reaches_ground() {
    let mut bytes = b"\x1b]0;".to_vec();
    bytes.extend(core::iter::repeat_n(b'x', 500));
    assert!(
        probe(&bytes).iter().all(|safe| !safe),
        "five hundred bytes into an open OSC string, still not resumable"
    );
}

/// Feeding one byte at a time and feeding the whole run produce the same screen.
///
/// The probe exists to be used byte by byte, and if that path diverged from the bulk
/// path then every keyframe would be taken from a slightly different terminal.
#[test]
fn byte_at_a_time_and_bulk_feeding_agree() {
    let bytes = crate::tests::support::CAPTURED;

    let mut single = Emulator::new(80, 24, Palette::XTERM).expect("geometry");
    for &byte in bytes {
        single.feed_byte(byte);
    }

    let mut bulk = Emulator::new(80, 24, Palette::XTERM).expect("geometry");
    bulk.feed(bytes);

    assert_eq!(single.screen(), bulk.screen());
}

/// Restoring a screen at a reported ground boundary reproduces the linear replay.
///
/// This is the invariant stated as an experiment rather than as an argument: at every
/// byte the probe calls safe, a fresh parser resumed from that screen and fed the rest
/// of the stream lands on the same screen as one that never stopped.
#[test]
fn resuming_at_every_reported_boundary_matches_the_linear_replay() {
    let bytes = crate::tests::support::CAPTURED;
    let reference = crate::tests::support::screen80(bytes);

    let mut walker = Emulator::new(80, 24, Palette::XTERM).expect("geometry");
    let mut checked = 0usize;
    for (index, &byte) in bytes.iter().enumerate() {
        if !walker.feed_byte(byte) {
            continue;
        }
        let mut resumed = Emulator::resume(walker.screen().clone());
        resumed.feed(&bytes[index + 1..]);
        assert_eq!(
            resumed.screen(),
            &reference,
            "resuming after byte {index} diverged"
        );
        checked += 1;
    }
    assert!(
        checked > 500,
        "the capture should offer hundreds of boundaries, found {checked}"
    );
}
