//! Backpressure: what a client is told when it cannot keep up with live output.
//!
//! Lag is driven by the broadcast channel's own state rather than by timing, so
//! these tests are deterministic: overflow the channel before the pump starts and
//! the very first receive reports the loss.

use std::sync::Arc;
use std::time::Duration;

use vitrum_core::OutputChunk;
#[cfg(unix)]
use vitrum_core::SessionManager;
use vitrum_proto::{ServerMsg, SessionId, decode_output};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;

use crate::{gap_notice, pump_output};

const SESSION: SessionId = SessionId(3);

/// Collect the messages a pump produced, bounded by a deadline.
async fn drain(rx: &mut mpsc::Receiver<Message>, want: usize) -> Vec<Message> {
    let mut got = Vec::new();
    while got.len() < want {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(m)) => got.push(m),
            Ok(None) => break,
            Err(_) => panic!("timed out with {} of {want} messages", got.len()),
        }
    }
    got
}

fn as_ctl(msg: &Message) -> ServerMsg {
    match msg {
        Message::Text(t) => serde_json::from_str(t).expect("control frames must parse"),
        other => panic!("expected a control frame, got {other:?}"),
    }
}

fn chunk(seq: u64, data: &[u8]) -> OutputChunk {
    OutputChunk {
        seq,
        data: Arc::from(data),
    }
}

/// A lagging client must be told about the gap and where to resume.
///
/// Silently splicing the stream is the failure this guards: the client would paint
/// the bytes after the hole at the offset before it, corrupting the viewport with
/// no way to detect it. The notice must arrive BEFORE the frame that follows the
/// gap, so the client can discard rather than misplace it.
#[tokio::test]
async fn a_lagging_client_is_told_the_gap_before_the_next_frame() {
    let (tx, rx) = broadcast::channel::<OutputChunk>(4);
    // Overflow the channel while the pump is not yet consuming.
    for i in 0..7u64 {
        tx.send(chunk(i * 10, b"0123456789"))
            .expect("a receiver exists");
    }
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let pump = tokio::spawn(pump_output(SESSION, rx, out_tx));

    let msgs = drain(&mut out_rx, 5).await;
    match as_ctl(&msgs[0]) {
        ServerMsg::Error { session, message, .. } => {
            assert_eq!(session, Some(SESSION));
            assert!(
                message.starts_with("output gap:"),
                "a client matches on this prefix: {message}"
            );
            assert!(
                message.contains('3') && message.contains("30"),
                "must name the 3 lost chunks and the resume offset 30: {message}"
            );
        }
        other => panic!("expected a gap error first, got {other:?}"),
    }

    // Everything the channel still held must follow, in order and intact.
    let mut at = 30u64;
    for msg in &msgs[1..] {
        match msg {
            Message::Binary(bytes) => {
                let (session, seq, payload) = decode_output(bytes).expect("decodable");
                assert_eq!(session, SESSION);
                assert_eq!(seq, at, "retained frames must keep their real offsets");
                assert_eq!(payload, b"0123456789");
                at += payload.len() as u64;
            }
            other => panic!("expected a data frame, got {other:?}"),
        }
    }
    assert_eq!(at, 70, "all four retained chunks were delivered");
    pump.abort();
}

/// A client that keeps up must never see a gap notice.
///
/// A false gap would send the client off to re-request history it already has,
/// which at twenty agents turns into a stampede of pointless work.
#[tokio::test]
async fn a_client_that_keeps_up_sees_no_gap() {
    let (tx, rx) = broadcast::channel::<OutputChunk>(16);
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let pump = tokio::spawn(pump_output(SESSION, rx, out_tx));

    for i in 0..8u64 {
        tx.send(chunk(i * 4, b"abcd")).expect("a receiver exists");
    }
    let msgs = drain(&mut out_rx, 8).await;
    assert_eq!(msgs.len(), 8);
    for (i, msg) in msgs.iter().enumerate() {
        match msg {
            Message::Binary(bytes) => {
                let (_, seq, payload) = decode_output(bytes).expect("decodable");
                assert_eq!(seq, i as u64 * 4);
                assert_eq!(payload, b"abcd");
            }
            other => panic!("expected only data frames, got {other:?}"),
        }
    }
    pump.abort();
}

