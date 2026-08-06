//! The five-state sidebar status, and exactly where each state comes from.
//!
//! The state set is T3 Code's: `approval | input | working | failed | ready`.
//! Splitting "blocked on the human" into approval and input is the right call
//! and worth adopting, because those two want different responses from the
//! operator.
//!
//! What differs is provenance. T3 Code reads each harness's structured event
//! stream, so it knows a session is blocked only for the harnesses it was built
//! against, and knows nothing at all for the rest. We spawn the child and hold
//! the PTY master, so we can ask the operating system what the foreground
//! process is actually doing, for any process, including agents nobody has ever
//! integrated. That answer arrives as [`Attention::waiting`].
//!
//! # Where each state comes from
//!
//! | State      | Linux / macOS                    | Windows                       |
//! |------------|----------------------------------|-------------------------------|
//! | `Working`  | OBSERVED, proven: the foreground process is not blocked on the terminal. | OBSERVED, inferred from recent output. |
//! | `Ready`    | OBSERVED, proven: the foreground process is blocked reading the terminal. | OBSERVED, inferred from a bell or from silence past [`IDLE_ATTENTION_MS`]. |
//! | `Failed`   | OBSERVED: the child exited nonzero or was signalled. | Same. |
//! | `Approval` | HINTED only.                     | HINTED only.                  |
//! | `Input`    | HINTED only.                     | HINTED only.                  |
//!
//! # Why approval and input stay hinted even with the syscall probe
//!
//! Because a `read()` is a `read()`. A shell sitting at a prompt, an agent
//! asking "which file?", and an agent asking "may I force-push?" all block in
//! the same syscall. The operating system can prove that the next move is the
//! operator's; it cannot tell you whether there is a question behind it, let
//! alone what kind.
//!
//! If `waiting` mapped to `Input`, every agent that finished its turn cleanly
//! would read "Needs input", the state would stop discriminating, and the
//! sidebar would be a flat list again. So `waiting` proves `Ready`, which is
//! precisely T3 Code's meaning of ready: the agent stopped and the next move is
//! yours, whether it finished, asked something, or proposed a plan. The hint
//! channel is what upgrades that into `Approval` or `Input` and supplies a
//! label.
//!
//! Nothing here guesses at those two states. [`SidebarStatus::is_observable`]
//! is false for them, and no code path in this crate produces either without an
//! [`AgentHint`](vitrum_proto::AgentHint).
//!
//! # Inferred versus proven
//!
//! Every resolution reports a [`StatusSource`], so a UI can distinguish what
//! the operating system proved, what we inferred from output timing, and what
//! the agent declared. On a platform that cannot answer the `waiting` question,
//! the source is `Bell`, `Idle` or `Output` and the UI can say the platform
//! cannot tell rather than implying a certainty it does not have.

use vitrum_proto::{Attention, HintState, IDLE_ATTENTION_MS, SessionStatus};
use serde::{Deserialize, Serialize};

/// What the sidebar says a session is doing.
///
/// Deliberately not `Ord`: two orderings are meaningful (urgency for the
/// sidebar, declaration order for docs) and a derived one would silently be
/// neither. Use [`SidebarStatus::urgency`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SidebarStatus {
    /// The agent declared it is blocked asking the operator to approve an
    /// action. Hinted only.
    Approval,
    /// The agent declared it is blocked asking the operator a question.
    /// Hinted only.
    Input,
    /// The child is computing. Nothing is wanted.
    Working,
    /// The child exited nonzero or was signalled.
    Failed,
    /// The agent stopped and the next move is the operator's, whether it
    /// finished the job, asked something, or died quietly. This is the resting
    /// state; whether it *needs* you is
    /// [`has_unseen_completion`](crate::view::SessionView::has_unseen_completion),
    /// tracked separately exactly as T3 Code tracks it.
    Ready,
}

/// Every status, in declaration order. Useful for exhaustive iteration in
/// counts and tests.
pub const ALL_STATUSES: [SidebarStatus; 5] = [
    SidebarStatus::Approval,
    SidebarStatus::Input,
    SidebarStatus::Working,
    SidebarStatus::Failed,
    SidebarStatus::Ready,
];

