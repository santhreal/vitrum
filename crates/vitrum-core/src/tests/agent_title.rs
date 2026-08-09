//! Who owns a session's name when the program behind it is an agent TUI.
//!
//! An agent TUI treats the terminal title bar as a status line and rewrites it
//! every turn: Gemini writes `Ready (kernel-notes)`, Codex writes
//! `[ ! ] Action Required`. A shell titles its terminal with what it is running,
//! which is the best name that session will ever have. The daemon reads both
//! off the same escape sequence, so the difference has to come from knowing
//! which program is on the other end.
//!
//! The class this closes: an agent's status text reaching the sidebar as a row
//! NAME. It shipped as a row reading `Ready (kernel-n…` beside a pill already
//! saying Ready, and as a row whose name changed every time the agent changed
//! what it was doing. Both surfaces are asserted here, because the announced
//! title still has to be recorded for the status resolver even when it is
//! refused as a name -- dropping it instead would have "fixed" the name and
//! silently disabled approval detection.
//!
//! What it does not catch: whether the model then reads the recorded title
//! correctly (that is `vitrum-model`'s title-claim suite), and Windows, where
//! ConPTY's own preamble title is the first thing every session sees.
//!
//! Unix only, like every escape-driven test in this suite: `cmd.exe /C` has no
//! portable way to emit an ESC byte, so on Windows these would test the shell.

#![cfg(not(windows))]

use std::path::PathBuf;

use vitrum_proto::{ProjectId, SessionId};

use crate::tests::helpers::{DEADLINE, wait_exit};
use crate::{SessionManager, SessionSpec};

/// A directory holding a copy of the platform shell under `name`.
///
/// The agent rule keys on the command's basename, and the daemon has to
/// actually execute that command, so proving the rule end to end needs a real
/// executable that really is called `codex`. A symlink to `/bin/sh` is one:
/// `AgentKind::of` sees `codex`, and the child is a shell that can emit the
/// escape sequence the test is about.
///
/// Returned by value so the directory outlives the session. It is deleted when
/// the test drops it.
struct FakeAgent {
    dir: PathBuf,
    command: String,
}

impl FakeAgent {
    fn named(name: &str) -> Self {
        // Unique per instance, not per name: several cases here run the same
        // agent name concurrently under one test binary, and a shared
        // directory made them race for the same symlink.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "vitrum-agent-title-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let command = dir.join(name);
        std::os::unix::fs::symlink("/bin/sh", &command).expect("symlink a shell under the name");
        FakeAgent {
            dir,
            command: command.to_string_lossy().into_owned(),
        }
    }

    /// A spec that runs `script` through this fake agent.
    fn spec(&self, script: &str) -> SessionSpec {
        SessionSpec {
            project_id: ProjectId(7),
            cwd: std::env::temp_dir(),
            command: self.command.clone(),
            args: vec!["-c".to_string(), script.to_string()],
            env: Vec::new(),
            cols: 80,
            rows: 24,
            title: None,
        }
    }
}

