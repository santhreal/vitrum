//! The session socket, and the data plane behind it.
//!
//! # The path one byte takes
//!
//! A byte written by an agent reaches the screen through exactly these steps,
//! and no others:
//!
//! 1. The daemon reads it from a PTY and puts it in a binary WebSocket frame
//!    behind a 17-byte header: kind, session, byte offset.
//! 2. [`run`] receives that frame on a tokio worker. tungstenite hands over a
//!    `Vec<u8>` it already owns; nothing here copies out of it.
//! 3. [`Frame::parse`] validates the header and records where the payload
//!    starts. The frame is moved, not resliced, into a [`SocketEvent::Output`].
//! 4. The UI thread takes it off an unbounded channel and gives it to
//!    [`PaneStream::output`], which checks the offset is contiguous and emits
//!    a [`PaneOp::Write`] carrying the same allocation.
//! 5. [`Net::drive`] hands the payload to [`PaneSink::write`] as a borrowed
//!    slice, which is [`vitrum_vt::Vt::feed`].
//!
//! There is no JSON, no base64, no `String`, no UTF-8 validation and no
//! re-encoding anywhere on that path, and there is exactly one copy on it: the
//! one tungstenite makes when it reads the socket into a buffer. A payload is
//! never revalidated, because a terminal must forward bytes it cannot
//! interpret, and because validating a stream that is only being forwarded is
//! work the operator waits for and never sees.
//!
//! The one channel hop is the thread boundary, and it is load-bearing: socket
//! I/O on the UI thread would put a blocking read between two frames.
//!
//! # What the client owes the pane
//!
//! [`PaneStream`] is the state machine, on the UI thread, pure and testable:
//! frames and commands in, [`PaneOp`]s and notices out. It owns the guarantees
//! the pane cannot check for itself.
//!
//! 1. **Framing.** A frame shorter than the header, or of an unknown kind, is
//!    refused with a reason. Nothing is painted from a header that did not
//!    parse.
//! 2. **Filtering.** A frame for a session the pane is not showing is dropped
//!    before it can move that session's offset.
//! 3. **Sequence continuity.** The offset on a frame is the session's
//!    cumulative byte count. A jump within one attachment is output that was
//!    lost between the daemon's ring and this process, and is said out loud,
//!    because the missing byte is as likely to be inside an escape sequence as
//!    inside a word. A re-attach resumes wherever the child has reached, which
//!    is not a gap and is not reported.
//! 4. **The backlog splice.** Between a focus change and its backfill the live
//!    frames are held, up to [`PENDING_CAP`]. When history lands, the held
//!    frames are spliced onto it BY BYTE OFFSET: the overlapping prefix of the
//!    straddling frame is dropped exactly once, and a joint the daemon's ring
//!    evicted is reported rather than papered over. Past the cap ordering is
//!    abandoned, the live bytes are painted, and the late backfill is
//!    discarded rather than allowed to rewind the transcript.
//! 5. **Reassembly.** Concatenation happens in arrival order and nothing on
//!    the path types a payload as text, so a character split across two frames
//!    is whole again before the parser sees it.
//! 6. **Ordering.** Everything that touches the screen travels as a
//!    [`PaneOp`], in one sequence. Two channels into one pane is two
//!    orderings, and a reset that overtook the write it was meant to precede
//!    would clear the repaint it was clearing for.

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;
use vitrum_proto::{FRAME_KIND_OUTPUT, OUTPUT_HEADER_LEN, ServerMsg, SessionId, decode_output};

#[cfg(test)]
mod tests;

/// Cap on live bytes held while waiting for a backfill to land.
///
/// Past this the backfill is abandoned and the buffer is flushed: a stalled
/// repaint must not turn into unbounded client memory just because an agent is
/// chatty. [`crate::wire::BACKFILL_CEILING_BYTES`] is sized against it.
pub(crate) const PENDING_CAP: usize = 1 << 20;

/// How far into a trimmed history window to look for a line boundary.
///
/// See [`resync_offset`]. 64 KiB is a thousand lines at the byte estimate in
/// [`crate::wire::BACKFILL_BYTES_PER_LINE`]; a window whose first 64 KiB hold
/// no line break at all is one long line, where there is nothing to
/// resynchronise to and the scan should stop rather than walk 8 MiB.
const RESYNC_SCAN: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// Bytes on the way to the screen
// ---------------------------------------------------------------------------

