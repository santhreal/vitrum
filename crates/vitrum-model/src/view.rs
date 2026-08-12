//! One sidebar row: the server's session projection plus the client-local state
//! the server has no business knowing.
//!
//! [`SessionInfo`] is authoritative for everything the daemon owns: lifecycle,
//! activity, unread, [`Attention`](vitrum_proto::Attention), and the agent's
//! last declared hint. What the daemon does not own is per-operator: whether
//! *you* have this row snoozed, and when *you* last looked at it. A second
//! window on the same daemon can legitimately disagree about both.
//!
//! Every derived answer here is a pure function of that pair plus a [`Clock`],
//! so the whole sidebar is reproducible from a snapshot and testable without a
//! daemon.

use vitrum_proto::{ProjectId, SessionId, SessionInfo};
use serde::{Deserialize, Serialize};

use crate::agent::AgentKind;
use crate::disposition::SettleOverride;
use crate::snooze::Snooze;
use crate::status::{
    DECLARATION_DWELL_MS, HeldClaim, SidebarStatus, StatusResolution, resolve_status,
};

/// The current instant and the operator's UTC offset.
///
/// Passed explicitly rather than read from the system so every derivation is a
/// pure function. That is what makes calendar-boundary behaviour testable
/// instead of something you can only observe by waiting until midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clock {
    /// Milliseconds since the Unix epoch, UTC.
    pub now_ms: u64,
    /// Seconds to add to UTC to reach the operator's wall clock. See
    /// [`crate::civil`] for what this can and cannot express across a
    /// daylight-saving transition.
    pub utc_offset_seconds: i32,
}

impl Clock {
    /// A clock at `now_ms` in UTC.
    pub fn utc(now_ms: u64) -> Self {
        Clock {
            now_ms,
            utc_offset_seconds: 0,
        }
    }
}

/// A session as the sidebar sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    /// The daemon's projection. Replaced wholesale on every `SessionUpdated`.
    pub info: SessionInfo,
    /// Set while this operator has the row parked.
    pub snooze: Option<Snooze>,
    /// The operator's explicit ruling on whether they are done with this row.
    /// `None` leaves it to the automatic rules in [`crate::disposition`].
    pub settle_override: Option<SettleOverride>,
    /// When this operator last had the row focused. `None` means never, which
    /// is different from "long ago".
    pub last_visited_ms: Option<u64>,
    /// A declared block this window is still showing after the declaration
    /// stopped arriving. See [`crate::status::DECLARATION_DWELL_MS`].
    ///
    /// `#[serde(default)]` because a snapshot written before the field existed
    /// must still load, and because a hold is worthless across a restart: the
    /// first push after one re-derives it.
    #[serde(default)]
    pub held_claim: Option<HeldClaim>,
}

impl SessionView {
    /// A view over a freshly received session, never visited and not snoozed.
    pub fn new(info: SessionInfo) -> Self {
        SessionView {
            info,
            snooze: None,
            settle_override: None,
            last_visited_ms: None,
            held_claim: None,
        }
    }

    pub fn id(&self) -> SessionId {
        self.info.id
    }

    pub fn project_id(&self) -> ProjectId {
        self.info.project_id
    }

    /// Status and the signal that produced it. See [`resolve_status`] for the
    /// precedence rules, and [`Self::settle_declaration`] for the one thing
    /// this adds to them.
    ///
    /// The title claim is resolved here rather than by the caller so that every
    /// consumer of a row — the pill, the sort, the project rollup — sees the
    /// same answer. A Codex session blocked on an approval gate that only the
    /// sidebar pill knew about would sort as if nothing were wanted.
    ///
    /// It reads `term_title`, what the program announced, and never `title`,
    /// the session's name. For an agent TUI those are deliberately different
    /// strings: the name is stable and may be the operator's, while the title
    /// bar is rewritten every turn. Reading the name here would let a session
    /// renamed `[ ! ] Action Required` claim to be blocked forever.
    ///
    /// A live hold wins over the fresh resolution, and is the only thing that
    /// does. It cannot invent a state: a hold is only ever set from a
    /// resolution that already said `Approval` or `Input`, so this reports
    /// something the agent declared, a moment later than it declared it.
    pub fn resolve_status(&self) -> StatusResolution {
        let fresh = self.raw_status();
        match self.held_claim {
            // A dead child is never held. Precedence rule 1 in
            // [`resolve_status`] says the exit wins over everything, and a
            // stale "act now" on a session that has already gone is the exact
            // thing that rule exists to prevent.
            Some(held) if self.info.status.is_live() && !fresh.status.requires_declaration() => {
                held.resolution
            }
            _ => fresh,
        }
    }

