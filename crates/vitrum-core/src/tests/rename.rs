//! Renaming a session: the daemon owns the name, so the daemon validates it.

use vitrum_proto::{SessionId, SessionStatus};

use crate::SessionManager;
#[cfg(not(windows))]
use crate::tests::helpers::collect;
use crate::tests::helpers::{shell_spec, wait_exit};

/// A rename must replace the generated title.
///
/// The title is the only thing distinguishing twenty rows that all say `claude`,
/// so a rename that did not stick would make the sidebar unusable at exactly
/// the scale this product is for.
#[tokio::test]
async fn a_rename_replaces_the_generated_title() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    assert_eq!(mgr.info(id).expect("info").title, "sh");
    mgr.rename(id, "auth refactor").expect("rename");
    assert_eq!(mgr.info(id).expect("info").title, "auth refactor");
    mgr.close(id).expect("close");
}

/// An all-whitespace title must be refused, and must not clobber the old one.
///
/// A row you cannot identify is worse than one with a generated name, and a
/// blank row is invisible: it reads as a rendering bug rather than as a session
/// somebody named badly.
#[tokio::test]
async fn a_whitespace_only_title_is_refused() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    mgr.rename(id, "keeper").expect("rename");

    for blank in ["", " ", "\t", "\n", "   \t \n  ", "\u{0b}\u{0c}\r"] {
        let err = mgr
            .rename(id, blank)
            .expect_err("a blank title must be refused");
        assert!(
            err.to_string().contains("empty"),
            "unhelpful error for {blank:?}: {err}"
        );
        assert_eq!(
            mgr.info(id).expect("info").title,
            "keeper",
            "a refused rename must leave the old title in place"
        );
    }
    mgr.close(id).expect("close");
}

/// Surrounding whitespace must be trimmed rather than stored.
///
/// A title pasted from anywhere carries a trailing newline, and storing it
/// leaves a row whose name silently differs from every string a user might
/// compare it against.
#[tokio::test]
async fn a_title_is_trimmed() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    mgr.rename(id, "  db migration \n").expect("rename");
    assert_eq!(mgr.info(id).expect("info").title, "db migration");
    mgr.close(id).expect("close");
}

/// Interior whitespace and non-ASCII must survive untouched.
///
/// Trimming is about the edges. A title that collapsed inner spaces, or mangled
/// a non-ASCII name, would be a different kind of surprise.
#[tokio::test]
async fn interior_text_survives_the_trim() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    mgr.rename(id, "  refactor:  auth ⇢ session  ")
        .expect("rename");
    assert_eq!(
        mgr.info(id).expect("info").title,
        "refactor:  auth ⇢ session"
    );
    mgr.close(id).expect("close");
}

/// Renaming an unknown session must be a named error rather than a panic.
///
/// A client racing a close against a rename is ordinary, and the id has to be
/// in the message or the error is untraceable across twenty sessions.
#[tokio::test]
async fn renaming_an_unknown_session_errors() {
    let mgr = SessionManager::new(1024);
    let err = mgr.rename(SessionId(404), "ghost").expect_err("must fail");
    assert!(err.to_string().contains("404"), "unhelpful error: {err}");
}

/// A rename must not disturb scrollback or the child.
///
/// The name is metadata. If renaming touched the byte stream it would corrupt
/// the terminal for a purely cosmetic action, and that is precisely the sort of
/// thing nobody would think to test until a user reported garbled output after
/// renaming a tab.
#[cfg(not(windows))]
#[tokio::test]
async fn a_rename_disturbs_neither_scrollback_nor_the_child() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec("echo before; read -r x; echo after=$x"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"before\r\n")).await;
    let (before_seq, before_bytes, _) = mgr.scrollback(id, u64::MAX, 4096).expect("scrollback");

    mgr.rename(id, "renamed mid-stream").expect("rename");

    let (after_seq, after_bytes, _) = mgr.scrollback(id, u64::MAX, 4096).expect("scrollback");
    assert_eq!(
        (before_seq, &before_bytes),
        (after_seq, &after_bytes),
        "a rename must not add, drop, or renumber a single byte"
    );

    // The child is still there and still listening.
    mgr.write(id, b"alive\n").expect("write");
    c.until(|b| b.ends_with(b"after=alive\r\n")).await;
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    assert_eq!(
        mgr.info(id).expect("info").title,
        "renamed mid-stream",
        "the name must survive the child it was applied to"
    );
    assert_eq!(
        c.bytes, b"before\r\nalive\r\nafter=alive\r\n",
        "the stream is exactly what the child wrote plus the echo"
    );
}

/// A rename must not change anything else in the projection.
///
/// Renaming is one field. A rename that also cleared unread, or reset the
/// attention state, would make the sidebar forget which agents need you the
/// moment you tidied up your tab names.
#[cfg(not(windows))]
#[tokio::test]
async fn a_rename_changes_only_the_title() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec("printf 'ding\\007'; read -r x"))
        .expect("spawn");
    let before = crate::tests::helpers::probe_now(&mgr, id).await;
    assert!(before.unread, "nothing is attached, so this is unread");
    assert!(before.attention.bell, "the child rang the bell");

    mgr.rename(id, "still unread").expect("rename");
    let after = mgr.info(id).expect("info");
    assert_eq!(after.title, "still unread");
    assert!(after.unread);
    assert!(after.attention.bell);
    assert_eq!(after.attention.waiting, before.attention.waiting);
    assert_eq!((after.cols, after.rows), (before.cols, before.rows));
    assert_eq!(after.created_at_ms, before.created_at_ms);
    assert_eq!(after.status, before.status);
    mgr.close(id).expect("close");
}

/// An exited session must still be renameable.
///
/// A finished run is exactly what you want to label before you come back to it,
/// and refusing would be an arbitrary restriction with no benefit.
#[tokio::test]
async fn an_exited_session_can_still_be_renamed() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("exit 0")).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    mgr.rename(id, "finished run").expect("rename");
    let info = mgr.info(id).expect("info");
    assert_eq!(info.title, "finished run");
    assert_eq!(info.status, SessionStatus::Exited { code: Some(0) });
}

/// A closed session must refuse a rename, because it is gone.
#[tokio::test]
async fn a_closed_session_cannot_be_renamed() {
    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    mgr.close(id).expect("close");
    let err = mgr.rename(id, "too late").expect_err("must fail");
    assert!(
        err.to_string().contains(&id.0.to_string()),
        "unhelpful error: {err}"
    );
}
