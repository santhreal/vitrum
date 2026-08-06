//! Sidebar ordering: three sections, and a deliberately quiet sort inside each.
//!
//! Ported from T3 Code's `sortThreadsForSidebarV2` and
//! `sortSettledThreadsForSidebarV2`. Their central insight is that the bands of
//! the list answer different questions and must not share a comparator: the
//! inbox is a work queue and the settled pile is history.
//!
//! # Coarse order is sections, not sorting
//!
//! [`Section::Active`], then [`Section::Snoozed`], then [`Section::Settled`].
//! That is where the strong ordering lives, and it is what keeps twenty dead
//! sessions from sitting above one that is streaming.
//!
//! # Inside the inbox, rows do not move
//!
//! [`ActiveOrder::Static`] is the default and it is T3 Code's rule: creation
//! order, newest first, full stop. Activity never reorders the inbox. A row
//! holds its position from open until it changes section, so the screen only
//! moves at lifecycle transitions and nothing shifts under the cursor while you
//! are reading it. Status is carried by the row's own indicator, and a woken
//! session reappears exactly where it was wearing a [`Disposition::Woke`] badge,
//! which only works because the sort did not move it.
//!
//! The obvious objection is that the one session waiting on an approval could
//! be anywhere in a list of twenty. The answer is not to reorder the list under
//! the operator's hands; it is
//! [`crate::traversal::adjacent_matching`], a keypress that jumps to the next
//! row that wants you. Movement is expensive and a keypress is cheap.
//!
//! [`ActiveOrder::Urgency`] is available for a caller that wants the other
//! trade: strict urgency first, rows moving as status changes. Both are total
//! orders and both are tested; the default is `Static`.
//!
//! # Total, stable, idempotent
//!
//! Every comparator ends in [`SessionId`](vitrum_proto::SessionId), which is
//! unique. That makes each a total order, so sorting is deterministic: the same
//! set produces the same list regardless of arrival order, and re-sorting an
//! already-sorted list is a no-op. A sidebar that re-derives its order on every
//! daemon update must not reshuffle rows whose data did not change.

use core::cmp::Ordering;

use crate::disposition::{Disposition, DispositionPolicy, Section};
use crate::view::{Clock, SessionView};

/// How the inbox section is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActiveOrder {
    /// Creation order, newest first. Rows never move except at section
    /// transitions. The default, and the reason the Woke badge works.
    #[default]
    Static,
    /// Urgency first, then attention, then newest. Puts the rows that want you
    /// at the top at the cost of moving rows as their status changes.
    Urgency,
}

/// Sort weight for one inbox row under [`ActiveOrder::Urgency`], highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrgencyKey {
    /// [`crate::status::SidebarStatus::urgency`] of the row.
    pub urgency: u8,
    /// [`vitrum_proto::Attention::priority`], which separates rows inside one
    /// status band. Two `Ready` rows are not equal if one of them rang the bell.
    pub attention: u8,
    /// Newest first.
    pub created_at_ms: u64,
    /// Unique final tiebreak; makes the order total.
    pub id: u64,
}

impl UrgencyKey {
    pub fn of(row: &SessionView) -> Self {
        UrgencyKey {
            urgency: row.status().urgency(),
            attention: row.info.attention.priority(),
            created_at_ms: row.info.created_at_ms,
            id: row.id().0,
        }
    }

    fn cmp_key(&self, other: &Self) -> Ordering {
        other
            .urgency
            .cmp(&self.urgency)
            .then(other.attention.cmp(&self.attention))
            .then(other.created_at_ms.cmp(&self.created_at_ms))
            .then(self.id.cmp(&other.id))
    }
}

/// Sort weight for one inbox row under [`ActiveOrder::Static`], highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticKey {
    /// Newest first.
    pub created_at_ms: u64,
    /// Unique final tiebreak; makes the order total.
    pub id: u64,
}

impl StaticKey {
    pub fn of(row: &SessionView) -> Self {
        StaticKey {
            created_at_ms: row.info.created_at_ms,
            id: row.id().0,
        }
    }

    fn cmp_key(&self, other: &Self) -> Ordering {
        other
            .created_at_ms
            .cmp(&self.created_at_ms)
            .then(self.id.cmp(&other.id))
    }
}

