//! What a session is called, and who gets to decide.
//!
//! The daemon runs the terminal engine over its own sessions' output, so a
//! program that names itself is named that way in the sidebar with no client
//! attached and no client involvement. These tests are the contract for who
//! wins when the program and the operator disagree.
//!
//! The escape-emitting cases are Unix only for the same reason every other
//! escape test in this suite is: `cmd.exe /C` has no portable way to put an
//! ESC byte into its output, so on Windows they would test the shell rather
//! than the daemon.
//!
//! Windows reaches the same rules by a different road. ConPTY opens every
//! session with a preamble that includes an OSC naming the shell, so it is the
//! one platform where the engine is handed a title no program asked for. That
//! title goes through the guards below like any other: it cannot blank a name,
//! and it cannot touch a session the creator or the operator has named.

use crate::SessionManager;
use crate::tests::helpers::{shell_spec, wait_exit};

/// A program that titles itself is shown under that name.
///
/// This is the whole point of running the engine daemon-side: `ssh prod` and a
/// long build both start life called "sh", and only the program knows better.
#[cfg(not(windows))]
#[tokio::test]
async fn a_program_naming_itself_renames_the_session() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("printf '\\033]2;deploy\\007'"))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(
        mgr.info(id).expect("info").title,
        "deploy",
        "the title the program set should be the session's name"
    );
}

/// OSC 0 sets the title too.
///
/// It is the sequence that sets the icon name and the title together, and it
/// is what most shells actually emit from their prompt. Handling only OSC 2
/// would mean the feature did nothing for the common case.
#[cfg(not(windows))]
#[tokio::test]
async fn the_combined_icon_and_title_sequence_counts() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec("printf '\\033]0;both\\007'"))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(mgr.info(id).expect("info").title, "both");
}

/// A name chosen when the session was created is not the program's to take.
///
/// vitrum spawns named sessions on the operator's behalf, and a shell that
/// retitles itself on every prompt would erase that name within one command.
#[cfg(not(windows))]
#[tokio::test]
async fn a_name_the_creator_chose_survives_the_program() {
    let mgr = SessionManager::new(4096);
    let mut spec = shell_spec("printf '\\033]2;theirs\\007'");
    spec.title = Some("mine".to_string());
    let id = mgr.spawn(spec).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(
        mgr.info(id).expect("info").title,
        "mine",
        "a session the creator named should keep that name"
    );
}

/// Renaming takes the name away from the program for good.
///
/// The operator renames a session precisely because they did not like what it
/// was called. Letting the next prompt undo that would make the rename look
/// broken rather than overridden.
#[cfg(not(windows))]
#[tokio::test]
async fn a_rename_takes_the_name_away_from_the_program() {
    let mgr = SessionManager::new(4096);
    // The program only titles itself after it has been answered, so the rename
    // below is guaranteed to happen first.
    let id = mgr
        .spawn(shell_spec("read -r x; printf '\\033]2;theirs\\007'"))
        .expect("spawn");
    mgr.rename(id, "ours").expect("rename");
    mgr.write(id, b"go\n").expect("write");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(
        mgr.info(id).expect("info").title,
        "ours",
        "the operator's name should outlast the program's"
    );
}

/// Clearing the title leaves the name alone.
///
/// Programs clear the title on the way out, and an empty string is a valid
/// thing to receive. Honouring it would leave a blank row in the sidebar,
/// which is strictly worse than the name the session already had.
///
/// The two sequences are deliberately separated by an answer the test controls.
/// Written back to back they arrive in one read, and the engine only keeps the
/// last title from a burst, so the clear would erase the name before anything
/// could look at it and the test would pass without the guard existing.
#[cfg(not(windows))]
#[tokio::test]
async fn an_empty_title_does_not_blank_the_name() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033]2;named\\007'; read -r x; printf '\\033]2;\\007'",
        ))
        .expect("spawn");
    titled(&mgr, id, "named").await;

    mgr.write(id, b"go\n").expect("write");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(
        mgr.info(id).expect("info").title,
        "named",
        "clearing the title should not leave the session nameless"
    );
}

/// Wait until `id` is called `want`, or fail saying what it was called instead.
///
/// A title crosses two threads on its way to the projection, so a test that
/// reads it the instant after spawning is racing the reader. This never masks a
/// wrong answer: a name that is simply incorrect never converges and the
/// assertion fires with the value it kept.
#[cfg(not(windows))]
async fn titled(mgr: &SessionManager, id: vitrum_proto::SessionId, want: &str) {
    let deadline = std::time::Instant::now() + crate::tests::helpers::DEADLINE;
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        last = mgr.info(id).expect("info").title;
        if last == want {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("session was still called {last:?} rather than {want:?}");
}
