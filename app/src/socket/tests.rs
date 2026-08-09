//! What the client half of the data plane owes the pane.
//!
//! These are the guarantees that used to be enforced in `bootstrap.js` and
//! could only be checked from the far side of a socket. The server suites in
//! `crates/vitrum-server/src/tests/seam_*.rs` prove the daemon emits a
//! contiguous, exactly-once byte stream; nothing there can prove what THIS
//! process does with it, and until the socket moved into Rust nothing could.
//!
//! Everything below drives [`PaneStream`] directly. It is pure by
//! construction — frames and commands in, [`PaneOp`]s and notices out — so a
//! case here is the real shipped path with no socket, no webview and no
//! timing.

use super::*;

const S: SessionId = SessionId(7);
const OTHER: SessionId = SessionId(8);

/// Four bytes of UTF-8: U+1F600, the character every split test uses.
///
/// Chosen over a two-byte one because four bytes can be split three ways, and
/// a reassembler that only handles a one-byte tail is a real bug shape.
const GRIN: [u8; 4] = [0xF0, 0x9F, 0x98, 0x80];

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
/// grid, so this is exactly the byte sequence the emulator will parse.
fn painted(ops: &[PaneOp]) -> Vec<u8> {
    let mut all = Vec::new();
    for op in ops {
        if let PaneOp::Write(bytes) = op {
            all.extend_from_slice(bytes);
        }
    }
    all
}

/// Decode the wire form `encode_ops` produces, so a test can assert on the
/// bytes that actually cross to the webview rather than on the enum.
fn decode_ops(mut buf: &[u8]) -> Vec<PaneOp> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        assert!(
            buf.len() >= OP_HEADER_LEN,
            "a trailing {} byte fragment: the stream is not self-describing",
            buf.len()
        );
        let tag = buf[0];
        let len = u32::from_le_bytes(buf[1..5].try_into().expect("four bytes")) as usize;
        let body = &buf[OP_HEADER_LEN..OP_HEADER_LEN + len];
        out.push(match tag {
            OP_WRITE => PaneOp::Write(body.to_vec()),
            OP_RESET => PaneOp::Reset,
            OP_SCROLL_FROM_END => {
                PaneOp::ScrollFromEnd(u32::from_le_bytes(body.try_into().expect("four bytes")))
            }
            OP_SCROLL_KEEP => PaneOp::ScrollKeep,
            other => panic!("unknown pane-op tag {other}"),
        });
        buf = &buf[OP_HEADER_LEN + len..];
    }
    out
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
/// likely to be inside an escape sequence as inside a word — so a gap has to be
/// said out loud. The opposite error is worse in practice: a re-attach
/// legitimately resumes at wherever the child has reached, and a client that
/// treated that as a gap would cry corruption on every tab switch. The server
/// end of this pair is
/// `seam_stream::attach_detach_and_reattach_carry_exact_bytes_and_no_backlog`,
/// which proves the daemon sends no backlog on re-attach and no gap within one;
/// this proves the client draws the right conclusion from both.
///
/// What this does NOT catch: reordering, which a WebSocket cannot do, and a
/// gap the daemon itself introduced, which is the server suite's claim.
#[test]
fn a_gap_within_one_attachment_is_reported_and_a_reconnect_resume_is_not() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();

    stream.focus(Some(S), &mut out);
    // The backfill for the focus, so the live path is not buffering.
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);

    stream.output(S, 0, b"abc".to_vec(), &mut out);
    stream.output(S, 3, b"def".to_vec(), &mut out);
    assert!(
        stream.take_notices().is_empty(),
        "contiguous frames must be silent"
    );

    // One byte missing between 6 and 7.
    stream.output(S, 7, b"ghi".to_vec(), &mut out);
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
    stream.output(S, 90_000, b"resumed".to_vec(), &mut out);
    assert!(
        stream.take_notices().is_empty(),
        "a re-attach resumes wherever the child reached; that is not a gap"
    );

    // And contiguity is re-armed from the new offset, not from the old one.
    stream.output(S, 90_010, b"x".to_vec(), &mut out);
    assert_eq!(
        stream.take_notices().len(),
        1,
        "the second attachment must be checked against its own first frame"
    );
}

