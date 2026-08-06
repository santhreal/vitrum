//! Reaping: exit codes, signalled children, and the ordering guarantee that a
//! terminal status implies every byte has already been published.

use vitrum_proto::SessionStatus;

use crate::SessionManager;
#[cfg(not(windows))]
use crate::tests::helpers::{collect, wait_exit_on};
use crate::tests::helpers::{shell_spec, wait_exit};

/// A clean exit must report code 0, not `None`.
///
/// `None` means "signalled" on the wire, so conflating the two would show a
/// successfully finished agent as killed.
#[tokio::test]
async fn clean_exit_reports_code_zero() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 0")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    assert_eq!(
        mgr.info(id).expect("info").status,
        SessionStatus::Exited { code: Some(0) }
    );
}

/// A nonzero exit must report that exact code.
///
/// The code is what tells a user whether their agent crashed or finished, so an
/// approximation like "nonzero" or a hardcoded 1 destroys the only diagnostic.
#[tokio::test]
async fn nonzero_exit_reports_the_exact_code() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 3")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(3));
    assert_eq!(
        mgr.info(id).expect("info").status,
        SessionStatus::Exited { code: Some(3) }
    );
}

/// `exit 1` must be reported as code 1, never as a signal.
///
/// portable-pty synthesises exit code 1 for signalled children, so a reaper that
/// looks at the code instead of the signal name cannot tell a failing command
/// from a killed one. This test is the guard on that specific confusion.
#[tokio::test]
async fn exit_one_is_not_reported_as_a_signal() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 1")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(1));
}

/// The largest single-byte exit status must survive intact.
///
/// Exit codes are truncated to 8 bits by the kernel, and 255 is the value most
/// likely to be mangled by a sign or width mistake on the way to `Option<i32>`.
#[tokio::test]
async fn exit_status_255_survives() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 255")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(255));
}

/// A killed child must report no exit code.
///
/// On Unix a killed process has a signal, not a status, and reporting the
/// synthesised 1 would be indistinguishable from a real failure. The status
/// receiver is taken before the close because closing unregisters the session.
#[cfg(unix)]
#[tokio::test]
async fn a_killed_child_reports_no_exit_code() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let status = mgr.subscribe_status(id).expect("status");
    mgr.close(id).expect("close");
    assert_eq!(wait_exit_on(status).await, None);
}

/// A terminal status must mean every byte the child produced is already visible.
///
/// The reaper runs on the reader thread after the read loop drains, so this
/// ordering is a real guarantee clients depend on: after `Exited` a client can
/// stop streaming and backfill from scrollback without losing the child's last
/// words, which are usually the error message that explains the exit.
#[cfg(not(windows))]
#[tokio::test]
async fn exited_status_implies_all_output_is_published() {
    let mgr = SessionManager::new(64 * 1024);
    // 400 separate writes then an immediate exit: plenty of chance for a reaper
    // that races the drain to publish the exit first.
    let id = mgr
        .spawn(shell_spec(
            "i=0; while [ $i -lt 400 ]; do printf y; i=$((i+1)); done; exit 7",
        ))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(7));
    let (from, bytes, more) = mgr.scrollback(id, u64::MAX, 4096).expect("session exists");
    assert_eq!(from, 0);
    assert!(!more);
    assert_eq!(bytes, vec![b'y'; 400]);
}

/// Writing to an exited session must fail rather than silently vanish.
///
/// A GUI that keeps a dead pane focused would otherwise swallow keystrokes with
/// no feedback at all.
#[tokio::test]
async fn write_to_an_exited_session_fails() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 0")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    let err = mgr.write(id, b"x").expect_err("write must fail");
    assert!(
        err.to_string().contains("has exited"),
        "unhelpful error: {err}"
    );
}

/// An exited session must stay in the registry with its status.
///
/// The sidebar shows exited sessions so the user can read the final output and
/// the exit code; dropping them on exit would erase the outcome.
#[tokio::test]
async fn an_exited_session_remains_listed() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 2")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(2));
    let list = mgr.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    assert_eq!(list[0].status, SessionStatus::Exited { code: Some(2) });
}

/// The status channel must fire once per transition and stay usable, so a client
/// learns about `Running` and `Exited` without ever polling.
#[cfg(not(windows))]
#[tokio::test]
async fn status_transitions_are_observable_without_polling() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        // The second read holds the child open, so Running is observable before
        // the exit overwrites it. A watch channel keeps only the latest value,
        // so a child that exited immediately would make Running unobservable.
        .spawn(shell_spec("read -r x; echo bye; read -r y; exit 5"))
        .expect("spawn");
    let mut status = mgr.subscribe_status(id).expect("status");
    assert_eq!(*status.borrow_and_update(), SessionStatus::Starting);

    let mut c = collect(&mgr, id);
    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.ends_with(b"bye\r\n")).await;

    status.changed().await.expect("running notification");
    assert_eq!(*status.borrow_and_update(), SessionStatus::Running);

    mgr.write(id, b"\n").expect("write");
    assert_eq!(wait_exit_on(status).await, Some(5));
}

/// An exited session must release its PTY writer queue.
///
/// The writer is a dedicated thread parked on that queue. Exited sessions stay in
/// the registry so the user can still read their output and exit code, so if the
/// queue outlived the child, every finished session would leave a thread parked
/// for the daemon's whole lifetime. Nothing can be written to a dead child, so
/// there is nothing to keep it for.
#[tokio::test]
async fn an_exited_session_releases_its_writer_queue() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 0")).expect("spawn");
    let session = mgr.get(id).expect("session");
    assert!(session.input_is_open(), "a live session must accept input");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    assert!(
        !session.input_is_open(),
        "the writer queue must be released when the child is reaped"
    );
    assert!(
        mgr.info(id).is_some(),
        "the session itself must remain listed"
    );
}

/// Sessions that have exited must not leave threads behind.
///
/// Two threads per session are unavoidable while a PTY is live, because a
/// blocking read and a blocking write have no portable async form. What is not
/// acceptable is keeping them after the child is gone: a user who runs agents all
/// day would accumulate a thread pair per finished session.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn exited_sessions_leave_no_threads_behind() {
    fn threads() -> usize {
        std::fs::read_dir("/proc/self/task")
            .expect("procfs is mounted on linux")
            .count()
    }

    let mgr = SessionManager::new(1024);
    let baseline = threads();
    let mut ids = Vec::new();
    for _ in 0..8 {
        ids.push(mgr.spawn(shell_spec("exit 0")).expect("spawn"));
    }
    for id in &ids {
        assert_eq!(wait_exit(&mgr, *id).await, Some(0));
    }

    // Threads unwind after the status flips, so this waits on a deadline rather
    // than assuming the unwind already happened.
    let deadline = tokio::time::Instant::now() + crate::tests::helpers::DEADLINE;
    loop {
        let now = threads();
        if now <= baseline + 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "8 exited sessions left {} threads above the baseline of {baseline}",
            now - baseline
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}
