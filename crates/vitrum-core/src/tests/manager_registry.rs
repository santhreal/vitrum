//! Registry bookkeeping: id assignment, listing, closing, and every way spawn
//! must refuse rather than half-create a session.

use vitrum_proto::SessionId;

use crate::tests::helpers::{shell_spec, wait_exit, wait_exit_on};
use crate::{SessionManager, SessionSpec};

/// Ids must start at 1 and increase.
///
/// Zero is reserved as "no session" by every client that stores an optional
/// focus, and a reused id would attach a client to the wrong PTY after a close.
#[tokio::test]
async fn ids_start_at_one_and_increase() {
    let mgr = SessionManager::new(1024);
    let a = mgr.spawn(shell_spec("exit 0")).expect("spawn a");
    let b = mgr.spawn(shell_spec("exit 0")).expect("spawn b");
    assert_eq!(a, SessionId(1));
    assert_eq!(b, SessionId(2));
    assert_eq!(wait_exit(&mgr, a).await, Some(0));
    assert_eq!(wait_exit(&mgr, b).await, Some(0));
}

/// A closed id must never be handed out again.
///
/// Recycling ids is how a client ends up streaming a new agent's output into a
/// closed tab's scrollback.
#[tokio::test]
async fn a_closed_id_is_not_reused() {
    let mgr = SessionManager::new(1024);
    let a = mgr.spawn(shell_spec("read -r x")).expect("spawn a");
    let status = mgr.subscribe_status(a).expect("status");
    mgr.close(a).expect("close");
    wait_exit_on(status).await;
    let b = mgr.spawn(shell_spec("exit 0")).expect("spawn b");
    assert_ne!(a, b);
    assert_eq!(b, SessionId(2));
    assert_eq!(wait_exit(&mgr, b).await, Some(0));
}

/// Listing must be ordered by id so the sidebar does not reshuffle on every
/// refresh, which would make rows unclickable.
#[tokio::test]
async fn listing_is_ordered_by_id() {
    let mgr = SessionManager::new(1024);
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(mgr.spawn(shell_spec("exit 0")).expect("spawn"));
    }
    let listed: Vec<SessionId> = mgr.list().iter().map(|i| i.id).collect();
    assert_eq!(listed, ids);
    for id in ids {
        assert_eq!(wait_exit(&mgr, id).await, Some(0));
    }
}

/// An unknown id must be `None`, not a default session, so a stale client id
/// cannot silently address the wrong PTY.
#[tokio::test]
async fn info_for_an_unknown_id_is_none() {
    let mgr = SessionManager::new(1024);
    assert!(mgr.info(SessionId(1)).is_none());
    assert!(mgr.list().is_empty());
}

/// Closing must remove the session from the registry immediately.
///
/// The user clicked the close button; the row has to leave the sidebar without
/// waiting for the child to die.
#[tokio::test]
async fn closing_removes_the_session_immediately() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    assert_eq!(mgr.list().len(), 1);
    let status = mgr.subscribe_status(id).expect("status");
    mgr.close(id).expect("close");
    assert!(mgr.list().is_empty());
    assert!(mgr.info(id).is_none());
    wait_exit_on(status).await;
}

/// Closing twice must report the second as an error rather than panicking, since
/// a double click on a close button is ordinary.
#[tokio::test]
async fn closing_an_unknown_session_errors() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let status = mgr.subscribe_status(id).expect("status");
    mgr.close(id).expect("first close");
    let err = mgr.close(id).expect_err("second close must fail");
    assert!(err.to_string().contains("no session"), "was: {err}");
    wait_exit_on(status).await;
}

/// Closing an already-exited session must still succeed.
///
/// The child is gone but the row is still on screen, and the user must be able to
/// dismiss it.
#[tokio::test]
async fn closing_an_exited_session_succeeds() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 0")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    mgr.close(id).expect("close must tolerate a dead child");
    assert!(mgr.info(id).is_none());
}

/// A command that does not exist must fail at spawn, not become a session that
/// dies mysteriously. The message has to name the command or the user cannot tell
/// a typo from a broken agent install.
#[tokio::test]
async fn spawning_a_missing_command_fails() {
    let mgr = SessionManager::new(1024);
    let mut spec = shell_spec("exit 0");
    spec.command = "vitrum-no-such-binary".to_string();
    spec.args.clear();
    let err = mgr.spawn(spec).expect_err("must fail");
    assert!(
        err.to_string().contains("vitrum-no-such-binary"),
        "unhelpful error: {err:#}"
    );
    assert!(
        mgr.list().is_empty(),
        "a failed spawn must leave no session"
    );
}

/// A working directory that is not a directory must be refused up front.
///
/// Opening a project whose folder was moved is common, and the error has to name
/// the path instead of surfacing as an opaque child failure.
#[tokio::test]
async fn spawning_with_a_bad_cwd_fails() {
    let mgr = SessionManager::new(1024);
    let mut spec = shell_spec("exit 0");
    spec.cwd = std::env::temp_dir().join("vitrum-definitely-not-here");
    let err = mgr.spawn(spec).expect_err("must fail");
    assert!(
        err.to_string().contains("vitrum-definitely-not-here"),
        "unhelpful error: {err:#}"
    );
    assert!(mgr.list().is_empty());
}

