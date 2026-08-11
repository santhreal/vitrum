//! What the client half of the data plane owes the pane.
//!
//! The server suites in `crates/vitrum-server/src/tests/seam_*.rs` prove the
//! daemon emits a contiguous, exactly-once byte stream. Nothing there can
//! prove what THIS process does with it. Everything below drives
//! [`PaneStream`] and [`Frame`] directly: they are pure by construction, so a
//! case here is the shipped path with no socket, no surface and no timing.

use super::*;

const S: SessionId = SessionId(7);
const OTHER: SessionId = SessionId(8);

/// Four bytes of UTF-8: U+1F600, the character every split test uses.
///
/// Chosen over a two-byte one because four bytes can be split three ways, and
/// a reassembler that only handles a one-byte tail is a real bug shape.
const GRIN: [u8; 4] = [0xF0, 0x9F, 0x98, 0x80];

/// One data frame as the daemon would put it on the socket, parsed.
fn frame(session: SessionId, seq: u64, payload: &[u8]) -> Frame {
    Frame::parse(vitrum_proto::encode_output(session, seq, payload))
        .expect("a frame this module just encoded")
}

/// Drive one call and return what it emitted.
fn ops(f: impl FnOnce(&mut PaneStream, &mut Vec<PaneOp>)) -> Vec<PaneOp> {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    f(&mut stream, &mut out);
    out
}

/// Every `Write` payload in `ops`, concatenated in emission order.
///
/// The pane applies ops in order and `Write` is the only one that reaches the
/// screen, so this is exactly the byte sequence the parser will see.
fn painted(ops: &[PaneOp]) -> Vec<u8> {
    let mut all = Vec::new();
    for op in ops {
        if let PaneOp::Write(chunk) = op {
            all.extend_from_slice(chunk.bytes());
        }
    }
    all
}

/// A pane that records what it was told, and where the bytes came from.
///
/// `addresses` is the point of it: it holds the address of the first byte of
/// every slice handed to [`PaneSink::write`], which is what proves the payload
/// reached the parser without being copied out of the message it arrived in.
#[derive(Default)]
struct Recorder {
    log: Vec<String>,
    bytes: Vec<u8>,
    addresses: Vec<usize>,
    flushes: usize,
}

impl PaneSink for Recorder {
    fn reset(&mut self) {
        self.log.push("reset".to_string());
    }
    fn write(&mut self, bytes: &[u8]) {
        self.log.push(format!("write {}", bytes.len()));
        self.addresses.push(bytes.as_ptr() as usize);
        self.bytes.extend_from_slice(bytes);
    }
    fn scroll_from_end(&mut self, lines: u32) {
        self.log.push(format!("scroll_from_end {lines}"));
    }
    fn keep_view(&mut self) {
        self.log.push("keep_view".to_string());
    }
    fn flush(&mut self) {
        self.flushes += 1;
    }
}

// ---------------------------------------------------------------------------
// The cost of the path
// ---------------------------------------------------------------------------

/// A live payload must reach the pane at the address it arrived at.
///
/// WHY: this is the whole latency claim, expressed as something a test can
/// decide. Every encoding the old path had — a base64 string, a JSON document,
/// a length-prefixed op buffer, an HTTP response body — moves the bytes to a
/// new address. So does a defensive `to_vec`, a `String::from_utf8` and a
/// `Cow::into_owned`. If the slice the parser is handed still points inside the
/// allocation tungstenite produced, then nothing on the path between them
/// copied, re-encoded, or revalidated it, and no such step can be added
/// without turning this red.
///
/// What this does NOT catch: a copy the pane itself makes after `write`
/// returns, which is `vitrum-vt`'s claim, and a copy inside tungstenite's own
/// read, which is the one copy this design accepts.
#[test]
fn a_live_payload_reaches_the_pane_without_being_copied() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);
    out.clear();

    let mut want = Vec::new();
    for i in 0..64u64 {
        let payload = vec![b'a' + (i % 26) as u8; 512];
        let wire = vitrum_proto::encode_output(S, i * 512, &payload);
        // The address the payload occupies inside the message the socket
        // produced, recorded before the message is moved anywhere.
        want.push(wire.as_ptr() as usize + OUTPUT_HEADER_LEN);
        stream.output(Frame::parse(wire).expect("well formed"), &mut out);
    }

    let mut sink = Recorder::default();
    apply(&out, &mut sink);

    assert_eq!(sink.addresses, want, "a payload was copied on the way in");
    assert_eq!(sink.bytes.len(), 64 * 512);
    assert_eq!(
        sink.flushes, 1,
        "sixty-four frames in one batch are one present, not sixty-four"
    );
}

