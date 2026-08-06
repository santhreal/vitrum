//! Live output: exact bytes on the broadcast channel, sequence offsets,
//! coalescing, and the status and unread transitions output drives.

#[cfg(not(windows))]
use vitrum_proto::SessionStatus;

use crate::SessionManager;
use crate::tests::helpers::{attach, collect, contains, settled, shell_spec, wait_exit};

/// The bytes a child writes must arrive on the broadcast channel byte for byte.
///
/// This is the product's whole job. A regression that reorders, truncates, or
/// re-encodes output shows up as a corrupted terminal, and the `\r\n` is part of
/// the contract: the PTY line discipline turns the child's `\n` into `\r\n`, and
/// a client that receives a bare `\n` will render a staircase.
///
/// Subscribing after `spawn` is safe rather than racy because output cannot be
/// published until the coalescing window has elapsed since the child's first
/// byte, which is orders of magnitude longer than the microseconds between
/// `spawn` returning and this subscribe.
#[tokio::test]
async fn child_output_arrives_verbatim_on_the_broadcast_channel() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr.spawn(shell_spec("echo vitrum-ok")).expect("spawn");
    let mut c = collect(&mgr, id);
    let want: &[u8] = b"vitrum-ok\r\n";
    c.until(|b| contains(b, want)).await;
    // Contains, not equals: a pseudoconsole surrounds the child's bytes with its
    // own, mode sets and a screen clear before and an OSC 0 naming the shell
    // after, and those are bytes a terminal is supposed to receive. The contract
    // is that the child's bytes arrive whole, in order, and unaltered.
    assert!(contains(&c.bytes, want), "received {:?}", c.bytes);
    assert_eq!(c.first_seq, Some(0), "the first byte of a session is seq 0");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// The broadcast stream and the scrollback ring must agree exactly.
///
/// They are written in the same critical section, so a divergence means a client
/// that backfills history would see different bytes than one that streamed live,
/// at the same offsets.
#[tokio::test]
async fn broadcast_and_scrollback_agree_byte_for_byte() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr.spawn(shell_spec("echo agreement")).expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| contains(b, b"agreement\r\n")).await;
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    // Both sides have to be finished before they can be compared. The ring stops
    // growing first, and the collector is then read up to that same head so the
    // comparison is the whole stream against the whole stream.
    let (from, bytes) = settled(&mgr, id, |_, b| contains(b, b"agreement\r\n")).await;
    c.until(|b| b.len() >= bytes.len()).await;
    assert_eq!(from, 0);
    assert_eq!(bytes, c.bytes);
    assert!(contains(&bytes, b"agreement\r\n"), "recorded {bytes:?}");
}

/// Output must be coalesced before it is broadcast.
///
/// A child doing 200 one-byte writes must not become 200 broadcast chunks: at 20
/// agents that is the difference between a few wakeups and tens of thousands per
/// second, and every chunk costs an allocation and a frame on every attached
/// socket. Asserting the byte total alongside the chunk bound keeps coalescing
/// from being "fixed" by dropping output.
#[cfg(not(windows))]
#[tokio::test]
async fn output_is_coalesced_into_few_chunks() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec(
            "i=0; while [ $i -lt 200 ]; do printf x; i=$((i+1)); done",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.len() >= 200).await;
    assert_eq!(c.bytes, vec![b'x'; 200]);
    assert!(
        c.chunks <= 20,
        "200 child writes became {} chunks; coalescing is not working",
        c.chunks
    );
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// A newline from the child must reach the client as CRLF, on every line.
///
/// The translation is done by the PTY, so this also proves output is not being
/// routed around it. A client that receives bare LF renders each line further
/// right than the last.
#[cfg(not(windows))]
#[tokio::test]
async fn newlines_arrive_as_crlf() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("printf 'a\nb\n'")).expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.len() >= 6).await;
    assert_eq!(c.bytes, b"a\r\nb\r\n");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// A session that has produced no output must report `Starting`, and the first
/// byte must move it to `Running`.
///
/// The sidebar uses this to distinguish "spawning" from "alive". If the
/// transition never fires, every live session looks stuck starting forever.
#[cfg(not(windows))]
#[tokio::test]
async fn first_output_moves_the_session_from_starting_to_running() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        // The trailing read keeps the child alive after it speaks, so the
        // Running observation cannot race the exit.
        .spawn(shell_spec("read -r x; echo started; read -r y"))
        .expect("spawn");
    assert_eq!(
        mgr.info(id).expect("info").status,
        SessionStatus::Starting,
        "no output yet, so the session cannot be Running"
    );

    let mut c = collect(&mgr, id);
    mgr.write(id, b"go\n").expect("write");
    c.until(|b| contains(b, b"started\r\n")).await;
    assert_eq!(mgr.info(id).expect("info").status, SessionStatus::Running);

    mgr.write(id, b"\n").expect("write");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// Output with nobody attached must set `unread`, and attaching must clear it.
///
/// This is the sidebar's activity dot. If output never sets it the user misses
/// an agent asking a question; if attaching never clears it the dot is always on
/// and therefore useless.
#[tokio::test]
async fn unread_is_set_while_unattached_and_cleared_by_subscribing() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("echo ping")).expect("spawn");
    assert!(!mgr.info(id).expect("info").unread, "starts read");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    assert!(
        mgr.info(id).expect("info").unread,
        "output arrived with no client attached"
    );
    let _rx = attach(&mgr, id);
    assert!(
        !mgr.info(id).expect("info").unread,
        "attaching means the client is now watching"
    );
}

/// Output that a client is actively receiving must not be marked unread.
///
/// Otherwise the focused session shows an unread dot for its own visible output.
#[tokio::test]
async fn output_to_an_attached_client_is_not_unread() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("echo watched")).expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| contains(b, b"watched\r\n")).await;
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    assert!(!mgr.info(id).expect("info").unread);
}

/// Activity timestamps must advance when output arrives, so the sidebar can sort
/// by recency instead of showing spawn order forever.
#[tokio::test]
async fn output_updates_last_activity() {
    let mgr = SessionManager::new(4096);
    let id = mgr.spawn(shell_spec("echo tick")).expect("spawn");
    let created = mgr.info(id).expect("info").created_at_ms;
    let mut c = collect(&mgr, id);
    c.until(|b| contains(b, b"tick\r\n")).await;
    let info = mgr.info(id).expect("info");
    assert!(
        info.last_activity_ms >= created,
        "activity {} predates creation {}",
        info.last_activity_ms,
        created
    );
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}
