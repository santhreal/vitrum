//! Keyboard traversal over the flattened visible tree.
//!
//! Ported from T3 Code's `resolveAdjacentThreadId`. Everything steps through the
//! list produced by [`crate::tree::visible_session_ids`], so a keypress can only
//! ever land on a row that is actually on screen: sessions inside a collapsed
//! group are skipped because they are not in the list at all.
//!
//! [`adjacent_matching`] is the piece T3 Code does not have, and it is what pays
//! for the deliberately static inbox order in [`crate::order`]. Rather than
//! reordering the list so the urgent row floats to the top, which moves
//! everything under the operator's cursor, we leave the list alone and give them
//! one key that jumps to the next row that wants them.

use vitrum_proto::SessionId;

/// Which way a step goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Previous,
    Next,
}

impl Direction {
    fn step(self, index: usize, len: usize) -> Option<usize> {
        match self {
            Direction::Previous => index.checked_sub(1),
            Direction::Next => (index + 1 < len).then_some(index + 1),
        }
    }

    fn entry(self, len: usize) -> Option<usize> {
        match self {
            Direction::Previous => len.checked_sub(1),
            Direction::Next => (len > 0).then_some(0),
        }
    }
}

/// What happens at the ends of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wrap {
    /// Stop at the ends and return `None`. T3 Code's behaviour: the list has a
    /// top and a bottom and holding the key down does not spin.
    #[default]
    Clamp,
    /// Continue from the other end.
    Around,
}

/// The session one step from `current`, or `None` if there is nowhere to go.
///
/// With no current session, a `Next` starts at the top and a `Previous` starts
/// at the bottom, so the first keypress after a fresh connect always lands
/// somewhere. A `current` that is not in the visible list yields `None` under
/// [`Wrap::Clamp`]: the selection is stale, and quietly jumping to the top would
/// move the operator somewhere they did not ask to go. Under [`Wrap::Around`] it
/// enters at the appropriate end, since wrapping already means "keep going".
pub fn adjacent(
    visible: &[SessionId],
    current: Option<SessionId>,
    direction: Direction,
    wrap: Wrap,
) -> Option<SessionId> {
    if visible.is_empty() {
        return None;
    }
    let Some(current) = current else {
        return direction.entry(visible.len()).map(|index| visible[index]);
    };
    let Some(index) = visible.iter().position(|session| *session == current) else {
        return match wrap {
            Wrap::Clamp => None,
            Wrap::Around => direction.entry(visible.len()).map(|index| visible[index]),
        };
    };
    match direction.step(index, visible.len()) {
        Some(next) => Some(visible[next]),
        None => match wrap {
            Wrap::Clamp => None,
            Wrap::Around => direction.entry(visible.len()).map(|index| visible[index]),
        },
    }
}

