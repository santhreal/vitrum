//! Exit reporting, Close, and the guarantee that sessions outlive clients.

use vitrum_proto::{ClientMsg, ServerMsg, SessionId, SessionStatus};

use crate::tests::client::{DEADLINE, Harness, create};

/// A natural exit must be reported with its exact code, unprompted.
///
/// The client has no timer to notice an exit with, so an unreported exit leaves a
/// dead pane looking alive forever.
#[tokio::test]
async fn a_natural_exit_is_reported_with_its_code() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "exit 6")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;
    match c.seen.find(|m| matches!(m, ServerMsg::Exited { .. })) {
        Some(ServerMsg::Exited { session, code }) => {
            assert_eq!(*session, id);
            assert_eq!(*code, Some(6));
        }
        other => panic!("expected Exited, got {other:?}"),
    }
}

/// A clean exit must be reported as code 0, distinct from a signal's absent code.
#[tokio::test]
async fn a_clean_exit_is_reported_as_zero() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(create(1, "exit 0")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;
    match c.seen.find(|m| matches!(m, ServerMsg::Exited { .. })) {
        Some(ServerMsg::Exited { code, .. }) => assert_eq!(*code, Some(0)),
        other => panic!("expected Exited, got {other:?}"),
    }
}

/// The updated projection must arrive with the exit, carrying the terminal status.
///
/// The sidebar row has to change from running to exited without asking, or the
/// client would need a refresh timer, which is the cost this design refuses.
#[tokio::test]
async fn the_projection_is_updated_when_a_session_exits() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(create(1, "exit 9")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;
    let updated: Vec<&vitrum_proto::SessionInfo> = c
        .seen
        .ctl
        .iter()
        .filter_map(|m| match m {
            ServerMsg::SessionUpdated(info) => Some(info),
            _ => None,
        })
        .collect();
    assert!(
        updated
            .iter()
            .any(|i| i.status == SessionStatus::Exited { code: Some(9) }),
        "no update carried the terminal status: {updated:?}"
    );
}

/// An exited session must stay listed, so the user can read its last output and
/// its exit code instead of having the row vanish.
#[tokio::test]
async fn an_exited_session_stays_listed() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(create(1, "exit 1")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;
    c.send(ClientMsg::List).await;
    c.until("a fresh snapshot", |s| {
        s.sessions()
            .is_some_and(|l| l.iter().any(|i| !i.status.is_live()))
    })
    .await;
    let sessions = c.seen.sessions().expect("sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].status, SessionStatus::Exited { code: Some(1) });
}

/// Close must terminate the child and report it as one exit delta.
///
/// The user clicked the close button. The session leaves the daemon immediately,
/// and the client learns through the same `Exited` delta a natural exit uses, with
/// no exit code because a killed child was signalled. No full list is broadcast:
/// that shape is quadratic in session count.
#[cfg(not(windows))]
#[tokio::test]
async fn close_terminates_the_session_and_reports_one_exit() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x")).await;
    assert_eq!(h.manager.list().len(), 1);

    c.send(ClientMsg::Close { session: id }).await;
    c.until("the exit delta", |s| s.exits() == 1).await;
    match c.seen.find(|m| matches!(m, ServerMsg::Exited { .. })) {
        Some(ServerMsg::Exited { session, code }) => {
            assert_eq!(*session, id);
            assert_eq!(*code, None, "a killed child was signalled, not exited");
        }
        other => panic!("expected Exited, got {other:?}"),
    }
    assert!(
        h.manager.info(id).is_none(),
        "the session must be gone from the daemon too"
    );

    // And an explicit list confirms the row is gone.
    c.send(ClientMsg::List).await;
    c.until("the snapshot", |s| s.sessions().is_some()).await;
    assert!(c.seen.sessions().expect("sessions").is_empty());
}

/// Closing an unknown session must be an error, since a double click on a close
/// button is ordinary and must not panic the daemon.
#[tokio::test]
async fn closing_an_unknown_session_errors() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(ClientMsg::Close {
        session: SessionId(31337),
    })
    .await;
    c.until("the error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("31337"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
}

/// A session must keep running when its client disconnects, and its history must
/// still be there for the next one.
///
/// This is the reason for a session server at all: quitting or crashing the GUI
/// must not kill twenty agents. The second client is a fresh connection, so it
/// proves the state lives in the daemon rather than in the socket.
#[cfg(not(windows))]
#[tokio::test]
async fn a_session_outlives_the_client_that_created_it() {
    let h = Harness::start(64 * 1024).await;
    let id = {
        let mut first = h.greeted().await;
        let id = first.create(create(1, "read -r x; echo survived")).await;
        id
        // `first` drops here: the GUI has quit.
    };

    let mut second = h.greeted().await;
    second.send(ClientMsg::List).await;
    second
        .until("the surviving session", |s| {
            s.sessions().is_some_and(|l| l.len() == 1)
        })
        .await;
    assert_eq!(second.seen.sessions().expect("sessions")[0].id, id);

    // Still driveable, and still recording.
    second
        .send(ClientMsg::Input {
            session: id,
            data: b"\n".to_vec(),
        })
        .await;
    second
        .until("the exit", |s| {
            s.has(|m| matches!(m, ServerMsg::Exited { .. }))
        })
        .await;
    let (from, bytes, _) = h
        .manager
        .scrollback(id, u64::MAX, 4096)
        .expect("history survived the reconnect");
    assert_eq!(from, 0);
    assert_eq!(bytes, b"\r\nsurvived\r\n");
}

