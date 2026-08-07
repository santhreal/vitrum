//! What a session is called and where it is, as the terminal reports them.
//!
//! The daemon runs the terminal engine over its own sessions' output, so a
//! program that names itself is named that way in the sidebar with no client
//! attached and no client involvement, and a shell that changes directory is
//! shown where it went. These tests are the contract for who wins when the
//! program and the operator disagree, and for which reports are believed.
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

// Every test below drives a real shell through an escape sequence, which is
// why they are all `not(windows)`; the imports they need are gated with them.
#[cfg(not(windows))]
use crate::SessionManager;
#[cfg(not(windows))]
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

/// A shell that moves is shown where it went.
///
/// OSC 7 is how a shell says so, and the daemon is the only thing positioned to
/// hear it: it sees every session's bytes whether or not a window is open.
#[cfg(not(windows))]
#[tokio::test]
async fn a_reported_directory_moves_the_session() {
    let dir = crate::tests::helpers::TempDir::new("vitrum-osc7-moved");
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec(&format!(
            "printf '\\033]7;file://host{}\\007'",
            dir.path.display()
        )))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(
        mgr.info(id).expect("info").cwd,
        dir.path.to_string_lossy(),
        "the session should be where the shell said it is"
    );
}

/// A directory that is not on this machine is not believed.
///
/// A session inside `ssh` reports the remote machine's paths, and adopting one
/// would tell the operator the session is somewhere it has never been and send
/// the branch lookup after a directory that does not exist.
#[cfg(not(windows))]
#[tokio::test]
async fn a_directory_this_machine_does_not_have_is_ignored() {
    let mgr = SessionManager::new(4096);
    let started = std::env::temp_dir().to_string_lossy().into_owned();
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033]7;file://remote/definitely/not/here/at/all\\007'",
        ))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(
        mgr.info(id).expect("info").cwd,
        started,
        "an unreachable directory should leave the session where it was"
    );
}

/// Moving into a repository shows that repository's branch.
///
/// The branch is resolved from the directory, so a session that walked into
/// another checkout has to stop reporting the branch of the one it left.
#[cfg(not(windows))]
#[tokio::test]
async fn moving_into_a_repository_picks_up_its_branch() {
    let dir = crate::tests::helpers::TempDir::new("vitrum-osc7-branch");
    let git = dir.path.join(".git");
    std::fs::create_dir_all(&git).expect("create .git");
    std::fs::write(git.join("HEAD"), "ref: refs/heads/topic\n").expect("write HEAD");

    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(shell_spec(&format!(
            "printf '\\033]7;file://host{}\\007'",
            dir.path.display()
        )))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(
        mgr.info(id).expect("info").git_branch.as_deref(),
        Some("topic"),
        "the branch should come from the directory the session moved into"
    );
}
