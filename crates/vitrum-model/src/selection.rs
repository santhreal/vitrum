//! Multi-select: an anchored selection set and the queries a context menu asks.
//!
//! Ported from T3 Code's selection handling and
//! `buildMultiSelectThreadContextMenuItems`. Ranges are defined over the
//! flattened visible list from [`crate::tree`], which is the same list traversal
//! walks, so a shift-click range can never include a row the operator cannot
//! see.
//!
//! The anchor is what makes shift-click feel right. Every platform's file
//! manager works this way: a plain click sets the anchor, a shift-click selects
//! from the anchor to the click without moving the anchor, so widening and
//! narrowing a range by repeated shift-clicks pivots around the row you started
//! on rather than the row you touched last.

use std::collections::BTreeSet;

use vitrum_proto::SessionId;
use serde::{Deserialize, Serialize};

use crate::disposition::Disposition;
use crate::view::{Clock, SessionView};

/// An anchored multi-selection.
///
/// Persisted with the window layout, hence the serde derives: closing and
/// reopening the app with three rows selected should not silently drop them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    selected: BTreeSet<SessionId>,
    anchor: Option<SessionId>,
    lead: Option<SessionId>,
}

impl Selection {
    /// An empty selection.
    pub fn new() -> Self {
        Selection::default()
    }

    /// A selection of exactly one row, anchored on it.
    pub fn single(session: SessionId) -> Self {
        let mut selection = Selection::new();
        selection.select_one(session);
        selection
    }

    /// Plain click: replace the selection with one row and re-anchor there.
    pub fn select_one(&mut self, session: SessionId) {
        self.selected.clear();
        self.selected.insert(session);
        self.anchor = Some(session);
        self.lead = Some(session);
    }

    /// Modifier click: add or remove one row and move the anchor to it.
    ///
    /// The anchor moves even when the row is being deselected, matching every
    /// file manager: a subsequent shift-click ranges from the row you last
    /// touched.
    pub fn toggle(&mut self, session: SessionId) {
        if !self.selected.remove(&session) {
            self.selected.insert(session);
        }
        self.anchor = Some(session);
        self.lead = Some(session);
    }

    /// Shift-click: select the inclusive range from the anchor to `session`,
    /// replacing whatever was selected.
    ///
    /// Falls back to a plain selection when there is no anchor, or when either
    /// end is not on screen. Ranging to an invisible row would select rows the
    /// operator cannot see, which is how bulk actions delete the wrong things.
    /// The anchor is left where it was, so shift-clicking again from the same
    /// anchor narrows or widens the same range.
    pub fn extend_to(&mut self, visible: &[SessionId], session: SessionId) {
        let Some((low, high)) = self.range_indices(visible, session) else {
            self.select_one(session);
            return;
        };
        self.selected = visible[low..=high].iter().copied().collect();
        self.lead = Some(session);
    }

    /// Ctrl-shift-click: union the anchored range into the existing selection.
    pub fn extend_to_additive(&mut self, visible: &[SessionId], session: SessionId) {
        let Some((low, high)) = self.range_indices(visible, session) else {
            self.toggle(session);
            return;
        };
        self.selected.extend(visible[low..=high].iter().copied());
        self.lead = Some(session);
    }

    /// Select every visible row, anchoring at the top and leading at the bottom.
    pub fn select_all(&mut self, visible: &[SessionId]) {
        self.selected = visible.iter().copied().collect();
        self.anchor = visible.first().copied();
        self.lead = visible.last().copied();
    }