/// Nothing on the per-chunk path may encode, decode or allocate.
///
/// WHY: the address test above proves today's code does not copy. This proves
/// the shape that would make it copy is absent, which is the guard that
/// survives someone adding a "quick" conversion. The scan is over the two
/// functions a live byte passes through and nothing else, derived from the
/// shipped source at run time so a renamed function fails loudly rather than
/// scanning nothing.
///
/// `format!` is allowed inside `output` because the only one there builds a
/// gap notice, which happens when the stream is already broken. The scan
/// therefore checks the encoding vocabulary, not allocation in general.
///
/// What this does NOT catch: an allocation inside a callee, which is why the
/// address assertion above exists as well.
#[test]
fn the_per_chunk_path_contains_no_encoding_of_any_kind() {
    let src: &str = include_str!("../socket.rs");

    let spans = [
        ("pub(crate) fn output(", "\n    }\n"),
        ("fn apply(ops: &[PaneOp]", "\n}\n"),
        ("Message::Binary(bytes) =>", "\n        Message::Close"),
    ];
    for (open, close) in spans {
        let at = src
            .find(open)
            .unwrap_or_else(|| panic!("`{open}` is no longer in socket.rs; the scan is blind"));
        let end = src[at..]
            .find(close)
            .map(|i| at + i)
            .unwrap_or_else(|| panic!("`{open}` no longer ends with `{close}`"));
        let body = &src[at..end];
        for banned in [
            "serde_json",
            "base64",
            "b64",
            "to_vec",
            "String::from_utf8",
            "from_utf8_lossy",
            "to_owned",
        ] {
            assert!(
                !body.contains(banned),
                "`{banned}` is on the per-chunk path in `{open}`; every byte an \
                 agent writes now pays for it"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Sequence continuity
// ---------------------------------------------------------------------------

/// A gap inside one attachment must be reported, and a resume after a
/// reconnect must not be.
///
/// WHY: the seq on a data frame is the session's cumulative byte count, and
/// the client is the only place the two ends can be compared. Painting across
/// a gap corrupts the parse from there on, because the missing byte is as
/// likely to be inside an escape sequence as inside a word. The opposite error
/// is worse in practice: a re-attach legitimately resumes at wherever the child
/// has reached, and a client that treated that as a gap would cry corruption on
/// every tab switch. The server end of this pair is
/// `seam_stream::attach_detach_and_reattach_carry_exact_bytes_and_no_backlog`.
///
/// What this does NOT catch: reordering, which a WebSocket cannot do, and a gap
/// the daemon itself introduced, which is the server suite's claim.
#[test]
fn a_gap_within_one_attachment_is_reported_and_a_reconnect_resume_is_not() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();

    stream.focus(Some(S), &mut out);
    // The backfill for the focus, so the live path is not holding frames.
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);

    stream.output(frame(S, 0, b"abc"), &mut out);
    stream.output(frame(S, 3, b"def"), &mut out);
    assert!(
        stream.take_notices().is_empty(),
        "contiguous frames must be silent"
    );

    // One byte missing between 6 and 7.
    stream.output(frame(S, 7, b"ghi"), &mut out);
    let notices = stream.take_notices();
    assert_eq!(notices.len(), 1, "one gap, one notice");
    assert!(
        notices[0].contains("byte 6") && notices[0].contains("byte 7"),
        "the notice must name both offsets so the hole is measurable: {}",
        notices[0]
    );

    // A reconnect: the socket died, the client re-attached, and the daemon
    // resumed at the child's current offset. Nothing is owed and nothing is
    // wrong.
    out.clear();
    stream.focus(Some(S), &mut out);
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);
    stream.output(frame(S, 90_000, b"resumed"), &mut out);
    assert!(
        stream.take_notices().is_empty(),
        "a re-attach resumes wherever the child reached; that is not a gap"
    );

    // And contiguity is re-armed from the new offset, not from the old one.
    stream.output(frame(S, 90_010, b"x"), &mut out);
    assert_eq!(
        stream.take_notices().len(),
        1,
        "the second attachment must be checked against its own first frame"
    );
}

/// Frames for a session the pane is not showing must not reach the screen.
///
/// WHY: with twenty agents running, most frames on the socket are for some
/// other pane. Painting one is another session's output appearing
/// mid-transcript, and, worse, it would poison the contiguity check for the
/// session that IS focused.
#[test]
fn a_frame_for_another_session_is_dropped_without_disturbing_the_focused_one() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);
    out.clear();

    stream.output(frame(S, 0, b"mine"), &mut out);
    stream.output(frame(OTHER, 999, b"theirs"), &mut out);
    stream.output(frame(S, 4, b"also mine"), &mut out);

    assert_eq!(painted(&out).as_slice(), b"minealso mine");
    assert!(
        stream.take_notices().is_empty(),
        "the foreign frame must not have moved the focused session's offset"
    );
}

