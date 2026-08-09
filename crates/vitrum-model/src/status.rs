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
//! | `Approval` | DECLARED: a hint, or a title this agent is known to publish while blocked. | Same. |
//! | `Input`    | DECLARED: a hint, or a title this agent is known to publish while blocked. | Same. |
//!
//! # Why approval and input are declared rather than observed
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
//! yours, whether it finished, asked something, or proposed a plan.
//!
//! Two channels upgrade that into `Approval` or `Input`, and both are the agent
//! speaking:
//!
//! 1. The [`hint`](crate::hint) channel, which an agent writes to us on
//!    purpose. It carries the state and a label, and it is the best evidence
//!    there is.
//! 2. The terminal title, read through the per-agent rule in
//!    [`AgentKind::title_claim`](crate::agent::AgentKind::title_claim). Codex
//!    titles itself `[ ! ] Action Required` while it holds an approval gate,
//!    and clears it when the turn resumes.
//!
//! A title is admissible where output timing is not, and the difference is not
//! subtle. Timing is us guessing what silence means, and silence means nothing
//! in particular: an agent thinking hard and an agent waiting for you look
//! identical. A title is a statement the agent chose to publish, in its own
//! words, about its own state, at the instant it entered that state and again
//! at the instant it left. We are not inferring it; we are reading it.
//!
//! What it is NOT is a hint. The agent wrote that string for a window title
//! bar, not for us, and we recognise it by a pattern that belongs to one agent
//! and goes stale the day that agent rewords its banner. So it resolves with
//! [`StatusSource::Title`], which reports [`StatusSource::is_inferred`] and
//! makes the sidebar hedge.
//!
//! What it still does not catch: any agent that blocks without retitling.
//! Gemini, opencode and veyyon publish nothing recognisable, so a Gemini
//! session sitting on a question reads `Ready` exactly as before. There is no
//! general rule to write, and a global match on somebody else's banner would
//! put "Needs approval" on a row that merely opened a file with that name.
//! [`SidebarStatus::requires_declaration`] is the invariant that holds: neither
//! state is ever reached from what we measured, only from what the agent said.
//!
//! # Inferred versus proven
//!
//! Every resolution reports a [`StatusSource`], so a UI can distinguish what
//! the operating system proved, what we inferred from output timing or from a
//! title, and what the agent declared to us directly. On a platform that cannot
//! answer the `waiting` question, the source is `Bell`, `Idle` or `Output` and
//! the UI can say the platform cannot tell rather than implying a certainty it
//! does not have.

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
    /// action, either over the hint channel or by publishing it in its
    /// terminal title. Never inferred from what the process is doing.
    Approval,
    /// The agent declared it is blocked asking the operator a question, either
    /// over the hint channel or by publishing it in its terminal title. Never
    /// inferred from what the process is doing.
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

    /// True when the agent has to say so for this state to appear.
    ///
    /// True for [`SidebarStatus::Approval`] and [`SidebarStatus::Input`], and
    /// only for those. Nothing we measure ourselves — the exit code, the
    /// foreground probe, a bell, output timing — can reach either, because a
    /// process blocked on a question and a process blocked at a prompt are the
    /// same process to the operating system. Both channels that do reach them,
    /// [`StatusSource::Hint`] and [`StatusSource::Title`], are the agent
    /// speaking about itself.
    ///
    /// This replaced an `is_observable` that meant the same partition and said
    /// the wrong thing about it. A title IS observed — we read it off the
    /// stream the same way we count a bell — and the property that actually
    /// matters is not how the evidence reached us but whether the agent
    /// produced it. A predicate named for observation would now have to answer
    /// "yes, but not like that".
    pub fn requires_declaration(self) -> bool {
        matches!(self, SidebarStatus::Approval | SidebarStatus::Input)
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
    /// The agent published this state in its terminal title, and the per-agent
    /// rule in [`AgentKind::title_claim`](crate::agent::AgentKind::title_claim)
    /// recognised it.
    ///
    /// A declaration, not a measurement: Codex sets `[ ! ] Action Required`
    /// when it puts up a gate and clears it when the turn resumes, so the state
    /// arrives and leaves on the agent's own say-so rather than on our reading
    /// of a silence.
    ///
    /// Weaker than [`StatusSource::Hint`] on two counts, which is why it is a
    /// separate variant and why [`StatusSource::is_inferred`] is true for it.
    /// The agent wrote the string for a window title bar and not for us, so it
    /// never consented to the meaning we read into it; and we recognise it by a
    /// pattern owned by one agent, which is wrong the day that agent rewords
    /// its banner. A hint is a contract. A title is a good-faith reading.
    Title,
}

