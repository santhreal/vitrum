//! List and CreateSession: what the sidebar is built from.

#[cfg(not(windows))]
use vitrum_proto::SessionStatus;
use vitrum_proto::{ClientMsg, ProjectId, ServerMsg};

use crate::tests::client::{Harness, create, create_in};

/// A fresh server must answer List with both snapshots, empty.
///
/// A client cannot tell "no sessions" from "no answer" otherwise, and would sit
/// on an empty sidebar waiting for a message that never comes.
#[tokio::test]
async fn list_answers_with_both_snapshots_when_empty() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(ClientMsg::List).await;
    c.until("both snapshots", |s| {
        s.projects().is_some() && s.sessions().is_some()
    })
    .await;
    assert!(c.seen.projects().expect("projects").is_empty());
    assert!(c.seen.sessions().expect("sessions").is_empty());
}

/// Creating a session must be acknowledged with the full new session, and an
/// explicit list must then include it.
///
/// The acknowledgement carries the server-assigned id, which the client needs
/// before it can attach to what it just created, and carrying the whole
/// projection is what makes a separate list broadcast unnecessary.
#[tokio::test]
async fn create_is_acknowledged_and_listed() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "exit 0")).await;

    let created = c.seen.created()[0].clone();
    assert_eq!(created.id, id);
    assert_eq!(created.project_id, ProjectId(1));
    assert_eq!((created.cols, created.rows), (80, 24));
    assert!(!created.unread, "a brand new session has nothing unread");

    c.send(ClientMsg::List).await;
    c.until("the snapshot", |s| s.sessions().is_some()).await;
    let sessions = c.seen.sessions().expect("snapshot");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, id);
}

/// A created session must register its project so the sidebar can group it.
///
/// The protocol has no create-project message, so first use is the only moment a
/// project can be recorded; missing it leaves every session ungrouped.
#[tokio::test]
async fn creating_a_session_registers_its_project() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let cwd = std::env::temp_dir();
    c.send(create_in(42, cwd.clone(), "exit 0")).await;
    c.until("the project snapshot", |s| {
        s.projects().is_some_and(|p| !p.is_empty())
    })
    .await;

    let projects = c.seen.projects().expect("projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].id, ProjectId(42));
    assert_eq!(projects[0].root, cwd.to_string_lossy());
    let expected_name = cwd
        .file_name()
        .expect("the temp dir has a name")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        projects[0].name, expected_name,
        "the row label must be the last path component, not the whole path"
    );
}

/// A second session in the same project must not duplicate or re-root it.
///
/// Two agents in one repository is the normal case; duplicating the project would
/// split the sidebar group in two.
#[tokio::test]
async fn a_second_session_does_not_duplicate_the_project() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.create(create(7, "exit 0")).await;
    c.until("the first project", |s| {
        s.projects().is_some_and(|p| !p.is_empty())
    })
    .await;
    c.create(create(7, "exit 0")).await;
    c.send(ClientMsg::List).await;
    c.until("two sessions", |s| {
        s.sessions().is_some_and(|l| l.len() == 2)
    })
    .await;
    assert_eq!(
        c.seen.projects().expect("projects").len(),
        1,
        "one project id must produce one project row"
    );
}

/// A create that cannot run must report an error and leave nothing behind.
///
/// A half-created session would show a row in the sidebar with no process, which
/// the user can neither use nor understand.
#[tokio::test]
async fn a_create_with_a_bad_cwd_reports_an_error_and_creates_nothing() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let missing = std::env::temp_dir().join("vitrum-server-no-such-dir");
    c.send(create_in(1, missing, "exit 0")).await;
    c.until("the error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("vitrum-server-no-such-dir"),
        "the error must name the path, got {:?}",
        c.seen.errors()[0]
    );
    assert!(
        !c.seen.has(|m| matches!(m, ServerMsg::SessionCreated(_))),
        "nothing was created"
    );
    assert!(h.manager.list().is_empty());
    assert!(
        c.seen.projects().is_none_or(|p| p.is_empty()),
        "a failed create must not register a project"
    );
}

/// A create with a command that does not exist must report an error naming it, so
/// a missing agent binary is distinguishable from a broken server.
#[tokio::test]
async fn a_create_with_a_missing_command_reports_an_error() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(ClientMsg::CreateSession {
        project_id: ProjectId(1),
        cwd: std::env::temp_dir().to_string_lossy().into_owned().into(),
        command: "vitrum-nonexistent-agent".into(),
        args: Vec::new(),
        cols: 80,
        rows: 24,
        title: None,
    })
    .await;
    c.until("the error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("vitrum-nonexistent-agent"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
}

