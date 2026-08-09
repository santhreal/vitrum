//! The title and directory capture must answer what a real terminal answers.
//!
//! The daemon used to learn a session's title and working directory by running
//! a full libghostty terminal engine over every byte and reading two strings
//! back off it. Measured through a real pty that parse was the majority of what
//! a session spent moving a megabyte, for a screen model nothing in the daemon
//! ever reads, so the two strings are now extracted by a state machine in
//! [`crate::scan`].
//!
//! The class this closes: the replacement quietly understanding fewer forms of
//! the same sequence than the engine did. A title that stops arriving is not a
//! crash and not a wrong value on screen; it is approval detection going quiet,
//! which looks exactly like an agent that is not blocked.
//!
//! So these are differential, not expectation-based. The engine is the oracle
//! and it is still in the build, so every case asserts that the cheap path and
//! the real terminal reached the same answer. Writing down what the answer
//! "should" be would only encode whatever this author believed about xterm.
//!
//! Every case is also run split at every single byte boundary, because a read
//! from a pty lands wherever the kernel put it: the introducer, the identifier,
//! the payload and the two-byte terminator all routinely arrive in different
//! chunks, and a capture that only works on whole sequences works only in
//! tests.
//!
//! What it does not catch: forms neither implementation understands, and any
//! payload long enough to hit either side's length bound, which is a deliberate
//! refusal here rather than agreement about a value.

use crate::scan::OutputScan;

/// What one implementation extracted from a stream.
#[derive(Debug, Default, PartialEq, Eq)]
struct Extracted {
    title: Option<String>,
    pwd: Option<String>,
}

/// Run the daemon's capture over `chunks`, last value wins.
fn by_scan(chunks: &[&[u8]]) -> Extracted {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    let mut out = Extracted::default();
    for chunk in chunks {
        scan.scan(chunk, &mut hints);
        if let Some(title) = scan.take_title() {
            out.title = Some(title);
        }
        if let Some(pwd) = scan.take_pwd() {
            out.pwd = Some(pwd);
        }
    }
    out
}

/// Run a real terminal engine over `chunks`, drained the way the daemon drained
/// it when the engine was on this path.
fn by_engine(chunks: &[&[u8]]) -> Extracted {
    let mut vt = vitrum_vt::Vt::new(vitrum_vt::VtOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 0,
    })
    .expect("a terminal engine");
    let mut out = Extracted::default();
    for chunk in chunks {
        vt.feed(chunk);
        if let Some(title) = vt.events().take_title() {
            out.title = Some(title);
        }
        if let Some(pwd) = vt.events().take_pwd() {
            out.pwd = Some(pwd);
        }
    }
    out
}

/// Every sequence both implementations are expected to agree about.
///
/// Named so a failure says which form broke rather than printing bytes. The
/// list is the contract: an OSC form the daemon must keep understanding belongs
/// here, and one that is added to the capture without being added here is
/// untested.
fn corpus() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("osc 2 bel", b"\x1b]2;a title\x07"),
        ("osc 2 st", b"\x1b]2;a title\x1b\\"),
        ("osc 0 bel", b"\x1b]0;icon and title\x07"),
        ("osc 0 st", b"\x1b]0;icon and title\x1b\\"),
        ("osc 1 icon only", b"\x1b]1;just an icon\x07"),
        ("osc 7 bel", b"\x1b]7;file:///src/vitrum\x07"),
        ("osc 7 st", b"\x1b]7;file:///src/vitrum\x1b\\"),
        ("osc 7 with host", b"\x1b]7;file://box/src/vitrum\x1b\\"),
        ("osc 7 percent encoded", b"\x1b]7;file:///src/two%20words\x07"),
        ("empty title bel", b"\x1b]2;\x07"),
        ("empty title st", b"\x1b]2;\x1b\\"),
        ("title then retitle", b"\x1b]2;first\x07\x1b]2;second\x07"),
        ("title then clear", b"\x1b]2;first\x07\x1b]2;\x1b\\"),
        ("title and pwd", b"\x1b]2;name\x07\x1b]7;file:///src\x07"),
        ("pwd then title", b"\x1b]7;file:///src\x1b\\\x1b]2;name\x1b\\"),
        ("utf8 title", "\x1b]2;kärnkraft ✓ 日本\x07".as_bytes()),
        ("title with spaces", b"\x1b]2;   padded title   \x07"),
        ("title amid text", b"before\x1b]2;mid\x07after\r\n"),
        ("title amid sgr", b"\x1b[1;32mgreen\x1b[0m\x1b]2;mid\x07\x1b[2Kmore"),
        ("csi only", b"\x1b[1;32mgreen\x1b[0m\r\n"),
        ("plain text", b"no escapes here at all\r\n"),
        ("bel alone", b"ding\x07dong"),
        ("unknown osc", b"\x1b]9;a notification\x07"),
        ("osc 777 notification", b"\x1b]777;notify;hi;there\x07"),
        ("osc 8 hyperlink", b"\x1b]8;;https://example.invalid\x07link\x1b]8;;\x07"),
        ("osc 4 palette", b"\x1b]4;1;rgb:ff/00/00\x07"),
        ("osc 133 prompt", b"\x1b]133;A\x07$ \x1b]133;B\x07"),
        ("title with no terminator", b"\x1b]2;unterminated and then some"),
        ("title cancelled by esc", b"\x1b]2;cancelled\x1bXleftover\x07"),
        ("cr in title", b"\x1b]2;before\rafter\x07"),
        ("tab in title", b"\x1b]2;before\tafter\x07"),
        ("cancel in title", b"\x1b]2;before\x18after\x07"),
        ("substitute in title", b"\x1b]2;before\x1aafter\x07"),
        ("nul in title", b"\x1b]2;before\x00after\x07"),
        ("delete in title", b"\x1b]2;before\x7fafter\x07"),
        ("newline in title", b"\x1b]2;before\nafter\x07"),
        ("control in pwd", b"\x1b]7;file:///src\rmore\x07"),
        ("osc identifier only", b"\x1b]2\x07"),
        ("title then unknown osc", b"\x1b]2;kept\x07\x1b]11;?\x07"),
        ("nested introducer", b"\x1b]2;first\x1b]2;second\x07"),
        ("double escape", b"\x1b\x1b]2;after a stray escape\x07"),
        ("osc with no identifier", b"\x1b];no identifier\x07"),
    ]
}

