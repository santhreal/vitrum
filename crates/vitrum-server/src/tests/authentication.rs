//! Who is allowed to talk to the daemon at all.
//!
//! WHY: this daemon creates sessions running any command the caller names and
//! streams every hosted agent's transcript back. Reaching it is therefore
//! equivalent to running code as the user who started it, and a loopback
//! listener is not a boundary — every other account on the machine can connect
//! to it, and so can any web page the operator visits, because a browser opens
//! a cross-origin WebSocket with no preflight and no same-origin check.
//!
//! Two layers answer that and both are asserted here: the `Origin` refusal at
//! the HTTP upgrade, which closes the browser case, and the shared token in
//! `Hello`, which closes the other-local-user case that no header check can.
//!
//! What these do NOT cover: whether the token file's mode is right, which is
//! `vitrum-proto`'s to prove because it owns the file, and whether the client
//! reads it, which is the app's.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;
use vitrum_proto::{ClientMsg, PROTOCOL_VERSION, ProjectId, ServerMsg};

use crate::tests::client::{DEADLINE, Harness, TEST_TOKEN};

/// A handshake carrying `Origin` is refused with 403 before any message.
///
/// This is the whole browser-borne attack. A page the operator visits can
/// script `new WebSocket("ws://127.0.0.1:7737")` and the browser will complete
/// the upgrade with no cross-origin check of any kind; the only thing that
/// distinguishes it from the real client is the header it is required to send.
#[tokio::test]
async fn a_handshake_carrying_origin_is_refused() {
    let h = Harness::start(4096).await;
    let mut request = format!("ws://127.0.0.1:{}", h.port)
        .into_client_request()
        .expect("a valid request");
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());

    let outcome = tokio::time::timeout(
        DEADLINE,
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("the daemon must answer rather than hang");

    match outcome {
        Ok(_) => panic!("a cross-origin handshake was accepted"),
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(
                response.status(),
                tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN,
                "a cross-origin handshake must be refused with 403"
            );
            let body = response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            assert!(
                body.contains("cross-origin"),
                "the refusal must say why, got {body:?}"
            );
        }
        Err(other) => panic!("expected an HTTP refusal, got {other}"),
    }
    assert!(
        h.manager.list().is_empty(),
        "a refused handshake must not have created anything"
    );
}

/// A handshake with no `Origin` still connects.
///
/// The negative control for the test above: a check that refused everything
/// would pass it and break every real client.
#[tokio::test]
async fn a_native_handshake_without_origin_still_connects() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.hello().await;
    assert!(
        c.seen.has(|m| matches!(m, ServerMsg::Welcome { .. })),
        "a native client must still be welcomed"
    );
}

/// A hello with the wrong token is refused and the socket closes.
#[tokio::test]
async fn a_wrong_token_is_refused_and_the_socket_closes() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send(ClientMsg::Hello {
        protocol: PROTOCOL_VERSION,
        // Same shape, same length, one character different: a comparison that
        // only checked the shape would accept this.
        token: format!("{}f", &TEST_TOKEN[1..]),
    })
    .await;
    c.until("the refusal", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].starts_with("authentication failed"),
        "unhelpful refusal: {:?}",
        c.seen.errors()[0]
    );
    c.until_closed().await;
    assert!(
        !c.seen.has(|m| matches!(m, ServerMsg::Welcome { .. })),
        "a refused client must not have been welcomed"
    );
}

/// A hello with an empty token is refused exactly as a wrong one is.
///
/// This is what a client that has not read the file sends, and it is the shape
/// a tolerant server would let through.
#[tokio::test]
async fn an_empty_token_is_refused() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send(ClientMsg::Hello {
        protocol: PROTOCOL_VERSION,
        token: String::new(),
    })
    .await;
    c.until("the refusal", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].starts_with("authentication failed"),
        "unhelpful refusal: {:?}",
        c.seen.errors()[0]
    );
    c.until_closed().await;
}

/// A hello with the token field absent entirely is refused.
///
/// Sent as raw JSON, because the typed enum cannot express it. This is exactly
/// what a protocol-2 client emits, and the case where a field added as
/// `Option<String>` would have kept the hole open.
#[tokio::test]
async fn a_hello_with_no_token_field_is_refused() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send_raw(tokio_tungstenite::tungstenite::Message::Text(format!(
        r#"{{"t":"hello","protocol":{PROTOCOL_VERSION}}}"#
    )))
    .await;
    c.until("the refusal", |s| !s.errors().is_empty()).await;
    assert!(
        !c.seen.has(|m| matches!(m, ServerMsg::Welcome { .. })),
        "a hello with no token must not be welcomed"
    );
}

/// A client refused for a bad token cannot create a session.
///
/// The refusal has to be a boundary rather than an opinion: a daemon that kept
/// serving after saying no would let an unauthenticated peer spawn processes,
/// which is the entire vulnerability.
#[tokio::test]
async fn a_client_refused_for_its_token_cannot_create_sessions() {
    let h = Harness::start(4096).await;
    let mut c = h.client().await;
    c.send(ClientMsg::Hello {
        protocol: PROTOCOL_VERSION,
        token: "f".repeat(64),
    })
    .await;
    c.until("the refusal", |s| !s.errors().is_empty()).await;
    c.send(ClientMsg::CreateSession {
        project_id: ProjectId(1),
        cwd: ".".to_string(),
        command: "true".to_string(),
        args: Vec::new(),
        cols: 80,
        rows: 24,
        title: None,
    })
    .await;
    c.until_closed().await;
    assert!(
        h.manager.list().is_empty(),
        "an unauthenticated peer created a session"
    );
}

