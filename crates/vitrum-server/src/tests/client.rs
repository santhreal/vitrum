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
///
/// The number turns a hang into a readable failure. It is not an assertion
/// about how fast a child starts, and every time it has fired it has measured
/// the machine rather than the code: 10.07s on a shared Windows VM, 30s on the
/// first session of a Windows on ARM run, 10.05s on a self-hosted Linux runner
/// with three other jobs on the same host. Each of those legs passed every
/// other test in the same binary.
///
/// So it is generous on both platforms, and longer on Windows, where a session
/// costs a `CreateProcess` and a pseudoconsole rather than a `fork`. Ninety
/// seconds is what `vitrum-vt`'s pty test already uses there for the same
/// reason. A wait that is really stuck still fails, inside a thirty minute leg.
#[cfg(not(windows))]
pub(crate) const DEADLINE: Duration = Duration::from_secs(30);
#[cfg(windows)]
pub(crate) const DEADLINE: Duration = Duration::from_secs(90);

/// Window for negative assertions. A bound on absence, not a wait for a result.
const QUIET: Duration = Duration::from_millis(200);

/// The token every harness daemon serves with.
///
/// A fixed value rather than a generated one: the suite has to be able to
/// present a WRONG token as well as the right one, and "wrong" is only
/// definable against something known. Generation itself is proved in
/// `vitrum-proto`, where the entropy source lives.
pub(crate) const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A daemon listening on an ephemeral port, plus the manager behind it.
pub(crate) struct Harness {
    pub(crate) port: u16,
    pub(crate) manager: Arc<SessionManager>,
    /// Aborting this stops the daemon, which is what a restart needs.
    serving: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Harness {
    pub(crate) async fn start(scrollback_bytes: usize) -> Self {
        Self::start_with_token(scrollback_bytes, TEST_TOKEN.to_string()).await
    }

    /// A daemon serving with a token the caller chose.
    ///
    /// The token is a parameter rather than a constant so a test can serve
    /// with one that a real `vitrum_proto::token::create_at` produced, which
    /// is the only way to exercise the restart sequence end to end.
    pub(crate) async fn start_with_token(scrollback_bytes: usize, token: String) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("binding an ephemeral loopback port");
        let port = listener
            .local_addr()
            .expect("reading the bound address")
            .port();
        let manager = Arc::new(SessionManager::new(scrollback_bytes));
        let serving = tokio::spawn(serve(listener, Arc::clone(&manager), token));
        Self {
            port,
            manager,
            serving,
        }
    }

