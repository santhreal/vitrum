//! OSC 7373 from a real child through to `SessionInfo.hint`.
//!
//! The scanner is unit-tested in `output_scan`; these tests prove the whole
//! path: a program prints the sequence, the kernel slices it wherever it likes,
//! the coalescer publishes it, and the projection a client reads carries the
//! declaration. They also pin the precedence the model expects, by resolving
//! the sidebar status from the projection rather than by asserting on fields.

use vitrum_model::agent::AgentKind;
use vitrum_model::status::{SidebarStatus, StatusSource, resolve_status};
#[cfg(target_os = "linux")]
use vitrum_proto::IDLE_ATTENTION_MS;
use vitrum_proto::{HintState, SessionStatus};

use crate::SessionManager;
#[cfg(target_os = "linux")]
use crate::tests::helpers::kernel_reports_other_processes;
use crate::tests::helpers::{collect, probe_now, shell_spec, wait_exit};

/// Resolve the sidebar status exactly as a client would, from the projection.
fn status(info: &vitrum_proto::SessionInfo) -> (SidebarStatus, StatusSource) {
    let resolution = resolve_status(
        &info.status,
        &info.attention,
        info.hint.as_ref().map(|h| h.state),
        // Resolved from the projection exactly as a client does, so these
        // tests cannot pass on a fiction: the sessions here run a shell, which
        // has no title rule, so the claim is always `None` and the hint
        // precedence is what is under test.
        AgentKind::of(&info.command).title_claim(&info.title),
    );
    (resolution.status, resolution.source)
}

/// A child that declares a state must have it land in the projection.
///
/// The whole opt-in channel in one assertion: a harness prints six bytes of
/// escape sequence and the sidebar knows something no observation could tell
/// it. If this path is broken the feature is invisible, because a hint that is
/// dropped looks exactly like a harness that never opted in.
#[cfg(not(windows))]
#[tokio::test]
async fn a_declared_hint_reaches_the_projection() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033]7373;approval;rm -rf build/\\033\\\\'; read -r x",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"\x1b\\")).await;

    let info = probe_now(&mgr, id).await;
    let hint = info
        .hint
        .expect("the declaration must reach the projection");
    assert_eq!(hint.state, HintState::Approval);
    assert_eq!(hint.label.as_deref(), Some("rm -rf build/"));
    assert!(
        hint.received_at_ms >= info.created_at_ms,
        "the hint must be stamped when it arrived, not left at zero"
    );
    mgr.close(id).expect("close");
}

/// A hint terminated by BEL must not also ring the bell.
///
/// BEL is a legal terminator for the sequence, so an agent that used it would
/// otherwise light the attention indicator on every single declaration, which
/// would make the indicator meaningless within one turn.
#[cfg(not(windows))]
#[tokio::test]
async fn a_bel_terminated_hint_does_not_ring_the_bell() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec("printf '\\033]7373;ready\\007'; read -r x"))
        .expect("spawn");
    // Unattached on purpose: the bell is only recorded for output nobody is
    // watching, so an attached test could not tell the difference.
    let info = probe_now(&mgr, id).await;
    assert_eq!(
        info.hint.as_ref().map(|h| h.state),
        Some(HintState::Ready),
        "the hint itself must still be read"
    );
    assert!(
        !info.attention.bell,
        "a hint's own terminator is not a request for the operator"
    );
    mgr.close(id).expect("close");
}

/// The last declaration in a burst wins.
///
/// An agent that says `working` and then `ready` in the same breath has
/// finished. Keeping the first would leave a spinner on a row that is waiting
/// for you, which is the failure mode this whole feature exists to prevent.
#[cfg(not(windows))]
#[tokio::test]
async fn the_last_declaration_in_a_burst_wins() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033]7373;working\\007out\\033]7373;ready;turn over\\007'; read -r x",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"\x07")).await;
    let info = probe_now(&mgr, id).await;
    let hint = info.hint.expect("a hint must be recorded");
    assert_eq!(hint.state, HintState::Ready);
    assert_eq!(hint.label.as_deref(), Some("turn over"));
    mgr.close(id).expect("close");
}