// ---------------------------------------------------------------------------
// Backlog, exactly once
// ---------------------------------------------------------------------------

/// History and the live bytes held behind it must meet exactly, with no byte
/// repeated and none lost.
///
/// WHY: an attach starts the live stream at the head as of the attach, and the
/// backfill is computed at the head as of the scrollback request. The two
/// windows overlap by exactly the bytes the child emitted in between, and the
/// offset is the only thing that says how many. One byte of overlap repeats a
/// line; one byte of gap corrupts the parse.
/// `seam_stream::backfill_and_live_meet_exactly_at_the_attach_offset` proves
/// the daemon's two answers abut; this proves the client's splice does.
///
/// What this does NOT catch: a ring that evicted the joint, which the next case
/// owns.
#[test]
fn held_live_bytes_are_spliced_onto_history_exactly_once() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);

    // Live frames arriving while the scrollback request is in flight. The
    // first straddles the resume offset, which is the case the splice exists
    // for: three of its bytes are already inside the history.
    stream.output(frame(S, 7, b"HIJK"), &mut out);
    stream.output(frame(S, 11, b"L"), &mut out);
    assert!(
        painted(&out).is_empty(),
        "nothing may reach the screen while a backfill is in flight"
    );

    // History is bytes 0..10 of the stream, so the live stream is owed from 10.
    stream.backfill(S, 0, 10, b"ABCDEFGHIJ".to_vec(), None, false, &mut out);

    assert_eq!(
        painted(&out).as_slice(),
        b"ABCDEFGHIJKL",
        "the overlapping prefix of the straddling frame must be dropped, not \
         repeated and not skipped"
    );
    assert!(stream.take_notices().is_empty());
}

/// A backfill that starts above the held live bytes must report the hole.
///
/// WHY: this is the reverse overlap, and it is real: after a reported gap the
/// bytes between the history and the first live frame may have been evicted
/// from the daemon's ring, so `resume_seq` lands BELOW the oldest frame the
/// client holds. Painting the frames anyway is correct, since the screen was
/// reset first and the alternative is a splice at a wrong offset, but the hole
/// is history the operator will never see and silently swallowing it is how a
/// transcript acquires an invisible edit.
#[test]
fn history_evicted_between_the_backfill_and_the_live_bytes_is_named() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.output(frame(S, 100, b"live"), &mut out);
    stream.backfill(S, 0, 90, b"history".to_vec(), None, false, &mut out);

    assert_eq!(painted(&out).as_slice(), b"historylive");
    let notices = stream.take_notices();
    assert_eq!(notices.len(), 1);
    assert!(
        notices[0].starts_with("10 bytes"),
        "the notice must measure the hole: {}",
        notices[0]
    );
}