    /// The resolution from the daemon's projection alone, ignoring any hold.
    ///
    /// The input to [`Self::settle_declaration`], and what a caller wants when
    /// it is asking what the agent is saying right now rather than what the
    /// operator is being shown.
    pub fn raw_status(&self) -> StatusResolution {
        resolve_status(
            &self.info.status,
            &self.info.attention,
            self.info.hint.as_ref().map(|hint| hint.state),
            self.info
                .term_title
                .as_deref()
                .and_then(|title| AgentKind::of(&self.info.command).title_claim(title)),
        )
    }

    /// Take the hold forward one ingest.
    ///
    /// Called once per message that changes a row's daemon projection, and
    /// nowhere else: the hold is advanced by new information, never by the
    /// passage of time alone. A session the daemon has stopped talking about
    /// keeps whatever it was last showing, which is the last thing anybody
    /// knew about it.
    ///
    /// Three cases, and the asymmetry between the first two is the point:
    ///
    /// - The agent is declaring a block. Adopt it and renew the hold. This
    ///   arm runs on every push while the gate is up, so the hold expires
    ///   [`DECLARATION_DWELL_MS`] after the LAST declaration, not after the
    ///   first.
    /// - It is not, and a hold is still live. Leave it. This is the arm that
    ///   swallows the dropped frame.
    /// - It is not, and the hold has lapsed. Drop it and let the fresh
    ///   resolution through.
    pub fn settle_declaration(&mut self, now_ms: u64) {
        let fresh = self.raw_status();
        if fresh.status.requires_declaration() {
            self.held_claim = Some(HeldClaim {
                resolution: fresh,
                until_ms: now_ms.saturating_add(DECLARATION_DWELL_MS),
            });
        } else if self.held_claim.is_some_and(|held| now_ms >= held.until_ms) {
            self.held_claim = None;
        }
    }

    /// Status alone, for callers that do not care where it came from.
    pub fn status(&self) -> SidebarStatus {
        self.resolve_status().status
    }

    /// The short label the agent attached to its declaration, if it sent one.
    ///
    /// Returns `None` for a hint on a session that has since exited, matching
    /// [`resolve_status`]: if the state is not shown, its label must not be
    /// either, or the row reads "Ready" beside "Approve this write?".
    pub fn hint_label(&self) -> Option<&str> {
        if !self.info.status.is_live() {
            return None;
        }
        self.info.hint.as_ref()?.label.as_deref()
    }

    /// When the session's most recent unit of work finished, if it has.
    ///
    /// Two ways to finish, in precedence order:
    ///
    /// 1. The child exited. `last_activity_ms` is the exit instant.
    /// 2. The agent declared `ready`. Its declaration instant is the finish.
    ///
    /// A live, unhinted session that has merely gone quiet is deliberately not
    /// a completion. Silence is evidence the turn ended, but not evidence of
    /// *when*, and an unseen-completion badge that cannot name its own instant
    /// would light up and never clear.
    pub fn completion_at_ms(&self) -> Option<u64> {
        if !self.info.status.is_live() {
            return Some(self.info.last_activity_ms);
        }
        self.info
            .hint
            .as_ref()
            .filter(|hint| hint.state == vitrum_proto::HintState::Ready)
            .map(|hint| hint.received_at_ms)
    }

    /// True when a session finished while the operator was not looking.
    ///
    /// This is T3 Code's `hasUnseenCompletion` and it is deliberately distinct
    /// from unread output: a working session produces unread output constantly
    /// and wants nothing, while a session that *finished* unseen is the thing
    /// you came to the sidebar to find.
    ///
    /// Two sources of "were you looking", and which applies is not a fallback
    /// but a statement about what the client knows:
    ///
    /// - A client that tracks focus supplies `last_visited_ms`, and the answer
    ///   is whether the completion happened after that visit.
    /// - A client that does not defers to the daemon's `unread`, which the
    ///   server maintains from the same focus notifications.
    ///
    /// This diverges from T3 Code, which answers `false` when it has no
    /// `lastVisitedAt`. A session that finished and has never been opened is
    /// the clearest possible unseen completion, so a missing visit stamp reads
    /// as "not seen", not as "seen".
    pub fn has_unseen_completion(&self) -> bool {
        let Some(completed_at_ms) = self.completion_at_ms() else {
            return false;
        };
        match self.last_visited_ms {
            Some(visited_ms) => completed_at_ms > visited_ms,
            None => self.info.unread,
        }
    }

    /// True while the snooze window is still open, ignoring the raised-hand
    /// rule.
    ///
    /// This is the raw wall-clock question. Whether the row is ACTUALLY parked
    /// is [`SessionView::effective_snoozed`], which also asks whether the
    /// session has raised its hand.
    pub fn is_asleep(&self, clock: Clock) -> bool {
        self.snooze
            .is_some_and(|snooze| snooze.is_asleep(clock.now_ms))
    }

