//! Disposition: the operator-owned axis, orthogonal to what the agent is doing.
//!
//! Ported from T3 Code's `threadSettled.ts`, which is the best idea in their
//! product. The sidebar is an inbox that drains, not a list that grows, and
//! that only works if two different facts are kept apart:
//!
//! - [`crate::status::SidebarStatus`] is what the AGENT is doing. Done means
//!   the agent finished.
//! - [`Disposition`] is what the OPERATOR has decided about it. Settled means
//!   *you* are finished with it.
//!
//! Conflating them is what makes a twenty-session list unusable: a session you
//! have read, judged and mentally closed keeps shouting because its process
//! exited badly, and a session you deliberately parked comes back the moment it
//! prints a line.
//!
//! ```text
//! Active  --snooze(until)-->  Snoozed  --timer or raised hand-->  Woke  --visit-->  Active
//!    |                                                                                |
//!    +----------------------------- settle / auto-settle -----------------------------+
//! ```
//!
//! # Six semantics worth copying exactly
//!
//! 1. **Raised hand.** A snoozed session un-snoozes early when something
//!    outranks the snooze. See [`SessionView::raised_hand`].
//! 2. **Fresh failures only.** A session snoozed while *already* failed stays
//!    snoozed: that snooze was the operator saying "I saw it, not now".
//! 3. **Timer wakes are derived.** Nothing fires when the wake time passes. The
//!    stale fields simply stop classifying as snoozed. There is no scheduler in
//!    this crate and there must never be one: a timer per snoozed session is
//!    exactly the idle CPU cost the product forbids.
//! 4. **You cannot snooze a session that is blocked on you.** Hiding a pending
//!    approval defeats the request. See [`SessionView::can_snooze`].
//! 5. **The inbox sort is static.** A woken session reappears in its original
//!    position and the [`Disposition::Woke`] indicator carries the weight.
//!    Rows jumping under the cursor is disorienting; see [`crate::order`].
//! 6. **Auto-settle has blockers.** Blocked work holds a session active
//!    regardless of any override; an explicit override then wins in both
//!    directions; otherwise inactivity past a window settles it.
//!
//! # Where we beat them for free
//!
//! T3 Code's raised hand needs per-harness events to know an agent is blocked
//! on the user. We hold the PTY, so [`Attention::waiting`](vitrum_proto::Attention::waiting)
//! tells us the foreground process went back to its prompt after the snooze was
//! set, for ANY agent, including ones nobody integrated. A snoozed session that
//! finishes its turn raises its hand whether or not it has ever heard of us.

use serde::{Deserialize, Serialize};

use crate::status::SidebarStatus;
use crate::view::{Clock, SessionView};

/// The operator's explicit ruling, which beats every automatic rule that is not
/// an activity blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettleOverride {
    /// "I am done with this." Settles regardless of the inactivity window.
    Settled,
    /// "Keep this up." Pins a session active and suppresses auto-settle.
    Active,
}

/// Where a session sits on the operator's axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Disposition {
    /// In the inbox.
    Active,
    /// In the inbox, and its snooze ended without the operator having looked
    /// since. Rendered as a loud badge precisely BECAUSE the sort did not move
    /// it: the row came back exactly where it was, so the badge is the only
    /// thing that can tell you it came back.
    Woke,
    /// Parked until its wake time, or until it raises its hand.
    Snoozed,
    /// Drained. Below the fold.
    Settled,
}

/// The three coarse bands the sidebar renders, in order.
///
/// [`Disposition::Woke`] belongs to [`Section::Active`]: a woken session is
/// back in the inbox, it simply arrives wearing a badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Section {
    Active,
    Snoozed,
    Settled,
}

impl Disposition {
    /// The band this disposition renders in.
    pub fn section(self) -> Section {
        match self {
            Disposition::Active | Disposition::Woke => Section::Active,
            Disposition::Snoozed => Section::Snoozed,
            Disposition::Settled => Section::Settled,
        }
    }

    /// Short operator-facing label. `Active` has none: it is the resting state
    /// and a badge on every row is a badge on no row.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Disposition::Active => None,
            Disposition::Woke => Some("Woke"),
            Disposition::Snoozed => Some("Snoozed"),
            Disposition::Settled => Some("Settled"),
        }
    }
}