/// Both implementations see the same title and directory in a whole sequence.
#[test]
fn the_capture_agrees_with_a_real_terminal() {
    for (name, bytes) in corpus() {
        assert_eq!(
            by_scan(&[bytes]),
            by_engine(&[bytes]),
            "{name}: the capture and the terminal engine disagree"
        );
    }
}

/// And in a sequence split anywhere a pty read could split it.
///
/// Every boundary, not a sampled one. The interesting splits are the ones
/// inside the two-byte ST terminator and between the identifier and its
/// semicolon, and picking boundaries by hand is how those get missed.
#[test]
fn the_capture_agrees_however_the_reads_land() {
    for (name, bytes) in corpus() {
        for at in 1..bytes.len() {
            let split = [&bytes[..at], &bytes[at..]];
            assert_eq!(
                by_scan(&split),
                by_engine(&split),
                "{name}: disagreement when the read boundary falls at byte {at}"
            );
        }
    }
}

/// And when the stream arrives one byte at a time.
///
/// The degenerate case a slow interactive session actually produces: a program
/// writing a title character by character gives the daemon a chunk per byte.
#[test]
fn the_capture_agrees_byte_by_byte() {
    for (name, bytes) in corpus() {
        let single: Vec<&[u8]> = bytes.chunks(1).collect();
        assert_eq!(
            by_scan(&single),
            by_engine(&single),
            "{name}: disagreement when every byte is its own read"
        );
    }
}

/// A payload no title could be is refused rather than retained.
///
/// This one is not differential. Output chooses how many bytes it writes before
/// a terminator, so a capture that grew a buffer to fit would let a session
/// decide how much memory the daemon allocates. The bound is the contract, and
/// what a terminal engine does with a 64 KiB title is its own business.
#[test]
fn an_unbounded_payload_is_refused_not_retained() {
    let mut stream = Vec::from(b"\x1b]2;".as_slice());
    stream.extend(std::iter::repeat_n(b'x', 64 * 1024));
    stream.extend_from_slice(b"\x07");

    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(&stream, &mut hints);
    assert_eq!(
        scan.take_title(),
        None,
        "a 64 KiB title must be dropped, not stored"
    );

    // And the parser is still in step afterwards: refusing one payload must not
    // cost the next title, or a single hostile sequence would silence approval
    // detection for the life of the session.
    scan.scan(b"\x1b]2;back to normal\x07", &mut hints);
    assert_eq!(scan.take_title(), Some("back to normal".to_string()));
}

/// A title is reported once, not once per scan.
///
/// `take_title` is how the coalescer decides whether anything changed, and a
/// capture that kept answering with the same string would push a projection
/// update to every attached window for every chunk of output.
#[test]
fn a_title_is_reported_once() {
    let mut scan = OutputScan::new();
    let mut hints = Vec::new();
    scan.scan(b"\x1b]2;once\x07", &mut hints);
    assert_eq!(scan.take_title(), Some("once".to_string()));
    scan.scan(b"more output with no title in it\r\n", &mut hints);
    assert_eq!(scan.take_title(), None);
}
