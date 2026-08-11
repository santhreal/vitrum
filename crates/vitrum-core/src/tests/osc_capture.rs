//! Title and directory capture, byte for byte.
//!
//! These are unit tests over `OutputScan`'s OSC half, driven the way a pty
//! drives it: whole sequences, sequences split at arbitrary boundaries, and
//! sequences a program got wrong. The end-to-end path through a real child is
//! `title` and `agent_title`; the abandonment bound is `osc_bound`; the bell
//! and the OSC 777 and 7373 sequences are `output_scan`. What is here is which
//! strings are believed and what they resolve to.
//!
//! The class these close: a capture that agrees with itself. Every assertion
//! names the exact string the sidebar would show, so a rule that quietly starts
//! keeping half a payload, or repairing one, fails rather than passing with
//! different text.

use crate::scan::OutputScan;

/// Feed `chunks` in order and take whatever the scan captured.
fn capture(chunks: &[&[u8]]) -> (Option<String>, Option<String>) {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    for chunk in chunks {
        scan.scan(chunk, &mut hints);
    }
    (scan.take_title(), scan.take_pwd())
}

/// The title one sequence produces.
fn title(bytes: &[u8]) -> Option<String> {
    capture(&[bytes]).0
}

/// The directory one sequence produces.
fn pwd(bytes: &[u8]) -> Option<String> {
    capture(&[bytes]).1
}

/// The two title identifiers set the title and nothing else does.
///
/// WHY: OSC 2 is the title and OSC 0 sets the icon name and the title
/// together, which is what most shells emit from their prompt. Handling only
/// one of them would leave the feature doing nothing for the common case.
///
/// OSC 1 is in the same family and is the icon name ALONE. It is not a title,
/// and a capture that took it would rename a session from a string the program
/// intended for a taskbar entry.
///
/// This does NOT check what happens to the name once a title is captured; that
/// is `title` and `agent_title`.
#[test]
fn the_title_identifiers_are_zero_and_two_and_nothing_else() {
    assert_eq!(title(b"\x1b]0;both\x07").as_deref(), Some("both"));
    assert_eq!(title(b"\x1b]2;window\x07").as_deref(), Some("window"));
    assert_eq!(title(b"\x1b]1;icon\x07"), None, "OSC 1 is the icon name");
    for ident in ["3", "4", "8", "9", "10", "11", "52", "133", "1337"] {
        let sequence = format!("\x1b]{ident};payload\x07");
        assert_eq!(
            title(sequence.as_bytes()),
            None,
            "OSC {ident} is not a title"
        );
    }
}

/// OSC 7 is the directory and is kept apart from the title.
///
/// WHY: both strings arrive on the same parser and are told apart only by the
/// number in front of them. A capture that crossed them would put a path in the
/// sidebar as a name, or send the branch lookup after a window title.
#[test]
fn the_directory_identifier_is_seven_and_does_not_touch_the_title() {
    let (name, dir) = capture(&[b"\x1b]7;file://host/src/project\x07"]);
    assert_eq!(name, None, "a directory report is not a title");
    assert_eq!(dir.as_deref(), Some("file://host/src/project"));

    let (name, dir) = capture(&[b"\x1b]2;deploy\x07\x1b]7;file://host/src/other\x07"]);
    assert_eq!(name.as_deref(), Some("deploy"));
    assert_eq!(dir.as_deref(), Some("file://host/src/other"));
}

/// Every terminator a real terminal honours ends a payload.
///
/// WHY: `OSC 2 ; text BEL` and `OSC 2 ; text ESC \` are the same sequence
/// written two ways and both are in the wild, so understanding one would
/// silently miss every title from the programs that use the other. A bare ESC
/// ends it too, because that is what a terminal does and it is what a program
/// relies on when it writes a title and immediately writes a CSI. CAN and SUB
/// are the two bytes that mean "abandon whatever you were parsing", and what
/// was read before them still counts.
///
/// This does NOT assert what the bytes AFTER the terminator do, which the
/// recovery test below covers.
#[test]
fn every_terminator_a_terminal_honours_closes_the_string() {
    let cases: &[(&[u8], &str)] = &[
        (b"\x1b]2;named\x07", "bell"),
        (b"\x1b]2;named\x1b\\", "string terminator"),
        (b"\x1b]2;named\x1b[0m", "a bare escape starting the next sequence"),
        (b"\x1b]2;named\x18", "cancel"),
        (b"\x1b]2;named\x1a", "substitute"),
    ];
    for (bytes, what) in cases {
        assert_eq!(
            title(bytes).as_deref(),
            Some("named"),
            "{what} must end the payload"
        );
    }
}