/// A hole is measured against the painted stream, not against the resume
/// offset, so contiguous frames after the splice report nothing.
///
/// WHY: this is a regression. The splice asked each held frame whether it
/// started above `resume_seq`, which is only the right question for the first
/// one: every later frame of a healthy run starts above it by exactly the bytes
/// its predecessors carried, and each was reported as evicted history. A false
/// hole is worse than a silent one, because the notice tells the operator the
/// transcript they are reading has bytes missing from it when it does not.
///
/// Three frames, because two is the shortest run that shows the bug and three
/// proves the offset keeps advancing rather than being right once.
#[test]
fn a_run_of_contiguous_frames_reports_no_hole_at_all() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);

    stream.output(frame(S, 8, b"IJKL"), &mut out);
    stream.output(frame(S, 12, b"MN"), &mut out);
    stream.output(frame(S, 14, b"OP"), &mut out);

    // History ends at 10, inside the first held frame, so the splice drops two
    // bytes of it and every later frame is already beyond the join.
    stream.backfill(S, 0, 10, b"ABCDEFGHIJ".to_vec(), None, false, &mut out);

    assert_eq!(
        painted(&out).as_slice(),
        b"ABCDEFGHIJKLMNOP",
        "the splice must drop the overlap once and keep every later frame whole"
    );
    assert_eq!(
        stream.take_notices(),
        Vec::<String>::new(),
        "a contiguous run has no missing history to report"
    );
}

/// Overflowing the pending buffer must paint the live bytes and discard the
/// backfill that lands afterwards.
///
/// WHY: a stalled repaint may not turn into unbounded client memory just
/// because an agent is chatty, and the live bytes are what the operator
/// actually needs. The half of that decision the tests keep missing is the
/// SECOND half: the abandoned backfill still arrives, and painting it after the
/// live bytes that already reached the screen would rewind the transcript.
#[test]
fn overflowing_the_pending_buffer_flushes_live_and_discards_the_late_backfill() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);

    let chunk = vec![b'x'; PENDING_CAP / 2 + 1];
    stream.output(frame(S, 0, &chunk), &mut out);
    assert!(painted(&out).is_empty(), "one chunk is still under the cap");
    stream.output(frame(S, chunk.len() as u64, &chunk), &mut out);

    assert_eq!(
        painted(&out).len(),
        chunk.len() * 2,
        "past the cap every held byte is painted at once"
    );
    let notices = stream.take_notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("overflowed"), "{}", notices[0]);

    out.clear();
    stream.backfill(S, 0, 0, b"history".to_vec(), None, false, &mut out);
    assert!(
        out.is_empty(),
        "the abandoned backfill must not rewind the screen, and must not reset it"
    );

    // And the pane is live again rather than stuck holding frames.
    stream.output(frame(S, chunk.len() as u64 * 2, b"after"), &mut out);
    assert_eq!(painted(&out).as_slice(), b"after");
}

// ---------------------------------------------------------------------------
// The multi-byte guarantee
// ---------------------------------------------------------------------------