/// A later declaration replaces an earlier one.
///
/// Two separate bursts, not one, so this pins the update path rather than the
/// within-chunk collapse above.
#[cfg(not(windows))]
#[tokio::test]
async fn a_later_declaration_replaces_the_earlier_one() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033]7373;working;building\\007'; read -r x; \
             printf '\\033]7373;input;which file?\\007'; read -r y",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"\x07")).await;
    assert_eq!(
        probe_now(&mgr, id).await.hint.map(|h| h.state),
        Some(HintState::Working)
    );

    mgr.write(id, b"go\n").expect("write");
    c.until(|b| b.windows(5).any(|w| w == b"input")).await;
    let info = probe_now(&mgr, id).await;
    let hint = info.hint.expect("the second declaration must be recorded");
    assert_eq!(hint.state, HintState::Input);
    assert_eq!(hint.label.as_deref(), Some("which file?"));
    mgr.close(id).expect("close");
}

/// A malformed sequence must leave the previous hint alone.
///
/// Dropping to `None` on a typo would be worse than ignoring it: the sidebar
/// would lose a real, still-accurate declaration because the agent botched a
/// later one.
#[cfg(not(windows))]
#[tokio::test]
async fn a_malformed_sequence_does_not_clear_a_good_hint() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033]7373;approval;may i\\007'; read -r x; \
             printf '\\033]7373;paused\\007\\033]777;notify;t;b\\007'; read -r y",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"\x07")).await;
    assert_eq!(
        probe_now(&mgr, id).await.hint.map(|h| h.state),
        Some(HintState::Approval)
    );

    mgr.write(id, b"go\n").expect("write");
    c.until(|b| b.windows(6).any(|w| w == b"notify")).await;
    let info = probe_now(&mgr, id).await;
    assert_eq!(
        info.hint.as_ref().map(|h| h.state),
        Some(HintState::Approval),
        "an unknown state token must be ignored, not applied and not cleared"
    );
    mgr.close(id).expect("close");
}

/// A harness that never emits a hint must stay `None` for its whole life.
///
/// The common case, and the one that has to keep working perfectly: every agent
/// that has never heard of OSC 7373 gets a fully useful sidebar from
/// observation alone.
#[cfg(not(windows))]
#[tokio::test]
async fn a_silent_harness_never_grows_a_hint() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033[32mhello\\033[0m\\r\\n'; read -r x",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"\r\n")).await;
    let info = probe_now(&mgr, id).await;
    assert_eq!(info.hint, None);
    assert!(!info.attention.bell);
    mgr.close(id).expect("close");
}

/// A declared `approval` must resolve to the Approval status, beating anything
/// the probe found.
///
/// `approval` and `input` have no observable form: the kernel can prove the
/// process is blocked on the terminal but never that there is a question behind
/// it. So the declaration is the only evidence there is, and it has to win.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_declaration_beats_the_observation_for_approval() {
    // This kernel will not say what a process is doing; see the helper.
    if !kernel_reports_other_processes() {
        return;
    }
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033]7373;approval;force push?\\007'; read -r x",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"\x07")).await;
    let info = probe_now(&mgr, id).await;
    assert_eq!(
        info.attention.waiting,
        Some(true),
        "the shell really is blocked on the terminal"
    );
    assert_eq!(
        status(&info),
        (SidebarStatus::Approval, StatusSource::Hint),
        "a declaration the OS cannot make must outrank what the OS did say"
    );
    mgr.close(id).expect("close");
}

/// With no hint at all, a blocked shell must resolve to Ready from the probe.
///
/// This is the claim that separates us from a shell that reads per-harness
/// event streams: the state is proven by the operating system, for a plain
/// `sh` that has never heard of us.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn observation_alone_resolves_a_blocked_shell_to_ready() {
    // This kernel will not say what a process is doing; see the helper.
    if !kernel_reports_other_processes() {
        return;
    }
    let mgr = SessionManager::new(8192);
    let id = mgr.spawn(shell_spec("read -r x")).expect("spawn");
    let info = probe_now(&mgr, id).await;
    assert_eq!(info.hint, None);
    assert_eq!(status(&info), (SidebarStatus::Ready, StatusSource::Waiting));
    mgr.close(id).expect("close");
}

