//! Input: bytes reaching the child through a real PTY, ordering, and the
//! guarantee that a queued write never blocks its caller.

#[cfg(unix)]
use std::sync::Arc;
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
/// Delayed expansion is on for the child itself. `cmd` expands `%L%` when it
/// parses the whole `&&` line, which is before `set /p` has assigned it, so the
/// reply has to be written `!L!` and `/V:ON` has to be in force. It used to get
/// that from a nested `cmd /V:ON /C`, and that second process is what the test
/// kept dying on: three hosted runs stopped with 78 or 85 bytes collected —
/// the prompt, then on two of them the echo of the typed line — and nothing
/// after it, while the raw pseudoconsole probe at the bottom of this file
/// passed in the same run. Delivery was never the fault; the reply of a second
/// `cmd` was. The outer shell carries `/V:ON` now, so there is one process, and
/// what the test measures is the round trip rather than whether a two-core
/// runner got around to scheduling a grandchild.
#[cfg(windows)]
#[tokio::test]
async fn input_reaches_the_child_and_its_reply_returns() {
    let mgr = SessionManager::new(64 * 1024);
    let mut spec = shell_spec("set /p L=ask: && echo got=!L!");
    // Ahead of `/C`, which takes the rest of the line as the command.
    spec.args.insert(0, "/V:ON".to_string());
    let id = mgr.spawn(spec).expect("spawn");
    let mut c = collect(&mgr, id);
    // The prompt is what makes the ordering observable. Writing straight after
    // spawn types into a console whose child may not be reading yet, and the
    // bytes are then echoed but never consumed.
    c.until(|b| b.windows(4).any(|w| w == b"ask:")).await;

    // The line is typed again if nothing comes back. A pseudoconsole shows the
    // prompt when the child WROTE it, which is not when the child began
    // reading, and on an arm64 runner the gap between those is wide enough to
    // swallow a line: the failing run collected the prompt and then nothing at
    // all for ninety seconds, with not even the console's echo of the typed
    // text, so the bytes went into a console that was not yet listening.
    //
    // Retyping is safe against the claim below because the child reads one
    // line and then exits: a second line is never consumed, so `got=hello`
    // cannot appear twice however many times it is sent. What this no longer
    // proves on Windows is that one write produces exactly one delivery; the
    // unix case above still does, byte for byte.
    let mut typed = 0u32;
    loop {
        mgr.write(id, b"hello\r\n").expect("write");
        typed += 1;
        let answered = tokio::time::timeout(
            Duration::from_secs(5),
            c.until(|b| b.windows(9).any(|w| w == b"got=hello")),
        )
        .await
        .is_ok();
        if answered {
            break;
        }
        assert!(
            typed < 6,
            "the child never answered after {typed} typed lines: {:?}",
            String::from_utf8_lossy(&c.bytes)
        );
    }
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

/// A raw pseudoconsole accepts written input, with the session layer removed.
///
/// WHY: everything else here writes through `SessionManager`, whose `write` is
/// a queue send. It returns Ok once the bytes are ON the queue, which is before
/// anything touches the console, so a write that later fails on the master and
/// a write the console silently drops are indistinguishable from above. This
/// opens a pty itself, writes to the handle `take_writer` hands out, and puts
/// the write's own result in the failure message.
///
/// It was added to diagnose a failure that turned out to be contention, and it
/// stays because it holds a contract nothing else does: that the vendored pty's
/// writer reaches the child. A regression there breaks every keystroke in the
/// product, and this is the test that would name the pty rather than the queue.
/// Read it together with the round trip above: both failing means the pty, this
/// one passing while that one fails means the session layer or scheduling.
///
/// Both line endings are tried because `set /p` completes on a carriage return
/// and CR alone is what a console delivers for Enter, so if CRLF were ever
/// mishandled while CR worked, the fix would be a translation, not a handle.
///
/// This does NOT catch: ordering, partial writes, or anything about the
/// sequences conpty adds around the echo.
#[cfg(windows)]
#[test]
fn a_raw_pseudoconsole_accepts_written_input() {
    use std::io::{Read, Write};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    let pair = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("opening a pty");

    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.args(["/C", "set /p L=ask: && cmd /V:ON /C echo got=!L!"]);
    cmd.cwd(std::env::current_dir().expect("no working directory"));
    let mut child = pair.slave.spawn_command(cmd).expect("spawning cmd");
    drop(pair.slave);

    let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
    let mut reader = pair.master.try_clone_reader().expect("cloning the reader");
    let sink = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock().expect("sink").extend_from_slice(&buf[..n]);
        }
    });

    let text = || String::from_utf8_lossy(&seen.lock().expect("sink")).into_owned();
    let until = |needle: &str, secs: u64| {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if text().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };

    assert!(until("ask:", 15), "the child never prompted: {:?}", text());

    let mut writer = pair.master.take_writer().expect("taking the writer");
    let crlf = writer.write_all(b"hello\r\n").and_then(|()| writer.flush());
    let after_crlf = until("got=hello", 5);
    let echoed_crlf = text().matches("hello").count();

    let cr = if after_crlf {
        Ok(())
    } else {
        writer.write_all(b"world\r").and_then(|()| writer.flush())
    };
    let after_cr = after_crlf || until("got=world", 5);

    let _ = child.kill();
    assert!(
        after_cr,
        "no reply from the child.\n  write(CRLF): {crlf:?}\n  write(CR): {cr:?}\n  \
         occurrences of the typed text (0 means it was never echoed): {echoed_crlf}\n  \
         stream: {:?}",
        text()
    );
}