/// Tuning for the automatic parts of the disposition rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispositionPolicy {
    /// Inactivity after which an unattended session settles itself. `None`
    /// disables auto-settle entirely, which is a legitimate choice: an operator
    /// who drains the inbox by hand should not have it drained behind them.
    pub auto_settle_after_ms: Option<u64>,
}

impl DispositionPolicy {
    /// Seven days. Long enough that nothing you are actually working on
    /// disappears, short enough that a month-old experiment does not sit in the
    /// inbox forever.
    pub const DEFAULT_AUTO_SETTLE_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

    /// Auto-settle disabled; only explicit rulings and acknowledged exits
    /// settle a row.
    pub fn manual() -> Self {
        DispositionPolicy {
            auto_settle_after_ms: None,
        }
    }
}

impl Default for DispositionPolicy {
    fn default() -> Self {
        DispositionPolicy {
            auto_settle_after_ms: Some(Self::DEFAULT_AUTO_SETTLE_MS),
        }
    }
}

impl SessionView {
    /// True when the agent is explicitly blocked on the operator.
    ///
    /// This is the activity blocker: it holds a session in the inbox regardless
    /// of a snooze or an explicit settle, and it forbids snoozing in the first
    /// place. Only the two declared states count. A session merely resting at
    /// its prompt is not blocked on you in this sense; it is finished, which is
    /// [`SidebarStatus::Ready`], and parking it is entirely reasonable.
    pub fn blocks_on_operator(&self) -> bool {
        matches!(
            self.status(),
            SidebarStatus::Approval | SidebarStatus::Input
        )
    }

    /// The instant a snoozed session raised its hand, if it has.
    ///
    /// Three triggers, each requiring that the event be NEWER than the snooze:
    ///
    /// 1. Work finished after the snooze. This is also the fresh-failure rule:
    ///    an exit stamps its completion at the exit instant, so a session
    ///    snoozed while ALREADY dead has a completion OLDER than its snooze and
    ///    stays parked. That is the point of comparing against `snoozed_at_ms`
    ///    rather than merely checking the failed flag.
    /// 2. The agent declared it is blocked on the operator. Nothing outranks
    ///    that, and [`SessionView::can_snooze`] guarantees it was not already
    ///    true when the snooze was set.
    /// 3. The foreground process went back to blocking on the terminal after
    ///    producing output since the snooze. This is the kernel telling us a
    ///    turn ended, for an agent that has never emitted a hint in its life.
    ///
    /// Checked in that order, because the instant reported has to be the most
    /// specific one available: a completion timestamp beats a declaration
    /// receipt, which beats "when output last stopped".
    fn raise_instant(&self) -> Option<u64> {
        let snooze = self.snooze?;
        if let Some(completed_at_ms) = self
            .completion_at_ms()
            .filter(|completed_at_ms| *completed_at_ms > snooze.snoozed_at_ms)
        {
            return Some(completed_at_ms);
        }
        if self.blocks_on_operator() {
            return Some(
                self.info
                    .hint
                    .as_ref()
                    .map_or(snooze.snoozed_at_ms, |hint| hint.received_at_ms),
            );
        }
        (self.info.attention.waiting == Some(true)
            && self.info.last_activity_ms > snooze.snoozed_at_ms)
            .then_some(self.info.last_activity_ms)
    }

    /// True when a snoozed session has raised its hand: something happened that
    /// outranks the operator's decision to park it.
    ///
    /// Raising a hand NEVER mutates the snooze. It is computed, not stored, so
    /// the operator's parked-until time survives and the row re-parks itself if
    /// the trigger clears.
    pub fn raised_hand(&self) -> bool {
        self.raise_instant().is_some()
    }

    /// True while a snooze is actually suppressing this row.
    ///
    /// Derived, never scheduled. The wake happens because this predicate starts
    /// answering false, not because anything fired. A snooze that has elapsed
    /// leaves its fields exactly as they were; they simply stop classifying.
    pub fn effective_snoozed(&self, clock: Clock) -> bool {
        let Some(snooze) = self.snooze else {
            return false;
        };
        if !snooze.is_asleep(clock.now_ms) {
            return false;
        }
        !self.raised_hand()
    }