/// A string that ends leaves the scan on the fast path and ready for the next.
///
/// WHY: the capture runs byte at a time for as long as a string is open, so a
/// terminator that did not actually return to ground would move a session's
/// whole output path off the vectorised skip and leave the next title
/// concatenated onto this one.
#[test]
fn a_finished_string_returns_to_ground_and_the_next_one_is_read() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(b"\x1b]2;first\x07", &mut hints);
    assert!(!scan.mid_sequence(), "the string is over");
    scan.scan(b"ordinary output\r\n", &mut hints);
    assert!(!scan.mid_sequence());
    scan.scan(b"\x1b]2;second\x1b\\", &mut hints);
    assert_eq!(scan.take_title().as_deref(), Some("second"));
}

/// An identifier with no digits at all is refused.
///
/// WHY: `OSC ; text` is malformed, and defaulting the missing number to zero
/// would let one stray semicolon in a program's output retitle the session.
#[test]
fn a_string_with_no_identifier_is_discarded() {
    assert_eq!(title(b"\x1b];text\x07"), None);
    assert_eq!(pwd(b"\x1b];file://host/src/project\x07"), None);
}

/// An identifier longer than any real one is refused rather than accumulated.
///
/// WHY: the identifier is parsed into a counter. A payload of digits is the
/// obvious way to attack one, and there is no OSC number with six digits, so
/// the sixth is where a string stops being one of ours.
#[test]
fn an_over_long_identifier_is_discarded() {
    assert_eq!(title(b"\x1b]000002;text\x07"), None, "six digits is not an id");
    assert_eq!(
        title(b"\x1b]00002;text\x07").as_deref(),
        Some("text"),
        "five digits is still an ordinary leading-zero identifier"
    );
}

/// A control byte inside a payload is dropped and the payload carries on.
///
/// WHY: a program that pads its status line with a stray carriage return still
/// meant the text around it, and refusing the whole string would lose a title
/// that a terminal would have shown. This is the one place the capture is
/// deliberately lenient, so it is stated rather than inherited.
///
/// This does NOT apply to the hint sequence, where a control byte abandons the
/// declaration outright; `output_scan` owns that difference.
#[test]
fn a_control_byte_inside_a_payload_is_dropped_not_fatal() {
    assert_eq!(title(b"\x1b]2;na\rmed\x07").as_deref(), Some("named"));
    assert_eq!(title(b"\x1b]2;a\x00b\x01c\x07").as_deref(), Some("abc"));
    assert_eq!(
        pwd(b"\x1b]7;file://host/src/pro\x0bject\x07").as_deref(),
        Some("file://host/src/project")
    );
}

/// A payload that is not UTF-8 is dropped, not repaired.
///
/// WHY: the title is rendered in a sidebar and the directory is opened as a
/// path. A lossy conversion would put replacement characters in one and name a
/// directory that does not exist in the other, and both failures look like the
/// product being broken rather than the program being wrong.
#[test]
fn an_invalid_utf8_payload_is_dropped() {
    assert_eq!(title(b"\x1b]2;bad\xffname\x07"), None);
    assert_eq!(pwd(b"\x1b]7;file://host/\xc3\x28\x07"), None);
    // A truncated multi-byte character, which is what a program writing a
    // sliced buffer produces.
    assert_eq!(title(b"\x1b]2;\xe4\xb8\x07"), None);
    // The valid form of the same character survives, so this is not simply
    // refusing anything non-ASCII.
    assert_eq!(title(b"\x1b]2;\xe4\xb8\x96\x07").as_deref(), Some("世"));
}