/// Frames for a session the pane is not showing must not reach the grid.
///
/// WHY: a window is attached to whatever the daemon says it is attached to, and
/// with twenty agents running most frames on the socket are for some other
/// pane. Painting one is another session's output appearing mid-transcript,
/// and — worse — it would poison the contiguity check for the session that IS
/// focused.
///
/// What this does NOT catch: the daemon sending frames for a session this
/// client never attached to, which `attach_stream.rs` owns.
#[test]
fn a_frame_for_another_session_is_dropped_without_disturbing_the_focused_one() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);
    out.clear();

    stream.output(S, 0, b"mine".to_vec(), &mut out);
    stream.output(OTHER, 999, b"theirs".to_vec(), &mut out);
    stream.output(S, 4, b"also mine".to_vec(), &mut out);

    assert_eq!(painted(&out).as_slice(), b"minealso mine");
    assert!(
        stream.take_notices().is_empty(),
        "the foreign frame must not have moved the focused session's offset"
    );
}

// ---------------------------------------------------------------------------
// Backlog, exactly once
// ---------------------------------------------------------------------------

/// History and the live bytes buffered behind it must meet exactly, with no
/// byte repeated and none lost.
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
fn buffered_live_bytes_are_spliced_onto_history_exactly_once() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);

    // Live frames arriving while the scrollback request is in flight. The
    // first straddles the resume offset, which is the case the splice exists
    // for: three of its bytes are already inside the history.
    stream.output(S, 7, b"HIJK".to_vec(), &mut out);
    stream.output(S, 11, b"L".to_vec(), &mut out);
    assert!(
        painted(&out).is_empty(),
        "nothing may reach the grid while a backfill is in flight"
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

/// A backfill that starts above the buffered live bytes must report the hole.
///
/// WHY: this is the reverse overlap, and it is real: after a reported gap the
/// bytes between the history and the first live frame may have been evicted
/// from the daemon's ring, so `resume_seq` lands BELOW the oldest frame the
/// client holds. Painting the frames anyway is correct — the grid was reset
/// first, so the alternative is a splice at a wrong offset — but the hole is
/// history the operator will never see and silently swallowing it is how a
/// transcript acquires an invisible edit.
///
/// What this does NOT catch: eviction the daemon reports through a raised
/// `from_seq`, which `scrollback_rpc.rs` owns.
#[test]
fn history_evicted_between_the_backfill_and_the_live_bytes_is_named() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.output(S, 100, b"live".to_vec(), &mut out);
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

/// Overflowing the pending buffer must paint the live bytes and discard the
/// backfill that lands afterwards.
///
/// WHY: a stalled repaint may not turn into unbounded client memory just
/// because an agent is chatty, and the live bytes are what the operator
/// actually needs. The half of that decision the tests keep missing is the
/// SECOND half: the abandoned backfill still arrives, and painting it after
/// the live bytes that already reached the grid would rewind the transcript.
///
/// What this does NOT catch: the cap's value, which is a memory budget rather
/// than a correctness claim.
#[test]
fn overflowing_the_pending_buffer_flushes_live_and_discards_the_late_backfill() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);

    let chunk = vec![b'x'; PENDING_CAP / 2 + 1];
    stream.output(S, 0, chunk.clone(), &mut out);
    assert!(painted(&out).is_empty(), "one chunk is still under the cap");
    stream.output(S, chunk.len() as u64, chunk.clone(), &mut out);

    assert_eq!(
        painted(&out).len(),
        chunk.len() * 2,
        "past the cap every buffered byte is painted at once"
    );
    let notices = stream.take_notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("overflowed"), "{}", notices[0]);

    out.clear();
    stream.backfill(S, 0, 0, b"history".to_vec(), None, false, &mut out);
    assert!(
        out.is_empty(),
        "the abandoned backfill must not rewind the grid, and must not reset it"
    );

    // And the pane is live again rather than stuck buffering.
    stream.output(S, chunk.len() as u64 * 2, b"after".to_vec(), &mut out);
    assert_eq!(painted(&out).as_slice(), b"after");
}

// ---------------------------------------------------------------------------
// The multi-byte guarantee
// ---------------------------------------------------------------------------