    /// When a previously snoozed session woke, or `None` if it never snoozed or
    /// is still parked.
    ///
    /// A wake is ONE transition, and this reports the instant of that one
    /// transition rather than the latest interesting thing to happen:
    ///
    /// - A hand raised BEFORE the scheduled wake is an early wake, and it stays
    ///   authoritative afterwards. Reporting the scheduled time instead would
    ///   resurface a badge the operator already cleared, the moment the original
    ///   timer elapsed. It also means a visit made before the trigger does not
    ///   suppress the badge: looking at a parked row is not the same as having
    ///   seen the thing that woke it.
    /// - Otherwise the wake is the scheduled one. Events after it are ordinary
    ///   activity and belong to
    ///   [`has_unseen_completion`](SessionView::has_unseen_completion), not to
    ///   the snooze. Without this, a row's stale snooze fields would mint a
    ///   fresh Woke badge on every completion for the rest of its life.
    pub fn woke_at(&self, clock: Clock) -> Option<u64> {
        let snooze = self.snooze?;
        match self.raise_instant() {
            Some(raised_at_ms) if raised_at_ms < snooze.wake_at_ms => Some(raised_at_ms),
            _ => (!snooze.is_asleep(clock.now_ms)).then_some(snooze.wake_at_ms),
        }
    }

    /// True when the row woke and the operator has not looked since.
    ///
    /// This is what makes [`Disposition::Woke`] clear itself: visiting the row
    /// retires the badge the same way it retires unread.
    pub fn has_unseen_wake(&self, clock: Clock) -> bool {
        let Some(woke_at_ms) = self.woke_at(clock) else {
            return false;
        };
        self.last_visited_ms
            .is_none_or(|visited_ms| visited_ms < woke_at_ms)
    }

    /// True when the operator is allowed to snooze this session.
    ///
    /// Refused only while the agent is blocked on them: hiding a pending
    /// approval defeats the request, and the row would raise its hand
    /// immediately anyway, so offering the action would be a lie. A running
    /// session IS snoozable; snooze affects visibility, never the agent.
    pub fn can_snooze(&self) -> bool {
        !self.blocks_on_operator()
    }

    /// True when the operator is allowed to settle this session.
    ///
    /// Refused while the agent is blocked on them, and refused mid-turn. T3
    /// Code refuses while the session is live at all; ours are persistent
    /// terminals rather than one run, so liveness alone cannot be the bar or
    /// nothing would ever be settleable. Mid-turn is the faithful analogue of
    /// their live-session block.
    pub fn can_settle(&self) -> bool {
        !self.blocks_on_operator() && self.status() != SidebarStatus::Working
    }

    /// Where this session sits on the operator's axis.
    ///
    /// Resolution order, and every step earns its place:
    ///
    /// 1. **Blocked on the operator wins over everything**, including an
    ///    explicit settle. A settled row that starts asking for approval must
    ///    come back; the operator settled a different situation.
    /// 2. **An effective snooze parks it.**
    /// 3. **A wake the operator has not seen shows the badge.**
    /// 4. **An explicit override rules, in both directions.**
    /// 5. **An unseen completion holds it in the inbox.** Finishing unseen is
    ///    the thing the inbox exists to surface.
    /// 6. **An acknowledged exit settles.** The process is gone and you looked.
    /// 7. **Inactivity past the window settles**, except mid-turn, where a
    ///    silent computing session must not be drained out from under a running
    ///    job.
    pub fn disposition(&self, clock: Clock, policy: DispositionPolicy) -> Disposition {
        if self.blocks_on_operator() {
            return Disposition::Active;
        }
        if self.effective_snoozed(clock) {
            return Disposition::Snoozed;
        }
        if self.has_unseen_wake(clock) {
            return Disposition::Woke;
        }
        match self.settle_override {
            Some(SettleOverride::Settled) => return Disposition::Settled,
            Some(SettleOverride::Active) => return Disposition::Active,
            None => {}
        }
        if self.has_unseen_completion() {
            return Disposition::Active;
        }
        if !self.info.status.is_live() {
            return Disposition::Settled;
        }
        if let Some(window_ms) = policy.auto_settle_after_ms
            && self.status() != SidebarStatus::Working
            && clock.now_ms.saturating_sub(self.info.last_activity_ms) >= window_ms
        {
            return Disposition::Settled;
        }
        Disposition::Active
    }

