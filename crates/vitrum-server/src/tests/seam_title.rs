//! A name, a status line, and an operator's pin, all the way to a client.
//!
//! Three facts share one wire struct here, and the shipped defect was two of
//! them being the same field. `SessionInfo::title` is the session's NAME: what
//! the sidebar row is called, what an operator may pin, what a shell is allowed
//! to write. `SessionInfo::term_title` is whatever the program last put in the
//! terminal title bar, which for an agent TUI is a status line rewritten every
//! turn. Reading one as the other put `Ready (kernel-n…` in the sidebar beside
//! a pill already saying Ready, and renamed the row on every turn so it could
//! not be found twice.
//!
//! `vitrum-core`'s `agent_title.rs` proves the rule against `SessionManager`.
//! This proves it against a websocket, which is a different claim: the
//! projection a client renders is built and serialized on a path of its own,
//! and a field that is correct in the manager and swapped, dropped or defaulted
//! on the wire looks exactly like the original defect to the operator.
//!
//! Unix only, like every escape-driven case in this suite: `cmd.exe /C` has no
//! portable way to emit an ESC byte, and there is no symlink-to-shell trick
//! that gives a Windows child an agent's basename and a working `-c`.

#![cfg(not(windows))]

use vitrum_model::{ALL_AGENT_KINDS, AgentKind};
use vitrum_proto::SessionInfo;

use crate::tests::client::{FakeAgent, Harness, create};

/// Gemini's status line, in the shape that shipped as a sidebar row name.
const AGENT_STATUS_LINE: &str = "Ready (kernel-notes)";

/// The command each kind resolves from, as an EXHAUSTIVE match.
///
/// Exhaustive rather than a lookup so a new [`AgentKind`] stops this module
/// compiling until someone writes down which basename it answers to. A
/// hand-kept list inside one test goes stale in silence, which is the same
/// failure as having no test.
///
/// `Unknown` gets a name this build recognises as nothing, which is a real
/// answer and not a fallback: an unrecognised program is far more likely to be
/// an ordinary one that titles itself sensibly.
pub(crate) fn command_for(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "claude",
        AgentKind::Codex => "codex",
        AgentKind::Gemini => "gemini",
        AgentKind::Opencode => "opencode",
        AgentKind::Veyyon => "veyyon",
        AgentKind::Shell => "sh",
        AgentKind::Unknown => "not-an-agent-this-build-knows",
    }
}

/// Whether this kind's terminal title is the session's name, as an EXHAUSTIVE
/// match written from the product decision rather than read from the code under
/// test.
///
/// Deriving this from [`AgentKind::title_is_a_name`] would make the assertions
/// below tautologies: flipping the rule to "everything names itself" would flip
/// the expectation with it and the suite would stay green against the exact
/// defect it exists to catch.
fn title_names_the_session(kind: AgentKind) -> bool {
    match kind {
        // Agent TUIs treat the title bar as a status line.
        AgentKind::Claude
        | AgentKind::Codex
        | AgentKind::Gemini
        | AgentKind::Opencode
        | AgentKind::Veyyon => false,
        // A shell titles itself with what it is running, which is the best name
        // that session will ever have; an unrecognised program is assumed to do
        // the same, because the cost of guessing wrong is a worse name rather
        // than a wrong status.
        AgentKind::Shell | AgentKind::Unknown => true,
    }
}

/// Announce `title`, then hold the session open.
///
/// A read rather than an exit: a session that has exited has already had its
/// projection frozen, and the bug being guarded is about what the sidebar shows
/// while the agent is sitting there waiting.
fn announce(title: &str) -> String {
    format!("printf '\\033]2;{title}\\007'; read -r _")
}

