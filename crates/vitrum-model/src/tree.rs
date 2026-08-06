//! The visible sidebar tree: which rows a keyboard or a click can actually
//! reach.
//!
//! Ported from T3 Code's `getVisibleSidebarThreadIds` (flatten the grouped tree,
//! skipping collapsed groups) and `getVisibleThreadsForProject` (show a preview
//! of a long project and always keep the active row reachable).
//!
//! Everything downstream keys off the flattened list: traversal steps through
//! it, shift-click ranges are defined over it, and selection is pruned against
//! it. Keeping one definition of "visible" is what stops a keyboard shortcut
//! from landing on a row the operator cannot see.

use vitrum_proto::{ProjectId, SessionId};

/// One project's row in the sidebar, with the sessions it would render.
///
/// `sessions` is already ordered by the caller, normally by [`crate::order`].
/// This type does not re-sort: ordering and visibility are separate decisions
/// and mixing them makes both untestable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectGroup {
    pub project_id: ProjectId,
    /// A collapsed group renders its header only. Its sessions are unreachable
    /// by keyboard and cannot be range-selected.
    pub collapsed: bool,
    pub sessions: Vec<SessionId>,
}

impl ProjectGroup {
    pub fn new(project_id: ProjectId, sessions: Vec<SessionId>) -> Self {
        ProjectGroup {
            project_id,
            collapsed: false,
            sessions,
        }
    }

    pub fn collapsed(mut self) -> Self {
        self.collapsed = true;
        self
    }
}

/// Flatten the grouped tree into the reachable session ids, in render order.
///
/// A collapsed group contributes nothing. Its sessions still exist and still
/// count towards its rollup; they are simply not on screen.
pub fn visible_session_ids(groups: &[ProjectGroup]) -> Vec<SessionId> {
    let capacity = groups
        .iter()
        .filter(|group| !group.collapsed)
        .map(|group| group.sessions.len())
        .sum();
    let mut visible = Vec::with_capacity(capacity);
    for group in groups {
        if group.collapsed {
            continue;
        }
        visible.extend_from_slice(&group.sessions);
    }
    visible
}

/// How a long project's session list is trimmed for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSplit {
    /// Rows to render, in the caller's order.
    pub visible: Vec<SessionId>,
    /// Rows behind the "show all" affordance.
    pub hidden: Vec<SessionId>,
}

impl PreviewSplit {
    /// True when there is anything behind the affordance.
    pub fn has_hidden(&self) -> bool {
        !self.hidden.is_empty()
    }
}

