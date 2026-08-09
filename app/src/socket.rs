//! The session WebSocket, and the data plane behind it.
//!
//! # What this module replaced
//!
//! Until this module existed the socket lived in `bootstrap.js`, and the
//! inventory below is what that file did with it. It is written down because
//! it is the specification this module has to satisfy byte for byte, not
//! because it is history: every clause here is a guarantee some server-side
//! seam test in `crates/vitrum-server/src/tests/seam_*.rs` already pins from
//! the other end.
//!
//! 1. **Connect.** `connect(url)` tore down any previous socket first, and
//!    detached its handlers before closing so a dying socket's `onclose` could
//!    not overwrite the new one's state with a stale "disconnected". A
//!    constructor throw was reported as `conn/error` rather than thrown.
//! 2. **Lifecycle.** `onopen`, `onerror` and `onclose` each pushed one
//!    `conn` event to Rust. Close codes were translated to sentences (1000,
//!    1001, 1006, 1011, 1012), and anything unrecognised kept its number so an
//!    unknown failure stayed identifiable.
//! 3. **Reconnect.** None here. The schedule is Rust's, in
//!    [`crate::sync::schedule_reconnect`], and it re-entered this file only by
//!    sending another `connect` command.
//! 4. **Two planes on one socket.** A text frame was `JSON.parse`d and
//!    forwarded to Rust verbatim as `{ev:"server"}`. A binary frame went to
//!    `onFrame` and never reached Rust at all.
//! 5. **Framing and the header strip.** `onFrame` refused a frame shorter than
//!    the 17-byte header, refused a kind other than
//!    [`vitrum_proto::FRAME_KIND_OUTPUT`], read the session id from bytes 1..9
//!    as a little-endian u64, and took the payload from byte 17. The seq at
//!    bytes 9..17 was decoded ONLY while a backfill was in flight, because on
//!    the live path nothing needs it.
//! 6. **Filtering.** A frame for a session other than the focused one, or an
//!    empty payload, or a window with no terminal built yet, was dropped
//!    before the header was decoded.
//! 7. **Batching.** None on the live path: that was measured and removed.
//!    `join` coalesced only where frames genuinely arrive in bulk, which is
//!    the splice after a focus and the overflow flush.
//! 8. **The multi-byte guarantee.** Concatenation happened in arrival order
//!    and nothing decoded a payload as text anywhere on the path, so a UTF-8
//!    character split across two frames was whole again before xterm's decoder
//!    saw it. This module keeps that by moving `Vec<u8>` and never a `String`.
//! 9. **Backlog and seq.** On a focus change the pane buffered live frames
//!    instead of painting them, up to [`PENDING_CAP`]; past the cap it gave up
//!    on ordering, painted the live bytes and marked the late backfill to be
//!    discarded. When the backfill landed it was painted first and the
//!    buffered frames were spliced onto it BY BYTE OFFSET against
//!    `resume_seq`, dropping the prefix of the first overlapping frame and
//!    reporting a hole if the ring had evicted the joint.
//! 10. **Scrollback paging.** `sync.rs` asks for a bigger window ending at the
//!     same head and repaints, because xterm.js cannot prepend. The pane
//!     buffered live output from the moment the request went out, reset the
//!     grid, painted history plus the spliced live bytes as ONE write, and
//!     then put the viewport back on the line the operator was reading,
//!     counted as a distance from the END of the buffer so a scrollback trim
//!     could not move it. A search jump did the same with a line computed from
//!     `jump_seq - from_seq`.
//!
//! # What is here now
//!
//! [`Net`] owns the connection. A tokio task holds the socket off the UI
//! thread and forwards decoded frames as [`SocketEvent`]s; [`PaneStream`] is
//! the state machine from clauses 5 to 10, on the UI thread, pure and
//! testable. It emits [`PaneOp`]s, which reach the webview over the binary
//! route in [`PaneQueue`] rather than through the JSON eval channel, so
//! nothing on this path is base64 and nothing is re-decoded.
//!
//! # What is NOT here
//!
//! The pane still renders with xterm.js this pass, so the last hop is a
//! `fetch` the webview drives. That hop is a `memcpy` and an
//! `ArrayBuffer`, not an encode; see `pane_channel_cost` in the tests for the
//! measurement.

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::tungstenite::Message;
use vitrum_dioxus_desktop::RequestAsyncResponder;
use vitrum_dioxus_desktop::wry::http::{Response, StatusCode};
use vitrum_proto::{FRAME_KIND_OUTPUT, OUTPUT_HEADER_LEN, ServerMsg, SessionId, decode_output};

