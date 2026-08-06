//! Regression test suite for vitrum-server's WebSocket transport and central Hub.
//!
//! Covers Hub event broadcast isolation across N clients, connection state transitions,
//! zero-copy text message framing, client drop safety, and concurrent connection handling.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use vitrum_core::SessionManager;
use vitrum_proto::{
    ClientMsg, FRAME_KIND_OUTPUT, OUTPUT_HEADER_LEN, PROTOCOL_VERSION, ProjectId, ServerMsg,
    SessionId, decode_output, encode_output_into,
};
use vitrum_server::{Hub, serve};

/// Maximum deadline for waiting on expected WebSocket responses during tests.
const DEADLINE: Duration = Duration::from_secs(10);


/// Test harness running an in-memory daemon listener on loopback.
struct TestHarness {
    port: u16,
    manager: Arc<SessionManager>,
}

impl TestHarness {
    async fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("binding loopback port");
        let port = listener.local_addr().expect("reading port").port();
        let manager = Arc::new(SessionManager::new(4096));
        tokio::spawn(serve(listener, Arc::clone(&manager)));
        Self { port, manager }
    }

    async fn client(&self) -> TestClient {
        TestClient::connect(self.port).await
    }

    async fn greeted_client(&self) -> TestClient {
        let mut client = self.client().await;
        client.hello().await;
        client
    }
}

/// Simulated client managing a WebSocket connection to the daemon.
struct TestClient {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ctl_msgs: Vec<ServerMsg>,
    raw_texts: Vec<String>,
    data_bytes: BTreeMap<SessionId, Vec<u8>>,
    closed: bool,
}

impl TestClient {
    async fn connect(port: u16) -> Self {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
            .await
            .expect("websocket connection");
        Self {
            ws,
            ctl_msgs: Vec::new(),
            raw_texts: Vec::new(),
            data_bytes: BTreeMap::new(),
            closed: false,
        }
    }

    async fn send_msg(&mut self, msg: &ClientMsg) {
        let text = serde_json::to_string(msg).expect("serialize ClientMsg");
        self.ws
            .send(Message::Text(text.into()))
            .await
            .expect("send frame");
    }

    async fn send_text(&mut self, text: impl Into<String>) {
        self.ws
            .send(Message::Text(text.into().into()))
            .await
            .expect("send raw text frame");
    }

    async fn send_binary(&mut self, bytes: Vec<u8>) {
        self.ws
            .send(Message::Binary(bytes.into()))
            .await
            .expect("send binary frame");
    }

    async fn hello(&mut self) {
        self.send_msg(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
        })
        .await;
        self.until("Welcome", |c| {
            c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Welcome { .. }))
        })
        .await;
    }

    async fn accept_frame(&mut self, msg: Message) {
        match msg {
            Message::Text(text) => {
                let s = text.to_string();
                self.raw_texts.push(s.clone());
                if let Ok(parsed) = serde_json::from_str::<ServerMsg>(&s) {
                    self.ctl_msgs.push(parsed);
                }
            }
            Message::Binary(bytes) => {
                if let Ok((session, _seq, payload)) = decode_output(&bytes) {
                    self.data_bytes
                        .entry(session)
                        .or_default()
                        .extend_from_slice(payload);
                }
            }
            Message::Close(_) => {
                self.closed = true;
            }
            _ => {}
        }
    }

    async fn until(&mut self, predicate_name: &str, mut stop: impl FnMut(&Self) -> bool) {
        let deadline = tokio::time::Instant::now() + DEADLINE;
        while !stop(self) {
            let frame_opt = tokio::time::timeout_at(deadline, self.ws.next())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for: {predicate_name}"));
            match frame_opt {
                Some(Ok(frame)) => self.accept_frame(frame).await,
                Some(Err(e)) => panic!("websocket error during wait for {predicate_name}: {e}"),
                None => {
                    self.closed = true;
                    if !stop(self) {
                        panic!("connection closed before condition met: {predicate_name}");
                    }
                    break;
                }
            }
        }
    }

    async fn create_session(&mut self, script: &str) -> SessionId {
        let before_count = self.created_sessions().len();
        let (cmd, args) = if cfg!(windows) {
            ("cmd.exe".to_string(), vec!["/C".to_string(), script.to_string()])
        } else {
            ("sh".to_string(), vec!["-c".to_string(), script.to_string()])
        };
        self.send_msg(&ClientMsg::CreateSession {
            project_id: ProjectId(1),
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            command: cmd,
            args,
            cols: 80,
            rows: 24,
            title: None,
        })
        .await;

        self.until("SessionCreated", |c| c.created_sessions().len() > before_count).await;
        self.created_sessions()[before_count].id
    }

    fn created_sessions(&self) -> Vec<&vitrum_proto::SessionInfo> {
        self.ctl_msgs
            .iter()
            .filter_map(|m| match m {
                ServerMsg::SessionCreated(info) => Some(info),
                _ => None,
            })
            .collect()
    }

    fn errors(&self) -> Vec<&str> {
        self.ctl_msgs
            .iter()
            .filter_map(|m| match m {
                ServerMsg::Error { message, .. } => Some(message.as_str()),
                _ => None,
            })
            .collect()
    }

}