/// A character split across two frames must be contiguous in what is painted.
///
/// WHY: this is the guarantee the whole data plane is shaped around, and it is
/// the one that breaks the instant anything on the path types a payload as
/// text. A `String` anywhere between the socket and the grid turns a split
/// U+1F600 into two replacement characters, which is three bytes where there
/// was two and shifts every offset after it: search hits point at the wrong
/// column and scrollback pages stop abutting.
/// `seam_stream::invalid_utf8_crosses_the_socket_verbatim` proves the bytes
/// reach this process intact. This proves they leave it intact, through the
/// splice — the one place in the client that concatenates frames — and through
/// the encoder that carries them to the webview.
///
/// What this does NOT catch: what the terminal engine does with the bytes,
/// which is the emulator's question; and a split introduced by the webview's
/// own read of the pane route, which cannot happen because the route delivers
/// one response body per push.
#[test]
fn a_character_split_across_two_frames_is_whole_before_it_reaches_the_view() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);

    // Split two-and-two while a backfill is in flight, so both halves are
    // buffered and the splice is what has to put them back together.
    stream.output(S, 0, GRIN[..2].to_vec(), &mut out);
    stream.output(S, 2, GRIN[2..].to_vec(), &mut out);
    stream.backfill(S, 0, 0, b"before ".to_vec(), None, false, &mut out);

    let mut want = b"before ".to_vec();
    want.extend_from_slice(&GRIN);
    assert_eq!(painted(&out), want, "the four bytes must be consecutive");

    // And the encoder that carries it to the webview must not touch them
    // either. One `Write`, one payload, byte for byte.
    let mut wire = Vec::new();
    encode_ops(&out, &mut wire);
    assert_eq!(painted(&decode_ops(&wire)), want);
}

/// Bytes that are not valid UTF-8 at all must survive the whole client path.
///
/// WHY: the split-character case above still round-trips through valid UTF-8
/// if something decodes and re-encodes it, so on its own it cannot see a lossy
/// hop. A lone `0xFF` can: a decode turns it into U+FFFD and there is no
/// re-encoding that gives the byte back. This is the mutation that escapes the
/// case above.
///
/// What this does NOT catch: a lossy hop inside the webview, which is why
/// `PaneOp::Write` reaches it as an `ArrayBuffer` and never as a JSON string.
#[test]
fn invalid_utf8_survives_the_splice_and_the_encoder() {
    let junk: Vec<u8> = vec![0xFF, 0x00, 0xFE, 0x1B, 0x80];
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.output(S, 0, junk.clone(), &mut out);
    stream.backfill(S, 0, 0, Vec::new(), None, false, &mut out);

    let mut wire = Vec::new();
    encode_ops(&out, &mut wire);
    assert_eq!(painted(&decode_ops(&wire)), junk);
}

// ---------------------------------------------------------------------------
// The rest of the state machine
// ---------------------------------------------------------------------------

/// A focus change must reset the grid before anything is painted into it.
///
/// WHY: the previous session may have left the grid in alternate-screen mode,
/// with a scroll region set or with SGR state pending, and any of those
/// corrupts the incoming repaint. A clear would not undo them; only a reset
/// does. The ordering is the claim as much as the reset is, which is why every
/// grid instruction travels on one channel.
///
/// What this does NOT catch: what xterm's `reset` actually restores.
#[test]
fn a_focus_change_resets_before_the_repaint_and_clears_the_pane_when_it_is_none() {
    let out = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(S, 0, 0, b"history".to_vec(), None, false, o);
    });
    assert_eq!(
        out,
        vec![PaneOp::Reset, PaneOp::Write(b"history".to_vec())],
        "reset first, once"
    );

    let cleared = ops(|s, o| s.focus(None, o));
    assert_eq!(cleared, vec![PaneOp::Reset]);
    let mut stream = PaneStream::default();
    let mut sink = Vec::new();
    stream.focus(None, &mut sink);
    assert_eq!(stream.focused(), None);
}

/// A page-back must reset, repaint, and land back on the line being read.
///
/// WHY: paging back is a repaint of a bigger window ending at the same head,
/// because xterm.js has no prepend. That means the grid is cleared and rebuilt
/// under an operator who is looking at a specific line, and putting them back
/// on it is the difference between "more history appeared above" and "the pane
/// jumped to the bottom".
///
/// What this does NOT catch: the row arithmetic, which needs the wrapped-line
/// layout only the emulator has and is why `ScrollKeep` carries no number.
#[test]
fn a_page_back_resets_repaints_and_asks_to_keep_the_view() {
    let mut stream = PaneStream::default();
    let mut out = Vec::new();
    stream.focus(Some(S), &mut out);
    stream.backfill(S, 100, 100, b"tail".to_vec(), None, false, &mut out);
    assert!(!stream.paging());

    out.clear();
    stream.arm_page_back();
    assert!(stream.paging(), "a second wheel tick must not send a second request");
    stream.output(S, 104, b"live".to_vec(), &mut out);
    stream.backfill(S, 0, 104, b"deeper tail".to_vec(), None, true, &mut out);

    assert_eq!(
        out,
        vec![
            PaneOp::Reset,
            PaneOp::Write(b"deeper taillive".to_vec()),
            PaneOp::ScrollKeep,
        ]
    );
    assert!(!stream.paging(), "the repaint releases the guard");
}