/// A run of PTY bytes, and where inside its own allocation they start.
///
/// The offset exists so a live frame can be moved from the socket to the
/// terminal engine without the payload ever being copied out from behind its
/// header. A spliced or trimmed run starts at a different index of a buffer
/// this module built; either way the pane is handed one slice and the
/// allocation is freed once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Chunk {
    buf: Vec<u8>,
    at: usize,
}

impl Chunk {
    /// Take ownership of `buf`, painting all of it.
    pub(crate) fn owned(buf: Vec<u8>) -> Self {
        Self { buf, at: 0 }
    }

    /// Take ownership of `buf`, painting it from `at` onward.
    ///
    /// `at` is clamped, because the callers derive it from a header length and
    /// a scan, and a panic on the data path would take the window down over a
    /// frame.
    fn from(buf: Vec<u8>, at: usize) -> Self {
        let at = at.min(buf.len());
        Self { buf, at }
    }

    /// The bytes to paint.
    pub(crate) fn bytes(&self) -> &[u8] {
        // The constructors clamp `at`, so this cannot be out of range.
        &self.buf[self.at..]
    }

    /// How many bytes will be painted.
    pub(crate) fn len(&self) -> usize {
        self.buf.len() - self.at
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One decoded data-plane frame: a payload, and where it sits in the stream.
///
/// Constructed only by [`Frame::parse`], so a `Frame` in hand is a header that
/// was validated. The whole WebSocket message is kept rather than the payload,
/// which is what makes the hop from the socket to the terminal engine free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    chunk: Chunk,
    /// Which session's stream this belongs to.
    pub(crate) session: SessionId,
    /// Absolute byte offset of the first payload byte within that stream.
    pub(crate) seq: u64,
}

impl Frame {
    /// Validate one binary WebSocket message and take ownership of it.
    ///
    /// Total: every rejected shape returns a sentence naming what was wrong,
    /// because the bytes arrive from a socket and a header that did not parse
    /// says nothing about which pane its tail belongs to.
    pub(crate) fn parse(buf: Vec<u8>) -> Result<Self, String> {
        let (session, seq) = match decode_output(&buf) {
            Ok((session, seq, _)) => (session, seq),
            Err(e) => {
                // Named separately because the two are different operator
                // problems: a short frame is a truncated read or a foreign
                // sender, an unknown kind is a daemon from another release.
                return Err(match e {
                    vitrum_proto::FrameError::TooShort { len } => format!(
                        "data frame is {len} bytes, need at least {OUTPUT_HEADER_LEN}; \
                         the daemon sent a truncated frame"
                    ),
                    vitrum_proto::FrameError::UnknownKind(kind) => format!(
                        "unknown data frame kind {kind}; this client understands \
                         only kind {FRAME_KIND_OUTPUT}"
                    ),
                });
            }
        };
        Ok(Self {
            chunk: Chunk::from(buf, OUTPUT_HEADER_LEN),
            session,
            seq,
        })
    }

    /// The PTY bytes, borrowed out of the message they arrived in.
    pub(crate) fn payload(&self) -> &[u8] {
        self.chunk.bytes()
    }

    /// Offset one past this frame's last byte.
    fn end(&self) -> u64 {
        self.seq.saturating_add(self.chunk.len() as u64)
    }
}

// ---------------------------------------------------------------------------
// Pane operations
// ---------------------------------------------------------------------------

/// One instruction for the terminal pane, in the order it must be applied.
///
/// Everything that touches the screen travels this way, including the resets.
/// Two channels into one pane is two orderings, and a reset that overtook the
/// write it was meant to precede would clear the repaint it was clearing FOR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PaneOp {
    /// Feed these bytes to the terminal engine, verbatim.
    Write(Chunk),
    /// Full reset. Not a clear: the previous session may have left the screen
    /// in alternate-screen mode, with a scroll region set or with SGR state
    /// pending, and any of those would corrupt the incoming repaint.
    Reset,
    /// Put the viewport on the logical line `n` lines from the end of the
    /// buffer. Used for a search jump.
    ScrollFromEnd(u32),
    /// Put the viewport back on the line the operator was reading when they
    /// asked for more history.
    ScrollKeep,
}

