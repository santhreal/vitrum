//! A WebSocket client that speaks the daemon's protocol.
//!
//! This is deliberately not the app's client. The app's job is to render; this
//! one's job is to apply pressure and to notice when an answer is wrong, so it
//! keeps the accounting the app has no reason to keep: which sequence numbers
//! arrived, how many bytes, and how long each request waited for its reply.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use futures_util::{FutureExt, SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use vitrum_proto::{ClientMsg, PROTOCOL_VERSION, ServerMsg, SessionId, decode_output};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The interpreter every measured session runs under.
///
/// The harness measures a daemon under a Linux rig, so this is a fixed part of
/// the workload rather than a choice: a workload that varied its shell would
/// vary what it is measuring.
const WORKLOAD_SHELL: &str = "/bin/sh";

/// Where those sessions start. Any directory that certainly exists will do; the
/// scripts never touch it.
const WORKLOAD_CWD: &str = "/tmp";

/// One connection to the daemon, with its own output accounting.
pub struct Client {
    socket: Socket,
    /// Total data-plane bytes this connection has received.
    pub bytes_in: u64,
    /// Total frames, control and data, this connection has received.
    pub frames_in: u64,
    /// Every session this connection has been told exited, and the code.
    ///
    /// Counted here rather than by the caller because an exit can arrive during
    /// any wait: a short-lived session finishes while later sessions are still
    /// being created, and the round trip that was waiting for a different reply
    /// would drop the exit on the floor. Recording it as the frame passes
    /// through means no phase of a workload can lose one.
    pub exits: HashMap<SessionId, Option<i32>>,
    /// Per-session output continuity, keyed by session.
    pub streams: HashMap<SessionId, Stream>,
}

/// What one session's output stream looked like on this connection.
///
/// `seq` is the byte offset of a frame's first byte within the session's output,
/// so continuity is checkable rather than inferable. A total byte count can only
/// say that something is missing; offsets say exactly how much and how many
/// times, which is the difference between a symptom and a defect.
#[derive(Debug, Clone, Default)]
pub struct Stream {
    /// Offset of the first byte this connection ever saw for the session.
    ///
    /// Non-zero is not loss: a session starts producing output the moment it is
    /// spawned, and a client that attaches a moment later legitimately begins
    /// mid-stream. Reporting it separately is what keeps that from being
    /// counted as a bug.
    pub first_seq: u64,
    /// Offset just past the last byte received in order.
    pub next_seq: u64,
    /// Frames that arrived at a higher offset than expected.
    pub gaps: u64,
    /// Bytes those gaps skipped. This is real loss.
    pub gap_bytes: u64,
    /// Frames that repeated or overlapped a range already received.
    pub overlaps: u64,
    pub frames: u64,
    pub bytes: u64,
}

/// The outcome of one `List`, with enough context to explain a slow answer.
#[derive(Debug, Clone)]
pub struct ListRead {
    /// The newest snapshot seen, or `None` when the daemon never sent one.
    pub sessions: Option<Vec<vitrum_proto::SessionInfo>>,
    /// How many snapshots arrived. More than one means the daemon was also
    /// resyncing this connection, which only happens when it fell behind.
    pub snapshots: usize,
    /// Control frames consumed while waiting, which is the backlog size.
    pub control_frames: usize,
    pub elapsed: Duration,
    /// False when the connection was still receiving when the budget ran out.
    pub converged: bool,
}

/// Anything the daemon sent: a control message, or PTY bytes.
#[derive(Debug)]
pub enum Incoming {
    Control(Box<ServerMsg>),
    Output(Output),
}

/// One data-plane frame: which session, which sequence, how many bytes.
#[derive(Debug, Clone)]
pub struct Output {
    pub session: SessionId,
    pub seq: u64,
    pub bytes: Vec<u8>,
}