/// A payload past the cap is refused whole rather than kept short.
///
/// WHY: half a title is not a title and half a path names the wrong directory,
/// so the payload is dropped rather than truncated. The cap also bounds what
/// output can make the daemon allocate, which matters because the payload is a
/// program's to choose the length of.
///
/// The boundary is asserted from both sides, so a cap that moved by one is
/// visible.
#[test]
fn an_over_long_payload_is_refused_rather_than_truncated() {
    let at_cap = "t".repeat(2048);
    let over_cap = "t".repeat(2049);
    assert_eq!(
        title(format!("\x1b]2;{at_cap}\x07").as_bytes()).as_deref(),
        Some(at_cap.as_str()),
        "a payload exactly at the cap is still a title"
    );
    assert_eq!(
        title(format!("\x1b]2;{over_cap}\x07").as_bytes()),
        None,
        "one byte over the cap must lose the whole payload"
    );
    assert_eq!(pwd(format!("\x1b]7;{over_cap}\x07").as_bytes()), None);
}

/// A refused payload costs only itself.
///
/// WHY: the refusal clears the buffer and the overflow flag. Leaving either
/// behind would make one over-long status line the last thing a session ever
/// reported, which is a permanent failure produced by one transient one.
#[test]
fn a_refused_payload_does_not_poison_the_next_one() {
    let over_cap = "t".repeat(2049);
    let stream = format!("\x1b]2;{over_cap}\x07\x1b]2;recovered\x07");
    assert_eq!(
        title(stream.as_bytes()).as_deref(),
        Some("recovered"),
        "a session that overflowed one title must keep reporting"
    );
}

/// An empty payload is a real value and not an absence.
///
/// WHY: programs clear the title on the way out, and the session layer has to
/// be able to tell "the program retracted its title" from "the program never
/// set one". Collapsing an empty payload to `None` would make a stale
/// `Action Required` unretractable.
#[test]
fn an_empty_payload_is_captured_as_an_empty_string() {
    assert_eq!(title(b"\x1b]2;\x07").as_deref(), Some(""));
    assert_eq!(title(b"\x1b]0;\x1b\\").as_deref(), Some(""));
    assert_eq!(pwd(b"\x1b]7;\x07").as_deref(), Some(""));
}

/// The last string in a burst is the one that is kept.
///
/// WHY: a run of output is published whole, so several titles routinely arrive
/// together. An agent that says `Working` and then `Ready` in one breath has
/// finished, and keeping the first would leave the sidebar a turn behind
/// forever.
#[test]
fn the_last_string_in_a_run_wins() {
    assert_eq!(
        title(b"\x1b]2;one\x07\x1b]2;two\x07\x1b]2;three\x07").as_deref(),
        Some("three")
    );
    assert_eq!(
        pwd(b"\x1b]7;file://h/a\x07\x1b]7;file://h/b\x07").as_deref(),
        Some("file://h/b")
    );
}

/// Taking a capture consumes it.
///
/// WHY: the session layer reads these once per published run and applies them.
/// A capture that survived being read would reapply an old title on every
/// subsequent run, which would undo a rename the operator made in between and
/// would look like the rename silently failing.
#[test]
fn a_capture_is_taken_once() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(b"\x1b]2;named\x07\x1b]7;file://h/src/p\x07", &mut hints);
    assert_eq!(scan.take_title().as_deref(), Some("named"));
    assert_eq!(scan.take_pwd().as_deref(), Some("file://h/src/p"));
    assert_eq!(scan.take_title(), None, "a title is reported once");
    assert_eq!(scan.take_pwd(), None, "a directory is reported once");

    scan.scan(b"plain output", &mut hints);
    assert_eq!(scan.take_title(), None, "output is not a title");
}

/// A sequence split at any byte is captured whole.
///
/// WHY: the kernel slices output wherever it likes and the coalescer publishes
/// whatever a run happened to contain, so a boundary can fall inside the
/// introducer, inside the identifier, inside the payload or between the two
/// bytes of the terminator. A capture that only matched whole sequences would
/// drop most real titles.
#[test]
fn a_sequence_split_at_every_byte_is_still_captured() {
    let whole: &[u8] = b"before\x1b]2;split title\x1b\\after";
    for at in 1..whole.len() {
        assert_eq!(
            capture(&[&whole[..at], &whole[at..]]).0.as_deref(),
            Some("split title"),
            "a split after byte {at} lost the title"
        );
    }
}