/// What the terminal pane offers this module.
///
/// Four calls, all of them things only the pane can do, and none of them
/// carrying a session id or a byte offset: which session is attached and where
/// its stream has reached are this module's to know. [`Self::write`] takes a
/// borrowed slice because the bytes are already in the right place; copying
/// them to hand them over would be the copy this whole path exists to avoid.
pub(crate) trait PaneSink {
    /// Reset the screen: modes, scroll region, rendition, alternate screen.
    fn reset(&mut self);
    /// Feed PTY bytes to the parser.
    fn write(&mut self, bytes: &[u8]);
    /// Put the viewport `lines` logical lines above the end of the buffer.
    fn scroll_from_end(&mut self, lines: u32);
    /// Put the viewport back where it was before the last reset.
    fn keep_view(&mut self);
    /// One batch of the above is over.
    ///
    /// Sixty-four frames delivered in one wakeup are sixty-four `feed` calls
    /// and one present. Without a batch end the pane would have to guess where
    /// a burst stopped, and guessing means either a frame per payload or a
    /// timer, and this program has no timers.
    fn flush(&mut self);
}

/// Apply `ops` to `sink`, in order, then end the batch.
fn apply(ops: &[PaneOp], sink: &mut dyn PaneSink) {
    for op in ops {
        match op {
            PaneOp::Write(chunk) => sink.write(chunk.bytes()),
            PaneOp::Reset => sink.reset(),
            PaneOp::ScrollFromEnd(lines) => sink.scroll_from_end(*lines),
            PaneOp::ScrollKeep => sink.keep_view(),
        }
    }
    sink.flush();
}

// ---------------------------------------------------------------------------
// The data-plane state machine
// ---------------------------------------------------------------------------

/// Where a trimmed history window can safely be replayed from.
///
/// The daemon's ring is a byte range, so a window that does not start at the
/// beginning of the stream can start in the middle of anything: a UTF-8
/// sequence, a CSI, an OSC string. Replaying from there feeds the parser the
/// tail of a sequence it never saw the head of, and the pane paints the
/// remainder as literal text.
///
/// A line break is the one byte that resynchronises this exactly. It cannot
/// occur inside a UTF-8 multi-byte sequence, whose continuation bytes are all
/// above 0x7F, and it cannot occur inside an escape sequence, whose parameter
/// and intermediate bytes are all above 0x1F. So the first CR or LF in the
/// window is a position the parser's state is known at, and everything before
/// it is at most the tail of one truncated line.
///
/// Bounded by [`RESYNC_SCAN`] and skipped entirely for a window that starts at
/// the beginning of the stream, where there is nothing to resynchronise from.
#[must_use]
fn resync_offset(history: &[u8], from_seq: u64) -> usize {
    if from_seq == 0 {
        return 0;
    }
    let scan = history.len().min(RESYNC_SCAN);
    history[..scan]
        .iter()
        .position(|b| *b == b'\n' || *b == b'\r')
        .map_or(0, |at| at + 1)
}

/// Everything the pane knows about the bytes it is painting.
///
/// Pure: it takes frames and commands and returns [`PaneOp`]s and notices, and
/// touches neither the socket nor the pane. That is what lets the ordering
/// guarantees be tested without either.
#[derive(Debug, Default)]
pub(crate) struct PaneStream {
    /// Session whose output is being painted, or `None`. Frames for anything
    /// else are dropped: only the focused pane renders.
    focus: Option<SessionId>,
    /// Byte offset the next frame for [`Self::focus`] must carry.
    ///
    /// `None` until the first frame of an attachment, and cleared by every
    /// focus change, because contiguity is a property of ONE attachment. A
    /// re-attach legitimately resumes at wherever the child has reached, which
    /// is the guarantee
    /// `seam_stream::attach_detach_and_reattach_carry_exact_bytes_and_no_backlog`
    /// pins from the server's end.
    next_seq: Option<u64>,
    /// True between a focus change or a page-back request and its backfill.
    backfilling: bool,
    /// Set when [`PENDING_CAP`] was hit, so the late backfill is discarded
    /// instead of painted after the live bytes that already went to the pane.
    drop_backfill: bool,
    /// True between a page-back request and its repaint, so holding the wheel
    /// at the top sends one request rather than one per tick.
    paging: bool,
    pending: Vec<Frame>,
    pending_bytes: usize,
    /// Things the operator has to be told, drained by the caller.
    notices: Vec<String>,
}