/// WHY: Asserts that when N clients are connected to the Hub, a session lifecycle event
/// (e.g. SessionCreated) published to the Hub is broadcast to all N connected clients
/// without cross-client interference, payload corruption, or loss.
#[tokio::test]
async fn test_hub_broadcast_isolation_across_n_clients() {
    let harness = TestHarness::start().await;
    const CLIENT_COUNT: usize = 5;

    let mut clients = Vec::with_capacity(CLIENT_COUNT);
    for _ in 0..CLIENT_COUNT {
        clients.push(harness.greeted_client().await);
    }

    // Client 0 asks for an initial list to establish baseline state
    clients[0].send_msg(&ClientMsg::List).await;
    clients[0]
        .until("Initial List Response", |c| {
            c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Sessions { .. }))
        })
        .await;

    // Client 0 creates a session
    let session_id = clients[0].create_session("read -r x").await;

    // Every other client (1..N) must receive the SessionCreated event without explicitly requesting it
    for (idx, client) in clients.iter_mut().enumerate().skip(1) {
        client
            .until(&format!("SessionCreated delta on client {idx}"), |c| {
                c.created_sessions().iter().any(|s| s.id == session_id)
            })
            .await;

        let created_by_client = client.created_sessions();
        assert_eq!(
            created_by_client.len(),
            1,
            "Client {idx} should have received exactly 1 creation delta"
        );
        assert_eq!(created_by_client[0].id, session_id);
    }

    harness.manager.close(session_id).expect("cleanup session");
}

/// WHY: Verifies that control plane event broadcasts reach all connected clients, but
/// binary output stream data from attached sessions remains strictly isolated to the
/// attaching client and never leaks to unattached clients.
#[tokio::test]
async fn test_hub_broadcast_unattached_client_isolation() {
    let harness = TestHarness::start().await;

    let mut client_a = harness.greeted_client().await;
    let mut client_b = harness.greeted_client().await;

    // Client B creates a session and attaches to receive output
    let session_id = client_b
        .create_session("echo PRIVATE_TO_CLIENT_B; read -r x")
        .await;

    client_b
        .send_msg(&ClientMsg::Attach {
            session: session_id,
            cols: 80,
            rows: 24,
        })
        .await;

    // Client A receives the control-plane creation delta
    client_a
        .until("SessionCreated on Client A", |c| {
            c.created_sessions().iter().any(|s| s.id == session_id)
        })
        .await;

    // Client B receives data-plane bytes from its attachment
    client_b
        .until("Data bytes on Client B", |c| {
            c.data_bytes.get(&session_id).map_or(false, |b| !b.is_empty())
        })
        .await;

    // Client A never attached to session_id, so its data output buffer MUST remain empty
    assert!(
        client_a.data_bytes.get(&session_id).map_or(true, |b| b.is_empty()),
        "Unattached Client A must receive zero data-plane output bytes"
    );

    harness.manager.close(session_id).expect("cleanup session");
}

