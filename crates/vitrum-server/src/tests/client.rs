//! A real WebSocket client and a running daemon, for tests that exercise the
//! protocol end to end rather than calling handlers directly.
//!
//! Every wait is bounded by a deadline and driven by arriving frames. Nothing
//! sleeps as a synchronisation mechanism, so a passing test finishes as fast as
//! the PTY does and a broken one fails with the frames it did see.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use vitrum_core::SessionManager;
use vitrum_proto::{
    ClientMsg, PROTOCOL_VERSION, ProjectId, ServerMsg, SessionId, decode_output,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::serve;

/// Upper bound on any single wait. Only reached when something is broken.
///
/// `pub(crate)` so a sibling test module can bound a wait of its own on the
/// same number rather than inventing a second one that drifts from it.
pub(crate) const DEADLINE: Duration = Duration::from_secs(10);

/// Window for negative assertions. A bound on absence, not a wait for a result.
const QUIET: Duration = Duration::from_millis(200);

/// A daemon listening on an ephemeral port, plus the manager behind it.
pub(crate) struct Harness {
    pub(crate) port: u16,
    pub(crate) manager: Arc<SessionManager>,
}

impl Harness {
    pub(crate) async fn start(scrollback_bytes: usize) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("binding an ephemeral loopback port");
        let port = listener
            .local_addr()
            .expect("reading the bound address")
            .port();
        let manager = Arc::new(SessionManager::new(scrollback_bytes));
        tokio::spawn(serve(listener, Arc::clone(&manager)));
        Self { port, manager }
    }

    pub(crate) async fn client(&self) -> Client {
        Client::connect(self.port).await
    }

    /// A greeted client, which is what every test past the handshake needs.
    pub(crate) async fn greeted(&self) -> Client {
        let mut c = self.client().await;
        c.hello().await;
        c
    }
}

/// Everything a client has received so far, split by plane.
#[derive(Default)]
pub(crate) struct Seen {
    pub(crate) ctl: Vec<ServerMsg>,
    /// Reassembled data-plane payload per session.
    out: BTreeMap<SessionId, Vec<u8>>,
    /// Seq of the first data frame seen per session.
    first_seq: BTreeMap<SessionId, u64>,
    /// Seq the next data frame for a session must carry.
    next_seq: BTreeMap<SessionId, u64>,
    pub(crate) data_frames: usize,
}