#[cfg(test)]
mod tests;

/// Cap on live bytes held while waiting for a backfill to land.
///
/// Past this the backfill is abandoned and the buffer is flushed: a stalled
/// repaint must not turn into unbounded client memory just because an agent is
/// chatty. Unchanged from the JavaScript it came from, because
/// [`crate::wire::BACKFILL_CEILING_BYTES`] is sized against it.
pub(crate) const PENDING_CAP: usize = 1 << 20;

/// Path the webview pulls pane bytes from.
///
/// A wry asset handler rather than the Dioxus eval channel. The eval channel
/// is JSON, and JSON strings must be valid UTF-8, so PTY bytes could only
/// cross it base64-encoded: a 4/3 size tax, an encode in Rust and an `atob`
/// plus a per-byte copy in JavaScript, on the hottest path in the product.
/// This route hands the same `Vec<u8>` the socket produced straight to the
/// webview as an `ArrayBuffer`.
///
/// The name is the first path segment, which is what
/// `dioxus-desktop`'s `desktop_handler` matches a registered handler on.
pub(crate) const PANE_ROUTE: &str = "vitrum-pane";

// ---------------------------------------------------------------------------
// Pane operations
// ---------------------------------------------------------------------------

/// One instruction for the terminal pane, in the order it must be applied.
///
/// Everything that touches the grid travels this way, including the resets
/// that used to be their own bridge command. Two channels into one pane is two
/// orderings, and a reset that overtook the write it was meant to precede
/// would clear the repaint it was clearing FOR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PaneOp {
    /// Write these bytes to the grid, verbatim.
    Write(Vec<u8>),
    /// Full reset. Not a clear: the previous session may have left the grid in
    /// alternate-screen mode, with a scroll region set or with SGR state
    /// pending, and any of those would corrupt the incoming repaint.
    Reset,
    /// Scroll to the logical line `n` lines from the end of the buffer, with a
    /// third of a screen of context above it. Used for a search jump.
    ScrollFromEnd(u32),
    /// Scroll back to the line the operator was reading when they asked for
    /// more history. The distance is the webview's, because only the webview
    /// can count wrapped rows.
    ScrollKeep,
}

const OP_WRITE: u8 = 0;
const OP_RESET: u8 = 1;
const OP_SCROLL_FROM_END: u8 = 2;
const OP_SCROLL_KEEP: u8 = 3;

/// Byte length of one pane-op header: tag plus payload length.
pub(crate) const OP_HEADER_LEN: usize = 1 + 4;

/// Append `ops` to `out` in the wire form `bootstrap.js::applyOps` reads.
///
/// `[tag:u8][len:u32 LE][payload]`, repeated. Length-prefixed rather than
/// self-delimiting because a `Write` payload is arbitrary bytes: any sentinel
/// a terminal stream cannot contain does not exist.
pub(crate) fn encode_ops(ops: &[PaneOp], out: &mut Vec<u8>) {
    for op in ops {
        let scroll;
        let (tag, payload): (u8, &[u8]) = match op {
            PaneOp::Write(bytes) => (OP_WRITE, bytes),
            PaneOp::Reset => (OP_RESET, &[]),
            PaneOp::ScrollFromEnd(back) => {
                scroll = back.to_le_bytes();
                (OP_SCROLL_FROM_END, &scroll)
            }
            PaneOp::ScrollKeep => (OP_SCROLL_KEEP, &[]),
        };
        out.reserve(OP_HEADER_LEN + payload.len());
        out.push(tag);
        // A payload longer than u32::MAX cannot exist: the largest single op
        // is a backfill, and `wire::PAGE_CEILING_BYTES` is 8 MiB.
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
    }
}