/// Sort weight for one row below the inbox, highest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettledKey {
    /// [`SessionView::settled_at_ms`]: when the work ended, or when the
    /// operator explicitly parked it.
    pub settled_at_ms: u64,
    /// Unique final tiebreak; makes the order total.
    pub id: u64,
}

impl SettledKey {
    pub fn of(row: &SessionView) -> Self {
        SettledKey {
            settled_at_ms: row.settled_at_ms(),
            id: row.id().0,
        }
    }

    fn cmp_key(&self, other: &Self) -> Ordering {
        other
            .settled_at_ms
            .cmp(&self.settled_at_ms)
            .then(self.id.cmp(&other.id))
    }
}

/// Order two inbox rows under `order`.
pub fn compare_active(left: &SessionView, right: &SessionView, order: ActiveOrder) -> Ordering {
    match order {
        ActiveOrder::Static => StaticKey::of(left).cmp_key(&StaticKey::of(right)),
        ActiveOrder::Urgency => UrgencyKey::of(left).cmp_key(&UrgencyKey::of(right)),
    }
}

/// Order two snoozed rows: soonest wake first, then id.
///
/// Ascending wake time, unlike every other section, because the useful question
/// about a parked row is "when does this come back". Ties fall to the id, and a
/// row somehow in this section without a snooze sorts last rather than first.
pub fn compare_snoozed(left: &SessionView, right: &SessionView) -> Ordering {
    let wake = |row: &SessionView| row.snooze.map_or(u64::MAX, |snooze| snooze.wake_at_ms);
    wake(left)
        .cmp(&wake(right))
        .then(left.id().0.cmp(&right.id().0))
}

/// Order two settled rows: most recently ended first, then id.
pub fn compare_settled(left: &SessionView, right: &SessionView) -> Ordering {
    SettledKey::of(left).cmp_key(&SettledKey::of(right))
}

/// Where each section starts and ends in an arranged slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionSplit {
    /// `rows[..active_end]` is the inbox.
    pub active_end: usize,
    /// `rows[active_end..snoozed_end]` is the snoozed band.
    pub snoozed_end: usize,
}

impl SectionSplit {
    pub fn active_len(&self) -> usize {
        self.active_end
    }

    pub fn snoozed_len(&self) -> usize {
        self.snoozed_end - self.active_end
    }

    pub fn settled_len(&self, total: usize) -> usize {
        total - self.snoozed_end
    }

    /// The section a position in the arranged slice belongs to.
    pub fn section_at(&self, index: usize) -> Section {
        if index < self.active_end {
            Section::Active
        } else if index < self.snoozed_end {
            Section::Snoozed
        } else {
            Section::Settled
        }
    }
}

/// Arrange a session list into its final sidebar order.
///
/// Sections come first, then each section is sorted with its own comparator.
pub fn arrange(
    rows: &mut [SessionView],
    clock: Clock,
    policy: DispositionPolicy,
    order: ActiveOrder,
) -> SectionSplit {
    // `sort_by_key` is stable, so this partitions into the three bands without
    // allocating a second vector. Each band is re-sorted immediately after.
    rows.sort_by_key(|row| row.section(clock, policy));
    let active_end = rows.partition_point(|row| row.section(clock, policy) == Section::Active);
    let snoozed_end = rows.partition_point(|row| row.section(clock, policy) <= Section::Snoozed);

    rows[..active_end].sort_by(|left, right| compare_active(left, right, order));
    rows[active_end..snoozed_end].sort_by(compare_snoozed);
    rows[snoozed_end..].sort_by(compare_settled);

    SectionSplit {
        active_end,
        snoozed_end,
    }
}

/// The arranged list split into three borrowed bands.
#[derive(Debug)]
pub struct Arranged<'a> {
    pub active: &'a [SessionView],
    pub snoozed: &'a [SessionView],
    pub settled: &'a [SessionView],
}

/// [`arrange`] returning the three bands rather than the indices.
pub fn arrange_sections<'a>(
    rows: &'a mut [SessionView],
    clock: Clock,
    policy: DispositionPolicy,
    order: ActiveOrder,
) -> Arranged<'a> {
    let split = arrange(rows, clock, policy, order);
    let (active, rest) = rows.split_at(split.active_end);
    let (snoozed, settled) = rest.split_at(split.snoozed_len());
    Arranged {
        active,
        snoozed,
        settled,
    }
}