impl PaneStream {
    /// The session the pane is pointed at.
    ///
    /// Test-only: production reads the focus through the state signal that set
    /// it, and a second reader of the same fact in the shipped build is how
    /// the two drift.
    #[cfg(test)]
    pub(crate) fn focused(&self) -> Option<SessionId> {
        self.focus
    }

    /// Whether a page-back request is outstanding. Test-only, as above.
    #[cfg(test)]
    pub(crate) fn paging(&self) -> bool {
        self.paging
    }

    /// Notices raised since the last drain.
    pub(crate) fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// Point the pane at `session`, or clear it.
    ///
    /// Resets the screen and starts holding live frames, so history and live
    /// output cannot interleave while the backfill is in flight.
    pub(crate) fn focus(&mut self, session: Option<SessionId>, ops: &mut Vec<PaneOp>) {
        self.focus = session;
        self.next_seq = None;
        self.pending.clear();
        self.pending_bytes = 0;
        self.drop_backfill = false;
        self.paging = false;
        self.backfilling = session.is_some();
        ops.push(PaneOp::Reset);
    }

    /// Claim the pane for a page-back, if one is not already in flight.
    ///
    /// Called only once the `Scrollback` request has actually gone out, so a
    /// request the client declined to send cannot leave the pane holding live
    /// output forever.
    pub(crate) fn arm_page_back(&mut self) {
        self.paging = true;
        self.backfilling = true;
    }

    /// One decoded data frame.
    pub(crate) fn output(&mut self, frame: Frame, ops: &mut Vec<PaneOp>) {
        if self.focus != Some(frame.session) || frame.chunk.is_empty() {
            return;
        }
        // Contiguity, checked once per frame against the offset the previous
        // frame ended at. The server's seq is the cumulative byte count of the
        // session's stream, so a mismatch is output that was lost or repeated
        // between the ring and this process, and painting across it corrupts
        // the parse from there on.
        if let Some(want) = self.next_seq
            && want != frame.seq
        {
            let seq = frame.seq;
            self.notices.push(format!(
                "the session stream jumped from byte {want} to byte {seq}; \
                 what is painted from here may be wrong"
            ));
        }
        self.next_seq = Some(frame.end());

        if self.backfilling {
            self.pending_bytes += frame.chunk.len();
            self.pending.push(frame);
            if self.pending_bytes > PENDING_CAP {
                // Give up on ordering the repaint rather than grow without
                // bound. The live bytes are what the operator actually needs.
                let mut all = Vec::with_capacity(self.pending_bytes);
                for held in self.pending.drain(..) {
                    all.extend_from_slice(held.payload());
                }
                self.pending_bytes = 0;
                self.backfilling = false;
                self.paging = false;
                self.drop_backfill = true;
                ops.push(PaneOp::Write(Chunk::owned(all)));
                self.notices.push(
                    "backfill buffer overflowed; painted live output without history".to_string(),
                );
            }
            return;
        }
        ops.push(PaneOp::Write(frame.chunk));
    }

