//! Two clients on one daemon: registry events must reach every connection while
//! output stays private to whoever attached.
//!
//! This is the reason the session server exists. You attach from a second window,
//! from a bare terminal, or from a GUI that just relaunched. A sidebar that
//! silently omits sessions started elsewhere defeats the entire architecture, and
//! it fails quietly, which is worse than failing loudly.

use vitrum_proto::{ClientMsg, ServerMsg};

use crate::tests::client::{Harness, create};

/// A session created by one client must appear on every other connected client,
/// with no further request from them.
///
/// The bug this locks out: scoping registry events to sessions a connection has
/// "mentioned" leaves a second window permanently stale for anything it did not
/// create itself, self-healing only on an explicit List that the client has no
/// reason to send because it believes it is subscribed.
#[tokio::test]
async fn a_session_created_elsewhere_appears_on_every_client() {
    let h = Harness::start(4096).await;

    // A does exactly what a client does on startup, and nothing more.
    let mut a = h.greeted().await;
    a.send(ClientMsg::List).await;
    a.until("a's initial snapshot", |s| s.sessions().is_some())
        .await;
    assert!(a.seen.sessions().expect("sessions").is_empty());

    let mut b = h.greeted().await;
    let mut msg = create(1, "read -r x");
    if let ClientMsg::CreateSession { title, .. } = &mut msg {
        *title = Some("made-by-B".to_string());
    }
    let id = b.create(msg).await;

    // A never asked again.
    a.until("b's session on a", |s| !s.created().is_empty()).await;
    let seen_by_a = a.seen.created();
    assert_eq!(seen_by_a.len(), 1, "exactly one create delta, not a snapshot");
    assert_eq!(seen_by_a[0].id, id);
    assert_eq!(seen_by_a[0].title, "made-by-B");
    let lists_on_a = a
        .seen
        .ctl
        .iter()
        .filter(|m| matches!(m, ServerMsg::Sessions { .. }))
        .count();
    assert_eq!(
        lists_on_a, 1,
        "only the snapshot A asked for; the create must arrive as a delta"
    );

    h.manager.close(id).expect("close");
}

/// A close performed by one client must reach the others as an exit delta.
///
/// Otherwise a second window keeps a row for a session that no longer exists, and
/// clicking it produces errors the user cannot explain.
#[cfg(not(windows))]
#[tokio::test]
async fn a_close_elsewhere_reaches_every_client() {
    let h = Harness::start(4096).await;
    let mut a = h.greeted().await;
    a.send(ClientMsg::List).await;
    a.until("a's initial snapshot", |s| s.sessions().is_some())
        .await;

    let mut b = h.greeted().await;
    let id = b.create(create(1, "read -r x")).await;
    a.until("the create delta", |s| !s.created().is_empty()).await;

    // A never touched this session: it did not create it and never attached.
    b.send(ClientMsg::Close { session: id }).await;
    a.until("the exit delta on a", |s| s.exits() == 1).await;
    match a.seen.find(|m| matches!(m, ServerMsg::Exited { .. })) {
        Some(ServerMsg::Exited { session, code }) => {
            assert_eq!(*session, id);
            assert_eq!(*code, None, "a killed child was signalled");
        }
        other => panic!("expected Exited, got {other:?}"),
    }
}

/// A natural exit must reach a client that never touched the session.
#[tokio::test]
async fn a_natural_exit_elsewhere_reaches_every_client() {
    let h = Harness::start(4096).await;
    let mut a = h.greeted().await;
    let mut b = h.greeted().await;
    let id = b.create(create(1, "exit 12")).await;

    a.until("the exit delta on a", |s| s.exits() == 1).await;
    match a.seen.find(|m| matches!(m, ServerMsg::Exited { .. })) {
        Some(ServerMsg::Exited { session, code }) => {
            assert_eq!(*session, id);
            assert_eq!(*code, Some(12));
        }
        other => panic!("expected Exited, got {other:?}"),
    }
}

