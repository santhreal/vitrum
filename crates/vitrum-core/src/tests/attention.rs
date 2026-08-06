//! Attention signals as a session actually produces them: a real child ringing
//! the bell, idle time, and failure.
//!
//! The scanner that finds those signals in the byte stream is exercised in
//! `output_scan`, against the chunk boundaries the kernel really produces.

use vitrum_proto::IDLE_ATTENTION_MS;

use crate::SessionManager;
#[cfg(not(windows))]
use crate::tests::helpers::collect;
use crate::tests::helpers::{attach, shell_spec, wait_exit};

/// A child that rings the bell must be reported as wanting the operator.
///
/// End to end through a real PTY: the byte has to survive the line discipline,
/// coalescing, and the projection.
#[cfg(not(windows))]
#[tokio::test]
async fn a_child_ringing_the_bell_wants_the_operator() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("printf 'ask\\007'")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let info = mgr.info(id).expect("info");
    assert!(info.attention.bell, "BEL in the output must raise the bell");
    assert_eq!(info.attention.priority(), 2);
    assert!(info.attention.wants_operator());
}

/// Attaching must acknowledge the bell.
///
/// The bell means "since you last looked". If attaching did not clear it the
/// indicator would latch on forever and stop distinguishing anything.
#[cfg(not(windows))]
#[tokio::test]
async fn attaching_acknowledges_the_bell() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("printf 'ask\\007'")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    assert!(mgr.info(id).expect("info").attention.bell);

    let _rx = attach(&mgr, id);
    assert!(!mgr.info(id).expect("info").attention.bell);
    assert_eq!(mgr.info(id).expect("info").attention.priority(), 0);
}

/// A bell in output the operator is already watching must not raise the flag.
///
/// Otherwise a focused session decorates itself with an attention indicator for
/// output that is on screen.
#[cfg(not(windows))]
#[tokio::test]
async fn a_bell_while_attached_does_not_raise_the_flag() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("read -r x; printf 'ask\\007'"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.contains(&0x07)).await;
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    assert!(
        !mgr.info(id).expect("info").attention.bell,
        "the operator was watching when it rang"
    );
}

/// A nonzero exit must mark the session failed, outranking every other signal.
#[tokio::test]
async fn a_nonzero_exit_marks_the_session_failed() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 4")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(4));
    let info = mgr.info(id).expect("info");
    assert!(info.attention.failed);
    assert_eq!(info.attention.priority(), 4);
}

/// A clean exit must not be marked failed, or every finished agent would demand
/// attention and the indicator would mean nothing.
#[tokio::test]
async fn a_clean_exit_is_not_a_failure() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 0")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    let info = mgr.info(id).expect("info");
    assert!(!info.attention.failed);
    assert_eq!(info.attention.priority(), 0);
}

/// A signalled child must count as failed.
///
/// `code: None` is the wire's way of saying "signalled", and a crashed or killed
/// agent is precisely what the operator has to see first.
#[cfg(unix)]
#[tokio::test]
async fn a_signalled_child_counts_as_failed() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let status = mgr.subscribe_status(id).expect("status");
    let session = mgr.get(id).expect("session");
    mgr.close(id).expect("close");
    assert_eq!(crate::tests::helpers::wait_exit_on(status).await, None);
    // The session left the registry on close, so its own projection is what a
    // still-attached client would read.
    assert!(session.snapshot().attention.failed);
}

/// Idle time must be silence the operator has not seen, and a fresh unattached
/// session must not yet claim to want attention.
///
/// Raw time since output would light up a session read five seconds ago and never
/// turn off, and an always-on indicator trains people to ignore it.
#[tokio::test]
async fn idle_time_is_derived_and_starts_below_the_threshold() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 0")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    let info = mgr.info(id).expect("info");
    assert!(
        info.attention.idle_ms < IDLE_ATTENTION_MS,
        "a session that just ran cannot be idle for {}ms",
        info.attention.idle_ms
    );
}

/// Once the operator has looked, idle time must read exactly zero.
///
/// This is the whole correction: the qualified signal means "this agent stopped
/// and you have not looked", so attaching after the last output has to reset it
/// to zero rather than letting it keep climbing.
#[tokio::test]
async fn idle_time_is_zero_once_the_operator_has_looked() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("echo seen")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    let _rx = attach(&mgr, id);
    assert_eq!(
        mgr.info(id).expect("info").attention.idle_ms,
        0,
        "focus is newer than the last output, so nothing is unseen"
    );
}

/// Output that arrives while a client is attached must keep idle at zero, because
/// the operator is watching it land.
#[cfg(not(windows))]
#[tokio::test]
async fn output_while_attached_keeps_idle_at_zero() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("read -r x; echo watched"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.ends_with(b"watched\r\n")).await;
    assert_eq!(mgr.info(id).expect("info").attention.idle_ms, 0);
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// Output produced with nobody attached must count as unseen silence.
///
/// The focus timestamp is older than the output, so the derived idle time has to
/// be the real elapsed time rather than zero.
#[tokio::test]
async fn unseen_output_leaves_idle_running() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("echo unseen")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    let first = mgr.info(id).expect("info").attention.idle_ms;
    let second = mgr.info(id).expect("info").attention.idle_ms;
    assert!(
        second >= first,
        "unseen idle time must not run backwards: {second} < {first}"
    );
}

/// Attaching must acknowledge a failure without resurrecting the session.
///
/// `attention.failed` means "unacknowledged failure" while `status` records what
/// the process actually did. Coupling them would either lose the exit code once
/// the user looked, or leave a read failure demanding attention forever.
#[tokio::test]
async fn a_read_failure_stays_dead_without_demanding_attention() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 101")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(101));
    assert!(mgr.info(id).expect("info").attention.failed);

    let _rx = attach(&mgr, id);
    let info = mgr.info(id).expect("info");
    assert_eq!(
        info.status,
        vitrum_proto::SessionStatus::Exited { code: Some(101) },
        "the exit code is history and must survive being read"
    );
    assert_eq!(
        info.attention,
        vitrum_proto::Attention::default(),
        "an acknowledged failure demands nothing"
    );
    assert!(!info.attention.wants_operator());
}