    /// History for `session`, followed by whatever was held behind it.
    ///
    /// `history` starts at `from_seq`. `resume_seq` is the offset the live
    /// stream is owed from; the two windows overlap by exactly the bytes the
    /// child emitted between the request and the answer, and the offset is the
    /// only thing that says how many.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn backfill(
        &mut self,
        session: SessionId,
        from_seq: u64,
        resume_seq: u64,
        history: Vec<u8>,
        jump_seq: Option<u64>,
        keep_view: bool,
        ops: &mut Vec<PaneOp>,
    ) {
        if self.focus != Some(session) {
            return;
        }
        self.paging = false;
        if self.drop_backfill {
            self.drop_backfill = false;
            return;
        }
        // A page-back is a REPAINT of a bigger window ending at the same head,
        // so the screen has to be cleared first. On an attach the focus change
        // already reset it and this would be a second clear of an empty grid.
        if keep_view {
            ops.push(PaneOp::Reset);
        }

        // Replayed as one write. The history and the live frames that overlap
        // it are consecutive bytes of one stream with nothing between them, so
        // there is no reason for the parser to see a boundary the daemon never
        // put there, and a split inside a multi-byte character would be a
        // split this module introduced.
        let start = resync_offset(&history, from_seq);
        let painted_from = from_seq.saturating_add(start as u64);
        let mut painted = history;
        let mut hole = 0u64;
        // Where the painted stream currently ends. It starts at the offset the
        // history was computed to and advances with each frame, because the
        // question a frame answers is whether it abuts what is already
        // painted, not whether it abuts the resume offset. Measuring every
        // frame against `resume_seq` reports a hole for the second frame of
        // any healthy pair, since only the first one starts there.
        let mut painted_to = resume_seq;
        for held in self.pending.drain(..) {
            let end = held.end();
            if end <= painted_to {
                continue;
            }
            // The reverse of an overlap: after a reported gap the bytes
            // between the backfill and the first live frame may have been
            // evicted from the server's ring, so the painted stream ends BELOW
            // the oldest held frame. The screen was reset before this ran, so
            // painting the frames anyway is correct rather than a splice at
            // the wrong offset, but the hole is real history the operator will
            // never see and it gets said out loud.
            if held.seq > painted_to && hole == 0 {
                hole = held.seq - painted_to;
            }
            let skip = painted_to.saturating_sub(held.seq) as usize;
            let payload = held.payload();
            painted.extend_from_slice(&payload[skip.min(payload.len())..]);
            painted_to = painted_to.max(end);
        }
        self.pending_bytes = 0;
        self.backfilling = false;
        if hole > 0 {
            self.notices.push(format!(
                "{hole} bytes of history were evicted before they could be painted"
            ));
        }

        // Where to land, computed here because the offset arithmetic is a u64
        // subtraction and the newline count is over the bytes that were just
        // painted. The pane is left with only the part it alone can do:
        // turning a logical line index into a viewport position, which depends
        // on how the grid wrapped it.
        let body = &painted[start.min(painted.len())..];
        let scroll = match jump_seq {
            Some(jump) if jump >= from_seq => {
                // Clamped, not skipped. A hit inside the partial first line
                // that `resync_offset` removed lands on the top of what was
                // actually replayed: the line it sat on is the line the trim
                // took, and leaving the viewport at the bottom would answer a
                // search by showing the operator somewhere else entirely.
                let at = jump.saturating_sub(painted_from) as usize;
                (at < body.len()).then(|| {
                    let back = body[at..].iter().filter(|b| **b == b'\n').count();
                    PaneOp::ScrollFromEnd(u32::try_from(back).unwrap_or(u32::MAX))
                })
            }
            // Out of range means the daemon returned less than was asked for,
            // in which case scrolling anywhere would be a guess.
            Some(_) => None,
            None => keep_view.then_some(PaneOp::ScrollKeep),
        };

        if !body.is_empty() {
            ops.push(PaneOp::Write(Chunk::from(painted, start)));
        }
        if let Some(scroll) = scroll {
            ops.push(scroll);
        }
    }

    /// Fixture mode's substitute for a session: literal lines, no socket.
    pub(crate) fn banner(&mut self, lines: &[String], ops: &mut Vec<PaneOp>) {
        self.backfilling = false;
        ops.push(PaneOp::Reset);
        ops.push(PaneOp::Write(Chunk::owned(lines.join("\r\n").into_bytes())));
    }
}

// ---------------------------------------------------------------------------
// The connection
// ---------------------------------------------------------------------------

/// Everything one socket can tell the UI thread.
#[derive(Debug)]
pub(crate) enum SocketEvent {
    Open,
    /// A control-plane message, already parsed.
    Server(Box<ServerMsg>),
    /// One data-plane frame, header validated, bytes untouched.
    ///
    /// Deliberately not a member of [`crate::wire::ClientEvent`]: PTY output
    /// never reaches the reducer, never marks a signal dirty, and never causes
    /// the shell to repaint. It goes to the pane and stops there.
    Output(Frame),
    /// The socket closed, cleanly or otherwise. Carries a sentence, not a code.
    Closed(String),
    /// The socket could not be opened, or refused mid-stream.
    Error(String),
    /// Something arrived that this client could not make sense of.
    Bad(String),
}