impl SidebarStatus {
    /// Sidebar sort and rollup weight, higher first.
    ///
    /// `Approval > Input > Failed > Ready > Working`.
    ///
    /// This is a deliberate refinement of T3 Code, whose pill priority ranks
    /// `Working` above the settled states. Their priority answers "what is the
    /// most interesting thing happening in this group"; ours answers "where is
    /// a human needed", which is the only question a twenty-session sidebar has
    /// to answer well. `Working` is the single state that wants nothing from
    /// you, so it ranks last. `Failed` sits under the explicit blocks because a
    /// failure is finished and will not get worse, while an approval prompt is
    /// holding up live work.
    ///
    /// This agrees with [`Attention::priority`], the coarse transport-level
    /// rank, signal for signal: `failed` outranks `waiting`, `bell` and `idle`,
    /// which all resolve to `Ready`, which outranks no signal at all, which
    /// resolves to `Working`. `Approval` and `Input` sit above `Failed` here and
    /// are invisible to the coarse rank, so a client without this model simply
    /// lacks them rather than ordering them wrongly.
    pub fn urgency(self) -> u8 {
        match self {
            SidebarStatus::Approval => 4,
            SidebarStatus::Input => 3,
            SidebarStatus::Failed => 2,
            SidebarStatus::Ready => 1,
            SidebarStatus::Working => 0,
        }
    }

    /// True when this state can be reached from observation alone.
    ///
    /// False for [`SidebarStatus::Approval`] and [`SidebarStatus::Input`]: they
    /// exist only when an agent declares them.
    pub fn is_observable(self) -> bool {
        !matches!(self, SidebarStatus::Approval | SidebarStatus::Input)
    }

    /// True when the operator is what the session is waiting on.
    pub fn wants_operator(self) -> bool {
        !matches!(self, SidebarStatus::Working)
    }

    /// Short operator-facing label.
    pub fn label(self) -> &'static str {
        match self {
            SidebarStatus::Approval => "Needs approval",
            SidebarStatus::Input => "Needs input",
            SidebarStatus::Working => "Working",
            SidebarStatus::Failed => "Failed",
            SidebarStatus::Ready => "Ready",
        }
    }

    /// Stable machine-readable token, matching the serde representation and the
    /// tokens accepted on the hint channel where they overlap.
    pub fn token(self) -> &'static str {
        match self {
            SidebarStatus::Approval => "approval",
            SidebarStatus::Input => "input",
            SidebarStatus::Working => "working",
            SidebarStatus::Failed => "failed",
            SidebarStatus::Ready => "ready",
        }
    }
}

/// Where a resolved status came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusSource {
    /// The child process exited; the exit itself decided the state.
    Exit,
    /// The operating system reports the foreground process blocked reading the
    /// terminal. Proof that the next move is the operator's.
    Waiting,
    /// The operating system reports the foreground process not blocked on the
    /// terminal. Proof that it is computing, even while silent.
    Foreground,
    /// The child rang the terminal bell. Used only where the platform cannot
    /// answer the `waiting` question.
    Bell,
    /// Inferred from silence past [`IDLE_ATTENTION_MS`], with output the
    /// operator has not seen. Used only where the platform cannot answer the
    /// `waiting` question.
    Idle,
    /// Inferred from recent output. Used only where the platform cannot answer
    /// the `waiting` question.
    Output,
    /// The agent declared this state over the OSC hint channel.
    Hint,
}

impl StatusSource {
    /// True for everything the shell worked out for itself, as opposed to what
    /// the agent declared.
    pub fn is_observed(self) -> bool {
        !matches!(self, StatusSource::Hint)
    }

    /// True when the status rests on output timing rather than on a direct
    /// answer from the operating system, the agent, or the child's exit.
    ///
    /// A UI should mark these, because they are the states that can be wrong:
    /// an agent thinking silently for a minute is inferred `Ready` and is
    /// actually working. On Linux and macOS this is never true for a live
    /// session; on Windows it is the only path available.
    pub fn is_inferred(self) -> bool {
        matches!(self, StatusSource::Idle | StatusSource::Output)
    }
}