/// A sequence arriving one byte to a chunk is captured.
///
/// WHY: the split test above proves one boundary at a time. A program writing
/// unbuffered produces a boundary at every byte at once, which is the case that
/// catches state kept in a local rather than in the parser.
#[test]
fn a_sequence_dribbled_out_one_byte_at_a_time_is_captured() {
    let whole: &[u8] = b"\x1b]7;file://host/src/project\x07";
    let chunks: Vec<&[u8]> = whole.chunks(1).collect();
    assert_eq!(
        capture(&chunks).1.as_deref(),
        Some("file://host/src/project")
    );
}

/// A doubled escape is a cancel and a fresh introducer, not a lost byte.
///
/// WHY: a stream that dropped a byte must not be able to leave the parser
/// permanently one state behind. `ESC ESC ]` opens a string exactly as `ESC ]`
/// does, and a title written straight after an abandoned escape still arrives.
#[test]
fn a_doubled_escape_still_opens_a_string() {
    assert_eq!(title(b"\x1b\x1b]2;named\x07").as_deref(), Some("named"));
    assert_eq!(title(b"\x1b]\x1b]2;named\x07").as_deref(), Some("named"));
}

/// An introducer that is not `]` is not a string.
///
/// WHY: ESC starts every escape sequence there is. Treating the next byte as an
/// opener regardless would read a CSI's parameters as an OSC identifier and
/// then swallow the rest of the line looking for a terminator that is not
/// coming.
#[test]
fn an_escape_that_is_not_an_osc_opens_nothing() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(b"\x1b[2J\x1b[1;31mred\x1b[0m\x1b(B", &mut hints);
    assert_eq!(scan.take_title(), None);
    assert_eq!(scan.take_pwd(), None);
    assert!(!scan.mid_sequence(), "ordinary escapes leave nothing open");
}

/// A discarded string swallows its own payload, and only a terminator ends it.
///
/// WHY: the payload of an identifier we do not want can contain anything,
/// including semicolons and text. Returning to ground at the first `;` after
/// the identifier would let the contents of an OSC 52 clipboard write become
/// the session's name.
///
/// An ESC is the one thing that does end it early, because an ESC ends any
/// string: a program that opens a sequence and then writes another has ended
/// the first, and the parser must not stay in the discard for the rest of the
/// session's output. That is asserted here too, so the leniency is a stated
/// rule rather than a gap in the test above it.
#[test]
fn a_discarded_string_swallows_its_own_payload() {
    assert_eq!(
        title(b"\x1b]52;c;bm90IGEgdGl0bGU=\x07"),
        None,
        "a clipboard write is not a title"
    );
    assert_eq!(
        title(b"\x1b]52;c;one;two;three\x07"),
        None,
        "a semicolon inside a discarded payload does not restart the parse"
    );
    assert_eq!(
        title(b"\x1b]52;c;payload\x07\x1b]2;real\x07").as_deref(),
        Some("real"),
        "the string after a discarded one is read normally"
    );
    assert_eq!(
        title(b"\x1b]52;c;\x1b]2;real\x07").as_deref(),
        Some("real"),
        "an escape ends a discarded string exactly as it ends a kept one"
    );
}

/// A string left open holds the scan, and the scan says so.
///
/// WHY: `mid_sequence` is what tells the coalescer it may not skip ahead. A
/// capture that reported ground while a payload was still open would let the
/// vectorised skip run past a title's terminator.
#[test]
fn an_open_string_reports_itself_as_mid_sequence() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(b"\x1b]2;half a ti", &mut hints);
    assert!(scan.mid_sequence(), "the payload is still open");
    assert_eq!(scan.take_title(), None, "an unterminated title is not one");
    scan.scan(b"tle\x07", &mut hints);
    assert!(!scan.mid_sequence());
    assert_eq!(scan.take_title().as_deref(), Some("half a title"));
}