/// Handle on the session socket and on the pane behind it.
///
/// Not `Copy`, and reached through a `CopyValue` in `main.rs`, because it owns
/// a channel and a sink. One per window: two windows are two attachments to
/// the same daemon and two independent panes.
pub(crate) struct Net {
    /// Where control-plane text goes. `None` before the first connect and
    /// between a socket dying and the next one.
    out: Option<UnboundedSender<String>>,
    /// Which socket is current. Events from an earlier one are discarded:
    /// without this a dying socket's close would overwrite the new socket's
    /// state with a stale "disconnected".
    epoch: u64,
    events: UnboundedSender<(u64, SocketEvent)>,
    /// The runtime the socket task runs on. Captured on the UI thread, where
    /// dioxus-desktop's multi-threaded runtime is in context, so the socket
    /// does its I/O on a worker rather than between two paints.
    runtime: Option<tokio::runtime::Handle>,
    /// The pane, once its surface exists.
    sink: Option<Rc<RefCell<dyn PaneSink>>>,
    /// Ops emitted before the surface existed.
    ///
    /// A window can be told to focus a session before its drawing area has
    /// been realised. Dropping those ops would lose the attach repaint and
    /// leave a blank pane with a live socket behind it, which is the worst of
    /// the three options; replaying them on attach costs one deferred vector
    /// that is emptied once per window.
    deferred: Vec<PaneOp>,
    deferred_bytes: usize,
    pub(crate) stream: PaneStream,
    /// Scratch, reused so a live frame costs no allocation beyond the payload
    /// it already owns.
    ops: Vec<PaneOp>,
}

impl Net {
    pub(crate) fn new() -> (Self, UnboundedReceiver<(u64, SocketEvent)>) {
        let (events, rx) = unbounded_channel();
        let net = Self {
            out: None,
            epoch: 0,
            events,
            runtime: tokio::runtime::Handle::try_current().ok(),
            sink: None,
            deferred: Vec::new(),
            deferred_bytes: 0,
            stream: PaneStream::default(),
            ops: Vec::new(),
        };
        (net, rx)
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Give this window's pane to the socket, and replay what it missed.
    pub(crate) fn attach_pane(&mut self, sink: Rc<RefCell<dyn PaneSink>>) {
        self.sink = Some(sink.clone());
        if self.deferred.is_empty() {
            return;
        }
        let held = std::mem::take(&mut self.deferred);
        self.deferred_bytes = 0;
        apply(&held, &mut *sink.borrow_mut());
    }

    /// Open, or reopen, the session socket.
    pub(crate) fn connect(&mut self, url: String) {
        self.epoch += 1;
        let epoch = self.epoch;
        let (tx, rx) = unbounded_channel();
        // Dropping the previous sender is what closes the previous task: its
        // outbound receiver returns `None` and it shuts its socket down
        // without reporting a close, because the close is ours.
        self.out = Some(tx);
        let events = self.events.clone();
        let Some(runtime) = self.runtime.clone() else {
            let _ = self.events.send((
                epoch,
                SocketEvent::Error(format!(
                    "{url}: this window has no async runtime to open a socket on; \
                     restart vitrum"
                )),
            ));
            return;
        };
        runtime.spawn(run(url, epoch, rx, events));
    }

    /// Close the current socket without opening another.
    ///
    /// Dropping the sender is the whole mechanism, the same one
    /// [`Net::connect`] relies on. Used when the client has decided it cannot
    /// complete the handshake, where leaving the socket open would hold a
    /// connection that will never say anything.
    pub(crate) fn hang_up(&mut self) {
        self.out = None;
    }

    /// Send one control-plane message.
    pub(crate) fn send(&self, text: String) {
        let Some(out) = &self.out else {
            tracing::debug!("control message dropped: no socket");
            return;
        };
        if out.send(text).is_err() {
            tracing::debug!("control message dropped: the socket task is gone");
        }
    }

    /// Run one pane state-machine step and apply what it produced.
    ///
    /// Every mutation of [`Self::stream`] goes through here, so ops reach the
    /// pane in the order the state machine emitted them and no caller can
    /// forget to flush.
    pub(crate) fn drive(&mut self, act: impl FnOnce(&mut PaneStream, &mut Vec<PaneOp>)) {
        self.ops.clear();
        act(&mut self.stream, &mut self.ops);
        if self.ops.is_empty() {
            return;
        }
        match &self.sink {
            Some(sink) => {
                let sink = sink.clone();
                apply(&self.ops, &mut *sink.borrow_mut());
            }
            None => {
                for op in self.ops.drain(..) {
                    if let PaneOp::Write(chunk) = &op {
                        self.deferred_bytes += chunk.len();
                    }
                    self.deferred.push(op);
                }
                // The same bound the live path has, for the same reason: a
                // window whose surface never appears must not accumulate a
                // session's whole output in a vector nobody will read.
                if self.deferred_bytes > PENDING_CAP {
                    self.deferred.clear();
                    self.deferred_bytes = 0;
                    self.deferred.push(PaneOp::Reset);
                }
            }
        }
        // The payloads are the only large thing here and they were moved in,
        // so releasing them now keeps the scratch vector's capacity without
        // keeping a megabyte of a backfill alive until the next frame.
        self.ops.clear();
    }
}

/// Own one socket until it dies or is superseded.
async fn run(
    url: String,
    epoch: u64,
    mut outbound: UnboundedReceiver<String>,
    events: UnboundedSender<(u64, SocketEvent)>,
) {
    let say = |event| events.send((epoch, event)).is_ok();

    let ws = match tokio_tungstenite::connect_async(url.as_str()).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            say(SocketEvent::Error(format!(
                "cannot reach {url}: {e}. Start vitrum-server, or point this \
                 window at another daemon in Settings."
            )));
            return;
        }
    };
    if !say(SocketEvent::Open) {
        return;
    }
    let (mut sink, mut stream) = ws.split();

    loop {
        tokio::select! {
            text = outbound.recv() => match text {
                Some(text) => {
                    if let Err(e) = sink.send(Message::Text(text)).await {
                        say(SocketEvent::Error(format!(
                            "the connection to {url} failed while sending: {e}"
                        )));
                        return;
                    }
                }
                // Superseded by a newer socket, or the window is gone. Close
                // quietly: reporting it would tell the UI the daemon dropped a
                // connection it replaced on purpose.
                None => {
                    let _ = sink.close().await;
                    return;
                }
            },
            frame = stream.next() => {
                let Some(frame) = frame else {
                    say(SocketEvent::Closed("the connection dropped".to_string()));
                    return;
                };
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(e) => {
                        say(SocketEvent::Error(format!("the connection failed: {e}")));
                        return;
                    }
                };
                if !accept(frame, &say) {
                    return;
                }
            }
        }
    }
}