/// A search jump must land on the hit's logical line, counted from the end.
///
/// WHY: xterm trims its buffer from the TOP once the scrollback limit is
/// reached, so a line index counted forwards stops matching the buffer as soon
/// as more history is painted than it holds. The distance from the LAST line is
/// stable under that trim. The count is done here, over the bytes that were
/// just painted, because it is a `u64` subtraction and a newline count; the
/// webview is left only with turning a logical line into a wrapped row.
///
/// What this does NOT catch: whether the daemon's hit offset is right, which is
/// `search_rpc.rs`.
#[test]
fn a_search_jump_scrolls_to_the_hit_counted_from_the_end() {
    // Hit at absolute offset 12, in a window starting at 10. Two newlines
    // follow it, so the hit's line is two logical lines from the last.
    let out = ops(|s, o| {
        s.focus(Some(S), o);
        s.backfill(S, 10, 10, b"ab HIT\nxx\nyy".to_vec(), Some(13), false, o);
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

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// A malformed data frame must be reported, never guessed at.
///
/// WHY: the header is the only thing that says which session a payload belongs
/// to and where it sits in the stream. A frame short of the header, or of a
/// kind this client does not know, cannot be salvaged — painting its tail into
/// whatever pane happens to be focused is how one agent's output appears in
/// another's transcript.
///
/// What this does NOT catch: a frame with a plausible but wrong header, which
/// nothing on either side can detect.
#[test]
fn a_malformed_data_frame_is_refused_with_a_reason() {
    assert!(split_output(&[]).is_err(), "an empty frame has no header");
    let short = vec![FRAME_KIND_OUTPUT; OUTPUT_HEADER_LEN - 1];
    let err = split_output(&short).expect_err("one byte short of a header");
    assert!(err.contains(&OUTPUT_HEADER_LEN.to_string()), "{err}");

    let mut wrong_kind = vitrum_proto::encode_output(S, 0, b"hi");
    wrong_kind[0] = 99;
    let err = split_output(&wrong_kind).expect_err("kind 99 is not output");
    assert!(err.contains("99"), "{err}");

    let good = vitrum_proto::encode_output(S, 4096, b"hi");
    assert_eq!(
        split_output(&good).expect("a well-formed frame"),
        (S, 4096, OUTPUT_HEADER_LEN)
    );
    assert_eq!(&good[OUTPUT_HEADER_LEN..], b"hi");
}

/// A close code must reach the operator as a sentence, and an unknown one must
/// stay identifiable.
///
/// WHY: `code 1006` on the sidebar banner tells an operator nothing. The
/// failure mode of fixing that is worse than the original: flattening every
/// code into "the connection closed" makes an unexpected failure
/// indistinguishable from a normal one, so an unrecognised code keeps its
/// number.
///
/// What this does NOT catch: whether the daemon sends the code it should.
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

/// The pane-op wire form must survive an empty payload and a payload
/// containing every byte value.
///
/// WHY: the framing is length-prefixed rather than self-delimiting precisely
/// because a `Write` payload is arbitrary bytes and no sentinel a terminal
/// stream cannot contain exists. A zero-length op is the boundary case that
/// breaks a reader that treats "no bytes" as "end of stream", and a payload
/// spanning 0..=255 is the one that breaks a reader that scans for a
/// terminator.
///
/// What this does NOT catch: a payload above `u32::MAX`, which cannot occur —
/// the largest single op is a backfill and `wire::PAGE_CEILING_BYTES` is 8 MiB.
#[test]
fn the_pane_op_framing_round_trips_every_byte_value() {
    let all: Vec<u8> = (0..=255u8).collect();
    let ops = vec![
        PaneOp::Reset,
        PaneOp::Write(Vec::new()),
        PaneOp::Write(all.clone()),
        PaneOp::ScrollFromEnd(0),
        PaneOp::ScrollFromEnd(u32::MAX),
        PaneOp::ScrollKeep,
        PaneOp::Write(all),
    ];
    let mut wire = Vec::new();
    encode_ops(&ops, &mut wire);
    assert_eq!(decode_ops(&wire), ops);
}
