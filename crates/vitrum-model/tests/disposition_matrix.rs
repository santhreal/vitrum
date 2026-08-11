//! What it actually takes to put a session row into each [`Disposition`].
//!
//! The complaint this file answers is that [`Disposition::Woke`] had never been
//! demonstrated. It had never been demonstrated by hand because the shortest
//! snooze preset is one hour (`snooze_presets` offers nothing shorter), so the
//! timer path to `Woke` costs an operator 3_600_000 ms of waiting. Three other
//! stimuli reach it in one second, and one stimulus that looks like it should
//! reach it does not, because of the order the resolution rules run in.
//!
//! Compiled as a separate crate, so every fact below is reachable through the
//! public API: nothing here can be proven with a private helper the shipped
//! client cannot call.
//!
//! # The resolution order, and what it costs
//!
//! [`SessionView::disposition`] answers in a fixed order, and only the first
//! rule that fires is visible:
//!
//! 1. blocked on the operator -> `Active`
//! 2. effective snooze -> `Snoozed`
//! 3. unseen wake -> `Woke`
//! 4. explicit override -> `Settled` / `Active`
//! 5. unseen completion -> `Active`
//! 6. dead process -> `Settled`
//! 7. inactivity past the window -> `Settled`
//!
//! Rule 1 sits above rule 3, so a parked row that declares `approval` over OSC
//! 7373 renders `Active` even though [`SessionView::has_unseen_wake`] is true
//! for it. That is not a bug and it is not a badge: it is the reason an
//! operator who parks a row and then answers its approval prompt never sees
//! `Woke`. The matrix below pins both halves of that at once.

use vitrum_model::{
    Clock, Disposition, DispositionPolicy, Section, SessionView, SettleOverride, SidebarStatus,
    Snooze, SnoozePresetId, snooze_presets, wake_countdown_label,
};
use vitrum_proto::{
    AgentHint, Attention, HintState, ProjectId, SessionId, SessionInfo, SessionStatus,
};

const NOW: u64 = 1_772_580_600_000;
const SECOND: u64 = 1_000;
const MINUTE: u64 = 60 * SECOND;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

/// Auto-settle off, so a case that settles proves it did so for the reason the
/// case names rather than because a week elapsed.
fn manual() -> DispositionPolicy {
    DispositionPolicy::manual()
}

/// The shipped default: seven days of inactivity.
fn week() -> DispositionPolicy {
    DispositionPolicy::default()
}

/// A deliberately short window, so an inactivity case does not need a
/// week-wide clock to make its point.
fn hourly() -> DispositionPolicy {
    DispositionPolicy {
        auto_settle_after_ms: Some(HOUR),
    }
}

/// One row under construction, with the fields a disposition depends on and no
/// others.
struct Row(SessionView);

impl Row {
    /// A live agent that is computing, quiet, seen, unhinted and never parked.
    fn new() -> Self {
        Row(SessionView::new(SessionInfo {
            id: SessionId(1),
            project_id: ProjectId(1),
            title: "agent".to_string(),
            cwd: "/srv/work".to_string(),
            command: "claude".to_string(),
            args: Vec::new(),
            status: SessionStatus::Running,
            created_at_ms: NOW - DAY,
            last_activity_ms: NOW - MINUTE,
            cols: 120,
            rows: 40,
            git_branch: Some("main".to_string()),
            worktree: None,
            unread: false,
            attention: Attention {
                bell: false,
                idle_ms: 0,
                failed: false,
                waiting: Some(false),
            },
            hint: None,
            term_title: None,
        }))
    }

    /// The child exited.
    ///
    /// Mirrors `vitrum_core::Session::finish`: a dead row carries no foreground
    /// process, so the probe answer is cleared rather than left stale, and a
    /// nonzero code raises `failed`. Building a dead row with `waiting:
    /// Some(true)` would model a state the daemon cannot produce.
    fn exited(mut self, code: i32) -> Self {
        self.0.info.status = SessionStatus::Exited { code: Some(code) };
        self.0.info.attention.failed = code != 0;
        self.0.info.attention.waiting = None;
        self
    }

    /// The operating system's answer to "is the foreground process blocked
    /// reading the terminal". `None` is a platform that cannot tell.
    fn waiting(mut self, waiting: Option<bool>) -> Self {
        self.0.info.attention.waiting = waiting;
        self
    }

