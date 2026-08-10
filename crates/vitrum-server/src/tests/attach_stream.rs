//! Attach, Detach, and the data plane those two switch on and off.

use vitrum_proto::{ClientMsg, ServerMsg};

use crate::tests::client::{Harness, create};

/// Attached output must arrive as binary frames carrying the exact child bytes.
///
/// This is the hot path of the whole product. The frame header must name the
/// session and the byte offset, and the payload must be verbatim: a client cannot
/// repair reordered, re-encoded, or misattributed output.
#[cfg(not(windows))]
#[tokio::test]
async fn attach_streams_the_exact_child_bytes() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    // Wait for input before speaking, so the attach is in place before any byte
    // exists and the stream provably starts at offset zero.
    let id = c.create(create(1, "read -r x; echo streamed")).await;

    c.attach(id, 80, 24).await;

    c.send(ClientMsg::Input {
        session: id,
        data: b"go\n".to_vec(),
    })
    .await;
    c.until("the child's output", |s| s.carries(id, b"streamed\r\n"))
        .await;

    assert_eq!(
        c.seen.bytes(id),
        b"go\r\nstreamed\r\n",
        "the echoed input then the child's line, verbatim"
    );
    assert_eq!(
        c.seen.first_seq(id),
        Some(0),
        "the first byte of a session is offset 0"
    );
}

/// The same claim on Windows, where the pseudoconsole frames the child's output
/// with bytes of its own and the whole stream is therefore not the child's alone.
///
/// The child's line must still cross the socket exactly once, unaltered, and the
/// stream must still start at offset zero.
///
/// The Linux fixture holds the child at `read -r x` so the attach is provably in
/// place before the first byte exists. `set /p` was the Windows counterpart and
/// is not one: it returned immediately on a pseudoconsole often enough that the
/// whole child ran and exited with code 0 before the attach landed, and the
/// stream this test is about was then never live at all. A child that sleeps
/// before it speaks buys the same ordering without depending on how a shell
/// built-in treats a console it has just been handed.
#[cfg(windows)]
#[tokio::test]
async fn attach_streams_the_exact_child_bytes() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "ping -n 3 127.0.0.1 >NUL && echo streamed")).await;

    c.attach(id, 80, 24).await;
    c.until("the child's output", |s| s.carries(id, b"streamed\r\n"))
        .await;

    let hits = c
        .seen
        .bytes(id)
        .windows(10)
        .filter(|w| *w == b"streamed\r\n")
        .count();
    assert_eq!(hits, 1, "the child's line must arrive exactly once");
    assert_eq!(
        c.seen.first_seq(id),
        Some(0),
        "the first byte of a session is offset 0"
    );
}

/// Attach must not replay history, and history must still be requestable.
///
/// An implicit replay would double-paint for any client that also backfills, and
/// no replay at all with no scrollback would make history unreachable. The split
/// is deliberate: live from attach, history on request.
#[tokio::test]
async fn attach_streams_live_only_while_history_stays_requestable() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "echo already-happened")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;

    c.attach(id, 80, 24).await;
    c.quiet().await;
    assert_eq!(
        c.seen.data_frames, 0,
        "attach must not replay: no data frames"
    );

    c.send(ClientMsg::Scrollback {
        session: id,
        before_seq: u64::MAX,
        max_bytes: 4096,
    })
    .await;
    c.until("the history", |s| {
        s.has(|m| matches!(m, ServerMsg::ScrollbackChunk { .. }))
    })
    .await;
    match c.seen.find(|m| matches!(m, ServerMsg::ScrollbackChunk { .. })) {
        Some(ServerMsg::ScrollbackChunk { data, from_seq, .. }) => {
            assert_eq!(*from_seq, 0);
            assert!(
                data.windows(18).any(|w| w == b"already-happened\r\n"),
                "history must carry the child's line: {data:?}"
            );
        }
        other => panic!("expected a scrollback chunk, got {other:?}"),
    }
}

/// Detach must stop the stream while the session keeps running and recording.
///
/// This is a tab switch. If output stopped, switching tabs would freeze an agent;
/// if frames kept arriving, an unfocused session would keep costing socket
/// bandwidth for output nobody is drawing.
#[cfg(not(windows))]
#[tokio::test]
async fn detach_stops_the_stream_but_not_the_session() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x; echo first; read -r y; echo second")).await;

    c.attach(id, 80, 24).await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the first line", |s| s.bytes(id).ends_with(b"first\r\n"))
        .await;
    let before_detach = c.seen.data_frames;

    c.send(ClientMsg::Detach { session: id }).await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;
    assert_eq!(
        c.seen.data_frames, before_detach,
        "no data frames may arrive after a detach"
    );

    // The session ran to completion regardless, and kept its history.
    let (from, bytes, _) = h
        .manager
        .scrollback(id, u64::MAX, 4096)
        .expect("the session outlived the detach");
    assert_eq!(from, 0);
    assert_eq!(bytes, b"\r\nfirst\r\n\r\nsecond\r\n");
}

/// Re-attaching must resume the live stream at the current head.
///
/// The seq of the resumed frames is what lets the client work out the range it
/// missed while detached, so it must be the real cumulative offset rather than a
/// restart from zero.
#[cfg(not(windows))]
#[tokio::test]
async fn re_attaching_resumes_at_the_current_offset() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x; echo one; read -r y; echo two")).await;

    c.attach(id, 80, 24).await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the first line", |s| s.bytes(id).ends_with(b"one\r\n"))
        .await;
    let seen_so_far = c.seen.bytes(id).len() as u64;
    assert_eq!(seen_so_far, 7, "the echoed \"\\r\\n\" plus \"one\\r\\n\"");

    c.send(ClientMsg::Detach { session: id }).await;
    c.attach(id, 80, 24).await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the second line", |s| s.bytes(id).ends_with(b"two\r\n"))
        .await;
    // The client sees a contiguous stream because it stayed attached across the
    // gap in wall time; the offsets are what prove continuity.
    assert_eq!(c.seen.bytes(id), b"\r\none\r\n\r\ntwo\r\n");
}

