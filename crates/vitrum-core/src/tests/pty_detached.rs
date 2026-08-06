//! Sessions with no client attached: they must keep running and keep filling
//! their ring, because a GUI restart or a tab switch must not cost history.

use crate::SessionManager;
use crate::tests::helpers::{collect, contains, settled, shell_spec, wait_exit};

/// Scrollback must be recorded when no client has ever subscribed.
///
/// This is the requirement that separates vitrum from a terminal that only buffers
/// what someone is watching: a user runs 20 agents and looks at one, and the other
/// 19 must still have their output when they are finally opened.
#[tokio::test]
async fn scrollback_is_recorded_with_no_subscriber() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec("echo nobody-watching"))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let (from, bytes) = settled(&mgr, id, |_, b| contains(b, b"nobody-watching\r\n")).await;
    assert_eq!(from, 0);
    // Contains, not equals or ends with: a pseudoconsole surrounds the child's
    // bytes with its own, mode sets and a screen clear before it and an OSC 0
    // naming the shell after it. Those are terminal bytes the frontend needs and
    // therefore belong in the ring, and where the host puts them is the host's
    // business. What must hold on every platform is that the child's line was
    // recorded, whole, with nobody listening.
    assert!(
        contains(&bytes, b"nobody-watching\r\n"),
        "recorded {bytes:?}"
    );
}

/// A session must survive a detach and keep producing, and the ring must contain
/// output from both before and after the detach.
///
/// Dropping the receiver is exactly what a client does on a tab switch. If output
/// stopped, or was only kept while attached, switching tabs would kill agents.
#[cfg(not(windows))]
#[tokio::test]
async fn a_detached_session_keeps_running_and_recording() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec("echo one; read -r x; echo two"))
        .expect("spawn");

    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"one\r\n")).await;
    assert_eq!(c.bytes, b"one\r\n");
    drop(c); // the detach

    mgr.write(id, b"\n").expect("write");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let (from, bytes, _) = mgr.scrollback(id, u64::MAX, 4096).expect("session exists");
    assert_eq!(from, 0);
    assert_eq!(
        bytes, b"one\r\n\r\ntwo\r\n",
        "history must span the detached period"
    );
}

/// Re-attaching must not replay: the live channel starts at the next chunk, and
/// history comes from an explicit scrollback request.
///
/// If attach replayed implicitly, a client that also backfills would paint every
/// byte twice, and one that does not would still be unable to page further back.
#[tokio::test]
async fn re_attaching_delivers_no_backlog() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr.spawn(shell_spec("echo history")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let mut late = collect(&mgr, id);
    late.expect_quiet().await;
    assert_eq!(late.bytes, b"", "attach must not replay");
    let (_, bytes) = settled(&mgr, id, |_, b| contains(b, b"history\r\n")).await;
    assert!(
        contains(&bytes, b"history\r\n"),
        "history is still available: {bytes:?}"
    );
}

/// Paging backwards must walk to the oldest retained byte and then stop.
///
/// `more` is the client's only signal that it has reached the start of history; if
/// it never goes false the client pages forever, and if it goes false early the
/// user cannot scroll to the beginning of the session.
#[cfg(not(windows))]
#[tokio::test]
async fn scrollback_pages_backwards_to_the_oldest_byte() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec(
            "i=0; while [ $i -lt 100 ]; do printf w; i=$((i+1)); done",
        ))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let mut cursor = u64::MAX;
    let mut collected: Vec<u8> = Vec::new();
    let mut pages = 0;
    loop {
        let (from, bytes, more) = mgr.scrollback(id, cursor, 30).expect("session exists");
        pages += 1;
        assert!(pages <= 10, "paging did not terminate");
        let mut next = bytes;
        next.extend_from_slice(&collected);
        collected = next;
        cursor = from;
        if !more {
            assert_eq!(from, 0, "the last page must reach the first byte");
            break;
        }
    }
    assert_eq!(pages, 4, "100 bytes in 30-byte pages");
    assert_eq!(collected, vec![b'w'; 100]);
}

/// Scrollback for an unknown session must be `None`, so a client holding a closed
/// session id gets a clean answer instead of an empty-looking success.
#[tokio::test]
async fn scrollback_of_an_unknown_session_is_none() {
    let mgr = SessionManager::new(1024);
    assert!(
        mgr.scrollback(vitrum_proto::SessionId(1), 0, 10).is_none(),
        "no session 1 exists yet"
    );
}

/// Subscribing to an unknown session must be `None` rather than a channel that
/// never yields, which would leave a client waiting on output forever.
#[tokio::test]
async fn subscribe_to_an_unknown_session_is_none() {
    let mgr = SessionManager::new(1024);
    assert!(
        mgr.attach(vitrum_proto::SessionId(1), mgr.new_viewer(), 80, 24)
            .is_err()
    );
    assert!(mgr.subscribe_status(vitrum_proto::SessionId(1)).is_none());
    assert!(
        mgr.subscribe_observations(vitrum_proto::SessionId(1))
            .is_none()
    );
}