/// A character split across two frames must be contiguous in what is painted.
///
/// WHY: this is the guarantee the whole data plane is shaped around, and it is
/// the one that breaks the instant anything on the path types a payload as
/// text. A `String` anywhere between the socket and the parser turns a split
/// U+1F600 into two replacement characters, which is three bytes where there
/// were two and shifts every offset after it: search hits point at the wrong
/// column and scrollback pages stop abutting.
/// `seam_stream::invalid_utf8_crosses_the_socket_verbatim` proves the bytes
/// reach this process intact. This proves they leave it intact, through the
/// splice, which is the one place in the client that concatenates frames.
///
/// What this does NOT catch: what the terminal engine does with the bytes,
/// which is `vitrum-vt`'s question.
#[test]
fn a_character_split_across_two_frames_is_whole_before_it_reaches_the_pane() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);

    // Split two-and-two while a backfill is in flight, so both halves are held
    // and the splice is what has to put them back together.
    stream.output(frame(S, 0, &GRIN[..2]), &mut out);
    stream.output(frame(S, 2, &GRIN[2..]), &mut out);
    stream.backfill(S, 0, 0, b"before ".to_vec(), None, false, &mut out);

    let mut want = b"before ".to_vec();
    want.extend_from_slice(&GRIN);
    assert_eq!(painted(&out), want, "the four bytes must be consecutive");

    // And the pane is handed them as one run, not as two writes it would have
    // to rejoin itself.
    let mut sink = Recorder::default();
    apply(&out, &mut sink);
    assert_eq!(sink.bytes, want);
    assert_eq!(
        sink.log,
        vec!["reset".to_string(), format!("write {}", want.len())],
        "the splice must reach the parser as one write"
    );

    // Split three-and-one as well: a reassembler that only handles a one-byte
    // tail is a real bug shape and the four-byte character is what exposes it.
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.output(frame(S, 0, &GRIN[..3]), &mut out);
    stream.output(frame(S, 3, &GRIN[3..]), &mut out);
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);
    assert_eq!(painted(&out).as_slice(), &GRIN);
}

/// Bytes that are not valid UTF-8 at all must survive the whole client path.
///
/// WHY: the split-character case above still round-trips through valid UTF-8 if
/// something decodes and re-encodes it, so on its own it cannot see a lossy
/// hop. A lone `0xFF` can: a decode turns it into U+FFFD and there is no
/// re-encoding that gives the byte back. This is the mutation that escapes the
/// case above.
#[test]
fn invalid_utf8_survives_the_splice_and_the_sink() {
    let junk: Vec<u8> = vec![0xFF, 0x00, 0xFE, 0x1B, 0x80];
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.output(frame(S, 0, &junk), &mut out);
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);

    let mut sink = Recorder::default();
    apply(&out, &mut sink);
    assert_eq!(sink.bytes, junk);
}

// ---------------------------------------------------------------------------
// The rest of the state machine
// ---------------------------------------------------------------------------

/// A focus change must reset the screen before anything is painted into it.
///
/// WHY: the previous session may have left the screen in alternate-screen mode,
/// with a scroll region set or with SGR state pending, and any of those
/// corrupts the incoming repaint. A clear would not undo them; only a reset
/// does. The ordering is the claim as much as the reset is, which is why every
/// screen instruction travels on one channel.
#[test]
fn a_focus_change_resets_before_the_repaint_and_clears_the_pane_when_it_is_none() {
    let out = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(S, 0, 0, b"history".to_vec(), None, false, o);
    });
    let mut sink = Recorder::default();
    apply(&out, &mut sink);
    assert_eq!(sink.log, vec!["reset".to_string(), "write 7".to_string()]);
    assert_eq!(sink.bytes, b"history");

    let cleared = ops(|s, o| s.focus(None, o));
    assert_eq!(cleared, vec![PaneOp::Reset]);
    let mut stream = PaneStream::default();
    let mut discard = Vec::new();
    stream.focus(None, &mut discard);
    assert_eq!(stream.focused(), None);
}

/// A page-back must reset, repaint, and land back on the line being read.
///
/// WHY: paging back is a repaint of a bigger window ending at the same head,
/// because the terminal engine offers no way to splice older bytes above what
/// it has already been fed. That means the screen is cleared and rebuilt under
/// an operator who is looking at a specific line, and putting them back on it
/// is the difference between "more history appeared above" and "the pane jumped
/// to the bottom".
#[test]
fn a_page_back_resets_repaints_and_asks_to_keep_the_view() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.backfill(S, 100, 100, b"\ntail".to_vec(), None, false, &mut out);
    assert!(!stream.paging());

    out.clear();
    stream.arm_page_back();
    assert!(
        stream.paging(),
        "a second wheel tick must not send a second request"
    );
    stream.output(frame(S, 105, b"live"), &mut out);
    stream.backfill(S, 0, 105, b"deeper tail".to_vec(), None, true, &mut out);

    let mut sink = Recorder::default();
    apply(&out, &mut sink);
    assert_eq!(
        sink.log,
        vec![
            "reset".to_string(),
            "write 15".to_string(),
            "keep_view".to_string()
        ]
    );
    assert_eq!(sink.bytes, b"deeper taillive");
    assert!(!stream.paging(), "the repaint releases the guard");
}