/// Every agent's announced title reaches a client as `term_title`, and never as
/// the row's name.
///
/// WHY: this is the shipped defect at the seam it is visible from. Both halves
/// have to be asserted together, because dropping the title instead of refusing
/// it as a name would "fix" the sidebar and silently disable the status
/// resolver, which reads `term_title` and nothing else. That would have traded
/// a cosmetic bug for a blocked agent reading Ready.
///
/// Membership comes from [`ALL_AGENT_KINDS`] and two exhaustive matches, so a
/// new agent has two ways to fail here and no way to slip past: the matches
/// stop compiling, and once they are filled in the assertions run on it.
/// Nothing is filtered on the property under test — filtering on
/// `title_is_a_name` is how the original defect passed a first attempt at this
/// test, because flipping the rule simply emptied the loop.
///
/// What this does NOT catch: whether the client then RESOLVES the recorded
/// title correctly, which `seam_status.rs` owns; and Windows, where ConPTY's
/// own preamble title is the first thing every session sees.
#[tokio::test]
async fn every_agents_announced_title_is_recorded_and_only_a_shell_is_named_by_it() {
    for kind in ALL_AGENT_KINDS {
        let command = command_for(kind);
        assert_eq!(
            AgentKind::of(command),
            kind,
            "the command this test runs for {kind:?} does not resolve back to it, \
             so every assertion below would be about the wrong agent"
        );

        let agent = FakeAgent::named(command);
        let h = Harness::start(1 << 16).await;
        let mut c = h.greeted().await;
        let id = c
            .create(agent.create(1, &announce(AGENT_STATUS_LINE)))
            .await;

        let info: SessionInfo = c
            .until_projection("the announced title", id, |i| i.term_title.is_some())
            .await;

        assert_eq!(
            info.term_title.as_deref(),
            Some(AGENT_STATUS_LINE),
            "{command}: the announced title must reach the client verbatim, \
             because the status resolver reads this field and no other"
        );

        if title_names_the_session(kind) {
            assert_eq!(
                info.title, AGENT_STATUS_LINE,
                "{command}: a program whose title IS its name must be renamed by it"
            );
        } else {
            assert_eq!(
                info.title, command,
                "{command}: an agent's status line reached the sidebar as the row's NAME"
            );
        }

        h.manager.close(id).expect("close");
    }
}

/// An agent that changes what it is doing must not change its name.
///
/// WHY: this is the half of the defect a single-title test cannot see. The
/// sidebar is a list someone scans; a row that renames itself every turn cannot
/// be found twice and sorting it is meaningless. The client sees a sequence of
/// projections rather than a final state, so the assertion is on every one of
/// them: not one may carry a title the agent published.
///
/// What this does NOT catch: a rename the operator performed, which is the next
/// case.
#[tokio::test]
async fn an_agents_name_survives_every_title_it_publishes() {
    let agent = FakeAgent::named("codex");
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c
        .create(agent.create(
            1,
            "printf '\\033]2;Working\\007'; read -r _; \
             printf '\\033]2;[ ! ] Action Required\\007'; read -r _; \
             printf '\\033]2;Ready (notes)\\007'; read -r _",
        ))
        .await;

    for (step, want) in [
        ("Working", "Working"),
        ("the approval banner", "[ ! ] Action Required"),
        ("the ready line", "Ready (notes)"),
    ] {
        c.until_projection(step, id, |i| i.term_title.as_deref() == Some(want))
            .await;
        c.input(id, b"\n").await;
    }

    let announced: Vec<String> = c
        .seen
        .ctl
        .iter()
        .filter_map(|m| match m {
            vitrum_proto::ServerMsg::SessionCreated(i)
            | vitrum_proto::ServerMsg::SessionUpdated(i)
                if i.id == id =>
            {
                i.term_title.clone()
            }
            _ => None,
        })
        .collect();
    assert!(
        announced.iter().any(|t| t == "Working")
            && announced.iter().any(|t| t == "[ ! ] Action Required")
            && announced.iter().any(|t| t == "Ready (notes)"),
        "all three announcements must have reached the client: {announced:?}"
    );

    for info in c.seen.ctl.iter().filter_map(|m| match m {
        vitrum_proto::ServerMsg::SessionCreated(i) | vitrum_proto::ServerMsg::SessionUpdated(i)
            if i.id == id =>
        {
            Some(i)
        }
        _ => None,
    }) {
        assert_eq!(
            info.title, "codex",
            "the row renamed itself when the agent restated what it was doing"
        );
    }

    h.manager.close(id).expect("close");
}