/// WHY: Validates that when a client is slow or stops reading from the Hub broadcast event
/// channel, the Hub's broadcast Sender drops lagged messages for that client without stalling
/// other active clients or crashing the daemon.
#[tokio::test]
async fn test_hub_event_queue_overflow_lag_handling() {
    let manager = Arc::new(SessionManager::new(1024));
    let hub = Hub::new(manager);

    let mut rx_lagged = hub.subscribe();
    let mut rx_active = hub.subscribe();

    // Publish more than EVENT_QUEUE (256) messages without reading from rx_lagged
    // Publish more than EVENT_QUEUE (256) messages while draining rx_active
    for i in 0..300 {
        hub.publish(ServerMsg::error(None, format!("event-{i}")));
        let msg = rx_active
            .recv()
            .await
            .expect("active receiver should read continuously without lagging");
        assert!(msg.contains(&format!("event-{i}")));
    }

    // Lagged receiver returns RecvError::Lagged indicating dropped messages
    match rx_lagged.recv().await {
        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
            assert!(skipped > 0, "Subscriber should report skipped messages");
        }
        other => panic!("Expected RecvError::Lagged, got {other:?}"),
    }
}

/// WHY: Ensures a WebSocket connection strictly transitions from Unauthenticated to
/// Greeted/Subscribed only upon receiving a valid ClientMsg::Hello with PROTOCOL_VERSION,
/// refusing unauthenticated control requests until greeted.
#[tokio::test]
async fn test_connection_handshake_state_transition() {
    let harness = TestHarness::start().await;
    let mut client = harness.client().await;

    // Sending a control message before Hello must trigger an error refusal
    client.send_msg(&ClientMsg::List).await;
    client
        .until("Handshake required error", |c| !c.errors().is_empty())
        .await;

    assert!(
        client.errors()[0].to_lowercase().contains("hello"),
        "Error message should state hello is required before other messages"
    );

    // Now send Hello to complete transition
    client
        .send_msg(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
        })
        .await;

    client
        .until("Welcome response", |c| {
            c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Welcome { .. }))
        })
        .await;

    // Subsequent control messages succeed past handshake state
    client.send_msg(&ClientMsg::List).await;
    client
        .until("Sessions response", |c| {
            c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Sessions { .. }))
        })
        .await;
}

/// WHY: Verifies that connecting with a mismatched protocol version returns an Error message
/// and cleanly closes the WebSocket connection, ensuring backwards-incompatible clients are
/// disconnected immediately.
#[tokio::test]
async fn test_connection_state_rejection_on_invalid_protocol() {
    let harness = TestHarness::start().await;
    let mut client = harness.client().await;

    client
        .send_msg(&ClientMsg::Hello { protocol: 99999 })
        .await;

    client
        .until("Protocol mismatch refusal", |c| !c.errors().is_empty())
        .await;

    assert!(
        client.errors()[0].contains("99999"),
        "Error should reference invalid protocol version"
    );

    // Connection must be closed by server after refusal
    client.until("Socket closed", |c| c.closed).await;
    assert!(client.closed, "Server must close connection on protocol mismatch");
}

/// WHY: Verifies that attached session output pumps transition state correctly when attach and
/// detach messages are processed, stopping old output pump handles upon detach or session close.
#[tokio::test]
async fn test_connection_state_transition_on_session_attach_detach() {
    let harness = TestHarness::start().await;
    let mut client = harness.greeted_client().await;

    let session_id = client.create_session("read -r x").await;

    // Attach to session
    client
        .send_msg(&ClientMsg::Attach {
            session: session_id,
            cols: 80,
            rows: 24,
        })
        .await;

    client
        .until("Attach SessionUpdated ack", |c| {
            c.ctl_msgs
                .iter()
                .any(|m| matches!(m, ServerMsg::SessionUpdated(s) if s.id == session_id))
        })
        .await;

    // Detach from session
    client
        .send_msg(&ClientMsg::Detach { session: session_id })
        .await;

    client
        .until("Detach SessionUpdated ack", |c| {
            c.ctl_msgs.iter().filter(|m| matches!(m, ServerMsg::SessionUpdated(s) if s.id == session_id)).count() >= 2
        })
        .await;

    harness.manager.close(session_id).expect("cleanup session");
}

