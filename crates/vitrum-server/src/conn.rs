//! One client connection: control plane in, control and data planes out.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, anyhow};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use vitrum_core::{OutputChunk, SessionSpec, ViewerId};
use vitrum_proto::{ClientMsg, PROTOCOL_VERSION, ServerMsg, SessionId, encode_output};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{StatusCode, header};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::hub::Hub;
use crate::now_ms;
use crate::search;

/// Outbound frames queued per connection.
///
/// Bounded on purpose. A client that stops reading must apply backpressure, not
/// grow the server's heap: once this fills, the output pump stops draining the
/// session's broadcast channel, that channel laps, and the client is told the
/// exact gap. Three bounded stages beat one unbounded buffer, which is how a
/// competing shell reached a 3.9 GB heap.
const OUT_QUEUE: usize = 256;

/// How long a closing connection may take to drain its queued frames.
///
/// Bounded so one unresponsive client cannot pin a task forever, and long enough
/// that a refusal or a final exit notice actually reaches a live one.
const SHUTDOWN_FLUSH: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a peer may take to complete the HTTP upgrade.
///
/// Without a deadline a peer that opens a socket and sends nothing holds a
/// task, a file descriptor and a slot in the accept loop for as long as the
/// daemon runs, and it costs one `nc` to do it. Ten seconds is far longer than
/// a loopback upgrade needs and short enough that the leak is not one.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Largest control message the daemon will assemble from a peer.
///
/// tungstenite's own default is 64 MiB per message and 16 MiB per frame, which
/// is a peer choosing how much of this daemon's heap to take. The largest
/// legitimate inbound message is a paste, base64 encoded, and 4 MiB of pasted
/// text is already far past what a terminal can usefully receive at once.
///
/// Outbound is unaffected: this bounds what is read, and scrollback answers
/// are bounded by the ring they come from.
const MAX_INBOUND_MESSAGE: usize = 4 * 1024 * 1024;

/// How long a connection may be silent before the daemon probes it.
///
/// A client that vanishes without closing its socket — a laptop suspended, a
/// network dropped, a process killed with `SIGKILL` on a machine that then
/// went away — leaves a live TCP connection that will never carry another
/// byte. Nothing above notices. The read parks forever, the connection's
/// sessions stay attached, and their geometry stays registered, so every other
/// window is held to the layout of a window that no longer exists. The kernel
/// does eventually give up, after two hours of default keepalive that this
/// socket has not even enabled.
///
/// Silence itself is normal and must not be punished: an operator watching an
/// agent work sends nothing for minutes. So silence triggers a probe rather
/// than a close, and every conforming WebSocket peer answers a ping without
/// its application code being involved.
pub(crate) const IDLE_PROBE: std::time::Duration = std::time::Duration::from_secs(20);

/// How long the peer has to say anything at all once probed.
///
/// Any frame counts, not just the pong: a client that is talking is alive.
pub(crate) const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the daemon will hold one frame for a client that is not reading.
///
/// The outbound queue is bounded so a stalled client applies backpressure
/// instead of growing the daemon's heap, and every send onto it therefore
/// waits. Waiting with no deadline is the other half of that bargain going
/// wrong: a client that stops draining parks the connection task inside
/// `dispatch`, which is upstream of the read that the heartbeat above lives
/// in, so the connection can never be probed and never ends.
///
/// Thirty seconds is far past a rendering client. The queue holds 256 frames,
/// and a client that has not taken one of them in half a minute is not drawing
/// anything.
const SEND_STALL: std::time::Duration = std::time::Duration::from_secs(30);

/// The websocket limits every accepted connection runs under.
fn socket_limits() -> WebSocketConfig {
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_INBOUND_MESSAGE);
    config.max_frame_size = Some(MAX_INBOUND_MESSAGE);
    config
}

/// Refuse any handshake that carries an `Origin` header.
///
/// A browser attaches `Origin` to every cross-origin WebSocket handshake and
/// cannot be made not to. It also sends that handshake with no preflight and
/// no same-origin check, so without this any page the operator visits can open
/// `ws://127.0.0.1:7737`, create a session running any command, and read every
/// agent's transcript. A native client never sends the header, so refusing it
/// costs nothing and closes the entire browser-borne case at the upgrade,
/// before a single control message is parsed.
///
/// This is the outer of two layers. It does not defend against another local
/// user, who simply omits the header; the token does that.
fn refuse_browsers(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    let Some(origin) = request.headers().get(header::ORIGIN) else {
        return Ok(response);
    };
    tracing::warn!(
        origin = %String::from_utf8_lossy(origin.as_bytes()),
        "refused a handshake carrying Origin"
    );
    let mut refusal = ErrorResponse::new(Some(
        "vitrum-server refuses cross-origin connections. This daemon runs commands on \
         request, so it accepts only local native clients, which never send an Origin \
         header.\n"
            .to_string(),
    ));
    *refusal.status_mut() = StatusCode::FORBIDDEN;
    Err(refusal)
}

