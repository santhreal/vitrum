//! Project rollup: what a collapsed project group shows.
//!
//! Ported from T3 Code's `resolveProjectStatusIndicator`, which reduces a
//! project's sessions to the single most urgent status among them. A collapsed
//! group has one dot's worth of room, so it must spend it on the thing most
//! likely to make you expand the group.
//!
//! Two refinements on top of theirs:
//!
//! - **Settled sessions do not vote for the indicator.** A project whose only
//!   activity was a session that finished yesterday would otherwise wear a
//!   permanent `Ready` dot. They are still counted, in [`ProjectRollup::settled`],
//!   so the group can show "12, 3 parked" without claiming anything is pending.
//! - **Counts come back alongside the indicator.** The indicator answers "should
//!   I look"; the counts answer "how much is in there", and a group header has
//!   room for both. T3 Code returns only the pill.

use vitrum_proto::ProjectId;
use serde::{Deserialize, Serialize};

use crate::disposition::{Disposition, DispositionPolicy};
use crate::status::{ALL_STATUSES, SidebarStatus};
use crate::view::{Clock, SessionView};

/// Fast 64-bit non-cryptographic hasher (FxHash) for `ProjectId`.
#[derive(Default)]
pub struct FxHasher {
    hash: u64,
}

const FX_HASH_K: u64 = 0x517cc1b727220a95;

impl core::hash::Hasher for FxHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.hash = self.hash.rotate_left(5) ^ (byte as u64) ^ FX_HASH_K;
        }
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.hash = self.hash.rotate_left(5) ^ i ^ FX_HASH_K;
    }
}

pub type FxBuildHasher = core::hash::BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;
/// How many sessions are in each state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCounts {
    pub approval: usize,
    pub input: usize,
    pub working: usize,
    pub failed: usize,
    pub ready: usize,
}

impl StatusCounts {
    /// Count one session.
    pub fn add(&mut self, status: SidebarStatus) {
        *self.slot(status) += 1;
    }

    /// Sessions counted in `status`.
    pub fn get(&self, status: SidebarStatus) -> usize {
        match status {
            SidebarStatus::Approval => self.approval,
            SidebarStatus::Input => self.input,
            SidebarStatus::Working => self.working,
            SidebarStatus::Failed => self.failed,
            SidebarStatus::Ready => self.ready,
        }
    }

    /// Sessions counted in total.
    pub fn total(&self) -> usize {
        self.approval + self.input + self.working + self.failed + self.ready
    }

    /// The most urgent state with at least one session, if any.
    ///
    /// Because [`SidebarStatus::urgency`] is injective, this is a well-defined
    /// maximum and does not depend on iteration order.
    pub fn most_urgent(&self) -> Option<SidebarStatus> {
        ALL_STATUSES
            .into_iter()
            .filter(|status| self.get(*status) > 0)
            .max_by_key(|status| status.urgency())
    }

    fn slot(&mut self, status: SidebarStatus) -> &mut usize {
        match status {
            SidebarStatus::Approval => &mut self.approval,
            SidebarStatus::Input => &mut self.input,
            SidebarStatus::Working => &mut self.working,
            SidebarStatus::Failed => &mut self.failed,
            SidebarStatus::Ready => &mut self.ready,
        }
    }
}

/// Everything a collapsed project header needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRollup {
    pub project_id: ProjectId,
    /// The single most urgent state among the project's ACTIVE sessions, or
    /// `None` when the project is empty or entirely settled. `None` is the
    /// resting state and should render as no dot at all, not as a grey one.
    pub indicator: Option<SidebarStatus>,
    /// Per-state counts over the inbox sessions only.
    pub counts: StatusCounts,
    /// Sessions in the settled band: drained, or auto-settled.
    pub settled: usize,
    /// Sessions currently parked on a snooze.
    pub snoozed: usize,
    /// Sessions back in the inbox after a snooze, not yet looked at.
    pub woke: usize,
    /// Sessions that finished while the operator was not looking. Counted over
    /// every session, in whatever band, because that is the badge the operator
    /// came to the sidebar to find.
    pub unseen_completions: usize,
    /// Every session in the project.
    pub total: usize,
}

impl ProjectRollup {
    /// An empty project: no sessions, nothing to show.
    pub fn empty(project_id: ProjectId) -> Self {
        ProjectRollup {
            project_id,
            indicator: None,
            counts: StatusCounts::default(),
            settled: 0,
            snoozed: 0,
            woke: 0,
            unseen_completions: 0,
            total: 0,
        }
    }

