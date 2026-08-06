//! The single pass over PTY output: BEL, OSC 777, and OSC 7373 hints.
//!
//! Every test here feeds the scanner the way the kernel feeds it, one chunk at
//! a time, because a read boundary lands wherever it lands and a scanner that
//! only works on whole sequences fails exactly when output is heaviest.

use vitrum_model::hint::{HintDeclaration, MAX_LABEL_CHARS, MAX_SEQUENCE_BYTES};
use vitrum_proto::HintState;

use crate::scan::OutputScan;

/// Feed `chunks` in order and return `(wanted the operator, declarations)`.
fn feed(chunks: &[&[u8]]) -> (bool, Vec<HintDeclaration>) {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    let mut wanted = false;
    for chunk in chunks {
        wanted |= scan.scan(chunk, &mut hints);
    }
    (wanted, hints)
}

fn declaration(state: HintState, label: Option<&str>) -> HintDeclaration {
    HintDeclaration {
        state,
        label: label.map(str::to_string),
    }
}

/// A bare BEL byte must raise the bell.
///
/// BEL is how every terminal program, taught or unknown, asks for a human. Miss
/// it and the sidebar's whole ordering feature silently degrades to creation
/// order, which is the failure this feature exists to fix.
#[test]
fn a_bare_bel_byte_is_a_signal() {
    assert!(feed(&[b"done\x07"]).0);
    assert!(feed(&[b"\x07"]).0);
    assert!(!feed(&[b"quiet output"]).0);
}

/// A BEL alone in its own chunk must be detected.
///
/// The degenerate split: the previous chunk ends exactly before the signal. A
/// scanner that only inspected chunk interiors would miss it.
#[test]
fn a_bel_alone_in_its_own_chunk_is_detected() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    assert!(!scan.scan(b"working", &mut hints));
    assert!(scan.scan(b"\x07", &mut hints));
}

/// An OSC 777 notification must raise the bell whether it ends in BEL or ST.
///
/// Scanning only for BEL would appear to work, because the common form ends in
/// BEL, and would then miss every ST-terminated notification.
#[test]
fn an_osc_777_notification_is_a_signal() {
    assert!(feed(&[b"\x1b]777;notify;title;body\x1b\\"]).0);
    assert!(feed(&[b"prefix\x1b]777;notify;t;b\x07suffix"]).0);
}

/// An OSC 777 introducer split at every possible boundary must still be found.
///
/// The kernel slices PTY output wherever it likes, so an introducer lands
/// across a chunk boundary regularly under load. A scanner without carry-over
/// would drop exactly the notifications that arrive during heavy output, which
/// is when an agent is most likely to be asking for something.
#[test]
fn an_osc_777_introducer_survives_every_split() {
    let whole: &[u8] = b"\x1b]777;notify";
    for split in 1..6 {
        let mut scan = OutputScan::new();
        let mut hints = Vec::new();
        assert!(
            !scan.scan(&whole[..split], &mut hints),
            "the first {split} bytes alone are not yet a signal"
        );
        assert!(
            scan.scan(&whole[split..], &mut hints),
            "signal split after {split} bytes must still be found"
        );
    }
}

/// An introducer sliced into three chunks must not be lost.
///
/// Six bytes can be cut twice, which is what defeats a scanner that keeps only
/// a one-chunk tail.
#[test]
fn an_osc_777_introducer_survives_two_slices() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    assert!(!scan.scan(b"\x1b]", &mut hints));
    assert!(!scan.scan(b"77", &mut hints));
    assert!(scan.scan(b"7;notify", &mut hints));
}

/// Carry-over must not manufacture a signal from unrelated bytes.
///
/// A false positive is as damaging as a miss: every session would claim to want
/// the operator and the ordering would be noise.
#[test]
fn a_partial_introducer_does_not_invent_a_signal() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    assert!(!scan.scan(b"ends with esc\x1b", &mut hints));
    assert!(!scan.scan(b"]778;not-the-one", &mut hints));
    assert!(!scan.scan(b"\x1b]77", &mut hints));
    assert!(!scan.scan(b"6;other", &mut hints));
}

/// An empty chunk must neither signal nor disturb a sequence in flight.
#[test]
fn an_empty_chunk_is_inert() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    assert!(!scan.scan(b"\x1b]777", &mut hints));
    assert!(!scan.scan(b"", &mut hints));
    assert!(
        scan.scan(b";x", &mut hints),
        "state must survive an empty chunk"
    );
}

/// A near-miss ESC immediately followed by the real introducer must be found.
///
/// The streaming matcher restarts on the failing byte; if it restarted on the
/// byte AFTER it, `\x1b\x1b]777;` would lose the signal because the second ESC
/// is both the failure and the next match's first byte.
#[test]
fn a_doubled_escape_before_the_introducer_still_matches() {
    assert!(feed(&[b"\x1b\x1b]777;notify"]).0);
    assert!(feed(&[b"\x1b]7\x1b]777;notify"]).0);
}