    /// An OSC 7373 declaration landing at `received_at_ms`.
    fn hint(mut self, state: HintState, received_at_ms: u64) -> Self {
        self.0.info.hint = Some(AgentHint {
            state,
            label: None,
            received_at_ms,
        });
        self
    }

    /// Output arrived while nobody was watching this session.
    fn unread(mut self) -> Self {
        self.0.info.unread = true;
        self
    }

    fn last_activity(mut self, last_activity_ms: u64) -> Self {
        self.0.info.last_activity_ms = last_activity_ms;
        self
    }

    fn visited(mut self, visited_ms: u64) -> Self {
        self.0.last_visited_ms = Some(visited_ms);
        self
    }

    fn park(mut self, snoozed_at_ms: u64, wake_at_ms: u64) -> Self {
        self.0.snooze = Some(Snooze {
            snoozed_at_ms,
            wake_at_ms,
        });
        self
    }

    fn ruled(mut self, settle_override: SettleOverride) -> Self {
        self.0.settle_override = Some(settle_override);
        self
    }

    fn build(self) -> SessionView {
        self.0
    }
}

/// One reproducible row and every operator-visible answer derived from it.
struct Case {
    name: &'static str,
    /// The defect this row would let through if the rule it pins were lost.
    locks_out: &'static str,
    row: SessionView,
    at: Clock,
    policy: DispositionPolicy,
    want: Disposition,
    /// The badge text. Asserted separately from `want` because the label is the
    /// only part of a disposition an operator can actually read.
    want_label: Option<&'static str>,
    want_section: Section,
    want_status: SidebarStatus,
    /// The exact instant [`SessionView::woke_at`] reports, which is the
    /// difference between an early wake and a scheduled one.
    want_woke_at: Option<u64>,
    want_unseen_wake: bool,
    /// The separate unseen-completion badge, which is not the same fact as the
    /// disposition and is not the same fact as unread.
    want_completion: bool,
}

