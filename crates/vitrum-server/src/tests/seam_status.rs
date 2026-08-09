//! What the sidebar says, resolved from a projection that came off a socket.
//!
//! The status an operator reads is produced by three components that never meet
//! in a unit test: a child writes an escape into a real PTY, the daemon parses
//! it and folds it into a projection, and the CLIENT resolves that projection
//! into a state through [`SessionView::resolve_status`]. Each of those is
//! covered on its own. The seam between them is where the shipped defect lived:
//! a session blocked on an approval prompt reported `Ready`, because approval
//! was only reachable through the opt-in hint channel and two of the three
//! shipped agents do not send it.
//!
//! So every case here spawns a real child with a real agent basename, waits for
//! the projection a real client received, and asks the real resolver what that
//! row now says.
//!
//! Both dimensions are enumerated from the source at run time — [`HintState`]
//! from [`HintState::ALL`], agents from [`ALL_AGENT_KINDS`] — and both
//! expectation tables are exhaustive matches. Adding a state or an agent stops
//! this module compiling, and once the match is filled in the assertions run on
//! the new member. There is no way to add either and keep the suite green
//! without recording a decision.
//!
//! Unix only, for the reason `seam_title.rs` gives.

#![cfg(not(windows))]

use vitrum_model::{
    ALL_AGENT_KINDS, AgentKind, SessionView, SidebarStatus, StatusSource, TitleClaim,
};
use vitrum_proto::HintState;

use crate::tests::client::{FakeAgent, Harness};
use crate::tests::seam_title::command_for;

/// Codex's approval banner, the one title this build reads as a claim.
const CODEX_BANNER: &str = "[ ! ] Action Required";

/// The token a harness writes for `state`, and the status a client must then
/// show, as an EXHAUSTIVE match.
///
/// Written from the product decision rather than read from `resolve_status`, so
/// a mutation that agrees with itself cannot silence this. Adding a
/// [`HintState`] stops this match compiling, and `HintState::ALL` is what the
/// loop walks, so the new state cannot be added to one and forgotten in the
/// other.
fn hint_case(state: HintState) -> (&'static str, SidebarStatus) {
    match state {
        // The two states nothing can observe. A blocked agent looks exactly
        // like a finished one to the operating system, so a declaration is the
        // only evidence there is and it must win outright.
        HintState::Approval => ("approval", SidebarStatus::Approval),
        HintState::Input => ("input", SidebarStatus::Input),
        // Beats the syscall probe: a TUI streaming a response sits in `ppoll`
        // on stdin and a socket, which reads as blocked on the terminal. The
        // agent is not ambiguous about it.
        HintState::Working => ("working", SidebarStatus::Working),
        // A finished turn. Identical to what observation would say, and
        // asserted anyway: the source must be the declaration, because a
        // `ready` hint that fell through to observation would report a
        // still-computing agent as working the instant it announced it had
        // stopped.
        HintState::Ready => ("ready", SidebarStatus::Ready),
    }
}