/// A disconnected client's attachment must be torn down.
///
/// A leaked pump would hold a broadcast receiver open, which makes the session look
/// attended forever: its output would stop counting as unread and its bell would
/// never raise for the operator who is no longer there.
#[cfg(not(windows))]
#[tokio::test]
async fn a_disconnect_releases_the_attachment() {
    let h = Harness::start(64 * 1024).await;
    let id = {
        let mut first = h.greeted().await;
        let id = first.create(create(1, "read -r x; echo late")).await;
        first.attach(id, 80, 24).await;
        id
    };

    // Wait for the daemon to actually notice the disconnect before driving any
    // output through the session.
    //
    // THIS IS THE WHOLE TEST AND IT USED TO BE ASSUMED. Dropping the connection
    // above closes a socket; it does not release the attachment. The daemon
    // releases it when its pump notices the peer is gone, and output produced
    // in that gap still reaches a live broadcast receiver, so it is still
    // marked read and the assertion at the end fails. Measured before this
    // wait existed: one failure in twenty runs of `-p vitrum-server --lib`,
    // against a suite whose green baseline is 0.39-0.42s. The test was
    // asserting a consequence of a precondition it never established, which is
    // the same defect as `no_autostart_leaves_a_dead_port_alone` assuming its
    // port was dead.
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while h.manager.watchers(id).expect("still listed") > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the daemon never released the attachment after its client vanished"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // Drive the session from a fresh connection that never attaches. It lists
    // first: status updates go to clients that have asked about a session, so a
    // connection that never mentioned it gets nothing.
    let mut second = h.greeted().await;
    second.send(ClientMsg::List).await;
    second
        .until("the session list", |s| {
            s.sessions().is_some_and(|l| l.len() == 1)
        })
        .await;
    second
        .send(ClientMsg::Input {
            session: id,
            data: b"\n".to_vec(),
        })
        .await;
    second
        .until("the exit", |s| {
            s.has(|m| matches!(m, ServerMsg::Exited { .. }))
        })
        .await;

    assert!(
        h.manager.info(id).expect("still listed").unread,
        "with the attached client gone, new output is unread again"
    );
}

/// Two sessions must report their exits independently, without one masking the
/// other, because twenty concurrent agents is the design point.
#[tokio::test]
async fn concurrent_sessions_report_independently() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(create(1, "exit 2")).await;
    c.send(create(1, "exit 3")).await;
    c.until("both exits", |s| {
        s.ctl
            .iter()
            .filter(|m| matches!(m, ServerMsg::Exited { .. }))
            .count()
            == 2
    })
    .await;

    let mut codes: Vec<Option<i32>> = c
        .seen
        .ctl
        .iter()
        .filter_map(|m| match m {
            ServerMsg::Exited { code, .. } => Some(*code),
            _ => None,
        })
        .collect();
    codes.sort();
    assert_eq!(codes, vec![Some(2), Some(3)]);
}

/// WHY: `List` reaches every retained session, including ones that already
/// exited, and `Hub::watch` used to start a status watcher for each. An exited
/// session's status never changes again, so that task parks forever holding two
/// `watch::Receiver`s and an `Arc<Hub>`: one leaked task per dead session, for
/// the life of a daemon that runs for days.
#[tokio::test]
async fn an_exited_session_gets_no_status_watcher() {
    let manager = std::sync::Arc::new(vitrum_core::SessionManager::new(4096));
    let hub = crate::Hub::new(std::sync::Arc::clone(&manager));
    let id = manager
        .spawn(spec("exit 0"))
        .expect("spawning a shell that exits at once");
    wait_until_dead(&manager, id).await;

    hub.watch(id);
    assert_eq!(
        hub.watcher_count(),
        0,
        "a session that can never change again was given a watcher"
    );
}

/// WHY: `Hub::watch` checks the session is live and only then subscribes, and a
/// child can die in that window. A `watch` value that changed before you
/// subscribed never fires `changed()`, so waiting first parked the task forever
/// and no client was ever told the session ended.
#[tokio::test]
async fn a_status_that_is_already_terminal_is_still_reported() {
    let manager = std::sync::Arc::new(vitrum_core::SessionManager::new(4096));
    let hub = crate::Hub::new(manager);
    let mut events = hub.subscribe();

    // Exactly the state the race produces: subscribed to a channel whose value
    // is already terminal, so nothing will ever change again.
    let (_status_tx, status) = tokio::sync::watch::channel(SessionStatus::Exited { code: Some(7) });
    let (_obs_tx, observations) = tokio::sync::watch::channel(0u64);

    tokio::time::timeout(
        DEADLINE,
        hub.watch_until_exit(SessionId(1), status, observations),
    )
    .await
    .expect("the watcher must not park on a status that is already terminal");

    let event = tokio::time::timeout(DEADLINE, events.recv())
        .await
        .expect("an exit must be published")
        .expect("the bus is open");
    assert_eq!(
        serde_json::from_str::<ServerMsg>(&event).expect("the bus carries JSON"),
        ServerMsg::Exited {
            session: SessionId(1),
            code: Some(7)
        }
    );
}

fn spec(command: &str) -> vitrum_core::SessionSpec {
    vitrum_core::SessionSpec {
        project_id: vitrum_proto::ProjectId(1),
        cwd: std::path::PathBuf::from("/tmp"),
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), command.to_string()],
        env: Vec::new(),
        cols: 80,
        rows: 24,
        title: None,
    }
}

async fn wait_until_dead(manager: &vitrum_core::SessionManager, id: SessionId) {
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while manager
        .info(id)
        .expect("the session is still listed")
        .status
        .is_live()
    {
        assert!(tokio::time::Instant::now() < deadline, "the child never exited");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}
