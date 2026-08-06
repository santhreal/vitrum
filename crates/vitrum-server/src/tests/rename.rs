//! Rename over the wire: the daemon owns the name, so every window sees it.

use vitrum_proto::{ClientMsg, ServerMsg};

use crate::tests::client::{Harness, create};

/// A rename must reach a SECOND window, not just the one that asked.
///
/// This is the whole reason renaming is a protocol operation instead of local
/// UI state. Two windows are independent views of one daemon; a title held in
/// one of them is invisible to the other and gone on restart, so the sidebar
/// would disagree with itself about what a session is called.
#[tokio::test]
async fn a_rename_by_one_client_is_seen_by_another() {
    let h = Harness::start(4096).await;
    let mut a = h.greeted().await;
    let mut b = h.greeted().await;
    let id = a.create(create(1, "read -r x")).await;

    // B learns the session exists through the same broadcast.
    b.until("the session to appear", |s| !s.created().is_empty())
        .await;
    assert_eq!(b.seen.created()[0].title, "sh");

    a.send(ClientMsg::Rename { session: id, title: "auth refactor".to_string().into() })
    .await;

    b.until("the rename", |s| {
        s.updates().iter().any(|i| i.title == "auth refactor")
    })
    .await;
    a.until("its own rename", |s| {
        s.updates().iter().any(|i| i.title == "auth refactor")
    })
    .await;
    assert_eq!(
        h.manager.info(id).expect("info").title,
        "auth refactor",
        "the daemon is the source of truth for the name"
    );
    h.manager.close(id).expect("close");
}

/// An all-whitespace title must be refused with a named error and change
/// nothing.
///
/// A blank row is invisible in the sidebar: it reads as a rendering bug rather
/// than as a session somebody named badly, and there is no way for the user to
/// tell which of twenty agents it is.
#[tokio::test]
async fn a_whitespace_only_rename_is_refused() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x")).await;
    c.send(ClientMsg::Rename { session: id, title: "keeper".to_string().into() })
    .await;
    c.until("the good rename", |s| {
        s.updates().iter().any(|i| i.title == "keeper")
    })
    .await;

    for blank in ["", "   ", "\t\n "] {
        c.send(ClientMsg::Rename { session: id, title: blank.to_string().into() })
        .await;
        c.until("the refusal", |s| {
            s.errors().iter().any(|m| m.contains("empty"))
        })
        .await;
        assert_eq!(
            h.manager.info(id).expect("info").title,
            "keeper",
            "a refused rename must leave the old title alone"
        );
        assert!(
            !c.seen.updates().iter().any(|i| i.title.trim().is_empty()),
            "a blank title must never be broadcast"
        );
    }
    h.manager.close(id).expect("close");
}

/// The refusal must name the session, so a client with twenty tabs can tell
/// which one it was.
#[tokio::test]
async fn a_rename_error_names_the_session() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r x")).await;
    c.send(ClientMsg::Rename { session: id, title: " ".to_string().into() })
    .await;
    c.until("the refusal", |s| !s.errors().is_empty()).await;
    match c.seen.find(|m| matches!(m, ServerMsg::Error { .. })) {
        Some(ServerMsg::Error { session, message, .. }) => {
            assert_eq!(*session, Some(id));
            assert!(message.contains(&id.0.to_string()), "unhelpful: {message}");
        }
        other => panic!("expected an Error, got {other:?}"),
    }
    h.manager.close(id).expect("close");
}

/// Renaming an unknown session must be an error rather than a panic, because a
/// rename racing a close is ordinary.
#[tokio::test]
async fn renaming_an_unknown_session_errors() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;
    c.send(ClientMsg::Rename { session: vitrum_proto::SessionId(404), title: "ghost".to_string().into() })
    .await;
    c.until("the refusal", |s| {
        s.errors().iter().any(|m| m.contains("404"))
    })
    .await;
}

/// A rename must disturb neither the byte stream nor the child.
///
/// The name is metadata. If renaming perturbed the data plane it would corrupt
/// the terminal for a purely cosmetic action, and nobody would connect the two
/// until a user reported garbled output after tidying up their tab names. This
/// renames mid-stream, with a client attached, and then requires the exact
/// bytes to be unbroken and the child to still answer.
#[cfg(not(windows))]
#[tokio::test]
async fn a_rename_disturbs_neither_the_stream_nor_the_child() {
    let h = Harness::start(64 * 1024).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(1, "echo before; read -r x; echo after=$x"))
        .await;
    c.attach(id, 80, 24).await;
    c.until("the first line", |s| s.bytes(id).ends_with(b"before\r\n"))
        .await;
    let seq_before = c.seen.first_seq(id);

    c.send(ClientMsg::Rename { session: id, title: "renamed mid-stream".to_string().into() })
    .await;
    c.until("the rename", |s| {
        s.updates().iter().any(|i| i.title == "renamed mid-stream")
    })
    .await;

    c.send(ClientMsg::Input { session: id, data: b"alive\n".to_vec().into() })
    .await;
    c.until("the child's reply", |s| {
        s.bytes(id).ends_with(b"after=alive\r\n")
    })
    .await;

    assert_eq!(
        c.seen.bytes(id),
        b"before\r\nalive\r\nafter=alive\r\n",
        "the stream must be exactly what the child wrote plus the echo"
    );
    assert_eq!(
        c.seen.first_seq(id),
        seq_before,
        "a rename must not renumber the stream"
    );
    // Scrollback agrees with what was streamed, so nothing was lost or doubled
    // behind the rename either.
    let (_, retained, _) = h
        .manager
        .scrollback(id, u64::MAX, 4096)
        .expect("scrollback");
    assert_eq!(retained, b"before\r\nalive\r\nafter=alive\r\n");
    h.manager.close(id).expect("close");
}