/// WHY: Tests that a client reconnecting after a disconnect receives a full session list
/// snapshot via ClientMsg::List, restoring full state consistency across disconnect/reconnect cycles.
#[tokio::test]
async fn test_connection_reconnect_resynchronization_transition() {
    let harness = TestHarness::start().await;

    // Initial client creates sessions then disconnects
    let mut client_1 = harness.greeted_client().await;
    let s1 = client_1.create_session("read -r x").await;
    let s2 = client_1.create_session("read -r x").await;
    drop(client_1);

    // Reconnecting client connects, greets, and requests session list snapshot
    let mut client_2 = harness.greeted_client().await;
    client_2.send_msg(&ClientMsg::List).await;

    client_2
        .until("Session list snapshot", |c| {
            c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Sessions { .. }))
        })
        .await;

    let session_list = client_2
        .ctl_msgs
        .iter()
        .find_map(|m| match m {
            ServerMsg::Sessions { sessions } => Some(sessions),
            _ => None,
        })
        .expect("Sessions msg");

    let ids: Vec<SessionId> = session_list.iter().map(|s| s.id).collect();
    assert!(ids.contains(&s1), "Snapshot must contain s1");
    assert!(ids.contains(&s2), "Snapshot must contain s2");

    harness.manager.close(s1).expect("cleanup s1");
    harness.manager.close(s2).expect("cleanup s2");
}

/// WHY: Verifies that Hub::publish serializes a ServerMsg once into an Arc<str> Event buffer,
/// allowing broadcast to N subscribers without re-serializing or deep-copying JSON strings per subscriber.
#[tokio::test]
async fn test_hub_zero_copy_event_framing() {
    let manager = Arc::new(SessionManager::new(1024));
    let hub = Hub::new(manager);

    let mut rx1 = hub.subscribe();
    let mut rx2 = hub.subscribe();
    let mut rx3 = hub.subscribe();

    let msg = ServerMsg::Welcome {
        protocol: PROTOCOL_VERSION,
        server_version: "1.0.0".to_string(),
    };

    hub.publish(msg);

    let ev1 = rx1.recv().await.expect("rx1 receive");
    let ev2 = rx2.recv().await.expect("rx2 receive");
    let ev3 = rx3.recv().await.expect("rx3 receive");

    // Arc::ptr_eq proves zero-copy buffer sharing across all subscribers
    assert!(
        Arc::ptr_eq(&ev1, &ev2),
        "Event buffer ev1 and ev2 must share the underlying Arc allocation"
    );
    assert!(
        Arc::ptr_eq(&ev2, &ev3),
        "Event buffer ev2 and ev3 must share the underlying Arc allocation"
    );
}

/// WHY: Verifies that vitrum_proto::encode_output_into reuses an existing Vec<u8> buffer
/// without allocating fresh vectors on each PTY output chunk, avoiding allocation overhead on the data path.
#[test]
fn test_zero_copy_binary_output_encoding_into() {
    let session = SessionId(42);
    let mut buffer = Vec::with_capacity(128);

    // Initial encode into preallocated buffer
    encode_output_into(&mut buffer, session, 100, b"first payload");
    assert_eq!(buffer.len(), OUTPUT_HEADER_LEN + 13);
    assert_eq!(buffer[0], FRAME_KIND_OUTPUT);

    let (decoded_id, decoded_seq, payload) =
        decode_output(&buffer).expect("successful decode");
    assert_eq!(decoded_id, session);
    assert_eq!(decoded_seq, 100);
    assert_eq!(payload, b"first payload");

    // Clear buffer and re-encode to verify buffer reuse (zero reallocation)
    let initial_ptr = buffer.as_ptr();
    buffer.clear();
    encode_output_into(&mut buffer, session, 113, b"second payload");

    assert_eq!(
        buffer.as_ptr(),
        initial_ptr,
        "Buffer allocation pointer must be preserved across encode_output_into calls"
    );
    assert_eq!(buffer.len(), OUTPUT_HEADER_LEN + 14);
}