    /// Drop everything, including the anchor.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.lead = None;
    }

    /// Drop selected rows that are no longer on screen.
    ///
    /// Called after the visible list changes. A selection holding closed
    /// sessions would make a bulk action operate on rows that no longer exist,
    /// and a count in a menu label that does not match what the operator can see
    /// is worse than useless.
    pub fn retain_visible(&mut self, visible: &[SessionId]) {
        let on_screen: BTreeSet<SessionId> = visible.iter().copied().collect();
        self.selected.retain(|session| on_screen.contains(session));
        if self.anchor.is_some_and(|anchor| !on_screen.contains(&anchor)) {
            self.anchor = None;
        }
        if self.lead.is_some_and(|lead| !on_screen.contains(&lead)) {
            self.lead = None;
        }
    }

    pub fn contains(&self, session: SessionId) -> bool {
        self.selected.contains(&session)
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// The row a shift-click ranges from.
    pub fn anchor(&self) -> Option<SessionId> {
        self.anchor
    }

    /// The row last touched, which is where the focus ring sits.
    pub fn lead(&self) -> Option<SessionId> {
        self.lead
    }

    /// Selected ids in id order. Use [`Selection::ordered`] for screen order.
    pub fn iter(&self) -> impl Iterator<Item = SessionId> + '_ {
        self.selected.iter().copied()
    }

    /// Selected ids in the order they appear on screen, which is the order a
    /// bulk action should apply in and the order a confirmation should list.
    pub fn ordered(&self, visible: &[SessionId]) -> Vec<SessionId> {
        visible
            .iter()
            .copied()
            .filter(|session| self.selected.contains(session))
            .collect()
    }

    fn range_indices(
        &self,
        visible: &[SessionId],
        session: SessionId,
    ) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let from = visible.iter().position(|other| *other == anchor)?;
        let to = visible.iter().position(|other| *other == session)?;
        let (low, high) = if from <= to { (from, to) } else { (to, from) };
        Some((low, high))
    }
}

/// What the selected rows are, in aggregate.
///
/// Computed once and handed to [`context_menu`] so the menu builder stays a pure
/// function of a handful of counts rather than of the whole session list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectionFacts {
    pub count: usize,
    /// Rows with output the operator has not seen.
    pub unread: usize,
    /// Rows currently parked.
    pub snoozed: usize,
    /// Rows whose child process is still alive.
    pub live: usize,
    /// Rows the operator may park.
    pub snoozable: usize,
    /// Rows the operator may drain.
    pub settleable: usize,
}

impl SelectionFacts {
    /// Summarise the selected rows.
    ///
    /// Rows in `selection` that are absent from `rows` are ignored rather than
    /// counted as zero-valued, so a stale selection cannot inflate `count` past
    /// what is really there.
    pub fn collect(
        selection: &Selection,
        rows: &[SessionView],
        clock: Clock,
        policy: crate::disposition::DispositionPolicy,
    ) -> Self {
        let mut facts = SelectionFacts::default();
        for row in rows {
            if !selection.contains(row.id()) {
                continue;
            }
            facts.count += 1;
            if row.info.unread {
                facts.unread += 1;
            }
            if row.disposition(clock, policy) == Disposition::Snoozed {
                facts.snoozed += 1;
            }
            if row.info.status.is_live() {
                facts.live += 1;
            }
            if row.can_snooze() {
                facts.snoozable += 1;
            }
            if row.can_settle() {
                facts.settleable += 1;
            }
        }
        facts
    }
}

/// An action a context menu can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuAction {
    MarkRead,
    MarkUnread,
    Snooze,
    Wake,
    Settle,
    Unsettle,
    Close,
}

/// One rendered menu row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub action: MenuAction,
    pub label: String,
    /// Shown but not selectable. Disabled beats hidden for actions that are
    /// sometimes available: a menu whose rows move around is a menu you cannot
    /// build muscle memory for.
    pub disabled: bool,
    /// Wants a confirmation and a red treatment.
    pub destructive: bool,
}