/// The next session in `direction` for which `matches` is true.
///
/// Never returns `current`, even under [`Wrap::Around`] and even when it is the
/// only match: "go to the next one that wants me" must move or report that there
/// is nowhere to move. Scans at most one full lap, so a list with no match
/// terminates rather than spinning.
///
/// With no current session the scan starts at the entry end and the entry row
/// itself is eligible, since there is no row to move away from.
pub fn adjacent_matching(
    visible: &[SessionId],
    current: Option<SessionId>,
    direction: Direction,
    wrap: Wrap,
    matches: impl Fn(SessionId) -> bool,
) -> Option<SessionId> {
    if visible.is_empty() {
        return None;
    }

    let start = match current {
        None => {
            let entry = direction.entry(visible.len())?;
            if matches(visible[entry]) {
                return Some(visible[entry]);
            }
            entry
        }
        Some(current) => match visible.iter().position(|session| *session == current) {
            Some(index) => index,
            None => match wrap {
                Wrap::Clamp => return None,
                Wrap::Around => {
                    let entry = direction.entry(visible.len())?;
                    if matches(visible[entry]) {
                        return Some(visible[entry]);
                    }
                    entry
                }
            },
        },
    };

    let mut index = start;
    for _ in 0..visible.len() {
        index = match direction.step(index, visible.len()) {
            Some(next) => next,
            None => match wrap {
                Wrap::Clamp => return None,
                Wrap::Around => direction.entry(visible.len())?,
            },
        };
        if index == start {
            return None;
        }
        if matches(visible[index]) {
            return Some(visible[index]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{ProjectGroup, visible_session_ids};
    use vitrum_proto::ProjectId;

    fn ids(values: &[u64]) -> Vec<SessionId> {
        values.iter().copied().map(SessionId).collect()
    }

    fn session(id: u64) -> SessionId {
        SessionId(id)
    }

    /// Plain stepping through the middle of the list, both directions. If this
    /// is wrong every arrow key in the sidebar is wrong.
    #[test]
    fn stepping_moves_one_row_in_each_direction() {
        let visible = ids(&[1, 2, 3]);
        assert_eq!(
            adjacent(&visible, Some(session(2)), Direction::Next, Wrap::Clamp),
            Some(session(3))
        );
        assert_eq!(
            adjacent(&visible, Some(session(2)), Direction::Previous, Wrap::Clamp),
            Some(session(1))
        );
    }

    /// Clamping stops at both ends. Holding the down arrow at the bottom must
    /// do nothing rather than teleport to the top.
    #[test]
    fn clamping_stops_at_both_ends() {
        let visible = ids(&[1, 2, 3]);
        assert_eq!(
            adjacent(&visible, Some(session(3)), Direction::Next, Wrap::Clamp),
            None
        );
        assert_eq!(
            adjacent(&visible, Some(session(1)), Direction::Previous, Wrap::Clamp),
            None
        );
    }

    /// Wrapping continues from the other end, at both ends. An implementation
    /// that wraps forwards but clamps backwards is a common half-done job.
    #[test]
    fn wrapping_continues_from_the_other_end_at_both_ends() {
        let visible = ids(&[1, 2, 3]);
        assert_eq!(
            adjacent(&visible, Some(session(3)), Direction::Next, Wrap::Around),
            Some(session(1))
        );
        assert_eq!(
            adjacent(&visible, Some(session(1)), Direction::Previous, Wrap::Around),
            Some(session(3))
        );
    }

    /// A single-row list wraps onto itself rather than reporting nowhere to go,
    /// which keeps the key responsive instead of looking broken.
    #[test]
    fn a_single_row_list_wraps_onto_itself_and_clamps_otherwise() {
        let visible = ids(&[7]);
        for direction in [Direction::Next, Direction::Previous] {
            assert_eq!(
                adjacent(&visible, Some(session(7)), direction, Wrap::Around),
                Some(session(7))
            );
            assert_eq!(
                adjacent(&visible, Some(session(7)), direction, Wrap::Clamp),
                None
            );
        }
    }

    /// With nothing selected, the first keypress must land somewhere sensible:
    /// down goes to the top of the list, up goes to the bottom.
    #[test]
    fn no_current_selection_enters_at_the_matching_end() {
        let visible = ids(&[1, 2, 3]);
        assert_eq!(
            adjacent(&visible, None, Direction::Next, Wrap::Clamp),
            Some(session(1))
        );
        assert_eq!(
            adjacent(&visible, None, Direction::Previous, Wrap::Clamp),
            Some(session(3))
        );
    }

    /// A selection that has vanished (its session closed, or its group
    /// collapsed) must not silently teleport the operator to the top under
    /// clamping. Under wrapping it enters at the end, since wrapping already
    /// means "keep going".
    #[test]
    fn a_current_session_missing_from_the_visible_list_is_handled_per_wrap_mode() {
        let visible = ids(&[1, 2, 3]);
        assert_eq!(
            adjacent(&visible, Some(session(99)), Direction::Next, Wrap::Clamp),
            None
        );
        assert_eq!(
            adjacent(&visible, Some(session(99)), Direction::Next, Wrap::Around),
            Some(session(1))
        );
        assert_eq!(
            adjacent(&visible, Some(session(99)), Direction::Previous, Wrap::Around),
            Some(session(3))
        );
    }

    /// An empty list has nowhere to go under every combination. The sidebar
    /// shows this on first connect and while a filter matches nothing.
    #[test]
    fn an_empty_list_never_yields_a_target() {
        for direction in [Direction::Next, Direction::Previous] {
            for wrap in [Wrap::Clamp, Wrap::Around] {
                assert_eq!(adjacent(&[], None, direction, wrap), None);
                assert_eq!(adjacent(&[], Some(session(1)), direction, wrap), None);
                assert_eq!(
                    adjacent_matching(&[], None, direction, wrap, |_| true),
                    None
                );
            }
        }
    }

    /// Traversal runs over the flattened tree, so a collapsed group's sessions
    /// are skipped entirely: stepping goes straight from the row before the
    /// group to the row after it.
    #[test]
    fn traversal_skips_the_sessions_inside_a_collapsed_group() {
        let groups = vec![
            ProjectGroup::new(ProjectId(1), ids(&[10, 11])),
            ProjectGroup::new(ProjectId(2), ids(&[20, 21, 22])).collapsed(),
            ProjectGroup::new(ProjectId(3), ids(&[30])),
        ];
        let visible = visible_session_ids(&groups);
        assert_eq!(visible, ids(&[10, 11, 30]));

        assert_eq!(
            adjacent(&visible, Some(session(11)), Direction::Next, Wrap::Clamp),
            Some(session(30)),
            "must jump over the collapsed group"
        );
        assert_eq!(
            adjacent(&visible, Some(session(30)), Direction::Previous, Wrap::Clamp),
            Some(session(11))
        );
        assert_eq!(
            adjacent(&visible, Some(session(20)), Direction::Next, Wrap::Clamp),
            None,
            "a row inside the collapsed group is not a valid position"
        );
    }

    /// Expanding a group makes its rows reachable again, which is the other half
    /// of the same contract.
    #[test]
    fn expanding_a_group_makes_its_rows_reachable() {
        let mut groups = vec![
            ProjectGroup::new(ProjectId(1), ids(&[10])),
            ProjectGroup::new(ProjectId(2), ids(&[20, 21])).collapsed(),
        ];
        assert_eq!(
            adjacent(
                &visible_session_ids(&groups),
                Some(session(10)),
                Direction::Next,
                Wrap::Clamp
            ),
            None
        );

        groups[1].collapsed = false;
        assert_eq!(
            adjacent(
                &visible_session_ids(&groups),
                Some(session(10)),
                Direction::Next,
                Wrap::Clamp
            ),
            Some(session(20))
        );
    }

    /// The jump-to-urgent key: skip past everything that does not match and land
    /// on the next row that does. This is what lets the inbox order stay static.
    #[test]
    fn matching_traversal_skips_non_matching_rows() {
        let visible = ids(&[1, 2, 3, 4, 5]);
        let wants_me = |session: SessionId| session.0 == 4;
        assert_eq!(
            adjacent_matching(&visible, Some(session(1)), Direction::Next, Wrap::Clamp, wants_me),
            Some(session(4))
        );
        assert_eq!(
            adjacent_matching(
                &visible,
                Some(session(5)),
                Direction::Previous,
                Wrap::Clamp,
                wants_me
            ),
            Some(session(4))
        );
    }

    /// No match means no move, in both wrap modes. Under wrapping this is also
    /// the termination guarantee: the scan must complete a lap and stop rather
    /// than spin.
    #[test]
    fn matching_traversal_terminates_when_nothing_matches() {
        let visible = ids(&[1, 2, 3, 4, 5]);
        for wrap in [Wrap::Clamp, Wrap::Around] {
            assert_eq!(
                adjacent_matching(&visible, Some(session(3)), Direction::Next, wrap, |_| false),
                None
            );
            assert_eq!(
                adjacent_matching(&visible, None, Direction::Next, wrap, |_| false),
                None
            );
        }
    }

    /// The current row is never the answer, even when it is the only match.
    /// "Next one that wants me" must move or report that it cannot.
    #[test]
    fn matching_traversal_never_returns_the_current_row() {
        let visible = ids(&[1, 2, 3]);
        let only_two = |session: SessionId| session.0 == 2;
        assert_eq!(
            adjacent_matching(
                &visible,
                Some(session(2)),
                Direction::Next,
                Wrap::Around,
                only_two
            ),
            None
        );
        assert_eq!(
            adjacent_matching(
                &visible,
                Some(session(2)),
                Direction::Previous,
                Wrap::Around,
                only_two
            ),
            None
        );
    }

    /// Wrapping finds a match behind the current row. Cycling repeatedly must
    /// visit every match and come back around, which is how a "next urgent" key
    /// is actually used when three rows want you.
    #[test]
    fn matching_traversal_wraps_and_cycles_through_every_match() {
        let visible = ids(&[1, 2, 3, 4, 5, 6]);
        let wants_me = |session: SessionId| matches!(session.0, 2 | 5);

        assert_eq!(
            adjacent_matching(&visible, Some(session(5)), Direction::Next, Wrap::Around, wants_me),
            Some(session(2)),
            "wraps past the end to the earlier match"
        );

        let mut visited = Vec::new();
        let mut cursor = Some(session(1));
        for _ in 0..4 {
            cursor = adjacent_matching(&visible, cursor, Direction::Next, Wrap::Around, wants_me);
            visited.push(cursor.expect("a match always exists here").0);
        }
        assert_eq!(visited, vec![2, 5, 2, 5]);
    }

    /// With nothing selected the entry row itself is eligible, since there is no
    /// row to move away from. Otherwise pressing the key on a fresh connect
    /// would skip a matching first row.
    #[test]
    fn matching_traversal_with_no_selection_can_return_the_entry_row() {
        let visible = ids(&[1, 2, 3]);
        assert_eq!(
            adjacent_matching(&visible, None, Direction::Next, Wrap::Clamp, |session| session.0
                == 1),
            Some(session(1))
        );
        assert_eq!(
            adjacent_matching(&visible, None, Direction::Previous, Wrap::Clamp, |session| {
                session.0 == 3
            }),
            Some(session(3))
        );
        assert_eq!(
            adjacent_matching(&visible, None, Direction::Next, Wrap::Clamp, |session| session.0
                == 3),
            Some(session(3))
        );
    }

    /// A stale current row under clamping yields nothing, matching `adjacent`.
    /// The two must agree or the two keys behave differently on the same list.
    #[test]
    fn matching_traversal_agrees_with_plain_traversal_on_a_stale_selection() {
        let visible = ids(&[1, 2, 3]);
        assert_eq!(
            adjacent_matching(&visible, Some(session(99)), Direction::Next, Wrap::Clamp, |_| true),
            None
        );
        assert_eq!(
            adjacent_matching(&visible, Some(session(99)), Direction::Next, Wrap::Around, |_| true),
            Some(session(1))
        );
    }

    /// Every row matching makes `adjacent_matching` behave exactly like
    /// `adjacent`. Two implementations of stepping must not disagree.
    #[test]
    fn matching_traversal_degenerates_to_plain_traversal_when_everything_matches() {
        let visible = ids(&[1, 2, 3, 4]);
        for direction in [Direction::Next, Direction::Previous] {
            for wrap in [Wrap::Clamp, Wrap::Around] {
                for current in [None, Some(session(1)), Some(session(2)), Some(session(4))] {
                    assert_eq!(
                        adjacent_matching(&visible, current, direction, wrap, |_| true),
                        adjacent(&visible, current, direction, wrap),
                        "{direction:?} {wrap:?} {current:?}"
                    );
                }
            }
        }
    }
}
