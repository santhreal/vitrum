//! Framing a paste so the child reads it as text.
//!
//! A paste is not typing. The bytes arrive all at once, and a shell that
//! cannot tell the difference runs every line of a multi-line paste the moment
//! it arrives, including the last one, which may be half a command. Bracketed
//! paste is how a child says it can tell: it sets mode 2004, and the pane
//! wraps the payload in a start and an end marker so the child can read the
//! whole thing as one unit of text and decide what to do with it.
//!
//! # The end marker is the dangerous part
//!
//! A payload containing the end marker closes the bracket early, and
//! everything after it is read as if the operator had typed it. That is a
//! command injection through the clipboard, and the clipboard is a place
//! arbitrary programs and arbitrary web pages write to. The marker is
//! therefore removed from the payload rather than escaped: there is no
//! escaping in this framing to escape it with, so removal is the only answer
//! that closes the hole.
//!
//! # Line endings
//!
//! A terminal's Enter is a carriage return. A payload copied from a text file
//! carries line feeds, and a payload copied from a Windows program carries
//! both. Passing those through unconverted gives a shell a line feed where it
//! wants a return, which reads as a line that never ends.

/// Start of a bracketed paste.
const START: &[u8] = b"\x1b[200~";
/// End of a bracketed paste. Also the sequence a payload may not contain.
const END: &[u8] = b"\x1b[201~";

/// Frame `text` for a child.
///
/// `bracketed` is the emulator's real mode state, not a setting and not a
/// guess: a child that did not set 2004 does not know what the markers are and
/// receives them as literal text in its input.
pub(crate) fn frame(text: &str, bracketed: bool) -> Vec<u8> {
    let body = sanitize(text);
    if !bracketed {
        return body;
    }
    let mut out = Vec::with_capacity(body.len() + START.len() + END.len());
    out.extend_from_slice(START);
    out.extend_from_slice(&body);
    out.extend_from_slice(END);
    out
}

/// The payload, with line endings normalised and the end marker removed.
///
/// Applied whether or not the paste is bracketed. An unbracketed paste
/// carrying the end marker is not an injection, but it is still a sequence the
/// child will act on that the operator did not type, and the line endings are
/// wrong either way.
fn sanitize(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(END) {
            i += END.len();
            continue;
        }
        match bytes[i] {
            // Both line endings become the one a terminal's Enter sends. The
            // pair is consumed together so a Windows payload does not produce
            // two returns per line.
            b'\r' => {
                out.push(b'\r');
                i += 1;
                if bytes.get(i) == Some(&b'\n') {
                    i += 1;
                }
            }
            b'\n' => {
                out.push(b'\r');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WHY: a payload that contains the end marker closes the bracket early,
    /// and everything after it is executed as though it were typed. The
    /// clipboard is written by every program on the desktop and by every web
    /// page the operator copies from, so this is reachable by anything that
    /// can get text onto the clipboard.
    ///
    /// The invariant is absolute and is asserted as such: after framing, the
    /// end marker appears exactly once, at the very end. Not "the payload was
    /// escaped", because there is no escaping in this framing.
    ///
    /// Does not catch: a child that mis-parses a marker it received correctly.
    #[test]
    fn a_pasted_end_marker_cannot_close_the_bracket_early() {
        let hostile = "ls\x1b[201~\rrm -rf /\r";
        let framed = frame(hostile, true);

        let occurrences = framed
            .windows(END.len())
            .filter(|w| *w == END)
            .count();
        assert_eq!(occurrences, 1, "the marker survived inside the payload");
        assert!(framed.ends_with(END));
        assert_eq!(framed, b"\x1b[200~ls\rrm -rf /\r\x1b[201~");

        // Two markers, adjacent markers, and a marker at each edge.
        for payload in [
            "\x1b[201~",
            "\x1b[201~\x1b[201~",
            "a\x1b[201~b\x1b[201~c",
            "\x1b[201~start",
            "end\x1b[201~",
        ] {
            let framed = frame(payload, true);
            let count = framed.windows(END.len()).filter(|w| *w == END).count();
            assert_eq!(count, 1, "{payload:?} produced {count} markers");
            assert!(framed.ends_with(END), "{payload:?}");
        }
    }

    /// WHY: the marker is stripped even when the paste is not bracketed. A
    /// child that never set 2004 still acts on the sequence, and the operator
    /// still did not type it.
    #[test]
    fn the_end_marker_is_removed_from_an_unbracketed_paste_too() {
        let out = frame("a\x1b[201~b", false);
        assert_eq!(out, b"ab");
        assert!(!out.windows(END.len()).any(|w| w == END));
    }

    /// WHY: a child that did not set 2004 does not know what the markers are
    /// and receives them as literal input, which appears in its command line
    /// as `[200~`.
    #[test]
    fn markers_are_sent_only_to_a_child_that_asked_for_them() {
        assert_eq!(frame("hello", true), b"\x1b[200~hello\x1b[201~");
        assert_eq!(frame("hello", false), b"hello");
    }

    /// WHY: a terminal's Enter is a carriage return. A line feed where a
    /// return belongs reads to a shell as a line that never ended, and a
    /// Windows payload sending both produces a blank command between every
    /// pair of lines.
    #[test]
    fn every_line_ending_becomes_the_one_enter_sends() {
        let cases: &[(&str, &[u8])] = &[
            ("one\ntwo", b"one\rtwo"),
            ("one\r\ntwo", b"one\rtwo"),
            ("one\rtwo", b"one\rtwo"),
            ("one\n\ntwo", b"one\r\rtwo"),
            ("one\r\n\r\ntwo", b"one\r\rtwo"),
            ("trailing\n", b"trailing\r"),
            ("\n", b"\r"),
            ("\r\n", b"\r"),
            // A lone return followed by text is not a Windows pair and must
            // not swallow the text.
            ("a\rb\r\nc", b"a\rb\rc"),
        ];
        for &(input, want) in cases {
            assert_eq!(frame(input, false), want, "{input:?}");
        }
    }

    /// WHY: a paste is arbitrary bytes from an arbitrary program, and framing
    /// must not corrupt what it does pass through.
    #[test]
    fn everything_that_is_not_a_marker_or_a_line_ending_passes_through_exactly() {
        for payload in [
            "plain ascii",
            "unicode: \u{1f600} \u{4e2d}\u{6587} \u{0301}",
            "tabs\tand\tspaces  ",
            "\x1b[31mcolour\x1b[0m",
            "\x1b[200~",
            "\x00\x01\x7f",
            "",
        ] {
            let out = frame(payload, false);
            let want: Vec<u8> = payload.bytes().collect();
            assert_eq!(out, want, "{payload:?}");
        }
    }

    /// WHY: an empty clipboard is a real case, and a child that receives a
    /// bare pair of markers must not see a byte of payload between them.
    #[test]
    fn an_empty_paste_is_still_framed() {
        assert_eq!(frame("", true), b"\x1b[200~\x1b[201~");
        assert!(frame("", false).is_empty());
    }
}