impl Drop for FakeAgent {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Announce `title`, then hold the session open long enough to read it.
///
/// The escape is followed by a read rather than an exit because a session that
/// has exited has already had its projection frozen, and the bug being guarded
/// is about what the sidebar shows while the agent is sitting there waiting.
fn announce(title: &str) -> String {
    format!("printf '\\033]2;{title}\\007'; read -r _")
}

/// Wait until `f` holds of the session's projection, or fail with what it was.
///
/// A title crosses two threads on its way to the projection, so reading it the
/// instant after spawning races the reader. A wrong answer never converges, so
/// this cannot mask one: it fails with the value the session kept.
async fn until(
    mgr: &SessionManager,
    id: SessionId,
    what: &str,
    f: impl Fn(&vitrum_proto::SessionInfo) -> bool,
) -> vitrum_proto::SessionInfo {
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        let info = mgr.info(id).expect("info");
        if f(&info) {
            return info;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "{what}: session was still name={:?} announced={:?}",
                info.title, info.term_title
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// An agent's title is recorded, and is not the session's name.
///
/// This is the shipped defect, in the exact shape it shipped: Gemini's title is
/// its own status line, and taking it as a name put `Ready (kernel-notes)` in
/// the sidebar next to a Ready pill.
#[tokio::test]
async fn an_agents_title_is_recorded_without_becoming_its_name() {
    let agent = FakeAgent::named("gemini");
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(agent.spec(&announce("Ready (kernel-notes)")))
        .expect("spawn");

    let info = until(&mgr, id, "the title should have been recorded", |info| {
        info.term_title.is_some()
    })
    .await;

    assert_eq!(
        info.term_title.as_deref(),
        Some("Ready (kernel-notes)"),
        "the announced title must be recorded verbatim, because the status \
         resolver reads it"
    );
    assert_ne!(
        info.title, "Ready (kernel-notes)",
        "an agent's status line must never become the session's name"
    );

    mgr.close(id).expect("close");
}

/// Codex's approval banner reaches the projection whole.
///
/// The banner is the one title that carries a state the sidebar must act on, so
/// it gets its own case: a rule that recorded titles but mangled this one would
/// pass the test above and still leave a blocked session reading Ready.
#[tokio::test]
async fn the_approval_banner_reaches_the_projection_verbatim() {
    let agent = FakeAgent::named("codex");
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(agent.spec(&announce("[ ! ] Action Required - codex")))
        .expect("spawn");

    let info = until(&mgr, id, "the banner should have been recorded", |info| {
        info.term_title.is_some()
    })
    .await;

    assert_eq!(
        info.term_title.as_deref(),
        Some("[ ! ] Action Required - codex"),
        "the approval banner is what makes a blocked Codex session visible"
    );

    mgr.close(id).expect("close");
}

/// A shell still names itself.
///
/// The fix must not cost the feature it is narrowing. `ssh prod` and a long
/// build both start life called `sh`, and the program's title is the only thing
/// that knows better.
#[tokio::test]
async fn a_shell_still_takes_its_title_as_a_name() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(crate::tests::helpers::shell_spec(&announce("deploy")))
        .expect("spawn");

    let info = until(&mgr, id, "the shell should have been renamed", |info| {
        info.title == "deploy"
    })
    .await;

    assert_eq!(
        info.term_title.as_deref(),
        Some("deploy"),
        "a shell's title is recorded as well as taken, so both channels agree"
    );

    mgr.close(id).expect("close");
}

/// An agent that changes what it is doing does not change its name.
///
/// This is the half of the defect a single-title test cannot see. The sidebar
/// is a list someone scans; a row that renames itself every turn cannot be
/// found twice, and sorting it is meaningless.
#[tokio::test]
async fn an_agents_name_survives_the_titles_it_publishes() {
    let agent = FakeAgent::named("codex");
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(agent.spec(
            "printf '\\033]2;[ ! ] Action Required - codex\\007'; read -r _; \
             printf '\\033]2;Working\\007'; read -r _",
        ))
        .expect("spawn");

    let first = until(&mgr, id, "the banner should have been recorded", |info| {
        info.term_title.as_deref() == Some("[ ! ] Action Required - codex")
    })
    .await;

    mgr.write(id, b"\n").expect("advance the child");

    let second = until(&mgr, id, "the second title should have arrived", |info| {
        info.term_title.as_deref() == Some("Working")
    })
    .await;

    assert_eq!(
        first.title, second.title,
        "the name must not follow the agent's status line"
    );

    mgr.close(id).expect("close");
}

/// A name the operator pinned is not touched, and neither is the record.
///
/// Renaming already took the name away from the program. It must not also take
/// away approval detection: a session the operator named is exactly the one
/// they are watching.
#[tokio::test]
async fn a_renamed_agent_still_reports_what_it_announces() {
    let agent = FakeAgent::named("codex");
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(agent.spec(&announce("[ ! ] Action Required - codex")))
        .expect("spawn");
    mgr.rename(id, "the risky one").expect("rename");

    let info = until(&mgr, id, "the banner should have been recorded", |info| {
        info.term_title.is_some()
    })
    .await;

    assert_eq!(info.title, "the risky one", "the operator's name stands");
    assert_eq!(
        info.term_title.as_deref(),
        Some("[ ! ] Action Required - codex"),
        "a renamed session must still be able to say it is blocked"
    );

    mgr.close(id).expect("close");
}

/// Clearing the title retracts the claim without blanking the name.
///
/// Programs clear the title on the way out. The name has always been protected
/// from that; the record must NOT be, because a cleared title is a real
/// retraction and a stale `Action Required` would pin a finished session to the
/// top of the sidebar forever.
#[tokio::test]
async fn clearing_the_title_retracts_the_claim_and_keeps_the_name() {
    let agent = FakeAgent::named("codex");
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(agent.spec(
            "printf '\\033]2;[ ! ] Action Required - codex\\007'; read -r _; \
             printf '\\033]2;\\007'; read -r _",
        ))
        .expect("spawn");

    let named = until(&mgr, id, "the banner should have been recorded", |info| {
        info.term_title.is_some()
    })
    .await;

    mgr.write(id, b"\n").expect("advance the child");

    let cleared = until(&mgr, id, "the title should have been cleared", |info| {
        info.term_title.is_none()
    })
    .await;

    assert_eq!(
        named.title, cleared.title,
        "clearing a title must not blank the row"
    );

    mgr.close(id).expect("close");
}

/// The name an agent session gets is derived from its command.
///
/// With the title refused, something else has to name the row, and it must be
/// the thing that does not change: a row called `codex` is findable, and a row
/// called `[ . ] Action Requir…` is not.
#[tokio::test]
async fn an_agent_session_is_named_for_its_command() {
    let agent = FakeAgent::named("codex");
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(agent.spec(&announce("[ ! ] Action Required - codex")))
        .expect("spawn");

    let info = until(&mgr, id, "the banner should have been recorded", |info| {
        info.term_title.is_some()
    })
    .await;

    assert!(
        info.title.contains("codex"),
        "an agent row should be named for the agent, was {:?}",
        info.title
    );

    mgr.close(id).expect("close");
}

/// EVERY agent this build knows refuses to be named by its title.
///
/// Membership comes from [`ALL_AGENT_KINDS`] and an exhaustive match, never
/// from `title_is_a_name` itself. Filtering on the property under test is how
/// this test first passed against the defect: flipping the rule to "everything
/// names itself" simply emptied the loop, and a guard that a mutation can
/// silence by agreeing with it is not a guard.
///
/// So a new agent has two ways to fail here and no way to slip past: the match
/// stops compiling, and if it is added as an agent the assertion runs on it.
///
/// [`ALL_AGENT_KINDS`]: vitrum_model::ALL_AGENT_KINDS
#[tokio::test]
async fn no_known_agent_is_named_by_its_title() {
    for kind in vitrum_model::ALL_AGENT_KINDS {
        // Exhaustive: the command each kind resolves from, or `None` for the
        // two kinds that are not agents at all.
        let command = match kind {
            vitrum_model::AgentKind::Claude => Some("claude"),
            vitrum_model::AgentKind::Codex => Some("codex"),
            vitrum_model::AgentKind::Gemini => Some("gemini"),
            vitrum_model::AgentKind::Opencode => Some("opencode"),
            vitrum_model::AgentKind::Veyyon => Some("veyyon"),
            // A shell and an unrecognised program are the two kinds whose
            // title IS their best name, which `a_shell_still_takes_its_title`
            // covers from the other side.
            vitrum_model::AgentKind::Shell | vitrum_model::AgentKind::Unknown => None,
        };
        let Some(command) = command else {
            continue;
        };

        let agent = FakeAgent::named(command);
        let mgr = SessionManager::new(4096);
        let id = mgr
            .spawn(agent.spec(&announce("Ready (somewhere)")))
            .expect("spawn");

        let info = until(&mgr, id, "the title should have been recorded", |info| {
            info.term_title.is_some()
        })
        .await;

        assert_ne!(
            info.title, "Ready (somewhere)",
            "{command} took its status line as a name"
        );

        mgr.close(id).expect("close");
    }
}

/// A session that never titles itself records nothing.
///
/// `None` has to stay distinguishable from "announced an empty string", because
/// the status resolver treats the absence of a claim as no evidence rather than
/// as a retraction of one.
#[tokio::test]
async fn a_silent_program_announces_nothing() {
    let mgr = SessionManager::new(4096);
    let id = mgr
        .spawn(crate::tests::helpers::shell_spec("exit 0"))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    assert_eq!(
        mgr.info(id).expect("info").term_title,
        None,
        "a program that set no title must not appear to have set one"
    );
}