    /// Stop the daemon and wait until its port is free again.
    ///
    /// Waiting matters: a restart test that rebinds before the kernel has
    /// released the listener is a flake, and one that does not wait is
    /// asserting against two daemons at once.
    pub(crate) async fn stop(self) {
        let port = self.port;
        self.serving.abort();
        let deadline = tokio::time::Instant::now() + DEADLINE;
        while TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the daemon on {port} was still accepting after it was stopped"
            );
            tokio::task::yield_now().await;
        }
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
        self.hello_with(TEST_TOKEN).await
    }

    /// Greet with a token the caller chose, and assert the reply.
    pub(crate) async fn hello_with(&mut self, token: &str) {
        self.send(ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            token: token.to_string(),
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

    /// Send a frame that the daemon may reject by resetting the socket.
    ///
    /// `send_raw` asserts the write succeeded, which is right for every test
    /// where the daemon is expected to answer. It is wrong for a frame the
    /// daemon is expected to refuse at the transport, where losing the race
    /// between our write and its reset is an ordinary outcome and not a
    /// failure.
    pub(crate) async fn ws_send_allowing_failure(
        &mut self,
        frame: Message,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        self.ws.send(frame).await
    }

    // The helpers from here to the end of this block drive a live child
    // through its PTY, and the suites that do that — naming, observation and
    // the three seam suites — are compiled off on Windows, where the PTY
    // layer's burst behaviour is a known open defect. A helper with no caller
    // is a dead-code error under the workspace's deny-warnings build, so each
    // carries the same condition as the suites that call it rather than an
    // allow that would also hide a genuinely orphaned one.
    /// Round-trip a `List` so everything sent before it has been processed.
    ///
    /// The daemon handles one connection's messages in order, so a reply to a
    /// later message proves an earlier one has already been applied. That is
    /// what makes `Detach` — which acknowledges nothing — observable without a
    /// sleep: detach, barrier, and from then on any frame that arrives is a
    /// frame the detach failed to stop.
    #[cfg(not(windows))]
    pub(crate) async fn barrier(&mut self) {
        let before = self.seen.ctl.len();
        self.send(ClientMsg::List).await;
        self.until("the list that closes this barrier", |s| {
            s.ctl[before..]
                .iter()
                .any(|m| matches!(m, ServerMsg::Sessions { .. }))
        })
        .await;
    }

    /// Detach, and return only once the daemon has acted on it.
    #[cfg(not(windows))]
    pub(crate) async fn detach(&mut self, session: SessionId) {
        self.send(ClientMsg::Detach { session }).await;
        self.barrier().await;
    }

    /// Rename, and return once the projection carrying the new name arrives.
    #[cfg(not(windows))]
    pub(crate) async fn rename(&mut self, session: SessionId, title: &str) {
        let want = title.to_string();
        self.send(ClientMsg::Rename {
            session,
            title: want.clone(),
        })
        .await;
        self.until("the renamed projection", move |s| {
            s.updates()
                .iter()
                .any(|i| i.id == session && i.title == want)
        })
        .await;
    }

    /// Send `data` to the child's PTY.
    #[cfg(not(windows))]
    pub(crate) async fn input(&mut self, session: SessionId, data: &[u8]) {
        self.send(ClientMsg::Input {
            session,
            data: data.to_vec(),
        })
        .await;
    }

    /// The newest projection this client has for `session`, if any arrived.
    #[cfg(not(windows))]
    pub(crate) fn projection(&self, session: SessionId) -> Option<&vitrum_proto::SessionInfo> {
        self.seen
            .ctl
            .iter()
            .rev()
            .find_map(|m| match m {
                ServerMsg::SessionCreated(info) | ServerMsg::SessionUpdated(info)
                    if info.id == session =>
                {
                    Some(info)
                }
                _ => None,
            })
    }

    /// Wait until `session`'s newest projection satisfies `f`.
    #[cfg(not(windows))]
    pub(crate) async fn until_projection(
        &mut self,
        what: &str,
        session: SessionId,
        f: impl Fn(&vitrum_proto::SessionInfo) -> bool,
    ) -> vitrum_proto::SessionInfo {
        self.until(what, |s| {
            s.ctl
                .iter()
                .rev()
                .find_map(|m| match m {
                    ServerMsg::SessionCreated(info) | ServerMsg::SessionUpdated(info)
                        if info.id == session =>
                    {
                        Some(info)
                    }
                    _ => None,
                })
                .is_some_and(&f)
        })
        .await;
        self.projection(session)
            .expect("the loop above only exits once a projection matched")
            .clone()
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

/// A create request running `command` with `args` verbatim, no shell in front.
///
/// The agent rules key on the command's BASENAME, so a test about them has to
/// choose the program's name rather than inherit `sh`.
///
/// Compiled where the suites that use it are; see the note in `impl Client`.
#[cfg(not(windows))]
pub(crate) fn create_command(project: u64, command: &str, args: &[&str]) -> ClientMsg {
    ClientMsg::CreateSession {
        project_id: ProjectId(project),
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        command: command.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cols: 80,
        rows: 24,
        title: None,
    }
}

/// A directory holding a shell under some other program's name.
///
/// `AgentKind::of` keys on the command's basename and the daemon really
/// executes that command, so proving an agent rule across the wire needs a real
/// executable that really is called `codex`. A symlink to `/bin/sh` is one: the
/// rule sees `codex`, and the child is a shell that can emit the escape the
/// test is about. The technique is `vitrum-core`'s `agent_title.rs`; it is
/// repeated here rather than shared because that module is `#[cfg(test)]` in
/// another crate and has no exported form.
///
/// Unix only: there is no symlink-to-`cmd.exe` equivalent that keeps a usable
/// `-c` interface, and every escape-driven test in this suite is Unix only for
/// the reason `naming.rs` gives.
#[cfg(not(windows))]
pub(crate) struct FakeAgent {
    dir: PathBuf,
    command: String,
}

#[cfg(not(windows))]
impl FakeAgent {
    /// A shell installed under `name`, in a directory unique to this instance.
    ///
    /// Unique per instance rather than per name: several cases here run the
    /// same agent name at the same time under one test binary, and a shared
    /// directory makes them race for the same symlink.
    pub(crate) fn named(name: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "vitrum-seam-agent-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let command = dir.join(name);
        std::os::unix::fs::symlink("/bin/sh", &command).expect("symlink a shell under the name");
        FakeAgent {
            dir,
            command: command.to_string_lossy().into_owned(),
        }
    }

    /// A create request running `script` through this fake agent.
    pub(crate) fn create(&self, project: u64, script: &str) -> ClientMsg {
        create_command(project, &self.command, &["-c", script])
    }
}

#[cfg(not(windows))]
impl Drop for FakeAgent {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