/// A BEL that terminates an OSC sequence must NOT ring the bell.
///
/// This is the bug the hint channel would otherwise introduce wholesale: the
/// legal terminator for `ESC ] 7373 ; ready` is a BEL, so every hint an agent
/// emitted would light up the attention indicator. The same applies to the
/// window title every shell prompt sets on every command.
#[test]
fn a_bel_terminating_an_osc_is_not_a_bell() {
    assert!(
        !feed(&[b"\x1b]0;my title\x07"]).0,
        "a window title must not demand the operator"
    );
    assert!(
        !feed(&[b"\x1b]7373;ready\x07"]).0,
        "a hint must not demand the operator through its own terminator"
    );
    assert!(
        feed(&[b"\x1b]0;my title\x07\x07"]).0,
        "a real BEL after the sequence is still a bell"
    );
}

/// A BEL terminator split from its payload must still be suppressed.
///
/// The suppression depends on the parser's phase surviving a chunk boundary; a
/// scanner that reset per chunk would ring on exactly the sequences that
/// straddle a read.
#[test]
fn a_split_osc_terminator_is_still_not_a_bell() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    assert!(!scan.scan(b"\x1b]0;title", &mut hints));
    assert!(!scan.scan(b"\x07rest", &mut hints));
}

/// An OSC 777 notification terminated by BEL is still a signal, from the
/// introducer, even though the terminating BEL itself is suppressed.
#[test]
fn osc_777_signals_from_its_introducer_not_its_terminator() {
    let (wanted, hints) = feed(&[b"\x1b]777;notify;done\x07"]);
    assert!(wanted);
    assert_eq!(hints, Vec::new());
}

/// A well-formed hint must be extracted, with both terminator forms.
#[test]
fn a_hint_is_extracted_with_either_terminator() {
    for sequence in [
        b"\x1b]7373;working\x07".as_slice(),
        b"\x1b]7373;working\x1b\\".as_slice(),
    ] {
        let (wanted, hints) = feed(&[sequence]);
        assert_eq!(hints, vec![declaration(HintState::Working, None)]);
        assert!(!wanted, "a hint is not a bell");
    }
}

/// Every state token must map to its state, and a label must come through.
#[test]
fn every_state_and_a_label_survive_the_scan() {
    let cases = [
        (
            b"\x1b]7373;approval;rm -rf build/\x07".as_slice(),
            HintState::Approval,
        ),
        (
            b"\x1b]7373;input;which file?\x07".as_slice(),
            HintState::Input,
        ),
        (
            b"\x1b]7373;working;compiling\x07".as_slice(),
            HintState::Working,
        ),
        (b"\x1b]7373;ready;done\x07".as_slice(), HintState::Ready),
    ];
    for (sequence, state) in cases {
        let (_, hints) = feed(&[sequence]);
        assert_eq!(hints.len(), 1, "{state:?}");
        assert_eq!(hints[0].state, state);
        assert!(hints[0].label.is_some(), "{state:?} lost its label");
    }
    let (_, labelled) = feed(&[b"\x1b]7373;approval;rm -rf build/\x07"]);
    assert_eq!(
        labelled,
        vec![declaration(HintState::Approval, Some("rm -rf build/"))]
    );
}

/// A hint split at every byte boundary must still be recognised exactly once.
///
/// A PTY read lands mid-sequence constantly; the whole reason to use a
/// byte-at-a-time parser is that the alternative silently drops hints under
/// load, which is when the agent is most likely to be declaring something.
#[test]
fn a_hint_survives_a_split_at_every_byte() {
    let whole: &[u8] = b"noise\x1b]7373;approval;may i?\x1b\\more";
    for split in 1..whole.len() {
        let (_, hints) = feed(&[&whole[..split], &whole[split..]]);
        assert_eq!(
            hints,
            vec![declaration(HintState::Approval, Some("may i?"))],
            "split after {split} bytes lost or duplicated the hint"
        );
    }
}

/// A hint arriving one byte per chunk must still be recognised.
///
/// The pathological slicing, and the one a scanner that peeks at chunk
/// interiors fails on completely.
#[test]
fn a_hint_arriving_one_byte_at_a_time_is_recognised() {
    let whole: &[u8] = b"\x1b]7373;ready;turn over\x1b\\";
    let chunks: Vec<&[u8]> = whole.chunks(1).collect();
    let (_, hints) = feed(&chunks);
    assert_eq!(
        hints,
        vec![declaration(HintState::Ready, Some("turn over"))]
    );
}

/// Two declarations in one chunk must both be reported, in order.
///
/// The session keeps the last one, but the scanner must not collapse them: a
/// caller that wanted the whole run would silently get only part of it.
#[test]
fn several_declarations_in_one_chunk_arrive_in_order() {
    let (_, hints) = feed(&[b"\x1b]7373;working\x07out\x1b]7373;ready;done\x1b\\"]);
    assert_eq!(
        hints,
        vec![
            declaration(HintState::Working, None),
            declaration(HintState::Ready, Some("done")),
        ]
    );
}