    /// Sessions in the inbox band, woken ones included.
    pub fn active(&self) -> usize {
        self.counts.total()
    }

    /// Fold one session into this rollup.
    ///
    /// Shared by both entry points so the two can never disagree about what a
    /// row contributes: a rollup that changes when you collapse a project is a
    /// rollup nobody trusts.
    fn absorb(&mut self, row: &SessionView, clock: Clock, policy: DispositionPolicy) {
        self.total += 1;
        if row.has_unseen_completion() {
            self.unseen_completions += 1;
        }
        match row.disposition(clock, policy) {
            Disposition::Snoozed => self.snoozed += 1,
            Disposition::Settled => self.settled += 1,
            Disposition::Woke => {
                self.woke += 1;
                self.counts.add(row.status());
            }
            Disposition::Active => self.counts.add(row.status()),
        }
    }
}

/// Roll up the sessions belonging to one project.
///
/// `rows` may contain sessions from other projects; only those matching
/// `project_id` are counted. That lets a caller pass its whole list without
/// pre-partitioning it.
pub fn rollup_project<'a>(
    project_id: ProjectId,
    rows: impl IntoIterator<Item = &'a SessionView>,
    clock: Clock,
    policy: DispositionPolicy,
) -> ProjectRollup {
    rollup_rows(
        project_id,
        rows.into_iter()
            .filter(|row| row.project_id() == project_id),
        clock,
        policy,
    )
}

/// Roll up an arbitrary set of rows under `project_id`, counting every one of
/// them whatever project they belong to.
///
/// The sidebar's buckets are not all projects: it can group by filesystem
/// directory or by a folder the operator invented, and a workspace bar rolls
/// up a whole workspace. [`rollup_project`]'s filter would return an all-zero
/// rollup for every one of those, so the header of a collapsed bucket would
/// lose its status chips. This is the same fold with the predicate removed,
/// and [`rollup_project`] is now a thin filter in front of it, so there stays
/// exactly one implementation.
///
/// **`project_id` is a label here, not an identity.** It lands verbatim in
/// [`ProjectRollup::project_id`] and nothing in this function checks it
/// against the rows. Two buckets given the same label produce rollups that
/// compare unequal but key equal, so a caller that puts these in a map owns
/// the uniqueness of whatever it passes.
pub fn rollup_rows<'a>(
    project_id: ProjectId,
    rows: impl IntoIterator<Item = &'a SessionView>,
    clock: Clock,
    policy: DispositionPolicy,
) -> ProjectRollup {
    let mut rollup = ProjectRollup::empty(project_id);
    for row in rows {
        rollup.absorb(row, clock, policy);
    }
    rollup.indicator = rollup.counts.most_urgent();
    rollup
}