/// A search jump must land on the hit's logical line, counted from the end.
///
/// WHY: the buffer is trimmed from the TOP once its limit is reached, so a line
/// index counted forwards stops matching as soon as more history is painted
/// than the buffer holds. The distance from the LAST line is stable under that
/// trim. The count is done here, over the bytes that were just painted, because
/// it is a `u64` subtraction and a newline count; the pane is left only with
/// turning a logical line into a viewport position.
#[test]
fn a_search_jump_scrolls_to_the_hit_counted_from_the_end() {
    // A window whose first byte is a line break, so nothing is trimmed off the
    // front. The hit is at absolute offset 13; two newlines follow it, so the
    // hit's line is two logical lines from the last.
    let out = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(S, 9, 10, b"\nab HIT\nxx\nyy".to_vec(), Some(13), false, o);
    });
    assert_eq!(out.last(), Some(&PaneOp::ScrollFromEnd(2)));

    // A hit the daemon could not actually cover: scrolling anywhere would be a
    // guess, so the viewport is left at the bottom.
    let short = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(S, 10, 10, b"ab".to_vec(), Some(9_000), false, o);
    });
    assert!(!short.iter().any(|op| matches!(op, PaneOp::ScrollFromEnd(_))));
    assert!(
        !short.iter().any(|op| matches!(op, PaneOp::ScrollKeep)),
        "an out-of-range jump is not a page-back and must not keep a view \
         nobody asked to keep"
    );
}

/// A history window that does not start at the beginning of the stream must be
/// replayed from a line boundary.
///
/// WHY: the daemon's ring is a byte range. A window trimmed to a byte budget
/// can begin in the middle of a CSI, and replaying from there feeds the parser
/// `1;32mDone` with no introducer, which the pane paints as literal text at the
/// top of every page-back. A line break is the one byte that cannot occur
/// inside an escape sequence or inside a UTF-8 multi-byte sequence, so the
/// first one is a position the parser state is known at.
///
/// The jump arithmetic has to follow the trim: a hit offset is absolute, and
/// counting it from the untrimmed start would put the viewport as many lines
/// out as the trimmed prefix held.
///
/// What this does NOT catch: a window whose first 64 KiB hold no line break,
/// where there is nothing to resynchronise to and the whole span is replayed.
#[test]
fn a_trimmed_history_window_is_replayed_from_the_first_line_boundary() {
    // Half an SGR sequence, then a real line.
    let out = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(S, 4_096, 4_096, b"1;32mDone\r\nnext line".to_vec(), None, false, o);
    });
    assert_eq!(
        painted(&out).as_slice(),
        b"\nnext line",
        "the truncated sequence must not reach the parser"
    );

    // A window that starts at the very beginning of the stream cannot be
    // truncated, so nothing is dropped from it.
    let whole = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(S, 0, 0, b"1;32mDone\r\nnext".to_vec(), None, false, o);
    });
    assert_eq!(painted(&whole).as_slice(), b"1;32mDone\r\nnext");

    // The hit is at absolute 4_106, which is `next line`. One line follows the
    // trim point, so the viewport lands zero lines from the end.
    let jump = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(
            S,
            4_096,
            4_096,
            b"1;32mDone\r\nnext line".to_vec(),
            Some(4_107),
            false,
            o,
        );
    });
    assert_eq!(
        jump.last(),
        Some(&PaneOp::ScrollFromEnd(0)),
        "the jump offset must be counted from the trimmed start, not the raw one"
    );

    // A hit that fell inside the truncated first line lands on the top of what
    // was replayed, never at the bottom. The line it sat on is the line the
    // trim took, and answering a search by showing the operator the newest
    // output instead is worse than answering it approximately.
    let inside = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(
            S,
            4_096,
            4_096,
            b"1;32mDone\r\nnext line".to_vec(),
            Some(4_098),
            false,
            o,
        );
    });
    assert_eq!(inside.last(), Some(&PaneOp::ScrollFromEnd(1)));
}