/// The titles this build has decided about for `kind`, each with the claim a
/// client must read from it, as an EXHAUSTIVE match.
///
/// Every kind is given the Codex banner as well as a plausible line of its own.
/// That is the sharp half: the rule is per agent and must not leak, so a build
/// that started matching the banner globally would put "Needs approval" on a
/// shell that merely opened a file with that name, and this table is what
/// notices.
fn title_cases(kind: AgentKind) -> [(&'static str, Option<TitleClaim>); 2] {
    match kind {
        // The one rule that exists. Codex sets this while it holds an approval
        // gate and clears it when the turn resumes.
        AgentKind::Codex => [
            (CODEX_BANNER, Some(TitleClaim::Approval)),
            ("Ready (notes)", None),
        ],
        // Claude declares through the hint channel instead.
        AgentKind::Claude => [(CODEX_BANNER, None), ("Claude Code", None)],
        // Gemini publishes a status line with no recognisable blocked state in
        // it. This is the honest gap the module header names: a Gemini session
        // sitting on a question reads Ready.
        AgentKind::Gemini => [(CODEX_BANNER, None), ("Ready (kernel-notes)", None)],
        AgentKind::Opencode => [(CODEX_BANNER, None), ("opencode", None)],
        AgentKind::Veyyon => [(CODEX_BANNER, None), ("veyyon", None)],
        // A shell's title is its command line and an unknown program's title is
        // anybody's guess, so neither may ever produce a claim.
        AgentKind::Shell => [(CODEX_BANNER, None), ("vim src/main.rs", None)],
        AgentKind::Unknown => [(CODEX_BANNER, None), ("some other program", None)],
    }
}

/// Every hint state, declared by every agent, resolves to the status the client
/// shows.
///
/// WHY: this closes the class behind the approval defect from the side the hint
/// channel owns. The channel is agent-independent by design — that is its whole
/// value against harnesses nobody integrated — so a state that works for one
/// basename and not another means something on the path is keying on the agent
/// when it must not. Twenty-eight real children prove it rather than one.
///
/// Both lists are derived from source at run time and both expectation tables
/// are exhaustive, so a new agent or a new state turns this red until someone
/// records what it should do.
///
/// What this does NOT catch: an agent that never sends a hint, which is exactly
/// the case the title test below exists for and the reason the defect shipped.
#[tokio::test]
async fn every_hint_state_from_every_agent_resolves_the_same_way_for_a_client() {
    for kind in ALL_AGENT_KINDS {
        let command = command_for(kind);
        assert_eq!(
            AgentKind::of(command),
            kind,
            "the command this test runs for {kind:?} does not resolve back to it"
        );

        for state in HintState::ALL {
            let (token, want) = hint_case(state);
            let agent = FakeAgent::named(command);
            let h = Harness::start(1 << 16).await;
            let mut c = h.greeted().await;
            let id = c
                .create(agent.create(
                    1,
                    &format!("printf '\\033]7373;{token};the label\\033\\\\'; read -r _"),
                ))
                .await;

            let info = c
                .until_projection("the declared hint", id, |i| i.hint.is_some())
                .await;
            let hint = info.hint.clone().expect("the wait above required one");
            assert_eq!(
                hint.state, state,
                "{command}: `{token}` arrived at the client as a different state"
            );
            assert_eq!(
                hint.label.as_deref(),
                Some("the label"),
                "{command}/{token}: the label must survive the crossing"
            );

            let view = SessionView::new(info);
            let resolved = view.resolve_status();
            assert_eq!(
                resolved.status, want,
                "{command}/{token}: the client resolved the wrong status"
            );
            assert_eq!(
                resolved.source,
                StatusSource::Hint,
                "{command}/{token}: the status must be attributed to the declaration, \
                 not to something the daemon guessed"
            );
            assert_eq!(
                view.hint_label(),
                Some("the label"),
                "{command}/{token}: the label a live agent sent must be shown"
            );

            h.manager.close(id).expect("close");
        }
    }
}

/// Every agent's terminal title resolves to the claim this build decided on,
/// and to no other.
///
/// WHY: this is the shipped defect. A Codex session holding an approval gate
/// declares it only in its title bar, so before the title rule existed the row
/// read `Ready` — the one answer the sidebar must never give while the operator
/// is being waited on. It is a seam bug end to end: the title is parsed on a
/// PTY reader thread, carried as `term_title`, and turned into a claim by the
/// client, and no single crate's tests span that.
///
/// The negative half is asserted just as hard. A build that matched the banner
/// for every agent would pass a Codex-only test and put "Needs approval" on
/// rows that merely printed those words, so every other kind is given the same
/// banner and must refuse it.
///
/// `requires_declaration` is the invariant underneath: neither blocked state
/// may ever be reached from something we measured, only from something the
/// agent said.
///
/// What this does NOT catch: an agent that blocks without retitling. Gemini,
/// opencode and veyyon publish nothing recognisable, so those rows still read
/// Ready, and the table above records that as a decision rather than hiding it.
#[tokio::test]
async fn every_agents_title_resolves_to_the_claim_this_build_decided_on() {
    for kind in ALL_AGENT_KINDS {
        let command = command_for(kind);

        for (title, want) in title_cases(kind) {
            let agent = FakeAgent::named(command);
            let h = Harness::start(1 << 16).await;
            let mut c = h.greeted().await;
            let id = c
                .create(agent.create(1, &format!("printf '\\033]2;{title}\\007'; read -r _")))
                .await;

            let info = c
                .until_projection("the announced title", id, |i| i.term_title.is_some())
                .await;
            assert_eq!(
                info.term_title.as_deref(),
                Some(title),
                "{command}: the title must reach the client verbatim"
            );

            let resolved = SessionView::new(info).resolve_status();
            match want {
                Some(claim) => {
                    assert_eq!(
                        resolved.status,
                        claim.status(),
                        "{command} announced {title:?} and the row did not report it"
                    );
                    assert_eq!(
                        resolved.source,
                        StatusSource::Title,
                        "{command}/{title:?}: the status must be attributed to the title, \
                         because a banner is a string we matched and the UI has to hedge"
                    );
                    assert!(
                        resolved.source.is_inferred(),
                        "{command}/{title:?}: a title claim is inferred, never proven"
                    );
                }
                None => {
                    assert_ne!(
                        resolved.source,
                        StatusSource::Title,
                        "{command} has no title rule, so {title:?} must produce no claim"
                    );
                    assert!(
                        !resolved.status.requires_declaration(),
                        "{command}/{title:?}: a blocked state was reached without a \
                         declaration, which no observation is allowed to do"
                    );
                }
            }

            h.manager.close(id).expect("close");
        }
    }
}

/// A hint outranks a title, and an exit outranks both, all the way to a client.
///
/// WHY: the two declaration channels can disagree, and the defect's fix added
/// the second one. An agent that hints `working` while its title still carries
/// yesterday's banner is working: the hint is a protocol it opted into, the
/// banner is a string we matched. And a dead session is not waiting for your
/// approval whatever it last declared, or the sidebar keeps a permanent "act
/// now" badge on a row with nothing behind it.
///
/// Precedence is only observable where both signals exist at once on a real
/// projection, which is here and nowhere else in the integration suites.
///
/// What this does NOT catch: the silence that retires a stale `working`
/// declaration, which takes thirty seconds by design and is pinned by
/// `vitrum-model`'s own precedence tests without waiting for it.
#[tokio::test]
async fn a_hint_outranks_a_title_and_an_exit_outranks_both() {
    let agent = FakeAgent::named("codex");
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c
        .create(agent.create(
            1,
            &format!(
                "printf '\\033]2;{CODEX_BANNER}\\007'; \
                 printf '\\033]7373;working;building\\033\\\\'; read -r _; exit 0"
            ),
        ))
        .await;

    let live = c
        .until_projection("both declarations", id, |i| {
            i.term_title.as_deref() == Some(CODEX_BANNER) && i.hint.is_some()
        })
        .await;
    let resolved = SessionView::new(live).resolve_status();
    assert_eq!(
        resolved.status,
        SidebarStatus::Working,
        "the hint the agent addressed to us must beat the banner we matched"
    );
    assert_eq!(resolved.source, StatusSource::Hint);

    c.input(id, b"\n").await;
    let dead = c
        .until_projection("the exited projection", id, |i| !i.status.is_live())
        .await;
    let resolved = SessionView::new(dead.clone()).resolve_status();
    assert_eq!(
        resolved.status,
        SidebarStatus::Ready,
        "a clean exit reads Ready whatever the session last declared"
    );
    assert_eq!(
        resolved.source,
        StatusSource::Exit,
        "the exit must be the stated reason, not a stale declaration that agreed with it"
    );
    assert_eq!(
        SessionView::new(dead).hint_label(),
        None,
        "a label from a declaration that is no longer shown must not be shown either"
    );
}

/// A child that fails must read Failed for a client, not Ready.
///
/// WHY: the exit branch of the resolver is reached only through a real
/// lifecycle transition, and the projection carrying it is built by the daemon
/// after the child is reaped. A nonzero code that arrived as `Some(0)`, or an
/// `attention.failed` that was never set, both surface as a finished-cleanly
/// row for a session that crashed.
///
/// What this does NOT catch: a signalled child, which reports no code at all
/// and is pinned by `vitrum-core`'s exit suite on the producing side.
#[tokio::test]
async fn a_failed_child_reads_failed_for_a_client() {
    let h = Harness::start(1 << 16).await;
    let mut c = h.greeted().await;
    let id = c
        .create(crate::tests::client::create(1, "exit 7"))
        .await;

    let dead = c
        .until_projection("the exited projection", id, |i| !i.status.is_live())
        .await;
    assert_eq!(
        dead.status,
        vitrum_proto::SessionStatus::Exited { code: Some(7) },
        "the exit code must cross the wire intact"
    );
    let resolved = SessionView::new(dead).resolve_status();
    assert_eq!(resolved.status, SidebarStatus::Failed);
    assert_eq!(resolved.source, StatusSource::Exit);
}
