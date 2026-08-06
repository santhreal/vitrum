//! The Hello exchange and how the server reacts to input it cannot use.

use vitrum_proto::{ClientMsg, PROTOCOL_VERSION, ServerMsg};
use tokio_tungstenite::tungstenite::Message;

use crate::tests::client::Harness;

/// Hello must be answered with Welcome carrying the version pair.
///
/// The client cannot decide whether to proceed without both numbers, and a
/// missing reply would leave a GUI showing a spinner with no diagnosis.
#[tokio::test]
async fn hello_is_answered_with_welcome() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send(ClientMsg::Hello {
        protocol: PROTOCOL_VERSION,
    })
    .await;
    c.until("welcome", |s| !s.ctl.is_empty()).await;
    match &c.seen.ctl[0] {
        ServerMsg::Welcome {
            protocol,
            server_version,
        } => {
            assert_eq!(*protocol, PROTOCOL_VERSION);
            assert_eq!(server_version, env!("CARGO_PKG_VERSION"));
        }
        other => panic!("expected Welcome, got {other:?}"),
    }
    assert_eq!(c.seen.ctl.len(), 1, "Welcome must not be padded with extras");
}

/// A mismatched protocol must be refused with an error naming both versions, and
/// the connection must close.
///
/// Papering over a version skew is worse than refusing: the two sides would
/// disagree about frame shapes and the failure would surface later as corrupted
/// terminal output with no obvious cause.
#[tokio::test]
async fn a_mismatched_protocol_is_refused_and_the_socket_closes() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send(ClientMsg::Hello { protocol: 9999 }).await;
    c.until("the refusal", |s| !s.ctl.is_empty()).await;
    let errors = c.seen.errors();
    assert_eq!(errors.len(), 1, "expected exactly one error: {errors:?}");
    assert!(
        errors[0].contains("9999") && errors[0].contains(&PROTOCOL_VERSION.to_string()),
        "the error must name both versions, got {:?}",
        errors[0]
    );
    c.until_closed().await;
}

/// A protocol mismatch must not leave a usable session channel open.
///
/// If the server kept serving a client it just refused, the refusal would be
/// advisory and a mismatched client could still spawn processes.
#[tokio::test]
async fn a_refused_client_cannot_create_sessions() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send(ClientMsg::Hello { protocol: 2 }).await;
    c.until("the refusal", |s| !s.ctl.is_empty()).await;
    c.until_closed().await;
    assert!(
        h.manager.list().is_empty(),
        "a refused client must not have created anything"
    );
}

/// Anything before Hello must be refused with a clear message.
///
/// Without an ordering rule the server would have to guess a protocol version to
/// answer with, which is exactly the skew the handshake exists to prevent.
#[tokio::test]
async fn messages_before_hello_are_refused() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send(ClientMsg::List).await;
    c.until("the refusal", |s| !s.ctl.is_empty()).await;
    assert_eq!(c.seen.errors(), vec!["expected hello before any other message"]);
    assert!(
        !c.seen.has(|m| matches!(m, ServerMsg::Sessions { .. })),
        "the list must not have been served"
    );
}

/// The ordering rule must not be a one-shot: a client can still greet after being
/// told off, rather than having to reconnect.
#[tokio::test]
async fn a_client_can_greet_after_being_refused_for_ordering() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send(ClientMsg::List).await;
    c.until("the refusal", |s| !s.ctl.is_empty()).await;
    c.hello().await;
    c.send(ClientMsg::List).await;
    c.until("the list", |s| {
        s.has(|m| matches!(m, ServerMsg::Sessions { .. }))
    })
    .await;
}

/// Malformed JSON must be reported and the connection must survive.
///
/// A connection can have twenty live agents behind it. Dropping it over one bad
/// frame would kill every one of them for a client-side bug.
#[tokio::test]
async fn malformed_json_is_reported_without_dropping_the_connection() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send_raw(Message::Text("{not json at all".to_string()))
        .await;
    c.until("the parse error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].starts_with("malformed control message"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );

    // Still usable.
    c.send(ClientMsg::List).await;
    c.until("the list", |s| {
        s.has(|m| matches!(m, ServerMsg::Sessions { .. }))
    })
    .await;
}

/// A known message shape with an unknown tag must be reported, not silently
/// ignored, or a client typo would look like a server that stopped responding.
#[tokio::test]
async fn an_unknown_message_tag_is_reported() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send_raw(Message::Text(r#"{"t":"teleport"}"#.to_string()))
        .await;
    c.until("the parse error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].starts_with("malformed control message"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
}

/// Binary frames from a client must be refused.
///
/// The data plane is server to client only. Accepting bytes there would create a
/// second, unversioned input path into the PTY.
#[tokio::test]
async fn binary_frames_from_a_client_are_refused() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send_raw(Message::Binary(vec![1, 2, 3])).await;
    c.until("the refusal", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("control messages must be JSON text"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
}

/// Two clients must be able to greet independently on the same daemon, which is
/// what makes a GUI restart while agents keep running possible.
#[tokio::test]
async fn two_clients_can_greet_independently() {
    let h = Harness::start(4096).await;
    let mut a = h.greeted().await;
    let mut b = h.greeted().await;
    a.send(ClientMsg::List).await;
    b.send(ClientMsg::List).await;
    a.until("a's list", |s| {
        s.has(|m| matches!(m, ServerMsg::Sessions { .. }))
    })
    .await;
    b.until("b's list", |s| {
        s.has(|m| matches!(m, ServerMsg::Sessions { .. }))
    })
    .await;
}