    /// The instant a settled row sorts and labels by.
    ///
    /// An explicit settle wins: snoozing stamps `snoozed_at_ms`, and mirroring
    /// T3 Code's `resolveSettledTimestamp` that stamp takes precedence over
    /// inferred activity. Otherwise the row sorts by when its work ended, with
    /// creation as the final net for a session that has produced nothing.
    pub fn settled_at_ms(&self) -> u64 {
        if let Some(snooze) = self.snooze {
            return snooze.snoozed_at_ms;
        }
        self.info.last_activity_ms.max(self.info.created_at_ms)
    }

    /// When the current working stretch began, when there is a defensible
    /// anchor for it.
    ///
    /// Only a declared `working` hint provides one. A PTY cannot tell you when
    /// the current turn started: output is continuous and turn boundaries are a
    /// harness concept. T3 Code reads the turn's `startedAt` out of its event
    /// stream, which is exactly the coupling we are not taking on.
    ///
    /// So unhinted sessions return `None` and the UI shows no elapsed timer,
    /// rather than showing session age dressed up as turn duration. Use
    /// [`SessionView::age_ms`] if session age is what you actually want.
    pub fn working_since_ms(&self) -> Option<u64> {
        if self.status() != SidebarStatus::Working {
            return None;
        }
        self.info
            .hint
            .as_ref()
            .filter(|hint| hint.state == vitrum_proto::HintState::Working)
            .map(|hint| hint.received_at_ms)
    }

    /// Elapsed milliseconds since the current working stretch began.
    pub fn working_elapsed_ms(&self, clock: Clock) -> Option<u64> {
        self.working_since_ms()
            .map(|since| clock.now_ms.saturating_sub(since))
    }

    /// Milliseconds since the session was created.
    pub fn age_ms(&self, clock: Clock) -> u64 {
        clock.now_ms.saturating_sub(self.info.created_at_ms)
    }
}