/// A peer that opens a socket and never completes the upgrade is dropped.
///
/// Termination, not a value: without a deadline on the handshake one `nc` per
/// connection pins a task and a file descriptor for the life of the daemon,
/// and nothing in the message layer can see it because no message ever
/// arrives. The assertion is that the read ends; if the bound is ever removed
/// this test hangs and fails on its own timeout rather than passing quietly.
#[tokio::test]
async fn a_peer_that_never_upgrades_is_dropped() {
    let h = Harness::start(4096).await;
    let mut sock = TcpStream::connect(("127.0.0.1", h.port))
        .await
        .expect("connecting");
    // A well-formed request line and then nothing: the daemon is left waiting
    // for headers that never come.
    sock.write_all(b"GET / HTTP/1.1\r\n")
        .await
        .expect("writing a partial request");

    // The handshake deadline is 10s; this bound is generously past it and is
    // what turns a removed deadline into a failure instead of a hang.
    let mut scratch = [0u8; 64];
    let ended = tokio::time::timeout(Duration::from_secs(30), sock.read(&mut scratch)).await;
    match ended {
        Ok(Ok(0)) => {}
        Ok(Ok(n)) => {
            // A 4xx and a close is also the daemon giving up on it, which is
            // the same guarantee.
            let text = String::from_utf8_lossy(&scratch[..n]).into_owned();
            assert!(
                text.starts_with("HTTP/1.1 4") || text.starts_with("HTTP/1.1 5"),
                "expected the daemon to end the connection, got {text:?}"
            );
        }
        Ok(Err(_)) => {}
        Err(_) => panic!("the daemon held a half-open handshake open past its own deadline"),
    }
}

/// Stop the daemon, start it again, and a client that reads the token file as
/// it stands connects.
///
/// WHY: the runtime directory survives until logout, so on the second start
/// the first daemon's token is still on disk. This product tells the operator
/// to restart the daemon in its own protocol-skew message and after an update,
/// so a second start that refuses its own leftover file would turn routine
/// advice into a dead end. `vitrum-proto` proves the file is replaced; this
/// proves the connection that depends on it.
///
/// It also proves the other half, which is the part a file-level test cannot
/// see: the FIRST daemon's token stops working, so a client that cached one
/// across a restart is refused rather than silently attached to nothing.
#[tokio::test]
async fn a_daemon_can_be_restarted_and_the_new_token_connects() {
    let dir = std::env::temp_dir().join(format!("vitrum-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("token");

    let first = vitrum_proto::token::create_at(&path).expect("the first start writes a token");
    let h = Harness::start_with_token(4096, first.clone()).await;
    {
        let mut c = h.client().await;
        c.hello_with(&first).await;
        assert!(c.seen.has(|m| matches!(m, ServerMsg::Welcome { .. })));
    }
    h.stop().await;

    let second = vitrum_proto::token::create_at(&path)
        .expect("the second start writes a token over the first");
    assert_ne!(first, second, "a restart must mint a fresh token");
    assert_eq!(
        vitrum_proto::token::load_from(&path).expect("a client reads the file"),
        second,
        "the file must hold the running daemon's token"
    );

    let h = Harness::start_with_token(4096, second.clone()).await;
    let mut c = h.client().await;
    c.hello_with(&second).await;
    assert!(
        c.seen.has(|m| matches!(m, ServerMsg::Welcome { .. })),
        "a client reading the token file after a restart must connect"
    );

    let mut stale = h.client().await;
    stale
        .send(ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            token: first,
        })
        .await;
    stale.until("the refusal", |s| !s.errors().is_empty()).await;
    assert!(
        stale.seen.errors()[0].starts_with("authentication failed"),
        "a token from the previous daemon must be refused: {:?}",
        stale.seen.errors()[0]
    );
    stale.until_closed().await;

    let _ = std::fs::remove_dir_all(&dir);
}

/// A control message larger than the cap ends the connection instead of being
/// assembled.
///
/// WHY: tungstenite's default is 64 MiB per message, which is a peer choosing
/// how much of this daemon's heap to take, per connection, before a single
/// byte is parsed. The bound has to be observable, so this sends one frame
/// past it and asserts the daemon refuses rather than answers: an
/// implementation that raised the cap again would hand back a normal parse
/// error instead.
///
/// It also asserts termination. A daemon that neither answered nor closed
/// would hang this test on its own deadline rather than pass quietly.
#[tokio::test]
async fn a_control_message_past_the_cap_ends_the_connection() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    // 4 MiB is the cap; this is comfortably past it and is still cheap to
    // produce, because the daemon must reject it on the frame header rather
    // than after reading it all.
    let huge = format!(r#"{{"t":"rename","session":1,"title":"{}"}}"#, "a".repeat(5 * 1024 * 1024));
    // The send itself may fail once the daemon resets the socket, which is the
    // same outcome from this side.
    let _ = c
        .ws_send_allowing_failure(tokio_tungstenite::tungstenite::Message::Text(huge))
        .await;
    c.until_closed().await;
}