/// A gap outstanding when the session ends must still be reported.
///
/// Otherwise the last lost bytes of a session are never accounted for: the client
/// would keep a hole in the final output, which is where the error message
/// explaining an exit usually lives.
#[tokio::test]
async fn a_gap_at_the_end_of_the_stream_is_still_reported() {
    let (tx, rx) = broadcast::channel::<OutputChunk>(2);
    tx.send(chunk(0, b"aa")).expect("a receiver exists");
    tx.send(chunk(2, b"bb")).expect("a receiver exists");
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let pump = tokio::spawn(pump_output(SESSION, rx, out_tx));

    // Deliver what fits, then lap the channel and close it.
    let delivered = drain(&mut out_rx, 2).await;
    assert_eq!(delivered.len(), 2);
    for i in 0..4u64 {
        tx.send(chunk(4 + i * 2, b"cc")).expect("a receiver exists");
    }
    drop(tx);

    // The pump reports the survivors, then the closure with the outstanding gap.
    let mut saw_gap = None;
    while let Ok(Some(msg)) = tokio::time::timeout(Duration::from_secs(5), out_rx.recv()).await {
        if let Message::Text(_) = msg {
            saw_gap = Some(as_ctl(&msg));
        }
    }
    match saw_gap {
        Some(ServerMsg::Error { message, .. }) => {
            assert!(
                message.starts_with("output gap:"),
                "unexpected wording: {message}"
            );
        }
        other => panic!("the end-of-stream gap was not reported: {other:?}"),
    }
    pump.abort();
}

/// The pump must exit when the client's queue is gone.
///
/// A pump that outlived its connection would hold a broadcast receiver open
/// forever, making the session look permanently attended and leaking a task per
/// disconnect.
#[tokio::test]
async fn the_pump_exits_when_the_client_queue_closes() {
    let (tx, rx) = broadcast::channel::<OutputChunk>(4);
    let (out_tx, out_rx) = mpsc::channel::<Message>(1);
    drop(out_rx);
    let pump = tokio::spawn(pump_output(SESSION, rx, out_tx));
    tx.send(chunk(0, b"x")).expect("a receiver exists");
    tokio::time::timeout(Duration::from_secs(5), pump)
        .await
        .expect("the pump must notice a gone client")
        .expect("the pump must not panic");
}

/// The pump must exit when the session's channel closes, which is how an ended
/// session releases the task that was streaming it.
#[tokio::test]
async fn the_pump_exits_when_the_session_channel_closes() {
    let (tx, rx) = broadcast::channel::<OutputChunk>(4);
    let (out_tx, _out_rx) = mpsc::channel::<Message>(4);
    let pump = tokio::spawn(pump_output(SESSION, rx, out_tx));
    drop(tx);
    tokio::time::timeout(Duration::from_secs(5), pump)
        .await
        .expect("the pump must notice the session ending")
        .expect("the pump must not panic");
}

/// The gap notice must name both the loss and the resume offset, in the exact
/// shape a client matches on.
#[test]
fn the_gap_notice_names_the_loss_and_the_resume_offset() {
    match gap_notice(SessionId(9), 12, 4_294_967_400) {
        ServerMsg::Error { session, message, .. } => {
            assert_eq!(session, Some(SessionId(9)));
            assert_eq!(
                message,
                "output gap: 12 chunk(s) dropped; re-request scrollback before seq 4294967400"
            );
        }
        other => panic!("a gap must be an error, got {other:?}"),
    }
}

/// A gap notice must survive the wire, since it is the only way a client learns it
/// has a hole.
#[test]
fn the_gap_notice_round_trips_as_json() {
    let notice = gap_notice(SessionId(2), 1, 64);
    let text = serde_json::to_string(&notice).expect("serializes");
    assert_eq!(
        serde_json::from_str::<ServerMsg>(&text).expect("deserializes"),
        notice
    );
}

/// A real session's output must reach a real socket without any gap.
///
/// The unit tests above force lag deliberately; this one guards the opposite
/// property, that the normal path never reports one. A pump that mishandled the
/// non-lagging case would break every session while looking fine under lag.
#[cfg(not(windows))]
#[tokio::test]
async fn a_real_session_streams_without_reporting_a_gap() {
    let mgr = Arc::new(SessionManager::new(64 * 1024));
    let (command, args) =
        crate::tests::client::shell("i=0; while [ $i -lt 300 ]; do printf q; i=$((i+1)); done");
    let id = mgr
        .spawn(vitrum_core::SessionSpec {
            project_id: vitrum_proto::ProjectId(1),
            cwd: std::env::temp_dir(),
            command,
            args,
            env: Vec::new(),
            cols: 80,
            rows: 24,
            title: None,
        })
        .expect("spawn");
    let rx = mgr.attach(id, mgr.new_viewer(), 80, 24).expect("attach");
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
    let pump = tokio::spawn(pump_output(id, rx, out_tx));

    let mut bytes = Vec::new();
    while bytes.len() < 300 {
        match tokio::time::timeout(Duration::from_secs(5), out_rx.recv()).await {
            Ok(Some(Message::Binary(frame))) => {
                let (_, seq, payload) = decode_output(&frame).expect("decodable");
                assert_eq!(seq as usize, bytes.len(), "offsets must stay contiguous");
                bytes.extend_from_slice(payload);
            }
            Ok(Some(other)) => panic!("unexpected frame during a healthy stream: {other:?}"),
            Ok(None) => break,
            Err(_) => panic!("timed out with {} of 300 bytes", bytes.len()),
        }
    }
    assert_eq!(bytes, vec![b'q'; 300]);
    pump.abort();
}