/// Attaching to a session that does not exist must be an error naming the id.
///
/// A stale id after a close is ordinary, and a silent failure would leave the
/// client waiting for output that can never arrive.
#[tokio::test]
async fn attaching_to_an_unknown_session_errors() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    // Sent raw rather than through the attach helper: the whole point is that no
    // acknowledgement is coming.
    c.send(ClientMsg::Attach {
        session: vitrum_proto::SessionId(404),
        cols: 80,
        rows: 24,
    })
    .await;
    c.until("the error", |s| !s.errors().is_empty()).await;
    assert!(
        c.seen.errors()[0].contains("404"),
        "unhelpful error: {:?}",
        c.seen.errors()[0]
    );
}

/// Attaching must apply the client's viewport to the PTY.
///
/// The attaching client is the one about to draw, so its geometry wins. Without
/// this, a session created at a default size renders at that size forever.
#[cfg(not(windows))]
#[tokio::test]
async fn attaching_applies_the_client_viewport() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x; stty size")).await;

    c.attach(id, 111, 37).await;
    match c.seen.find(|m| matches!(m, ServerMsg::SessionUpdated(_))) {
        Some(ServerMsg::SessionUpdated(info)) => {
            assert_eq!((info.cols, info.rows), (111, 37));
        }
        other => panic!("expected SessionUpdated, got {other:?}"),
    }

    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the child's view of its size", |s| {
        s.bytes(id).ends_with(b"\r\n") && s.bytes(id).len() > 2
    })
    .await;
    assert_eq!(
        c.seen.bytes(id),
        b"\r\n37 111\r\n",
        "the child must see the attaching client's geometry"
    );
}

/// Attaching must acknowledge the unread flag, since the client is now watching.
#[tokio::test]
async fn attaching_clears_unread() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "echo noticed")).await;
    c.until("the exit", |s| {
        s.has(|m| matches!(m, ServerMsg::Exited { .. }))
    })
    .await;
    assert!(
        h.manager.info(id).expect("info").unread,
        "output arrived before anyone attached"
    );

    c.attach(id, 80, 24).await;
    assert!(
        !c.seen.last_update().unread,
        "the projection sent by this attach must show the session as read"
    );
}

/// Two clients attached to one session must both receive its output.
///
/// Sessions are daemon-owned, so a second window on the same agent is a view, not
/// a takeover.
#[cfg(not(windows))]
#[tokio::test]
async fn two_attached_clients_both_receive_output() {
    let h = Harness::start(64 * 1024).await;
    let mut a = h.greeted().await;
    let id = a.create(create(1, "read -r x; echo shared")).await;

    let mut b = h.greeted().await;
    for c in [&mut a, &mut b] {
        c.attach(id, 80, 24).await;
    }

    a.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    a.until("a's copy", |s| s.bytes(id).ends_with(b"shared\r\n"))
        .await;
    b.until("b's copy", |s| s.bytes(id).ends_with(b"shared\r\n"))
        .await;
    assert_eq!(a.seen.bytes(id), b.seen.bytes(id));
}

/// Escape sequences must cross the socket as real bytes.
///
/// The data plane exists so PTY output is never text. If a control byte reached
/// the client as its printable spelling, the pane would show `\033[32m` instead of
/// colouring a word, which is the visible symptom of a transport that decoded
/// bytes it had no business decoding.
#[cfg(not(windows))]
#[tokio::test]
async fn escape_sequences_cross_the_socket_as_real_bytes() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(1, "read -r x; printf '\\033[32mgreen\\033[0m'"))
        .await;
    c.attach(id, 80, 24).await;
    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the coloured output", |s| s.bytes(id).ends_with(b"[0m"))
        .await;
    assert_eq!(c.seen.bytes(id), b"\r\n\x1b[32mgreen\x1b[0m");
    assert_eq!(
        c.seen.bytes(id).iter().filter(|b| **b == 0x1b).count(),
        2,
        "two real ESC bytes arrived, not eleven printable characters"
    );
}

/// An escape sequence split across two data frames must reassemble exactly on the
/// client.
///
/// The child emits a lone ESC, waits for input so the coalescing window provably
/// closes and the frame is sent, then emits the remainder. A client concatenating
/// frame payloads by offset must end up with the original bytes: a boundary that
/// dropped or duplicated one byte would leave the emulator parsing the following
/// output as escape parameters and corrupt the whole screen, not one character.
#[cfg(not(windows))]
#[tokio::test]
async fn an_escape_split_across_data_frames_reassembles() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(
            1,
            "read -r x; printf '\\033'; read -r y; printf '[31mred\\033[0m'",
        ))
        .await;
    c.attach(id, 80, 24).await;

    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the bare ESC", |s| s.bytes(id).ends_with(b"\x1b")).await;
    let frames_after_esc = c.seen.data_frames;
    assert_eq!(c.seen.bytes(id), b"\r\n\x1b");

    c.send(ClientMsg::Input {
        session: id,
        data: b"\n".to_vec(),
    })
    .await;
    c.until("the rest of the sequence", |s| {
        s.bytes(id).ends_with(b"[0m")
    })
    .await;
    assert!(
        c.seen.data_frames > frames_after_esc,
        "the remainder must arrive in a later frame, or the split is not tested"
    );
    assert_eq!(c.seen.bytes(id), b"\r\n\x1b\r\n[31mred\x1b[0m");
}
