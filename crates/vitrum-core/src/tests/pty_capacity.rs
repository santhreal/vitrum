//! A live session whose output exceeds its ring: eviction must be reported
//! honestly so a client can tell history is gone rather than being served the
//! wrong bytes.

use crate::SessionManager;
use crate::tests::helpers::{shell_spec, wait_exit, whole_stream};

/// Long enough to overflow every ring these tests configure, on a platform that
/// prepends a pty preamble as well as one that does not.
const PAYLOAD: &str = "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuv";

/// The script every test in this file runs.
fn script() -> String {
    format!("echo {PAYLOAD}")
}

/// A session that produced more than its capacity must report the correct
/// `oldest_seq`, i.e. the offset of the oldest byte it still holds.
///
/// This is the number a reconnecting client compares against its own cursor to
/// decide whether it can resume or must resync. If it were the retained length,
/// or reset to zero, the client would request an offset the server no longer has
/// and either loop or paint at the wrong place.
#[tokio::test]
async fn a_session_over_capacity_reports_the_correct_oldest_seq() {
    let cap = 32;
    let whole = whole_stream(&script()).await;
    let total = whole.len() as u64;
    assert!(total > cap as u64, "the payload did not overflow the ring");

    let mgr = SessionManager::new(cap);
    let id = mgr.spawn(shell_spec(&script())).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let (from, bytes, more) = mgr.scrollback(id, u64::MAX, 4096).expect("session exists");
    assert_eq!(
        from,
        total - cap as u64,
        "oldest retained byte is capacity bytes back from the head"
    );
    assert!(
        !more,
        "nothing older than the oldest retained byte survives"
    );
    assert_eq!(bytes.len(), cap);
    assert_eq!(bytes, &whole[whole.len() - cap..]);
}

/// A request bounded above the head must clamp to the head rather than refusing
/// or returning future offsets, because `u64::MAX` is the agreed way to say
/// "everything up to now".
#[tokio::test]
async fn scrollback_clamps_before_seq_to_the_head() {
    let cap = 32;
    let mgr = SessionManager::new(cap);
    let id = mgr.spawn(shell_spec(&script())).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let unbounded = mgr.scrollback(id, u64::MAX, 4096).expect("session exists");
    // An unbounded read reaches the head, so its own answer names it.
    let head = unbounded.0 + unbounded.1.len() as u64;
    let at_head = mgr.scrollback(id, head, 4096).expect("session exists");
    let past_head = mgr
        .scrollback(id, head + 10_000, 4096)
        .expect("session exists");
    assert_eq!(unbounded, at_head);
    assert_eq!(unbounded, past_head);
}

/// A request for history older than the oldest retained byte must return an empty
/// chunk with `more` false, which tells the client it has reached the start of
/// what exists instead of leaving it paging forever.
#[tokio::test]
async fn scrollback_before_the_oldest_byte_is_empty_and_final() {
    let cap = 32;
    let total = whole_stream(&script()).await.len() as u64;

    let mgr = SessionManager::new(cap);
    let id = mgr.spawn(shell_spec(&script())).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let oldest = total - cap as u64;
    let (from, bytes, more) = mgr.scrollback(id, oldest, 4096).expect("session exists");
    assert_eq!(from, oldest);
    assert_eq!(bytes, b"");
    assert!(!more);

    let (from, bytes, more) = mgr.scrollback(id, 0, 4096).expect("session exists");
    assert_eq!(from, oldest, "even seq 0 answers with the oldest retained");
    assert_eq!(bytes, b"");
    assert!(!more);
}

/// A `max_bytes` smaller than what is retained must return the newest slice and
/// report that more remains, so paging backwards starts from the bottom of the
/// viewport like a scrollbar does.
#[tokio::test]
async fn scrollback_returns_the_newest_slice_first() {
    let cap = 32;
    let whole = whole_stream(&script()).await;
    let total = whole.len() as u64;

    let mgr = SessionManager::new(cap);
    let id = mgr.spawn(shell_spec(&script())).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let (from, bytes, more) = mgr.scrollback(id, u64::MAX, 8).expect("session exists");
    assert_eq!(from, total - 8);
    assert_eq!(bytes, &whole[whole.len() - 8..]);
    assert!(more, "24 retained bytes are still older than this page");
}

/// A session configured with no scrollback at all must still track offsets and
/// answer coherently, since a zero ring is a legitimate low-memory setting.
#[tokio::test]
async fn a_zero_capacity_session_reports_no_history() {
    let total = whole_stream(&script()).await.len() as u64;

    let mgr = SessionManager::new(0);
    let id = mgr.spawn(shell_spec(&script())).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    let (from, bytes, more) = mgr.scrollback(id, u64::MAX, 4096).expect("session exists");
    assert_eq!(from, total, "the head is still counted");
    assert_eq!(bytes, b"");
    assert!(!more);
}