/// Turn one frame into events. Returns whether the connection continues.
fn accept(frame: Message, say: &impl Fn(SocketEvent) -> bool) -> bool {
    match frame {
        Message::Text(text) => match serde_json::from_str::<ServerMsg>(&text) {
            Ok(msg) => say(SocketEvent::Server(Box::new(msg))),
            Err(e) => say(SocketEvent::Bad(format!(
                "the daemon sent a control frame this client cannot read: {e}. \
                 Restart vitrum-server so both ends come from the same release."
            ))),
        },
        // The whole message is moved into the frame. Nothing decodes it, so a
        // multi-byte character split across two frames is still split exactly
        // where the child split it, and nothing copies it, so the payload the
        // parser reads is the one the socket wrote.
        Message::Binary(bytes) => match Frame::parse(bytes) {
            Ok(frame) => say(SocketEvent::Output(frame)),
            Err(detail) => say(SocketEvent::Bad(detail)),
        },
        Message::Close(frame) => {
            let detail = frame.map_or_else(
                || "the connection dropped".to_string(),
                |f| close_reason(f.code.into(), f.reason.as_ref()),
            );
            say(SocketEvent::Closed(detail));
            false
        }
        // tungstenite answers pings itself; nothing else is ours to handle.
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => true,
    }
}

/// A close code as a sentence.
///
/// A close code is a protocol number, not an explanation. `code 1006` on the
/// sidebar banner tells an operator nothing; it is the WebSocket code for a
/// connection that dropped without a close frame, which is what happens when
/// the daemon dies or the socket is cut. The ones worth naming are named, and
/// anything unrecognised still prints its number rather than being flattened
/// into a vague sentence: an unknown failure must stay identifiable.
pub(crate) fn close_reason(code: u16, reason: &str) -> String {
    if !reason.is_empty() {
        return format!("{reason} (code {code})");
    }
    match code {
        1000 => "the daemon closed the connection".to_string(),
        1001 => "the daemon is shutting down".to_string(),
        1006 => "the connection dropped".to_string(),
        1011 => "the daemon hit an internal error".to_string(),
        1012 => "the daemon is restarting".to_string(),
        other => format!("the connection closed with code {other}"),
    }
}