/// Coarse elapsed label: `"9s"`, `"5m"`, `"2h 07m"`.
///
/// Ported from T3 Code's `formatWorkingDurationLabel`, with one change: the
/// minute part of the hour form is zero-padded. Their `2h 7m` and `2h 17m` have
/// different widths, and a label that changes width every ten minutes makes the
/// row it sits in twitch.
///
/// Truncating rather than rounding is deliberate: a timer that reads `1m`
/// before a full minute has passed looks broken.
pub fn format_duration_label(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disposition::{Disposition, DispositionPolicy};
    use crate::status::StatusSource;
    use crate::testkit::{ViewBuilder, view};
    use vitrum_proto::{HintState, IDLE_ATTENTION_MS};

    const NOW: u64 = 1_772_580_600_000;

    fn clock() -> Clock {
        Clock::utc(NOW)
    }

    /// A live session with fresh output has not completed, so the completion
    /// badge must stay dark no matter how much unread output it has produced.
    /// Conflating unread with completion is the exact bug this model exists to
    /// avoid: twenty working agents would all claim to be finished.
    #[test]
    fn unread_output_from_a_live_session_is_not_a_completion() {
        let row = ViewBuilder::new(1).running().unread(true).build();
        assert_eq!(row.completion_at_ms(), None);
        assert!(!row.has_unseen_completion());
    }

    /// A quiet live session still has no completion instant: we know it stopped
    /// talking, not that it finished. Manufacturing an instant here would make
    /// the badge permanent.
    #[test]
    fn silence_alone_is_not_a_completion() {
        let row = ViewBuilder::new(1)
            .running()
            .idle_ms(IDLE_ATTENTION_MS * 10)
            .unread(true)
            .build();
        assert_eq!(row.status(), SidebarStatus::Ready);
        assert_eq!(row.completion_at_ms(), None);
        assert!(!row.has_unseen_completion());
    }

    /// An exit is a completion stamped at the exit instant, and it counts as
    /// unseen while the daemon still reports unread.
    #[test]
    fn an_exit_completes_at_its_own_instant() {
        let row = ViewBuilder::new(1)
            .exited(0)
            .last_activity_ms(NOW - 5_000)
            .unread(true)
            .build();
        assert_eq!(row.completion_at_ms(), Some(NOW - 5_000));
        assert!(row.has_unseen_completion());
    }

    /// A declared `ready` completes a live session, which is how an opted-in
    /// harness gets an end-of-turn badge without exiting. The instant is the
    /// declaration's, not now, so the badge survives later refreshes.
    #[test]
    fn a_ready_hint_completes_a_live_session_at_its_declaration_instant() {
        let row = ViewBuilder::new(1)
            .running()
            .hint(HintState::Ready, None, NOW - 2_000)
            .unread(true)
            .build();
        assert_eq!(row.completion_at_ms(), Some(NOW - 2_000));
        assert!(row.has_unseen_completion());
    }

    /// A `working` declaration is not a completion. Only `ready` is, or the
    /// badge would fire on every turn start.
    #[test]
    fn a_working_hint_is_not_a_completion() {
        let row = ViewBuilder::new(1)
            .running()
            .hint(HintState::Working, None, NOW - 2_000)
            .unread(true)
            .build();
        assert_eq!(row.completion_at_ms(), None);
        assert!(!row.has_unseen_completion());
    }

    /// A local visit stamp overrides the daemon's unread flag in both
    /// directions. A second window that has looked at the row must clear its
    /// own badge without clearing the first window's.
    #[test]
    fn a_local_visit_stamp_decides_over_the_daemon_unread_flag() {
        let completed_at = NOW - 10_000;

        let visited_after = ViewBuilder::new(1)
            .exited(0)
            .last_activity_ms(completed_at)
            .unread(true)
            .last_visited_ms(Some(completed_at + 1))
            .build();
        assert!(!visited_after.has_unseen_completion());

        let visited_before = ViewBuilder::new(1)
            .exited(0)
            .last_activity_ms(completed_at)
            .unread(false)
            .last_visited_ms(Some(completed_at - 1))
            .build();
        assert!(visited_before.has_unseen_completion());

        let visited_exactly = ViewBuilder::new(1)
            .exited(0)
            .last_activity_ms(completed_at)
            .unread(true)
            .last_visited_ms(Some(completed_at))
            .build();
        assert!(
            !visited_exactly.has_unseen_completion(),
            "a visit at the completion instant counts as having seen it"
        );
    }

    /// Never visited plus finished is the clearest unseen completion there is.
    /// T3 Code answers false here; we answer true, and the daemon's unread flag
    /// is what settles it when the client tracks no visits.
    #[test]
    fn a_never_visited_finished_session_defers_to_the_daemon_unread_flag() {
        let seen = ViewBuilder::new(1).exited(0).unread(false).build();
        assert_eq!(seen.last_visited_ms, None);
        assert!(!seen.has_unseen_completion());

        let unseen = ViewBuilder::new(1).exited(0).unread(true).build();
        assert!(unseen.has_unseen_completion());
    }

    /// A signalled child reports no exit code at all, and must still read as a
    /// completed failure that the inbox surfaces until it is looked at.
    /// Treating a kill as "no information" would hide every OOM-killed agent.
    #[test]
    fn a_signalled_child_is_an_unseen_failed_completion() {
        let killed = ViewBuilder::new(1)
            .signalled()
            .last_activity_ms(NOW - 500)
            .unread(true)
            .build();
        assert_eq!(killed.info.status, vitrum_proto::SessionStatus::Exited { code: None });
        assert_eq!(killed.status(), SidebarStatus::Failed);
        assert_eq!(killed.completion_at_ms(), Some(NOW - 500));
        assert!(killed.has_unseen_completion());
        assert_eq!(
            killed.disposition(clock(), DispositionPolicy::manual()),
            Disposition::Active
        );

        let seen = ViewBuilder::new(1)
            .signalled()
            .last_activity_ms(NOW - 500)
            .last_visited_ms(Some(NOW))
            .build();
        assert_eq!(
            seen.disposition(clock(), DispositionPolicy::manual()),
            Disposition::Settled
        );
    }

    /// A live session is never drained by the disposition rules however quiet
    /// it is, because it can still speak.
    #[test]
    fn a_live_session_is_never_settled() {
        let row = ViewBuilder::new(1)
            .running()
            .idle_ms(IDLE_ATTENTION_MS * 100)
            .build();
        assert_eq!(
            row.disposition(clock(), DispositionPolicy::manual()),
            Disposition::Active
        );
    }

    /// A failure you have not looked at stays above the fold. Sinking it on
    /// exit would bury exactly the row that needs you, which is the failure
    /// mode of a plain "exited means history" rule.
    #[test]
    fn an_unseen_failure_stays_active_until_it_is_opened() {
        let policy = DispositionPolicy::manual();
        let unseen = ViewBuilder::new(1).exited(1).unread(true).build();
        assert_eq!(unseen.status(), SidebarStatus::Failed);
        assert_eq!(unseen.disposition(clock(), policy), Disposition::Active);

        let opened = ViewBuilder::new(1)
            .exited(1)
            .last_activity_ms(NOW - 1_000)
            .last_visited_ms(Some(NOW))
            .build();
        assert_eq!(opened.status(), SidebarStatus::Failed);
        assert_eq!(opened.disposition(clock(), policy), Disposition::Settled);
    }

    /// Snooze parks a row regardless of what it is doing, and stops parking it
    /// the instant it wakes, without ever changing what the agent is doing.
    #[test]
    fn snooze_parks_until_the_wake_instant_and_not_past_it() {
        let policy = DispositionPolicy::manual();
        let mut row = ViewBuilder::new(1).running().waiting(Some(false)).build();
        row.snooze = Some(Snooze {
            snoozed_at_ms: NOW - 1_000,
            wake_at_ms: NOW + 1_000,
        });
        assert!(row.is_asleep(clock()));
        assert_eq!(row.disposition(clock(), policy), Disposition::Snoozed);

        let awake = Clock::utc(NOW + 1_000);
        assert!(!row.is_asleep(awake));
        assert_eq!(row.disposition(awake, policy), Disposition::Woke);
        assert_eq!(
            row.status(),
            SidebarStatus::Working,
            "snooze must not change what the agent is doing"
        );
    }

    /// The settled sort key: an explicit snooze stamp beats inferred activity,
    /// so a just-snoozed old session sorts to the top of the settled pile
    /// rather than to the bottom.
    #[test]
    fn an_explicit_snooze_stamp_outranks_inferred_activity_for_the_settled_key() {
        let mut old = ViewBuilder::new(1)
            .exited(0)
            .created_at_ms(1_000)
            .last_activity_ms(2_000)
            .build();
        assert_eq!(old.settled_at_ms(), 2_000);

        old.snooze = Some(Snooze {
            snoozed_at_ms: 9_000,
            wake_at_ms: 99_000,
        });
        assert_eq!(old.settled_at_ms(), 9_000);
    }

    /// A session that never produced output has `last_activity_ms` at or below
    /// creation; the key must not collapse to zero and shove it to the bottom.
    #[test]
    fn the_settled_key_falls_back_to_creation_for_a_silent_session() {
        let row = ViewBuilder::new(1)
            .exited(0)
            .created_at_ms(5_000)
            .last_activity_ms(0)
            .build();
        assert_eq!(row.settled_at_ms(), 5_000);
    }

    /// A turn anchor exists only when the harness declared one. Inventing one
    /// from session creation would label a six-hour-old session as having
    /// worked for six hours on its current turn.
    #[test]
    fn only_a_declared_working_hint_yields_a_turn_anchor() {
        let unhinted = ViewBuilder::new(1).running().created_at_ms(NOW - 60_000).build();
        assert_eq!(unhinted.status(), SidebarStatus::Working);
        assert_eq!(unhinted.working_since_ms(), None);
        assert_eq!(unhinted.working_elapsed_ms(clock()), None);
        assert_eq!(unhinted.age_ms(clock()), 60_000);

        let hinted = ViewBuilder::new(1)
            .running()
            .created_at_ms(NOW - 60_000)
            .hint(HintState::Working, None, NOW - 9_000)
            .build();
        assert_eq!(hinted.working_since_ms(), Some(NOW - 9_000));
        assert_eq!(hinted.working_elapsed_ms(clock()), Some(9_000));
    }

    /// A stale `working` hint that has been retired by silence must stop
    /// reporting a turn anchor too, or the row shows a timer counting up for a
    /// turn that ended.
    #[test]
    fn a_retired_working_hint_reports_no_turn_anchor() {
        let row = ViewBuilder::new(1)
            .running()
            .idle_ms(IDLE_ATTENTION_MS)
            .hint(HintState::Working, None, NOW - 60_000)
            .build();
        assert_eq!(row.resolve_status().source, StatusSource::Idle);
        assert_eq!(row.working_since_ms(), None);
    }

    /// A hint label belongs to a live declaration. Showing "Approve this write?"
    /// next to a Ready badge on a dead session is a lie about what is happening.
    #[test]
    fn a_hint_label_is_dropped_once_the_session_exits() {
        let live = ViewBuilder::new(1)
            .running()
            .hint(HintState::Approval, Some("write src/main.rs"), NOW)
            .build();
        assert_eq!(live.hint_label(), Some("write src/main.rs"));
        assert_eq!(live.status(), SidebarStatus::Approval);

        let dead = ViewBuilder::new(1)
            .exited(0)
            .hint(HintState::Approval, Some("write src/main.rs"), NOW)
            .build();
        assert_eq!(dead.hint_label(), None);
        assert_eq!(dead.status(), SidebarStatus::Ready);
    }

    /// Duration labels are user-visible and sit in a fixed-width slot. The
    /// zero-padded minute in the hour form is what stops the row twitching as
    /// the timer crosses ten minutes.
    #[test]
    fn duration_labels_render_at_every_unit_boundary() {
        assert_eq!(format_duration_label(0), "0s");
        assert_eq!(format_duration_label(999), "0s");
        assert_eq!(format_duration_label(1_000), "1s");
        assert_eq!(format_duration_label(59_999), "59s");
        assert_eq!(format_duration_label(60_000), "1m");
        assert_eq!(format_duration_label(3_599_999), "59m");
        assert_eq!(format_duration_label(3_600_000), "1h 00m");
        assert_eq!(format_duration_label(3_600_000 + 7 * 60_000), "1h 07m");
        assert_eq!(format_duration_label(3_600_000 + 17 * 60_000), "1h 17m");
        assert_eq!(format_duration_label(26 * 3_600_000), "26h 00m");
    }

    /// A clock that has gone backwards (an NTP correction between two refreshes)
    /// must not underflow the unsigned elapsed maths and print a 584-million-year
    /// duration.
    #[test]
    fn elapsed_maths_saturates_when_the_clock_goes_backwards() {
        let row = ViewBuilder::new(1)
            .running()
            .created_at_ms(NOW + 5_000)
            .hint(HintState::Working, None, NOW + 5_000)
            .build();
        assert_eq!(row.age_ms(clock()), 0);
        assert_eq!(row.working_elapsed_ms(clock()), Some(0));
    }

    /// The whole row model is persisted and restored, so it must survive JSON.
    /// A field rename here silently discards every snooze and visit stamp on
    /// upgrade.
    #[test]
    fn a_session_view_round_trips_through_json() {
        let mut row = view(7);
        row.snooze = Some(Snooze {
            snoozed_at_ms: 1,
            wake_at_ms: 2,
        });
        row.last_visited_ms = Some(3);
        let json = serde_json::to_string(&row).expect("view serialises");
        let back: SessionView = serde_json::from_str(&json).expect("view round-trips");
        assert_eq!(back, row);
        assert!(json.contains("\"lastVisitedMs\":3"));
        assert!(json.contains("\"snooze\":{\"snoozedAtMs\":1,\"wakeAtMs\":2}"));
    }

    /// THE HEADLINE CASE, at the layer every consumer actually calls. A Codex
    /// session parked on "Would you like to run the following command?" sets
    /// the title `[ ! ] Action Required`, and the operating system sees only a
    /// process blocked on a read — which is `Ready`, which is what the sidebar
    /// showed while the operator was being waited on.
    #[test]
    fn a_codex_session_titled_action_required_needs_approval() {
        let row = ViewBuilder::new(1)
            .command("codex")
            .term_title("[ ! ] Action Required - codex")
            .waiting(Some(true))
            .build();
        assert_eq!(
            row.resolve_status(),
            StatusResolution::new(SidebarStatus::Approval, StatusSource::Title)
        );
        assert!(
            row.resolve_status().source.is_inferred(),
            "a title-derived state must hedge; it is a reading, not a protocol"
        );
        assert!(row.status().wants_operator());
    }

    /// The rule belongs to the agent that produces the banner. The same title
    /// on an agent with no rule, on a shell, and on a command this build cannot
    /// name must change nothing: a global string match would put "Needs
    /// approval" on any session that happened to title itself that way.
    #[test]
    fn the_same_title_on_another_agent_is_not_a_declaration() {
        for command in ["claude", "gemini", "opencode", "veyyon", "bash", "make", ""] {
            let row = ViewBuilder::new(1)
                .command(command)
                .term_title("[ ! ] Action Required")
                .waiting(Some(true))
                .build();
            assert_eq!(
                row.resolve_status(),
                StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting),
                "{command:?} read another agent's banner as a declaration"
            );
        }
    }

    /// BOTH DIRECTIONS. The claim is a function of the current title and
    /// nothing else, so it appears when Codex raises the banner and is gone the
    /// instant Codex clears it. A row that stuck on "Needs approval" after the
    /// agent moved on would be worse than never showing the state: it trains
    /// the operator to ignore the pill.
    #[test]
    fn a_title_claim_appears_and_clears_with_the_title() {
        let quiet = ViewBuilder::new(1)
            .command("codex")
            .term_title("codex")
            .waiting(Some(true))
            .build();
        assert_eq!(quiet.status(), SidebarStatus::Ready);

        let mut row = quiet.clone();
        row.info.term_title = Some("[ ! ] Action Required - codex".to_string());
        assert_eq!(
            row.resolve_status(),
            StatusResolution::new(SidebarStatus::Approval, StatusSource::Title)
        );

        // ... and back. The agent answered the gate and retitled.
        row.info.term_title = Some("codex".to_string());
        assert_eq!(row.resolve_status(), quiet.resolve_status());
        assert_eq!(row.status(), SidebarStatus::Ready);

        // Once more, so the transition is proven repeatable rather than a
        // one-shot that happens to survive the first flip.
        row.info.term_title = Some("[ ! ] Action Required".to_string());
        assert_eq!(row.status(), SidebarStatus::Approval);
    }

    /// THE FLAP. Codex ANIMATES the marker in its approval banner, and the
    /// sidebar used to animate with it.
    ///
    /// Recorded from a live daemon holding a Codex session parked on an
    /// approval gate: the session wrote 222 title changes while that one gate
    /// was up, alternating `[ ! ] Action Required` and `[ . ] Action Required`
    /// in equal numbers, some pairs under half a second apart. The probe
    /// answered `waiting: Some(true)` throughout and no hint was ever sent, so
    /// with a rule that matched only the `!` phase every second title resolved
    /// to no claim, fell through to the observed signals and read `Ready`.
    /// The row alternated Approval and Ready twice a second for as long as the
    /// gate went unanswered, and Ready is the one answer that must never
    /// appear while the operator is being waited on.
    ///
    /// The claim is withdrawn only when the agent moves on, which the tail of
    /// the recording shows it doing: a spinner frame, then the bare name.
    ///
    /// The suffix is synthetic. Codex appends the session's own name after the
    /// banner and the recorded one names a machine.
    #[test]
    fn the_blinking_codex_banner_holds_approval_for_the_whole_gate() {
        let mut row = ViewBuilder::new(1)
            .command("codex")
            .term_title("[ ! ] Action Required | agent")
            .waiting(Some(true))
            // The recorded row had been silent for minutes: blocking on a human
            // is silent by definition, and silence must not retire the claim.
            .idle_ms(193_973)
            .build();

        for phase in 0..222 {
            let title = if phase % 2 == 0 {
                "[ ! ] Action Required | agent"
            } else {
                "[ . ] Action Required | agent"
            };
            row.info.term_title = Some(title.to_string());
            assert_eq!(
                row.resolve_status(),
                StatusResolution::new(SidebarStatus::Approval, StatusSource::Title),
                "title write {phase} ({title:?}) withdrew the approval the \
                 operator was being asked to answer"
            );
        }

        // The gate was answered. Codex retitles, and only then does the claim
        // end.
        row.info.term_title = Some("⠴ agent".to_string());
        assert_eq!(
            row.resolve_status(),
            StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting)
        );
        row.info.term_title = Some("agent".to_string());
        assert_eq!(
            row.resolve_status(),
            StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting)
        );
    }

    /// PRECEDENCE at the row level: a deliberate `working` hint beats the
    /// banner. Codex's title lags its own state, so a row whose agent is
    /// telling us it is busy must not be dragged back to "Needs approval" by a
    /// string.
    #[test]
    fn a_working_hint_beats_the_title_banner_on_a_row() {
        let row = ViewBuilder::new(1)
            .command("codex")
            .term_title("[ ! ] Action Required - codex")
            .waiting(Some(true))
            .hint(HintState::Working, None, NOW)
            .build();
        assert_eq!(
            row.resolve_status(),
            StatusResolution::new(SidebarStatus::Working, StatusSource::Hint)
        );
    }

    /// The session's NAME is never read as a declaration.
    ///
    /// The two strings were one field until an agent's status line started
    /// arriving as a row name. Collapsing them again would be silent: every
    /// test above would still pass, because a real Codex session has the
    /// banner in both places. What breaks is a session that merely happens to
    /// be CALLED that — one the operator renamed, or one an agent named after
    /// a task — which would then claim to be blocked for the rest of its life,
    /// with nothing able to retract it.
    #[test]
    fn a_session_named_after_the_banner_is_not_blocked() {
        let row = ViewBuilder::new(1)
            .command("codex")
            .title("[ ! ] Action Required - codex")
            .waiting(Some(true))
            .build();
        assert_eq!(
            row.resolve_status(),
            StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting),
            "the name is not a channel the agent speaks on"
        );
    }

    /// A session that has announced nothing claims nothing.
    ///
    /// `None` has to stay distinct from "announced something unrecognised", so
    /// that the resolver treats silence as no evidence rather than as a state.
    #[test]
    fn a_session_that_never_titled_itself_claims_nothing() {
        let row = ViewBuilder::new(1)
            .command("codex")
            .waiting(Some(true))
            .build();
        assert_eq!(row.info.term_title, None);
        assert_eq!(
            row.resolve_status(),
            StatusResolution::new(SidebarStatus::Ready, StatusSource::Waiting)
        );
    }

    // ───────────────────────────────────────────────────────────────────────
    // The declaration hold
    //
    // WHY: a row that changes its mind while you are reading it is not a fast
    // row, it is an unreliable one. Both states that need a declaration are
    // scraped off a string the agent rewrites constantly, so a TUI redrawing
    // its frame dropped the banner for one push and the row resolved
    // Approval, Working, Approval, once a second, for as long as the gate was
    // up.
    //
    // What these do NOT catch: a flap between two states that BOTH require a
    // declaration (Approval to Input and back), which the hold does not damp
    // because each arrival is a fresh declaration and adopting it instantly is
    // the rule. Nothing observed has ever produced that pair.
    // ───────────────────────────────────────────────────────────────────────

    /// A codex session mid-gate, whose title carries the approval banner.
    fn gated() -> ViewBuilder {
        ViewBuilder::new(1)
            .command("codex")
            .running()
            .term_title("[ ! ] Action Required")
    }

    /// The reproduction, as a sequence. Fails on the pre-hold code at the
    /// second assertion, which is the frame the banner is missing from.
    #[test]
    fn a_dropped_declaration_does_not_end_a_declared_block() {
        let mut row = gated().build();
        row.settle_declaration(NOW);
        assert_eq!(row.status(), SidebarStatus::Approval);

        // The agent redraws and the banner is gone for one push.
        row.info.term_title = Some("codex".to_string());
        row.settle_declaration(NOW + 900);
        assert_eq!(
            row.status(),
            SidebarStatus::Approval,
            "one push without the banner ended the block"
        );
        assert_eq!(
            row.resolve_status().source,
            StatusSource::Title,
            "the held row must replay the source too, or its hedge flickers \
             instead of its word"
        );

        // The banner comes back, as it does on the next frame.
        row.info.term_title = Some("[ ! ] Action Required".to_string());
        row.settle_declaration(NOW + 1_800);
        assert_eq!(row.status(), SidebarStatus::Approval);
    }

    /// The hold is bounded. An agent that genuinely moved on is reported so,
    /// and this is the assertion that stops the fix becoming a lie: without
    /// it, "never flaps" is satisfiable by never changing at all.
    #[test]
    fn a_declaration_that_stays_gone_lapses() {
        let mut row = gated().waiting(Some(true)).build();
        row.settle_declaration(NOW);
        assert_eq!(row.status(), SidebarStatus::Approval);

        row.info.term_title = Some("codex".to_string());
        row.settle_declaration(NOW + DECLARATION_DWELL_MS - 1);
        assert_eq!(row.status(), SidebarStatus::Approval, "lapsed early");

        row.settle_declaration(NOW + DECLARATION_DWELL_MS);
        assert_eq!(
            row.status(),
            SidebarStatus::Ready,
            "the hold outlived its own bound"
        );
        assert_eq!(row.held_claim, None, "a lapsed hold must be dropped");
    }

    /// The dwell counts from the LAST declaration, not the first. A gate held
    /// for a minute must not lapse two seconds into it.
    #[test]
    fn every_declaration_renews_the_hold() {
        let mut row = gated().build();
        for step in 0..60 {
            row.settle_declaration(NOW + step * 1_000);
        }
        row.info.term_title = Some("codex".to_string());
        row.settle_declaration(NOW + 59_500);
        assert_eq!(row.status(), SidebarStatus::Approval);
    }

    /// Escalation is never delayed. The asymmetry is the design: you are told
    /// late that you are no longer needed, never that you are.
    #[test]
    fn a_block_is_adopted_the_instant_it_arrives() {
        let mut row = ViewBuilder::new(1).command("codex").running().build();
        row.settle_declaration(NOW);
        assert_eq!(row.status(), SidebarStatus::Working);

        row.info.term_title = Some("[ ! ] Action Required".to_string());
        row.settle_declaration(NOW + 1);
        assert_eq!(
            row.status(),
            SidebarStatus::Approval,
            "a declaration waited for a dwell it must never wait for"
        );
    }

    /// Precedence rule 1 outranks the hold: the exit wins over everything.
    /// A stale "act now" on a session that has already gone is exactly what
    /// that rule exists to prevent, and a hold must not smuggle one back.
    #[test]
    fn an_exit_ends_a_held_block_immediately() {
        for (code, expected) in [(0, SidebarStatus::Ready), (2, SidebarStatus::Failed)] {
            let mut row = gated().build();
            row.settle_declaration(NOW);
            assert_eq!(row.status(), SidebarStatus::Approval);

            row.info.status = vitrum_proto::SessionStatus::Exited { code: Some(code) };
            row.info.attention.failed = code != 0;
            assert_eq!(
                row.status(),
                expected,
                "a hold outlived the child that declared it"
            );
        }
    }

    /// A hold can only ever replay a state the agent declared. It has no way
    /// to reach `Working`, `Ready` or `Failed`, so it cannot invent a reading
    /// the resolver would not have produced.
    #[test]
    fn a_hold_never_holds_an_observed_state() {
        for title in ["codex", "[ ! ] Action Required"] {
            let mut row = ViewBuilder::new(1)
                .command("codex")
                .running()
                .term_title(title)
                .waiting(Some(true))
                .build();
            row.settle_declaration(NOW);
            if let Some(held) = row.held_claim {
                assert!(
                    held.resolution.status.requires_declaration(),
                    "held an observed state: {held:?}"
                );
            }
        }
    }

    /// `raw_status` is what the agent is saying right now, and stays visible
    /// through a hold. Without it there is no way to ask the question the
    /// hold's own bookkeeping needs answered.
    #[test]
    fn the_raw_resolution_is_reachable_through_a_hold() {
        let mut row = gated().waiting(Some(true)).build();
        row.settle_declaration(NOW);
        row.info.term_title = Some("codex".to_string());
        assert_eq!(row.status(), SidebarStatus::Approval);
        assert_eq!(row.raw_status().status, SidebarStatus::Ready);
    }
}