    /// The band this session renders in.
    pub fn section(&self, clock: Clock, policy: DispositionPolicy) -> Section {
        self.disposition(clock, policy).section()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::ViewBuilder;
    use vitrum_proto::HintState;

    const NOW: u64 = 1_772_580_600_000;
    const HOUR: u64 = 3_600_000;

    fn clock() -> Clock {
        Clock::utc(NOW)
    }

    fn policy() -> DispositionPolicy {
        DispositionPolicy::manual()
    }

    /// Semantic 3, and the one with a performance contract attached: a snooze
    /// expires because the predicate stops answering true, not because anything
    /// fired. Nothing in this crate schedules, so twenty snoozed sessions cost
    /// exactly zero while parked.
    #[test]
    fn a_timer_wake_is_derived_from_the_clock_with_no_event() {
        let row = ViewBuilder::new(1)
            .running()
            .waiting(Some(false))
            .last_activity_ms(NOW - HOUR)
            .snooze(NOW - HOUR, NOW + HOUR)
            .build();

        // Parked, and the row is untouched.
        assert!(row.effective_snoozed(clock()));
        assert_eq!(row.disposition(clock(), policy()), Disposition::Snoozed);
        assert_eq!(row.woke_at(clock()), None);

        // The very same row, one instant past its wake time, with no mutation
        // of any kind in between.
        let awake = Clock::utc(NOW + HOUR);
        assert!(!row.effective_snoozed(awake));
        assert_eq!(row.woke_at(awake), Some(NOW + HOUR));
        assert_eq!(row.disposition(awake, policy()), Disposition::Woke);
        assert_eq!(
            row.snooze.expect("snooze fields are never mutated").wake_at_ms,
            NOW + HOUR
        );
    }

    /// Semantic 1: a declared block outranks the snooze immediately, and the
    /// snooze fields survive so the row re-parks itself if the block clears.
    #[test]
    fn a_blocking_declaration_raises_the_hand_without_clearing_the_snooze() {
        for state in [HintState::Approval, HintState::Input] {
            let row = ViewBuilder::new(1)
                .running()
                .waiting(Some(true))
                .snooze(NOW - HOUR, NOW + HOUR)
                .hint(state, Some("may I?"), NOW - 60_000)
                .build();
            assert!(row.raised_hand(), "{state:?}");
            assert!(!row.effective_snoozed(clock()));
            assert_eq!(row.disposition(clock(), policy()), Disposition::Active);
            assert_eq!(
                row.snooze.expect("snooze retained").wake_at_ms,
                NOW + HOUR,
                "raising a hand must not mutate the snooze"
            );
        }
    }

    /// The hand goes back down. Because raising is computed and not stored, a
    /// session whose block clears while still inside its snooze window re-parks
    /// itself rather than staying permanently awake.
    #[test]
    fn the_hand_goes_back_down_when_the_trigger_clears() {
        let raised = ViewBuilder::new(1)
            .running()
            .waiting(Some(false))
            .last_activity_ms(NOW - HOUR)
            .snooze(NOW - HOUR, NOW + HOUR)
            .hint(HintState::Approval, None, NOW - 60_000)
            .build();
        assert!(raised.raised_hand());

        let mut cleared = raised.clone();
        cleared.info_mut().hint = None;
        assert!(!cleared.raised_hand());
        assert!(cleared.effective_snoozed(clock()));
        assert_eq!(cleared.disposition(clock(), policy()), Disposition::Snoozed);
    }

    /// Semantic 2, the one most easily got wrong: a session snoozed while
    /// ALREADY failed stays snoozed. That snooze was the operator saying "I saw
    /// it, not now", and un-parking it immediately would make snooze useless for
    /// exactly the rows people most want to park.
    #[test]
    fn a_stale_failure_does_not_raise_the_hand_but_a_fresh_one_does() {
        let stale = ViewBuilder::new(1)
            .exited(1)
            .last_activity_ms(NOW - 2 * HOUR)
            .unread(true)
            .snooze(NOW - HOUR, NOW + HOUR)
            .build();
        assert_eq!(stale.status(), SidebarStatus::Failed);
        assert!(!stale.raised_hand(), "a failure older than the snooze is not news");
        assert!(stale.effective_snoozed(clock()));
        assert_eq!(stale.disposition(clock(), policy()), Disposition::Snoozed);

        let fresh = ViewBuilder::new(1)
            .exited(1)
            .last_activity_ms(NOW - 60_000)
            .unread(true)
            .snooze(NOW - 2 * HOUR, NOW + HOUR)
            .build();
        assert!(fresh.raised_hand(), "a failure after the snooze is news");
        assert!(!fresh.effective_snoozed(clock()));
        assert_eq!(fresh.woke_at(clock()), Some(NOW - 60_000));
    }

    /// A failure exactly at the snooze instant is not fresh. The boundary has to
    /// be strict, or the act of snoozing a just-failed row races with itself.
    #[test]
    fn a_failure_at_the_snooze_instant_is_not_fresh() {
        let row = ViewBuilder::new(1)
            .exited(1)
            .last_activity_ms(NOW - HOUR)
            .unread(true)
            .snooze(NOW - HOUR, NOW + HOUR)
            .build();
        assert!(!row.raised_hand());
        assert!(row.effective_snoozed(clock()));
    }

    /// Work that finishes after the snooze raises the hand. This is the "you
    /// parked it, then it finished" case, and it is what makes snooze safe to
    /// use on a running session.
    #[test]
    fn work_completing_after_the_snooze_raises_the_hand() {
        let row = ViewBuilder::new(1)
            .running()
            .waiting(Some(false))
            .snooze(NOW - HOUR, NOW + HOUR)
            .hint(HintState::Ready, None, NOW - 60_000)
            .unread(true)
            .build();
        assert!(row.raised_hand());
        assert_eq!(row.woke_at(clock()), Some(NOW - 60_000));
        assert_eq!(row.disposition(clock(), policy()), Disposition::Woke);
    }

    /// Our free upgrade over T3 Code. An agent that has never emitted a hint,
    /// snoozed while working, goes back to its prompt: the kernel says it is
    /// blocked on the terminal and its output is newer than the snooze, so the
    /// hand goes up. T3 Code cannot do this without harness integration.
    #[test]
    fn an_unhinted_agent_returning_to_its_prompt_raises_its_hand() {
        let row = ViewBuilder::new(1)
            .running()
            .waiting(Some(true))
            .last_activity_ms(NOW - 60_000)
            .snooze(NOW - HOUR, NOW + HOUR)
            .unread(true)
            .build();
        assert_eq!(row.info.hint, None, "no harness integration involved");
        assert!(row.raised_hand());
        assert_eq!(row.woke_at(clock()), Some(NOW - 60_000));
        assert_eq!(row.disposition(clock(), policy()), Disposition::Woke);
    }

    /// The same signal must NOT raise a hand when nothing new happened: a
    /// session already sitting at its prompt when you parked it stays parked.
    /// Without the freshness comparison, snoozing an idle session would be a
    /// no-op, which is the most common thing an operator wants to do.
    #[test]
    fn a_session_already_at_its_prompt_when_snoozed_stays_parked() {
        let row = ViewBuilder::new(1)
            .running()
            .waiting(Some(true))
            .last_activity_ms(NOW - 2 * HOUR)
            .snooze(NOW - HOUR, NOW + HOUR)
            .build();
        assert!(!row.raised_hand());
        assert!(row.effective_snoozed(clock()));
        assert_eq!(row.disposition(clock(), policy()), Disposition::Snoozed);
    }

    /// A platform that cannot answer the probe must not raise hands it cannot
    /// justify. `None` is not `Some(true)`.
    #[test]
    fn an_unknown_probe_never_raises_a_hand_by_itself() {
        let row = ViewBuilder::new(1)
            .running()
            .waiting(None)
            .last_activity_ms(NOW - 60_000)
            .snooze(NOW - HOUR, NOW + HOUR)
            .build();
        assert!(!row.raised_hand());
        assert!(row.effective_snoozed(clock()));
    }

    /// Semantic 4: the guard. Offering snooze on a row that would raise its hand
    /// the same instant is a lie, and hiding a pending approval defeats the
    /// request.
    #[test]
    fn a_session_blocked_on_the_operator_cannot_be_snoozed() {
        for state in [HintState::Approval, HintState::Input] {
            let blocked = ViewBuilder::new(1)
                .running()
                .waiting(Some(true))
                .hint(state, None, NOW)
                .build();
            assert!(!blocked.can_snooze(), "{state:?}");
            assert!(!blocked.can_settle(), "{state:?}");
        }
    }

    /// Everything not blocked on the operator IS snoozable, including a running
    /// session. Snooze affects visibility, never the agent, so refusing it for
    /// live work would be pure paternalism.
    #[test]
    fn running_and_resting_sessions_are_both_snoozable() {
        let working = ViewBuilder::new(1).running().waiting(Some(false)).build();
        assert_eq!(working.status(), SidebarStatus::Working);
        assert!(working.can_snooze());

        let resting = ViewBuilder::new(2).running().waiting(Some(true)).build();
        assert_eq!(resting.status(), SidebarStatus::Ready);
        assert!(resting.can_snooze());

        let dead = ViewBuilder::new(3).exited(1).unread(true).build();
        assert!(dead.can_snooze());
    }

    /// Settle is refused mid-turn: draining a row whose agent is actively
    /// computing would hide work in flight. It is allowed once the agent rests,
    /// which is the whole point of an inbox.
    #[test]
    fn settle_is_refused_mid_turn_and_allowed_at_rest() {
        let working = ViewBuilder::new(1).running().waiting(Some(false)).build();
        assert!(!working.can_settle());

        let resting = ViewBuilder::new(2).running().waiting(Some(true)).build();
        assert!(resting.can_settle());

        let failed = ViewBuilder::new(3).exited(1).unread(true).build();
        assert!(failed.can_settle());
    }

    /// Semantic 6, first clause: an activity blocker beats an explicit settle.
    /// The operator settled a session that was finished; one that has started
    /// asking for approval is a different situation and must come back.
    #[test]
    fn a_blocking_declaration_overrides_an_explicit_settle() {
        let mut row = ViewBuilder::new(1)
            .running()
            .waiting(Some(true))
            .hint(HintState::Approval, None, NOW)
            .build();
        row.settle_override = Some(SettleOverride::Settled);
        assert_eq!(row.disposition(clock(), policy()), Disposition::Active);
    }

    /// Semantic 6, second clause: past the blockers an explicit override rules
    /// in both directions, including holding a row in the inbox that every
    /// automatic rule would drain.
    #[test]
    fn an_explicit_override_rules_in_both_directions() {
        let base = ViewBuilder::new(1)
            .exited(0)
            .last_activity_ms(NOW - 10)
            .last_visited_ms(Some(NOW))
            .build();
        assert_eq!(base.disposition(clock(), policy()), Disposition::Settled);

        let mut pinned = base.clone();
        pinned.settle_override = Some(SettleOverride::Active);
        assert_eq!(pinned.disposition(clock(), policy()), Disposition::Active);

        let mut drained = ViewBuilder::new(2).running().waiting(Some(true)).build();
        assert_eq!(drained.disposition(clock(), policy()), Disposition::Active);
        drained.settle_override = Some(SettleOverride::Settled);
        assert_eq!(drained.disposition(clock(), policy()), Disposition::Settled);
    }

    /// Semantic 6, third clause. Inactivity past the window drains a row, and
    /// the window is measured from last activity so a chatty session never
    /// drains under the operator.
    #[test]
    fn inactivity_past_the_window_settles_a_row() {
        let window = DispositionPolicy {
            auto_settle_after_ms: Some(24 * HOUR),
        };

        let recent = ViewBuilder::new(1)
            .running()
            .waiting(Some(true))
            .last_activity_ms(NOW - 23 * HOUR)
            .build();
        assert_eq!(recent.disposition(clock(), window), Disposition::Active);

        let stale = ViewBuilder::new(2)
            .running()
            .waiting(Some(true))
            .last_activity_ms(NOW - 24 * HOUR)
            .build();
        assert_eq!(stale.disposition(clock(), window), Disposition::Settled);

        // Disabled means disabled.
        assert_eq!(
            stale.disposition(clock(), DispositionPolicy::manual()),
            Disposition::Active
        );
    }

    /// Auto-settle must never drain a session that is mid-turn, even one that
    /// has been silently computing for longer than the window. Draining a
    /// running job out from under the operator is the worst possible outcome.
    #[test]
    fn auto_settle_never_drains_a_session_that_is_computing() {
        let window = DispositionPolicy {
            auto_settle_after_ms: Some(HOUR),
        };
        let row = ViewBuilder::new(1)
            .running()
            .waiting(Some(false))
            .last_activity_ms(NOW - 100 * HOUR)
            .build();
        assert_eq!(row.status(), SidebarStatus::Working);
        assert_eq!(row.disposition(clock(), window), Disposition::Active);
    }

    /// An unseen completion holds the row in the inbox even past the window.
    /// Draining something you have never looked at is the one thing an inbox
    /// must not do.
    #[test]
    fn an_unseen_completion_survives_the_auto_settle_window() {
        let window = DispositionPolicy {
            auto_settle_after_ms: Some(HOUR),
        };
        let unseen = ViewBuilder::new(1)
            .exited(1)
            .last_activity_ms(NOW - 100 * HOUR)
            .unread(true)
            .build();
        assert!(unseen.has_unseen_completion());
        assert_eq!(unseen.disposition(clock(), window), Disposition::Active);

        let seen = ViewBuilder::new(2)
            .exited(1)
            .last_activity_ms(NOW - 100 * HOUR)
            .last_visited_ms(Some(NOW))
            .build();
        assert_eq!(seen.disposition(clock(), window), Disposition::Settled);
    }

    /// The Woke badge clears on a visit, and only on a visit made after the
    /// wake. This is what stops the badge from being permanent.
    #[test]
    fn the_woke_badge_clears_only_on_a_visit_after_the_wake() {
        let build = |visited: Option<u64>| {
            ViewBuilder::new(1)
                .running()
                .waiting(Some(false))
                .last_activity_ms(NOW - 5 * HOUR)
                .snooze(NOW - 5 * HOUR, NOW - HOUR)
                .last_visited_ms(visited)
                .build()
        };

        let never = build(None);
        assert_eq!(never.woke_at(clock()), Some(NOW - HOUR));
        assert_eq!(never.disposition(clock(), policy()), Disposition::Woke);

        let before = build(Some(NOW - 2 * HOUR));
        assert_eq!(before.disposition(clock(), policy()), Disposition::Woke);

        let after = build(Some(NOW - HOUR + 1));
        assert!(!after.has_unseen_wake(clock()));
        assert_eq!(after.disposition(clock(), policy()), Disposition::Active);
    }

    /// A raised-hand wake reports the TRIGGERING instant, not the scheduled wake
    /// time. Reporting the scheduled time would let a visit made before the
    /// trigger suppress a badge for something that had not happened yet.
    #[test]
    fn an_early_wake_reports_the_trigger_not_the_scheduled_time() {
        let row = ViewBuilder::new(1)
            .exited(1)
            .last_activity_ms(NOW - HOUR)
            .unread(true)
            .snooze(NOW - 5 * HOUR, NOW + 10 * HOUR)
            .last_visited_ms(Some(NOW - 2 * HOUR))
            .build();
        assert_eq!(
            row.woke_at(clock()),
            Some(NOW - HOUR),
            "the failure instant, not the scheduled wake"
        );
        assert!(
            row.has_unseen_wake(clock()),
            "a visit before the trigger must not suppress the badge"
        );
        assert_eq!(row.disposition(clock(), policy()), Disposition::Woke);
    }

    /// An early wake stays authoritative after the scheduled time passes too,
    /// or a badge the operator already cleared would resurface the moment the
    /// original timer elapsed.
    #[test]
    fn an_early_wake_stays_authoritative_past_the_scheduled_time() {
        let row = ViewBuilder::new(1)
            .exited(1)
            .last_activity_ms(NOW - 5 * HOUR)
            .unread(true)
            .snooze(NOW - 6 * HOUR, NOW - HOUR)
            .last_visited_ms(Some(NOW - 4 * HOUR))
            .build();
        assert_eq!(row.woke_at(clock()), Some(NOW - 5 * HOUR));
        assert!(!row.has_unseen_wake(clock()));
        assert_eq!(row.disposition(clock(), policy()), Disposition::Settled);
    }

    /// A row that never snoozed never wakes. Without this the Woke badge would
    /// appear on rows that were never parked.
    #[test]
    fn a_row_that_never_snoozed_has_no_wake() {
        let row = ViewBuilder::new(1).running().waiting(Some(true)).build();
        assert_eq!(row.woke_at(clock()), None);
        assert!(!row.has_unseen_wake(clock()));
        assert!(!row.raised_hand());
        assert_eq!(row.disposition(clock(), policy()), Disposition::Active);
    }

    /// Woke belongs to the Active band. The badge exists precisely because the
    /// row comes back where it was, so putting it in its own section would
    /// defeat the design.
    #[test]
    fn woke_renders_in_the_active_band() {
        assert_eq!(Disposition::Active.section(), Section::Active);
        assert_eq!(Disposition::Woke.section(), Section::Active);
        assert_eq!(Disposition::Snoozed.section(), Section::Snoozed);
        assert_eq!(Disposition::Settled.section(), Section::Settled);
        assert!(Section::Active < Section::Snoozed);
        assert!(Section::Snoozed < Section::Settled);
    }

    /// Labels are user-visible and Active deliberately has none: a badge on
    /// every row is a badge on no row.
    #[test]
    fn only_the_non_resting_dispositions_carry_a_label() {
        assert_eq!(Disposition::Active.label(), None);
        assert_eq!(Disposition::Woke.label(), Some("Woke"));
        assert_eq!(Disposition::Snoozed.label(), Some("Snoozed"));
        assert_eq!(Disposition::Settled.label(), Some("Settled"));
    }

    /// The full lifecycle in one pass, driven only by the clock and by visits.
    /// This is the drain loop the sidebar is built around.
    #[test]
    fn a_session_walks_the_whole_disposition_cycle() {
        let mut row = ViewBuilder::new(1)
            .running()
            .waiting(Some(false))
            .created_at_ms(NOW - 10 * HOUR)
            .last_activity_ms(NOW)
            .build();
        assert_eq!(row.disposition(clock(), policy()), Disposition::Active);

        // Park it.
        assert!(row.can_snooze());
        row.snooze = Some(crate::snooze::Snooze {
            snoozed_at_ms: NOW,
            wake_at_ms: NOW + 8 * HOUR,
        });
        assert_eq!(row.disposition(clock(), policy()), Disposition::Snoozed);

        // The timer elapses. Nothing fired.
        let woken = Clock::utc(NOW + 8 * HOUR);
        assert_eq!(row.disposition(woken, policy()), Disposition::Woke);

        // The operator looks.
        row.last_visited_ms = Some(NOW + 8 * HOUR + 1);
        assert_eq!(row.disposition(woken, policy()), Disposition::Active);

        // The agent finishes. The spent snooze must NOT mint a second Woke
        // badge for this: the wake already happened and was seen, so a later
        // completion is ordinary unseen work.
        let info = row.info_mut();
        info.status = vitrum_proto::SessionStatus::Exited { code: Some(0) };
        info.last_activity_ms = NOW + 9 * HOUR;
        info.unread = true;
        let done = Clock::utc(NOW + 10 * HOUR);
        assert_eq!(row.woke_at(done), Some(NOW + 8 * HOUR), "still the one wake");
        assert!(row.has_unseen_completion());
        assert_eq!(row.disposition(done, policy()), Disposition::Active);

        // The operator drains it.
        row.info_mut().unread = false;
        row.last_visited_ms = Some(NOW + 9 * HOUR + 1);
        assert_eq!(row.disposition(done, policy()), Disposition::Settled);
    }

    /// The policy is persisted with the operator's settings, and the override is
    /// persisted per session, so both must survive JSON.
    #[test]
    fn policy_and_override_round_trip_through_json() {
        let policy = DispositionPolicy::default();
        assert_eq!(
            policy.auto_settle_after_ms,
            Some(DispositionPolicy::DEFAULT_AUTO_SETTLE_MS)
        );
        let json = serde_json::to_string(&policy).expect("policy serialises");
        assert_eq!(json, r#"{"autoSettleAfterMs":604800000}"#);
        assert_eq!(
            serde_json::from_str::<DispositionPolicy>(&json).expect("policy round-trips"),
            policy
        );

        assert_eq!(
            serde_json::to_string(&SettleOverride::Settled).expect("override serialises"),
            "\"settled\""
        );
        assert_eq!(
            serde_json::to_string(&Disposition::Woke).expect("disposition serialises"),
            "\"woke\""
        );
    }
}