/// Whether the connection continues after a message.
#[derive(PartialEq, Eq, Debug)]
enum Flow {
    Continue,
    Close,
}

/// Run one accepted TCP connection to completion.
pub async fn serve_connection(stream: TcpStream, hub: Arc<Hub>) -> anyhow::Result<()> {
    // Bounded, because the upgrade is the one phase a peer controls the
    // duration of. Everything after it is driven by messages this daemon can
    // refuse.
    let ws = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        tokio_tungstenite::accept_hdr_async_with_config(
            stream,
            refuse_browsers,
            Some(socket_limits()),
        ),
    )
    .await
    .map_err(|_| {
        anyhow!("a peer did not finish the websocket handshake within {HANDSHAKE_TIMEOUT:?}")
    })?
    .context("websocket handshake")?;
    let (sink, mut incoming) = ws.split();
    let (out_tx, out_rx) = mpsc::channel::<Message>(OUT_QUEUE);
    let mut writer = tokio::spawn(write_loop(sink, out_rx));

    let mut conn = Conn::new(hub, out_tx);
    // Whether a probe is already outstanding. Cleared by any frame at all,
    // because a peer that speaks is a peer that is there.
    let mut probed = false;
    let result = loop {
        let quiet_for = if probed { PROBE_DEADLINE } else { IDLE_PROBE };
        let frame = match tokio::time::timeout(quiet_for, incoming.next()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break Ok(()),
            Err(_) if !probed => {
                probed = true;
                // Queued with a deadline like everything else: a client that
                // cannot take a two-byte ping in thirty seconds is the case
                // this probe exists to end, not a reason to park here.
                match tokio::time::timeout(SEND_STALL, conn.out.send(Message::Ping(Vec::new())))
                    .await
                {
                    Ok(Ok(())) => continue,
                    // The writer is gone, which means the socket is.
                    Ok(Err(_)) => break Ok(()),
                    Err(_) => {
                        break Err(anyhow!(
                            "client stopped reading: it took no frame for {SEND_STALL:?}. \
                             Reconnect the window; its sessions keep running."
                        ));
                    }
                }
            }
            Err(_) => {
                break Err(anyhow!(
                    "client vanished: no frame for {IDLE_PROBE:?} and no answer to a ping \
                     within {PROBE_DEADLINE:?}. Reconnect the window; its sessions keep \
                     running and its scrollback is retained."
                ));
            }
        };
        probed = false;
        let frame = match frame {
            Ok(f) => f,
            // A GUI that is quit, crashes, or loses its network does not send a
            // closing handshake. That is an ordinary end of connection, and
            // logging it as an error would bury real failures in noise.
            Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => break Ok(()),
            Err(WsError::Protocol(
                tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
            )) => break Ok(()),
            Err(e) => break Err(e).context("reading a websocket frame"),
        };
        let flow = match frame {
            Message::Text(text) => match conn.dispatch(&text).await {
                Ok(flow) => flow,
                Err(e) => break Err(e),
            },
            Message::Binary(_) => {
                let refusal = ServerMsg::error(
                    None,
                    "binary frames carry output only; control messages must be JSON text",
                );
                match conn.send(&refusal).await {
                    Ok(()) => Flow::Continue,
                    Err(e) => break Err(e),
                }
            }
            Message::Close(_) => Flow::Close,
            // tungstenite answers pings itself; nothing else is ours to handle.
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => Flow::Continue,
        };
        if flow == Flow::Close {
            break Ok(());
        }
    };

    // Dropping the connection aborts every pump and releases the last queue
    // sender, which is what tells the writer to flush and close.
    drop(conn);
    // The queue must drain before the socket goes: a client refused for a
    // protocol mismatch learns why from a frame that is still queued at this
    // point, and aborting the writer here would replace the explanation with an
    // abrupt reset.
    if tokio::time::timeout(SHUTDOWN_FLUSH, &mut writer)
        .await
        .is_err()
    {
        tracing::debug!("client did not drain queued frames; dropping the socket");
        writer.abort();
    }
    result
}