// ---------------------------------------------------------------------------
// Framing and failure
// ---------------------------------------------------------------------------

/// A malformed data frame must be refused with a reason, and must not take the
/// connection down with it.
///
/// WHY: the header is the only thing that says which session a payload belongs
/// to and where it sits in the stream. A frame short of the header, or of a
/// kind this client does not know, cannot be salvaged: painting its tail into
/// whatever pane happens to be focused is how one agent's output appears in
/// another's transcript. Dropping the connection instead would turn one bad
/// frame from a daemon one release ahead into a window that reconnects forever.
///
/// What this does NOT catch: a frame with a plausible but wrong header, which
/// nothing on either side can detect.
#[test]
fn a_malformed_data_frame_is_refused_with_a_reason_and_the_socket_survives() {
    let err = Frame::parse(Vec::new()).expect_err("an empty frame has no header");
    assert!(err.contains(&OUTPUT_HEADER_LEN.to_string()), "{err}");
    assert!(err.contains("truncated"), "{err}");

    let short = vec![FRAME_KIND_OUTPUT; OUTPUT_HEADER_LEN - 1];
    let err = Frame::parse(short).expect_err("one byte short of a header");
    assert!(err.contains(&(OUTPUT_HEADER_LEN - 1).to_string()), "{err}");

    let mut wrong_kind = vitrum_proto::encode_output(S, 0, b"hi");
    wrong_kind[0] = 99;
    let err = Frame::parse(wrong_kind).expect_err("kind 99 is not output");
    assert!(err.contains("99"), "{err}");

    let good = frame(S, 4096, b"hi");
    assert_eq!(good.session, S);
    assert_eq!(good.seq, 4096);
    assert_eq!(good.payload(), b"hi");

    // The connection continues. `accept` returns false only for a close.
    let said = std::cell::RefCell::new(Vec::new());
    let carry_on = accept(Message::Binary(vec![1, 2, 3]), &|ev| {
        said.borrow_mut().push(format!("{ev:?}"));
        true
    });
    assert!(carry_on, "one undecodable frame must not close the socket");
    assert!(said.borrow()[0].starts_with("Bad("), "{said:?}");

    // A close does end it, and says why.
    let ended = accept(Message::Close(None), &|_| true);
    assert!(!ended);
}

/// A close code must reach the operator as a sentence, and an unknown one must
/// stay identifiable.
///
/// WHY: `code 1006` on the sidebar banner tells an operator nothing. The
/// failure mode of fixing that is worse than the original: flattening every
/// code into "the connection closed" makes an unexpected failure
/// indistinguishable from a normal one, so an unrecognised code keeps its
/// number.
#[test]
fn close_codes_become_sentences_and_unknown_ones_keep_their_number() {
    assert_eq!(close_reason(1000, ""), "the daemon closed the connection");
    assert_eq!(close_reason(1006, ""), "the connection dropped");
    assert_eq!(close_reason(1012, ""), "the daemon is restarting");
    assert_eq!(
        close_reason(4999, ""),
        "the connection closed with code 4999"
    );
    // A reason the daemon supplied outranks this table, but the code stays
    // attached: the sentence is for the operator and the number is for a bug
    // report.
    assert_eq!(
        close_reason(1011, "session store is corrupt"),
        "session store is corrupt (code 1011)"
    );
}