/// Every source, in declaration order. Useful for exhaustive iteration in
/// tests and in a UI that has to say something about each one.
pub const ALL_STATUS_SOURCES: [StatusSource; 8] = [
    StatusSource::Exit,
    StatusSource::Waiting,
    StatusSource::Foreground,
    StatusSource::Bell,
    StatusSource::Idle,
    StatusSource::Output,
    StatusSource::Hint,
    StatusSource::Title,
];

impl StatusSource {
    /// True for everything we worked out for ourselves, as opposed to what the
    /// agent published about itself.
    ///
    /// False for both declaration channels. [`StatusSource::Title`] reaches us
    /// through the output stream, but the sentence in it was written by the
    /// agent, and grouping it with the bell would lose the only distinction
    /// this predicate exists to make.
    pub fn is_observed(self) -> bool {
        !matches!(self, StatusSource::Hint | StatusSource::Title)
    }

    /// True when the status rests on a reading that can be wrong, rather than
    /// on a direct answer from the operating system, the agent's own hint, or
    /// the child's exit.
    ///
    /// A UI should mark these. [`StatusSource::Idle`] and
    /// [`StatusSource::Output`] can be wrong about the state: an agent thinking
    /// silently for a minute is inferred `Ready` and is actually working.
    /// [`StatusSource::Title`] can be wrong about the reading: the agent really
    /// did publish that banner, and our rule for what it means is a per-agent
    /// pattern rather than a protocol. Either way the honest UI hedges.
    ///
    /// On Linux and macOS the timing sources are never reached for a live
    /// session; on Windows they are the only path available. A title claim is
    /// platform-independent, so this is the one inferred source a Linux row can
    /// show.
    pub fn is_inferred(self) -> bool {
        matches!(
            self,
            StatusSource::Idle | StatusSource::Output | StatusSource::Title
        )
    }
}

/// A blocked state an agent published in its terminal title.
///
/// Only the two states the agent alone can know. A title never produces
/// `Working`, `Ready` or `Failed`: those are measured, the measurement is
/// better evidence than a banner, and a title rule that could set them would be
/// a second, worse status engine running beside the real one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TitleClaim {
    /// The agent is holding for the operator to approve something.
    Approval,
    /// The agent is holding for an answer.
    Input,
}