/// Malformed sequences must be rejected, never defaulted.
///
/// Any program can print any bytes, and a coding agent prints other programs'
/// output verbatim all day. A parser that guessed would let a log line rewrite
/// the sidebar.
#[test]
fn malformed_sequences_are_rejected() {
    let bad: &[&[u8]] = &[
        b"\x1b]7373\x07",             // no state
        b"\x1b]7373;\x07",            // empty state
        b"\x1b]7373;paused\x07",      // unknown state, must not default
        b"\x1b]7373;Ready\x07",       // wrong case
        b"\x1b]07373;ready\x07",      // padded number is not ours
        b"\x1b]737;ready\x07",        // wrong number
        b"\x1b]73730;ready\x07",      // longer number
        b"\x1b]777;notify;ready\x07", // someone else's OSC
        b"7373;ready\x07",            // no introducer
        b"\x1b[7373;ready\x07",       // CSI, not OSC
    ];
    for sequence in bad {
        let (_, hints) = feed(&[sequence]);
        assert_eq!(
            hints,
            Vec::new(),
            "accepted a malformed sequence: {:?}",
            String::from_utf8_lossy(sequence)
        );
    }
}

/// An unterminated sequence must be abandoned rather than buffered forever.
///
/// A stream that prints `ESC ]` and never terminates it must not be able to
/// make the daemon allocate, and must not swallow the next real hint.
#[test]
fn an_unterminated_sequence_is_abandoned_and_recovers() {
    let mut payload = Vec::from(*b"\x1b]7373;");
    payload.extend(std::iter::repeat_n(b'x', MAX_SEQUENCE_BYTES + 64));
    payload.extend_from_slice(b"\x1b]7373;ready\x07");
    let (_, hints) = feed(&[&payload]);
    assert_eq!(hints, vec![declaration(HintState::Ready, None)]);
}

/// A control byte inside a payload ends it as malformed.
///
/// A newline inside what looked like an OSC means the producer emitted
/// something else entirely, and continuing to buffer would fuse two unrelated
/// runs of output into one bogus sequence.
#[test]
fn a_control_byte_inside_a_payload_abandons_it() {
    let (_, hints) = feed(&[b"\x1b]7373;rea\ndy\x07"]);
    assert_eq!(hints, Vec::new());
}

/// An over-long label is truncated, not thrown away.
///
/// Losing a valid `approval` because its label ran long would discard the one
/// piece of information the operator most needs.
#[test]
fn an_over_long_label_is_truncated_not_rejected() {
    let long = "z".repeat(MAX_LABEL_CHARS + 40);
    let sequence = format!("\x1b]7373;approval;{long}\x07");
    let (_, hints) = feed(&[sequence.as_bytes()]);
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].state, HintState::Approval);
    assert_eq!(
        hints[0]
            .label
            .as_deref()
            .map(str::chars)
            .map(Iterator::count),
        Some(MAX_LABEL_CHARS)
    );
}

/// Ordinary escape-heavy output must produce nothing at all.
///
/// Agent output is mostly SGR and cursor motion. If any of that registered as a
/// hint or a bell, every session would be permanently lit.
#[test]
fn ordinary_ansi_output_is_silent() {
    let noisy = b"\x1b[2J\x1b[H\x1b[38;5;204mstatus\x1b[0m\r\n\x1b[1;31merror\x1b[m\r\n\x1b[?25l";
    let (wanted, hints) = feed(&[noisy]);
    assert!(!wanted);
    assert_eq!(hints, Vec::new());
}

/// A rejected sequence must be counted, so "never showed up" and "was
/// malformed" are distinguishable for a harness author.
#[test]
fn rejected_sequences_are_counted() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    assert_eq!(scan.rejected_hints(), 0);
    scan.scan(b"\x1b]7373;paused\x07", &mut hints);
    assert_eq!(scan.rejected_hints(), 1);
    scan.scan(b"\x1b]7373;ready\x07", &mut hints);
    assert_eq!(
        scan.rejected_hints(),
        1,
        "a good sequence must not be counted as rejected"
    );
    assert_eq!(hints, vec![declaration(HintState::Ready, None)]);
}

/// A hint and a real bell in the same chunk must both be seen.
///
/// The BEL suppression is positional, not chunk-wide; suppressing the whole
/// chunk would lose a genuine bell that happened to share a read with a hint.
#[test]
fn a_hint_and_a_real_bell_coexist() {
    let (wanted, hints) = feed(&[b"\x1b]7373;input;name?\x1b\\\x07"]);
    assert!(wanted, "the trailing bare BEL is still a bell");
    assert_eq!(hints, vec![declaration(HintState::Input, Some("name?"))]);
}