/// Per-connection state: what this client is attached to.
///
/// There is no per-session status watcher here any more. Registry events come
/// from the daemon-wide bus, so a client sees sessions started by another window
/// or by a bare terminal, which is the reason for having a session server at all.
struct Conn {
    hub: Arc<Hub>,
    out: mpsc::Sender<Message>,
    /// This connection's identity for geometry purposes.
    ///
    /// One per connection, because a window is one viewport. Windows are
    /// independent views of the same daemon, so two of them can show the same
    /// session at different sizes and the daemon has to reconcile that rather
    /// than let the newest layout win.
    viewer: ViewerId,
    /// Output pumps, one per attached session. These are what stay private to a
    /// connection: the binary firehose belongs to whoever is drawing that pane.
    attached: HashMap<SessionId, JoinHandle<()>>,
    /// Forwards the shared registry bus to this client. Starts at the handshake.
    events: Option<JoinHandle<()>>,
    greeted: bool,
}

impl Conn {
    fn new(hub: Arc<Hub>, out: mpsc::Sender<Message>) -> Self {
        let viewer = hub.manager.new_viewer();
        Self {
            hub,
            out,
            viewer,
            attached: HashMap::new(),
            events: None,
            greeted: false,
        }
    }

    async fn dispatch(&mut self, text: &str) -> anyhow::Result<Flow> {
        let msg: ClientMsg = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                // A parse failure is the client's bug, not a reason to drop a
                // connection that may have twenty live sessions behind it.
                self.error(None, format!("malformed control message: {e}"))
                    .await?;
                return Ok(Flow::Continue);
            }
        };

        if !self.greeted && !matches!(msg, ClientMsg::Hello { .. }) {
            self.error(None, "expected hello before any other message".to_string())
                .await?;
            return Ok(Flow::Continue);
        }

        match msg {
            ClientMsg::Hello { protocol, token } => {
                if protocol != PROTOCOL_VERSION {
                    // Refuse rather than guess. A version skew that is papered
                    // over surfaces later as corrupted output.
                    //
                    // The fix is named as well as the fault, and the sentence
                    // is kept under `vitrum_proto::MAX_ERROR_CHARS` so the
                    // wire layer does not cut the middle out of it. The usual
                    // cause is a client that applied a staged update while
                    // this daemon kept running the old code, and restarting
                    // the daemon ends every session it holds, which the
                    // operator has to know before they do it.
                    self.error(
                        None,
                        format!(
                            "unsupported protocol {protocol}; this daemon speaks \
                             {PROTOCOL_VERSION} and is version {}. Restart \
                             vitrum-server to match your client; that ends every \
                             session it holds, so do it when your agents are idle.",
                            env!("CARGO_PKG_VERSION")
                        ),
                    )
                    .await?;
                    return Ok(Flow::Close);
                }
                // The version is checked first on purpose: a client that is
                // simply out of date must be told so rather than told its
                // credentials are wrong, because the two send an operator to
                // opposite places. Version 2 carried no token at all, so a
                // version-2 client never reaches this line.
                if !self.hub.token_matches(&token) {
                    // Constant in what it says, whatever was presented. An
                    // error that distinguished "no token" from "wrong token"
                    // from "wrong length" would answer questions a caller has
                    // no legitimate reason to be asking.
                    tracing::warn!("refused a hello with an invalid token");
                    self.error(None, self.hub.token_refusal()).await?;
                    return Ok(Flow::Close);
                }
                self.greeted = true;
                // Registry events start flowing now, and only now: a client that
                // has not agreed on a protocol version cannot be sent typed state.
                self.start_event_forwarding();
                self.send(&ServerMsg::Welcome {
                    protocol: PROTOCOL_VERSION,
                    server_version: env!("CARGO_PKG_VERSION").to_string(),
                })
                .await?;
            }

            ClientMsg::List => self.send_snapshots().await?,

            ClientMsg::CreateSession {
                project_id,
                cwd,
                command,
                args,
                cols,
                rows,
                title,
            } => {
                let spec = SessionSpec {
                    project_id,
                    cwd: PathBuf::from(&cwd),
                    command,
                    args,
                    // The protocol carries no environment, so the child inherits
                    // the daemon's, plus the TERM the PTY layer guarantees.
                    env: Vec::new(),
                    cols,
                    rows,
                    title,
                };
                match self.hub.manager.spawn(spec) {
                    Ok(id) => {
                        // Deltas, and broadcast rather than sent only here.
                        // Broadcasting the full session list would make startup
                        // traffic quadratic in session count, but sending the
                        // delta only to the creating connection would leave every
                        // other window's sidebar silently missing this session,
                        // which defeats the point of a session server.
                        // `info` can be `None`: another window may have closed
                        // this session between the spawn and here. That is a
                        // race, not a protocol violation, and failing the
                        // connection would take a window's twenty other live
                        // sessions down with it.
                        let Some(info) = self.hub.manager.info(id) else {
                            self.error(Some(id), "the session was closed as it was created".to_string())
                                .await?;
                            return Ok(Flow::Continue);
                        };
                        self.hub.watch(id);
                        self.hub.publish(ServerMsg::SessionCreated(info));
                        // Announces the project too, if this created one.
                        self.hub.ensure_project(project_id, &cwd);
                        // A new session may be the second agent in a directory
                        // another one is already working in, which is the whole
                        // condition detection exists to catch. Hooked here, on
                        // the path that already publishes the delta, so nothing
                        // has to poll. A no-op while nobody is watching.
                        self.hub.sync_overlap();
                    }
                    Err(e) => self.error(None, format!("{e:#}")).await?,
                }
            }

            ClientMsg::Attach {
                session,
                cols,
                rows,
            } => {
                // Re-attaching replaces the previous pump instead of doubling
                // it, and it happens before the new attach so the geometry this
                // connection registers is the one it just sent.
                self.detach(session);
                let rx = match self.hub.manager.attach(session, self.viewer, cols, rows) {
                    Ok(rx) => rx,
                    Err(e) => {
                        self.error(Some(session), format!("{e:#}")).await?;
                        return Ok(Flow::Continue);
                    }
                };
                let handle = tokio::spawn(pump_output(session, rx, self.out.clone()));
                self.attached.insert(session, handle);
                self.hub.watch(session);
                // Attaching cleared unread and any pending bell and may have
                // changed the size every attached window has to fit inside, all
                // of which is daemon-wide state, so every window's sidebar needs
                // to see it, not just this one.
                if let Some(info) = self.hub.manager.info(session) {
                    self.hub.publish(ServerMsg::SessionUpdated(info));
                }
            }

            ClientMsg::Detach { session } => {
                // Idempotent: a tab switch may detach something already detached,
                // and the session keeps running either way.
                self.detach(session);
            }

            ClientMsg::Input { session, data } => {
                if let Err(e) = self.hub.manager.write(session, &data) {
                    self.error(Some(session), format!("{e:#}")).await?;
                }
            }

            ClientMsg::Resize {
                session,
                cols,
                rows,
            } => {
                if let Err(e) = self.hub.manager.resize(session, self.viewer, cols, rows) {
                    self.error(Some(session), format!("{e:#}")).await?;
                }
            }

            ClientMsg::Rename { session, title } => {
                match self.hub.manager.rename(session, &title) {
                    Ok(()) => {
                        // Published to EVERY connection, not just this one. A
                        // rename is daemon-wide state, so a second window must
                        // not keep showing the old name until its next List.
                        if let Some(info) = self.hub.manager.info(session) {
                            self.hub.publish(ServerMsg::SessionUpdated(info));
                        }
                    }
                    Err(e) => self.error(Some(session), format!("{e:#}")).await?,
                }
            }

            ClientMsg::Close { session } => match self.hub.manager.close(session) {
                Ok(()) => {
                    // The status watcher stays alive on purpose: killing the
                    // child makes the PTY report EOF, so every connected client
                    // learns the child died through one Exited delta.
                    //
                    // But `close` also REMOVES the session from the registry,
                    // and Exited does not say that. Without an explicit removal
                    // the row lives forever in every sidebar, and clicking it
                    // asks the daemon for a session it has already forgotten,
                    // which answers "no session N". Exited means the CHILD died
                    // and the scrollback is still there; SessionRemoved means
                    // the SESSION is gone. They are different facts and a
                    // client needs both.
                    self.detach(session);
                    self.hub.publish(ServerMsg::SessionRemoved { session });
                    // The project existed because a session was running in
                    // that directory. That stopped being true one line ago, so
                    // the window is told the folder is gone.
                    self.hub.publish_projects();
                    // A dead session cannot be fighting over a file, and its
                    // pid is reapable, so it leaves detection now rather than
                    // at the next time anyone asks.
                    self.hub.sync_overlap();
                }
                Err(e) => self.error(Some(session), format!("{e:#}")).await?,
            },

            ClientMsg::Scrollback {
                session,
                before_seq,
                max_bytes,
            } => match self
                .hub
                .manager
                .scrollback(session, before_seq, max_bytes as usize)
            {
                Some((from_seq, data, more)) => {
                    self.send(&ServerMsg::ScrollbackChunk {
                        session,
                        from_seq,
                        data,
                        more,
                    })
                    .await?
                }
                None => {
                    self.error(Some(session), format!("no session {}", session.0))
                        .await?
                }
            },

            ClientMsg::Search {
                sessions,
                pattern,
                regex,
                case_insensitive,
                whole_word,
                context_lines,
                max_hits,
            } => {
                // Awaited, because a search is a request and its answer belongs
                // to this connection in order. It does not stall the daemon:
                // `search::answer` moves the sweep onto a blocking thread, so
                // the PTY coalescers and every other client's output keep
                // running on the runtime while this one waits.
                let query = search::query_from_wire(
                    &pattern,
                    regex,
                    case_insensitive,
                    whole_word,
                    context_lines,
                    max_hits,
                );
                let answer = search::answer(Arc::clone(&self.hub.manager), sessions, query).await;
                self.send(&answer).await?;
            }

            ClientMsg::WatchCollisions { enabled } => {
                // Off the runtime. Turning detection on walks every session's
                // tree and spends one `inotify_add_watch` per directory; on a
                // four-thousand-directory checkout that is milliseconds of
                // syscalls, and the PTY coalescers and every other client's
                // output are tasks on this same runtime.
                //
                // The service publishes the resulting report to every window,
                // because the subscription is daemon-wide. It is also returned
                // here so the connection that asked gets its answer in order
                // and never has to tell a reply from a broadcast.
                let hub = Arc::clone(&self.hub);
                let publisher = self.hub.publisher();
                let answer = tokio::task::spawn_blocking(move || {
                    let live = crate::overlap::live_sessions(&hub.manager);
                    hub.overlap
                        .set_watching(enabled, &live, &publisher, now_ms())
                })
                .await
                .context("running the collision watcher control")?;
                self.send(&answer).await?;
            }

            ClientMsg::Collisions => {
                // One read lock and a projection, so this runs inline. A
                // window that has just connected renders the contested set
                // immediately instead of waiting for a change that, in a quiet
                // repository, never comes.
                let report = self.hub.overlap.report(now_ms());
                self.send(&report).await?;
            }
        }
        Ok(Flow::Continue)
    }

    /// Send the full project and session lists.
    ///
    /// Only in answer to an explicit List, and also the recovery path for a client
    /// that lapped the event bus. Watching is ensured here too, so a manager that
    /// already had sessions in it before any client connected still produces
    /// registry events.
    async fn send_snapshots(&mut self) -> anyhow::Result<()> {
        let projects = self.hub.projects_now();
        let sessions = self.hub.manager.list();
        for info in &sessions {
            self.hub.watch(info.id);
        }
        self.send(&ServerMsg::Projects { projects }).await?;
        self.send(&ServerMsg::Sessions { sessions }).await
    }

    async fn send(&self, msg: &ServerMsg) -> anyhow::Result<()> {
        send_json(&self.out, msg).await
    }

    async fn error(&self, session: Option<SessionId>, message: String) -> anyhow::Result<()> {
        tracing::debug!(?session, %message, "reporting a protocol error");
        self.send(&ServerMsg::error(session, message)).await
    }

    /// Stop drawing `session`: kill this connection's pump and release the size
    /// constraint it was placing on the PTY.
    ///
    /// Both halves matter. Leaving the pump alive keeps output flowing to a
    /// pane nobody is drawing, and leaving the geometry registered pins the
    /// session to the layout of a window that has moved on, so a second window
    /// stays letterboxed inside a size no one is using.
    fn detach(&mut self, session: SessionId) {
        if let Some(handle) = self.attached.remove(&session) {
            handle.abort();
        }
        self.hub.manager.detach(session, self.viewer);
    }

    /// Begin forwarding the daemon-wide registry bus to this client.
    fn start_event_forwarding(&mut self) {
        if self.events.is_some() {
            return;
        }
        let handle = tokio::spawn(pump_events(
            Arc::clone(&self.hub),
            self.hub.subscribe(),
            self.out.clone(),
        ));
        self.events = Some(handle);
    }
}