/// A status together with the signal that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResolution {
    pub status: SidebarStatus,
    pub source: StatusSource,
}

impl StatusResolution {
    const fn new(status: SidebarStatus, source: StatusSource) -> Self {
        StatusResolution { status, source }
    }
}

/// Resolve the sidebar status for one session.
///
/// Precedence, highest first:
///
/// 1. **The exit wins over everything.** A process that is gone is not waiting
///    for your approval, whatever it last declared. A stale `approval` hint on
///    a dead session would park a permanent "act now" badge on a row where
///    there is nothing to act on.
/// 2. **A declaration beats an observation, for the states only the agent can
///    know.** `approval` and `input` have no observable form, so a declaration
///    is the only evidence there is.
/// 3. **A `working` declaration also beats `waiting`.** The syscall probe is
///    ambiguous for event-loop programs: a TUI agent streaming a response sits
///    in `ppoll` on stdin and its network socket, which looks like blocking on
///    the terminal. The agent is not ambiguous about it, so it wins.
/// 4. **Except that prolonged silence retires a stale `working` declaration.**
///    An agent that announced `working` and then finished without announcing
///    `ready` would otherwise spin forever. Once its output has been silent and
///    unseen past [`IDLE_ATTENTION_MS`], observation takes over. `approval` and
///    `input` are never retired this way, because being blocked on a human is
///    silent by definition.
/// 5. **Otherwise the observed signals decide**, in the order
///    [`Attention::priority`] ranks them: failure, then the operating system's
///    answer, then a bell, then unseen silence, then recent output. Proof beats
///    a beep: a session the OS reports as computing is `Working` even if it rang
///    the bell, and the bell still lifts it inside its band through
///    `Attention::priority`.
///
/// Snooze does not appear here. Snoozing changes whether a row is settled and
/// where it sorts, not what the agent is doing.
pub fn resolve_status(
    session_status: &SessionStatus,
    attention: &Attention,
    hint: Option<HintState>,
) -> StatusResolution {
    if !session_status.is_live() {
        return if attention.failed || exited_badly(session_status) {
            StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit)
        } else {
            StatusResolution::new(SidebarStatus::Ready, StatusSource::Exit)
        };
    }

    if let Some(state) = hint {
        match state {
            HintState::Approval => {
                return StatusResolution::new(SidebarStatus::Approval, StatusSource::Hint);
            }
            HintState::Input => {
                return StatusResolution::new(SidebarStatus::Input, StatusSource::Hint);
            }
            HintState::Ready => {
                return StatusResolution::new(SidebarStatus::Ready, StatusSource::Hint);
            }
            HintState::Working => {
                if attention.idle_ms < IDLE_ATTENTION_MS {
                    return StatusResolution::new(SidebarStatus::Working, StatusSource::Hint);
                }
            }
        }
    }

    resolve_observed(attention)
}

/// Status from observable signals alone, for a live child.
///
/// Split out so the hint path can fall through to it and so tests can pin the
/// unhinted behaviour, which is the common case: most harnesses will never emit
/// a hint and must still get a useful sidebar.
fn resolve_observed(attention: &Attention) -> StatusResolution {
    if attention.failed {
        // The server only sets this on a bad exit, so reaching it on a live row
        // means the server saw a failure the lifecycle field has not caught up
        // with. Trust the explicit flag.
        return StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit);
    }
    match attention.waiting {
        Some(true) => StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting),
        Some(false) => StatusResolution::new(SidebarStatus::Working, StatusSource::Foreground),
        None => {
            if attention.bell {
                StatusResolution::new(SidebarStatus::Ready, StatusSource::Bell)
            } else if attention.idle_ms >= IDLE_ATTENTION_MS {
                StatusResolution::new(SidebarStatus::Ready, StatusSource::Idle)
            } else {
                StatusResolution::new(SidebarStatus::Working, StatusSource::Output)
            }
        }
    }
}