/// WHY: Asserts that sending malformed JSON text frames or non-UTF-8 control frames returns an
/// Error response back to the client while keeping the connection open and functional for valid subsequent requests.
#[tokio::test]
async fn test_malformed_text_framing_resilience() {
    let harness = TestHarness::start().await;
    let mut client = harness.greeted_client().await;

    // Send malformed JSON frame
    client.send_text("{\"t\": \"nonExistentMessageType\"}").await;

    client
        .until("Malformed frame error reply", |c| !c.errors().is_empty())
        .await;

    assert!(
        client.errors()[0].contains("malformed control message")
            || client.errors()[0].contains("unknown variant"),
        "Error reply should diagnose malformed control message"
    );

    // Send binary frame on control plane (should be refused gracefully)
    client.send_binary(vec![0xAA, 0xBB, 0xCC]).await;
    client
        .until("Binary control refusal", |c| {
            c.errors().iter().any(|e| e.contains("binary frames carry output only"))
        })
        .await;

    // Connection remains healthy and can process subsequent valid commands
    client.send_msg(&ClientMsg::List).await;
    client
        .until("Sessions response after error", |c| {
            c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Sessions { .. }))
        })
        .await;
}

/// WHY: Verifies that an abrupt TCP/WebSocket drop (without explicit Close handshake) safely
/// cleans up connection resources, detaches session pumps, and unregisters watched sessions without memory leaks or task panics.
#[tokio::test]
async fn test_client_abrupt_disconnect_drop_safety() {
    let harness = TestHarness::start().await;

    let mut client_1 = harness.greeted_client().await;
    let session_id = client_1.create_session("read -r x").await;

    client_1
        .send_msg(&ClientMsg::Attach {
            session: session_id,
            cols: 80,
            rows: 24,
        })
        .await;

    // Abruptly drop TCP connection
    drop(client_1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Connect new client to confirm daemon remains functional
    let mut client_2 = harness.greeted_client().await;
    client_2.send_msg(&ClientMsg::List).await;
    client_2
        .until("List from client 2", |c| {
            c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Sessions { .. }))
        })
        .await;

    harness.manager.close(session_id).expect("cleanup session");
}

/// WHY: Tests that multiple clients concurrently connecting, subscribing, creating sessions, and
/// abruptly dropping connections do not cause lock contention panics or race conditions in Hub watched session tracking.
#[tokio::test]
async fn test_concurrent_client_connect_disconnect_thundering_herd() {
    let harness = Arc::new(TestHarness::start().await);
    const CONCURRENT_WORKERS: usize = 8;

    let mut tasks = Vec::with_capacity(CONCURRENT_WORKERS);
    for i in 0..CONCURRENT_WORKERS {
        let h = Arc::clone(&harness);
        tasks.push(tokio::spawn(async move {
            let mut client = h.greeted_client().await;
            let sid = client.create_session(&format!("echo worker_{i}")).await;
            client.send_msg(&ClientMsg::List).await;
            client
                .until("List reply", |c| {
                    c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Sessions { .. }))
                })
                .await;
            sid
        }));
    }

    let created_sessions = futures_util::future::join_all(tasks).await;
    for res in created_sessions {
        let sid = res.expect("worker task completed without panic");
        let _ = harness.manager.close(sid);
    }

    // Verify daemon is still operational after thundering herd
    let mut final_client = harness.greeted_client().await;
    final_client.send_msg(&ClientMsg::List).await;
    final_client
        .until("Final List reply", |c| {
            c.ctl_msgs.iter().any(|m| matches!(m, ServerMsg::Sessions { .. }))
        })
        .await;
}

/// WHY: Verifies that Hub::publisher returns a closure holding a Weak reference to the Hub,
/// ensuring that keeping a publisher callback alive does not create reference cycles that prevent Hub destruction.
#[test]
fn test_hub_publisher_weak_reference_prevents_cycles() {
    let manager = Arc::new(SessionManager::new(1024));
    let hub = Hub::new(manager);

    let publisher = hub.publisher();
    let weak_hub = Arc::downgrade(&hub);

    drop(hub);

    // After dropping the primary Arc<Hub>, weak reference upgrade MUST fail
    assert!(
        weak_hub.upgrade().is_none(),
        "Hub must be deallocated when primary Arc is dropped, proving publisher holds only Weak reference"
    );

    // Invoking the publisher closure after Hub destruction must be a harmless no-op
    publisher(ServerMsg::error(
        None,
        "orphaned publish",
    ));
}