/// A `working` declaration must beat the probe's answer while output is fresh.
///
/// The syscall probe is ambiguous for event-loop programs; the agent is not
/// ambiguous about itself. So a spinning agent that declared `working` reads as
/// working even though it is sitting in a read between tokens.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_working_declaration_beats_a_blocked_observation() {
    // This kernel will not say what a process is doing; see the helper.
    if !kernel_reports_other_processes() {
        return;
    }
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec("printf '\\033]7373;working\\007'; read -r x"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"\x07")).await;
    let info = probe_now(&mgr, id).await;
    assert_eq!(info.attention.waiting, Some(true));
    assert!(
        info.attention.idle_ms < IDLE_ATTENTION_MS,
        "the declaration is still fresh"
    );
    assert_eq!(status(&info), (SidebarStatus::Working, StatusSource::Hint));
    mgr.close(id).expect("close");
}

/// An exited session's stale declaration must not park an "act now" badge.
///
/// The model retires it by precedence rather than the daemon deleting it, so
/// this pins the whole arrangement: the daemon keeps what the agent last said,
/// the exit outranks it, and the row reads from the exit.
#[cfg(not(windows))]
#[tokio::test]
async fn an_exit_outranks_a_stale_approval() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033]7373;approval;may i\\007'; exit 3",
        ))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(3));
    let info = mgr.info(id).expect("info");
    assert_eq!(
        info.hint.as_ref().map(|h| h.state),
        Some(HintState::Approval),
        "the daemon keeps the record of what the agent last declared"
    );
    assert_eq!(info.status, SessionStatus::Exited { code: Some(3) });
    assert_eq!(
        status(&info),
        (SidebarStatus::Failed, StatusSource::Exit),
        "a dead session is not waiting for your approval"
    );
}

/// A hint split across PTY reads must still arrive.
///
/// Not hypothetical: a sequence emitted byte by byte, as a program writing
/// unbuffered does, crosses read boundaries constantly. A scanner that only
/// matched whole sequences would drop most real declarations.
#[cfg(not(windows))]
#[tokio::test]
async fn a_hint_dribbled_out_byte_by_byte_still_arrives() {
    let mgr = SessionManager::new(8192);
    // One `printf` per byte, so the shell issues a separate write for each and
    // the reader sees the sequence in pieces.
    let script = "\\033 ]7373 ; input ; slow? \\007"
        .split_whitespace()
        .map(|piece| format!("printf '{piece}'; "))
        .collect::<String>();
    let id = mgr
        .spawn(shell_spec(&format!("{script} read -r x")))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"\x07")).await;
    let info = probe_now(&mgr, id).await;
    assert_eq!(
        info.hint.map(|h| (h.state, h.label)),
        Some((HintState::Input, Some("slow?".to_string())))
    );
    mgr.close(id).expect("close");
}

/// A hint must wake the observation channel so other windows learn about it.
///
/// Without this a second window keeps rendering the agent's previous state
/// until something else happens to push an update, which for a blocked agent is
/// never.
#[cfg(not(windows))]
#[tokio::test]
async fn a_hint_publishes_an_observation() {
    let mgr = SessionManager::new(8192);
    let id = mgr
        .spawn(shell_spec(
            "read -r go; printf '\\033]7373;ready;done\\007'; read -r x",
        ))
        .expect("spawn");
    let mut observations = mgr.subscribe_observations(id).expect("observations");
    let mut c = collect(&mgr, id);
    // Drain whatever the initial probe published so the next change is ours.
    probe_now(&mgr, id).await;
    observations.mark_unchanged();

    mgr.write(id, b"go\n").expect("write");
    c.until(|b| b.ends_with(b"\x07")).await;

    let deadline = tokio::time::Instant::now() + crate::tests::helpers::DEADLINE;
    loop {
        tokio::time::timeout_at(deadline, observations.changed())
            .await
            .expect("a hint must publish an observation")
            .expect("the observation channel closed");
        if mgr.info(id).expect("info").hint.is_some() {
            break;
        }
    }
    mgr.close(id).expect("close");
}