/// Output frames must NOT reach a client that has not attached.
///
/// This is the regression that would hurt most if broadcasting registry events
/// were done carelessly: twenty agents' output sprayed at every window would undo
/// the entire memory and bandwidth argument for a thin client. A never attaches,
/// so A must receive registry deltas and zero binary frames while B receives the
/// bytes.
#[cfg(not(windows))]
#[tokio::test]
async fn output_frames_never_reach_a_client_that_did_not_attach() {
    let h = Harness::start(64 * 1024).await;
    let mut a = h.greeted().await;
    a.send(ClientMsg::List).await;
    a.until("a's initial snapshot", |s| s.sessions().is_some())
        .await;

    let mut b = h.greeted().await;
    let id = b.create(create(1, "read -r x; echo private-to-b")).await;
    b.attach(id, 80, 24).await;
    b.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    b.until("b's output", |s| {
        s.bytes(id).ends_with(b"private-to-b\r\n")
    })
    .await;

    // A must see the lifecycle but never the bytes.
    a.until("the exit delta on a", |s| s.exits() == 1).await;
    a.quiet().await;
    assert_eq!(
        a.seen.data_frames, 0,
        "a never attached and must receive no output frames"
    );
    assert!(
        !a.seen.created().is_empty(),
        "a must still see the registry deltas"
    );
    assert_eq!(b.seen.bytes(id), b"\r\nprivate-to-b\r\n");
}

/// Registry events must not be sent before the handshake completes.
///
/// A client that has not agreed on a protocol version cannot be sent typed state,
/// and sending it anyway would leak the session list to a peer the server is about
/// to refuse.
#[tokio::test]
async fn registry_events_wait_for_the_handshake() {
    let h = Harness::start(4096).await;
    // Connected but silent: no Hello.
    let mut silent = h.client().await;

    let mut b = h.greeted().await;
    let id = b.create(create(1, "exit 0")).await;
    b.until("the exit", |s| s.exits() == 1).await;

    silent.quiet().await;
    assert!(
        silent.seen.ctl.is_empty(),
        "an ungreeted client must receive nothing, got {:?}",
        silent.seen.ctl
    );

    // And once it greets, it can still catch up on request.
    silent.hello().await;
    silent.send(ClientMsg::List).await;
    silent
        .until("the snapshot", |s| s.sessions().is_some())
        .await;
    let sessions = silent.seen.sessions().expect("sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, id);
}

/// A project created by one client must be announced to the others, so the
/// sidebar's grouping is not missing a heading for a session it can see.
#[tokio::test]
async fn a_new_project_elsewhere_reaches_every_client() {
    let h = Harness::start(4096).await;
    let mut a = h.greeted().await;
    let mut b = h.greeted().await;
    b.create(create(99, "exit 0")).await;

    a.until("the project announcement on a", |s| {
        s.projects().is_some_and(|p| !p.is_empty())
    })
    .await;
    let projects = a.seen.projects().expect("projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, vitrum_proto::ProjectId(99));
}

/// One client disconnecting must not stop deltas reaching the others.
///
/// The bus is daemon-wide; a dropped connection removes one receiver and nothing
/// else. A GUI crash must not leave the remaining windows blind.
#[tokio::test]
async fn a_disconnect_does_not_break_delivery_to_others() {
    let h = Harness::start(4096).await;
    let mut a = h.greeted().await;
    {
        let mut doomed = h.greeted().await;
        doomed.create(create(1, "exit 0")).await;
    }
    a.until("the first exit", |s| s.exits() == 1).await;

    let mut c = h.greeted().await;
    let id = c.create(create(1, "exit 5")).await;
    a.until("the second exit", |s| s.exits() == 2).await;
    let codes: Vec<Option<i32>> = a
        .seen
        .ctl
        .iter()
        .filter_map(|m| match m {
            ServerMsg::Exited { session, code } if *session == id => Some(*code),
            _ => None,
        })
        .collect();
    assert_eq!(codes, vec![Some(5)]);
}