/// A control frame this client cannot read must be reported with the action
/// that fixes it.
///
/// WHY: a daemon from another release sends a message shape this build has no
/// variant for. Swallowing it leaves a window that is connected and silently
/// missing half its updates; reporting "invalid JSON" leaves an operator with
/// nothing to do. The sentence has to name the restart.
#[test]
fn an_unreadable_control_frame_names_the_corrective_action() {
    let said = std::cell::RefCell::new(String::new());
    accept(Message::Text(r#"{"t":"fromTheFuture"}"#.to_string()), &|ev| {
        *said.borrow_mut() = format!("{ev:?}");
        true
    });
    let said = said.into_inner();
    assert!(said.starts_with("Bad("), "{said}");
    assert!(said.contains("vitrum-server"), "{said}");
}

// ---------------------------------------------------------------------------
// The pane, before and after it exists
// ---------------------------------------------------------------------------

/// Ops emitted before the surface exists must be replayed onto it, in order.
///
/// WHY: a window can be told to focus a session before its drawing area has
/// been realised, which is the ordinary case for a session restored at
/// startup. Dropping those ops loses the attach repaint and leaves a blank pane
/// with a live socket behind it, which reads as a hung daemon.
///
/// What this does NOT catch: a surface that never appears, which the bound
/// below covers.
#[test]
fn ops_emitted_before_the_surface_exists_are_replayed_onto_it() {
    let (mut net, _rx) = Net::new();
    net.drive(|s, o| s.focus(Some(S), o));
    net.drive(|s, o| s.backfill(S, 0, 0, b"history".to_vec(), None, false, o));

    let sink = Rc::new(RefCell::new(Recorder::default()));
    net.attach_pane(sink.clone());
    assert_eq!(
        sink.borrow().log,
        vec!["reset".to_string(), "write 7".to_string()]
    );
    assert_eq!(sink.borrow().bytes, b"history");

    // And once attached, ops go straight through rather than accumulating.
    net.drive(|s, o| s.output(frame(S, 7, b"live"), o));
    assert_eq!(sink.borrow().bytes, b"historylive");
}

/// A window whose surface never appears must not accumulate a session's whole
/// output.
///
/// WHY: the held vector is the one unbounded structure on the path. A pane that
/// is never realised, because the window was closed while a session was
/// attached, would otherwise hold every byte the agent writes until the process
/// exits. Past the bound the held ops are dropped for a single reset, which is
/// the honest outcome: the surface will repaint from a backfill when it arrives.
#[test]
fn a_surface_that_never_appears_does_not_grow_without_bound() {
    let (mut net, _rx) = Net::new();
    net.drive(|s, o| s.focus(Some(S), o));
    net.drive(|s, o| s.backfill(S, 0, 0, Vec::new(), None, false, o));

    let chunk = vec![b'z'; 64 * 1024];
    let mut seq = 0u64;
    for _ in 0..64 {
        net.drive(|s, o| s.output(frame(S, seq, &chunk), o));
        seq += chunk.len() as u64;
    }

    let sink = Rc::new(RefCell::new(Recorder::default()));
    net.attach_pane(sink.clone());
    let held = sink.borrow().bytes.len();
    assert!(
        held <= PENDING_CAP + 64 * 1024,
        "{held} bytes were held for a surface that does not exist"
    );
    assert_eq!(
        sink.borrow().log.first().map(String::as_str),
        Some("reset"),
        "what survives the bound must start with a reset, or the pane replays \
         a fragment of a stream as if it were the start of one"
    );
}

/// A socket with no runtime under it must say so rather than fail silently.
///
/// WHY: `Net` captures the tokio handle at construction, on the UI thread. A
/// window built outside a runtime would otherwise call `connect` and never
/// open anything, leaving "connecting" on screen forever with no reason and no
/// retry.
#[test]
fn a_connect_with_no_runtime_reports_it_instead_of_hanging() {
    let (mut net, mut rx) = Net::new();
    net.connect("ws://127.0.0.1:1/".to_string());
    let (epoch, ev) = rx.try_recv().expect("a refusal, not silence");
    assert_eq!(epoch, net.epoch());
    let SocketEvent::Error(detail) = ev else {
        panic!("expected an error");
    };
    assert!(detail.contains("restart vitrum"), "{detail}");
}
