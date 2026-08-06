//! Input: bytes reaching the child through a real PTY, ordering, and the
//! guarantee that a queued write never blocks its caller.

#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::time::Duration;

use crate::SessionManager;
use crate::tests::helpers::{collect, shell_spec};

/// Input written to a session must reach the child, and the child's reply must
/// come back on the same PTY.
///
/// This is the full keyboard round trip. The expected stream is exact and
/// includes the line discipline's echo of the typed newline, which is what a real
/// terminal shows: the echo is emitted by the PTY when the byte is written, so it
/// is always ordered before the child's response.
#[cfg(unix)]
#[tokio::test]
async fn input_reaches_the_child_and_its_reply_returns() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("read -r line; printf 'got=%s' \"$line\""))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    mgr.write(id, b"hello\n").expect("write");
    c.until(|b| b.ends_with(b"got=hello")).await;
    assert_eq!(c.bytes, b"hello\r\ngot=hello");
    assert_eq!(c.first_seq, Some(0));
}

/// The same round trip on Windows, where ConPTY interleaves its own escape
/// sequences with the echoed input, so the assertion is on the child's reply
/// appearing exactly once rather than on the whole stream.
///
/// The reply is printed by a nested `cmd /V:ON` because `cmd` expands `%L%` when
/// it parses the whole `&&` line, which is before `set /p` has assigned it. The
/// child inherits `L` through the environment and delayed expansion reads it at
/// the moment it runs.
#[cfg(windows)]
#[tokio::test]
async fn input_reaches_the_child_and_its_reply_returns() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec("set /p L= && cmd /V:ON /C echo got=!L!"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    mgr.write(id, b"hello\r\n").expect("write");
    c.until(|b| b.windows(9).any(|w| w == b"got=hello")).await;
    let hits = c.bytes.windows(9).filter(|w| *w == b"got=hello").count();
    assert_eq!(hits, 1, "the child must answer exactly once");
}

/// Writes must reach the child in the order they were made.
///
/// Input is queued to a dedicated writer thread, so a queue that reorders, or a
/// design that wrote from many tasks under a lock, would scramble keystrokes.
/// The bracketed replies are extracted because the echo of each typed line
/// interleaves with them nondeterministically; the ordering assertion is exact.
#[cfg(unix)]
#[tokio::test]
async fn input_order_is_preserved_across_many_writes() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec("while read -r l; do printf '[%s]' \"$l\"; done"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    for i in 1..=5u8 {
        mgr.write(id, format!("{i}\n").as_bytes()).expect("write");
    }
    c.until(|b| b.windows(3).any(|w| w == b"[5]")).await;

    let mut replies = Vec::new();
    let mut rest = c.bytes.as_slice();
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
    assert_eq!(replies, b"[1][2][3][4][5]");
    mgr.close(id).expect("close");
}

/// A write must not block the caller even when the child has stopped reading.
///
/// A PTY write blocks once the terminal's input buffer fills, so writing inline
/// would wedge whichever runtime worker handled the keystroke, and a single
/// paste into a stopped agent would stall unrelated sessions. The payload has no
/// newline, so the canonical-mode line buffer fills and the underlying write
/// really does block behind the queue.
#[cfg(unix)]
#[tokio::test]
async fn a_large_write_to_a_stalled_child_returns_immediately() {
    let mgr = Arc::new(SessionManager::new(4096));
    let id = mgr.spawn(shell_spec("read -r x; exit 0")).expect("spawn");
    let payload = vec![b'z'; 512 * 1024];

    let m = Arc::clone(&mgr);
    let write = tokio::task::spawn_blocking(move || m.write(id, &payload));
    tokio::time::timeout(Duration::from_secs(2), write)
        .await
        .expect("write must not block on the pty")
        .expect("write task must not panic")
        .expect("write must be accepted");

    mgr.close(id).expect("close");
}

/// Writing to a session that does not exist must be an error, not a panic and
/// not a silent success, because a stale pane id in a client is normal after a
/// close and must produce a diagnosable message.
#[tokio::test]
async fn write_to_an_unknown_session_errors() {
    let mgr = SessionManager::new(1024);
    let err = mgr
        .write(vitrum_proto::SessionId(4242), b"x")
        .expect_err("must fail");
    assert!(err.to_string().contains("4242"), "unhelpful error: {err}");
}