/// Build the context menu for the current selection.
///
/// Every label carries the count, following T3 Code, because a bulk action with
/// no visible count is how you close nineteen sessions meaning to close one.
/// An empty selection produces an empty menu rather than a menu of disabled
/// rows: there is nothing to act on, so there is nothing to show.
pub fn context_menu(facts: SelectionFacts) -> Vec<MenuItem> {
    if facts.count == 0 {
        return Vec::new();
    }
    let count = facts.count;

    let mut items = Vec::with_capacity(5);

    if facts.unread == count {
        items.push(MenuItem {
            action: MenuAction::MarkRead,
            label: format!("Mark read ({count})"),
            disabled: false,
            destructive: false,
        });
    } else {
        items.push(MenuItem {
            action: MenuAction::MarkUnread,
            label: format!("Mark unread ({count})"),
            disabled: false,
            destructive: false,
        });
    }

    if facts.snoozed == count {
        items.push(MenuItem {
            action: MenuAction::Wake,
            label: format!("Wake ({count})"),
            disabled: false,
            destructive: false,
        });
    } else {
        items.push(MenuItem {
            action: MenuAction::Snooze,
            label: format!("Snooze ({count})"),
            // Refused outright when nothing in the selection may be parked,
            // rather than silently parking the subset that can: a bulk action
            // that half-applies is impossible to reason about.
            disabled: facts.snoozable < count,
            destructive: false,
        });
    }

    items.push(MenuItem {
        action: MenuAction::Settle,
        label: format!("Settle ({count})"),
        disabled: facts.settleable < count,
        destructive: false,
    });

    items.push(MenuItem {
        action: MenuAction::Close,
        label: if facts.live > 0 {
            format!("Close ({count}, {} running)", facts.live)
        } else {
            format!("Close ({count})")
        },
        disabled: false,
        destructive: true,
    });

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disposition::DispositionPolicy;
    use crate::testkit::ViewBuilder;
    use vitrum_proto::HintState;

    const NOW: u64 = 1_772_580_600_000;
    const HOUR: u64 = 3_600_000;

    fn ids(values: &[u64]) -> Vec<SessionId> {
        values.iter().copied().map(SessionId).collect()
    }

    fn session(id: u64) -> SessionId {
        SessionId(id)
    }

    fn clock() -> Clock {
        Clock::utc(NOW)
    }

    fn policy() -> DispositionPolicy {
        DispositionPolicy::manual()
    }

    /// A plain click replaces everything and re-anchors, which is the baseline
    /// every other gesture is defined against.
    #[test]
    fn a_plain_click_replaces_the_selection_and_moves_the_anchor() {
        let mut selection = Selection::new();
        selection.select_one(session(1));
        assert_eq!(selection.ordered(&ids(&[1, 2, 3])), ids(&[1]));
        assert_eq!(selection.anchor(), Some(session(1)));

        selection.select_one(session(3));
        assert_eq!(selection.ordered(&ids(&[1, 2, 3])), ids(&[3]));
        assert_eq!(selection.anchor(), Some(session(3)));
        assert_eq!(selection.len(), 1);
    }

    /// Toggling adds and removes, and moves the anchor either way so the next
    /// shift-click ranges from the row last touched.
    #[test]
    fn toggling_adds_removes_and_always_moves_the_anchor() {
        let mut selection = Selection::new();
        selection.toggle(session(2));
        assert!(selection.contains(session(2)));
        assert_eq!(selection.anchor(), Some(session(2)));

        selection.toggle(session(4));
        assert_eq!(selection.len(), 2);
        assert_eq!(selection.anchor(), Some(session(4)));

        selection.toggle(session(2));
        assert!(!selection.contains(session(2)));
        assert_eq!(selection.len(), 1);
        assert_eq!(
            selection.anchor(),
            Some(session(2)),
            "the anchor follows a deselect too"
        );
    }

    /// Range extension selects the inclusive span in visible order, in both
    /// directions. Getting the endpoints exclusive is the classic bug and it
    /// silently drops the row the operator clicked.
    #[test]
    fn range_extension_is_inclusive_in_both_directions() {
        let visible = ids(&[1, 2, 3, 4, 5]);

        let mut downward = Selection::new();
        downward.select_one(session(2));
        downward.extend_to(&visible, session(4));
        assert_eq!(downward.ordered(&visible), ids(&[2, 3, 4]));

        let mut upward = Selection::new();
        upward.select_one(session(4));
        upward.extend_to(&visible, session(2));
        assert_eq!(upward.ordered(&visible), ids(&[2, 3, 4]));
    }

    /// The anchor does not move on a shift-click, so repeated shift-clicks
    /// pivot around the original row. Widening then narrowing must land back on
    /// the smaller range, not accumulate.
    #[test]
    fn repeated_range_extension_pivots_on_the_unmoved_anchor() {
        let visible = ids(&[1, 2, 3, 4, 5]);
        let mut selection = Selection::new();
        selection.select_one(session(3));

        selection.extend_to(&visible, session(5));
        assert_eq!(selection.ordered(&visible), ids(&[3, 4, 5]));
        assert_eq!(selection.anchor(), Some(session(3)));
        assert_eq!(selection.lead(), Some(session(5)));

        selection.extend_to(&visible, session(4));
        assert_eq!(
            selection.ordered(&visible),
            ids(&[3, 4]),
            "narrowing must not keep the previously selected row"
        );

        selection.extend_to(&visible, session(1));
        assert_eq!(
            selection.ordered(&visible),
            ids(&[1, 2, 3]),
            "crossing the anchor flips the range"
        );
        assert_eq!(selection.anchor(), Some(session(3)));
    }

    /// A range with no anchor degenerates to a plain click rather than
    /// selecting nothing, so the first shift-click after a clear still does
    /// something sensible.
    #[test]
    fn range_extension_without_an_anchor_selects_one_row() {
        let visible = ids(&[1, 2, 3]);
        let mut selection = Selection::new();
        selection.extend_to(&visible, session(2));
        assert_eq!(selection.ordered(&visible), ids(&[2]));
        assert_eq!(selection.anchor(), Some(session(2)));
    }

    /// Ranging to a row that is not on screen must not select invisible rows.
    /// A bulk action over rows the operator cannot see is how the wrong things
    /// get closed.
    #[test]
    fn range_extension_refuses_endpoints_that_are_not_visible() {
        let visible = ids(&[1, 2, 3]);

        let mut to_hidden = Selection::new();
        to_hidden.select_one(session(1));
        to_hidden.extend_to(&visible, session(99));
        assert_eq!(to_hidden.iter().collect::<Vec<_>>(), ids(&[99]));
        assert_eq!(to_hidden.ordered(&visible), Vec::new());

        let mut from_hidden = Selection::new();
        from_hidden.select_one(session(99));
        from_hidden.extend_to(&visible, session(2));
        assert_eq!(
            from_hidden.ordered(&visible),
            ids(&[2]),
            "an off-screen anchor cannot define a range"
        );
    }

    /// Additive extension unions rather than replaces, which is the
    /// ctrl-shift-click gesture: build a selection out of several ranges.
    #[test]
    fn additive_range_extension_unions_with_the_existing_selection() {
        let visible = ids(&[1, 2, 3, 4, 5, 6]);
        let mut selection = Selection::new();
        selection.select_one(session(1));
        selection.extend_to(&visible, session(2));
        assert_eq!(selection.ordered(&visible), ids(&[1, 2]));

        selection.toggle(session(5));
        selection.extend_to_additive(&visible, session(6));
        assert_eq!(selection.ordered(&visible), ids(&[1, 2, 5, 6]));
    }

    /// Selecting everything anchors at the top and leads at the bottom, so a
    /// following shift-click narrows from the top rather than doing nothing.
    #[test]
    fn select_all_anchors_at_the_top_and_leads_at_the_bottom() {
        let visible = ids(&[4, 7, 9]);
        let mut selection = Selection::new();
        selection.select_all(&visible);
        assert_eq!(selection.ordered(&visible), visible);
        assert_eq!(selection.anchor(), Some(session(4)));
        assert_eq!(selection.lead(), Some(session(9)));

        selection.extend_to(&visible, session(7));
        assert_eq!(selection.ordered(&visible), ids(&[4, 7]));
    }

    /// Closed sessions must drop out of the selection, and an anchor that
    /// vanished must be forgotten. Otherwise a menu says "Close (3)" for two
    /// rows, or a shift-click ranges from a row that no longer exists.
    #[test]
    fn pruning_drops_rows_and_anchors_that_left_the_screen() {
        let mut selection = Selection::new();
        selection.select_all(&ids(&[1, 2, 3, 4]));
        assert_eq!(selection.len(), 4);

        selection.retain_visible(&ids(&[2, 3]));
        assert_eq!(selection.ordered(&ids(&[2, 3])), ids(&[2, 3]));
        assert_eq!(selection.len(), 2);
        assert_eq!(selection.anchor(), None, "the anchor was row 1, now gone");
        assert_eq!(selection.lead(), None, "the lead was row 4, now gone");

        selection.retain_visible(&ids(&[3]));
        assert_eq!(selection.len(), 1);
        selection.retain_visible(&[]);
        assert!(selection.is_empty());
    }

    /// Pruning must keep an anchor that is still on screen, or every collapse
    /// of an unrelated group would silently reset the operator's pivot.
    #[test]
    fn pruning_keeps_an_anchor_that_is_still_visible() {
        let mut selection = Selection::new();
        selection.select_one(session(2));
        selection.extend_to(&ids(&[1, 2, 3]), session(3));
        selection.retain_visible(&ids(&[2, 3]));
        assert_eq!(selection.anchor(), Some(session(2)));
        assert_eq!(selection.lead(), Some(session(3)));
    }

    /// Screen order is not id order. A bulk action applies top to bottom, so
    /// `ordered` must follow the visible list and not the underlying set.
    #[test]
    fn ordered_follows_screen_order_not_id_order() {
        let visible = ids(&[30, 10, 20]);
        let mut selection = Selection::new();
        selection.select_all(&visible);
        assert_eq!(selection.ordered(&visible), ids(&[30, 10, 20]));
        assert_eq!(selection.iter().collect::<Vec<_>>(), ids(&[10, 20, 30]));
    }

    /// Clearing forgets the anchor too. Leaving it behind would make the next
    /// shift-click select a range from a row that is no longer highlighted.
    #[test]
    fn clearing_forgets_the_anchor() {
        let mut selection = Selection::single(session(5));
        assert_eq!(selection.anchor(), Some(session(5)));
        selection.clear();
        assert!(selection.is_empty());
        assert_eq!(selection.anchor(), None);
        assert_eq!(selection.lead(), None);
    }

    /// An empty selection produces no menu at all. A menu of disabled rows on a
    /// right-click over blank space is noise.
    #[test]
    fn an_empty_selection_produces_no_menu() {
        assert_eq!(context_menu(SelectionFacts::default()), Vec::new());
    }

    /// Every label carries the count. This is the guard against closing
    /// nineteen sessions when you meant one, and the running count on Close is
    /// the extra warning for the destructive action.
    #[test]
    fn every_menu_label_carries_the_selection_count() {
        let facts = SelectionFacts {
            count: 3,
            unread: 1,
            snoozed: 0,
            live: 2,
            snoozable: 3,
            settleable: 3,
        };
        let menu = context_menu(facts);
        assert_eq!(
            menu.iter().map(|item| item.label.as_str()).collect::<Vec<_>>(),
            vec![
                "Mark unread (3)",
                "Snooze (3)",
                "Settle (3)",
                "Close (3, 2 running)",
            ]
        );
        assert_eq!(
            menu.iter().map(|item| item.action).collect::<Vec<_>>(),
            vec![
                MenuAction::MarkUnread,
                MenuAction::Snooze,
                MenuAction::Settle,
                MenuAction::Close,
            ]
        );
        assert!(menu.iter().all(|item| !item.disabled));
        assert_eq!(
            menu.iter().filter(|item| item.destructive).count(),
            1,
            "only Close is destructive"
        );
    }

    /// The menu flips to the inverse action when the whole selection is already
    /// in that state, rather than offering a no-op.
    #[test]
    fn the_menu_offers_the_inverse_action_when_the_whole_selection_matches() {
        let all_unread = SelectionFacts {
            count: 2,
            unread: 2,
            snoozed: 2,
            live: 0,
            snoozable: 2,
            settleable: 2,
        };
        let menu = context_menu(all_unread);
        assert_eq!(
            menu.iter().map(|item| item.action).collect::<Vec<_>>(),
            vec![
                MenuAction::MarkRead,
                MenuAction::Wake,
                MenuAction::Settle,
                MenuAction::Close,
            ]
        );
        assert_eq!(menu[0].label, "Mark read (2)");
        assert_eq!(menu[1].label, "Wake (2)");
        assert_eq!(menu[3].label, "Close (2)", "no running suffix when none are live");
    }

    /// A partially-eligible bulk action is disabled rather than half-applied.
    /// Snoozing three rows where one is blocked on the operator would silently
    /// hide two and leave one, which nobody can reason about.
    #[test]
    fn a_bulk_action_is_disabled_when_it_cannot_apply_to_the_whole_selection() {
        let mixed = SelectionFacts {
            count: 3,
            unread: 0,
            snoozed: 0,
            live: 3,
            snoozable: 2,
            settleable: 1,
        };
        let menu = context_menu(mixed);
        let snooze = menu.iter().find(|item| item.action == MenuAction::Snooze).unwrap();
        assert!(snooze.disabled);
        assert_eq!(snooze.label, "Snooze (3)");

        let settle = menu.iter().find(|item| item.action == MenuAction::Settle).unwrap();
        assert!(settle.disabled);

        let close = menu.iter().find(|item| item.action == MenuAction::Close).unwrap();
        assert!(!close.disabled, "closing is always permitted");
    }

    /// Facts must be gathered from the real rows, including the guards, so the
    /// menu's disabled states track the disposition rules rather than
    /// duplicating them.
    #[test]
    fn facts_are_gathered_from_the_real_rows() {
        let rows = vec![
            ViewBuilder::new(1).running().waiting(Some(false)).unread(true).build(),
            ViewBuilder::new(2)
                .running()
                .waiting(Some(true))
                .hint(HintState::Approval, None, NOW)
                .build(),
            ViewBuilder::new(3)
                .running()
                .waiting(Some(false))
                .last_activity_ms(NOW - HOUR)
                .snooze(NOW - HOUR, NOW + HOUR)
                .build(),
            ViewBuilder::new(4).exited(0).last_visited_ms(Some(NOW)).build(),
        ];
        let mut selection = Selection::new();
        selection.select_all(&ids(&[1, 2, 3, 4]));

        let facts = SelectionFacts::collect(&selection, &rows, clock(), policy());
        assert_eq!(
            facts,
            SelectionFacts {
                count: 4,
                unread: 1,
                snoozed: 1,
                live: 3,
                // Row 2 is blocked on the operator, so it can be neither parked
                // nor drained. Rows 1 and 3 are mid-turn, so only the exited
                // row 4 can be drained.
                snoozable: 3,
                settleable: 1,
            }
        );

        let menu = context_menu(facts);
        assert!(menu.iter().find(|item| item.action == MenuAction::Snooze).unwrap().disabled);
        assert_eq!(
            menu.iter().find(|item| item.action == MenuAction::Close).unwrap().label,
            "Close (4, 3 running)"
        );
    }

    /// A selection holding ids that are not in the row list must not inflate the
    /// count. The menu would then promise an action over rows that do not exist.
    #[test]
    fn facts_ignore_selected_ids_with_no_matching_row() {
        let rows = vec![ViewBuilder::new(1).running().waiting(Some(false)).build()];
        let mut selection = Selection::new();
        selection.select_all(&ids(&[1, 2, 3]));
        assert_eq!(selection.len(), 3);

        let facts = SelectionFacts::collect(&selection, &rows, clock(), policy());
        assert_eq!(facts.count, 1);
        assert_eq!(context_menu(facts)[0].label, "Mark unread (1)");
    }

    /// The selection is persisted with the window layout, so it has to survive
    /// JSON with its anchor intact. Losing the anchor on restart makes the first
    /// shift-click after reopening behave differently from every later one.
    #[test]
    fn a_selection_round_trips_through_json_with_its_anchor() {
        let visible = ids(&[1, 2, 3, 4]);
        let mut selection = Selection::new();
        selection.select_one(session(2));
        selection.extend_to(&visible, session(4));

        let json = serde_json::to_string(&selection).expect("selection serialises");
        let back: Selection = serde_json::from_str(&json).expect("selection round-trips");
        assert_eq!(back, selection);
        assert_eq!(back.anchor(), Some(session(2)));
        assert_eq!(back.lead(), Some(session(4)));
        assert_eq!(back.ordered(&visible), ids(&[2, 3, 4]));
    }
}