// ---------------------------------------------------------------------------
// The data-plane state machine
// ---------------------------------------------------------------------------

/// A live frame held back while a backfill is in flight.
#[derive(Debug)]
struct Held {
    /// Absolute byte offset of this payload in the session's stream.
    seq: u64,
    data: Vec<u8>,
}

/// Everything the pane knows about the bytes it is painting.
///
/// Pure: it takes frames and commands and returns [`PaneOp`]s and notices, and
/// touches neither the socket nor the webview. That is what lets the ordering
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
    /// instead of painted after the live bytes that already went to the grid.
    drop_backfill: bool,
    /// True between a page-back request and its repaint, so holding the wheel
    /// at the top sends one request rather than one per tick.
    paging: bool,
    pending: Vec<Held>,
    pending_bytes: usize,
    /// Things the operator has to be told, drained by the caller.
    notices: Vec<String>,
}

impl PaneStream {
    /// The session the pane is pointed at.
    ///
    /// Test-only: production reads the focus through the state signal that set
    /// it, and a second reader of the same fact in the shipped build is how
    /// the two drift. Without the attribute this is a dead function, and this
    /// crate builds with warnings denied.
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
    /// Resets the grid and starts buffering live frames, so history and live
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
    /// request the client declined to send cannot leave the pane buffering
    /// live output forever.
    pub(crate) fn arm_page_back(&mut self) {
        self.paging = true;
        self.backfilling = true;
    }

    /// One decoded data frame.
    pub(crate) fn output(
        &mut self,
        session: SessionId,
        seq: u64,
        data: Vec<u8>,
        ops: &mut Vec<PaneOp>,
    ) {
        if self.focus != Some(session) || data.is_empty() {
            return;
        }
        // Contiguity, checked once per frame against the offset the previous
        // frame ended at. The server's seq is the cumulative byte count of the
        // session's stream, so a mismatch is output that was lost or repeated
        // between the ring and this process, and painting across it corrupts
        // the parse from there on: the missing byte is as likely to be inside
        // an escape sequence as inside a word.
        if let Some(want) = self.next_seq
            && want != seq
        {
            self.notices.push(format!(
                "the session stream jumped from byte {want} to byte {seq}; \
                 what is painted from here may be wrong"
            ));
        }
        self.next_seq = Some(seq.saturating_add(data.len() as u64));

        if self.backfilling {
            self.pending_bytes += data.len();
            self.pending.push(Held { seq, data });
            if self.pending_bytes > PENDING_CAP {
                // Give up on ordering the repaint rather than grow without
                // bound. The live bytes are what the operator actually needs.
                let mut all = Vec::with_capacity(self.pending_bytes);
                for held in self.pending.drain(..) {
                    all.extend_from_slice(&held.data);
                }
                self.pending_bytes = 0;
                self.backfilling = false;
                self.paging = false;
                self.drop_backfill = true;
                ops.push(PaneOp::Write(all));
                self.notices.push(
                    "backfill buffer overflowed; painted live output without history".to_string(),
                );
            }
            return;
        }
        ops.push(PaneOp::Write(data));
    }

    /// History for `session`, followed by whatever was buffered behind it.
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
        // so the grid has to be cleared first. On an attach the focus change
        // already reset it and this would be a second clear of an empty grid.
        if keep_view {
            ops.push(PaneOp::Reset);
        }