/// The requested title and geometry must survive into the projection, because the
/// client draws from the projection and cannot recover them from anywhere else.
#[tokio::test]
async fn the_requested_title_and_geometry_are_reflected() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let (command, args) = crate::tests::client::shell("exit 0");
    let id = c
        .create(ClientMsg::CreateSession {
            project_id: ProjectId(3),
            cwd: std::env::temp_dir().to_string_lossy().into_owned().into(),
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
            cols: 132,
            rows: 43,
            title: Some("codex".into()),
        })
        .await;
    // Asserted on the create delta, which is what a client actually receives now.
    let created = c.seen.created()[0];
    assert_eq!(created.id, id);
    assert_eq!(created.title, "codex");
    assert_eq!((created.cols, created.rows), (132, 43));
}

/// A session listed before it has spoken must read as Starting, so the sidebar can
/// show spawning apart from alive.
#[cfg(not(windows))]
#[tokio::test]
async fn a_silent_new_session_lists_as_starting() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x")).await;
    // The list is only sent on request now, so ask for it.
    c.send(ClientMsg::List).await;
    c.until("the snapshot", |s| s.sessions().is_some()).await;
    assert_eq!(
        c.seen.sessions().expect("sessions")[0].status,
        SessionStatus::Starting
    );
    h.manager.close(id).expect("close");
}

/// A second client must see sessions the first one created.
///
/// Session ownership belongs to the daemon, not to a connection: that is what
/// makes closing and reopening the GUI harmless.
#[tokio::test]
async fn a_second_client_sees_existing_sessions() {
    let h = Harness::start(4096).await;
    let mut a = h.greeted().await;
    let id = a.create(create(5, "exit 0")).await;

    let mut b = h.greeted().await;
    b.send(ClientMsg::List).await;
    b.until("b's snapshot", |s| s.sessions().is_some()).await;
    let sessions = b.seen.sessions().expect("sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, id);
    assert_eq!(
        b.seen.projects().expect("projects").len(),
        1,
        "projects are daemon-wide too"
    );
}

/// Creating N sessions must produce exactly N deltas and ZERO unsolicited session
/// lists.
///
/// This is the shape guard, and it can only be enforced by counting frames. A
/// full `Sessions` broadcast per create makes startup traffic quadratic in
/// session count: n creates send n lists averaging n/2 fully serialized
/// SessionInfo each, so 20 sessions put ~200 session objects on the wire and 200
/// sessions put ~20,000. Re-serializing all state on every change is the exact
/// failure that became a headline performance bug in a competing shell, and
/// `SessionCreated` already carries the whole new session, so the list is pure
/// duplication.
#[tokio::test]
async fn creating_many_sessions_sends_deltas_and_no_lists() {
    let n = 20;
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    for _ in 0..n {
        c.send(create(1, "exit 0")).await;
    }
    // Waiting for every exit drains everything these sessions will ever push, so
    // a stray list cannot slip in after the count.
    c.until("all creations and exits", |s| {
        s.created().len() == n && s.exits() == n
    })
    .await;

    assert_eq!(c.seen.created().len(), n, "one delta per session");
    let lists = c
        .seen
        .ctl
        .iter()
        .filter(|m| matches!(m, ServerMsg::Sessions { .. }))
        .count();
    assert_eq!(
        lists, 0,
        "no session list may be sent unless the client asked for one"
    );
}

/// The project list must be sent once per new project, not once per session.
///
/// Projects are small, but re-sending them on every create is the same quadratic
/// shape at a smaller constant, and it tells a client to re-render its groups for
/// no reason.
#[tokio::test]
async fn the_project_list_is_sent_once_per_new_project() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    for _ in 0..5 {
        c.send(create(1, "exit 0")).await;
    }
    c.until("all creations and exits", |s| {
        s.created().len() == 5 && s.exits() == 5
    })
    .await;
    let announcements = c
        .seen
        .ctl
        .iter()
        .filter(|m| matches!(m, ServerMsg::Projects { .. }))
        .count();
    assert_eq!(
        announcements, 1,
        "five sessions in one project is one project announcement"
    );

    // A genuinely new project is a real change and must be announced.
    c.send(create(2, "exit 0")).await;
    c.until("the second project", |s| {
        s.ctl
            .iter()
            .filter(|m| matches!(m, ServerMsg::Projects { .. }))
            .count()
            == 2
    })
    .await;
    assert_eq!(c.seen.projects().expect("projects").len(), 2);
}

/// An explicit List must still be answered with both full snapshots.
///
/// Deltas are the steady state, but a client that just connected, or one
/// recovering from a gap, needs a complete picture on demand.
#[tokio::test]
async fn an_explicit_list_still_returns_full_snapshots() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    for _ in 0..3 {
        c.send(create(1, "exit 0")).await;
    }
    c.until("all creations", |s| s.created().len() == 3).await;
    c.send(ClientMsg::List).await;
    c.until("the snapshot", |s| {
        s.sessions().is_some_and(|l| l.len() == 3)
    })
    .await;
    assert_eq!(c.seen.sessions().expect("sessions").len(), 3);
    assert_eq!(c.seen.projects().expect("projects").len(), 1);
}