impl Drop for Conn {
    /// A gone client must not leave pumps running: they would hold broadcast
    /// receivers open, which makes their sessions look attended forever and keeps
    /// output flowing into a queue nobody drains.
    ///
    /// It must not keep constraining geometry either. A window that crashes
    /// while showing an 80-column session would otherwise hold every other
    /// window at 80 columns for as long as the daemon runs.
    fn drop(&mut self) {
        for (session, handle) in &self.attached {
            handle.abort();
            self.hub.manager.detach(*session, self.viewer);
        }
        if let Some(handle) = &self.events {
            handle.abort();
        }
    }
}

/// Forward registry events to one client for the life of its connection.
///
/// Every connected client gets every registry event, which is what makes a second
/// window, a bare terminal, or a relaunched GUI show sessions it did not start.
pub async fn pump_events(
    hub: Arc<Hub>,
    mut rx: broadcast::Receiver<crate::hub::Event>,
    out: mpsc::Sender<Message>,
) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                // Already JSON: the hub serialized it once for every window.
                if out.send(Message::Text(event.to_string())).await.is_err() {
                    return;
                }
            }
            // Missing registry deltas means this client's picture of the session
            // list is wrong, and unlike output there is no offset to resume from.
            // A full snapshot is the only honest repair.
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!(dropped = n, "client lapped the registry bus; resyncing");
                let projects = hub.projects_now();
                let sessions = hub.manager.list();
                if send_json(&out, &ServerMsg::Projects { projects })
                    .await
                    .is_err()
                {
                    return;
                }
                if send_json(&out, &ServerMsg::Sessions { sessions })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Stream one session's live output to the client as binary frames.
