//! Input and Resize: the two messages that must reach the kernel's PTY rather
//! than only the server's bookkeeping.

use vitrum_proto::{ClientMsg, ServerMsg, SessionId};

use crate::tests::client::{Harness, create};

/// Keystrokes must reach the child and its reply must come back.
///
/// The full loop over a socket: JSON in, PTY write, child read, PTY output,
/// binary frame out. The expected bytes include the line discipline's echo, which
/// is emitted when the byte is written and therefore always precedes the reply.
#[cfg(unix)]
#[tokio::test]
async fn input_reaches_the_child_and_the_reply_returns() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(1, "read -r line; printf 'got=%s' \"$line\""))
        .await;

    c.attach(id, 80, 24).await;

    c.send(ClientMsg::Input { session: id, data: b"typed\n".to_vec().into() })
    .await;
    c.until("the reply", |s| s.bytes(id).ends_with(b"got=typed"))
        .await;
    assert_eq!(c.seen.bytes(id), b"typed\r\ngot=typed");
}

/// Input must preserve order across separate messages, or a paste arrives
/// scrambled.
#[cfg(unix)]
#[tokio::test]
async fn input_order_is_preserved_across_messages() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(1, "while read -r l; do printf '[%s]' \"$l\"; done"))
        .await;

    c.attach(id, 80, 24).await;

    for i in 1..=4u8 {
        c.send(ClientMsg::Input { session: id, data: format!("{i}\n").into_bytes().into() })
        .await;
    }
    c.until("the last reply", |s| {
        s.bytes(id).windows(3).any(|w| w == b"[4]")
    })
    .await;

    // The echo of each typed line interleaves with the replies, so the ordering
    // assertion is on the replies themselves.
    let bytes = c.seen.bytes(id).to_vec();
    let mut replies = Vec::new();
    let mut rest = bytes.as_slice();
    while let Some(open) = rest.iter().position(|b| *b == b'[') {
        let close = open
            + 1
            + rest[open + 1..]
                .iter()
                .position(|b| *b == b']')
                .expect("every reply is closed");
        replies.extend_from_slice(&rest[open..=close]);
        rest = &rest[close + 1..];
    }
    assert_eq!(replies, b"[1][2][3][4]");
    h.manager.close(id).expect("close");
}

/// A resize message must reach the kernel, so the child sees the new size.
///
/// The read in the middle sequences the resize before the second query, so no
/// timing assumption is needed. A server that updated only its own projection
/// would pass a naive test and leave every full-screen agent drawing at the old
/// width.
#[cfg(unix)]
#[tokio::test]
async fn resize_reaches_the_child() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "stty size; read -r x; stty size")).await;

    c.attach(id, 80, 24).await;
    c.until("the first size", |s| s.bytes(id).ends_with(b"24 80\r\n"))
        .await;

    c.send(ClientMsg::Resize {
        session: id,
        cols: 140,
        rows: 55,
    })
    .await;
    c.send(ClientMsg::Input { session: id, data: b"\n".to_vec().into() })
    .await;
    c.until("the new size", |s| s.bytes(id).ends_with(b"55 140\r\n"))
        .await;
    assert_eq!(c.seen.bytes(id), b"24 80\r\n\r\n55 140\r\n");
}

/// A resize must be reflected in the projection too, since the client redraws
/// from it.
///
/// The client attaches first, because only an attached client constrains the
/// size: a window that is not drawing this session has no business reflowing
/// the child for whoever is.
#[cfg(not(windows))]
#[tokio::test]
async fn resize_updates_the_projection() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x")).await;
    c.attach(id, 80, 24).await;

    c.send(ClientMsg::Resize {
        session: id,
        cols: 200,
        rows: 60,
    })
    .await;
    c.send(ClientMsg::List).await;
    c.until("a snapshot with the new size", |s| {
        s.sessions()
            .is_some_and(|l| l.iter().any(|i| i.cols == 200))
    })
    .await;
    let sessions = c.seen.sessions().expect("sessions");
    assert_eq!((sessions[0].cols, sessions[0].rows), (200, 60));
    h.manager.close(id).expect("close");
}

/// A resize from a client that is not attached must be ignored, not applied.
///
/// This is the multi-window bug in its smallest form. Two windows show the same
/// session; one switches to another tab and detaches, then its layout code
/// fires one more resize. Honouring it would reflow the child to the geometry
/// of a window that is not even displaying it.
#[cfg(not(windows))]
#[tokio::test]
async fn a_resize_from_a_detached_client_is_ignored() {
    let h = Harness::start(4096).await;
    let mut drawing = h.greeted().await;
    let mut background = h.greeted().await;
    let id = drawing.create(create(1, "read -r x")).await;
    drawing.attach(id, 120, 40).await;

    background
        .send(ClientMsg::Resize {
            session: id,
            cols: 20,
            rows: 5,
        })
        .await;
    background.send(ClientMsg::List).await;
    background
        .until("a snapshot", |s| s.sessions().is_some())
        .await;
    let sessions = background.seen.sessions().expect("sessions");
    assert_eq!(
        (sessions[0].cols, sessions[0].rows),
        (120, 40),
        "only the attached window's geometry may reach the pty"
    );
    assert!(
        background.seen.errors().is_empty(),
        "an ignored resize is not an error: {:?}",
        background.seen.errors()
    );
    h.manager.close(id).expect("close");
}

/// Input for an unknown session must be an error naming the id, not a silent
/// discard: a client with a stale focus would otherwise lose keystrokes with no
/// feedback.
#[tokio::test]
async fn input_to_an_unknown_session_errors() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(ClientMsg::Input { session: SessionId(77), data: b"x".to_vec().into() })
    .await;
    c.until("the error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("77"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
}

/// Input to a session that has exited must be an error, since the keystrokes
/// cannot go anywhere and the user needs to know the pane is dead.
#[tokio::test]
async fn input_to_an_exited_session_errors() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "exit 0")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;

    c.send(ClientMsg::Input { session: id, data: b"x".to_vec().into() })
    .await;
    c.until("the error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("has exited"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
}

/// Resizing an unknown session must be an error rather than a panic, because a
/// window resize racing a close is ordinary.
#[tokio::test]
async fn resize_of_an_unknown_session_errors() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(ClientMsg::Resize {
        session: SessionId(88),
        cols: 80,
        rows: 24,
    })
    .await;
    c.until("the error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("88"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
}

/// Detaching a session that was never attached must be silently fine.
///
/// A client detaching on every tab switch will do this, and an error would train
/// it to ignore errors.
#[tokio::test]
async fn detaching_what_was_never_attached_is_harmless() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(ClientMsg::Detach {
        session: SessionId(1234),
    })
    .await;
    c.quiet().await;
    assert!(c.seen.errors().is_empty(), "{:?}", c.seen.errors());
}