impl TitleClaim {
    /// The state this claim resolves to.
    pub fn status(self) -> SidebarStatus {
        match self {
            TitleClaim::Approval => SidebarStatus::Approval,
            TitleClaim::Input => SidebarStatus::Input,
        }
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
    pub(crate) const fn new(status: SidebarStatus, source: StatusSource) -> Self {
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
/// 5. **The hint channel outranks the title, on every state including
///    `working`.** Both are the agent talking, and when they disagree the one
///    the agent addressed to us is the one it meant. An agent that hints
///    `working` while its title still carries yesterday's banner is working:
///    the hint is a protocol it opted into, the banner is a string we matched.
///    A `working` hint that silence has already retired is no longer evidence
///    of anything, so a title claim is read after that point.
/// 6. **A title claim beats every observation.** It is a declaration, and the
///    states it can produce are exactly the ones observation cannot reach. It
///    is never retired by silence, for the same reason a blocking hint is not:
///    an agent waiting on a human emits nothing, and the claim ends when the
///    agent retitles.
/// 7. **Otherwise the observed signals decide**, in the order
///    [`Attention::priority`] ranks them: failure, then the operating system's
///    answer, then a bell, then unseen silence, then recent output. Proof beats
///    a beep: a session the OS reports as computing is `Working` even if it rang
///    the bell, and the bell still lifts it inside its band through
///    `Attention::priority`.
///
/// `title` is the caller's already-resolved claim rather than a raw string,
/// because reading a title is per agent: see
/// [`AgentKind::title_claim`](crate::agent::AgentKind::title_claim), which
/// [`SessionView::resolve_status`](crate::view::SessionView::resolve_status)
/// applies for every row.
///
/// Snooze does not appear here. Snoozing changes whether a row is settled and
/// where it sorts, not what the agent is doing.
pub fn resolve_status(
    session_status: &SessionStatus,
    attention: &Attention,
    hint: Option<HintState>,
    title: Option<TitleClaim>,
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

    if let Some(claim) = title {
        return StatusResolution::new(claim.status(), StatusSource::Title);
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
    /// are not derivable from anything we measure, not even with the syscall
    /// probe, because a shell at a prompt and an agent asking a question block
    /// in the same syscall. If someone later adds an inference path to either,
    /// this fails and forces the question back into review.
    ///
    /// The claim moved when titles became admissible, and it moved in one
    /// direction only: the two states now have a second DECLARATION channel,
    /// and still no observation channel. So the matrix below sweeps every
    /// observable input with both declaration channels silent, and the
    /// membership assertions are derived from
    /// [`SidebarStatus::requires_declaration`] over [`ALL_STATUSES`] rather
    /// than listing states, so a sixth state cannot join without a ruling.
    #[test]
    fn states_that_require_a_declaration_are_never_produced_without_one() {
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
                                None,
                            );
                            produced.push(resolved.status);
                            assert!(
                                resolved.source.is_observed(),
                                "undeclared resolution claimed a declared source: {resolved:?}"
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(produced.len(), 300);
        for status in ALL_STATUSES {
            assert_eq!(
                !produced.contains(&status),
                status.requires_declaration(),
                "{status:?} requires a declaration: {}, but the undeclared matrix \
                 produced it: {}",
                status.requires_declaration(),
                produced.contains(&status)
            );
        }
    }

    /// The other half of that partition: every state that requires a
    /// declaration must be reachable from BOTH declaration channels, or the
    /// predicate is describing a state nothing can produce.
    ///
    /// Derived from [`ALL_STATUSES`] and from the mappings, so adding a
    /// declarable state without wiring it to a channel fails here rather than
    /// shipping a state the sidebar can never show.
    #[test]
    fn every_state_that_requires_a_declaration_has_both_channels_wired() {
        let hinted = [
            (HintState::Approval, SidebarStatus::Approval),
            (HintState::Input, SidebarStatus::Input),
        ];
        let titled = [
            (TitleClaim::Approval, SidebarStatus::Approval),
            (TitleClaim::Input, SidebarStatus::Input),
        ];
        for status in ALL_STATUSES.iter().filter(|s| s.requires_declaration()) {
            let (hint, _) = hinted
                .iter()
                .find(|(_, produced)| produced == status)
                .unwrap_or_else(|| panic!("{status:?} requires a declaration but no hint reaches it"));
            let (claim, _) = titled
                .iter()
                .find(|(_, produced)| produced == status)
                .unwrap_or_else(|| panic!("{status:?} requires a declaration but no title reaches it"));
            assert_eq!(
                resolve_status(&SessionStatus::Running, &UNKNOWN, Some(*hint), None),
                StatusResolution::new(*status, StatusSource::Hint)
            );
            assert_eq!(
                resolve_status(&SessionStatus::Running, &UNKNOWN, None, Some(*claim)),
                StatusResolution::new(*status, StatusSource::Title)
            );
        }
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
        let resolved = resolve_status(&SessionStatus::Running, &observed, None, None);
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
        let quiet = resolve_status(&SessionStatus::Running, &UNKNOWN, None, None);
        assert_eq!(
            quiet,
            StatusResolution::new(SidebarStatus::Working, StatusSource::Output)
        );
        assert!(quiet.source.is_inferred());

        let silent = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, None),
            None,
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
            let resolved = resolve_status(&lifecycle, &UNKNOWN, None, None);
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
        let clean = resolve_status(&SessionStatus::Exited { code: Some(0) }, &UNKNOWN, None, None);
        assert_eq!(clean, StatusResolution::new(SidebarStatus::Ready, StatusSource::Exit));

        let nonzero = resolve_status(&SessionStatus::Exited { code: Some(1) }, &UNKNOWN, None, None);
        assert_eq!(nonzero, StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit));

        let signalled = resolve_status(&SessionStatus::Exited { code: None }, &UNKNOWN, None, None);
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
            resolve_status(&SessionStatus::Running, &observed, None, None).status,
            SidebarStatus::Ready
        );
        assert_eq!(
            resolve_status(&SessionStatus::Running, &observed, Some(HintState::Approval), None),
            StatusResolution::new(SidebarStatus::Approval, StatusSource::Hint)
        );
        assert_eq!(
            resolve_status(&SessionStatus::Running, &observed, Some(HintState::Input), None),
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
                let resolved = resolve_status(&SessionStatus::Running, &observed, Some(state), None);
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
                resolve_status(&SessionStatus::Exited { code: Some(0) }, &UNKNOWN, Some(state), None);
            assert_eq!(
                clean,
                StatusResolution::new(SidebarStatus::Ready, StatusSource::Exit),
                "hint {state:?} survived a clean exit"
            );

            let crashed =
                resolve_status(&SessionStatus::Exited { code: Some(2) }, &UNKNOWN, Some(state), None);
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
            None,
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
            None,
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
            None,
        );
        assert_eq!(fresh, StatusResolution::new(SidebarStatus::Working, StatusSource::Hint));

        let stale = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, Some(true)),
            Some(HintState::Working),
            None,
        );
        assert_eq!(stale, StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting));

        // Retired, but the probe can still say it is genuinely computing, in
        // which case the row stays Working with an observed source.
        let still_busy = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, Some(false)),
            Some(HintState::Working),
            None,
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
                None,
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
            let urgency = resolve_status(&SessionStatus::Running, &observed, None, None)
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

    /// Two sources are declarations and three are readings that can be wrong.
    /// A UI that marks declared and uncertain states differently depends on
    /// this split being exact.
    ///
    /// The expectation is an EXHAUSTIVE match rather than a list, so a new
    /// source cannot be added without ruling on both predicates: the crate
    /// stops compiling until it is classified, and [`ALL_STATUS_SOURCES`]
    /// stops matching until it is enumerated.
    #[test]
    fn status_sources_partition_into_declared_proven_and_inferred() {
        // (is_observed, is_inferred) per source, written out.
        let expected = |source: StatusSource| match source {
            StatusSource::Exit => (true, false),
            StatusSource::Waiting => (true, false),
            StatusSource::Foreground => (true, false),
            StatusSource::Bell => (true, false),
            StatusSource::Idle => (true, true),
            StatusSource::Output => (true, true),
            // Declared, and hedged: the agent published the banner, we
            // interpreted it.
            StatusSource::Title => (false, true),
            // Declared, and not hedged: the agent addressed it to us.
            StatusSource::Hint => (false, false),
        };
        for source in ALL_STATUS_SOURCES {
            assert_eq!(
                (source.is_observed(), source.is_inferred()),
                expected(source),
                "{source:?} is classified differently from its ruling"
            );
        }
        assert_eq!(
            ALL_STATUS_SOURCES
                .iter()
                .filter(|source| !source.is_observed())
                .count(),
            2,
            "exactly two declaration channels: the hint and the title"
        );
        assert_eq!(
            ALL_STATUS_SOURCES
                .iter()
                .filter(|source| source.is_inferred())
                .count(),
            3
        );
    }

    /// On Linux and macOS a live session is never inferred from timing, because
    /// the probe always answers. That is the platform guarantee the UI leans on
    /// to avoid showing an uncertainty marker where there is no uncertainty.
    ///
    /// A title claim is the one exception, and it is deliberate: it is
    /// available on every platform and it is hedged on every platform, because
    /// what is uncertain about it is the reading rather than the probe.
    #[test]
    fn a_live_session_with_a_probe_answer_is_never_inferred_unless_it_was_titled() {
        for waiting in [Some(true), Some(false)] {
            for bell in [false, true] {
                for idle_ms in [0, IDLE_ATTENTION_MS * 10] {
                    let observed = attention(bell, idle_ms, false, waiting);
                    let resolved =
                        resolve_status(&SessionStatus::Running, &observed, None, None);
                    assert!(
                        !resolved.source.is_inferred(),
                        "inferred with a probe answer: {resolved:?}"
                    );

                    let titled = resolve_status(
                        &SessionStatus::Running,
                        &observed,
                        None,
                        Some(TitleClaim::Approval),
                    );
                    assert_eq!(
                        titled,
                        StatusResolution::new(SidebarStatus::Approval, StatusSource::Title)
                    );
                    assert!(
                        titled.source.is_inferred(),
                        "a title claim must hedge even where the probe answers"
                    );
                }
            }
        }
    }

    /// The headline case, end to end at this layer: a session whose agent
    /// published a blocked banner resolves to that state, with the title source
    /// and the hedge, beating every observation including the probe that says
    /// the process is merely blocked reading the terminal.
    ///
    /// Before this rule the same inputs resolved to `Ready`, which is the exact
    /// wrong answer: the pane holds "Would you like to run the following
    /// command?" and the row says nothing is wanted.
    #[test]
    fn a_title_claim_beats_every_live_observation() {
        for (claim, expected) in [
            (TitleClaim::Approval, SidebarStatus::Approval),
            (TitleClaim::Input, SidebarStatus::Input),
        ] {
            for observed in [
                UNKNOWN,
                attention(true, 0, false, None),
                attention(false, IDLE_ATTENTION_MS * 100, false, None),
                attention(false, 0, false, Some(false)),
                attention(false, 0, false, Some(true)),
                attention(true, IDLE_ATTENTION_MS * 100, false, Some(false)),
            ] {
                let resolved =
                    resolve_status(&SessionStatus::Running, &observed, None, Some(claim));
                assert_eq!(
                    resolved,
                    StatusResolution::new(expected, StatusSource::Title),
                    "title claim {claim:?} lost to {observed:?}"
                );
                assert!(resolved.source.is_inferred());
                assert!(!resolved.source.is_observed());
            }
        }
    }

    /// PRECEDENCE. A hint is the agent addressing us on a channel it opted
    /// into; a title is a banner we recognised. When they disagree the hint
    /// wins, on every state including `working`.
    ///
    /// The live shape of the bug: an agent whose title still carries the
    /// approval banner from the gate you just answered, while it hints
    /// `working` because it is off running the command. Reading the title there
    /// would park "Needs approval" on a row that is busy, which is the same
    /// class of lie as missing the gate in the first place.
    #[test]
    fn a_live_hint_beats_a_title_claim_on_every_state() {
        for (hint, expected) in [
            (HintState::Working, SidebarStatus::Working),
            (HintState::Ready, SidebarStatus::Ready),
            (HintState::Approval, SidebarStatus::Approval),
            (HintState::Input, SidebarStatus::Input),
        ] {
            for claim in [TitleClaim::Approval, TitleClaim::Input] {
                let resolved = resolve_status(
                    &SessionStatus::Running,
                    &attention(false, 0, false, Some(true)),
                    Some(hint),
                    Some(claim),
                );
                assert_eq!(
                    resolved,
                    StatusResolution::new(expected, StatusSource::Hint),
                    "title {claim:?} overrode the deliberate hint {hint:?}"
                );
            }
        }
    }

    /// A `working` hint that silence has already retired is no longer evidence,
    /// so the title is read at that point. The precedence above is between two
    /// LIVE declarations, not between a title and a declaration we have
    /// ourselves decided to stop believing.
    #[test]
    fn a_retired_working_hint_yields_to_a_title_claim() {
        let fresh = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS - 1, false, Some(true)),
            Some(HintState::Working),
            Some(TitleClaim::Approval),
        );
        assert_eq!(fresh, StatusResolution::new(SidebarStatus::Working, StatusSource::Hint));

        let retired = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS, false, Some(true)),
            Some(HintState::Working),
            Some(TitleClaim::Approval),
        );
        assert_eq!(
            retired,
            StatusResolution::new(SidebarStatus::Approval, StatusSource::Title)
        );
    }

    /// A title claim is never retired by silence, for the same reason a
    /// blocking hint is not: an agent waiting on a human emits nothing. The
    /// claim ends when the agent retitles, which is tested as a transition in
    /// `crate::view`.
    #[test]
    fn a_title_claim_is_exempt_from_the_staleness_rule() {
        let resolved = resolve_status(
            &SessionStatus::Running,
            &attention(false, IDLE_ATTENTION_MS * 1000, false, Some(true)),
            None,
            Some(TitleClaim::Approval),
        );
        assert_eq!(
            resolved,
            StatusResolution::new(SidebarStatus::Approval, StatusSource::Title)
        );
    }

    /// A dead process is not waiting for your approval, whatever its last title
    /// said. Titles outlive the process that set them — the terminal keeps the
    /// string — so without this a crashed Codex session would keep an "act now"
    /// badge forever.
    #[test]
    fn an_exit_overrides_a_stale_title_claim() {
        for claim in [TitleClaim::Approval, TitleClaim::Input] {
            assert_eq!(
                resolve_status(
                    &SessionStatus::Exited { code: Some(0) },
                    &UNKNOWN,
                    None,
                    Some(claim),
                ),
                StatusResolution::new(SidebarStatus::Ready, StatusSource::Exit)
            );
            assert_eq!(
                resolve_status(
                    &SessionStatus::Exited { code: Some(2) },
                    &UNKNOWN,
                    None,
                    Some(claim),
                ),
                StatusResolution::new(SidebarStatus::Failed, StatusSource::Exit)
            );
        }
    }
}