/// Roll up every project present in `rows`, in first-appearance order.
///
/// First-appearance order rather than sorted-by-id: the caller has already
/// decided how its projects are ordered, and re-sorting here would silently
/// override that. Projects with no sessions never appear, since they are not in
/// `rows`; call [`rollup_project`] directly for those.
pub fn rollup_all(
    rows: &[SessionView],
    clock: Clock,
    policy: DispositionPolicy,
) -> Vec<ProjectRollup> {
    let mut rollups: Vec<ProjectRollup> = Vec::new();
    let mut index_map: FxHashMap<ProjectId, usize> = FxHashMap::default();

    for row in rows {
        let project_id = row.project_id();
        let next_idx = rollups.len();
        let &mut idx = index_map.entry(project_id).or_insert_with(|| {
            rollups.push(ProjectRollup::empty(project_id));
            next_idx
        });
        rollups[idx].absorb(row, clock, policy);
    }
    for rollup in &mut rollups {
        rollup.indicator = rollup.counts.most_urgent();
    }
    rollups
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::ViewBuilder;
    use vitrum_proto::HintState;

    const NOW: u64 = 1_772_580_600_000;

    fn clock() -> Clock {
        Clock::utc(NOW)
    }

    fn policy() -> DispositionPolicy {
        DispositionPolicy::manual()
    }

    /// The core promise: one approval among nineteen working sessions is what
    /// the collapsed group shows. If the most urgent state did not win, a
    /// collapsed project would hide the one row that needs you.
    #[test]
    fn one_approval_among_many_working_sessions_wins_the_indicator() {
        let mut rows: Vec<SessionView> = (0..19)
            .map(|id| ViewBuilder::new(id).running().waiting(Some(false)).build())
            .collect();
        rows.push(
            ViewBuilder::new(99)
                .running()
                .waiting(Some(true))
                .hint(HintState::Approval, Some("rm -rf /"), NOW)
                .build(),
        );

        let rollup = rollup_project(vitrum_proto::ProjectId(1), &rows, clock(), policy());
        assert_eq!(rollup.indicator, Some(SidebarStatus::Approval));
        assert_eq!(rollup.counts.approval, 1);
        assert_eq!(rollup.counts.working, 19);
        assert_eq!(rollup.total, 20);
        assert_eq!(rollup.settled, 0);
    }

    /// The full urgency ladder, one rung at a time. Adding a more urgent
    /// session must always take over the indicator, and adding a less urgent
    /// one must never disturb it.
    #[test]
    fn the_indicator_tracks_the_most_urgent_state_in_both_directions() {
        let ladder = [
            (
                SidebarStatus::Working,
                ViewBuilder::new(1).running().waiting(Some(false)).build(),
            ),
            (
                SidebarStatus::Ready,
                ViewBuilder::new(2).running().waiting(Some(true)).build(),
            ),
            (
                SidebarStatus::Failed,
                ViewBuilder::new(3).exited(1).unread(true).build(),
            ),
            (
                SidebarStatus::Input,
                ViewBuilder::new(4)
                    .running()
                    .hint(HintState::Input, None, NOW)
                    .build(),
            ),
            (
                SidebarStatus::Approval,
                ViewBuilder::new(5)
                    .running()
                    .hint(HintState::Approval, None, NOW)
                    .build(),
            ),
        ];

        // Ascending: each addition takes over.
        let mut rows = Vec::new();
        for (expected, row) in ladder.clone() {
            rows.push(row);
            let rollup = rollup_project(vitrum_proto::ProjectId(1), &rows, clock(), policy());
            assert_eq!(rollup.indicator, Some(expected));
        }

        // Descending: the top of the ladder holds regardless of arrival order.
        let mut reversed: Vec<SessionView> =
            ladder.iter().rev().map(|(_, row)| row.clone()).collect();
        let rollup = rollup_project(vitrum_proto::ProjectId(1), &reversed, clock(), policy());
        assert_eq!(rollup.indicator, Some(SidebarStatus::Approval));
        reversed.remove(0);
        assert_eq!(
            rollup_project(vitrum_proto::ProjectId(1), &reversed, clock(), policy()).indicator,
            Some(SidebarStatus::Input)
        );
    }

    /// Settled sessions must not vote. A project whose only session finished
    /// yesterday and was read would otherwise wear a permanent Ready dot, and a
    /// permanent indicator trains people to ignore indicators.
    #[test]
    fn settled_sessions_are_counted_but_do_not_light_the_indicator() {
        let rows = vec![
            ViewBuilder::new(1)
                .exited(0)
                .last_activity_ms(NOW - 86_400_000)
                .last_visited_ms(Some(NOW))
                .build(),
            ViewBuilder::new(2)
                .exited(1)
                .last_activity_ms(NOW - 86_400_000)
                .last_visited_ms(Some(NOW))
                .build(),
            ViewBuilder::new(3)
                .running()
                .snooze(NOW - 1, NOW + 60_000)
                .build(),
        ];
        let rollup = rollup_project(vitrum_proto::ProjectId(1), &rows, clock(), policy());
        assert_eq!(rollup.indicator, None);
        assert_eq!(rollup.counts, StatusCounts::default());
        assert_eq!(rollup.settled, 2, "two drained exits");
        assert_eq!(rollup.snoozed, 1, "the parked row is its own band");
        assert_eq!(rollup.woke, 0);
        assert_eq!(rollup.total, 3);
        assert_eq!(rollup.active(), 0);
    }

    /// An unseen failure is active, so it does light the indicator. The whole
    /// point of the settled rule is that acknowledgement, not exit, is what
    /// retires a row.
    #[test]
    fn an_unseen_failure_still_lights_the_indicator() {
        let rows = vec![ViewBuilder::new(1).exited(2).unread(true).build()];
        let rollup = rollup_project(vitrum_proto::ProjectId(1), &rows, clock(), policy());
        assert_eq!(rollup.indicator, Some(SidebarStatus::Failed));
        assert_eq!(rollup.counts.failed, 1);
        assert_eq!(rollup.settled, 0);
        assert_eq!(rollup.unseen_completions, 1);
    }

    /// Unseen completions are counted across the fold, because a session that
    /// finished unseen and then got snoozed still finished unseen. The badge and
    /// the indicator answer different questions.
    #[test]
    fn unseen_completions_are_counted_even_for_settled_rows() {
        let rows = vec![
            ViewBuilder::new(1)
                .exited(0)
                .last_activity_ms(NOW - 1_000)
                .unread(true)
                .snooze(NOW - 10, NOW + 60_000)
                .build(),
            ViewBuilder::new(2)
                .exited(0)
                .last_activity_ms(NOW - 1_000)
                .last_visited_ms(Some(NOW))
                .build(),
        ];
        let rollup = rollup_project(vitrum_proto::ProjectId(1), &rows, clock(), policy());
        assert_eq!(rollup.settled, 1);
        assert_eq!(rollup.snoozed, 1);
        assert_eq!(rollup.unseen_completions, 1);
        assert_eq!(rollup.indicator, None);
    }

    /// A project with no sessions must render as nothing pending, not as an
    /// error and not as a grey dot.
    #[test]
    fn an_empty_project_rolls_up_to_nothing() {
        let rollup = rollup_project(vitrum_proto::ProjectId(7), &[], clock(), policy());
        assert_eq!(rollup, ProjectRollup::empty(vitrum_proto::ProjectId(7)));
        assert_eq!(rollup.indicator, None);
        assert_eq!(rollup.total, 0);
        assert_eq!(rollup.active(), 0);
    }

    /// Sessions from other projects must not leak into a project's rollup. The
    /// caller passes its whole list, so a missing filter would make every
    /// project show every other project's state.
    #[test]
    fn sessions_from_other_projects_are_ignored() {
        let rows = vec![
            ViewBuilder::new(1)
                .project(1)
                .running()
                .waiting(Some(false))
                .build(),
            ViewBuilder::new(2)
                .project(2)
                .running()
                .hint(HintState::Approval, None, NOW)
                .build(),
        ];
        let first = rollup_project(vitrum_proto::ProjectId(1), &rows, clock(), policy());
        assert_eq!(first.indicator, Some(SidebarStatus::Working));
        assert_eq!(first.total, 1);

        let second = rollup_project(vitrum_proto::ProjectId(2), &rows, clock(), policy());
        assert_eq!(second.indicator, Some(SidebarStatus::Approval));
        assert_eq!(second.total, 1);
    }

    /// `rollup_all` must agree with `rollup_project` for every project and must
    /// preserve the caller's ordering. Two implementations that disagree would
    /// make a group header change when you collapse it.
    #[test]
    fn rollup_all_matches_per_project_rollup_and_preserves_input_order() {
        let rows = vec![
            ViewBuilder::new(1)
                .project(9)
                .running()
                .waiting(Some(false))
                .build(),
            ViewBuilder::new(2)
                .project(4)
                .running()
                .hint(HintState::Input, None, NOW)
                .build(),
            ViewBuilder::new(3)
                .project(9)
                .exited(1)
                .unread(true)
                .build(),
            ViewBuilder::new(4)
                .project(4)
                .exited(0)
                .last_visited_ms(Some(NOW))
                .build(),
        ];
        let all = rollup_all(&rows, clock(), policy());
        assert_eq!(
            all.iter()
                .map(|rollup| rollup.project_id.0)
                .collect::<Vec<_>>(),
            vec![9, 4],
            "first-appearance order, not sorted by id"
        );
        for rollup in &all {
            assert_eq!(
                *rollup,
                rollup_project(rollup.project_id, &rows, clock(), policy())
            );
        }
        assert_eq!(all[0].indicator, Some(SidebarStatus::Failed));
        assert_eq!(all[0].total, 2);
        assert_eq!(all[1].indicator, Some(SidebarStatus::Input));
        assert_eq!(all[1].settled, 1);
    }

    /// Counts must be exhaustive and must not double count. A slot wired to the
    /// wrong field is invisible until a group shows "3 working" for one working
    /// session.
    #[test]
    fn counts_cover_every_state_exactly_once() {
        let mut counts = StatusCounts::default();
        for status in ALL_STATUSES {
            counts.add(status);
        }
        assert_eq!(
            counts,
            StatusCounts {
                approval: 1,
                input: 1,
                working: 1,
                failed: 1,
                ready: 1,
            }
        );
        assert_eq!(counts.total(), 5);
        for status in ALL_STATUSES {
            assert_eq!(counts.get(status), 1, "{status:?}");
        }
        counts.add(SidebarStatus::Ready);
        assert_eq!(counts.ready, 2);
        assert_eq!(counts.total(), 6);
    }

    /// `most_urgent` over an empty set is `None`, and over a single state is
    /// that state. The rollup indicator is defined by this, so both edges matter.
    #[test]
    fn most_urgent_handles_empty_and_single_state_counts() {
        assert_eq!(StatusCounts::default().most_urgent(), None);

        let mut only_working = StatusCounts::default();
        only_working.add(SidebarStatus::Working);
        assert_eq!(only_working.most_urgent(), Some(SidebarStatus::Working));

        let mut mixed = StatusCounts::default();
        mixed.add(SidebarStatus::Working);
        mixed.add(SidebarStatus::Ready);
        mixed.add(SidebarStatus::Failed);
        assert_eq!(mixed.most_urgent(), Some(SidebarStatus::Failed));
    }

    /// The rollup is handed to the view layer and may be cached across a
    /// reconnect, so it has to survive JSON with its field names intact.
    #[test]
    fn a_rollup_round_trips_through_json() {
        let rows = vec![
            ViewBuilder::new(1).running().waiting(Some(true)).build(),
            ViewBuilder::new(2)
                .running()
                .snooze(NOW - 1, NOW + 5)
                .build(),
        ];
        let rollup = rollup_project(vitrum_proto::ProjectId(1), &rows, clock(), policy());
        let json = serde_json::to_string(&rollup).expect("rollup serialises");
        assert!(json.contains("\"indicator\":\"ready\""));
        assert!(json.contains("\"unseenCompletions\":0"));
        let back: ProjectRollup = serde_json::from_str(&json).expect("rollup round-trips");
        assert_eq!(back, rollup);
    }

    /// A bucket that is not a project must count every row it was given.
    ///
    /// Locks out the defect that motivated [`rollup_rows`]: the sidebar can
    /// group by filesystem directory or by a folder the operator invented, and
    /// [`rollup_project`]'s `project_id` filter returned an all-zero rollup for
    /// every such bucket, so a collapsed folder header lost its status chips
    /// entirely. Three rows from three different projects, none of them
    /// matching the label, must still all be counted.
    #[test]
    fn rollup_rows_counts_a_bucket_drawn_from_several_projects() {
        let rows = vec![
            ViewBuilder::new(1)
                .project(7)
                .running()
                .waiting(Some(true))
                .build(),
            ViewBuilder::new(2)
                .project(8)
                .running()
                .waiting(Some(false))
                .build(),
            ViewBuilder::new(3).project(9).exited(0).build(),
        ];
        let label = vitrum_proto::ProjectId(u64::MAX);
        let rollup = rollup_rows(label, &rows, clock(), policy());

        assert_eq!(rollup.total, 3, "every row in the bucket is counted");
        assert_eq!(rollup.project_id, label, "the id is a label, echoed back");
        assert!(
            rollup.indicator.is_some(),
            "a non-empty bucket must light its collapsed header"
        );

        let filtered = rollup_project(label, &rows, clock(), policy());
        assert_eq!(
            filtered.total, 0,
            "and this is the behaviour rollup_rows exists to avoid"
        );
    }

    /// `rollup_project` must stay exactly a filter in front of `rollup_rows`,
    /// or the two folds drift and a project header and a folder header start
    /// disagreeing about the same rows.
    #[test]
    fn rollup_project_is_rollup_rows_over_the_matching_rows() {
        let rows = vec![
            ViewBuilder::new(1)
                .project(1)
                .running()
                .waiting(Some(true))
                .build(),
            ViewBuilder::new(2)
                .project(2)
                .running()
                .waiting(Some(false))
                .build(),
            ViewBuilder::new(3).project(1).exited(0).build(),
        ];
        let mine: Vec<SessionView> = rows
            .iter()
            .filter(|r| r.project_id() == vitrum_proto::ProjectId(1))
            .cloned()
            .collect();
        assert_eq!(
            rollup_project(vitrum_proto::ProjectId(1), &rows, clock(), policy()),
            rollup_rows(vitrum_proto::ProjectId(1), &mine, clock(), policy())
        );
    }
}