/// An empty command must be refused rather than reaching the PTY layer, where it
/// would panic inside the command builder.
#[tokio::test]
async fn spawning_an_empty_command_fails() {
    let mgr = SessionManager::new(1024);
    let mut spec = shell_spec("exit 0");
    spec.command = String::new();
    let err = mgr.spawn(spec).expect_err("must fail");
    assert!(err.to_string().contains("empty"), "was: {err}");
}

/// Spawning outside a Tokio runtime must be a clear error.
///
/// The coalescing window needs a timer, so a caller that forgot the runtime would
/// otherwise get a panic from deep inside Tokio with no hint about the cause.
#[test]
fn spawning_outside_a_runtime_errors() {
    let mgr = SessionManager::new(1024);
    let err = mgr.spawn(shell_spec("exit 0")).expect_err("must fail");
    assert!(
        err.to_string().contains("Tokio runtime"),
        "unhelpful error: {err:#}"
    );
}

/// The projection must carry back exactly what was asked for, because the sidebar
/// renders these fields and a client cannot recover them from anywhere else.
#[tokio::test]
async fn the_projection_reflects_the_spec() {
    let mgr = SessionManager::new(1024);
    let (command, args) = crate::tests::helpers::shell("exit 0");
    let spec = SessionSpec {
        project_id: vitrum_proto::ProjectId(11),
        cwd: std::env::temp_dir(),
        command: command.clone(),
        args: args.clone(),
        env: vec![("VITRUM_TEST".to_string(), "1".to_string())],
        cols: 100,
        rows: 30,
        title: Some("my agent".to_string()),
    };
    let id = mgr.spawn(spec).expect("spawn");
    let info = mgr.info(id).expect("info");
    assert_eq!(info.id, id);
    assert_eq!(info.project_id, vitrum_proto::ProjectId(11));
    assert_eq!(info.title, "my agent");
    assert_eq!(info.command, command);
    assert_eq!(info.args, args);
    assert_eq!(info.cwd, std::env::temp_dir().to_string_lossy());
    assert_eq!((info.cols, info.rows), (100, 30));
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// Without a title, the tab must be labelled with the command's file name rather
/// than a whole path, which would not fit a sidebar row.
#[tokio::test]
async fn the_default_title_is_the_command_basename() {
    let mgr = SessionManager::new(1024);
    let mut spec = shell_spec("exit 0");
    let full = std::path::Path::new(&spec.command)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .expect("the shell has a file name");
    spec.title = None;
    let id = mgr.spawn(spec).expect("spawn");
    assert_eq!(mgr.info(id).expect("info").title, full);
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// Environment overrides must reach the child.
///
/// Agents are configured through the environment, so a dropped variable means the
/// wrong model, the wrong key, or no authentication at all.
#[cfg(not(windows))]
#[tokio::test]
async fn spec_environment_reaches_the_child() {
    let mgr = SessionManager::new(4096);
    let mut spec = shell_spec("printf '%s' \"$VITRUM_MARKER\"");
    spec.env = vec![("VITRUM_MARKER".to_string(), "carried".to_string())];
    let id = mgr.spawn(spec).expect("spawn");
    let mut c = crate::tests::helpers::collect(&mgr, id);
    c.until(|b| b.len() >= 7).await;
    assert_eq!(c.bytes, b"carried");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// EVERY advertised terminal capability reaches the child, and each is
/// overridable.
///
/// Driven from `DEFAULT_TERM_ENV` rather than a list written here, so adding a
/// variable without deciding what a child should see fails instead of shipping
/// unread. `TERM` alone was set for a year while the renderer was already
/// 24-bit, and Gemini CLI spent that year printing "True color (24-bit)
/// support not detected" and quantising itself to 256 colours.
///
/// The override half matters as much as the default: a session started to
/// reproduce a rendering bug has to be able to claim to be a dumb terminal.
#[cfg(not(windows))]
#[tokio::test]
async fn every_advertised_capability_reaches_the_child_and_can_be_overridden() {
    for (key, want) in crate::session::DEFAULT_TERM_ENV {
        let mgr = SessionManager::new(4096);
        let id = mgr
            .spawn(shell_spec(&format!("printf '%s' \"${key}\"")))
            .expect("spawn");
        let mut c = crate::tests::helpers::collect(&mgr, id);
        c.until(|b| b.len() >= want.len()).await;
        assert_eq!(
            c.bytes,
            want.as_bytes(),
            "{key} should have reached the child as {want:?}"
        );
        assert_eq!(wait_exit(&mgr, id).await, Some(0));

        let mut spec = shell_spec(&format!("printf '%s' \"${key}\""));
        spec.env = vec![(key.to_string(), "overridden".to_string())];
        let id = mgr.spawn(spec).expect("spawn");
        let mut c = crate::tests::helpers::collect(&mgr, id);
        c.until(|b| b.len() >= 10).await;
        assert_eq!(c.bytes, b"overridden", "{key} should have been overridable");
        assert_eq!(wait_exit(&mgr, id).await, Some(0));
    }
}

/// The colour claim is the engine's to make, not this crate's to invent.
///
/// `vitrum-vt` publishes what it can render and proves it renders that; this
/// crate's only job is to tell children. Asserting the identity here is what
/// stops someone "fixing" a colour bug by editing the string on this side,
/// where no test can tell whether the engine agrees.
#[test]
fn the_colour_claim_comes_from_the_engine() {
    let advertised = crate::session::DEFAULT_TERM_ENV
        .iter()
        .find(|(k, _)| *k == "COLORTERM")
        .map(|(_, v)| *v);
    assert_eq!(advertised, Some(vitrum_vt::COLORTERM));
}