fn matrix() -> Vec<Case> {
    vec![
        // Start a session and let it run. Locks out the resting state acquiring
        // a badge it has not earned.
        Case {
            name: "active/computing",
            locks_out: "a badge on every row, which is a badge on no row",
            row: Row::new()
                .waiting(Some(false))
                .last_activity(NOW - MINUTE)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Working,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // Let the agent finish its turn, then open the row. Locks out silence
        // being read as a completion: a live unhinted session that has merely
        // gone quiet has no completion instant to name.
        Case {
            name: "active/resting-at-the-prompt",
            locks_out: "an unhinted lull minting a completion badge that can never clear",
            row: Row::new()
                .waiting(Some(true))
                .last_activity(NOW - 2 * MINUTE)
                .visited(NOW - MINUTE)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // Settle a row, then have its agent ask for approval. Locks out an
        // approval prompt staying buried under the fold because the operator
        // settled a different situation earlier.
        Case {
            name: "active/blocked-beats-an-explicit-settle",
            locks_out: "a pending approval hidden below the fold by a stale settle",
            row: Row::new()
                .waiting(Some(true))
                .hint(HintState::Approval, NOW - MINUTE)
                .last_activity(NOW - MINUTE)
                .visited(NOW)
                .ruled(SettleOverride::Settled)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Approval,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // A session that died four days ago and was never opened, under a
        // one-hour window. Locks out the inbox draining something nobody looked
        // at, which is the one thing an inbox must not do.
        Case {
            name: "active/unseen-completion-outlives-the-window",
            locks_out: "auto-settle discarding a failure the operator never saw",
            row: Row::new()
                .exited(1)
                .last_activity(NOW - 100 * HOUR)
                .unread()
                .build(),
            at: Clock::utc(NOW),
            policy: hourly(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Failed,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: true,
        },
        // Pin a week-idle row open. Locks out an explicit "keep this up" being
        // overruled by the automatic window.
        Case {
            name: "active/pinned-open-against-the-window",
            locks_out: "auto-settle overruling the operator's explicit hold",
            row: Row::new()
                .waiting(Some(true))
                .last_activity(NOW - 8 * DAY)
                .visited(NOW - 8 * DAY + SECOND)
                .ruled(SettleOverride::Active)
                .build(),
            at: Clock::utc(NOW),
            policy: week(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // A silent long-running job. Locks out draining a session out from
        // under work that is still in flight.
        Case {
            name: "active/computing-is-never-drained",
            locks_out: "a month-long silent build being settled while it runs",
            row: Row::new()
                .waiting(Some(false))
                .last_activity(NOW - 30 * DAY)
                .build(),
            at: Clock::utc(NOW),
            policy: week(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Working,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // Right-click a working row, Snooze, "In 1 hour". Locks out the park
        // failing to take on a session that is mid-turn.
        Case {
            name: "snoozed/parked-just-now",
            locks_out: "a snooze refusing to hide a row that is still computing",
            row: Row::new()
                .waiting(Some(false))
                .last_activity(NOW - 10 * MINUTE)
                .park(NOW, NOW + HOUR)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Snoozed,
            want_label: Some("Snoozed"),
            want_section: Section::Snoozed,
            want_status: SidebarStatus::Working,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // Park a row whose last output landed at the same instant the park did.
        // Locks out the freshness comparison going non-strict, which would make
        // parking an idle session an instant no-op: the row would raise its
        // hand on the output that was already there when you parked it.
        Case {
            name: "snoozed/already-at-the-prompt-when-parked",
            locks_out: "snoozing an idle row waking it again in the same instant",
            row: Row::new()
                .waiting(Some(true))
                .last_activity(NOW)
                .park(NOW, NOW + HOUR)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Snoozed,
            want_label: Some("Snoozed"),
            want_section: Section::Snoozed,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // Park a row that failed an hour before you parked it. Locks out the
        // fresh-failure rule collapsing into "is it failed", which would make
        // snooze useless for exactly the rows people most want to park. Note
        // the completion badge stays lit: the row is parked, not acknowledged.
        Case {
            name: "snoozed/a-failure-older-than-the-park",
            locks_out: "\"I saw it, not now\" being undone the instant it is said",
            row: Row::new()
                .exited(1)
                .last_activity(NOW - 2 * HOUR)
                .unread()
                .park(NOW - HOUR, NOW + HOUR)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Snoozed,
            want_label: Some("Snoozed"),
            want_section: Section::Snoozed,
            want_status: SidebarStatus::Failed,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: true,
        },
        // The same output-since-the-park story on Windows, where ConPTY cannot
        // answer the probe. Locks out UNKNOWN being read as "blocked on the
        // terminal", which would un-park every noisy row on that platform.
        Case {
            name: "snoozed/an-unknown-probe-raises-no-hand",
            locks_out: "Option::None being treated as Some(true) on a platform that cannot tell",
            row: Row::new()
                .waiting(None)
                .last_activity(NOW - MINUTE)
                .park(NOW - 10 * MINUTE, NOW + HOUR)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Snoozed,
            want_label: Some("Snoozed"),
            want_section: Section::Snoozed,
            want_status: SidebarStatus::Working,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // The hour elapses with nothing mutated and nothing scheduled. Locks
        // out a wake that needs an event to fire, and pins the reported
        // instant to the scheduled time rather than to "now".
        Case {
            name: "woke/the-timer-elapsed",
            locks_out: "a snooze that only expires if something fires a timer",
            row: Row::new()
                .waiting(Some(false))
                .last_activity(NOW - HOUR)
                .park(NOW - HOUR, NOW)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Woke,
            want_label: Some("Woke"),
            want_section: Section::Active,
            want_status: SidebarStatus::Working,
            want_woke_at: Some(NOW),
            want_unseen_wake: true,
            want_completion: false,
        },
        // Park a row, open it, then let the child exit. Locks out a visit made
        // BEFORE the trigger suppressing the badge: looking at a parked row is
        // not the same as having seen the thing that woke it.
        Case {
            name: "woke/the-child-exited-after-the-park",
            locks_out: "a pre-trigger visit swallowing the only notice the row came back",
            row: Row::new()
                .exited(0)
                .last_activity(NOW - MINUTE)
                .unread()
                .visited(NOW - 5 * MINUTE)
                .park(NOW - 10 * MINUTE, NOW + 50 * MINUTE)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Woke,
            want_label: Some("Woke"),
            want_section: Section::Active,
            want_status: SidebarStatus::Ready,
            want_woke_at: Some(NOW - MINUTE),
            want_unseen_wake: true,
            want_completion: true,
        },
        // A harness that opted in declares `ready` after the park. Locks out
        // the declaration channel being ignored by the raised-hand rule.
        Case {
            name: "woke/a-ready-declaration-after-the-park",
            locks_out: "an agent announcing it finished and the parked row staying hidden",
            row: Row::new()
                .waiting(Some(true))
                .hint(HintState::Ready, NOW - 2 * MINUTE)
                .last_activity(NOW - 2 * MINUTE)
                .unread()
                .park(NOW - 20 * MINUTE, NOW + 40 * MINUTE)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Woke,
            want_label: Some("Woke"),
            want_section: Section::Active,
            want_status: SidebarStatus::Ready,
            want_woke_at: Some(NOW - 2 * MINUTE),
            want_unseen_wake: true,
            want_completion: true,
        },
        // An agent that has never emitted a hint prints its prompt after the
        // park and blocks reading the terminal. Locks out the loss of the one
        // wake path that needs no harness integration at all. It carries NO
        // completion badge, because a PTY cannot name the instant a turn ended.
        Case {
            name: "woke/an-unhinted-agent-returned-to-its-prompt",
            locks_out: "the kernel-observed end of turn being dropped as a wake trigger",
            row: Row::new()
                .waiting(Some(true))
                .last_activity(NOW - 30 * SECOND)
                .unread()
                .park(NOW - 5 * MINUTE, NOW + 55 * MINUTE)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Woke,
            want_label: Some("Woke"),
            want_section: Section::Active,
            want_status: SidebarStatus::Ready,
            want_woke_at: Some(NOW - 30 * SECOND),
            want_unseen_wake: true,
            want_completion: false,
        },
        // THE ORDERING TRAP. A parked row declares `approval`. The wake is real
        // and unseen, but rule 1 fires before rule 3, so the row renders Active
        // with no badge. Locks out anyone "fixing" this by reordering the rules
        // and thereby labelling a live approval prompt "Woke".
        Case {
            name: "active/approval-on-a-parked-row-never-reads-woke",
            locks_out: "a pending approval rendering as Woke instead of as the request it is",
            row: Row::new()
                .waiting(Some(true))
                .hint(HintState::Approval, NOW - MINUTE)
                .last_activity(NOW - MINUTE)
                .park(NOW - 10 * MINUTE, NOW + 50 * MINUTE)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Approval,
            want_woke_at: Some(NOW - MINUTE),
            want_unseen_wake: true,
            want_completion: false,
        },
        // The child exited and you opened the row afterwards. Locks out a dead,
        // acknowledged row clinging to the inbox.
        Case {
            name: "settled/an-acknowledged-exit",
            locks_out: "the inbox never draining, which is the failure the whole axis exists for",
            row: Row::new()
                .exited(0)
                .last_activity(NOW - 10 * MINUTE)
                .visited(NOW - MINUTE)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Settled,
            want_label: Some("Settled"),
            want_section: Section::Settled,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // Select a resting row and settle it. Locks out the explicit ruling
        // being ignored for a session that is alive and perfectly healthy.
        Case {
            name: "settled/an-explicit-ruling",
            locks_out: "\"I am done with this\" failing on a live session",
            row: Row::new()
                .waiting(Some(true))
                .last_activity(NOW - MINUTE)
                .visited(NOW)
                .ruled(SettleOverride::Settled)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Settled,
            want_label: Some("Settled"),
            want_section: Section::Settled,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // Eight days of silence on a live shell, under the shipped seven-day
        // window. Locks out a month-old experiment sitting in the inbox forever.
        Case {
            name: "settled/inactivity-past-the-window",
            locks_out: "a forgotten shell holding an inbox slot indefinitely",
            row: Row::new()
                .waiting(Some(true))
                .last_activity(NOW - 8 * DAY)
                .visited(NOW - 8 * DAY + SECOND)
                .build(),
            at: Clock::utc(NOW),
            policy: week(),
            want: Disposition::Settled,
            want_label: Some("Settled"),
            want_section: Section::Settled,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // The operator watched the child exit with the tab open, so the daemon
        // never raised `unread`. Locks out a completion badge on a completion
        // the operator literally watched happen.
        Case {
            name: "settled/an-exit-watched-live-carries-no-badge",
            locks_out: "a \"finished while you were away\" badge on work you sat and watched",
            row: Row::new().exited(0).last_activity(NOW - MINUTE).build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Settled,
            want_label: Some("Settled"),
            want_section: Section::Settled,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // A working agent producing output nobody has read. Locks out unread
        // and unseen-completion collapsing into one flag, which would bury the
        // one row the operator opened the sidebar to find under every chatty one.
        Case {
            name: "active/unread-output-is-not-a-completion",
            locks_out: "a streaming agent wearing the badge that means \"this finished\"",
            row: Row::new()
                .waiting(Some(false))
                .unread()
                .last_activity(NOW - 5 * SECOND)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Working,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
        // The mirror: a completion the operator has not seen, on a row the
        // daemon does not consider unread because the tab was subscribed.
        // Locks out the completion badge being derived from `unread` alone.
        Case {
            name: "active/a-completion-after-the-last-visit",
            locks_out: "the completion badge collapsing into the unread dot",
            row: Row::new()
                .waiting(Some(true))
                .hint(HintState::Ready, NOW - MINUTE)
                .last_activity(NOW - MINUTE)
                .visited(NOW - 5 * MINUTE)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: true,
        },
        // Open the row after it finished. Locks out a completion badge that no
        // amount of looking can retire.
        Case {
            name: "active/a-completion-cleared-by-a-later-visit",
            locks_out: "a permanent completion badge that a visit cannot clear",
            row: Row::new()
                .waiting(Some(true))
                .hint(HintState::Ready, NOW - 5 * MINUTE)
                .last_activity(NOW - 5 * MINUTE)
                .visited(NOW)
                .build(),
            at: Clock::utc(NOW),
            policy: manual(),
            want: Disposition::Active,
            want_label: None,
            want_section: Section::Active,
            want_status: SidebarStatus::Ready,
            want_woke_at: None,
            want_unseen_wake: false,
            want_completion: false,
        },
    ]
}

/// Every row above, checked on all seven answers a sidebar draws from it.
///
/// One table rather than twenty-two functions because the value of a matrix is
/// that neighbouring states are visible side by side: the difference between
/// `snoozed/a-failure-older-than-the-park` and `woke/the-child-exited-after-the-park`
/// is one timestamp comparison, and reading them apart hides that.
#[test]
fn every_disposition_has_a_reproducible_row() {
    for case in matrix() {
        let Case {
            name,
            locks_out,
            row,
            at,
            policy,
            want,
            want_label,
            want_section,
            want_status,
            want_woke_at,
            want_unseen_wake,
            want_completion,
        } = case;

        let got = row.disposition(at, policy);
        assert_eq!(got, want, "{name}: would allow {locks_out}");
        assert_eq!(got.label(), want_label, "{name}: badge text");
        assert_eq!(row.section(at, policy), want_section, "{name}: band");
        assert_eq!(row.status(), want_status, "{name}: agent-axis status");
        assert_eq!(
            row.woke_at(at),
            want_woke_at,
            "{name}: reported wake instant"
        );
        assert_eq!(
            row.has_unseen_wake(at),
            want_unseen_wake,
            "{name}: unseen wake"
        );
        assert_eq!(
            row.has_unseen_completion(),
            want_completion,
            "{name}: unseen-completion badge"
        );
    }

    for want in [
        Disposition::Active,
        Disposition::Woke,
        Disposition::Snoozed,
        Disposition::Settled,
    ] {
        assert!(
            matrix().iter().any(|case| case.want == want),
            "no row in the matrix reaches {want:?}, so that state is undemonstrated"
        );
    }
}

/// The timer path to `Woke` costs an operator a full hour, because the menu
/// offers nothing shorter.
///
/// Locks out a preset table that silently loses its floor, and records the
/// measured cost that makes the other three stimuli worth knowing about.
#[test]
fn the_shortest_park_a_hand_can_make_is_one_hour() {
    let clock = Clock::utc(NOW);
    let presets = snooze_presets(clock);
    let soonest = presets
        .iter()
        .min_by_key(|preset| preset.wake_at_ms)
        .expect("the menu always offers at least the hour preset");

    assert_eq!(soonest.id, SnoozePresetId::Hour);
    assert_eq!(soonest.label, "In 1 hour");
    assert_eq!(soonest.wake_at_ms, NOW + HOUR);
    assert_eq!(wake_countdown_label(soonest.wake_at_ms, NOW), "1h");

    let row = Row::new()
        .waiting(Some(false))
        .last_activity(NOW - MINUTE)
        .park(NOW, soonest.wake_at_ms)
        .build();

    // One millisecond short of the hour, and exactly on it. `is_asleep` is
    // strictly `now < wake`, so the row wakes at its own instant.
    assert_eq!(
        row.disposition(Clock::utc(NOW + HOUR - 1), manual()),
        Disposition::Snoozed
    );
    assert_eq!(
        row.disposition(Clock::utc(NOW + HOUR), manual()),
        Disposition::Woke
    );
    assert_eq!(row.woke_at(Clock::utc(NOW + HOUR)), Some(NOW + HOUR));
}

/// Three stimuli reach `Woke` one second after the park, and the fourth
/// candidate does not reach it at all.
///
/// Locks out the belief that `Woke` requires waiting out a snooze, and locks
/// out the reordering that would make a live approval prompt render as `Woke`.
#[test]
fn three_stimuli_wake_a_parked_row_in_a_second_and_approval_does_not() {
    let parked_at = NOW;
    let wake_at = NOW + HOUR;
    let fired_at = NOW + SECOND;
    let at = Clock::utc(NOW + 2 * SECOND);

    let by_exit = Row::new()
        .park(parked_at, wake_at)
        .exited(0)
        .last_activity(fired_at)
        .unread()
        .build();
    let by_declaration = Row::new()
        .park(parked_at, wake_at)
        .waiting(Some(true))
        .hint(HintState::Ready, fired_at)
        .last_activity(fired_at)
        .unread()
        .build();
    let by_prompt = Row::new()
        .park(parked_at, wake_at)
        .waiting(Some(true))
        .last_activity(fired_at)
        .unread()
        .build();

    for (stimulus, row) in [
        ("the child exits", by_exit),
        ("the agent declares ready", by_declaration),
        ("an unhinted agent returns to its prompt", by_prompt),
    ] {
        assert!(
            row.is_asleep(at),
            "{stimulus}: the scheduled wake must still be 59 minutes away"
        );
        assert!(row.raised_hand(), "{stimulus}: the hand must be up");
        assert_eq!(
            row.disposition(at, manual()),
            Disposition::Woke,
            "{stimulus}"
        );
        assert_eq!(
            row.disposition(at, manual()).label(),
            Some("Woke"),
            "{stimulus}"
        );
        assert_eq!(
            row.woke_at(at),
            Some(fired_at),
            "{stimulus}: the wake instant is the trigger, not the scheduled {wake_at}"
        );
        assert_eq!(
            row.snooze.expect("the park is never mutated").wake_at_ms,
            wake_at,
            "{stimulus}: raising a hand must not rewrite the operator's park"
        );
    }

    // Same park, same instant, one different declaration: rule 1 answers first.
    let by_approval = Row::new()
        .park(parked_at, wake_at)
        .waiting(Some(true))
        .hint(HintState::Approval, fired_at)
        .last_activity(fired_at)
        .build();
    assert_eq!(by_approval.status(), SidebarStatus::Approval);
    assert_eq!(by_approval.woke_at(at), Some(fired_at));
    assert!(
        by_approval.has_unseen_wake(at),
        "the wake is real; it is simply outranked"
    );
    assert_eq!(by_approval.disposition(at, manual()), Disposition::Active);
    assert_eq!(by_approval.disposition(at, manual()).label(), None);
    assert!(
        !by_approval.can_snooze(),
        "re-parking a row blocked on you would be a lie"
    );
}

/// A visit before the trigger does not clear the badge; a visit after it does,
/// and the scheduled wake passing later does not bring it back.
///
/// Locks out both halves of the clear-on-visit rule: a badge that can never be
/// retired, and a retired badge that resurrects itself on the original timer.
#[test]
fn only_a_visit_after_the_trigger_retires_the_woke_badge() {
    let parked_at = NOW;
    let visited_at = NOW + 10 * MINUTE;
    let fired_at = NOW + 20 * MINUTE;
    let wake_at = NOW + HOUR;
    let at = Clock::utc(NOW + 25 * MINUTE);

    let unseen = Row::new()
        .park(parked_at, wake_at)
        .waiting(Some(true))
        .last_activity(fired_at)
        .unread()
        .visited(visited_at)
        .build();
    assert_eq!(unseen.woke_at(at), Some(fired_at));
    assert!(
        unseen.has_unseen_wake(at),
        "looking at a parked row is not seeing what later woke it"
    );
    assert_eq!(unseen.disposition(at, manual()), Disposition::Woke);
    assert_eq!(unseen.disposition(at, manual()).label(), Some("Woke"));

    let mut opened = unseen.clone();
    opened.last_visited_ms = Some(fired_at + 1);
    assert!(!opened.has_unseen_wake(at));
    assert_eq!(opened.disposition(at, manual()), Disposition::Active);
    assert_eq!(opened.disposition(at, manual()).label(), None);

    // The originally scheduled hour now elapses. The early wake stays
    // authoritative, so the cleared badge does not come back.
    let after_schedule = Clock::utc(wake_at + MINUTE);
    assert_eq!(
        opened.woke_at(after_schedule),
        Some(fired_at),
        "the wake was the trigger and there is only ever one wake"
    );
    assert!(!opened.has_unseen_wake(after_schedule));
    assert_eq!(
        opened.disposition(after_schedule, manual()),
        Disposition::Active
    );
}

/// Answering an approval on a parked row puts it back to sleep.
///
/// Locks out a stored raised-hand flag: raising is computed, so when the
/// trigger clears the operator's park is still in force and still has its
/// original wake time.
#[test]
fn a_parked_row_re_parks_itself_once_the_block_clears() {
    let wake_at = NOW + HOUR;

    let blocked = Row::new()
        .park(NOW, wake_at)
        .waiting(Some(true))
        .hint(HintState::Approval, NOW + SECOND)
        .last_activity(NOW + SECOND)
        .build();
    assert_eq!(
        blocked.disposition(Clock::utc(NOW + 2 * SECOND), manual()),
        Disposition::Active
    );

    // The operator approves; the agent goes back to work and says so.
    let resumed = Row::new()
        .park(NOW, wake_at)
        .waiting(Some(false))
        .hint(HintState::Working, NOW + 3 * SECOND)
        .last_activity(NOW + 3 * SECOND)
        .build();
    let at = Clock::utc(NOW + 4 * SECOND);
    assert_eq!(resumed.status(), SidebarStatus::Working);
    assert!(!resumed.raised_hand());
    assert_eq!(resumed.disposition(at, manual()), Disposition::Snoozed);
    assert_eq!(resumed.disposition(at, manual()).label(), Some("Snoozed"));
    assert_eq!(
        resumed
            .snooze
            .expect("the park survived the interruption")
            .wake_at_ms,
        wake_at
    );
}

/// The whole cycle on one row: Active, Snoozed, Woke, Active, then drained.
///
/// The last two steps lock out the defect that would make `Woke` useless: a
/// spent snooze whose stale fields mint a fresh badge on every later
/// completion for the rest of the row's life.
#[test]
fn a_row_walks_the_cycle_and_a_spent_park_mints_only_one_badge() {
    let mut row = Row::new().waiting(Some(false)).last_activity(NOW).build();
    let start = Clock::utc(NOW);
    assert_eq!(row.disposition(start, manual()), Disposition::Active);
    assert!(row.can_snooze());

    row.snooze = Some(Snooze {
        snoozed_at_ms: NOW,
        wake_at_ms: NOW + HOUR,
    });
    assert_eq!(row.disposition(start, manual()), Disposition::Snoozed);
    assert_eq!(row.disposition(start, manual()).label(), Some("Snoozed"));

    let woken = Clock::utc(NOW + HOUR);
    assert_eq!(row.disposition(woken, manual()), Disposition::Woke);
    assert_eq!(row.woke_at(woken), Some(NOW + HOUR));

    row.last_visited_ms = Some(NOW + HOUR + SECOND);
    assert_eq!(row.disposition(woken, manual()), Disposition::Active);
    assert_eq!(row.disposition(woken, manual()).label(), None);

    // An hour later the agent finishes. That is ordinary unseen work, not a
    // second wake.
    row.info.status = SessionStatus::Exited { code: Some(0) };
    row.info.attention.waiting = None;
    row.info.last_activity_ms = NOW + 2 * HOUR;
    row.info.unread = true;
    let done = Clock::utc(NOW + 3 * HOUR);
    assert_eq!(
        row.woke_at(done),
        Some(NOW + HOUR),
        "the one wake was the timer, an hour before this completion"
    );
    assert!(!row.has_unseen_wake(done));
    assert!(row.has_unseen_completion());
    assert_eq!(row.disposition(done, manual()), Disposition::Active);

    row.last_visited_ms = Some(NOW + 2 * HOUR + SECOND);
    row.info.unread = false;
    assert_eq!(row.disposition(done, manual()), Disposition::Settled);
    assert_eq!(row.disposition(done, manual()).label(), Some("Settled"));
}