        // Painted as one write. The history and the live frames that overlap
        // it are consecutive bytes of one stream with nothing between them, so
        // there is no reason for the emulator to decode and reflow them in
        // pieces — and a split inside a multi-byte character would be a split
        // this module put there.
        let mut painted = history;
        let mut hole = 0u64;
        // Where the painted stream currently ends. It starts at the offset the
        // history was computed to and advances with each frame, because the
        // question a frame answers is whether it abuts what is already
        // painted — not whether it abuts the resume offset. Measuring every
        // frame against `resume_seq` reports a hole for the second frame of
        // any healthy pair, since only the first one starts there.
        let mut painted_to = resume_seq;
        for held in self.pending.drain(..) {
            let end = held.seq.saturating_add(held.data.len() as u64);
            if end <= painted_to {
                continue;
            }
            // The reverse of an overlap: after a reported gap the bytes
            // between the backfill and the first live frame may have been
            // evicted from the server's ring, so the painted stream ends BELOW
            // the oldest buffered frame. The grid was reset before this ran, so
            // painting the frames anyway is correct rather than a splice at
            // the wrong offset, but the hole is real history the operator will
            // never see and it gets said out loud.
            if held.seq > painted_to && hole == 0 {
                hole = held.seq - painted_to;
            }
            let skip = painted_to.saturating_sub(held.seq) as usize;
            painted.extend_from_slice(&held.data[skip.min(held.data.len())..]);
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
        // painted. The webview is left with only the part it alone can do:
        // turning a logical line index into a buffer row, which depends on how
        // the grid wrapped it.
        let scroll = match jump_seq {
            Some(jump) if jump >= from_seq => {
                let at = (jump - from_seq) as usize;
                (at < painted.len()).then(|| {
                    let back = painted[at..].iter().filter(|b| **b == b'\n').count();
                    PaneOp::ScrollFromEnd(u32::try_from(back).unwrap_or(u32::MAX))
                })
            }
            // Out of range means the daemon returned less than was asked for,
            // in which case scrolling anywhere would be a guess.
            Some(_) => None,
            None => keep_view.then_some(PaneOp::ScrollKeep),
        };

        if !painted.is_empty() {
            ops.push(PaneOp::Write(painted));
        }
        if let Some(scroll) = scroll {
            ops.push(scroll);
        }
    }

    /// Fixture mode's substitute for a session: literal lines, no socket.
    pub(crate) fn banner(&mut self, lines: &[String], ops: &mut Vec<PaneOp>) {
        self.backfilling = false;
        ops.push(PaneOp::Reset);
        ops.push(PaneOp::Write(lines.join("\r\n").into_bytes()));
    }
}

// ---------------------------------------------------------------------------
// The route the webview pulls from
// ---------------------------------------------------------------------------

/// Bytes waiting for the webview, and the request parked on them.
///
/// A long poll, not a timer and not a poll loop: the webview holds one `fetch`
/// open, this answers it the moment there is anything to say, and the webview
/// immediately opens the next one. An idle window has one parked request and
/// wakes for nothing, which is the same idle cost the WebSocket had when it
/// lived in JavaScript.
#[derive(Default)]
pub(crate) struct PaneQueue {
    buf: Vec<u8>,
    waiting: Option<RequestAsyncResponder>,
}

impl PaneQueue {
    /// Hand `body` to the webview.
    fn answer(responder: RequestAsyncResponder, body: Vec<u8>) {
        // `no-store` because every response is different and a cached one
        // would be the same bytes painted twice.
        let response = Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            .header("Cache-Control", "no-store")
            .body(body)
            .expect("a static header set and a byte body always build");
        responder.respond(response);
    }

    /// Queue encoded ops, and deliver them now if a request is parked.
    pub(crate) fn push(&mut self, encoded: &[u8]) {
        self.buf.extend_from_slice(encoded);
        if self.buf.is_empty() {
            return;
        }
        if let Some(responder) = self.waiting.take() {
            Self::answer(responder, std::mem::take(&mut self.buf));
        }
    }