/// Count the rows in each disposition, for section headers like "Snoozed (3)".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SectionCounts {
    pub active: usize,
    pub woke: usize,
    pub snoozed: usize,
    pub settled: usize,
}

impl SectionCounts {
    pub fn of(rows: &[SessionView], clock: Clock, policy: DispositionPolicy) -> Self {
        let mut counts = SectionCounts::default();
        for row in rows {
            match row.disposition(clock, policy) {
                Disposition::Active => counts.active += 1,
                Disposition::Woke => counts.woke += 1,
                Disposition::Snoozed => counts.snoozed += 1,
                Disposition::Settled => counts.settled += 1,
            }
        }
        counts
    }

    /// Rows in the inbox, woken ones included.
    pub fn inbox(&self) -> usize {
        self.active + self.woke
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::SidebarStatus;
    use crate::testkit::ViewBuilder;
    use vitrum_proto::{HintState, IDLE_ATTENTION_MS};

    const NOW: u64 = 1_772_580_600_000;
    const HOUR: u64 = 3_600_000;

    fn clock() -> Clock {
        Clock::utc(NOW)
    }

    fn policy() -> DispositionPolicy {
        DispositionPolicy::manual()
    }

    fn ids(rows: &[SessionView]) -> Vec<u64> {
        rows.iter().map(|row| row.id().0).collect()
    }

    /// The default inbox order is deliberately static: newest first and nothing
    /// else. An approval-blocked row does NOT jump to the top, because rows
    /// moving under the cursor is the disorientation this design rejects.
    #[test]
    fn the_static_inbox_order_is_creation_order_regardless_of_status() {
        let mut rows = vec![
            ViewBuilder::new(1).running().waiting(Some(false)).created_at_ms(1_000).build(),
            ViewBuilder::new(2)
                .running()
                .waiting(Some(true))
                .hint(HintState::Approval, None, NOW)
                .created_at_ms(2_000)
                .build(),
            ViewBuilder::new(3).running().waiting(Some(false)).created_at_ms(3_000).build(),
        ];
        rows.sort_by(|left, right| compare_active(left, right, ActiveOrder::Static));
        assert_eq!(ids(&rows), vec![3, 2, 1]);
        assert_eq!(rows[1].status(), SidebarStatus::Approval);
    }

    /// The opt-in ordering does the other thing: one row per status, all created
    /// together so only urgency can decide.
    #[test]
    fn the_urgency_inbox_order_puts_human_blocks_on_top() {
        let mut rows = vec![
            ViewBuilder::new(1).running().waiting(Some(false)).build(),
            ViewBuilder::new(2)
                .running()
                .hint(HintState::Input, None, NOW)
                .build(),
            ViewBuilder::new(3).exited(1).unread(true).build(),
            ViewBuilder::new(4)
                .running()
                .hint(HintState::Approval, None, NOW)
                .build(),
            ViewBuilder::new(5).running().waiting(Some(true)).build(),
        ];
        rows.sort_by(|left, right| compare_active(left, right, ActiveOrder::Urgency));
        assert_eq!(ids(&rows), vec![4, 2, 3, 5, 1]);
        assert_eq!(
            rows.iter().map(SessionView::status).collect::<Vec<_>>(),
            vec![
                SidebarStatus::Approval,
                SidebarStatus::Input,
                SidebarStatus::Failed,
                SidebarStatus::Ready,
                SidebarStatus::Working,
            ]
        );
    }

    /// Inside one status band the urgency order falls to attention, so a bell
    /// row outranks a merely idle one even though it is older.
    #[test]
    fn attention_separates_rows_within_one_status_band() {
        let mut rows = vec![
            ViewBuilder::new(1)
                .running()
                .idle_ms(IDLE_ATTENTION_MS)
                .created_at_ms(9_000)
                .build(),
            ViewBuilder::new(2).running().bell(true).created_at_ms(1_000).build(),
        ];
        rows.sort_by(|left, right| compare_active(left, right, ActiveOrder::Urgency));
        assert_eq!(
            rows.iter().map(SessionView::status).collect::<Vec<_>>(),
            vec![SidebarStatus::Ready, SidebarStatus::Ready]
        );
        assert_eq!(ids(&rows), vec![2, 1]);
    }

    /// Fully tied rows fall to the id, which is unique. Without this the order
    /// would depend on arrival order and the list would jitter every refresh.
    #[test]
    fn fully_tied_rows_are_broken_by_ascending_id() {
        for order in [ActiveOrder::Static, ActiveOrder::Urgency] {
            let mut rows = vec![
                ViewBuilder::new(30).running().created_at_ms(1_000).build(),
                ViewBuilder::new(10).running().created_at_ms(1_000).build(),
                ViewBuilder::new(20).running().created_at_ms(1_000).build(),
            ];
            rows.sort_by(|left, right| compare_active(left, right, order));
            assert_eq!(ids(&rows), vec![10, 20, 30], "{order:?}");
        }
    }

    /// The order must not depend on how rows arrived. Every permutation of a
    /// tied set has to produce the same list, or two clients showing the same
    /// daemon disagree and a single client jitters between refreshes.
    #[test]
    fn every_input_permutation_of_a_tied_set_produces_one_order() {
        let source: Vec<SessionView> = (1..=4)
            .map(|id| ViewBuilder::new(id).running().created_at_ms(1_000).build())
            .collect();
        let permutations: [[usize; 4]; 6] = [
            [0, 1, 2, 3],
            [3, 2, 1, 0],
            [1, 0, 3, 2],
            [2, 3, 0, 1],
            [0, 3, 1, 2],
            [3, 0, 2, 1],
        ];
        for order in [ActiveOrder::Static, ActiveOrder::Urgency] {
            for permutation in permutations {
                let mut rows: Vec<SessionView> =
                    permutation.iter().map(|index| source[*index].clone()).collect();
                rows.sort_by(|left, right| compare_active(left, right, order));
                assert_eq!(ids(&rows), vec![1, 2, 3, 4], "{order:?} {permutation:?}");
            }
        }
    }

    /// Sorting an already-sorted list must change nothing. The sidebar re-runs
    /// this on every daemon update, so a comparator that is merely "mostly
    /// consistent" would make rows swap places on an unrelated field change.
    #[test]
    fn repeated_sorts_are_idempotent() {
        let build = || {
            vec![
                ViewBuilder::new(5).running().bell(true).created_at_ms(4_000).build(),
                ViewBuilder::new(2)
                    .running()
                    .hint(HintState::Approval, None, NOW)
                    .created_at_ms(1_000)
                    .build(),
                ViewBuilder::new(9).running().created_at_ms(7_000).build(),
                ViewBuilder::new(1).exited(3).unread(true).created_at_ms(2_000).build(),
                ViewBuilder::new(7).running().created_at_ms(7_000).build(),
            ]
        };
        for (order, expected) in [
            (ActiveOrder::Static, vec![7, 9, 5, 1, 2]),
            (ActiveOrder::Urgency, vec![2, 1, 5, 7, 9]),
        ] {
            let mut rows = build();
            rows.sort_by(|left, right| compare_active(left, right, order));
            assert_eq!(ids(&rows), expected, "{order:?}");
            for _ in 0..5 {
                rows.sort_by(|left, right| compare_active(left, right, order));
                assert_eq!(ids(&rows), expected, "{order:?} repeated");
            }
        }
    }

    /// Both comparators must be strict total orders: antisymmetric and
    /// transitive. A comparator that is not gets `sort_by` into undefined
    /// territory and produces different lists on different inputs.
    #[test]
    fn both_active_comparators_are_strict_total_orders() {
        let rows = vec![
            ViewBuilder::new(1).running().created_at_ms(1_000).build(),
            ViewBuilder::new(2).running().created_at_ms(1_000).bell(true).build(),
            ViewBuilder::new(3)
                .running()
                .hint(HintState::Input, None, NOW)
                .created_at_ms(5_000)
                .build(),
            ViewBuilder::new(4).exited(1).unread(true).created_at_ms(1_000).build(),
            ViewBuilder::new(5).starting().created_at_ms(9_000).build(),
        ];
        for order in [ActiveOrder::Static, ActiveOrder::Urgency] {
            for left in &rows {
                assert_eq!(compare_active(left, left, order), Ordering::Equal);
                for right in &rows {
                    assert_eq!(
                        compare_active(left, right, order),
                        compare_active(right, left, order).reverse(),
                        "antisymmetry broken for {} vs {} under {order:?}",
                        left.id().0,
                        right.id().0
                    );
                    for third in &rows {
                        if compare_active(left, right, order) == Ordering::Less
                            && compare_active(right, third, order) == Ordering::Less
                        {
                            assert_eq!(
                                compare_active(left, third, order),
                                Ordering::Less,
                                "transitivity broken under {order:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Settled rows are history and order by when the work ended, not by
    /// urgency and not by creation. A failure from an hour ago must not outrank
    /// a session that finished a minute ago just because it failed.
    #[test]
    fn settled_rows_order_by_when_work_ended() {
        let mut rows = vec![
            ViewBuilder::new(1)
                .exited(1)
                .created_at_ms(1)
                .last_activity_ms(NOW - HOUR)
                .last_visited_ms(Some(NOW))
                .build(),
            ViewBuilder::new(2)
                .exited(0)
                .created_at_ms(9_000)
                .last_activity_ms(NOW - 60_000)
                .last_visited_ms(Some(NOW))
                .build(),
            ViewBuilder::new(3)
                .exited(0)
                .created_at_ms(5_000)
                .last_activity_ms(NOW - 600_000)
                .last_visited_ms(Some(NOW))
                .build(),
        ];
        rows.sort_by(compare_settled);
        assert_eq!(ids(&rows), vec![2, 3, 1]);
    }

    /// Settled ties break by id, and the settled sort must be idempotent for
    /// the same reason the inbox one is.
    #[test]
    fn settled_ties_break_by_id_and_repeated_sorts_are_stable() {
        let build = |id: u64| {
            ViewBuilder::new(id)
                .exited(0)
                .created_at_ms(1_000)
                .last_activity_ms(2_000)
                .last_visited_ms(Some(NOW))
                .build()
        };
        let mut rows = vec![build(42), build(7), build(19)];
        rows.sort_by(compare_settled);
        assert_eq!(ids(&rows), vec![7, 19, 42]);
        rows.sort_by(compare_settled);
        assert_eq!(ids(&rows), vec![7, 19, 42]);
    }

    /// The snoozed band sorts by soonest wake, ascending, because the only
    /// question worth asking about a parked row is when it comes back. Every
    /// other band is newest-first, so this one is easy to get backwards.
    #[test]
    fn the_snoozed_band_sorts_by_soonest_wake_first() {
        let mut rows = vec![
            ViewBuilder::new(1).running().snooze(NOW - 10, NOW + 8 * HOUR).build(),
            ViewBuilder::new(2).running().snooze(NOW - 10, NOW + HOUR).build(),
            ViewBuilder::new(3).running().snooze(NOW - 10, NOW + 3 * HOUR).build(),
        ];
        rows.sort_by(compare_snoozed);
        assert_eq!(ids(&rows), vec![2, 3, 1]);
        rows.sort_by(compare_snoozed);
        assert_eq!(ids(&rows), vec![2, 3, 1]);
    }

    /// A snoozed row sorts by when it was parked once it reaches the settled
    /// band, which puts the one you just parked at the top instead of buried by
    /// its own age.
    #[test]
    fn a_freshly_snoozed_old_session_sorts_to_the_top_of_settled() {
        let mut rows = vec![
            ViewBuilder::new(1)
                .exited(0)
                .created_at_ms(1)
                .last_activity_ms(NOW - 1_000)
                .last_visited_ms(Some(NOW))
                .build(),
            ViewBuilder::new(2)
                .running()
                .created_at_ms(1)
                .last_activity_ms(1)
                .snooze(NOW - 10, NOW + HOUR)
                .build(),
        ];
        rows.sort_by(compare_settled);
        assert_eq!(ids(&rows), vec![2, 1]);
    }

    /// The three bands, correctly partitioned and each correctly sorted. This is
    /// the defect Main filed: a streaming session must never sit below dead
    /// ones, and sections are what guarantee it.
    #[test]
    fn arrange_splits_three_sections_and_sorts_each() {
        let mut rows = vec![
            // settled: seen clean exit
            ViewBuilder::new(1)
                .exited(0)
                .last_activity_ms(NOW - 5_000)
                .last_visited_ms(Some(NOW))
                .build(),
            // inbox: working, newest
            ViewBuilder::new(2).running().waiting(Some(false)).created_at_ms(9_000).build(),
            // snoozed
            ViewBuilder::new(3)
                .running()
                .waiting(Some(false))
                .last_activity_ms(NOW - HOUR)
                .snooze(NOW - HOUR, NOW + 2 * HOUR)
                .build(),
            // inbox: unseen failure
            ViewBuilder::new(4).exited(2).unread(true).created_at_ms(3_000).build(),
            // settled: seen failure, older
            ViewBuilder::new(5)
                .exited(1)
                .last_activity_ms(NOW - 900_000)
                .last_visited_ms(Some(NOW))
                .build(),
            // snoozed, waking sooner
            ViewBuilder::new(6)
                .running()
                .waiting(Some(false))
                .last_activity_ms(NOW - HOUR)
                .snooze(NOW - HOUR, NOW + HOUR)
                .build(),
        ];
        let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Static);
        assert_eq!(split.active_end, 2);
        assert_eq!(split.snoozed_end, 4);
        assert_eq!(split.active_len(), 2);
        assert_eq!(split.snoozed_len(), 2);
        assert_eq!(split.settled_len(rows.len()), 2);

        assert_eq!(ids(&rows[..2]), vec![2, 4], "inbox, newest first");
        assert_eq!(ids(&rows[2..4]), vec![6, 3], "snoozed, soonest wake first");
        assert_eq!(ids(&rows[4..]), vec![1, 5], "settled, most recent first");

        assert_eq!(split.section_at(0), Section::Active);
        assert_eq!(split.section_at(2), Section::Snoozed);
        assert_eq!(split.section_at(5), Section::Settled);
    }

    /// `arrange` must be idempotent as a whole, not just its parts: the
    /// partition plus all three sorts, re-run, must produce the identical list.
    #[test]
    fn arrange_is_idempotent() {
        let mut rows = vec![
            ViewBuilder::new(1).running().waiting(Some(false)).created_at_ms(1_000).build(),
            ViewBuilder::new(2)
                .exited(0)
                .last_activity_ms(NOW - 10)
                .last_visited_ms(Some(NOW))
                .build(),
            ViewBuilder::new(3)
                .running()
                .waiting(Some(false))
                .last_activity_ms(NOW - HOUR)
                .snooze(NOW - HOUR, NOW + HOUR)
                .build(),
            ViewBuilder::new(4).running().bell(true).created_at_ms(2_000).build(),
        ];
        let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Static);
        let first = ids(&rows);
        for _ in 0..3 {
            assert_eq!(arrange(&mut rows, clock(), policy(), ActiveOrder::Static), split);
            assert_eq!(ids(&rows), first);
        }
        assert_eq!(first, vec![4, 1, 3, 2]);
    }

    /// The point of the static order plus the Woke badge: a session that wakes
    /// returns to the inbox in the position it left, not at the top. If the sort
    /// moved it, the badge would be redundant and the list would lurch.
    #[test]
    fn a_waking_row_returns_to_its_original_inbox_position() {
        let build = || {
            vec![
                ViewBuilder::new(1).running().waiting(Some(false)).created_at_ms(3_000).build(),
                ViewBuilder::new(2)
                    .running()
                    .waiting(Some(false))
                    .created_at_ms(2_000)
                    .last_activity_ms(NOW - 2 * HOUR)
                    .snooze(NOW - 2 * HOUR, NOW + HOUR)
                    .build(),
                ViewBuilder::new(3).running().waiting(Some(false)).created_at_ms(1_000).build(),
            ]
        };

        let mut parked = build();
        let split = arrange(&mut parked, clock(), policy(), ActiveOrder::Static);
        assert_eq!(ids(&parked[..split.active_end]), vec![1, 3]);
        assert_eq!(ids(&parked[split.active_end..split.snoozed_end]), vec![2]);

        let mut woken = build();
        let awake = Clock::utc(NOW + HOUR);
        let split = arrange(&mut woken, awake, policy(), ActiveOrder::Static);
        assert_eq!(split.snoozed_len(), 0);
        assert_eq!(
            ids(&woken[..split.active_end]),
            vec![1, 2, 3],
            "the woken row is back between its neighbours, not on top"
        );
        assert_eq!(woken[1].disposition(awake, policy()), Disposition::Woke);
    }

    /// Empty and single-element lists must not panic and must report a sane
    /// split. The sidebar renders these on first connect.
    #[test]
    fn arrange_handles_empty_and_single_lists() {
        let mut empty: Vec<SessionView> = Vec::new();
        let split = arrange(&mut empty, clock(), policy(), ActiveOrder::Static);
        assert_eq!(split, SectionSplit { active_end: 0, snoozed_end: 0 });

        let mut one = vec![ViewBuilder::new(1).running().waiting(Some(false)).build()];
        let split = arrange(&mut one, clock(), policy(), ActiveOrder::Static);
        assert_eq!(split, SectionSplit { active_end: 1, snoozed_end: 1 });

        let mut settled = vec![
            ViewBuilder::new(1)
                .exited(0)
                .last_visited_ms(Some(NOW))
                .build(),
        ];
        let split = arrange(&mut settled, clock(), policy(), ActiveOrder::Static);
        assert_eq!(split, SectionSplit { active_end: 0, snoozed_end: 0 });
        assert_eq!(split.settled_len(1), 1);
    }

    /// The borrowed-bands form must agree with the index form exactly, since
    /// the app uses whichever is convenient at the call site.
    #[test]
    fn arrange_sections_matches_the_index_form() {
        let build = || {
            vec![
                ViewBuilder::new(1).running().waiting(Some(false)).build(),
                ViewBuilder::new(2)
                    .exited(0)
                    .last_visited_ms(Some(NOW))
                    .build(),
                ViewBuilder::new(3)
                    .running()
                    .waiting(Some(false))
                    .last_activity_ms(NOW - HOUR)
                    .snooze(NOW - HOUR, NOW + HOUR)
                    .build(),
            ]
        };
        let mut by_index = build();
        let split = arrange(&mut by_index, clock(), policy(), ActiveOrder::Static);

        let mut by_section = build();
        let arranged = arrange_sections(&mut by_section, clock(), policy(), ActiveOrder::Static);
        assert_eq!(arranged.active.len(), split.active_len());
        assert_eq!(arranged.snoozed.len(), split.snoozed_len());
        assert_eq!(arranged.settled.len(), split.settled_len(3));
        assert_eq!(ids(arranged.active), vec![1]);
        assert_eq!(ids(arranged.snoozed), vec![3]);
        assert_eq!(ids(arranged.settled), vec![2]);
    }

    /// Section headers show counts, and a woken row counts towards the inbox
    /// rather than towards a band of its own.
    #[test]
    fn section_counts_report_woke_separately_but_count_it_in_the_inbox() {
        let rows = vec![
            ViewBuilder::new(1).running().waiting(Some(false)).build(),
            ViewBuilder::new(2)
                .running()
                .waiting(Some(false))
                .last_activity_ms(NOW - 5 * HOUR)
                .snooze(NOW - 5 * HOUR, NOW - HOUR)
                .build(),
            ViewBuilder::new(3)
                .running()
                .waiting(Some(false))
                .last_activity_ms(NOW - HOUR)
                .snooze(NOW - HOUR, NOW + HOUR)
                .build(),
            ViewBuilder::new(4)
                .exited(0)
                .last_visited_ms(Some(NOW))
                .build(),
        ];
        let counts = SectionCounts::of(&rows, clock(), policy());
        assert_eq!(
            counts,
            SectionCounts {
                active: 1,
                woke: 1,
                snoozed: 1,
                settled: 1,
            }
        );
        assert_eq!(counts.inbox(), 2);
    }

    /// Exposed keys must describe the comparators, since callers memoise them.
    /// A key that disagrees would make a memoised sort differ from a fresh one.
    #[test]
    fn exposed_keys_describe_the_comparators() {
        let row = ViewBuilder::new(11)
            .running()
            .hint(HintState::Input, None, NOW)
            .created_at_ms(4_000)
            .bell(true)
            .build();
        let urgency = UrgencyKey::of(&row);
        assert_eq!(urgency.urgency, SidebarStatus::Input.urgency());
        assert_eq!(urgency.attention, 2);
        assert_eq!(urgency.created_at_ms, 4_000);
        assert_eq!(urgency.id, 11);
        assert_eq!(
            StaticKey::of(&row),
            StaticKey {
                created_at_ms: 4_000,
                id: 11
            }
        );

        let mut parked = ViewBuilder::new(12).exited(0).last_activity_ms(8_000).build();
        assert_eq!(
            SettledKey::of(&parked),
            SettledKey {
                settled_at_ms: 8_000,
                id: 12
            }
        );
        parked.snooze = Some(crate::snooze::Snooze {
            snoozed_at_ms: 6_000,
            wake_at_ms: 60_000,
        });
        assert_eq!(SettledKey::of(&parked).settled_at_ms, 6_000);
    }
}