/// Trim one project's sessions to a preview, keeping the active row reachable.
///
/// Below the limit, or expanded, everything shows. Above it, the first
/// `preview_limit` show, plus the active session if it fell outside the
/// preview: a row you are looking at must never vanish from the sidebar because
/// it aged past the cut. The rescued row keeps its position in the caller's
/// order rather than being appended, so expanding the group does not make rows
/// jump.
pub fn preview_sessions(
    sessions: &[SessionId],
    active: Option<SessionId>,
    expanded: bool,
    preview_limit: usize,
) -> PreviewSplit {
    if expanded || sessions.len() <= preview_limit {
        return PreviewSplit {
            visible: sessions.to_vec(),
            hidden: Vec::new(),
        };
    }

    let rescued = active.filter(|active| sessions[preview_limit..].contains(active));

    let mut visible = Vec::with_capacity(preview_limit + usize::from(rescued.is_some()));
    let mut hidden = Vec::with_capacity(sessions.len() - preview_limit);
    for (index, session) in sessions.iter().enumerate() {
        if index < preview_limit || Some(*session) == rescued {
            visible.push(*session);
        } else {
            hidden.push(*session);
        }
    }
    PreviewSplit { visible, hidden }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[u64]) -> Vec<SessionId> {
        values.iter().copied().map(SessionId).collect()
    }

    fn group(project: u64, sessions: &[u64]) -> ProjectGroup {
        ProjectGroup::new(ProjectId(project), ids(sessions))
    }

    /// The flattened list is what every keyboard shortcut walks. Groups
    /// contribute in order and a collapsed group contributes nothing, or a
    /// shortcut lands on a row that is not on screen.
    #[test]
    fn collapsed_groups_contribute_nothing_to_the_visible_list() {
        let groups = vec![
            group(1, &[10, 11]),
            group(2, &[20, 21, 22]).collapsed(),
            group(3, &[30]),
        ];
        assert_eq!(visible_session_ids(&groups), ids(&[10, 11, 30]));
    }

    /// Every group collapsed means nothing is reachable, which the traversal
    /// and selection layers must handle rather than assume away.
    #[test]
    fn a_fully_collapsed_tree_is_empty() {
        let groups = vec![group(1, &[10]).collapsed(), group(2, &[20]).collapsed()];
        assert_eq!(visible_session_ids(&groups), Vec::new());
    }

    /// An expanded but empty group must not disturb the flattening, since a
    /// project with no sessions is a normal state right after creation.
    #[test]
    fn empty_groups_do_not_disturb_the_flattening() {
        let groups = vec![group(1, &[]), group(2, &[20]), group(3, &[])];
        assert_eq!(visible_session_ids(&groups), ids(&[20]));
        assert_eq!(visible_session_ids(&[]), Vec::new());
    }

    /// Group order is the caller's, and duplicates across groups are preserved
    /// rather than deduplicated. This layer reports what is rendered; it does
    /// not correct the caller's model.
    #[test]
    fn flattening_preserves_caller_order_exactly() {
        let groups = vec![group(9, &[3, 1, 2]), group(4, &[7])];
        assert_eq!(visible_session_ids(&groups), ids(&[3, 1, 2, 7]));
    }

    /// Below the limit nothing is hidden, and at exactly the limit nothing is
    /// hidden either. An off-by-one here shows a "show all" affordance that
    /// reveals nothing.
    #[test]
    fn a_short_project_is_never_trimmed() {
        for count in 0..=5u64 {
            let sessions = ids(&(0..count).collect::<Vec<_>>());
            let split = preview_sessions(&sessions, None, false, 5);
            assert_eq!(split.visible, sessions, "count {count}");
            assert_eq!(split.hidden, Vec::new(), "count {count}");
            assert!(!split.has_hidden());
        }
    }

    /// Past the limit the tail is hidden, and the split must be exact so the
    /// affordance can say how many rows are behind it.
    #[test]
    fn a_long_project_is_trimmed_to_the_preview_limit() {
        let sessions = ids(&[1, 2, 3, 4, 5, 6, 7]);
        let split = preview_sessions(&sessions, None, false, 3);
        assert_eq!(split.visible, ids(&[1, 2, 3]));
        assert_eq!(split.hidden, ids(&[4, 5, 6, 7]));
        assert!(split.has_hidden());
    }

    /// Expanding shows everything and hides nothing, which is the state the
    /// "show all" affordance switches into.
    #[test]
    fn an_expanded_project_shows_everything() {
        let sessions = ids(&[1, 2, 3, 4, 5]);
        let split = preview_sessions(&sessions, Some(SessionId(5)), true, 2);
        assert_eq!(split.visible, sessions);
        assert_eq!(split.hidden, Vec::new());
    }

    /// The row you are looking at must stay on screen. Without the rescue, a
    /// session drops out of the sidebar the moment three newer ones appear, and
    /// the pane you are typing into has no row.
    #[test]
    fn the_active_row_is_rescued_from_behind_the_cut() {
        let sessions = ids(&[1, 2, 3, 4, 5, 6]);
        let split = preview_sessions(&sessions, Some(SessionId(5)), false, 3);
        assert_eq!(split.visible, ids(&[1, 2, 3, 5]));
        assert_eq!(split.hidden, ids(&[4, 6]));
    }

    /// A rescued row keeps its place in the ordering rather than being appended
    /// to the preview, so expanding the group does not make rows jump around.
    #[test]
    fn a_rescued_row_keeps_its_position_in_the_ordering() {
        let sessions = ids(&[10, 20, 30, 40, 50]);
        let split = preview_sessions(&sessions, Some(SessionId(40)), false, 2);
        assert_eq!(split.visible, ids(&[10, 20, 40]));
        assert_eq!(split.hidden, ids(&[30, 50]));

        let expanded = preview_sessions(&sessions, Some(SessionId(40)), true, 2);
        let visible_positions: Vec<usize> = split
            .visible
            .iter()
            .map(|session| expanded.visible.iter().position(|other| other == session).unwrap())
            .collect();
        assert_eq!(visible_positions, vec![0, 1, 3], "rescued row keeps its rank");
    }

    /// An active row already inside the preview must not be duplicated, and an
    /// active row belonging to another project must not be conjured into this
    /// one.
    #[test]
    fn rescue_does_not_duplicate_or_invent_rows() {
        let sessions = ids(&[1, 2, 3, 4]);

        let inside = preview_sessions(&sessions, Some(SessionId(2)), false, 2);
        assert_eq!(inside.visible, ids(&[1, 2]));
        assert_eq!(inside.hidden, ids(&[3, 4]));

        let foreign = preview_sessions(&sessions, Some(SessionId(99)), false, 2);
        assert_eq!(foreign.visible, ids(&[1, 2]));
        assert_eq!(foreign.hidden, ids(&[3, 4]));
    }

    /// A zero preview limit hides everything except a rescued active row. A
    /// caller can legitimately configure this to collapse projects down to
    /// headers, and it must not panic on the empty preview slice.
    #[test]
    fn a_zero_preview_limit_hides_everything_but_the_active_row() {
        let sessions = ids(&[1, 2, 3]);
        let split = preview_sessions(&sessions, None, false, 0);
        assert_eq!(split.visible, Vec::new());
        assert_eq!(split.hidden, sessions);

        let with_active = preview_sessions(&sessions, Some(SessionId(3)), false, 0);
        assert_eq!(with_active.visible, ids(&[3]));
        assert_eq!(with_active.hidden, ids(&[1, 2]));
    }

    /// Visible and hidden must partition the input exactly: no row lost, no row
    /// counted twice. A lost row is a session you can never reach again.
    #[test]
    fn the_preview_split_partitions_the_input() {
        let sessions = ids(&[1, 2, 3, 4, 5, 6, 7, 8]);
        for limit in 0..=9 {
            for active in [None, Some(SessionId(1)), Some(SessionId(6)), Some(SessionId(8))] {
                for expanded in [false, true] {
                    let split = preview_sessions(&sessions, active, expanded, limit);
                    let mut recombined = split.visible.clone();
                    recombined.extend_from_slice(&split.hidden);
                    recombined.sort_unstable();
                    let mut expected = sessions.clone();
                    expected.sort_unstable();
                    assert_eq!(
                        recombined, expected,
                        "limit {limit} active {active:?} expanded {expanded}"
                    );
                }
            }
        }
    }
}