impl Client {
    /// Connect and complete the handshake.
    ///
    /// A version mismatch fails here rather than at the first odd reply,
    /// because every later measurement would be of two programs disagreeing.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let (socket, _) = connect_async(url)
            .await
            .with_context(|| format!("connecting to {url}"))?;
        let mut c = Self {
            socket,
            bytes_in: 0,
            frames_in: 0,
            exits: HashMap::new(),
            streams: HashMap::new(),
        };
        c.send(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            // The same file the GUI reads. A benchmark that carried its own
            // credential would be measuring a path the product does not have.
            token: vitrum_proto::token::load()
                .context("reading the daemon's authentication token")?,
        })
        .await?;
        match c.next_control(Duration::from_secs(10)).await? {
            ServerMsg::Welcome { protocol, .. } if protocol == PROTOCOL_VERSION => Ok(c),
            ServerMsg::Welcome { protocol, .. } => bail!(
                "this harness speaks protocol {PROTOCOL_VERSION}, the daemon speaks {protocol}; \
                 run a daemon built from the same tree"
            ),
            other => bail!("expected a welcome, got {other:?}"),
        }
    }

    pub async fn send(&mut self, msg: &ClientMsg) -> anyhow::Result<()> {
        let text = serde_json::to_string(msg)?;
        self.socket.send(Message::Text(text)).await?;
        Ok(())
    }

    /// Send raw text, for the fuzzer, which is the only caller that needs to
    /// put something on the wire that `ClientMsg` cannot represent.
    pub async fn send_raw(&mut self, text: String) -> anyhow::Result<()> {
        self.socket.send(Message::Text(text)).await?;
        Ok(())
    }

    /// The next frame of any kind, or `None` when the deadline passes.
    ///
    /// A zero timeout means "whatever is already buffered", not "nothing". It
    /// polls the socket once and gives up only if the poll is pending. Treating
    /// zero as an immediate `None` would make every drain a no-op, and a drain
    /// that reads nothing leaves the backlog in place for the next measured
    /// round trip to be blamed for.
    pub async fn next(&mut self, timeout: Duration) -> anyhow::Result<Option<Incoming>> {
        let deadline = Instant::now() + timeout;
        loop {
            let frame = if timeout.is_zero() {
                match self.socket.next().now_or_never() {
                    Some(f) => f,
                    None => return Ok(None),
                }
            } else {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(None);
                }
                match tokio::time::timeout(left, self.socket.next()).await {
                    Ok(f) => f,
                    Err(_) => return Ok(None),
                }
            };
            let Some(frame) = frame else {
                bail!("the daemon closed the connection")
            };
            self.frames_in += 1;
            match frame? {
                Message::Text(t) => {
                    let msg: ServerMsg = serde_json::from_str(&t)
                        .with_context(|| format!("decoding a control frame: {t:.200}"))?;
                    if let ServerMsg::Exited { session, code } = &msg {
                        self.exits.insert(*session, *code);
                    }
                    return Ok(Some(Incoming::Control(Box::new(msg))));
                }
                Message::Binary(b) => {
                    let (session, seq, bytes) = decode_output(&b)
                        .map_err(|e| anyhow!("decoding a data frame of {} bytes: {e:?}", b.len()))?;
                    self.bytes_in += bytes.len() as u64;
                    let end = seq + bytes.len() as u64;
                    let s = self.streams.entry(session).or_insert(Stream {
                        first_seq: seq,
                        next_seq: seq,
                        ..Stream::default()
                    });
                    if seq > s.next_seq {
                        s.gaps += 1;
                        s.gap_bytes += seq - s.next_seq;
                    } else if seq < s.next_seq {
                        s.overlaps += 1;
                    }
                    // Monotonic: an out-of-order or repeated frame must not pull
                    // the expected offset backwards and manufacture a gap out of
                    // the bytes already accounted for.
                    s.next_seq = s.next_seq.max(end);
                    s.frames += 1;
                    s.bytes += bytes.len() as u64;
                    return Ok(Some(Incoming::Output(Output {
                        session,
                        seq,
                        bytes: bytes.to_vec(),
                    })));
                }
                // Ping, pong and close carry no payload this harness measures.
                Message::Close(_) => bail!("the daemon closed the connection"),
                _ => continue,
            }
        }
    }

    /// The next control message, skipping data frames but still counting them.
    pub async fn next_control(&mut self, timeout: Duration) -> anyhow::Result<ServerMsg> {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next(left).await? {
                Some(Incoming::Control(m)) => return Ok(*m),
                Some(Incoming::Output(_)) => continue,
                None => bail!("no control message within {timeout:?}"),
            }
        }
    }

    /// Read until nothing arrives for `quiet`, or until `budget` runs out.
    ///
    /// Returns every control message consumed, and whether the connection went
    /// quiet within the budget. A fixed sleep cannot do this job: the daemon
    /// broadcasts one message per connection per mutation, so the time to
    /// deliver a storm scales with the storm, and a sleep long enough for the
    /// worst case is dead time in every other run. Waiting for quiet measures
    /// convergence instead of guessing at it.
    pub async fn drain_until_quiet(
        &mut self,
        quiet: Duration,
        budget: Duration,
    ) -> anyhow::Result<(Vec<ServerMsg>, bool)> {
        let deadline = Instant::now() + budget;
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok((seen, false));
            }
            match self.next(quiet.min(left)).await? {
                Some(Incoming::Control(m)) => seen.push(*m),
                Some(Incoming::Output(_)) => {}
                // Nothing for a whole quiet window: converged.
                None if left > quiet => return Ok((seen, true)),
                None => return Ok((seen, false)),
            }
        }
    }

    /// The daemon's current session list, read unambiguously.
    ///
    /// `List` has no request id, and the daemon also pushes an unsolicited
    /// `Sessions` snapshot to any connection that lags the broadcast bus. So the
    /// first snapshot after a `List` is not necessarily the answer to it: it may
    /// be a resync describing an earlier moment. Taking the LAST snapshot before
    /// the connection goes quiet is the only reading that cannot be stale.
    pub async fn list_now(&mut self, quiet: Duration, budget: Duration) -> anyhow::Result<ListRead> {
        let start = Instant::now();
        self.send(&ClientMsg::List).await?;
        let (seen, converged) = self.drain_until_quiet(quiet, budget).await?;
        let elapsed = start.elapsed();
        let control_frames = seen.len();
        let mut snapshots = 0;
        let mut sessions = None;
        for m in seen {
            if let ServerMsg::Sessions { sessions: s } = m {
                snapshots += 1;
                sessions = Some(s);
            }
        }
        Ok(ListRead {
            sessions,
            snapshots,
            control_frames,
            elapsed,
            converged,
        })
    }

    /// Consume whatever has already arrived, without waiting for more.
    ///
    /// A latency measurement is only honest if the reply it times is the reply
    /// to the request it sent. The daemon publishes unsolicited updates as
    /// sessions start and produce their first bytes, so a measured round trip
    /// taken with those still queued would time an event that predates the
    /// request. Draining first is what makes the number mean what it says.
    pub async fn drain_ready(&mut self) -> anyhow::Result<()> {
        while self.next(Duration::ZERO).await?.is_some() {}
        Ok(())
    }

    /// Send `msg`, then wait for the first control message `want` accepts.
    ///
    /// Returns how long the round trip took, which is the number every load
    /// measurement is built from.
    pub async fn round_trip<T>(
        &mut self,
        msg: &ClientMsg,
        timeout: Duration,
        mut want: impl FnMut(ServerMsg) -> Option<T>,
    ) -> anyhow::Result<(T, Duration)> {
        let start = Instant::now();
        self.send(msg).await?;
        let deadline = start + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("no matching reply to {msg:?} within {timeout:?}");
            }
            let got = self.next_control(left).await?;
            // An error naming this request is the answer, not noise to skip.
            if let ServerMsg::Error { message, .. } = &got {
                bail!("the daemon refused {msg:?}: {message}");
            }
            // A message that is not the answer is another connection's traffic,
            // so there is nothing to carry out of the predicate but the answer.
            if let Some(v) = want(got) {
                return Ok((v, start.elapsed()));
            }
        }
    }

    /// Create a session and return its id.
    ///
    /// `tag` must be unique across the whole run. The daemon broadcasts
    /// `SessionCreated` to every connection, so a client that accepts the first
    /// one it sees will happily adopt another connection's session id when two
    /// connections create at once. Matching on the title this call chose is what
    /// makes the answer belong to the request.
    ///
    /// `script` runs under the workload shell: every measured session is a
    /// generator, so the interpreter and directory are the harness's business
    /// rather than each caller's.
    pub async fn create_session(
        &mut self,
        tag: &str,
        script: &str,
        cols: u16,
        rows: u16,
        timeout: Duration,
    ) -> anyhow::Result<(SessionId, Duration)> {
        let msg = ClientMsg::CreateSession {
            project_id: vitrum_proto::ProjectId(1),
            cwd: WORKLOAD_CWD.to_string(),
            command: WORKLOAD_SHELL.to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            cols,
            rows,
            title: Some(tag.to_string()),
        };
        let tag = tag.to_string();
        self.round_trip(&msg, timeout, move |m| match m {
            ServerMsg::SessionCreated(info) if info.title == tag => Some(info.id),
            _ => None,
        })
        .await
    }

    pub async fn close_session(
        &mut self,
        session: SessionId,
        timeout: Duration,
    ) -> anyhow::Result<Duration> {
        let msg = ClientMsg::Close { session };
        let (_, d) = self
            .round_trip(&msg, timeout, |m| match m {
                ServerMsg::SessionRemoved { session: s } if s == session => Some(()),
                _ => None,
            })
            .await?;
        Ok(d)
    }

    /// Attach to `session` at this window's geometry.
    ///
    /// The daemon confirms by broadcasting the session's updated projection.
    /// Matching on the session id is safe here: attach acknowledges itself
    /// with a `SessionUpdated` naming this session, and a second window
    /// attaching at the same moment names a different id.
    pub async fn attach(
        &mut self,
        session: SessionId,
        cols: u16,
        rows: u16,
        timeout: Duration,
    ) -> anyhow::Result<Duration> {
        let msg = ClientMsg::Attach {
            session,
            cols,
            rows,
        };
        let (_, d) = self
            .round_trip(&msg, timeout, |m| match m {
                ServerMsg::SessionUpdated(info) if info.id == session => Some(()),
                _ => None,
            })
            .await?;
        Ok(d)
    }

    /// Stop receiving live output for `session`. The session keeps running.
    pub async fn detach(&mut self, session: SessionId, timeout: Duration) -> anyhow::Result<Duration> {
        let msg = ClientMsg::Detach { session };
        let (_, d) = self
            .round_trip(&msg, timeout, |m| match m {
                ServerMsg::SessionUpdated(info) if info.id == session => Some(()),
                _ => None,
            })
            .await?;
        Ok(d)
    }

    /// Write `data` to the session's PTY. Input has no acknowledgement, so the
    /// recorded latency is the send itself.
    pub async fn send_input(&mut self, session: SessionId, data: &[u8]) -> anyhow::Result<Duration> {
        let start = Instant::now();
        self.send(&ClientMsg::Input {
            session,
            data: data.to_vec(),
        })
        .await?;
        Ok(start.elapsed())
    }

    /// Resize `session` to this window's geometry.
    ///
    /// The daemon acknowledges resize with a broadcast `SessionUpdated` only
    /// when the PTY actually changes size; a no-op resize (the session is
    /// already at the smallest requested geometry) publishes nothing. So there
    /// is no reply to wait for — the recorded latency is the send, and the
    /// convergence check reads the geometry back with `list_now`.
    pub async fn resize(
        &mut self,
        session: SessionId,
        cols: u16,
        rows: u16,
        _timeout: Duration,
    ) -> anyhow::Result<Duration> {
        let start = Instant::now();
        self.send(&ClientMsg::Resize {
            session,
            cols,
            rows,
        })
        .await?;
        Ok(start.elapsed())
    }

    /// Fetch history older than `before_seq`, collecting every chunk until the
    /// daemon reports `more: false`.
    pub async fn scrollback(
        &mut self,
        session: SessionId,
        before_seq: u64,
        max_bytes: u32,
        timeout: Duration,
    ) -> anyhow::Result<(Vec<u8>, Duration)> {
        let start = Instant::now();
        let deadline = Instant::now() + timeout;
        let mut data = Vec::new();
        let mut before = before_seq;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!("scrollback for session {} never finished within {timeout:?}", session.0);
            }
            // The daemon answers each Scrollback request with exactly one
            // chunk; to page back the client re-asks with that chunk's
            // from_seq as the new before_seq. So the loop is over requests.
            self.send(&ClientMsg::Scrollback {
                session,
                before_seq: before,
                max_bytes,
            })
            .await?;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    bail!("scrollback for session {} never finished within {timeout:?}", session.0);
                }
                match self.next_control(left).await? {
                    ServerMsg::ScrollbackChunk {
                        session: s,
                        data: chunk,
                        from_seq,
                        more,
                        ..
                    } if s == session => {
                        data.extend_from_slice(&chunk);
                        if more {
                            before = from_seq;
                            break;
                        }
                        return Ok((data, start.elapsed()));
                    }
                    ServerMsg::Error { session: s, message, .. } if s == Some(session) => {
                        bail!("scrollback for session {} refused: {message}", session.0);
                    }
                    // The daemon broadcasts other control traffic (session
                    // updates, created/closed notices) on the same channel;
                    // skip anything that is not this session's scrollback.
                    _ => continue,
                }
            }
        }
    }

    /// Run one server-side search and return the hits.
    pub async fn search(
        &mut self,
        sessions: &[SessionId],
        pattern: &str,
        regex: bool,
        case_insensitive: bool,
        whole_word: bool,
        timeout: Duration,
    ) -> anyhow::Result<(Vec<vitrum_proto::SearchHit>, Duration)> {
        let msg = ClientMsg::Search {
            sessions: sessions.to_vec(),
            pattern: pattern.to_string(),
            regex,
            case_insensitive,
            whole_word,
            context_lines: 0,
            max_hits: 1000,
        };
        let (hits, d) = self
            .round_trip(&msg, timeout, |m| match m {
                ServerMsg::SearchResults { hits, .. } => Some(hits),
                _ => None,
            })
            .await?;
        Ok((hits, d))
    }
}