/// True when a terminal state represents a failure.
///
/// Signalled processes report `code: None` and count as failures: a child that
/// was killed did not finish its work.
fn exited_badly(session_status: &SessionStatus) -> bool {
    match session_status {
        SessionStatus::Exited { code: Some(code) } => *code != 0,
        SessionStatus::Exited { code: None } => true,
        SessionStatus::Starting | SessionStatus::Running => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attention(bell: bool, idle_ms: u64, failed: bool, waiting: Option<bool>) -> Attention {
        Attention {
            bell,
            idle_ms,
            failed,
            waiting,
        }
    }

    /// A platform that cannot answer the waiting question, with nothing else
    /// going on. This is the Windows baseline.
    const UNKNOWN: Attention = Attention {
        bell: false,
        idle_ms: 0,
        failed: false,
        waiting: None,
    };

    /// The crate's central honesty claim, asserted as code: approval and input
    /// are not derivable from observation, not even with the syscall probe,
    /// because a shell at a prompt and an agent asking a question block in the
    /// same syscall. If someone later adds an inference path to either, this
    /// fails and forces the question back into review.
    #[test]
    fn approval_and_input_are_never_produced_without_a_hint() {
        let mut produced = Vec::new();
        for bell in [false, true] {
            for idle_ms in [0, 1, IDLE_ATTENTION_MS - 1, IDLE_ATTENTION_MS, 10_000_000] {
                for failed in [false, true] {
                    for waiting in [None, Some(true), Some(false)] {
                        for lifecycle in [
                            SessionStatus::Starting,
                            SessionStatus::Running,
                            SessionStatus::Exited { code: Some(0) },
                            SessionStatus::Exited { code: Some(1) },
                            SessionStatus::Exited { code: None },
                        ] {
                            let resolved = resolve_status(
                                &lifecycle,
                                &attention(bell, idle_ms, failed, waiting),
                                None,
                            );
                            produced.push(resolved.status);
                            assert!(
                                resolved.source.is_observed(),
                                "unhinted resolution claimed a hint source: {resolved:?}"
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(produced.len(), 300);
        assert!(!produced.contains(&SidebarStatus::Approval));
        assert!(!produced.contains(&SidebarStatus::Input));
        assert!(!SidebarStatus::Approval.is_observable());
        assert!(!SidebarStatus::Input.is_observable());
        assert!(SidebarStatus::Working.is_observable());
        assert!(SidebarStatus::Ready.is_observable());
        assert!(SidebarStatus::Failed.is_observable());
    }

    /// The operating system's answer is proof and must be reported as proof.
    /// A foreground process blocked reading the terminal means the next move is
    /// the operator's; one that is not blocked is computing.
    #[test]
    fn the_foreground_probe_decides_ready_versus_working_outright() {
        let blocked = resolve_status(
            &SessionStatus::Running,
            &attention(false, 0, false, Some(true)),
            None,
        );
        assert_eq!(
            blocked,
            StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting)
        );
        assert!(!blocked.source.is_inferred());

        let computing = resolve_status(
            &SessionStatus::Running,
            &attention(false, 0, false, Some(false)),
            None,
        );
        assert_eq!(
            computing,
            StatusResolution::new(SidebarStatus::Working, StatusSource::Foreground)
        );
        assert!(!computing.source.is_inferred());
    }

    /// The probe's whole value: an agent thinking silently for minutes is
    /// working, and the idle heuristic that would call it Ready must not get a
    /// vote when the operating system can answer.
    #[test]
    fn a_silent_but_computing_process_is_working_not_ready() {
        let resolved = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS * 100, false, Some(false)),
            None,
        );
        assert_eq!(
            resolved,
            StatusResolution::new(SidebarStatus::Working, StatusSource::Foreground)
        );
    }

    /// Proof beats a beep. A process the OS reports as computing stays Working
    /// even after a bell, because a bell is frequently an incidental completion
    /// beep from a subprocess. The bell is not lost: it lifts the row inside its
    /// band through `Attention::priority`.
    #[test]
    fn a_bell_does_not_override_a_process_proven_to_be_computing() {
        let observed = attention(true, 0, false, Some(false));
        let resolved = resolve_status(&SessionStatus::Running, &observed, None);
        assert_eq!(
            resolved,
            StatusResolution::new(SidebarStatus::Working, StatusSource::Foreground)
        );
        assert_eq!(observed.priority(), 2, "the bell still ranks the row");
    }

    /// Windows reports `None`, and `None` must not behave like `Some(false)`.
    /// Collapsing them would make every Windows session claim it is provably
    /// working, which is a confident lie.
    #[test]
    fn an_unknown_probe_falls_back_to_inference_and_is_marked_inferred() {
        let quiet = resolve_status(&SessionStatus::Running, &UNKNOWN, None);
        assert_eq!(
            quiet,
            StatusResolution::new(SidebarStatus::Working, StatusSource::Output)
        );
        assert!(quiet.source.is_inferred());

        let silent = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, None),
            None,
        );
        assert_eq!(
            silent,
            StatusResolution::new(SidebarStatus::Ready, StatusSource::Idle)
        );
        assert!(silent.source.is_inferred());

        // The same inputs with a real answer produce the opposite verdict,
        // which is exactly why None must not be flattened.
        let answered = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, Some(false)),
            None,
        );
        assert_eq!(answered.status, SidebarStatus::Working);
        assert_ne!(answered.status, silent.status);
    }

    /// On a platform that cannot answer, a bell is the only immediate signal
    /// and must not wait for the idle timer. This is the universal path that
    /// works for every terminal program ever written.
    #[test]
    fn a_bell_reaches_ready_immediately_where_the_probe_is_unavailable() {
        let resolved = resolve_status(
            &SessionStatus::Running,
            &attention(true, 0, false, None),
            None,
        );
        assert_eq!(
            resolved,
            StatusResolution::new(SidebarStatus::Ready, StatusSource::Bell)
        );
    }

    /// The idle threshold is the end-of-turn signal on a platform without the
    /// probe. Off-by-one here means a session flips to Ready a beat early or
    /// never.
    #[test]
    fn inferred_silence_flips_to_ready_exactly_at_the_threshold() {
        let just_under = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS - 1, false, None),
            None,
        );
        assert_eq!(
            just_under,
            StatusResolution::new(SidebarStatus::Working, StatusSource::Output)
        );

        let at = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, None),
            None,
        );
        assert_eq!(at, StatusResolution::new(SidebarStatus::Ready, StatusSource::Idle));
    }

    /// A live session with fresh output and no probe is the baseline. If this
    /// drifts to `Ready` the whole sidebar reads as idle while twenty agents
    /// are running.
    #[test]
    fn a_starting_session_is_working() {
        for lifecycle in [SessionStatus::Starting, SessionStatus::Running] {
            let resolved = resolve_status(&lifecycle, &UNKNOWN, None);
            assert_eq!(
                resolved,
                StatusResolution::new(SidebarStatus::Working, StatusSource::Output)
            );
        }
    }

    /// Exit code decides failure, and a signalled child (`code: None`) counts as
    /// failed. Treating a kill as a clean finish would hide crashed agents in
    /// the settled pile.
    #[test]
    fn exit_code_decides_failed_versus_ready_and_signals_count_as_failure() {
        let clean = resolve_status(&SessionStatus::Exited { code: Some(0) }, &UNKNOWN, None);
        assert_eq!(clean, StatusResolution::new(SidebarStatus::Ready, StatusSource::Exit));

        let nonzero = resolve_status(&SessionStatus::Exited { code: Some(1) }, &UNKNOWN, None);
        assert_eq!(nonzero, StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit));

        let signalled = resolve_status(&SessionStatus::Exited { code: None }, &UNKNOWN, None);
        assert_eq!(signalled, StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit));
    }

    /// A failure outranks the probe. A child that exited badly is Failed even if
    /// a stale probe result still claims its foreground process is blocked.
    #[test]
    fn a_failure_outranks_every_other_observed_signal() {
        for waiting in [None, Some(true), Some(false)] {
            let resolved = resolve_status(
                &SessionStatus::Running,
                &attention(true, 0, true, waiting),
                None,
            );
            assert_eq!(
                resolved,
                StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit),
                "waiting {waiting:?}"
            );
        }
    }

    /// The server sets `attention.failed` alongside the exit; if the two ever
    /// disagree the failure flag must still surface rather than being swallowed
    /// by a `code: Some(0)`.
    #[test]
    fn an_explicit_failure_flag_survives_a_zero_exit_code() {
        let resolved = resolve_status(
            &SessionStatus::Exited { code: Some(0) },
            &attention(false, 0, true, None),
            None,
        );
        assert_eq!(resolved, StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit));
    }

    /// The hint's real job: upgrading an observed Ready into the specific thing
    /// the agent wants. The probe proves the process stopped; only the agent
    /// knows there is an approval gate behind it.
    #[test]
    fn a_hint_upgrades_an_observed_ready_into_approval_or_input() {
        let observed = attention(false, 0, false, Some(true));
        assert_eq!(
            resolve_status(&SessionStatus::Running, &observed, None).status,
            SidebarStatus::Ready
        );
        assert_eq!(
            resolve_status(&SessionStatus::Running, &observed, Some(HintState::Approval)),
            StatusResolution::new(SidebarStatus::Approval, StatusSource::Hint)
        );
        assert_eq!(
            resolve_status(&SessionStatus::Running, &observed, Some(HintState::Input)),
            StatusResolution::new(SidebarStatus::Input, StatusSource::Hint)
        );
    }

    /// Approval and input must outrank every live observation, including a
    /// probe that says the process is computing. The agent told us precisely
    /// what it wants and a generic inference must not overwrite it.
    #[test]
    fn a_blocking_hint_outranks_every_live_observation() {
        for state in [HintState::Approval, HintState::Input] {
            let expected = if state == HintState::Approval {
                SidebarStatus::Approval
            } else {
                SidebarStatus::Input
            };
            for observed in [
                UNKNOWN,
                attention(true, 0, false, None),
                attention(false, IDLE_ATTENTION_MS * 100, false, None),
                attention(false, 0, false, Some(false)),
                attention(false, 0, false, Some(true)),
                attention(true, IDLE_ATTENTION_MS * 100, false, Some(false)),
            ] {
                let resolved = resolve_status(&SessionStatus::Running, &observed, Some(state));
                assert_eq!(
                    resolved,
                    StatusResolution::new(expected, StatusSource::Hint),
                    "hint {state:?} lost to {observed:?}"
                );
            }
        }
    }

    /// A dead process cannot be waiting for approval. Without this, a harness
    /// that is killed at an approval prompt leaves a permanent "act now" badge
    /// on a row where nothing can be acted on.
    #[test]
    fn an_exit_overrides_a_stale_blocking_hint() {
        for state in [
            HintState::Approval,
            HintState::Input,
            HintState::Working,
            HintState::Ready,
        ] {
            let clean =
                resolve_status(&SessionStatus::Exited { code: Some(0) }, &UNKNOWN, Some(state));
            assert_eq!(
                clean,
                StatusResolution::new(SidebarStatus::Ready, StatusSource::Exit),
                "hint {state:?} survived a clean exit"
            );

            let crashed =
                resolve_status(&SessionStatus::Exited { code: Some(2) }, &UNKNOWN, Some(state));
            assert_eq!(
                crashed,
                StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit),
                "hint {state:?} survived a crash"
            );
        }
    }

    /// A `ready` declaration must win over a probe that says the process is
    /// computing, so a harness that announces the end of its turn while still
    /// flushing output does not keep reading as Working.
    #[test]
    fn a_ready_hint_beats_a_computing_probe() {
        let resolved = resolve_status(
            &SessionStatus::Running,
            &attention(false, 0, false, Some(false)),
            Some(HintState::Ready),
        );
        assert_eq!(resolved, StatusResolution::new(SidebarStatus::Ready, StatusSource::Hint));
    }

    /// The TUI case. A terminal agent streaming a response sits in `ppoll` on
    /// stdin plus its network socket, which the probe can read as blocked on the
    /// terminal. The agent is not ambiguous about it, so its declaration wins
    /// and the row does not flicker to Ready mid-response.
    #[test]
    fn a_working_hint_beats_an_ambiguous_blocked_probe() {
        let resolved = resolve_status(
            &SessionStatus::Running,
            &attention(false, 0, false, Some(true)),
            Some(HintState::Working),
        );
        assert_eq!(resolved, StatusResolution::new(SidebarStatus::Working, StatusSource::Hint));
    }

    /// A harness that announces `working` and then finishes without announcing
    /// `ready` must not spin forever. Past the idle threshold, observation
    /// retires the stale declaration, and the probe then gets the final say.
    #[test]
    fn a_stale_working_hint_is_retired_by_prolonged_silence() {
        let fresh = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS - 1, false, Some(true)),
            Some(HintState::Working),
        );
        assert_eq!(fresh, StatusResolution::new(SidebarStatus::Working, StatusSource::Hint));

        let stale = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, Some(true)),
            Some(HintState::Working),
        );
        assert_eq!(stale, StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting));

        // Retired, but the probe can still say it is genuinely computing, in
        // which case the row stays Working with an observed source.
        let still_busy = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, Some(false)),
            Some(HintState::Working),
        );
        assert_eq!(
            still_busy,
            StatusResolution::new(SidebarStatus::Working, StatusSource::Foreground)
        );
    }

    /// The blocking hints are explicitly exempt from the staleness rule. Waiting
    /// for a human produces no output by definition, so retiring an `approval`
    /// hint on silence would delete the badge precisely when it matters.
    #[test]
    fn blocking_hints_are_exempt_from_the_staleness_rule() {
        for (state, expected) in [
            (HintState::Approval, SidebarStatus::Approval),
            (HintState::Input, SidebarStatus::Input),
        ] {
            let resolved = resolve_status(
                &SessionStatus::Running,
                &attention(false, IDLE_ATTENTION_MS * 1000, false, Some(true)),
                Some(state),
            );
            assert_eq!(resolved, StatusResolution::new(expected, StatusSource::Hint));
        }
    }

    /// Urgency is the single ordering used by both the sidebar sort and the
    /// project rollup. Pinning the exact ranks stops a "small tweak" in one
    /// consumer from silently reordering the other.
    #[test]
    fn urgency_ranks_human_blocks_above_failure_above_rest_above_working() {
        assert_eq!(SidebarStatus::Approval.urgency(), 4);
        assert_eq!(SidebarStatus::Input.urgency(), 3);
        assert_eq!(SidebarStatus::Failed.urgency(), 2);
        assert_eq!(SidebarStatus::Ready.urgency(), 1);
        assert_eq!(SidebarStatus::Working.urgency(), 0);

        let mut ranked = ALL_STATUSES;
        ranked.sort_by_key(|status| core::cmp::Reverse(status.urgency()));
        assert_eq!(
            ranked,
            [
                SidebarStatus::Approval,
                SidebarStatus::Input,
                SidebarStatus::Failed,
                SidebarStatus::Ready,
                SidebarStatus::Working,
            ]
        );
    }

    /// Our five-state urgency and the transport's coarse `Attention::priority`
    /// must agree on relative order, or a client using the coarse rank and one
    /// using this model show different lists for the same daemon. Asserted by
    /// walking the coarse rank downwards and checking urgency never increases.
    #[test]
    fn urgency_is_monotone_against_the_transport_coarse_rank() {
        let ranked_signals = [
            attention(false, 0, true, None),         // failed, priority 4
            attention(false, 0, false, Some(true)),  // waiting, priority 3
            attention(true, 0, false, None),         // bell, priority 2
            attention(false, IDLE_ATTENTION_MS, false, None), // idle, priority 1
            attention(false, 0, false, Some(false)), // nothing, priority 0
        ];
        let mut previous_priority = u8::MAX;
        let mut previous_urgency = u8::MAX;
        for observed in ranked_signals {
            let priority = observed.priority();
            let urgency = resolve_status(&SessionStatus::Running, &observed, None)
                .status
                .urgency();
            assert!(priority < previous_priority, "coarse rank not descending");
            assert!(
                urgency <= previous_urgency,
                "urgency {urgency} rose while coarse rank fell to {priority}"
            );
            previous_priority = priority;
            previous_urgency = urgency;
        }
        assert_eq!(previous_priority, 0);
        assert_eq!(previous_urgency, SidebarStatus::Working.urgency());
    }

    /// Urgency must be injective across the five states, or the rollup's
    /// "single most urgent" pick becomes order-dependent on input.
    #[test]
    fn every_status_has_a_distinct_urgency() {
        let mut seen = Vec::new();
        for status in ALL_STATUSES {
            assert!(!seen.contains(&status.urgency()), "duplicate urgency for {status:?}");
            seen.push(status.urgency());
        }
        assert_eq!(seen.len(), 5);
    }

    /// Working is the one state that asks nothing of the operator. This drives
    /// the "jump to next session needing me" traversal, so a wrong answer here
    /// makes that key skip or stop on the wrong row.
    #[test]
    fn only_working_wants_nothing_from_the_operator() {
        assert!(!SidebarStatus::Working.wants_operator());
        for status in [
            SidebarStatus::Approval,
            SidebarStatus::Input,
            SidebarStatus::Failed,
            SidebarStatus::Ready,
        ] {
            assert!(status.wants_operator(), "{status:?} should want the operator");
        }
    }

    /// Labels and tokens are user-visible and wire-visible respectively. Pinning
    /// them keeps a rename from quietly changing persisted state or the UI.
    #[test]
    fn labels_and_tokens_are_stable() {
        let rendered: Vec<(&str, &str)> = ALL_STATUSES
            .iter()
            .map(|status| (status.label(), status.token()))
            .collect();
        assert_eq!(
            rendered,
            vec![
                ("Needs approval", "approval"),
                ("Needs input", "input"),
                ("Working", "working"),
                ("Failed", "failed"),
                ("Ready", "ready"),
            ]
        );
    }

    /// The serde token must match `token()` exactly, because persisted UI state
    /// and the hint channel both round-trip through these strings.
    #[test]
    fn serde_representation_matches_the_token() {
        for status in ALL_STATUSES {
            let json = serde_json::to_string(&status).expect("status serialises");
            assert_eq!(json, format!("\"{}\"", status.token()));
            let back: SidebarStatus = serde_json::from_str(&json).expect("status round-trips");
            assert_eq!(back, status);
        }
    }

    /// Exactly one source is a declaration and exactly two are inferences. A UI
    /// that marks declared and uncertain states differently depends on this
    /// split being exact.
    #[test]
    fn status_sources_partition_into_declared_proven_and_inferred() {
        let sources = [
            StatusSource::Exit,
            StatusSource::Waiting,
            StatusSource::Foreground,
            StatusSource::Bell,
            StatusSource::Idle,
            StatusSource::Output,
            StatusSource::Hint,
        ];
        assert_eq!(sources.iter().filter(|source| source.is_observed()).count(), 6);
        assert_eq!(sources.iter().filter(|source| source.is_inferred()).count(), 2);
        assert!(!StatusSource::Hint.is_observed());
        assert!(StatusSource::Idle.is_inferred());
        assert!(StatusSource::Output.is_inferred());
        assert!(!StatusSource::Waiting.is_inferred());
        assert!(!StatusSource::Foreground.is_inferred());
        assert!(!StatusSource::Bell.is_inferred());
    }

    /// On Linux and macOS a live session is never inferred, because the probe
    /// always answers. That is the platform guarantee the UI leans on to avoid
    /// showing an uncertainty marker where there is no uncertainty.
    #[test]
    fn a_live_session_with_a_probe_answer_is_never_inferred() {
        for waiting in [Some(true), Some(false)] {
            for bell in [false, true] {
                for idle_ms in [0, IDLE_ATTENTION_MS * 10] {
                    let resolved = resolve_status(
                        &SessionStatus::Running,
                        &attention(bell, idle_ms, false, waiting),
                        None,
                    );
                    assert!(
                        !resolved.source.is_inferred(),
                        "inferred with a probe answer: {resolved:?}"
                    );
                }
            }
        }
    }
}