///
/// Lag is reported, never hidden. When the client falls far enough behind that
/// the broadcast channel laps, the dropped bytes are gone from the live stream,
/// and the only honest thing to do is name the resume offset so the client can
/// backfill from scrollback. Silently splicing would leave its viewport corrupted
/// with no way to notice.
pub async fn pump_output(
    session: SessionId,
    mut rx: broadcast::Receiver<OutputChunk>,
    out: mpsc::Sender<Message>,
) {
    let mut dropped = 0u64;
    let mut delivered_through = 0u64;
    loop {
        match rx.recv().await {
            Ok(chunk) => {
                if dropped > 0 {
                    let notice = gap_notice(session, dropped, chunk.seq);
                    dropped = 0;
                    if send_json(&out, &notice).await.is_err() {
                        return;
                    }
                }
                delivered_through = chunk.seq + chunk.data.len() as u64;
                let frame = Message::binary(encode_output(session, chunk.seq, &chunk.data));
                if out.send(frame).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => dropped += n,
            Err(broadcast::error::RecvError::Closed) => {
                // The session ended while a gap was outstanding. Report it
                // anyway, or the client would keep a hole it never learns about.
                if dropped > 0 {
                    let _ = send_json(&out, &gap_notice(session, dropped, delivered_through)).await;
                }
                return;
            }
        }
    }
}

/// The message that tells a client it lost live output and where to resume.
pub fn gap_notice(session: SessionId, dropped: u64, resume_seq: u64) -> ServerMsg {
    ServerMsg::error(
        Some(session),
        format!(
            "output gap: {dropped} chunk(s) dropped; re-request scrollback before seq {resume_seq}"
        ),
    )
}

/// Queue one control-plane message, waiting no longer than [`SEND_STALL`].
///
/// The wait is bounded because the queue is. Backpressure is the design — a
/// client that stops reading must slow the daemon down rather than grow its
/// heap — but an unbounded wait on a bounded queue is not backpressure, it is
/// a parked task holding a viewer registration and every attachment behind it.
/// A client that has taken nothing for half a minute has stopped rendering,
/// and the connection is ended so its sessions stop being held to its
/// geometry. The sessions themselves keep running.
async fn send_json(out: &mpsc::Sender<Message>, msg: &ServerMsg) -> anyhow::Result<()> {
    let text = serde_json::to_string(msg).context("serializing a server message")?;
    match tokio::time::timeout(SEND_STALL, out.send(Message::Text(text))).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(anyhow!("client connection is gone")),
        Err(_) => Err(anyhow!(
            "client stopped reading: it took no frame for {SEND_STALL:?}. Reconnect the \
             window; its sessions keep running."
        )),
    }
}

/// Owns the socket's write half so every producer can queue without a lock.
async fn write_loop(
    mut sink: SplitSink<WebSocketStream<TcpStream>, Message>,
    mut rx: mpsc::Receiver<Message>,
) {
    while let Some(msg) = rx.recv().await {
        if sink.send(msg).await.is_err() {
            break;
        }
    }
    let _ = sink.close().await;
}