/// A shell's title becomes its name, over the wire and on every change.
///
/// WHY: the fix must not cost the feature it narrows. `ssh prod` and a long
/// build both start life called `sh`, and the program's title is the only thing
/// that knows better. A rule that refused every title would have made the
/// sidebar strictly worse than before the defect.
///
/// What this does NOT catch: a shell this build's `SHELLS` table does not list,
/// which resolves to `Unknown` and is named by its title for a different
/// reason.
#[tokio::test]
async fn a_shells_title_becomes_its_name_on_the_wire() {
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(1, &announce("vim src/main.rs")))
        .await;

    let info = c
        .until_projection("the shell's chosen name", id, |i| {
            i.title == "vim src/main.rs"
        })
        .await;
    assert_eq!(
        info.term_title.as_deref(),
        Some("vim src/main.rs"),
        "a shell that named itself must still have its announcement recorded, \
         because the two fields answer different questions"
    );
    h.manager.close(id).expect("close");
}

/// An operator rename pins the name and does NOT silence the title channel.
///
/// WHY: renaming already takes the name away from the program. It must not also
/// take away status detection: a session the operator bothered to name is
/// exactly the one they are watching, and a pin that muted `term_title` would
/// remove the state they renamed the row to follow. The rename travels as a
/// control message and the title arrives on the reader thread, so this is two
/// independent writers to one projection and the only place their interaction
/// is observable is a client.
///
/// A shell is used deliberately: it is the kind whose title WOULD otherwise
/// become its name, so the pin has something real to refuse.
///
/// What this does NOT catch: a rename racing an announcement in the other
/// order, which the daemon serialises under one lock and which has no
/// operator-visible difference.
#[tokio::test]
async fn a_rename_pins_the_name_without_silencing_the_announced_title() {
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c
        .create(create(
            1,
            "printf '\\033]2;before\\007'; read -r _; printf '\\033]2;after\\007'; read -r _",
        ))
        .await;

    c.until_projection("the shell's first name", id, |i| i.title == "before")
        .await;
    c.rename(id, "release cut").await;

    c.input(id, b"\n").await;
    let info = c
        .until_projection("the second announcement", id, |i| {
            i.term_title.as_deref() == Some("after")
        })
        .await;

    assert_eq!(
        info.title, "release cut",
        "an announcement overwrote a name the operator pinned"
    );
    assert_eq!(
        info.term_title.as_deref(),
        Some("after"),
        "the pin silenced the channel the status resolver reads"
    );
    h.manager.close(id).expect("close");
}

/// A program that never titles itself must report nothing, not an empty string.
///
/// WHY: `None` and `Some("")` mean different things to the resolver. Absence of
/// a claim is no evidence; an empty claim is a retraction of one. A serializer
/// that defaulted the field, or a client that read a missing field as empty,
/// would collapse the two and make every silent session look like it had just
/// cleared a banner.
///
/// What this does NOT catch: the retraction path itself, where a program clears
/// its title on the way out, which `vitrum-core` owns.
#[tokio::test]
async fn a_silent_program_announces_nothing_on_the_wire() {
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c.create(create(1, "read -r _")).await;

    // A barrier, not a sleep: the list is answered after the create, so any
    // projection the session had produced by then has already been delivered.
    c.barrier().await;
    let listed = c
        .seen
        .sessions()
        .expect("a list must have arrived")
        .iter()
        .find(|i| i.id == id)
        .expect("the session must be listed")
        .clone();
    assert_eq!(
        listed.term_title, None,
        "a program that set no title must not appear to have set one"
    );
    assert_eq!(
        listed.title, "sh",
        "an unnamed session is named for its command"
    );
    h.manager.close(id).expect("close");
}