    /// Answer a `fetch`, or park it until there is something to send.
    pub(crate) fn serve(&mut self, responder: RequestAsyncResponder) {
        if !self.buf.is_empty() {
            Self::answer(responder, std::mem::take(&mut self.buf));
            return;
        }
        // Only one request can be in flight; the webview's pump is a single
        // loop. A second means the previous page is gone, so the old responder
        // is closed out rather than leaked.
        if let Some(stale) = self.waiting.replace(responder) {
            Self::answer(stale, Vec::new());
        }
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
    /// One data-plane payload, header stripped, bytes untouched.
    Output {
        session: SessionId,
        seq: u64,
        data: Vec<u8>,
    },
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
/// a channel and a queue. One per window: two windows are two attachments to
/// the same daemon and two independent panes.
pub(crate) struct Net {
    /// Where control-plane text goes. `None` before the first connect and
    /// between a socket dying and the next one.
    out: Option<UnboundedSender<String>>,
    /// Which socket is current. Events from an earlier one are discarded:
    /// without this a dying socket's close would overwrite the new socket's
    /// state with a stale "disconnected", which is the exact bug the
    /// JavaScript detached its handlers to avoid.
    epoch: u64,
    events: UnboundedSender<(u64, SocketEvent)>,
    /// The runtime the socket task runs on. Captured on the UI thread, where
    /// dioxus-desktop's multi-threaded runtime is in context, so the socket
    /// does its I/O on a worker rather than between two paints.
    runtime: Option<tokio::runtime::Handle>,
    pub(crate) pane: Rc<RefCell<PaneQueue>>,
    pub(crate) stream: PaneStream,
    /// Scratch, reused so a live frame costs no allocation beyond the payload
    /// it already owns.
    ops: Vec<PaneOp>,
    encoded: Vec<u8>,
}

impl Net {
    pub(crate) fn new() -> (Self, UnboundedReceiver<(u64, SocketEvent)>) {
        let (events, rx) = unbounded_channel();
        let net = Self {
            out: None,
            epoch: 0,
            events,
            runtime: tokio::runtime::Handle::try_current().ok(),
            pane: Rc::new(RefCell::new(PaneQueue::default())),
            stream: PaneStream::default(),
            ops: Vec::new(),
            encoded: Vec::new(),
        };
        (net, rx)
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
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
                SocketEvent::Error(format!("{url}: no async runtime to open a socket on")),
            ));
            return;
        };
        runtime.spawn(run(url, epoch, rx, events));
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

    /// Encode whatever the pane state machine just produced and hand it over.
    ///
    /// Every mutation of [`Self::stream`] goes through here, so ops reach the
    /// webview in the order the state machine emitted them and no caller can
    /// forget to flush.
    pub(crate) fn drive(&mut self, act: impl FnOnce(&mut PaneStream, &mut Vec<PaneOp>)) {
        self.ops.clear();
        act(&mut self.stream, &mut self.ops);
        if self.ops.is_empty() {
            return;
        }
        self.encoded.clear();
        encode_ops(&self.ops, &mut self.encoded);
        self.pane.borrow_mut().push(&self.encoded);
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
            say(SocketEvent::Error(format!("cannot reach {url}: {e}")));
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
                        say(SocketEvent::Error(format!("the connection failed while sending: {e}")));
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
                "the daemon sent a control frame this client cannot read: {e}"
            ))),
        },
        Message::Binary(bytes) => match split_output(&bytes) {
            Ok((session, seq, at)) => say(SocketEvent::Output {
                session,
                seq,
                // The one copy on this path, and it is the copy that gets the
                // payload out of the socket's own buffer. Nothing decodes it,
                // so a multi-byte character split across two frames is still
                // split exactly where the child split it.
                data: bytes[at..].to_vec(),
            }),
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

/// Validate a data frame's header and return where its payload starts.
///
/// Returns the offset rather than the slice so the caller can copy out of the
/// frame it already owns without borrowing across the match.
pub(crate) fn split_output(frame: &[u8]) -> Result<(SessionId, u64, usize), String> {
    if frame.len() < OUTPUT_HEADER_LEN {
        return Err(format!(
            "data frame is {} bytes, need at least {OUTPUT_HEADER_LEN}",
            frame.len()
        ));
    }
    if frame[0] != FRAME_KIND_OUTPUT {
        return Err(format!("unknown frame kind {}", frame[0]));
    }
    let (session, seq, _) =
        decode_output(frame).map_err(|e| format!("undecodable data frame: {e}"))?;
    Ok((session, seq, OUTPUT_HEADER_LEN))
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