impl Seen {
    pub(crate) fn bytes(&self, session: SessionId) -> &[u8] {
        self.out.get(&session).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn first_seq(&self, session: SessionId) -> Option<u64> {
        self.first_seq.get(&session).copied()
    }

    /// Whether this session's stream carries `needle` anywhere.
    ///
    /// A pseudoconsole surrounds a child's bytes with its own, mode sets and a
    /// screen clear before and an OSC 0 naming the shell after, so where the
    /// child's line sits in the stream is the host's business. What crosses the
    /// socket must be the child's bytes, whole and unaltered.
    pub(crate) fn carries(&self, session: SessionId, needle: &[u8]) -> bool {
        self.bytes(session)
            .windows(needle.len())
            .any(|w| w == needle)
    }

    /// The first control message matching `pred`, if any has arrived.
    pub(crate) fn find(&self, pred: impl Fn(&ServerMsg) -> bool) -> Option<&ServerMsg> {
        self.ctl.iter().find(|m| pred(m))
    }

    pub(crate) fn has(&self, pred: impl Fn(&ServerMsg) -> bool) -> bool {
        self.find(pred).is_some()
    }

    /// Every error message received, for asserting on wording.
    pub(crate) fn errors(&self) -> Vec<&str> {
        self.ctl
            .iter()
            .filter_map(|m| match m {
                ServerMsg::Error { message, .. } => Some(message.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Every session-created delta received, oldest first.
    pub(crate) fn created(&self) -> Vec<&vitrum_proto::SessionInfo> {
        self.ctl
            .iter()
            .filter_map(|m| match m {
                ServerMsg::SessionCreated(info) => Some(info),
                _ => None,
            })
            .collect()
    }

    /// How many exit deltas have arrived.
    pub(crate) fn exits(&self) -> usize {
        self.ctl
            .iter()
            .filter(|m| matches!(m, ServerMsg::Exited { .. }))
            .count()
    }

    /// Every session projection pushed so far, oldest first.
    pub(crate) fn updates(&self) -> Vec<&vitrum_proto::SessionInfo> {
        self.ctl
            .iter()
            .filter_map(|m| match m {
                ServerMsg::SessionUpdated(info) => Some(info),
                _ => None,
            })
            .collect()
    }

    /// The most recent session projection.
    pub(crate) fn last_update(&self) -> &vitrum_proto::SessionInfo {
        self
            .updates()
            .last()
            .expect("a session update must have arrived")
    }

    /// The most recent session list snapshot.
    pub(crate) fn sessions(&self) -> Option<&Vec<vitrum_proto::SessionInfo>> {
        self.ctl.iter().rev().find_map(|m| match m {
            ServerMsg::Sessions { sessions } => Some(sessions),
            _ => None,
        })
    }

    /// The most recent project list snapshot.
    pub(crate) fn projects(&self) -> Option<&Vec<vitrum_proto::ProjectInfo>> {
        self.ctl.iter().rev().find_map(|m| match m {
            ServerMsg::Projects { projects } => Some(projects),
            _ => None,
        })
    }

    fn accept(&mut self, frame: Message) {
        match frame {
            Message::Text(text) => {
                let msg: ServerMsg = serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("server sent unparseable control frame {text}: {e}"));
                self.ctl.push(msg);
            }
            Message::Binary(bytes) => {
                let (session, seq, payload) = decode_output(&bytes)
                    .unwrap_or_else(|e| panic!("server sent an undecodable data frame: {e}"));
                if let Some(expected) = self.next_seq.get(&session) {
                    assert_eq!(
                        seq, *expected,
                        "data-plane seq must be the cumulative byte offset with no gaps"
                    );
                }
                self.first_seq.entry(session).or_insert(seq);
                self.next_seq.insert(session, seq + payload.len() as u64);
                self.out.entry(session).or_default().extend_from_slice(payload);
                self.data_frames += 1;
            }
            Message::Close(_) => {}
            other => panic!("unexpected frame kind from the server: {other:?}"),
        }
    }
}

pub(crate) struct Client {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    pub(crate) seen: Seen,
    /// Set once the server closes the socket.
    pub(crate) closed: bool,
}

impl Client {
    pub(crate) async fn connect(port: u16) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("connecting to the daemon");
        Self {
            ws,
            seen: Seen::default(),
            closed: false,
        }
    }

    pub(crate) async fn send(&mut self, msg: ClientMsg) {
        let text = serde_json::to_string(&msg).expect("client messages must serialize");
        self.send_raw(Message::Text(text)).await;
    }

    pub(crate) async fn send_raw(&mut self, frame: Message) {
        self.ws.send(frame).await.expect("sending to the daemon");
    }

    /// Greet the server and assert the reply.
    pub(crate) async fn hello(&mut self) {
        self.send(ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
        })
        .await;
        self.until("welcome", |s| {
            s.has(|m| matches!(m, ServerMsg::Welcome { .. }))
        })
        .await;
    }

    /// Read frames until `stop` accepts what has arrived.
    ///
    /// Panics with the frames seen so far rather than hanging, which makes a
    /// protocol regression readable instead of a timeout in CI.
    pub(crate) async fn until(&mut self, what: &str, mut stop: impl FnMut(&Seen) -> bool) {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        while !stop(&self.seen) {
            let frame = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "timed out waiting for {what}; control: {:?}; data frames: {}",
                        self.seen.ctl, self.seen.data_frames
                    )
                });
            match frame {
                Some(Ok(f)) => self.seen.accept(f),
                Some(Err(e)) => panic!("websocket error while waiting for {what}: {e}"),
                None => {
                    self.closed = true;
                    assert!(
                        stop(&self.seen),
                        "server closed the socket before {what}; control: {:?}",
                        self.seen.ctl
                    );
                    return;
                }
            }
        }
    }

    /// Create a session and return the id from its `SessionCreated` delta.
    ///
    /// The id comes from the delta rather than a session list because the server
    /// deliberately does not broadcast a list on create: doing so would make
    /// startup traffic quadratic in session count.
    pub(crate) async fn create(&mut self, msg: ClientMsg) -> SessionId {
        let before = self.seen.created().len();
        self.send(msg).await;
        self.until("the session to be created", |s| {
            s.created().len() > before || !s.errors().is_empty()
        })
        .await;
        let created = self.seen.created();
        assert!(
            created.len() > before,
            "create failed: {:?}",
            self.seen.errors()
        );
        created[before].id
    }

    /// Attach and wait for the acknowledgement this attach produced.
    ///
    /// Counting updates first matters: a session that already exited has pushed a
    /// projection of its own, so waiting for "any SessionUpdated" would return
    /// immediately and let the attach's real acknowledgement land later, in the
    /// middle of whatever the test asserts next.
    pub(crate) async fn attach(&mut self, session: SessionId, cols: u16, rows: u16) {
        let before = self.seen.updates().len();
        self.send(ClientMsg::Attach {
            session,
            cols,
            rows,
        })
        .await;
        self.until("this attach's acknowledgement", |s| {
            s.updates().len() > before
        })
        .await;
    }

    /// Assert nothing further arrives within the quiet window.
    pub(crate) async fn quiet(&mut self) {
        match tokio::time::timeout(QUIET, self.ws.next()).await {
            Err(_) => {}
            Ok(None) => self.closed = true,
            Ok(Some(Ok(frame))) => panic!("expected silence, got {frame:?}"),
            Ok(Some(Err(e))) => panic!("expected silence, got a websocket error: {e}"),
        }
    }

    /// Wait for the server to close the socket, draining what it sends first.
    ///
    /// The frames queued before the close are what carry the reason, so they must
    /// be collected rather than discarded with the socket.
    pub(crate) async fn until_closed(&mut self) {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        loop {
            let frame = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "the server never closed the socket; control: {:?}",
                        self.seen.ctl
                    )
                });
            match frame {
                Some(Ok(f)) => self.seen.accept(f),
                // A close frame the peer sent, or the stream simply ending, both
                // mean the server is done with this client.
                Some(Err(_)) | None => break,
            }
        }
        self.closed = true;
    }
}

/// A create request running `script` in the platform shell from a real directory.
pub(crate) fn create(project: u64, script: &str) -> ClientMsg {
    create_in(project, std::env::temp_dir(), script)
}

pub(crate) fn create_in(project: u64, cwd: PathBuf, script: &str) -> ClientMsg {
    let (command, args) = shell(script);
    ClientMsg::CreateSession {
        project_id: ProjectId(project),
        cwd: cwd.to_string_lossy().into_owned(),
        command,
        args,
        cols: 80,
        rows: 24,
        title: None,
    }
}

/// Run `script` through the platform shell. Both shells agree on `echo` and
/// `exit`; anything shell-specific sits behind a `cfg` in the test using it.
pub(crate) fn shell(script: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), script.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        (
            "sh".to_string(),
            vec!["-c".to_string(), script.to_string()],
        )
    }
}
