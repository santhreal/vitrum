use super::*;
use crate::testkit::{HOUR, NOW, info, project, row};
use vitrum_model::SidebarStatus;
use vitrum_proto::HintState;

fn clock() -> Clock {
    Clock::utc(NOW)
}

/// Fold a message at a fixed instant.
///
/// The fold takes `now_ms` so a test can put the clock exactly on a wake
/// boundary; almost none of them care, so they go through this.
fn apply(st: &mut UiState, msg: ServerMsg) -> Reaction {
    st.apply(msg, NOW)
}

/// `(id, project, offset)`, where the offset is milliseconds after a base
/// an hour before [`NOW`]. Rows come out Working and in the inbox, which is
/// the resting state everything else is measured against.
///
/// Every project gets its OWN root. The fixture used to name them all `p`,
/// so `with(&[1, 2])` built two projects rooted at one directory — which is
/// exactly the daemon defect [`inbox::coalesce_projects`] now folds away,
/// and a fixture that produces it is a fixture testing the bug.
fn with(projects: &[u64], sessions: &[(u64, u64, u64)]) -> UiState {
    let mut st = UiState::default();
    st.daemon.projects = projects
        .iter()
        .map(|p| project(*p, &format!("p{p}")))
        .collect();
    st.daemon.sessions = sessions
        .iter()
        .map(|(id, pid, at)| {
            row(*id)
                .project(*pid)
                .waiting(Some(false))
                .created_at_ms(NOW - HOUR + at)
                .last_activity_ms(NOW - HOUR + at)
                .build()
        })
        .collect();
    st
}

/// The bucket key for the project [`with`] builds under daemon id `p`.
///
/// Keyed on the DIRECTORY, not on the id, which is the whole point: the
/// collapse bit an operator sets belongs to the repo they collapsed and has
/// to survive the daemon handing that repo a different id.
fn pk(p: u64) -> GroupKey {
    GroupKey::Project(ProjectId(inbox::fnv1a(
        inbox::project_key(&format!("/src/p{p}")).as_bytes(),
    )))
}

/// Every row of a group's inbox, preview cut included.
fn inbox(group: &SidebarGroup<'_>) -> Vec<u64> {
    group
        .bands
        .active
        .iter()
        .chain(group.bands.hidden.iter())
        .map(|r| r.id().0)
        .collect()
}

/// Workspace ids in display order. `WorkspaceSet` exposes `iter()` rather
/// than an id list, because the bar wants the whole workspace; assertions
/// want the ids.
fn window_on(workspace: WorkspaceId) -> WindowState {
    WindowState {
        workspace,
        ..WindowState::default()
    }
}

/// Window number `index`, opened onto `workspace`.
fn window_at(index: usize, workspace: WorkspaceId) -> WindowState {
    WindowState {
        index,
        workspace,
        ..WindowState::default()
    }
}

fn ws_ids(set: &WorkspaceSet) -> Vec<WorkspaceId> {
    set.iter().map(|w| w.id).collect()
}

fn ids(rows: &[&SessionView]) -> Vec<u64> {
    rows.iter().map(|r| r.id().0).collect()
}

// ---- Control-plane fold ---------------------------------------------

/// A matching `Welcome` must flip the banner to live and carry the version.
/// If this regresses the banner sits on "connecting" forever while data
/// flows, which trains users to ignore the banner entirely.
#[test]
fn welcome_with_matching_protocol_goes_live() {
    let mut st = UiState::default();
    assert_eq!(
        apply(
            &mut st,
            ServerMsg::Welcome {
                protocol: PROTOCOL_VERSION,
                server_version: "0.1.0".into(),
            }
        ),
        Reaction::None
    );
    assert_eq!(
        st.daemon.conn,
        ConnState::Live {
            server_version: "0.1.0".into()
        }
    );
}

/// A protocol mismatch must be a visible failure, not a silent downgrade.
/// Accepting a mismatched server means decoding frames with the wrong
/// layout and painting garbage into the terminal.
#[test]
fn welcome_with_wrong_protocol_fails_visibly() {
    let mut st = UiState::default();
    apply(
        &mut st,
        ServerMsg::Welcome {
            protocol: PROTOCOL_VERSION + 7,
            server_version: "9.9.9".into(),
        },
    );
    let ConnState::Failed { detail } = &st.daemon.conn else {
        panic!("expected Failed, got {:?}", st.daemon.conn);
    };
    assert!(
        detail.contains(&format!("client speaks {PROTOCOL_VERSION}")),
        "{detail}"
    );
}

/// `SessionUpdated` must replace the daemon's half in place and leave the
/// operator's half alone. Remove-then-push would send a row to the bottom
/// of its group every time its activity time ticked, and it would drop the
/// snooze every time a parked agent printed a line.
#[test]
fn session_updated_keeps_list_position_and_client_local_state() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    st.snooze(&[SessionId(11)], NOW + HOUR, NOW);
    st.daemon.visit(SessionId(11), NOW - 60_000);

    let mut changed = info(11);
    changed.title = "renamed".into();
    changed.status = SessionStatus::Exited { code: Some(3) };
    apply(&mut st, ServerMsg::SessionUpdated(changed));

    assert_eq!(
        st.daemon
            .sessions
            .iter()
            .map(|r| r.id().0)
            .collect::<Vec<_>>(),
        vec![10, 11, 12]
    );
    assert_eq!(st.daemon.sessions[1].info.title, "renamed");
    assert_eq!(
        st.daemon.sessions[1].info.status,
        SessionStatus::Exited { code: Some(3) }
    );
    assert_eq!(
        st.daemon.sessions[1].snooze.map(|s| s.wake_at_ms),
        Some(NOW + HOUR),
        "a daemon update must not un-park a row the operator parked"
    );
    assert_eq!(
        st.daemon.sessions[1].last_visited_ms,
        Some(NOW - 60_000),
        "the daemon's update is not a visit, so the stamp must not move"
    );
}

/// An update for an unknown id must append rather than be dropped. The
/// server is allowed to push an update the client has no snapshot for
/// (reconnect ordering); dropping it hides a live session forever.
#[test]
fn session_updated_for_unknown_id_appends() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    apply(&mut st, ServerMsg::SessionUpdated(info(99)));
    assert_eq!(
        st.daemon
            .sessions
            .iter()
            .map(|r| r.id().0)
            .collect::<Vec<_>>(),
        vec![10, 99]
    );
}

/// A full snapshot must keep the operator's axis. A reconnect is exactly
/// when a client re-lists, and un-snoozing every parked row at that moment
/// is the worst possible time to lose them.
#[test]
fn a_snapshot_keeps_the_operators_snooze_settle_and_visit_stamps() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.snooze(&[SessionId(10)], NOW + 2 * HOUR, NOW - 1000);
    st.daemon.sessions[1].settle_override = Some(SettleOverride::Settled);
    st.daemon.visit(SessionId(11), NOW - 500);

    apply(
        &mut st,
        ServerMsg::Sessions {
            sessions: vec![info(10), info(11)],
        },
    );

    assert_eq!(
        st.row(SessionId(10))
            .and_then(|r| r.snooze)
            .map(|s| s.wake_at_ms),
        Some(NOW + 2 * HOUR)
    );
    assert_eq!(
        st.row(SessionId(11)).and_then(|r| r.settle_override),
        Some(SettleOverride::Settled)
    );
    assert_eq!(
        st.row(SessionId(11)).and_then(|r| r.last_visited_ms),
        Some(NOW - 500),
        "a snapshot with nothing focused is not a visit either"
    );
}

/// `Exited` must mutate the existing row's status and touch nothing else.
/// Replacing the row would drop the title and cwd the sidebar draws.
#[test]
fn exited_sets_status_without_losing_the_row() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.daemon.sessions[0].info.title = "claude".into();
    apply(
        &mut st,
        ServerMsg::Exited {
            session: SessionId(10),
            code: None,
        },
    );
    assert_eq!(
        st.daemon.sessions[0].info.status,
        SessionStatus::Exited { code: None }
    );
    assert_eq!(st.daemon.sessions[0].info.title, "claude");
}

/// `Exited` for an id the client never saw must not panic or invent a row.
#[test]
fn exited_for_unknown_session_is_inert() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    apply(
        &mut st,
        ServerMsg::Exited {
            session: SessionId(4242),
            code: Some(1),
        },
    );
    assert_eq!(st.daemon.sessions.len(), 1);
    assert_eq!(st.daemon.sessions[0].info.status, SessionStatus::Running);
}

/// `resume_seq` must be `from_seq + data.len()`, the offset one past the
/// backfill. The bridge splices buffered live frames against exactly this
/// number; if it is off by even one byte the terminal either drops a byte
/// or double-writes one, and a dropped byte inside a CSI escape corrupts
/// the rest of the screen.
#[test]
fn scrollback_chunk_computes_the_exact_resume_offset() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.open(SessionId(10), NOW);
    let r = apply(
        &mut st,
        ServerMsg::ScrollbackChunk {
            session: SessionId(10),
            from_seq: 1_000,
            data: vec![b'a', b'b', b'c', b'd'],
            more: true,
        },
    );
    assert_eq!(
        r,
        Reaction::Backfill {
            session: SessionId(10),
            from_seq: 1_000,
            resume_seq: 1_004,
            bytes: vec![b'a', b'b', b'c', b'd'],
            jump_seq: None,
            keep_view: false,
            more: true,
        }
    );
}

/// An empty chunk (client asked for history older than the oldest retained
/// byte) must still produce a resume offset equal to `from_seq`, so the
/// bridge flushes its buffered live frames instead of holding them forever.
#[test]
fn empty_scrollback_chunk_still_resumes() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.open(SessionId(10), NOW);
    assert_eq!(
        apply(
            &mut st,
            ServerMsg::ScrollbackChunk {
                session: SessionId(10),
                from_seq: 77,
                data: vec![],
                more: false,
            }
        ),
        Reaction::Backfill {
            session: SessionId(10),
            from_seq: 77,
            resume_seq: 77,
            bytes: vec![],
            jump_seq: None,
            keep_view: false,
            more: false,
        }
    );
}

/// A search jump must reach the bridge, and must not survive its answer.
///
/// THE BUG this locks out, in two halves. The first is R2: the hit's byte
/// offset was taken from the wire and thrown away, so the tooltip's
/// promise to "jump to this line" focused the session and left you
/// wherever the head-anchored history stopped. The second is the way the
/// obvious fix goes wrong: an intent that is set and never cleared makes
/// every LATER repaint of that session jump back to a hit the operator
/// finished with, including the repaint a page-back produces.
#[test]
fn a_search_jump_reaches_the_bridge_once_and_then_stops() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.open(SessionId(10), NOW);
    st.window.history_intent = HistoryIntent::Jump(4_096);

    let first = apply(
        &mut st,
        ServerMsg::ScrollbackChunk {
            session: SessionId(10),
            from_seq: 1_000,
            data: vec![b'x'; 8],
            more: true,
        },
    );
    assert_eq!(
        first,
        Reaction::Backfill {
            session: SessionId(10),
            from_seq: 1_000,
            resume_seq: 1_008,
            bytes: vec![b'x'; 8],
            jump_seq: Some(4_096),
            keep_view: false,
            more: true,
        }
    );
    assert_eq!(st.window.history_intent, HistoryIntent::Attach);

    let second = apply(
        &mut st,
        ServerMsg::ScrollbackChunk {
            session: SessionId(10),
            from_seq: 1_000,
            data: vec![b'x'; 8],
            more: true,
        },
    );
    let Reaction::Backfill { jump_seq, .. } = second else {
        panic!("the second chunk must still paint");
    };
    assert_eq!(jump_seq, None, "the jump repeated on a later repaint");
}

/// A page-back must keep the viewport, and the anchor must track what was
/// actually returned.
///
/// `span` is what the next page-back grows from. Recording the REQUESTED
/// budget instead of the returned length would make every page-back after
/// the daemon runs out of history ask for a larger window and get the same
/// bytes, so the operator would scroll and scroll against a buffer that
/// never grows and never says why.
#[test]
fn a_page_back_keeps_the_view_and_records_what_came_back() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.open(SessionId(10), NOW);
    st.window.history_intent = HistoryIntent::PageBack;

    let r = apply(
        &mut st,
        ServerMsg::ScrollbackChunk {
            session: SessionId(10),
            from_seq: 512,
            data: vec![b'y'; 40],
            more: false,
        },
    );
    let Reaction::Backfill {
        keep_view,
        jump_seq,
        more,
        ..
    } = r
    else {
        panic!("a page-back must paint");
    };
    assert!(keep_view, "the operator was snapped back to the bottom");
    assert_eq!(jump_seq, None);
    assert!(!more);

    assert_eq!(
        st.window.history,
        HistoryWindow {
            session: Some(SessionId(10)),
            from_seq: 512,
            span: 40,
            more: false,
        }
    );
}

/// Moving focus must forget the previous session's history anchor.
///
/// Otherwise a page-back in the newly focused pane asks for the region
/// before the PREVIOUS session's window, which is an offset into a
/// different stream: the daemon answers with real bytes from the wrong
/// place in the wrong session's history.
#[test]
fn focusing_another_session_drops_the_history_anchor() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.open(SessionId(10), NOW);
    apply(
        &mut st,
        ServerMsg::ScrollbackChunk {
            session: SessionId(10),
            from_seq: 9_000,
            data: vec![b'z'; 4],
            more: true,
        },
    );
    assert_eq!(st.window.history.session, Some(SessionId(10)));

    st.open(SessionId(11), NOW);
    assert_eq!(st.window.history, HistoryWindow::default());
    assert_eq!(st.window.history_intent, HistoryIntent::Attach);

    // Re-opening the session already focused must NOT reset it: that path
    // runs on every click of the active row, and clearing there would make
    // paging impossible for anyone who clicks the row they are on.
    st.window.history.from_seq = 77;
    st.open(SessionId(11), NOW);
    assert_eq!(st.window.history.from_seq, 77);
}

/// A chunk for a session that is no longer focused must be discarded. The
/// single terminal has already been reset and repointed at another
/// session, so painting the stale chunk writes one agent's output into
/// another agent's pane.
#[test]
fn scrollback_chunk_for_unfocused_session_is_dropped() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.open(SessionId(10), NOW);
    st.open(SessionId(11), NOW);
    assert_eq!(
        apply(
            &mut st,
            ServerMsg::ScrollbackChunk {
                session: SessionId(10),
                from_seq: 0,
                data: vec![b'x'],
                more: false,
            }
        ),
        Reaction::None
    );
}

/// A gap error for the focused session must ask for a repaint, not land in
/// the error banner. Silently splicing across a gap is the failure the
/// byte-offset `seq` exists to prevent.
#[test]
fn gap_error_for_focused_session_requests_refill() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.open(SessionId(10), NOW);
    assert_eq!(
        apply(
            &mut st,
            ServerMsg::error(
                Some(SessionId(10)),
                format!("{GAP_ERROR_PREFIX} resume at 4096")
            )
        ),
        Reaction::Refill {
            session: SessionId(10)
        }
    );
    assert_eq!(st.window.flash, None);
}

/// A gap error for a session that is not focused must not trigger a
/// repaint of the pane the user is actually looking at.
#[test]
fn gap_error_for_other_session_does_not_repaint() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.open(SessionId(11), NOW);
    assert_eq!(
        apply(
            &mut st,
            ServerMsg::error(
                Some(SessionId(10)),
                format!("{GAP_ERROR_PREFIX} resume at 10")
            )
        ),
        Reaction::None
    );
    assert_eq!(
        st.window.flash.as_ref().map(|f| f.text.as_str()),
        Some("session 10: output gap: resume at 10")
    );
    assert_eq!(
        st.window.flash.as_ref().map(|f| f.kind),
        Some(FlashKind::Error)
    );
}

/// A plain error must surface with its session id attached, and a
/// connection-wide error without one. Losing the id makes an error about
/// one of twenty agents unattributable.
#[test]
fn plain_errors_are_recorded_with_and_without_a_session() {
    let mut st = UiState::default();
    apply(
        &mut st,
        ServerMsg::error(Some(SessionId(3)), "spawn failed: No such file"),
    );
    assert_eq!(
        st.window.flash.as_ref().map(|f| f.text.as_str()),
        Some("session 3: spawn failed: No such file")
    );
    apply(
        &mut st,
        ServerMsg::error(None, "scrollback budget exhausted"),
    );
    assert_eq!(
        st.window.flash.as_ref().map(|f| f.text.as_str()),
        Some("scrollback budget exhausted")
    );
}

// ---- Tab strip -------------------------------------------------------

/// A `Sessions` snapshot that drops a session must drop its tab and move
/// focus to the neighbour on the right. Leaving a tab pointing at a dead id
/// renders a tab whose title lookup fails on every frame.
#[test]
fn snapshot_prunes_dead_tabs_and_moves_focus_right() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    st.open(SessionId(10), NOW);
    st.open(SessionId(11), NOW);
    st.open(SessionId(12), NOW);
    st.window.focused = Some(SessionId(11));

    apply(
        &mut st,
        ServerMsg::Sessions {
            sessions: vec![info(10), info(12)],
        },
    );

    assert_eq!(st.window.tabs, vec![SessionId(10), SessionId(12)]);
    assert_eq!(st.window.focused, Some(SessionId(12)));
}

/// When the focused tab was last in the strip, pruning must fall back to
/// the tab on its left rather than to nothing.
#[test]
fn snapshot_falls_back_left_for_the_last_tab() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.open(SessionId(10), NOW);
    st.open(SessionId(11), NOW);
    apply(
        &mut st,
        ServerMsg::Sessions {
            sessions: vec![info(10)],
        },
    );
    assert_eq!(st.window.tabs, vec![SessionId(10)]);
    assert_eq!(st.window.focused, Some(SessionId(10)));
}

/// Pruning every session must clear focus rather than leave a dangling id.
/// A dangling focus makes the client keep sending `Input` for a session the
/// server has forgotten.
#[test]
fn snapshot_that_empties_everything_clears_focus() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.open(SessionId(10), NOW);
    apply(&mut st, ServerMsg::Sessions { sessions: vec![] });
    assert!(st.window.tabs.is_empty());
    assert_eq!(st.window.focused, None);
    assert!(
        st.window.selection.is_empty(),
        "a selection holding closed sessions makes a bulk action act on rows that are gone"
    );
}

/// Opening an already-open session must focus it without duplicating the
/// tab. Double-clicking a sidebar row is the common way to reach this.
#[test]
fn open_is_idempotent_on_the_tab_strip() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.open(SessionId(10), NOW);
    st.open(SessionId(11), NOW);
    st.open(SessionId(10), NOW);
    assert_eq!(st.window.tabs, vec![SessionId(10), SessionId(11)]);
    assert_eq!(st.window.focused, Some(SessionId(10)));
}

/// The strip must cap at [`MAX_TABS`] and evict the least recently used
/// tab, keeping strip order stable. At twenty concurrent agents an
/// uncapped strip gives each tab 51px, which fits a status dot and nothing
/// else, and the tabs the user is actually working in get pushed off the
/// end by ones they opened once and forgot.
#[test]
fn the_strip_caps_and_evicts_the_least_recently_used_tab() {
    let ids: Vec<(u64, u64, u64)> = (0..MAX_TABS as u64 + 1).map(|i| (10 + i, 1, i)).collect();
    let mut st = with(&[1], &ids);
    for i in 0..MAX_TABS as u64 {
        st.open(SessionId(10 + i), NOW);
    }
    assert_eq!(st.window.tabs.len(), MAX_TABS);

    // Re-touch the oldest so it is no longer the eviction candidate.
    st.open(SessionId(10), NOW);
    st.open(SessionId(10 + MAX_TABS as u64), NOW);

    assert_eq!(st.window.tabs.len(), MAX_TABS);
    assert!(
        st.window.tabs.contains(&SessionId(10)),
        "a tab used one action ago must not be evicted"
    );
    assert!(
        !st.window.tabs.contains(&SessionId(11)),
        "the least recently used tab must be the one evicted"
    );
    assert_eq!(st.window.focused, Some(SessionId(10 + MAX_TABS as u64)));
}

/// Eviction must never touch the session list. A tab leaving the strip is
/// a display decision; the child is still running and its sidebar row must
/// still be there, one click from coming back.
#[test]
fn eviction_removes_a_tab_but_never_a_session() {
    let ids: Vec<(u64, u64, u64)> = (0..MAX_TABS as u64 + 3).map(|i| (10 + i, 1, i)).collect();
    let mut st = with(&[1], &ids);
    let before = st.daemon.sessions.len();
    for i in 0..MAX_TABS as u64 + 3 {
        st.open(SessionId(10 + i), NOW);
    }
    assert_eq!(st.daemon.sessions.len(), before);
    assert_eq!(st.window.tabs.len(), MAX_TABS);
    assert_eq!(
        inbox(&st.tree(clock())[0]).len(),
        before,
        "the sidebar must still list every session"
    );
}

/// The focused tab must never be the eviction victim, even when it is the
/// oldest by recency. Evicting the pane the user is looking at would blank
/// the terminal mid-read.
#[test]
fn the_focused_tab_is_never_evicted() {
    let ids: Vec<(u64, u64, u64)> = (0..MAX_TABS as u64 + 1).map(|i| (10 + i, 1, i)).collect();
    let mut st = with(&[1], &ids);
    for i in 0..MAX_TABS as u64 {
        st.open(SessionId(10 + i), NOW);
    }
    // Focus the least recently used tab without going through open().
    st.window.focused = Some(SessionId(10));
    st.window.tabs.push(SessionId(999));
    st.window.evict_stale_tabs();
    assert!(st.window.tabs.contains(&SessionId(10)));
}

/// Strip order must not change when a tab is focused. Reordering on focus
/// moves a tab out from under the pointer between the mousedown and the
/// next click, so the second click lands on the wrong session.
#[test]
fn focusing_a_tab_does_not_reorder_the_strip() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    st.open(SessionId(10), NOW);
    st.open(SessionId(11), NOW);
    st.open(SessionId(12), NOW);
    let order = st.window.tabs.clone();
    st.open(SessionId(10), NOW);
    st.cycle(1);
    st.focus_index(2);
    assert_eq!(st.window.tabs, order);
}

/// Keyboard switching must count as use. Otherwise a tab reached only with
/// Alt+N or Ctrl+Tab looks stale to the evictor and gets dropped out from
/// under a user who never touches the mouse.
#[test]
fn keyboard_switching_protects_a_tab_from_eviction() {
    let ids: Vec<(u64, u64, u64)> = (0..MAX_TABS as u64 + 1).map(|i| (10 + i, 1, i)).collect();
    let mut st = with(&[1], &ids);
    for i in 0..MAX_TABS as u64 {
        st.open(SessionId(10 + i), NOW);
    }
    st.focus_index(0); // reach the oldest tab by keyboard only
    st.open(SessionId(10 + MAX_TABS as u64), NOW);
    assert!(
        st.window.tabs.contains(&SessionId(10)),
        "Alt+1 must count as using that tab"
    );
}

/// Closing a non-focused tab must not disturb focus.
#[test]
fn closing_an_unfocused_tab_leaves_focus_alone() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    for id in [10, 11, 12] {
        st.open(SessionId(id), NOW);
    }
    st.window.focused = Some(SessionId(12));
    st.close_tab(SessionId(10));
    assert_eq!(st.window.tabs, vec![SessionId(11), SessionId(12)]);
    assert_eq!(st.window.focused, Some(SessionId(12)));
}

/// Closing the focused tab must land on the right neighbour, and closing
/// the only tab must clear focus.
#[test]
fn closing_the_focused_tab_picks_the_right_neighbour() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    for id in [10, 11, 12] {
        st.open(SessionId(id), NOW);
    }
    st.window.focused = Some(SessionId(11));
    st.close_tab(SessionId(11));
    assert_eq!(st.window.focused, Some(SessionId(12)));

    st.close_tab(SessionId(12));
    assert_eq!(st.window.focused, Some(SessionId(10)));
    st.close_tab(SessionId(10));
    assert_eq!(st.window.focused, None);
    assert!(st.window.tabs.is_empty());
}

/// Cycling must wrap in both directions. Without `rem_euclid`, going left
/// from the first tab underflows the index cast and panics.
#[test]
fn cycle_wraps_both_ways() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    for id in [10, 11, 12] {
        st.open(SessionId(id), NOW);
    }
    st.window.focused = Some(SessionId(10));

    st.cycle(-1);
    assert_eq!(st.window.focused, Some(SessionId(12)));
    st.cycle(1);
    assert_eq!(st.window.focused, Some(SessionId(10)));
    st.cycle(1);
    assert_eq!(st.window.focused, Some(SessionId(11)));
}

/// Cycling with no tabs open must clear focus, not panic on a modulo by
/// zero. An empty strip is the app's start state, and Ctrl+Tab there is a
/// perfectly ordinary thing for a user to press.
#[test]
fn cycle_on_an_empty_strip_is_safe() {
    let mut st = UiState::default();
    st.cycle(1);
    assert_eq!(st.window.focused, None);
    st.cycle(-1);
    assert_eq!(st.window.focused, None);
}

/// Alt+N past the end must do nothing rather than clamp to the last tab.
/// Clamping means Alt+9 always switches somewhere, so a misremembered
/// shortcut silently moves the user off the pane they were reading.
#[test]
fn focus_index_out_of_range_is_a_no_op() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.open(SessionId(10), NOW);
    st.open(SessionId(11), NOW);
    st.window.focused = Some(SessionId(10));
    st.focus_index(7);
    assert_eq!(st.window.focused, Some(SessionId(10)));
    st.focus_index(1);
    assert_eq!(st.window.focused, Some(SessionId(11)));
}

// ---- Grouping and the three bands ------------------------------------

/// Grouping must keep projects in the server's order. Project order comes
/// from the server so every client shows the same sidebar; reordering it
/// here would make two clients disagree about the same server.
#[test]
fn groups_preserve_project_order_and_sort_newest_first() {
    let mut st = with(&[2, 1], &[(30, 1, 300), (10, 2, 100), (20, 1, 50)]);
    st.daemon.projects[0].name = "beta".into();
    st.daemon.projects[1].name = "alpha".into();

    let g = st.tree(clock());
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].project.unwrap().name, "beta");
    assert_eq!(ids(&g[0].bands.active), vec![10]);
    assert_eq!(g[1].project.unwrap().name, "alpha");
    assert_eq!(ids(&g[1].bands.active), vec![30, 20]);
}

/// Sessions created at the same instant must order by id. Without the
/// tiebreak the sort is stable only by luck of input order, and two rows
/// swap places whenever an unrelated snapshot arrives.
#[test]
fn groups_break_exact_ties_by_id() {
    let st = with(&[1], &[(30, 1, 500), (10, 1, 500), (20, 1, 500)]);
    assert_eq!(ids(&st.tree(clock())[0].bands.active), vec![10, 20, 30]);
}

/// THE INBOX DOES NOT REORDER ON STATUS. This is the model's deliberate
/// choice and the reason the Woke badge works: a row holds its position
/// from open until it changes band, so nothing shifts under the cursor
/// while you are reading it. An approval-blocked row stays exactly where
/// it was created; `attention_target` is what takes you to it.
#[test]
fn the_inbox_order_never_moves_a_row_because_its_status_changed() {
    let mut st = with(&[1], &[(10, 1, 100), (11, 1, 200), (12, 1, 300)]);
    let before = ids(&st.tree(clock())[0].bands.active);
    assert_eq!(before, vec![12, 11, 10]);

    // The oldest row starts asking for an approval, which is the single
    // most urgent thing in the list.
    st.daemon.sessions[0].info.attention.waiting = Some(true);
    st.daemon.sessions[0].info.hint = Some(vitrum_proto::AgentHint {
        state: HintState::Approval,
        label: Some("force-push?".into()),
        received_at_ms: NOW,
    });

    let after = ids(&st.tree(clock())[0].bands.active);
    assert_eq!(after, before, "an urgent row must not jump the queue");
    assert_eq!(
        st.row(SessionId(10)).unwrap().status(),
        SidebarStatus::Approval
    );
    assert_eq!(
        st.attention_target(clock(), Direction::Next),
        Some(SessionId(10)),
        "the jump key is what pays for the static order"
    );
}

/// Sessions whose project is missing must land in a trailing bucket, never
/// be dropped. `Projects` and `Sessions` are separate snapshots and
/// legitimately race on connect; a dropped session is an agent the user
/// cannot reach.
///
/// They now get one bucket per directory, named with the path, instead of
/// one anonymous lump, and they carry a rollup like every other bucket, so
/// collapsing one no longer hides its status chips.
#[test]
fn orphan_sessions_get_their_own_trailing_group() {
    let st = with(&[1], &[(10, 1, 0), (11, 404, 1)]);
    let g = st.tree(clock());
    assert_eq!(g.len(), 2);
    assert_eq!(g[0].key, pk(1));
    assert_eq!(
        g[1].key,
        GroupKey::Directory(directory_key(&inbox::project_key("/tmp")))
    );
    // Recomputed from the same cwd rather than written out, because the key a
    // directory gets is a platform fact three ways: `/tmp` is itself on Linux,
    // canonicalises to `/private/tmp` on macOS, and does not exist on Windows
    // so it keeps the typed text with its separators unified. Still an
    // independent expectation: a label taken from anything but the directory
    // fails this.
    let tmp = inbox::project_key("/tmp");
    assert_eq!(g[1].label, tmp, "the bucket is named by the directory");
    assert_eq!(g[1].project, None);
    assert_eq!(ids(&g[1].bands.active), vec![11]);
    assert_eq!(
        g[1].bands.rollup.as_ref().map(|r| r.total),
        Some(1),
        "a bucket that is not a project still rolls up, so a collapsed \
         header keeps its chips"
    );
    assert!(g[1].collapsible());
}

/// With no orphans there must be no empty trailing bucket, otherwise the
/// sidebar grows a permanent blank group.
#[test]
fn no_orphan_group_when_every_session_has_a_project() {
    let st = with(&[1, 2], &[(10, 1, 0), (11, 2, 1)]);
    let g = st.tree(clock());
    assert_eq!(g.len(), 2);
    assert!(g.iter().all(|x| x.project.is_some()));
}

/// A project whose sessions all live in another workspace must not appear
/// in this one.
///
/// This replaces `empty_projects_still_render`, whose premise was that "a
/// project the daemon reports with no sessions is a place to start one".
/// Both halves of that stopped being true. The daemon's registry now only
/// holds a project while a session references it, so it cannot report a
/// session-less one; and `daemon.projects` is daemon-wide, so zipping it
/// against this workspace's buckets drew a header for every project that
/// existed anywhere. Switching to a freshly created workspace showed a
/// project header reading "No sessions here yet" over nothing, which is
/// exactly what a separate top-level context must not do.
#[test]
fn a_project_whose_sessions_are_elsewhere_is_not_drawn_here() {
    let mut st = with(&[1, 2], &[(10, 1, 0), (11, 2, 1)]);
    let review = st.daemon.workspaces.create("Review").unwrap();

    // Both projects are visible while both sessions are in this workspace.
    assert_eq!(st.tree(clock()).len(), 2);

    // Move project 2's only session to the other workspace.
    place(&mut st.daemon, 11, review);
    let here = st.tree(clock());
    assert_eq!(
        here.len(),
        1,
        "the other workspace's project is still drawn here: {:?}",
        here.iter().map(|g| g.label.clone()).collect::<Vec<_>>()
    );
    assert!(here.iter().all(|g| !g.is_empty()));

    // And the workspace it moved to shows that one and nothing else.
    let mut there = st.clone();
    there.window.workspace = review;
    let shown = there.tree(clock());
    assert_eq!(shown.len(), 1);
    assert!(!shown[0].is_empty());
}

/// A brand new workspace draws NOTHING.
///
/// Not a header, not an empty bucket, not a placeholder. The operator
/// required completely separate workspaces with blank sidebars,
/// and a header over nothing is the one thing that makes a blank sidebar
/// look broken instead of empty.
#[test]
fn a_new_workspace_draws_an_entirely_blank_sidebar() {
    let mut st = with(&[1, 2], &[(10, 1, 0), (11, 2, 1)]);
    let fresh = st.daemon.workspaces.create("Fresh").unwrap();
    st.window.workspace = fresh;
    assert!(
        st.tree(clock()).is_empty(),
        "a workspace nothing has ever run in still draws buckets"
    );
}

/// The three bands must be the model's, each with its own comparator, and
/// a row must appear in exactly one of them.
#[test]
fn rows_split_into_the_models_three_bands() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    st.snooze(&[SessionId(11)], NOW + HOUR, NOW);
    st.daemon.sessions[2].info.status = SessionStatus::Exited { code: Some(0) };
    st.daemon.sessions[2].info.attention.waiting = None;
    st.daemon.visit(SessionId(12), NOW);

    let g = st.tree(clock());
    assert_eq!(ids(&g[0].bands.active), vec![10]);
    assert_eq!(ids(&g[0].bands.snoozed), vec![11]);
    assert_eq!(ids(&g[0].bands.settled), vec![12]);
    assert_eq!(g[0].len(), 3);
    assert_eq!(
        g[0].section(Section::Snoozed).len(),
        1,
        "the parked band is addressable by Section, which is what the markup loops over"
    );
}

// ---- Snooze ----------------------------------------------------------

/// Snoozing must park the row and the countdown must come from the model,
/// not from the client's own arithmetic.
#[test]
fn snoozing_parks_a_row_and_the_countdown_comes_from_the_model() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    assert_eq!(st.snooze(&[SessionId(10)], NOW + 2 * HOUR, NOW), 1);

    let r = st.row(SessionId(10)).unwrap();
    assert_eq!(
        r.disposition(clock(), st.daemon.settings.policy),
        Disposition::Snoozed
    );
    assert_eq!(
        vitrum_model::wake_countdown_label(NOW + 2 * HOUR, NOW),
        "2h"
    );
    assert_eq!(
        crate::inbox::disposition_badge(r, clock(), st.daemon.settings.policy)
            .map(|b| b.text)
            .as_deref(),
        Some("2h")
    );
}

/// A session blocked on the operator must NOT be snoozable, and the refusal
/// has to be visible. Hiding a pending approval defeats the request, and
/// the row would raise its hand and come straight back, so offering the
/// action at all would be a lie.
#[test]
fn a_session_blocked_on_the_operator_cannot_be_snoozed_and_the_menu_says_why() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.daemon.sessions[0].info.attention.waiting = Some(true);
    st.daemon.sessions[0].info.hint = Some(vitrum_proto::AgentHint {
        state: HintState::Approval,
        label: None,
        received_at_ms: NOW,
    });

    assert!(!st.row(SessionId(10)).unwrap().can_snooze());
    assert_eq!(
        st.snooze(&[SessionId(10)], NOW + HOUR, NOW),
        0,
        "the mutation itself must refuse, not just the menu"
    );
    assert_eq!(st.row(SessionId(10)).unwrap().snooze, None);

    let items = st.menu_items(SessionId(10), clock());
    let head = items
        .iter()
        .find(|i| i.action == MenuAction::SnoozeHeader)
        .expect("the snooze caption is always shown, disabled when refused");
    assert!(!head.enabled);
    assert_eq!(
        head.hint.as_deref(),
        Some("blocked on you \u{2014} it would wake immediately")
    );
    assert!(
        !items
            .iter()
            .any(|i| matches!(i.action, MenuAction::Snooze(_))),
        "a refused snooze must not still offer its presets"
    );
}

/// The presets offered are the model's, with the model's labels and wake
/// instants. Reimplementing "tomorrow 9:00" in the client is how a snooze
/// set the evening before a clock change lands an hour out.
#[test]
fn the_snooze_presets_are_the_models() {
    let st = with(&[1], &[(10, 1, 0)]);
    let mine = st.snooze_presets(clock());
    let theirs = vitrum_model::snooze_presets(clock());
    assert_eq!(mine, theirs);
    assert!(
        mine.iter().any(|p| p.id == SnoozePresetId::Hour),
        "the hour preset is always offered"
    );
    assert!(mine.iter().any(|p| p.id == SnoozePresetId::Tomorrow));
    assert!(mine.iter().any(|p| p.id == SnoozePresetId::NextWeek));

    let items = st.menu_items(SessionId(10), clock());
    for preset in &mine {
        let entry = items
            .iter()
            .find(|i| i.action == MenuAction::Snooze(preset.id))
            .unwrap_or_else(|| panic!("no menu entry for {:?}", preset.id));
        assert!(entry.label.trim_start().starts_with(preset.label));
        assert_eq!(
            entry.hint.as_deref(),
            Some(preset.when_label.as_str()),
            "the entry must state the resulting time, not just the choice"
        );
    }
}

/// An elapsed snooze must wake the row IN PLACE, wearing the Woke badge.
/// The inbox sort is static, so the row comes back exactly where it was and
/// the badge is the only thing that can say it came back.
#[test]
fn an_elapsed_snooze_wakes_the_row_in_place_wearing_a_badge() {
    let mut st = with(&[1], &[(10, 1, 100), (11, 1, 200), (12, 1, 300)]);
    let before = ids(&st.tree(clock())[0].bands.active);
    st.snooze(&[SessionId(11)], NOW + HOUR, NOW);
    assert_eq!(ids(&st.tree(clock())[0].bands.snoozed), vec![11]);

    // Two hours later the snooze has simply stopped classifying. Nothing
    // fired; there is no timer anywhere in this program.
    let later = Clock::utc(NOW + 2 * HOUR);
    let g = st.tree(later);
    assert_eq!(
        ids(&g[0].bands.active),
        before,
        "the woken row must reappear in its original position"
    );
    assert!(g[0].bands.snoozed.is_empty());
    assert_eq!(
        st.row(SessionId(11))
            .unwrap()
            .disposition(later, st.daemon.settings.policy),
        Disposition::Woke
    );
}

/// Visiting a woken row retires the badge, the same way a visit retires
/// unread. A badge that needed its own dismissal would never be cleared.
#[test]
fn visiting_a_woken_row_retires_the_woke_badge() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.snooze(&[SessionId(10)], NOW + HOUR, NOW);
    let later = Clock::utc(NOW + 2 * HOUR);
    assert_eq!(
        st.row(SessionId(10))
            .unwrap()
            .disposition(later, st.daemon.settings.policy),
        Disposition::Woke
    );

    st.open(SessionId(10), NOW + 2 * HOUR);
    assert_eq!(
        st.row(SessionId(10))
            .unwrap()
            .disposition(later, st.daemon.settings.policy),
        Disposition::Active,
        "looking at the row IS the acknowledgement"
    );
}

/// Waking must clear the snooze outright rather than let it expire. Stale
/// snooze fields would mint a fresh Woke badge on the row's next completion
/// for the rest of its life.
#[test]
fn waking_clears_the_snooze_rather_than_letting_it_expire() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.snooze(&[SessionId(10)], NOW + HOUR, NOW);
    st.wake(&[SessionId(10)], NOW);
    assert_eq!(st.row(SessionId(10)).unwrap().snooze, None);
    assert_eq!(
        st.row(SessionId(10))
            .unwrap()
            .disposition(clock(), st.daemon.settings.policy),
        Disposition::Active
    );
}

/// A snoozed row's menu offers waking, not more snoozing, and names the
/// time it is parked until.
#[test]
fn a_parked_rows_menu_offers_waking_and_names_the_wake_time() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    let wake_at = NOW + 3 * HOUR;
    st.snooze(&[SessionId(10)], wake_at, NOW);
    let items = st.menu_items(SessionId(10), clock());
    let wake = items
        .iter()
        .find(|i| i.action == MenuAction::Wake)
        .expect("a parked row offers Wake");
    assert_eq!(
        wake.hint.as_deref(),
        Some(
            format!(
                "parked until {}",
                vitrum_model::wake_description(wake_at, clock())
            )
            .as_str()
        )
    );
    assert!(!items.iter().any(|i| i.action == MenuAction::SnoozeHeader));
}

// ---- Settle ----------------------------------------------------------

/// Settling must refuse a row mid-turn and say why. Draining a session out
/// from under a running job is exactly the surprise the refusal exists to
/// stop.
#[test]
fn settle_refuses_a_working_row_and_says_why() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    assert_eq!(
        st.row(SessionId(10)).unwrap().status(),
        SidebarStatus::Working
    );
    assert_eq!(st.settle(&[SessionId(10)], NOW), 0);

    let items = st.menu_items(SessionId(10), clock());
    let settle = items
        .iter()
        .find(|i| i.action == MenuAction::Settle)
        .expect("Settle is shown, disabled");
    assert!(!settle.enabled);
    assert_eq!(
        settle.hint.as_deref(),
        Some("still working \u{2014} wait for the turn to end")
    );
}

/// A resting row settles, drops out of the inbox, and can be pulled back.
#[test]
fn settling_drains_a_resting_row_and_unsettling_returns_it() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.daemon.sessions[0].info.attention.waiting = Some(true);
    assert_eq!(
        st.row(SessionId(10)).unwrap().status(),
        SidebarStatus::Ready
    );
    assert_eq!(st.settle(&[SessionId(10)], NOW), 1);
    assert_eq!(ids(&st.tree(clock())[0].bands.settled), vec![10]);

    st.unsettle(&[SessionId(10)]);
    assert_eq!(ids(&st.tree(clock())[0].bands.active), vec![10]);
}

/// Unseen completion is its own axis: marking a row unseen re-arms it, and
/// visiting clears it. This is what distinguishes "output I have not read"
/// from "a job finished while I was away".
#[test]
fn marking_a_row_unseen_rearms_the_completion_indicator() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.daemon.sessions[0].info.status = SessionStatus::Exited { code: Some(0) };
    st.daemon.sessions[0].info.unread = true;
    assert!(st.row(SessionId(10)).unwrap().has_unseen_completion());

    st.mark_seen(&[SessionId(10)], NOW);
    assert!(!st.row(SessionId(10)).unwrap().has_unseen_completion());

    st.mark_unseen(&[SessionId(10)]);
    assert!(st.row(SessionId(10)).unwrap().has_unseen_completion());
}

/// A visit stamp must never move backwards. An out-of-order stamp from a
/// slow message would otherwise resurrect a badge the operator cleared.
#[test]
fn a_visit_stamp_never_moves_backwards() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.daemon.visit(SessionId(10), NOW);
    st.daemon.visit(SessionId(10), NOW - 10_000);
    assert_eq!(st.row(SessionId(10)).unwrap().last_visited_ms, Some(NOW));
}

// ---- Bands, preview and the visible list ------------------------------

/// Band expansion is remembered per group AND per band. One shared flag
/// would open every project's Done section the moment you opened one.
#[test]
fn band_expansion_is_remembered_per_group_and_per_band() {
    let mut st = with(&[1, 2], &[(10, 1, 0), (20, 2, 0)]);
    assert!(!st.section_open(pk(1), Section::Settled));
    assert!(
        st.section_open(pk(1), Section::Active),
        "the inbox has no head, so there would be nothing to reopen it with"
    );

    st.toggle_section(pk(1), Section::Settled);
    assert!(st.section_open(pk(1), Section::Settled));
    assert!(!st.section_open(pk(2), Section::Settled));
    assert!(!st.section_open(pk(1), Section::Snoozed));

    st.toggle_section(pk(1), Section::Active);
    assert!(
        st.section_open(pk(1), Section::Active),
        "the inbox cannot be collapsed"
    );
}

/// A filter forces every band and every preview open. Answering a search
/// with a collapsed band reads as "no results" for rows the user asked for
/// by name.
#[test]
fn a_filter_forces_every_band_and_preview_open() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    assert!(!st.section_open(pk(1), Section::Settled));
    assert!(!st.window.preview_expanded(pk(1)));
    st.window.filter = "s".into();
    assert!(st.section_open(pk(1), Section::Settled));
    assert!(st.section_open(pk(1), Section::Snoozed));
    assert!(st.window.preview_expanded(pk(1)));
}

/// The visible list must exclude everything that is off screen: a
/// collapsed project, a collapsed band, and the preview cut. Two
/// definitions of "visible" is how a keypress lands on a row nobody can
/// see.
#[test]
fn the_visible_list_excludes_collapsed_projects_bands_and_previewed_rows() {
    let ids_in: Vec<(u64, u64, u64)> = (0..12).map(|i| (10 + i, 1, i)).collect();
    let mut st = with(&[1, 2], &ids_in);
    st.daemon.sessions.push(
        row(99)
            .project(2)
            .waiting(Some(true))
            .created_at_ms(NOW - HOUR)
            .last_activity_ms(NOW - HOUR)
            .build(),
    );
    st.settle(&[SessionId(99)], NOW);

    let visible = st.visible_ids(clock());
    assert_eq!(
        visible.len(),
        crate::inbox::PREVIEW_LIMIT,
        "the preview cut and the collapsed Done band both remove rows"
    );
    assert!(
        !visible.contains(&SessionId(99)),
        "a row in a collapsed band is not reachable"
    );

    st.toggle_section(pk(2), Section::Settled);
    assert!(st.visible_ids(clock()).contains(&SessionId(99)));

    st.toggle_preview(pk(1));
    assert_eq!(st.visible_ids(clock()).len(), 13);

    st.window.collapsed.insert(pk(1));
    assert_eq!(st.visible_ids(clock()), vec![SessionId(99)]);
}

/// The attention count and the visible list must see EXACTLY the same
/// rows, at every combination of the three things that hide one.
///
/// They are two public answers over one private walk
/// ([`WindowState::visible_rows_of`]), and this is the property that walk
/// exists to guarantee. Before it, the count flattened the tree into an
/// owned id list and then asked `DaemonState::row` to find every id again
/// in the session vector; the two could not disagree because one was
/// literally built from the other, and making the count cheap is exactly
/// the change that could have separated them.
///
/// Locks out a count that reads a collapsed bucket, a closed band or a
/// row behind the preview cut. All three are silent: the number on the
/// jump affordance would simply be too big, and pressing it would move
/// focus to a row that is not on screen.
#[test]
fn the_attention_count_and_the_visible_list_see_the_same_rows() {
    // Twelve rows in one bucket, so the preview cut bites, plus a drained
    // row in a second bucket so a closed band has something in it. Half
    // the twelve carry an approval, so the count is a PROPER SUBSET of the
    // visible list at every step: a walk that read one row too many or too
    // few would otherwise be hidden behind a predicate that answered the
    // same way for everything.
    let ids_in: Vec<(u64, u64, u64)> = (0..12).map(|i| (10 + i, 1, i)).collect();
    let mut st = with(&[1, 2], &ids_in);
    for row in &mut st.daemon.sessions {
        if row.id().0 % 2 == 0 {
            row.info.hint = Some(vitrum_proto::AgentHint {
                state: HintState::Approval,
                label: Some("approve this write?".to_string()),
                received_at_ms: NOW - 60_000,
            });
        }
    }
    st.daemon.sessions.push(
        row(99)
            .project(2)
            .waiting(Some(true))
            .created_at_ms(NOW - HOUR)
            .last_activity_ms(NOW - HOUR)
            .build(),
    );
    st.settle(&[SessionId(99)], NOW);

    // The specification, spelled out over the PUBLIC visible list: the
    // count is the rows on screen that want the operator. Whatever
    // `attention_count_of` does internally, it has to agree with this.
    let agrees = |st: &UiState, what: &str| -> (usize, usize) {
        let tree = st.tree(clock());
        let visible = st.window.visible_ids_of(&tree);
        let policy = st.daemon.policy();
        let want = visible
            .iter()
            .filter(|id| {
                st.daemon
                    .row(**id)
                    .is_some_and(|row| inbox::wants_operator(row, clock(), policy))
            })
            .count();
        let counted = st.window.attention_count_of(&st.daemon, &tree, clock());
        assert_eq!(
            counted, want,
            "{what}: the jump affordance counted {counted} of the rows on \
             screen, {visible:?}, and {want} of them want the operator"
        );
        (visible.len(), counted)
    };

    // The preview cut alone: the newest eight of twelve, four of them
    // asking for approval.
    assert_eq!(agrees(&st, "at rest"), (inbox::PREVIEW_LIMIT, 4));
    // A band opened underneath it. The drained row is on screen and wants
    // nothing, which is the case that separates the two numbers.
    st.toggle_section(pk(2), Section::Settled);
    assert_eq!(
        agrees(&st, "with the Done shelf open"),
        (inbox::PREVIEW_LIMIT + 1, 4)
    );
    // The preview cut lifted: four more rows, two of them asking.
    st.toggle_preview(pk(1));
    assert_eq!(agrees(&st, "with the inbox fully shown"), (13, 6));
    // A whole bucket shut. Every approval was in it.
    st.window.collapsed.insert(pk(1));
    assert_eq!(agrees(&st, "with the first bucket collapsed"), (1, 0));
    // Both shut: nothing on screen, and nothing to jump to.
    st.window.collapsed.insert(pk(2));
    assert_eq!(agrees(&st, "with everything collapsed"), (0, 0));
}

/// Revealing must open all three things that can hide a row. Opening only
/// the project leaves the jump key moving focus into a collapsed band,
/// which is indistinguishable from the key doing nothing.
#[test]
fn reveal_opens_the_project_the_band_and_the_preview() {
    let ids_in: Vec<(u64, u64, u64)> = (0..12).map(|i| (10 + i, 1, i)).collect();
    let mut st = with(&[1], &ids_in);
    st.daemon.sessions[0].info.attention.waiting = Some(true);
    st.settle(&[SessionId(10)], NOW);
    st.window.collapsed.insert(pk(1));
    assert!(st.visible_ids(clock()).is_empty());

    st.reveal(SessionId(10), clock());
    assert!(!st.window.collapsed.contains(&pk(1)));
    assert!(st.section_open(pk(1), Section::Settled));
    assert!(st.window.preview_expanded(pk(1)));
    assert!(st.visible_ids(clock()).contains(&SessionId(10)));
}

// ---- Traversal --------------------------------------------------------

/// The jump key walks only rows the operator is actually blocking. At
/// twenty agents "not working" matches almost everything, so a broader
/// predicate makes the first press land somewhere useless.
#[test]
fn the_jump_key_visits_only_rows_that_want_the_operator() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2), (13, 1, 3)]);
    // 11 is blocked on an approval, 13 failed, the rest are working.
    st.daemon.sessions[1].info.attention.waiting = Some(true);
    st.daemon.sessions[1].info.hint = Some(vitrum_proto::AgentHint {
        state: HintState::Input,
        label: None,
        received_at_ms: NOW,
    });
    st.daemon.sessions[3].info.status = SessionStatus::Exited { code: Some(1) };
    st.daemon.sessions[3].info.attention.failed = true;
    st.daemon.sessions[3].info.unread = true;

    assert_eq!(st.attention_count(clock()), 2);
    st.window.focused = None;
    let first = st.attention_target(clock(), Direction::Next).unwrap();
    st.window.focused = Some(first);
    let second = st.attention_target(clock(), Direction::Next).unwrap();
    assert_ne!(first, second);
    let mut got = vec![first.0, second.0];
    got.sort_unstable();
    assert_eq!(got, vec![11, 13]);
}

/// The jump must never return the row it started on, even when that row is
/// the only match. "Go to the next one that wants me" has to move or report
/// that there is nowhere to move.
#[test]
fn the_jump_key_never_returns_the_row_it_started_on() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.daemon.sessions[0].info.attention.waiting = Some(true);
    st.daemon.sessions[0].info.hint = Some(vitrum_proto::AgentHint {
        state: HintState::Approval,
        label: None,
        received_at_ms: NOW,
    });
    st.window.focused = Some(SessionId(10));
    assert_eq!(st.attention_count(clock()), 1);
    assert_eq!(st.attention_target(clock(), Direction::Next), None);
    assert_eq!(st.attention_target(clock(), Direction::Previous), None);
}

/// A parked row stays off the queue even when it would otherwise qualify.
/// Jumping to it would undo the operator's decision to park it on their
/// behalf.
#[test]
fn the_jump_key_skips_parked_rows() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.daemon.sessions[0].info.status = SessionStatus::Exited { code: Some(1) };
    st.daemon.sessions[0].info.attention.failed = true;
    st.snooze(&[SessionId(10)], NOW + HOUR, NOW);
    st.toggle_section(pk(1), Section::Snoozed);

    assert!(st.visible_ids(clock()).contains(&SessionId(10)));
    assert_eq!(st.attention_count(clock()), 0);
    assert_eq!(st.attention_target(clock(), Direction::Next), None);
}

/// Plain stepping clamps at both ends. Holding the down arrow at the bottom
/// of a twenty-row list must stop, not spin back to the top.
#[test]
fn stepping_clamps_at_both_ends() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    // Newest first, so the visible order is 12, 11, 10.
    st.window.focused = Some(SessionId(12));
    assert_eq!(
        st.step_target(clock(), Direction::Previous),
        None,
        "the first row has nothing above it"
    );
    assert_eq!(
        st.step_target(clock(), Direction::Next),
        Some(SessionId(11))
    );
    st.window.focused = Some(SessionId(10));
    assert_eq!(st.step_target(clock(), Direction::Next), None);
}

// ---- Selection and the context menu -----------------------------------

/// Shift-click selects the inclusive range from the anchor, in screen
/// order, and leaves the anchor where it was so repeated shift-clicks
/// pivot around the row you started on.
#[test]
fn shift_click_selects_the_visible_range_from_the_anchor() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2), (13, 1, 3)]);
    // Visible order is newest first: 13, 12, 11, 10.
    st.click_row(SessionId(12), Click::Plain, clock());
    st.click_row(SessionId(10), Click::Range, clock());
    assert_eq!(
        st.window.selection.ordered(&st.visible_ids(clock())),
        vec![SessionId(12), SessionId(11), SessionId(10)]
    );
    assert_eq!(st.window.selection.anchor(), Some(SessionId(12)));

    // Narrowing from the same anchor rather than from the last click.
    st.click_row(SessionId(11), Click::Range, clock());
    assert_eq!(
        st.window.selection.ordered(&st.visible_ids(clock())),
        vec![SessionId(12), SessionId(11)]
    );
}

/// A range must never cross into a band the operator cannot see. Bulk
/// actions on invisible rows are how you close the wrong nineteen sessions.
#[test]
fn a_range_never_reaches_a_row_inside_a_collapsed_band() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    st.daemon.sessions[2].info.attention.waiting = Some(true);
    st.settle(&[SessionId(12)], NOW);
    assert!(!st.visible_ids(clock()).contains(&SessionId(12)));

    st.click_row(SessionId(11), Click::Plain, clock());
    st.click_row(SessionId(12), Click::Range, clock());
    assert_eq!(
        st.window.selection.iter().collect::<Vec<_>>(),
        vec![SessionId(12)],
        "an unreachable endpoint falls back to selecting just that row"
    );
}

/// Ctrl-click toggles one row and moves the anchor, in both directions.
#[test]
fn ctrl_click_toggles_one_row_and_moves_the_anchor() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.click_row(SessionId(10), Click::Toggle, clock());
    st.click_row(SessionId(11), Click::Toggle, clock());
    assert_eq!(st.window.selection.len(), 2);
    st.click_row(SessionId(10), Click::Toggle, clock());
    assert_eq!(st.window.selection.len(), 1);
    assert_eq!(st.window.selection.anchor(), Some(SessionId(10)));
}

/// A right-click inside a multi-selection acts on the whole selection; one
/// outside it acts on the single row. That is what every file manager does
/// and what stops a stray right-click from operating on nineteen sessions.
#[test]
fn a_right_click_outside_the_selection_acts_on_one_row() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    st.click_row(SessionId(12), Click::Plain, clock());
    st.click_row(SessionId(11), Click::Range, clock());
    assert_eq!(st.window.selection.len(), 2);

    assert_eq!(
        st.menu_targets(SessionId(11), clock()),
        vec![SessionId(12), SessionId(11)]
    );
    assert_eq!(st.menu_targets(SessionId(10), clock()), vec![SessionId(10)]);
}

/// Bulk labels carry their count, straight from the model. A bulk action
/// with no visible count is how you close nineteen sessions meaning to
/// close one.
#[test]
fn the_bulk_menu_labels_carry_their_counts() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    st.select_all_visible(clock());
    let items = st.menu_items(SessionId(11), clock());
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();

    assert!(labels.contains(&"Mark unread (3)"), "{labels:?}");
    assert!(labels.contains(&"Snooze (3)"), "{labels:?}");
    assert!(labels.contains(&"Settle (3)"), "{labels:?}");
    assert!(labels.contains(&"Close (3, 3 running)"), "{labels:?}");
    assert!(
        items
            .iter()
            .find(|i| i.action == MenuAction::Terminate)
            .is_some_and(|i| i.danger),
        "closing three live children is destructive and must be marked"
    );
}

/// A bulk snooze must refuse outright when any row in the selection is
/// blocked, and name how many. Half-applying a bulk action is impossible to
/// reason about.
#[test]
fn the_bulk_snooze_is_refused_when_any_row_is_blocked_and_says_how_many() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    st.daemon.sessions[0].info.attention.waiting = Some(true);
    st.daemon.sessions[0].info.hint = Some(vitrum_proto::AgentHint {
        state: HintState::Approval,
        label: None,
        received_at_ms: NOW,
    });
    st.select_all_visible(clock());

    let items = st.menu_items(SessionId(11), clock());
    let head = items
        .iter()
        .find(|i| i.action == MenuAction::SnoozeHeader)
        .expect("bulk snooze is shown");
    assert!(!head.enabled);
    assert_eq!(head.hint.as_deref(), Some("1 blocked on you"));
    assert!(
        !items
            .iter()
            .any(|i| matches!(i.action, MenuAction::Snooze(_)))
    );
}

/// A selection of rows that are all parked offers Wake instead of Snooze,
/// because that is the only useful thing to do with them.
#[test]
fn a_fully_parked_selection_offers_wake_instead_of_snooze() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    st.snooze(&[SessionId(10), SessionId(11)], NOW + HOUR, NOW);
    st.toggle_section(pk(1), Section::Snoozed);
    st.select_all_visible(clock());

    let items = st.menu_items(SessionId(10), clock());
    assert!(items.iter().any(|i| i.label == "Wake (2)"));
    assert!(!items.iter().any(|i| i.action == MenuAction::SnoozeHeader));
}

/// An empty selection produces no bulk menu at all, and a menu on a session
/// that vanished produces nothing rather than an empty box that still
/// swallows the next click.
#[test]
fn a_menu_on_a_vanished_session_is_empty() {
    let st = with(&[1], &[(10, 1, 0)]);
    assert!(st.menu_items(SessionId(404), clock()).is_empty());
}

// ---- Rollup -----------------------------------------------------------

/// A collapsed project header shows the most urgent state among its inbox
/// rows and the per-state counts. Settled rows are counted but do not vote
/// for the indicator, or a project whose only activity finished yesterday
/// would wear a permanent dot.
#[test]
fn the_project_rollup_reports_the_most_urgent_state_and_the_counts() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2), (13, 1, 3)]);
    st.daemon.sessions[0].info.attention.waiting = Some(true);
    st.daemon.sessions[0].info.hint = Some(vitrum_proto::AgentHint {
        state: HintState::Approval,
        label: None,
        received_at_ms: NOW,
    });
    st.daemon.sessions[1].info.status = SessionStatus::Exited { code: Some(1) };
    st.daemon.sessions[1].info.attention.failed = true;
    // Unseen: an exit nobody has looked at is what holds a failure in the
    // inbox. Without it the model reads the exit as acknowledged and
    // settles the row, which is correct but is a different test.
    st.daemon.sessions[1].info.unread = true;
    st.daemon.sessions[3].info.attention.waiting = Some(true);
    st.settle(&[SessionId(13)], NOW);

    let g = st.tree(clock());
    let rollup = g[0]
        .bands
        .rollup
        .as_ref()
        .expect("a named project rolls up");
    assert_eq!(rollup.indicator, Some(SidebarStatus::Approval));
    assert_eq!(rollup.counts.approval, 1);
    assert_eq!(rollup.counts.failed, 1);
    assert_eq!(rollup.counts.working, 1);
    assert_eq!(rollup.settled, 1);
    assert_eq!(rollup.total, 4);
    assert_eq!(
        crate::inbox::rollup_chips(rollup),
        vec![
            (SidebarStatus::Approval, 1),
            (SidebarStatus::Failed, 1),
            (SidebarStatus::Working, 1),
        ]
    );
}

// ---- Filtering --------------------------------------------------------

/// A filter must match on title, command, cwd and branch. Those are the
/// four things a user remembers about a session; matching only the title
/// makes the filter useless for twenty sessions that all say "claude".
#[test]
fn filter_matches_every_searchable_field() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2), (13, 1, 3)]);
    st.daemon.sessions[0].info.title = "review the parser".into();
    st.daemon.sessions[1].info.command = "opencode".into();
    st.daemon.sessions[2].info.cwd = "/src/kernel-notes".into();
    st.daemon.sessions[3].info.git_branch = Some("perf/ftrace".into());

    for (query, want) in [
        ("parser", 10),
        ("opencode", 11),
        ("kernel-notes", 12),
        ("ftrace", 13),
    ] {
        st.window.filter = query.to_string();
        assert_eq!(
            st.visible_ids(clock()),
            vec![SessionId(want)],
            "query {query:?}"
        );
    }
}

/// Filtering must be case-insensitive and must ignore surrounding
/// whitespace, because a query is typed under time pressure and pasting a
/// branch name drags a space along with it.
#[test]
fn filter_ignores_case_and_surrounding_whitespace() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.daemon.sessions[0].info.title = "Claude Code".into();
    for query in ["claude", "CLAUDE", "  ClAuDe  ", "code"] {
        st.window.filter = query.to_string();
        assert_eq!(
            st.tree(clock()).iter().map(|g| g.len()).sum::<usize>(),
            1,
            "query {query:?} should match"
        );
    }
}

/// An empty or whitespace-only filter must show everything. A query of one
/// space would otherwise blank the sidebar with no visible cause.
#[test]
fn blank_filter_shows_everything() {
    let mut st = with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    for query in ["", "   ", "\t"] {
        st.window.filter = query.to_string();
        assert_eq!(st.tree(clock()).iter().map(|g| g.len()).sum::<usize>(), 2);
        assert!(!st.filter_matched_nothing(clock()), "query {query:?}");
    }
}

/// A filter that empties a project must hide the project header too, or
/// the result of a search is a wall of empty group headers with the one
/// match buried among them.
#[test]
fn filter_hides_projects_with_no_matches() {
    let mut st = with(&[1, 2], &[(10, 1, 0), (11, 2, 1)]);
    st.daemon.sessions[0].info.title = "keep me".into();
    st.daemon.sessions[1].info.title = "hide me".into();
    st.window.filter = "keep".into();

    let g = st.tree(clock());
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].project.unwrap().id, ProjectId(1));
    assert!(!st.filter_matched_nothing(clock()));
}

/// A filter matching nothing must be reported as such, so the sidebar can
/// say "no match for X" instead of rendering as if there were no sessions
/// at all. Those are different situations and only one is the user's doing.
#[test]
fn filter_with_no_matches_is_distinguishable_from_no_sessions() {
    let mut st = with(&[1], &[(10, 1, 0)]);
    st.window.filter = "zzzznope".into();
    assert!(st.tree(clock()).is_empty());
    assert!(st.filter_matched_nothing(clock()));

    let empty = UiState::default();
    assert!(empty.tree(clock()).is_empty());
    assert!(
        !empty.filter_matched_nothing(clock()),
        "an empty server is not a failed search"
    );
}

/// Filtering must not reorder the rows it keeps. The static inbox order is
/// the same order whether or not a query is narrowing it.
#[test]
fn filter_preserves_the_static_order() {
    let mut st = with(&[1], &[(10, 1, 100), (11, 1, 200), (12, 1, 300)]);
    for r in &mut st.daemon.sessions {
        r.info.title = format!("agent {}", r.id().0);
    }
    st.daemon.sessions[0].info.attention.failed = true;
    st.window.filter = "agent".into();
    assert_eq!(ids(&st.tree(clock())[0].bands.active), vec![12, 11, 10]);
}

// ---- Attention rail (still the protocol's, not the model's) ------------

/// The rail must pick the most urgent signal when several are set,
/// matching the server's own priority ladder. Showing "silent" on a session
/// that also failed buries the one fact that matters, and the rail is a
/// single border so there is no room to show both.
#[test]
fn attention_rail_reports_the_most_urgent_signal() {
    let all = Attention {
        bell: true,
        idle_ms: 10 * IDLE_ATTENTION_MS,
        failed: true,
        waiting: Some(true),
    };
    assert_eq!(
        attention_modifier(&all),
        Some("rg-session--attention-failed")
    );
    assert_eq!(attention_label(&all), "failed - needs you");

    let belled = Attention {
        bell: true,
        idle_ms: 10 * IDLE_ATTENTION_MS,
        failed: false,
        waiting: Some(false),
    };
    assert_eq!(
        attention_modifier(&belled),
        Some("rg-session--attention-bell")
    );
    assert_eq!(attention_label(&belled), "rang the bell - needs you");

    let silent = Attention {
        idle_ms: 45_000,
        ..Attention::default()
    };
    assert_eq!(
        attention_modifier(&silent),
        Some("rg-session--attention-idle")
    );
    assert_eq!(attention_label(&silent), "silent for 45s - needs you");
}

/// A working session must have no marker at all. A permanently lit
/// indicator on every row trains people to ignore the indicator, which is
/// exactly the failure the `unseen` qualifier on `idle_ms` exists to avoid.
#[test]
fn working_sessions_carry_no_attention_marker() {
    assert_eq!(attention_modifier(&Attention::default()), None);
    assert_eq!(
        attention_modifier(&Attention {
            idle_ms: IDLE_ATTENTION_MS - 1,
            ..Attention::default()
        }),
        None
    );
}

/// One millisecond under the idle threshold must still read as Working on a
/// platform that cannot probe. At the threshold it flips to an INFERRED
/// Ready, which the pill has to mark as uncertain rather than assert.
#[test]
fn the_idle_threshold_flips_an_unprobeable_row_to_an_inferred_ready() {
    let just_under = row(1)
        .waiting(None)
        .idle_ms(IDLE_ATTENTION_MS - 1)
        .unread(true)
        .build();
    assert_eq!(just_under.status(), SidebarStatus::Working);

    let at = row(1)
        .waiting(None)
        .idle_ms(IDLE_ATTENTION_MS)
        .unread(true)
        .build();
    let resolved = at.resolve_status();
    assert_eq!(resolved.status, SidebarStatus::Ready);
    assert!(
        resolved.source.is_inferred(),
        "a status derived from silence is a guess and the UI must say so"
    );
}

// ---- Status labels and chrome -----------------------------------------

/// A clean exit, a non-zero exit and a signalled exit must read differently,
/// and the signalled case must not be the bare word "signalled".
///
/// "exited 137" (an OOM kill reported through the shell convention) and
/// "exited 1" mean very different things to someone debugging an agent, and
/// the old wording for a signalled child said only "signalled": an operator
/// could not tell a Ctrl+C from a crash, and could not tell that the number
/// had been lost rather than never existed.
#[test]
fn status_labels_distinguish_clean_non_zero_and_signalled_exits() {
    assert_eq!(status_label(&SessionStatus::Starting), "starting");
    assert_eq!(status_label(&SessionStatus::Running), "running");
    assert_eq!(
        status_label(&SessionStatus::Exited { code: Some(0) }),
        "exited 0"
    );
    assert_eq!(
        status_label(&SessionStatus::Exited { code: Some(137) }),
        "exited 137"
    );
    assert_eq!(
        status_label(&SessionStatus::Exited { code: None }),
        "killed by a signal (the number is not carried on the wire)"
    );
}

/// An abnormal Windows termination must decode, not appear as a negative int.
///
/// The daemon reports an NTSTATUS-shaped code verbatim, so a bare `{code}`
/// format renders an access violation as `exited -1073741819`, which tells an
/// operator nothing. Routing through `vitrum_fmt::exit` is what decodes it.
#[test]
fn a_windows_abnormal_exit_code_is_decoded_rather_than_printed_raw() {
    assert_eq!(
        status_label(&SessionStatus::Exited {
            code: Some(0xc000_0005u32 as i32)
        }),
        "exited 0xc0000005 (access violation)"
    );
}

/// The drag must clamp to the stylesheet's own min and max. Storing an
/// unclamped width lets the drag accumulate thousands of pixels of slack
/// that the user then has to drag back through before the edge moves.
#[test]
fn sidebar_width_clamps_to_the_stylesheet_bounds() {
    let mut st = UiState::default();
    // `set_sidebar_width_in` is the one the resizer calls; the unbounded
    // variant it replaced had no production caller, so this asserted a
    // clamp the product never reached. A window wide enough that the 32%
    // fraction is not the binding limit isolates the stylesheet bounds.
    let roomy = 4000.0;
    st.window.set_sidebar_width_in(10.0, roomy);
    assert_eq!(st.window.sidebar_width, SIDEBAR_MIN_PX);
    st.window.set_sidebar_width_in(10_000.0, roomy);
    assert_eq!(st.window.sidebar_width, SIDEBAR_MAX_PX);
    st.window.set_sidebar_width_in(300.0, roomy);
    assert_eq!(st.window.sidebar_width, 300.0);
}

/// Only a failed connection offers retry. Offering it while connected or in
/// fixture mode invites a click that tears down a working socket.
#[test]
fn only_failures_are_retryable() {
    assert!(
        ConnState::Failed {
            detail: "refused".into()
        }
        .is_retryable()
    );
    assert!(!ConnState::Connecting.is_retryable());
    assert!(!ConnState::Fixture.is_retryable());
    let live = ConnState::Live {
        server_version: "1".into(),
    };
    assert!(!live.is_retryable());
}

/// Fixture mode must announce itself in the banner. The whole point of the
/// flag is that fake data can never be mistaken for a live server.
#[test]
fn fixture_banner_says_it_is_fixture_data() {
    assert_eq!(
        ConnState::Fixture.banner_text("ws://127.0.0.1:7737"),
        "FIXTURE DATA - no server connection"
    );
    assert_eq!(
        ConnState::Fixture.banner_class(),
        "rg-sidebar__status rg-sidebar__status--fixture"
    );
}

/// A failed banner must quote the reason. "disconnected" alone does not
/// distinguish "server not running" from "server crashed mid-session".
#[test]
fn failed_banner_quotes_the_reason() {
    assert_eq!(
        ConnState::Failed {
            detail: "cannot reach ws://127.0.0.1:7737".into()
        }
        .banner_text("ws://127.0.0.1:7737"),
        "disconnected - cannot reach ws://127.0.0.1:7737"
    );
}
// ---- The daemon / window split ---------------------------------------

/// A daemon with `projects` and `sessions`, and no windows attached.
fn daemon_with(projects: &[u64], sessions: &[(u64, u64, u64)]) -> DaemonState {
    let mut d = DaemonState {
        projects: projects
            .iter()
            .map(|p| project(*p, &format!("p{p}")))
            .collect(),
        sessions: sessions
            .iter()
            .map(|(id, pid, at)| {
                row(*id)
                    .project(*pid)
                    .waiting(Some(false))
                    .created_at_ms(NOW - HOUR + at)
                    .last_activity_ms(NOW - HOUR + at)
                    .build()
            })
            .collect(),
        ..DaemonState::default()
    };
    let infos: Vec<SessionInfo> = d.sessions.iter().map(|r| r.info.clone()).collect();
    d.workspaces.adopt(infos.iter());
    d
}

/// File a session into a workspace by id, panicking on a typo'd test.
fn place(d: &mut DaemonState, id: u64, ws: WorkspaceId) {
    let info = d
        .session(SessionId(id))
        .expect("test session exists")
        .clone();
    d.workspaces.assign(&info, ws).expect("workspace exists");
}

/// The badge count must span every workspace, not the one a window is on.
///
/// A dock badge or launcher entry is one per process, so counting a
/// window's workspace would leave an agent that failed in a workspace
/// nobody is looking at completely unreported. Those are exactly the ones
/// a badge exists for: the operator is not looking at them, which is why
/// they need telling.
#[test]
fn the_badge_count_spans_every_workspace() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    let scratch = daemon.workspaces.create("Scratch").unwrap();
    // One failure in the workspace on screen, one filed out of sight, and
    // one healthy session that must not be counted anywhere.
    daemon.row_mut(SessionId(10)).unwrap().info.status = SessionStatus::Exited { code: Some(1) };
    daemon.row_mut(SessionId(11)).unwrap().info.status = SessionStatus::Exited { code: Some(1) };
    place(&mut daemon, 11, scratch);

    assert_eq!(
        daemon.attention_total(clock()),
        2,
        "the badge must count the failure filed in a workspace nobody has \
         on screen, and must not count the healthy session"
    );
    // Moving a session between workspaces changes nothing: the number is
    // about the process, and filing is about a window.
    place(&mut daemon, 10, scratch);
    assert_eq!(
        daemon.attention_total(clock()),
        2,
        "filing a session elsewhere must not change a per-process count"
    );

    daemon.row_mut(SessionId(11)).unwrap().info.status = SessionStatus::Running;
    assert_eq!(
        daemon.attention_total(clock()),
        1,
        "a recovered session must come back off the badge"
    );
}

/// Two windows on ONE daemon must be able to look at different workspaces
/// at the same time without touching each other's tabs, focus or filter.
///
/// This is the whole reason the split exists. Before it, `focused` and
/// `sessions` sat in one struct, so a second window meant a second copy of
/// the session list and multi-window was unimplementable.
#[test]
fn two_windows_view_different_workspaces_without_interfering() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    let review = daemon.workspaces.create("Review").unwrap();
    place(&mut daemon, 12, review);

    let mut left = window_on(DEFAULT_WORKSPACE);
    let mut right = window_on(review);

    left.open(&mut daemon, SessionId(10), NOW);
    right.open(&mut daemon, SessionId(12), NOW);
    left.filter = "session 1".into();

    assert_eq!(left.tabs, vec![SessionId(10)]);
    assert_eq!(right.tabs, vec![SessionId(12)]);
    assert_eq!(left.focused, Some(SessionId(10)));
    assert_eq!(right.focused, Some(SessionId(12)));
    assert_eq!(right.filter, "", "a filter is one window's typing");

    // Each window sees only its own workspace's rows.
    let mine = |w: &WindowState, d: &DaemonState| -> Vec<u64> {
        w.visible_ids(d, clock()).iter().map(|s| s.0).collect()
    };
    assert_eq!(mine(&left, &daemon), vec![11, 10]);
    assert_eq!(mine(&right, &daemon), vec![12]);

    // Closing the left window's tab must not disturb the right one.
    left.close_tab(SessionId(10));
    assert!(left.tabs.is_empty());
    assert_eq!(right.tabs, vec![SessionId(12)]);
    assert_eq!(right.focused, Some(SessionId(12)));
}

/// One fold, N reactions. The socket delivers a message once however many
/// windows are open, so the daemon half must run once and each window must
/// decide for itself. Only the window focused on the session paints the
/// backfill; the other one must not, or two terminals repaint from the
/// same bytes.
#[test]
fn one_broadcast_reaches_every_window_and_only_one_paints() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let mut left = window_on(DEFAULT_WORKSPACE);
    let mut right = window_on(DEFAULT_WORKSPACE);
    left.open(&mut daemon, SessionId(10), NOW);
    right.open(&mut daemon, SessionId(11), NOW);

    let broadcast = daemon.apply(ServerMsg::ScrollbackChunk {
        session: SessionId(10),
        from_seq: 100,
        data: b"hello".to_vec(),
        more: false,
    });
    assert_eq!(
        left.receive(&mut daemon, &broadcast, NOW),
        Reaction::Backfill {
            session: SessionId(10),
            from_seq: 100,
            resume_seq: 105,
            bytes: b"hello".to_vec(),
            jump_seq: None,
            keep_view: false,
            more: false,
        }
    );
    assert_eq!(
        right.receive(&mut daemon, &broadcast, NOW),
        Reaction::None,
        "a window not focused on the session must not paint its history"
    );
}

/// A session-scoped error must not flash in a window whose sidebar cannot
/// show that session. Naming a session the operator cannot find is worse
/// than saying nothing.
#[test]
fn a_session_error_flashes_only_where_the_session_is_visible() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let other = daemon.workspaces.create("Other").unwrap();
    let mut here = window_on(DEFAULT_WORKSPACE);
    let mut elsewhere = window_on(other);

    let broadcast = daemon.apply(ServerMsg::error(Some(SessionId(10)), "spawn failed"));
    here.receive(&mut daemon, &broadcast, NOW);
    elsewhere.receive(&mut daemon, &broadcast, NOW);

    assert_eq!(
        here.flash.as_ref().map(|f| f.text.clone()),
        Some("session 10: spawn failed".to_string())
    );
    assert_eq!(elsewhere.flash, None);
}

/// An error with no session is everyone's problem and flashes everywhere.
#[test]
fn an_unscoped_error_flashes_in_every_window() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let other = daemon.workspaces.create("Other").unwrap();
    let mut here = window_on(DEFAULT_WORKSPACE);
    let mut elsewhere = window_on(other);
    let broadcast = daemon.apply(ServerMsg::error(None, "socket closed"));
    here.receive(&mut daemon, &broadcast, NOW);
    elsewhere.receive(&mut daemon, &broadcast, NOW);
    assert!(here.flash.is_some());
    assert!(elsewhere.flash.is_some());
}

/// A full `Sessions` snapshot replaces the DAEMON half and must leave every
/// window's own state alone.
///
/// The daemon pushes a snapshot on every reconnect. If that reset the
/// sidebar width, reopened collapsed groups, cleared the filter or moved
/// focus, a flaky network would rearrange the operator's window several
/// times a minute.
#[test]
fn a_daemon_snapshot_replaces_daemon_state_without_clobbering_the_window() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let mut window = window_on(DEFAULT_WORKSPACE);
    window.open(&mut daemon, SessionId(11), NOW);
    window.open(&mut daemon, SessionId(10), NOW);
    window.sidebar_width = 400.0;
    window.sidebar_collapsed = true;
    window.filter = "session".into();
    window.collapsed.insert(pk(1));
    window.sections_expanded.insert((pk(1), Section::Settled));
    window.workspace_bar_open = true;
    // An operator ruling, which lives daemon-side and must also survive.
    daemon.snooze(&[SessionId(11)], NOW + HOUR, NOW);

    let fresh: Vec<SessionInfo> = [10u64, 11, 12]
        .iter()
        .map(|id| {
            row(*id)
                .project(1)
                .created_at_ms(NOW - HOUR + id)
                .build()
                .info
        })
        .collect();
    let broadcast = daemon.apply(ServerMsg::Sessions { sessions: fresh });
    assert_eq!(broadcast, Broadcast::SessionsChanged);
    window.receive(&mut daemon, &broadcast, NOW);

    assert_eq!(daemon.sessions.len(), 3, "the daemon half was replaced");
    assert_eq!(window.sidebar_width, 400.0);
    assert!(window.sidebar_collapsed);
    assert_eq!(window.filter, "session");
    assert!(window.collapsed.contains(&pk(1)));
    assert!(
        window
            .sections_expanded
            .contains(&(pk(1), Section::Settled))
    );
    assert!(window.workspace_bar_open);
    assert_eq!(window.tabs, vec![SessionId(11), SessionId(10)]);
    assert_eq!(window.focused, Some(SessionId(10)));
    assert!(
        daemon.row(SessionId(11)).unwrap().snooze.is_some(),
        "a snapshot must never un-park a row the operator parked"
    );
}

/// The operator axis is DAEMON state, so a ruling made through one window
/// is immediately true in the other. Two windows disagreeing about whether
/// a row is parked would give one park two wakeups.
#[test]
fn a_snooze_in_one_window_is_a_snooze_in_the_other() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let left = window_on(DEFAULT_WORKSPACE);
    let right = window_on(DEFAULT_WORKSPACE);

    daemon.snooze(&[SessionId(10)], NOW + HOUR, NOW);

    for w in [&left, &right] {
        let tree = w.tree(&daemon, clock());
        assert_eq!(
            ids(tree[0].section(Section::Snoozed)),
            vec![10],
            "one park, one truth, however many windows are looking"
        );
        assert!(tree[0].section(Section::Active).is_empty());
    }
}

// ---- Workspaces -------------------------------------------------------

/// A fresh install has exactly one workspace and it is not id zero, so a
/// zero read out of a corrupt file is always detectable. It is also
/// NAMELESS: the operator never created it, so seeding it with a name
/// would put a decision nobody made into the loudest text in the window.
#[test]
fn a_fresh_workspace_set_holds_exactly_one_nameless_workspace() {
    let set = WorkspaceSet::default();
    assert_eq!(set.len(), 1);
    assert_eq!(ws_ids(&set), vec![DEFAULT_WORKSPACE]);
    assert_eq!(DEFAULT_WORKSPACE, WorkspaceId(1));
    let first = set.get(DEFAULT_WORKSPACE).unwrap();
    assert_eq!(first.name, "", "the first workspace carries no chosen name");
    assert_eq!(
        first.display_name(),
        "Workspace",
        "a nameless workspace draws the bare noun, never a seeded label"
    );
    assert_ne!(
        first.display_name(),
        "Default",
        "the refused seeded name must not come back through the fallback"
    );
    assert_eq!(set.intake(), DEFAULT_WORKSPACE);
}

/// A NEW workspace is genuinely blank. This is the definition the user
/// gave: creating one gives you an empty sidebar and you then put sessions
/// in it. A workspace that inherited rows would be a filter, not a context.
#[test]
fn a_new_workspace_starts_blank_and_a_session_appears_only_in_its_own() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let scratch = daemon.workspaces.create("Scratch").unwrap();

    let here = window_on(DEFAULT_WORKSPACE);
    let there = window_on(scratch);
    assert_eq!(here.visible_ids(&daemon, clock()).len(), 2);
    assert!(
        there.tree(&daemon, clock()).iter().all(|g| g.is_empty()),
        "a workspace you just made holds nothing"
    );

    place(&mut daemon, 11, scratch);
    assert_eq!(
        here.visible_ids(&daemon, clock()),
        vec![SessionId(10)],
        "a moved session leaves the workspace it came from"
    );
    assert_eq!(there.visible_ids(&daemon, clock()), vec![SessionId(11)]);
    assert_eq!(daemon.workspaces.session_count(scratch), 1);
    assert_eq!(
        daemon.workspace_of(SessionId(11)),
        Some(scratch),
        "a session belongs to exactly one workspace"
    );
}

/// Deleting a workspace that still holds sessions must be refused, and the
/// refusal must say how many, because the alternatives are destroying the
/// operator's filing silently or dumping rows somewhere they never asked.
#[test]
fn deleting_a_non_empty_workspace_is_refused_with_a_count() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let scratch = daemon.workspaces.create("Scratch").unwrap();
    place(&mut daemon, 10, scratch);
    place(&mut daemon, 11, scratch);

    assert_eq!(
        daemon.workspaces.delete(scratch),
        Err(WorkspaceError::NotEmpty { sessions: 2 })
    );
    assert_eq!(
        WorkspaceError::NotEmpty { sessions: 2 }.to_string(),
        "still holds 2 sessions; move them out first"
    );
    assert_eq!(
        WorkspaceError::NotEmpty { sessions: 1 }.to_string(),
        "still holds 1 session; move them out first",
        "the count is in the sentence, so it has to agree with the verb"
    );
    assert!(daemon.workspaces.contains(scratch));

    daemon
        .move_to_workspace(&[SessionId(10), SessionId(11)], DEFAULT_WORKSPACE)
        .unwrap();
    assert_eq!(daemon.workspaces.delete(scratch), Ok(()));
    assert!(!daemon.workspaces.contains(scratch));
}

/// The last workspace can never be deleted: zero workspaces has no
/// coherent sidebar and nothing to fall back to.
#[test]
fn the_last_workspace_cannot_be_deleted_even_when_empty() {
    let mut set = WorkspaceSet::default();
    assert_eq!(
        set.delete(DEFAULT_WORKSPACE),
        Err(WorkspaceError::LastWorkspace)
    );
    assert_eq!(set.delete(WorkspaceId(404)), Err(WorkspaceError::Unknown));
    assert_eq!(set.len(), 1);
}

/// A blank name is refused everywhere a name is taken. An unnamed
/// workspace has nothing to draw in the switcher and nothing to click.
#[test]
fn blank_names_are_refused_and_names_are_trimmed() {
    let mut set = WorkspaceSet::default();
    assert_eq!(set.create("   "), Err(WorkspaceError::BlankName));
    let id = set.create("  Review  ").unwrap();
    assert_eq!(set.get(id).unwrap().name, "Review");
    assert_eq!(set.rename(id, "\t\n"), Err(WorkspaceError::BlankName));
    assert_eq!(set.get(id).unwrap().name, "Review");
    assert_eq!(set.create_folder(id, ""), Err(WorkspaceError::BlankName));
}

/// The set names the next workspace, because uniqueness is a fact about
/// the set. Locks out "Default17": a UI counting its own button presses
/// appended a counter to a name that already had one.
#[test]
fn the_set_suggests_a_name_that_is_not_already_taken() {
    let mut set = WorkspaceSet::default();
    assert_eq!(set.suggested_name(), "Workspace 2");

    let two = set.create(&set.suggested_name().clone()).unwrap();
    assert_eq!(set.get(two).unwrap().name, "Workspace 2");
    assert_eq!(set.suggested_name(), "Workspace 3");

    // A hand-typed name occupying the next slot must be stepped over, not
    // duplicated.
    set.create("Workspace 3").unwrap();
    assert_eq!(set.suggested_name(), "Workspace 4");
    set.rename(two, "Workspace 4").unwrap();
    assert_eq!(set.suggested_name(), "Workspace 2");
}

/// Reordering is by position and refuses a target past the end rather than
/// silently clamping, because a drag that lands nowhere should not move
/// the item to a third place the operator did not pick.
#[test]
fn workspaces_reorder_by_position_and_refuse_a_target_past_the_end() {
    let mut set = WorkspaceSet::default();
    let b = set.create("B").unwrap();
    let c = set.create("C").unwrap();
    assert_eq!(ws_ids(&set), vec![DEFAULT_WORKSPACE, b, c]);

    set.move_to(c, 0).unwrap();
    assert_eq!(ws_ids(&set), vec![c, DEFAULT_WORKSPACE, b]);
    assert_eq!(set.first(), c);
    assert_eq!(set.move_to(c, 3), Err(WorkspaceError::OutOfRange));
    assert_eq!(ws_ids(&set), vec![c, DEFAULT_WORKSPACE, b]);
}

/// Deleting the workspace a window is showing must repoint that window
/// rather than leave it rendering nothing. A daemon-level delete cannot
/// reach into the windows, so each one reconciles.
#[test]
fn deleting_the_shown_workspace_repoints_the_window() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let scratch = daemon.workspaces.create("Scratch").unwrap();
    let mut window = window_on(scratch);
    assert!(window.tree(&daemon, clock()).iter().all(|g| g.is_empty()));

    daemon.workspaces.delete(scratch).unwrap();
    assert!(
        window.reconcile_workspace(&mut daemon, NOW),
        "the window had to move"
    );
    assert_eq!(window.workspace, DEFAULT_WORKSPACE);
    assert_eq!(window.visible_ids(&daemon, clock()), vec![SessionId(10)]);
    assert!(
        !window.reconcile_workspace(&mut daemon, NOW),
        "a window on a live workspace must not be disturbed"
    );
}

/// Switching workspace swaps the whole tab strip and brings it back on the
/// way home. A workspace is a virtual desktop, not a filter: leaving and
/// returning must find what you had open.
#[test]
fn switching_workspace_parks_the_tab_strip_and_restores_it() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let scratch = daemon.workspaces.create("Scratch").unwrap();
    place(&mut daemon, 11, scratch);

    let mut window = window_on(DEFAULT_WORKSPACE);
    window.open(&mut daemon, SessionId(10), NOW);
    window.filter = "leftover".into();

    window.set_workspace(&mut daemon, scratch, NOW).unwrap();
    assert!(
        window.tabs.is_empty(),
        "the other workspace's tabs are gone"
    );
    assert_eq!(window.focused, None);
    assert_eq!(
        window.filter, "",
        "a stale query would read as a broken switch"
    );
    assert_eq!(
        daemon.workspaces.intake(),
        scratch,
        "the next session lands where the operator is looking"
    );

    window.open(&mut daemon, SessionId(11), NOW);
    window
        .set_workspace(&mut daemon, DEFAULT_WORKSPACE, NOW)
        .unwrap();
    assert_eq!(
        window.tabs,
        vec![SessionId(10)],
        "the parked strip came back"
    );
    assert_eq!(window.focused, Some(SessionId(10)));

    window.set_workspace(&mut daemon, scratch, NOW).unwrap();
    assert_eq!(window.tabs, vec![SessionId(11)]);
    assert_eq!(
        window.set_workspace(&mut daemon, WorkspaceId(404), NOW),
        Err(WorkspaceError::Unknown)
    );
}

/// Unfiled sessions must NOT follow intake around. Without an explicit
/// adoption pass, switching to a new workspace would drag every session
/// that had never been filed along with it, which is the exact opposite of
/// a blank sidebar.
#[test]
fn unfiled_sessions_do_not_follow_the_intake_workspace() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let scratch = daemon.workspaces.create("Scratch").unwrap();
    daemon.workspaces.set_intake(scratch).unwrap();

    let there = window_on(scratch);
    assert!(
        there.tree(&daemon, clock()).iter().all(|g| g.is_empty()),
        "sessions adopted before the switch stay where they were"
    );

    // A session the daemon reports AFTER the switch lands in intake.
    let fresh = row(12).project(1).created_at_ms(NOW).build().info;
    daemon.apply(ServerMsg::SessionCreated(fresh));
    assert_eq!(daemon.workspace_of(SessionId(12)), Some(scratch));
    assert_eq!(there.visible_ids(&daemon, clock()), vec![SessionId(12)]);
}

/// A placement is keyed by id AND creation stamp, so a daemon that
/// restarts and hands `SessionId(10)` to a different session cannot
/// inherit the old one's workspace. Persisted filing outlives the daemon
/// that minted the ids in it, so this is not hypothetical.
#[test]
fn a_recycled_session_id_does_not_inherit_the_old_placement() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let scratch = daemon.workspaces.create("Scratch").unwrap();
    place(&mut daemon, 10, scratch);
    assert_eq!(daemon.workspace_of(SessionId(10)), Some(scratch));

    // Same id, different session: a new daemon reusing the number.
    let recycled = row(10).project(1).created_at_ms(NOW + HOUR).build().info;
    daemon.apply(ServerMsg::Sessions {
        sessions: vec![recycled],
    });
    assert_eq!(
        daemon.workspace_of(SessionId(10)),
        Some(daemon.workspaces.intake()),
        "a different session with a recycled id starts unfiled"
    );
}

/// A session the daemon stops listing must take its placement with it, or
/// the workspace keeps counting a row nobody can see and the delete guard
/// refuses forever.
#[test]
fn a_removed_session_releases_its_workspace_placement() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let scratch = daemon.workspaces.create("Scratch").unwrap();
    place(&mut daemon, 10, scratch);
    assert_eq!(daemon.workspaces.session_count(scratch), 1);

    let broadcast = daemon.apply(ServerMsg::SessionRemoved {
        session: SessionId(10),
    });
    assert_eq!(broadcast, Broadcast::SessionsChanged);
    assert_eq!(daemon.workspaces.session_count(scratch), 0);
    assert_eq!(daemon.workspaces.delete(scratch), Ok(()));
}

// ---- Grouping ---------------------------------------------------------

/// Directory grouping asks the DAEMON which root a session runs under,
/// because the daemon already answered that in `project_id`. Sessions with
/// no project get one bucket per distinct cwd, labelled with the path,
/// rather than the single anonymous lump they used to share.
#[test]
fn directory_grouping_buckets_by_project_then_by_cwd() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    daemon.projects[0].name = "vitrum".into();
    for (id, cwd) in [(20u64, "/tmp/a"), (21, "/tmp/b"), (22, "/tmp/a")] {
        daemon.sessions.push(
            row(id)
                .project(404)
                .cwd(cwd)
                .created_at_ms(NOW - HOUR + id)
                .build(),
        );
    }
    let infos: Vec<SessionInfo> = daemon.sessions.iter().map(|r| r.info.clone()).collect();
    daemon.workspaces.adopt(infos.iter());

    let window = window_on(DEFAULT_WORKSPACE);
    let tree = window.tree(&daemon, clock());
    let shape: Vec<(GroupKey, &str, Vec<u64>)> = tree
        .iter()
        .map(|g| (g.key, g.label.as_str(), ids(g.section(Section::Active))))
        .collect();
    // Recomputed from the cwds above rather than written out: see
    // `orphan_sessions_get_their_own_trailing_group` for why the key is a
    // platform fact. The cwds are what a client sends; these are what the
    // sidebar draws.
    let (a, b) = (inbox::project_key("/tmp/a"), inbox::project_key("/tmp/b"));
    assert_eq!(
        shape,
        vec![
            (pk(1), "vitrum", vec![10]),
            (GroupKey::Directory(directory_key(&a)), a.as_str(), vec![22, 20]),
            (GroupKey::Directory(directory_key(&b)), b.as_str(), vec![21]),
        ]
    );
    assert_eq!(tree[0].root.as_deref(), Some(inbox::project_key("/src/p1").as_str()));
    assert_eq!(tree[1].root.as_deref(), Some(a.as_str()));
    assert!(
        tree[1].bands.rollup.is_some(),
        "a bucket that is not a project still lights its collapsed header"
    );
    assert_eq!(tree[1].bands.rollup.as_ref().unwrap().total, 2);
}

/// Switching a workspace to named grouping must re-bucket the SAME rows
/// under the operator's folders, keep empty folders visible so there is
/// something to file into, and put the rest in one Unfiled bucket.
#[test]
fn switching_to_named_grouping_produces_the_folder_tree() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    let ws = DEFAULT_WORKSPACE;
    let build = daemon.workspaces.create_folder(ws, "Build").unwrap();
    let review = daemon.workspaces.create_folder(ws, "Review").unwrap();
    let empty = daemon.workspaces.create_folder(ws, "Later").unwrap();
    for (id, folder) in [(10u64, build), (11, review)] {
        let info = daemon.session(SessionId(id)).unwrap().clone();
        daemon
            .workspaces
            .assign_folder(&info, Some(folder))
            .unwrap();
    }

    let window = window_on(ws);
    assert_eq!(
        window.tree(&daemon, clock()).len(),
        1,
        "still one project bucket while grouping is Directory"
    );

    daemon.workspaces.get_mut(ws).unwrap().grouping = Grouping::Named;
    let tree = window.tree(&daemon, clock());
    let shape: Vec<(GroupKey, &str, Vec<u64>)> = tree
        .iter()
        .map(|g| (g.key, g.label.as_str(), ids(g.section(Section::Active))))
        .collect();
    assert_eq!(
        shape,
        vec![
            (GroupKey::Folder(build), "Build", vec![10]),
            (GroupKey::Folder(review), "Review", vec![11]),
            (GroupKey::Folder(empty), "Later", vec![]),
            (GroupKey::Unfiled, "Unfiled", vec![12]),
        ]
    );
    assert!(tree[0].collapsible());
    assert!(
        !tree[3].collapsible(),
        "Unfiled has no name to look for its rows under, so it cannot be shut"
    );

    // And back again: the same rows, the original buckets.
    daemon.workspaces.get_mut(ws).unwrap().grouping = Grouping::Directory;
    assert_eq!(window.tree(&daemon, clock()).len(), 1);
}

/// THE DEFECT, end to end. Four sessions in one directory, created under
/// four different project ids, drew four groups all called `vitrum` each
/// holding one session. A project is its directory, so this is one group
/// with four rows.
#[test]
fn four_project_ids_for_one_root_draw_one_group() {
    let mut daemon = daemon_with(&[], &[]);
    // What a daemon that keys its registry on the id it was handed reports
    // after four clients each mint their own for one repo. The trailing
    // separator is there because that is one of the ways it happens.
    daemon.projects = vec![
        ProjectInfo {
            id: ProjectId(11),
            name: "vitrum".into(),
            root: "/src/vitrum".into(),
        },
        ProjectInfo {
            id: ProjectId(22),
            name: "vitrum".into(),
            root: "/src/vitrum/".into(),
        },
        ProjectInfo {
            id: ProjectId(33),
            name: "vitrum".into(),
            root: "/src/vitrum".into(),
        },
        ProjectInfo {
            id: ProjectId(44),
            name: "vitrum".into(),
            root: "/src/vitrum".into(),
        },
    ];
    daemon.sessions = [(10u64, 11u64), (11, 22), (12, 33), (13, 44)]
        .into_iter()
        .map(|(id, pid)| {
            row(id)
                .project(pid)
                .cwd("/src/vitrum")
                .waiting(Some(false))
                .created_at_ms(NOW - HOUR + id)
                .last_activity_ms(NOW - HOUR + id)
                .build()
        })
        .collect();
    let infos: Vec<SessionInfo> = daemon.sessions.iter().map(|r| r.info.clone()).collect();
    daemon.workspaces.adopt(infos.iter());

    let tree = window_on(DEFAULT_WORKSPACE).tree(&daemon, clock());
    assert_eq!(tree.len(), 1, "one directory is one project group");
    assert_eq!(tree[0].label, "vitrum");
    let root = inbox::project_key("/src/vitrum");
    assert_eq!(tree[0].root.as_deref(), Some(root.as_str()));
    assert_eq!(ids(tree[0].section(Section::Active)), vec![13, 12, 11, 10]);
    assert_eq!(
        tree[0]
            .bands
            .rollup
            .as_ref()
            .expect("a bucket rolls up")
            .total,
        4,
        "the collapsed header must count all four, not the one its lead id owns"
    );
    assert_eq!(
        tree[0].key,
        GroupKey::Project(ProjectId(inbox::fnv1a(
            inbox::project_key("/src/vitrum").as_bytes()
        ))),
        "the bucket is keyed on the directory, so the collapse bit outlives the ids"
    );
}

/// Two sessions in one directory reached by two spellings of its path must
/// share a bucket even when the daemon knows no project for either, which
/// is the same defect one level down.
#[test]
fn two_spellings_of_one_cwd_draw_one_directory_bucket() {
    let mut daemon = daemon_with(&[], &[]);
    daemon.sessions = [(10u64, "/tmp"), (11, "/tmp/"), (12, "/tmp//")]
        .into_iter()
        .map(|(id, cwd)| {
            row(id)
                .project(404)
                .cwd(cwd)
                .waiting(Some(false))
                .created_at_ms(NOW - HOUR + id)
                .last_activity_ms(NOW - HOUR + id)
                .build()
        })
        .collect();
    let infos: Vec<SessionInfo> = daemon.sessions.iter().map(|r| r.info.clone()).collect();
    daemon.workspaces.adopt(infos.iter());

    let tree = window_on(DEFAULT_WORKSPACE).tree(&daemon, clock());
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].label, inbox::project_key("/tmp"));
    assert_eq!(ids(tree[0].section(Section::Active)), vec![12, 11, 10]);
}

/// The bucket the operator is in goes to the top and STAYS there while
/// other projects change state. A section that moves under the cursor is
/// worse than no section, so an approval landing in another project must
/// not touch the order.
#[test]
fn the_current_project_is_pinned_and_survives_activity_elsewhere() {
    let mut st = with(&[1, 2, 3], &[(10, 1, 0), (20, 2, 0), (30, 3, 0)]);
    let order =
        |st: &UiState| -> Vec<String> { st.tree(clock()).into_iter().map(|g| g.label).collect() };
    assert_eq!(order(&st), vec!["p1", "p2", "p3"], "daemon order, unpinned");

    st.open(SessionId(30), NOW);
    assert_eq!(order(&st), vec!["p3", "p1", "p2"]);
    let tree = st.tree(clock());
    assert!(
        tree[0].current,
        "the pinned bucket is the one drawn as current"
    );
    assert!(
        !tree[1].current && !tree[2].current,
        "exactly one bucket can be current"
    );

    // Another project starts shouting. This is the thing that must NOT move
    // the pinned section.
    st.daemon.sessions[0].info.attention.failed = true;
    st.daemon.sessions[0].info.status = SessionStatus::Exited { code: Some(1) };
    st.daemon.sessions[1].info.hint = Some(vitrum_proto::AgentHint {
        state: HintState::Approval,
        label: None,
        received_at_ms: NOW,
    });
    assert_eq!(
        order(&st),
        vec!["p3", "p1", "p2"],
        "a failure and an approval elsewhere must not reorder anything"
    );
    assert!(st.tree(clock())[0].current);

    // The operator moving is the ONE thing that moves the section. The
    // previous current does NOT keep a seat near the top: the base order is
    // always the daemon's, and exactly one bucket is lifted out of it, so
    // p3 returns to the slot it has always had rather than the pin
    // accumulating a history of everywhere the operator has been.
    st.open(SessionId(20), NOW);
    assert_eq!(order(&st), vec!["p2", "p1", "p3"]);
    assert!(st.tree(clock())[0].current);
}

/// With nothing focused the current bucket falls back to the last tab
/// touched, so closing the focused session does not drop the section the
/// operator was working in.
#[test]
fn the_current_project_falls_back_to_the_last_tab_touched() {
    let mut st = with(&[1, 2], &[(10, 1, 0), (20, 2, 0)]);
    st.open(SessionId(10), NOW);
    st.open(SessionId(20), NOW);
    assert_eq!(st.tree(clock())[0].label, "p2");

    st.window.focused = None;
    assert_eq!(
        st.window.tab_mru.last(),
        Some(&SessionId(20)),
        "the mru is oldest first"
    );
    assert_eq!(
        st.tree(clock())[0].label,
        "p2",
        "unfocused, the last place the operator was is still where they were"
    );

    st.window.tab_mru.clear();
    let tree = st.tree(clock());
    assert_eq!(
        tree[0].label, "p1",
        "no signal, no pin: daemon order returns"
    );
    assert!(!tree.iter().any(|g| g.current));
}

/// A focused row hidden behind its bucket's "show all" affordance still
/// makes that bucket current. Answering otherwise would unpin the project
/// the operator is in the moment its inbox grew past eight rows.
#[test]
fn a_bucket_is_current_even_when_the_preview_cut_hid_the_focused_row() {
    let sessions: Vec<(u64, u64, u64)> = (0..12)
        .map(|i| (100 + i, 2u64, 1_000 * (i + 1)))
        .chain(std::iter::once((10, 1, 0)))
        .collect();
    let mut st = with(&[1, 2], &sessions);
    // Oldest of the twelve, so it sorts last and lands behind the cut.
    st.open(SessionId(100), NOW);

    let tree = st.tree(clock());
    assert_eq!(tree[0].label, "p2");
    assert!(tree[0].current);
    assert!(
        tree[0]
            .bands
            .active
            .iter()
            .any(|r| r.id() == SessionId(100)),
        "the focused row is rescued from the cut, which is the easy half"
    );

    // Now put a row behind the cut that is NOT focused and make it current
    // through recency instead, which is the half that needs `holds`.
    st.window.focused = None;
    st.window.tab_mru = vec![SessionId(100)];
    assert!(st.tree(clock())[0].current);
}

/// Folder grouping is left alone. Folders carry an order the operator
/// arranged in Settings, and imposing a pin on a list that had none is a
/// different thing from overriding an explicit arrangement.
#[test]
fn folder_grouping_is_never_repinned() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let ws = DEFAULT_WORKSPACE;
    let first = daemon.workspaces.create_folder(ws, "First").unwrap();
    daemon.workspaces.create_folder(ws, "Second").unwrap();
    let info = daemon.session(SessionId(10)).unwrap().clone();
    daemon.workspaces.assign_folder(&info, Some(first)).unwrap();
    daemon.workspaces.get_mut(ws).unwrap().grouping = Grouping::Named;

    let mut window = window_on(ws);
    window.focused = Some(SessionId(11));
    let tree = window.tree(&daemon, clock());
    assert_eq!(
        tree.iter().map(|g| g.label.as_str()).collect::<Vec<_>>(),
        vec!["First", "Second", "Unfiled"],
        "the operator's folder order is untouched"
    );
    assert!(!tree.iter().any(|g| g.current));
}

/// Deleting a folder unfiles its sessions rather than losing them, so it
/// needs no guard. Folder ids are unique across every workspace, so two
/// workspaces cannot share a collapse bit.
#[test]
fn deleting_a_folder_unfiles_its_sessions_and_ids_are_globally_unique() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let other = daemon.workspaces.create("Other").unwrap();
    let a = daemon
        .workspaces
        .create_folder(DEFAULT_WORKSPACE, "A")
        .unwrap();
    let b = daemon.workspaces.create_folder(other, "B").unwrap();
    assert_ne!(
        a, b,
        "folder ids are minted from the set, not per workspace"
    );

    let info = daemon.session(SessionId(10)).unwrap().clone();
    daemon.workspaces.assign_folder(&info, Some(a)).unwrap();
    assert_eq!(
        daemon
            .workspaces
            .get(DEFAULT_WORKSPACE)
            .unwrap()
            .folder_of(&info),
        Some(a)
    );
    assert_eq!(
        daemon.workspaces.assign_folder(&info, Some(b)),
        Err(WorkspaceError::UnknownFolder),
        "a folder of a workspace this session does not live in is not a target"
    );

    daemon
        .workspaces
        .delete_folder(DEFAULT_WORKSPACE, a)
        .unwrap();
    assert_eq!(
        daemon
            .workspaces
            .get(DEFAULT_WORKSPACE)
            .unwrap()
            .folder_of(&info),
        None
    );
    assert_eq!(daemon.workspaces.session_count(DEFAULT_WORKSPACE), 1);
}

/// Moving a session to another workspace must drop its folder, because
/// folder ids are global and a stale entry would keep the row filed under
/// a folder its new workspace does not own.
#[test]
fn moving_a_session_between_workspaces_clears_its_folder() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let other = daemon.workspaces.create("Other").unwrap();
    let a = daemon
        .workspaces
        .create_folder(DEFAULT_WORKSPACE, "A")
        .unwrap();
    let info = daemon.session(SessionId(10)).unwrap().clone();
    daemon.workspaces.assign_folder(&info, Some(a)).unwrap();

    daemon.move_to_workspace(&[SessionId(10)], other).unwrap();
    assert_eq!(
        daemon
            .workspaces
            .get(DEFAULT_WORKSPACE)
            .unwrap()
            .folder_of(&info),
        None
    );
    assert_eq!(daemon.workspaces.get(other).unwrap().folder_of(&info), None);
}

/// Band visibility is per workspace and keyed on disposition, so hiding
/// Woke removes woken rows from the inbox band without hiding the inbox.
#[test]
fn section_visibility_hides_one_disposition_at_a_time() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    // Only a finished row can be drained; `can_settle` refuses a Working
    // one, and a test that quietly settled nothing would assert nothing.
    daemon.sessions.push(
        row(11)
            .project(1)
            .exited(Some(0))
            .waiting(Some(false))
            .visited(NOW)
            .created_at_ms(NOW - HOUR + 1)
            .last_activity_ms(NOW - HOUR + 1)
            .build(),
    );
    let infos: Vec<SessionInfo> = daemon.sessions.iter().map(|r| r.info.clone()).collect();
    daemon.workspaces.adopt(infos.iter());
    assert_eq!(daemon.settle(&[SessionId(11)], NOW), 1);
    let window = window_on(DEFAULT_WORKSPACE);
    assert_eq!(
        ids(window.tree(&daemon, clock())[0].section(Section::Settled)),
        vec![11]
    );

    daemon
        .workspaces
        .get_mut(DEFAULT_WORKSPACE)
        .unwrap()
        .sections
        .set(Disposition::Settled, false);
    let tree = window.tree(&daemon, clock());
    assert!(
        tree[0].section(Section::Settled).is_empty(),
        "a hidden band arrives with zero rows, so its head goes too"
    );
    assert_eq!(ids(tree[0].section(Section::Active)), vec![10]);

    let vis = daemon.workspaces.get(DEFAULT_WORKSPACE).unwrap().sections;
    assert_eq!(vis.hidden_count(), 1);
    assert!(!vis.shows(Disposition::Settled));
    assert!(vis.shows(Disposition::Woke));
}

// ---- Menu -------------------------------------------------------------

/// Filing has to be reachable from the row, or a new workspace is a blank
/// sidebar with no way to fill it. The workspace you are already in is not
/// offered, because moving a row where it already is does nothing.
#[test]
fn the_row_menu_offers_every_other_workspace_and_never_the_current_one() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let review = daemon.workspaces.create("Review").unwrap();
    let window = window_on(DEFAULT_WORKSPACE);

    let items = window.menu_items(&daemon, SessionId(10), clock());
    let filing: Vec<(&MenuAction, &str)> = items
        .iter()
        .filter(|i| {
            matches!(
                i.action,
                MenuAction::MoveToWorkspaceHeader | MenuAction::MoveToWorkspace(_)
            )
        })
        .map(|i| (&i.action, i.label.as_str()))
        .collect();
    assert_eq!(
        filing,
        vec![
            (&MenuAction::MoveToWorkspaceHeader, "Move to workspace"),
            (&MenuAction::MoveToWorkspace(review), "  Review"),
        ]
    );
    assert!(MenuAction::MoveToWorkspaceHeader.is_caption());
    assert!(
        !items
            .iter()
            .any(|i| matches!(i.action, MenuAction::MoveToFolder(_))),
        "folders are only a target while the workspace groups by them"
    );
}

/// With one workspace there is nowhere to move to, so the caption must not
/// appear at all rather than head an empty list.
#[test]
fn the_row_menu_omits_filing_when_there_is_only_one_workspace() {
    let daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let window = window_on(DEFAULT_WORKSPACE);
    assert!(
        !window
            .menu_items(&daemon, SessionId(10), clock())
            .iter()
            .any(|i| matches!(i.action, MenuAction::MoveToWorkspaceHeader))
    );
}

/// In named grouping the menu also offers the folders, Unfiled first so
/// there is always a way back out of one.
#[test]
fn named_grouping_adds_the_folder_targets_with_unfiled_first() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0)]);
    let ws = daemon.workspaces.get_mut(DEFAULT_WORKSPACE).unwrap();
    ws.grouping = Grouping::Named;
    let build = daemon
        .workspaces
        .create_folder(DEFAULT_WORKSPACE, "Build")
        .unwrap();
    let window = window_on(DEFAULT_WORKSPACE);

    let folders: Vec<MenuAction> = window
        .menu_items(&daemon, SessionId(10), clock())
        .iter()
        .filter(|i| {
            matches!(
                i.action,
                MenuAction::MoveToFolderHeader | MenuAction::MoveToFolder(_)
            )
        })
        .map(|i| i.action)
        .collect();
    assert_eq!(
        folders,
        vec![
            MenuAction::MoveToFolderHeader,
            MenuAction::MoveToFolder(None),
            MenuAction::MoveToFolder(Some(build)),
        ]
    );
}

// ---- Settings and the sidebar width ------------------------------------

/// The sidebar cap has to be relative to the window as well as absolute.
/// 448px is 12% of a 3840px window and 56% of an 800px one, and the second
/// leaves a terminal narrower than the sidebar next to it.
#[test]
fn the_sidebar_is_capped_by_a_fraction_of_the_window_as_well_as_absolutely() {
    let mut w = WindowState::default();
    w.set_sidebar_width_in(10_000.0, 3840.0);
    assert_eq!(
        w.sidebar_width, SIDEBAR_MAX_PX,
        "the absolute cap still wins"
    );

    w.set_sidebar_width_in(10_000.0, 800.0);
    assert_eq!(w.sidebar_width, 800.0 * SIDEBAR_MAX_FRACTION);

    w.set_sidebar_width_in(10.0, 600.0);
    assert_eq!(
        w.sidebar_width, SIDEBAR_MIN_PX,
        "the legibility floor outranks the fraction, so a narrow window \
         gets a cramped terminal rather than an unreadable sidebar"
    );
}

/// A text scale read back from a hand-edited file is the case that
/// actually produces an unusable window, and it does not go through a
/// slider, so the clamp lives on the setter.
#[test]
fn the_text_scale_clamps_at_both_ends() {
    let mut s = Settings::default();
    assert_eq!(s.text_scale_pct, 100);
    s.set_text_scale(5);
    assert_eq!(s.text_scale_pct, TEXT_SCALE_MIN_PCT);
    s.set_text_scale(5_000);
    assert_eq!(s.text_scale_pct, TEXT_SCALE_MAX_PCT);
}

/// `--server` must stay authoritative when the settings field is empty,
/// which is the case it exists for.
#[test]
fn an_empty_daemon_url_setting_defers_to_the_command_line() {
    let mut s = Settings::default();
    assert_eq!(s.resolved_daemon_url("ws://cli"), "ws://cli");
    s.daemon_url = "  ws://other  ".into();
    assert_eq!(s.resolved_daemon_url("ws://cli"), "ws://other");
}

/// The terminal defaults must be the ones actually measured and the ones
/// `bootstrap.js` mounts with. A default that disagrees with the mount
/// silently changes the terminal the first time the modal is saved.
#[test]
fn the_terminal_defaults_are_the_measured_ones() {
    let t = TerminalPrefs::default();
    assert_eq!(
        t.renderer,
        TermRenderer::Dom,
        "WebGL costs 0.244% idle CPU here"
    );
    assert_eq!(
        t.scrollback_lines, 1_000,
        "must match bootstrap.js at mount"
    );
    assert_eq!(t.font_family, "", "empty means --rg-font-mono, one source");
}

/// The settings layer is one of the exclusive layers, and switching tab
/// must never be able to open it sideways.
#[test]
fn the_settings_layer_opens_on_a_tab_and_only_switches_while_open() {
    let mut st = UiState::default();
    st.set_settings_tab(SettingsTab::Keyboard);
    assert_eq!(st.window.layer, Layer::None, "switching cannot summon it");

    st.window.layer = Layer::Settings(SettingsTab::Workspaces);
    assert_eq!(st.window.layer, Layer::Settings(SettingsTab::Workspaces));
    st.set_settings_tab(SettingsTab::Advanced);
    assert_eq!(st.window.layer, Layer::Settings(SettingsTab::Advanced));
    assert!(st.window.layer.is_open());
    assert_eq!(SettingsTab::default(), SettingsTab::Appearance);
}

/// A profile written by a build that predates the first-run sheet must load,
/// and must count as never onboarded. Defaulting the other way would mean the
/// sheet never appears for anyone upgrading, which is every existing user.
#[test]
fn an_older_profile_has_not_been_onboarded() {
    let older = r#"{"showBranch":false,"textScalePct":120}"#;
    let s: Settings = serde_json::from_str(older).expect("an older profile must still load");
    assert!(!s.show_branch);
    assert_eq!(s.text_scale_pct, 120);
    assert!(!s.onboarded);
    assert_eq!(s.last_seen_version(), None);
}

/// Finishing onboarding must also bank the running version. Without that, the
/// operator reads the walkthrough and is immediately handed the release notes
/// for the version they installed a minute ago.
#[test]
fn finishing_onboarding_also_counts_as_reading_the_notes() {
    let v = semver::Version::parse("0.1.0").unwrap();
    let mut s = Settings::default();
    s.finish_onboarding(&v);
    assert!(s.onboarded);
    assert_eq!(s.last_seen_version(), Some(v));
}

/// Reading the notes must not mark the profile onboarded. They are separate
/// facts and collapsing them would suppress the first-run sheet for anyone
/// who happened to see release notes first.
#[test]
fn reading_the_notes_is_not_onboarding() {
    let mut s = Settings::default();
    s.mark_seen(&semver::Version::parse("0.2.0").unwrap());
    assert!(!s.onboarded);
    assert_eq!(s.seen_version, "0.2.0");
}

/// A seen-version string that is not a version reads as never seen, so the
/// notes show once more. A hand-edited or truncated profile must not be able
/// to swallow a release note permanently.
#[test]
fn an_unreadable_seen_version_shows_the_notes_again() {
    let mut s = Settings::default();
    s.seen_version = "not-a-version".to_string();
    assert_eq!(s.last_seen_version(), None);
}

/// Fixture mode is not a live connection. The first-run sheet reads this to
/// decide whether to tell you to start the daemon, and `--fixture` opened no
/// socket at all.
#[test]
fn fixture_data_does_not_count_as_connected() {
    assert!(
        ConnState::Live {
            server_version: "1".into()
        }
        .is_live()
    );
    assert!(!ConnState::Fixture.is_live());
    assert!(!ConnState::Connecting.is_live());
    assert!(
        !ConnState::Failed {
            detail: "refused".into()
        }
        .is_live()
    );
}

// ---- Persistence -------------------------------------------------------

/// Everything the operator authored has to survive a restart: the
/// workspaces, their folders, which session is in which, the settings, and
/// each window's workspace and layout.
#[test]
fn the_whole_document_round_trips_through_json() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let review = daemon.workspaces.create("Review").unwrap();
    daemon.workspaces.move_to(review, 0).unwrap();
    let folder = daemon.workspaces.create_folder(review, "Waiting").unwrap();
    place(&mut daemon, 11, review);
    let info = daemon.session(SessionId(11)).unwrap().clone();
    daemon
        .workspaces
        .assign_folder(&info, Some(folder))
        .unwrap();
    daemon.workspaces.get_mut(review).unwrap().grouping = Grouping::Named;
    daemon
        .workspaces
        .get_mut(review)
        .unwrap()
        .sections
        .set(Disposition::Snoozed, false);
    daemon.settings.theme = ThemePref::Dark;
    daemon.settings.show_branch = false;
    daemon.settings.set_text_scale(140);
    daemon
        .settings
        .keyboard
        .overrides
        .insert("focusNext".into(), "ctrl+j".into());

    let mut left = window_on(DEFAULT_WORKSPACE);
    left.open(&mut daemon, SessionId(10), NOW);
    left.sidebar_width = 400.0;
    left.sidebar_collapsed = true;
    left.workspace_bar_open = true;
    let mut right = window_at(1, DEFAULT_WORKSPACE);
    right.open(&mut daemon, SessionId(10), NOW);
    right.set_workspace(&mut daemon, review, NOW).unwrap();
    right.open(&mut daemon, SessionId(11), NOW);

    let doc = Persisted::capture(&daemon, [&left, &right]);
    let text = encode_ui_state(&doc);
    let back = match parse_ui_state(&text) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("expected a loaded document, got {other}"),
    };
    assert_eq!(back, doc, "the document is byte-for-byte the same shape");

    let mut fresh = DaemonState::default();
    back.restore_daemon(&mut fresh);
    assert_eq!(ws_ids(&fresh.workspaces), vec![review, DEFAULT_WORKSPACE]);
    assert_eq!(
        fresh.workspaces.get(review).unwrap().grouping,
        Grouping::Named
    );
    assert!(!fresh.workspaces.get(review).unwrap().sections.snoozed);
    assert_eq!(
        fresh.workspaces.get(review).unwrap().folders()[0].name,
        "Waiting"
    );
    assert_eq!(fresh.workspaces.workspace_of(&info), review);
    assert_eq!(
        fresh.workspaces.get(review).unwrap().folder_of(&info),
        Some(folder),
        "which folder a session sits in has to survive too"
    );
    assert_eq!(fresh.settings.theme, ThemePref::Dark);
    assert!(!fresh.settings.show_branch);
    assert_eq!(fresh.settings.text_scale_pct, 140);
    assert_eq!(
        fresh
            .settings
            .keyboard
            .overrides
            .get("focusNext")
            .map(String::as_str),
        Some("ctrl+j")
    );

    let mut w0 = WindowState::default();
    assert!(back.restore_window(&mut w0));
    assert_eq!(w0.workspace, DEFAULT_WORKSPACE);
    assert_eq!(w0.sidebar_width, 400.0);
    assert!(w0.sidebar_collapsed);
    assert!(w0.workspace_bar_open);
    assert_eq!(w0.tabs, vec![SessionId(10)]);

    let mut w1 = window_at(1, DEFAULT_WORKSPACE);
    assert!(back.restore_window(&mut w1));
    assert_eq!(
        w1.workspace, review,
        "each window remembers its own workspace"
    );
    assert_eq!(w1.tabs, vec![SessionId(11)]);
    assert!(
        !back.restore_window(&mut window_at(2, DEFAULT_WORKSPACE)),
        "a window opened since the last save has nothing to restore"
    );
}

/// One window saving must not delete another window's layout.
///
/// Each desktop window has its own VirtualDom and therefore its own
/// `UiState`, so a window that captured the whole document would write
/// itself into slot 0 and drop every other entry. This is the merge that
/// stops it, and the padding rule that stops an unsaved slot inheriting a
/// neighbour's tabs.
#[test]
fn each_window_writes_only_its_own_slot() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1)]);
    let review = daemon.workspaces.create("Review").unwrap();
    let mut left = window_on(DEFAULT_WORKSPACE);
    left.open(&mut daemon, SessionId(10), NOW);
    left.sidebar_width = 300.0;
    let mut right = window_at(2, DEFAULT_WORKSPACE);
    right.set_workspace(&mut daemon, review, NOW).unwrap();
    right.sidebar_width = 420.0;

    let mut doc = Persisted {
        workspaces: daemon.workspaces.clone(),
        ..Persisted::default()
    };
    // Window 2 saves first, before window 0 or 1 ever has.
    doc.put_window(&right);
    assert_eq!(doc.windows.len(), 3);
    assert_eq!(doc.windows[2].sidebar_width, 420.0);
    assert_eq!(doc.windows[2].workspace, review);
    assert_eq!(
        doc.windows[0].strip,
        Strip::default(),
        "a padded slot must not inherit another window's tabs"
    );

    // Window 0 then saves and must leave window 2 alone.
    doc.put_window(&left);
    assert_eq!(doc.windows.len(), 3);
    assert_eq!(doc.windows[0].sidebar_width, 300.0);
    assert_eq!(doc.windows[0].strip.tabs, vec![SessionId(10)]);
    assert_eq!(doc.windows[2].sidebar_width, 420.0);
    assert_eq!(doc.windows[2].workspace, review);
}

/// The arrangement an operator builds by hand has to come back whole.
///
/// Locks out the defect a settings audit found: `sidebar_collapsed` and
/// the tab strip were written to disk only when some UNRELATED control
/// happened to commit afterwards, so a restart silently discarded a
/// deliberate arrangement, and the round-trip suite never noticed because
/// it asserted the workspace and `tabs` and stopped there. `tab_mru`
/// decides which session the strip evicts and which bucket the sidebar
/// pins, `focused` decides what is on screen, `parked` is every workspace
/// the window is not currently showing, and `home` and `folder_of` are the
/// filing itself. Each is asserted by value, because a strip that comes
/// back the right LENGTH with the wrong contents is the bug.
#[test]
fn the_strip_the_collapse_and_the_filing_all_survive_a_restart() {
    let mut daemon = daemon_with(&[1], &[(10, 1, 0), (11, 1, 1), (12, 1, 2)]);
    let review = daemon.workspaces.create("Review").unwrap();
    let waiting = daemon.workspaces.create_folder(review, "Waiting").unwrap();
    place(&mut daemon, 12, review);
    let filed = daemon.session(SessionId(12)).unwrap().clone();
    daemon
        .workspaces
        .assign_folder(&filed, Some(waiting))
        .unwrap();
    // Two switches the audit found had no coverage at all.
    daemon.settings.confirm_terminate = false;
    daemon.settings.notifications.skip_focused_session = false;

    let mut w = window_on(DEFAULT_WORKSPACE);
    w.open(&mut daemon, SessionId(10), NOW);
    w.open(&mut daemon, SessionId(11), NOW);
    // Focus 10 again, so MRU and strip order genuinely differ: a snapshot
    // that dropped `tab_mru` and rebuilt it from `tabs` would still pass a
    // test where the two happen to agree.
    w.open(&mut daemon, SessionId(10), NOW);
    w.sidebar_collapsed = true;
    // A second workspace with its own strip, parked by switching away.
    w.set_workspace(&mut daemon, review, NOW).unwrap();
    w.open(&mut daemon, SessionId(12), NOW);
    w.set_workspace(&mut daemon, DEFAULT_WORKSPACE, NOW)
        .unwrap();

    assert_eq!(w.tabs, vec![SessionId(10), SessionId(11)]);
    assert_eq!(w.tab_mru, vec![SessionId(11), SessionId(10)]);
    assert_eq!(w.focused, Some(SessionId(10)));

    let doc = Persisted::capture(&daemon, [&w]);
    let back = match parse_ui_state(&encode_ui_state(&doc)) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("expected a loaded document, got {other}"),
    };

    let mut fresh = WindowState::default();
    assert!(back.restore_window(&mut fresh));
    assert_eq!(fresh.workspace, DEFAULT_WORKSPACE);
    assert!(
        fresh.sidebar_collapsed,
        "a collapsed sidebar came back expanded"
    );
    assert_eq!(fresh.tabs, vec![SessionId(10), SessionId(11)]);
    assert_eq!(
        fresh.tab_mru,
        vec![SessionId(11), SessionId(10)],
        "eviction order is part of the strip, not something to rebuild"
    );
    assert_eq!(fresh.focused, Some(SessionId(10)));
    assert_eq!(
        fresh.parked.get(&review).map(|s| s.tabs.clone()),
        Some(vec![SessionId(12)]),
        "the workspace the window was not showing keeps its own strip"
    );
    assert_eq!(
        fresh.parked.get(&review).and_then(|s| s.focused),
        Some(SessionId(12))
    );

    let mut daemon_back = DaemonState::default();
    back.restore_daemon(&mut daemon_back);
    assert_eq!(
        daemon_back.workspaces.workspace_of(&filed),
        review,
        "which workspace a session is filed in is the filing"
    );
    assert_eq!(
        daemon_back
            .workspaces
            .get(review)
            .unwrap()
            .folder_of(&filed),
        Some(waiting)
    );
    assert_eq!(
        daemon_back.workspaces.get(review).unwrap().folders()[0].name,
        "Waiting"
    );
    assert!(!daemon_back.settings.confirm_terminate);
    assert!(!daemon_back.settings.notifications.skip_focused_session);

    // A rename and a delete are arrangement too, and neither was covered.
    let scratch = daemon.workspaces.create("Scratch").unwrap();
    daemon.workspaces.rename(review, "Reviewing").unwrap();
    daemon.workspaces.delete(scratch).unwrap();
    let after = match parse_ui_state(&encode_ui_state(&Persisted::capture(&daemon, [&w]))) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("expected a loaded document, got {other}"),
    };
    let mut renamed = DaemonState::default();
    after.restore_daemon(&mut renamed);
    assert_eq!(renamed.workspaces.get(review).unwrap().name, "Reviewing");
    assert_eq!(
        ws_ids(&renamed.workspaces),
        vec![DEFAULT_WORKSPACE, review],
        "a deleted workspace must not come back"
    );
}

/// A document written before a field existed must load with everything the
/// operator arranged still in it.
///
/// Locks out the way this file loses a profile: adding a REQUIRED field to
/// [`WindowSnapshot`] or [`Persisted`]. Serde then refuses every older
/// `ui.json` with "missing field", [`parse_ui_state`] calls that
/// [`UiStateLoad::Corrupt`], [`load_prefs`] hands back defaults, and
/// [`Persisted::restore_daemon`] writes those defaults over the live
/// state — every workspace, folder, session placement and window layout
/// gone on the first launch after the upgrade, reported only as a flash.
/// The document below is byte-for-byte a pre-`searchOptions` file and is
/// deliberately hand-written rather than produced by `encode_ui_state`,
/// because a generated one would grow the new field and prove nothing.
#[test]
fn a_document_written_before_a_field_existed_still_loads_whole() {
    let old = r#"{
      "version": 1,
      "settings": { "theme": "dark", "textScalePct": 140 },
      "workspaces": {
        "list": [
          { "id": 2, "name": "Review", "grouping": "named",
            "sections": { "active": true, "woke": true, "snoozed": false, "settled": true },
            "folders": [ { "id": 1, "name": "Waiting" } ],
            "folderOf": [ [ { "id": 11, "createdAtMs": 1772577000000 }, 1 ] ] },
          { "id": 1, "name": "Default", "grouping": "directory",
            "sections": { "active": true, "woke": true, "snoozed": true, "settled": true },
            "folders": [], "folderOf": [] }
        ],
        "home": [ [ { "id": 11, "createdAtMs": 1772577000000 }, 2 ] ],
        "intake": 2,
        "nextWorkspace": 3,
        "nextFolder": 2
      },
      "windows": [
        { "workspace": 1, "sidebarWidth": 400.0, "sidebarCollapsed": true,
          "workspaceBarOpen": true,
          "strip": { "tabs": [10, 11], "tabMru": [11, 10], "focused": 10 },
          "parked": [] }
      ]
    }"#;

    let doc = match parse_ui_state(old) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("a pre-upgrade document must load, got {other}"),
    };

    let mut daemon = DaemonState::default();
    doc.restore_daemon(&mut daemon);
    assert_eq!(
        ws_ids(&daemon.workspaces),
        vec![WorkspaceId(2), DEFAULT_WORKSPACE],
        "the operator's workspaces, in their order"
    );
    assert_eq!(
        daemon.workspaces.get(WorkspaceId(2)).unwrap().name,
        "Review"
    );
    assert_eq!(
        daemon.workspaces.get(WorkspaceId(2)).unwrap().folders()[0].name,
        "Waiting"
    );
    assert!(
        !daemon
            .workspaces
            .get(WorkspaceId(2))
            .unwrap()
            .sections
            .snoozed
    );

    // The placement, which is the field a corrupt-and-default would lose
    // most quietly: the session simply reappears in the wrong list.
    let mut placed = info(11);
    placed.created_at_ms = 1_772_577_000_000;
    assert_eq!(daemon.workspaces.workspace_of(&placed), WorkspaceId(2));
    assert_eq!(
        daemon
            .workspaces
            .get(WorkspaceId(2))
            .unwrap()
            .folder_of(&placed),
        Some(FolderId(1))
    );
    assert_eq!(daemon.settings.theme, ThemePref::Dark);
    assert_eq!(daemon.settings.text_scale_pct, 140);

    let mut w = WindowState::default();
    assert!(doc.restore_window(&mut w));
    assert_eq!(w.sidebar_width, 400.0);
    assert!(w.sidebar_collapsed);
    assert_eq!(w.tabs, vec![SessionId(10), SessionId(11)]);
    assert_eq!(w.tab_mru, vec![SessionId(11), SessionId(10)]);
    assert_eq!(w.focused, Some(SessionId(10)));
    assert_eq!(
        w.search.options,
        crate::ui::search::Options::default(),
        "a field the file predates takes its default, it does not fail the file"
    );

    // And the new field really does round-trip once it is written.
    let mut set = WindowState::default();
    set.search.options = crate::ui::search::Options {
        regex: true,
        case_insensitive: false,
        whole_word: true,
    };
    let text = encode_ui_state(&Persisted::capture(&DaemonState::default(), [&set]));
    let on_disk: serde_json::Value =
        serde_json::from_str(&text).expect("what we write is what we read");
    assert_eq!(
        on_disk["windows"][0]["searchOptions"],
        serde_json::json!({ "regex": true, "caseInsensitive": false, "wholeWord": true }),
        "the switches must reach the file in the wire shape: {text}"
    );
    let back = match parse_ui_state(&text) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("expected a loaded document, got {other}"),
    };
    let mut restored = WindowState::default();
    assert!(back.restore_window(&mut restored));
    assert_eq!(restored.search.options, set.search.options);
    assert_eq!(
        restored.search.query, "",
        "the query is the question being asked now and must never reach the profile"
    );
    assert_eq!(
        restored.search.answer, None,
        "five hundred hits and their context lines must never reach the profile"
    );
    assert!(
        !text.contains("\"hits\"") && !text.contains("\"query\""),
        "the profile carries the switches and nothing else from the search"
    );
}

/// The file has to land beside the platform's other config, not next to
/// the binary and not in a cache. Workspaces and settings are things the
/// operator authored; clearing a cache must not delete them.
#[test]
fn the_state_file_lives_in_the_platform_config_directory() {
    let paths = AppPaths::for_current_platform().expect("this platform resolves its dirs");
    assert_eq!(
        ui_state_path().expect("so does the state path"),
        paths.config_dir.join("ui.json")
    );
    assert_eq!(UI_STATE_FILE, "ui.json");
    assert_ne!(
        paths.config_dir, paths.cache_dir,
        "if these ever collapse, this test is the one that says so"
    );
}

/// A missing file is a first launch and must be silent. A file that is
/// there and unusable must say so, because losing an operator's workspaces
/// every launch without explanation is the failure this enum exists to
/// prevent.
#[test]
fn a_missing_file_is_silent_and_a_broken_one_is_not() {
    let dir = std::env::temp_dir().join(format!("vitrum-state-{}", std::process::id()));
    let path = dir.join("ui.json");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(load_ui_state(&path), UiStateLoad::Missing);
    assert_eq!(load_ui_state(&path).or_default().1, None);

    let doc = Persisted::default();
    save_ui_state(&path, &doc).expect("save creates its directory");
    assert_eq!(load_ui_state(&path), UiStateLoad::Loaded(Box::new(doc)));
    assert!(
        !path.with_extension("json.tmp").exists(),
        "the atomic write must not leave its temporary behind"
    );

    std::fs::write(&path, "{ not json").unwrap();
    assert!(matches!(load_ui_state(&path), UiStateLoad::Corrupt { .. }));
    assert!(load_ui_state(&path).or_default().1.is_some());

    std::fs::write(&path, r#"{"version":99}"#).unwrap();
    assert_eq!(
        load_ui_state(&path),
        UiStateLoad::Unsupported { version: 99 },
        "a future file reports its version rather than a pile of field errors"
    );
    assert_eq!(
        UiStateLoad::Unsupported { version: 99 }.to_string(),
        "workspace file is version 99, this build understands 1"
    );

    std::fs::write(&path, r#"{"settings":{}}"#).unwrap();
    assert!(matches!(load_ui_state(&path), UiStateLoad::Corrupt { .. }));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- The `SessionUpdated` budget ------------------------------------
//
// The objective states one number for this path: a single
// `ServerMsg::SessionUpdated` must be absorbed in under 16ms at twenty
// sessions. The daemon pushes one per active session per second, so at
// that load the client has to absorb twenty of them a second while a
// 60Hz compositor is asking for a frame every 16.6ms. These build the
// state the number is quoted at and drive it without a window.

/// One frame at 60Hz, which is the whole budget for absorbing one update.
const FRAME_BUDGET: core::time::Duration = core::time::Duration::from_millis(16);

/// Sessions, and windows, that the objective is stated at.
const LOAD: usize = 20;

/// Real directories on disk, because [`inbox::project_key`] calls
/// `std::fs::canonicalize` and the two branches cost very different
/// amounts: an existing path walks and resolves every component, a
/// missing one fails on the first syscall. An operator's sessions run in
/// directories that exist, so a fixture rooted at `/src/p1` benchmarks the
/// cheap branch and reports a number the product never sees.
/// Eleven of them: eight repository roots the daemon has project records
/// for, and three detached directories it has none for.
fn bench_roots() -> Vec<String> {
    let base = std::env::temp_dir().join(format!("vitrum-bench-{}", std::process::id()));
    (0..11)
        .map(|i| {
            let dir = if i < 8 {
                base.join(format!("repo-{i}"))
                    .join("worktrees")
                    .join("main")
            } else {
                base.join(format!("detached-{i}"))
            };
            std::fs::create_dir_all(&dir).expect("temp dir");
            // Canonical, so a root and its own `project_key` are the same
            // string and a test can name a bucket key without asking the
            // filesystem a second time. `/tmp` is a symlink on some
            // platforms, which would otherwise make the two differ.
            inbox::project_key(&dir.to_string_lossy())
        })
        .collect()
}

/// The eight roots the daemon registers projects for.
const REPOS: usize = 8;

/// Twenty sessions over eight repositories, shaped like a real load.
///
/// Twenty daemon project records over eight roots, because the protocol
/// has no "create project" message and every client that ever started a
/// session in a repo minted its own id for it — that fan-in is exactly
/// what [`inbox::coalesce_projects`] exists to fold. Three sessions run in
/// directories no project record covers, which is the bucket-per-cwd path.
/// Statuses span all five, two rows are parked and two are drained, titles
/// are the 60-character case the objective measures at, and branches are
/// the long `wip/` case.
fn bench_daemon(roots: &[String]) -> DaemonState {
    let mut st = DaemonState::default();
    st.projects = (0..LOAD as u64)
        .map(|i| ProjectInfo {
            id: ProjectId(1000 + i),
            name: format!("repo-{}", i as usize % REPOS),
            root: roots[i as usize % REPOS].clone(),
        })
        .collect();

    st.sessions = (0..LOAD as u64)
        .map(|i| {
            let orphan = i >= LOAD as u64 - 3;
            let mut info = info(i + 1);
            // An orphan's project id matches no record the daemon sent,
            // which is the only way to reach the bucket-per-cwd path.
            info.project_id = if orphan {
                ProjectId(9000 + i)
            } else {
                ProjectId(1000 + i)
            };
            info.title = format!("refactor the daemon session watcher, pass {i:02} of twenty");
            info.cwd = if orphan {
                // A directory the daemon has no project record for, which
                // is the only way to reach the bucket-per-cwd path.
                roots[REPOS + (i as usize % 3)].clone()
            } else {
                roots[i as usize % REPOS].clone()
            };
            info.command = "/usr/bin/claude".to_string();
            info.git_branch = Some(format!("wip/very-long-feature-branch-{i}"));
            info.created_at_ms = NOW - HOUR - i * 1_000;
            info.last_activity_ms = NOW - i * 30_000;
            info.unread = i % 3 == 0;
            info.attention = Attention {
                bell: i % 6 == 0,
                failed: matches!(i % 5, 4),
                waiting: Some(i % 4 == 0),
                idle_ms: i * 1_000,
            };
            info.status = match i % 5 {
                3 => SessionStatus::Exited { code: Some(0) },
                4 => SessionStatus::Exited { code: Some(1) },
                _ => SessionStatus::Running,
            };
            info.hint = (i % 5 == 1).then(|| vitrum_proto::AgentHint {
                state: HintState::Working,
                label: Some("running the test suite".to_string()),
                received_at_ms: NOW - 120_000,
            });
            let mut view = SessionView::new(info);
            if i % 9 == 2 {
                view.snooze = Some(Snooze {
                    snoozed_at_ms: NOW - HOUR,
                    wake_at_ms: NOW + HOUR,
                });
            }
            if i % 9 == 5 {
                view.settle_override = Some(SettleOverride::Settled);
            }
            view.last_visited_ms = (i % 2 == 0).then_some(NOW - 600_000);
            view
        })
        .collect();

    let infos: Vec<SessionInfo> = st.sessions.iter().map(|row| row.info.clone()).collect();
    st.workspaces.adopt(infos.iter());
    st
}

/// Twenty windows onto that daemon, each with its own strip and focus.
fn bench_windows(daemon: &DaemonState) -> Vec<WindowState> {
    (0..LOAD)
        .map(|i| {
            let focused = daemon.sessions[i % daemon.sessions.len()].id();
            let mut w = window_at(i, DEFAULT_WORKSPACE);
            w.tabs = daemon
                .sessions
                .iter()
                .skip(i)
                .take(MAX_TABS)
                .map(|row| row.id())
                .collect();
            w.tab_mru = w.tabs.clone();
            w.focused = Some(focused);
            w
        })
        .collect()
}

/// Which derivation the benchmark drives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fold {
    /// Everything re-derived from the filesystem on every paint: one
    /// `realpath` per project root and one per session cwd, per paint, per
    /// window. This is what the sidebar did before [`FoldedProjects`] and
    /// [`DirKeys`] landed.
    Cold,
    /// The memos, re-derived only when their inputs change.
    Memo,
}

/// One `SessionUpdated` exactly as the client absorbs it: the daemon fold,
/// this window's reaction, and the derivation the sidebar runs on the
/// paint that follows — `tree` once, then the attention count taken from
/// that same tree, which is what `ui::sidebar::Sidebar` does.
fn absorb(
    daemon: &mut DaemonState,
    windows: &mut [WindowState],
    info: SessionInfo,
    clock: Clock,
    fold: Fold,
) -> usize {
    let broadcast = daemon.apply(ServerMsg::SessionUpdated(info));
    let mut sink = 0;
    for window in windows.iter_mut() {
        window.receive(daemon, &broadcast, clock.now_ms);
        if fold == Fold::Cold {
            daemon.forget_folded();
            daemon.forget_dir_keys();
        }
        let tree = window.tree(daemon, clock);
        sink += window.attention_count_of(daemon, &tree, clock);
        sink += tree.iter().map(SidebarGroup::len).sum::<usize>();
    }
    sink
}

/// A plausible next update for session `i`: new activity, new idle, and
/// the status flip that makes a row change band.
fn bench_update(daemon: &DaemonState, i: usize, now_ms: u64) -> SessionInfo {
    let mut info = daemon.sessions[i % daemon.sessions.len()].info.clone();
    info.last_activity_ms = now_ms;
    info.attention = Attention {
        bell: i % 5 == 0,
        failed: info.attention.failed,
        waiting: Some(i % 3 == 0),
        idle_ms: (i as u64 % 7) * 1_000,
    };
    info.unread = i % 2 == 0;
    info
}

/// Mean and worst wall-clock cost of absorbing one update, over `runs`.
fn measure(windows_per_fold: usize, runs: usize, fold: Fold) -> (f64, f64) {
    let roots = bench_roots();
    let mut daemon = bench_daemon(&roots);
    let mut windows = bench_windows(&daemon);
    windows.truncate(windows_per_fold);
    let clock = clock();

    // Warm every lazily-filled path before the first timed iteration.
    for i in 0..LOAD {
        let update = bench_update(&daemon, i, NOW);
        core::hint::black_box(absorb(&mut daemon, &mut windows, update, clock, fold));
    }

    let mut total = core::time::Duration::ZERO;
    let mut worst = core::time::Duration::ZERO;
    for i in 0..runs {
        let update = bench_update(&daemon, i, NOW);
        let at = std::time::Instant::now();
        core::hint::black_box(absorb(&mut daemon, &mut windows, update, clock, fold));
        let took = at.elapsed();
        total += took;
        worst = worst.max(took);
    }
    (
        total.as_secs_f64() * 1e3 / runs as f64,
        worst.as_secs_f64() * 1e3,
    )
}

/// One `SessionUpdated` at twenty sessions must be absorbed inside one
/// 60Hz frame. The daemon pushes one per live session per second, so an
/// over-budget fold does not drop one frame, it drops twenty a second and
/// the whole shell reads as sticky under exactly the load the product is
/// sold for.
///
/// Locks out: any change that puts per-update work back on the whole
/// session set — a full re-derivation of the sidebar tree, a re-`realpath`
/// of every project root, or a clone of the session vector.
#[test]
fn one_session_updated_is_absorbed_inside_a_frame() {
    let (cold, cold_worst) = measure(1, 200, Fold::Cold);
    let (memo, memo_worst) = measure(1, 200, Fold::Memo);
    println!(
        "SessionUpdated, one window:     refold {cold:.3} ms (worst {cold_worst:.3}), \
         memo {memo:.3} ms (worst {memo_worst:.3})"
    );
    let (cold20, _) = measure(LOAD, 50, Fold::Cold);
    let (memo20, memo20_worst) = measure(LOAD, 50, Fold::Memo);
    println!(
        "SessionUpdated, twenty windows: refold {cold20:.3} ms, \
         memo {memo20:.3} ms (worst {memo20_worst:.3})"
    );

    // Only release is held to the number. A debug build of this fold is
    // roughly an order of magnitude slower and asserting there would make
    // `cargo test` fail for a reason that has nothing to do with the code.
    if cfg!(debug_assertions) {
        return;
    }
    assert!(
        memo < FRAME_BUDGET.as_secs_f64() * 1e3,
        "one SessionUpdated took {memo:.3} ms against a 16 ms frame"
    );
    assert!(
        memo * 2.0 < cold,
        "the memo is meant to at least halve the derivation and did not: \
         {memo:.3} ms against {cold:.3} ms"
    );
}

// ---- The paint budget -----------------------------------------------
//
// `measure` above is the MESSAGE path: one daemon fold, twenty windows'
// reactions, and twenty derivations, all in one number. That is the right
// number for the objective and the wrong one for attributing a change,
// because halving a single derivation moves it by a few percent and the
// timer noise is the same order. What follows isolates the derivation ONE
// paint runs, and counts its allocations, which is the figure with no
// timer in it at all.

thread_local! {
    /// Allocations on this thread since the counter was armed.
    static ALLOCATIONS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
    /// Is this thread inside a measurement?
    static COUNTING: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

/// The system allocator, counting per thread.
///
/// Per THREAD, and deliberately: `vitrum-search`'s `no_allocation.rs`
/// counts into a process-global `AtomicUsize` and therefore has to carry a
/// comment forbidding a second `#[test]` in the file, because the harness
/// runs tests in parallel threads and every one of them allocates into the
/// same counter. This binary has six hundred tests in it and no such
/// option. A thread-local counter measures the thread that armed it and is
/// indifferent to what the other five hundred and ninety-nine are doing.
///
/// Both cells are `const`-initialised `Cell`s of `Copy` types, so the TLS
/// slot has no destructor and no lazy first-touch initialisation. That is
/// load-bearing rather than tidy: this code runs INSIDE the allocator, and
/// a TLS slot that allocated when first read would recurse into itself.
struct Counting;

/// Count one allocation against the current thread.
fn count_one() {
    if COUNTING.get() {
        ALLOCATIONS.set(ALLOCATIONS.get() + 1);
    }
}

unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        count_one();
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        count_one();
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, new_size: usize) -> *mut u8 {
        count_one();
        unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// How many times `body` allocated on this thread.
fn allocations(body: impl FnOnce()) -> usize {
    ALLOCATIONS.set(0);
    COUNTING.set(true);
    body();
    COUNTING.set(false);
    ALLOCATIONS.get()
}

/// Everything one paint asks the model for, and nothing else.
///
/// `ui::sidebar::Sidebar` touches state exactly twice: `tree` once, then
/// `attention_count_of` over that same tree. The sum is returned so the
/// optimiser cannot delete either half.
fn one_paint(daemon: &DaemonState, window: &WindowState, clock: Clock) -> usize {
    let tree = window.tree(daemon, clock);
    let waiting = window.attention_count_of(daemon, &tree, clock);
    waiting + tree.iter().map(SidebarGroup::len).sum::<usize>()
}

/// Mean microseconds, and allocations, for one paint's derivation over a
/// given daemon.
fn paint_cost_of(daemon: &DaemonState, runs: usize) -> (f64, usize) {
    let windows = bench_windows(daemon);
    let window = &windows[0];
    let clock = clock();

    // Warm both memos and every lazily-grown vector, so the count below is
    // a STEADY-STATE paint and not a first one.
    for _ in 0..64 {
        core::hint::black_box(one_paint(daemon, window, clock));
    }
    let allocs = allocations(|| {
        core::hint::black_box(one_paint(daemon, window, clock));
    });

    let at = std::time::Instant::now();
    for _ in 0..runs {
        core::hint::black_box(one_paint(daemon, window, clock));
    }
    (at.elapsed().as_secs_f64() * 1e6 / runs as f64, allocs)
}

/// The same daemon carrying `extra` more sessions in the SAME buckets.
///
/// More rows, not more headers. The question the scaling assertion asks is
/// whether a paint's cost tracks ROWS; adding buckets would answer a
/// different question and hide the one that matters. Each new row is a
/// copy of an existing one with a fresh id, so it keeps that row's
/// `project_id` and cwd and therefore lands in the bucket it was copied
/// from. They come through `SessionView::new`, unparked and unsettled, so
/// they pile into the Active band — which is the band with the preview cut
/// and the most machinery per row.
fn with_more_rows(roots: &[String], extra: usize) -> DaemonState {
    let mut st = bench_daemon(roots);
    let base = st.sessions.len() as u64;
    for i in 0..extra as u64 {
        let mut info = st.sessions[(i % base) as usize].info.clone();
        info.id = SessionId(10_000 + i);
        info.created_at_ms = NOW - HOUR - (base + i) * 1_000;
        st.sessions.push(SessionView::new(info));
    }
    let infos: Vec<SessionInfo> = st.sessions.iter().map(|row| row.info.clone()).collect();
    st.workspaces.adopt(infos.iter());
    st
}

/// A paint's allocations must track BUCKETS, not rows.
///
/// The allocation count is the real guard and the timing is a sanity
/// check, because the count is exact and reproducible on any machine while
/// the timer is neither.
///
/// Two measurements over the same eleven buckets, one at twenty rows and
/// one at two thousand, and the assertion is on the SLOPE between them:
/// allocations per extra row drawn. A `Vec` that doubles as it grows
/// contributes a logarithmic term, which is real and is not the defect;
/// dividing by the number of rows added amortises it away and leaves a
/// bound that only a genuinely per-row allocation can cross.
///
/// Locks out, by construction rather than by inspection: a `String` built
/// per row per paint, a `SessionView` cloned per row per paint, and the
/// flatten-then-look-each-id-back-up shape `attention_count_of` used to
/// have, which allocated a `Vec<SessionId>` per band of every bucket.
/// Each of those is one or more allocations per row and lands far above
/// the bound; the amortised doubling sits far below it.
#[test]
fn one_paint_derives_the_sidebar_without_allocating_per_row() {
    let roots = bench_roots();
    let (micros, allocs) = paint_cost_of(&bench_daemon(&roots), 2_000);
    let deep = with_more_rows(&roots, 99 * LOAD);
    let added = deep.sessions.len() - LOAD;
    // Fewer runs: this one is a hundred times the rows and the timing is
    // only printed, never asserted on.
    let (deep_micros, deep_allocs) = paint_cost_of(&deep, 100);
    let per_row = (deep_allocs - allocs) as f64 / added as f64;
    println!(
        "paint: {LOAD} rows {micros:.3} us / {allocs} allocations, \
         {} rows {deep_micros:.3} us / {deep_allocs} allocations \
         ({per_row:.3} per extra row)",
        deep.sessions.len()
    );

    let buckets = REPOS + 3;
    assert!(
        allocs < 8 * buckets,
        "one paint made {allocs} allocations over {buckets} buckets, which \
         is more than a handful per bucket"
    );
    assert!(
        per_row < 0.5,
        "{added} more rows in the same {buckets} buckets cost \
         {} more allocations, {per_row:.3} per row, so a paint allocates \
         per ROW rather than per bucket",
        deep_allocs - allocs
    );
    if cfg!(debug_assertions) {
        return;
    }
    assert!(
        micros < 1_000.0,
        "one paint's derivation took {micros:.3} us of a 16,600 us frame"
    );
}

/// The memo must produce EXACTLY the tree a full re-derivation produces,
/// at every step of a sequence that moves each of its inputs.
///
/// Locks out a stale [`FoldedProjects`]: a fold kept across a project list
/// that has changed draws a header for a root the daemon dropped, files
/// rows under a bucket that no longer exists, or keys a collapse bit to a
/// directory the operator never collapsed. All three are silent — the
/// sidebar still renders — which is why this compares whole trees rather
/// than counts.
#[test]
fn the_memo_and_a_full_re_derivation_agree_at_every_step() {
    let roots = bench_roots();
    let mut daemon = bench_daemon(&roots);
    let mut window = bench_windows(&daemon).remove(0);
    let clock = clock();

    // A first tree, so every later step starts from a WARM memo. Comparing
    // two cold derivations would prove nothing.
    let first = agree(&daemon, &window, clock, "at rest");
    assert_eq!(
        first.len(),
        REPOS + 3,
        "eight repository buckets and three detached directories"
    );
    assert_eq!(
        first.iter().map(SidebarGroup::len).sum::<usize>(),
        LOAD,
        "every session lands in exactly one bucket"
    );

    for i in 0..LOAD {
        let update = bench_update(&daemon, i, NOW);
        daemon.apply(ServerMsg::SessionUpdated(update));
        agree(&daemon, &window, clock, "after an update");
    }

    // The daemon registers a project for one of the detached directories.
    // The bucket must move from Directory to Project, which is the single
    // most visible thing a stale fold would get wrong.
    let detached = roots[REPOS].clone();
    let mut projects = daemon.projects.clone();
    projects.push(ProjectInfo {
        id: ProjectId(2000),
        name: "adopted".to_string(),
        root: detached.clone(),
    });
    daemon.apply(ServerMsg::Projects {
        projects: projects.clone(),
    });
    let mut orphan = daemon.sessions[LOAD - 3].info.clone();
    orphan.project_id = ProjectId(2000);
    daemon.apply(ServerMsg::SessionUpdated(orphan));
    let adopted = agree(&daemon, &window, clock, "after a project was registered");
    assert!(
        adopted
            .iter()
            .any(|g| g.key == GroupKey::Project(ProjectId(inbox::fnv1a(detached.as_bytes())))),
        "the newly registered root must be drawn as a project bucket"
    );

    // A rename changes no key, and must still change the header text
    // without invalidating anything: `lead` is an index, not a copy.
    projects[0].name = "renamed".to_string();
    daemon.apply(ServerMsg::Projects {
        projects: projects.clone(),
    });
    let renamed = agree(&daemon, &window, clock, "after a project was renamed");
    assert!(
        renamed.iter().any(|g| g.label == "renamed"),
        "a renamed project must read its new name on the next paint"
    );

    // Same id, different root. This is the case an id-only or a length-only
    // staleness check gets wrong, and it re-keys the bucket.
    projects[0].root = roots[REPOS + 1].clone();
    daemon.apply(ServerMsg::Projects {
        projects: projects.clone(),
    });
    agree(&daemon, &window, clock, "after a root moved under one id");

    // Projects removed, down to none at all.
    projects.truncate(2);
    daemon.apply(ServerMsg::Projects {
        projects: projects.clone(),
    });
    agree(&daemon, &window, clock, "after projects were dropped");
    daemon.apply(ServerMsg::Projects {
        projects: Vec::new(),
    });
    let none = agree(&daemon, &window, clock, "with no projects at all");
    assert!(
        none.iter().all(|g| g.project.is_none()),
        "with no project records every bucket is a bare directory"
    );

    // A filter, which cuts rows before bucketing, and the other grouping
    // mode, which does not consult the fold at all.
    daemon.apply(ServerMsg::Projects { projects });
    window.filter = "pass 0".to_string();
    agree(&daemon, &window, clock, "under a filter");
    window.filter.clear();
    daemon
        .workspaces
        .get_mut(DEFAULT_WORKSPACE)
        .unwrap()
        .grouping = Grouping::Named;
    agree(&daemon, &window, clock, "grouped by folder");
}

/// A session that moves to a directory the memo has never resolved must
/// still land in the right bucket.
///
/// This is the one thing a cwd memo can get wrong that `agree` above
/// CANNOT reveal, and the distinction is the whole reason this test
/// exists: `agree` calls [`DaemonState::forget_dir_keys`] before
/// comparing, so a memo that never invalidated itself would simply be
/// rebuilt there and both trees would match. Verified by mutation — a
/// `dir_keys` whose staleness check reads `keys.is_empty()` instead of
/// comparing the cwds passes every other test in this file.
///
/// The oracle here is a `DaemonState` that has never held a memo at all,
/// built field by field so `..Default::default()` supplies empty caches:
/// cloning would carry the memo across, since `Cache` clones its contents.
/// That is an oracle a stale memo cannot pass.
///
/// The move is to a TRAILING-SLASH spelling of a directory that already
/// has a bucket, because a memo miss falls back to the raw cwd text and
/// raw text that is already canonical would hide the miss. `/dir` and
/// `/dir/` are one directory, so the correct answer is one bucket holding
/// two rows; a stale memo draws two headers over one folder, which is
/// exactly the defect [`inbox::project_key`] exists to prevent.
#[test]
fn a_session_that_moves_to_an_unresolved_directory_re_keys_its_bucket() {
    let roots = bench_roots();
    let mut daemon = bench_daemon(&roots);
    let window = bench_windows(&daemon).remove(0);
    let clock = clock();

    // Warm the memo against the cwds the fixture started with.
    let before = window.tree(&daemon, clock);
    let detached = GroupKey::Directory(directory_key(&roots[REPOS + 2]));
    assert!(
        before.iter().any(|g| g.key == detached),
        "the fixture must start with a bucket for the directory being emptied"
    );

    // Session 18 runs in `roots[REPOS + 2]` and is the only row there.
    // Move it to a second spelling of `roots[REPOS]`, where session 19
    // already runs.
    let mut moved = daemon.sessions[LOAD - 3].info.clone();
    assert_eq!(moved.cwd, roots[REPOS + 2]);
    moved.cwd = format!("{}/", roots[REPOS]);
    daemon.apply(ServerMsg::SessionUpdated(moved));

    let fresh = DaemonState {
        projects: daemon.projects.clone(),
        sessions: daemon.sessions.clone(),
        workspaces: daemon.workspaces.clone(),
        settings: daemon.settings.clone(),
        ..DaemonState::default()
    };
    let after = window.tree(&daemon, clock);
    assert_eq!(
        after,
        window.tree(&fresh, clock),
        "the memoised tree and one derived with no memo at all disagree, \
         so the cwd memo went stale under a session that moved"
    );

    let shared = GroupKey::Directory(directory_key(&roots[REPOS]));
    assert!(
        !after.iter().any(|g| g.key == detached),
        "the directory the session left must stop drawing a header"
    );
    assert_eq!(
        after.iter().filter(|g| g.key == shared).count(),
        1,
        "two spellings of one directory drew two headers"
    );
    assert_eq!(
        after
            .iter()
            .find(|g| g.key == shared)
            .map(SidebarGroup::len),
        Some(2),
        "both rows must be in the one bucket that directory has"
    );
}

/// Derive the tree twice — once from whatever the memos hold, once after
/// throwing BOTH away — and fail unless the two are identical.
///
/// Both, because there are two: the project fold and the cwd resolution.
/// Dropping only one would leave the other's staleness untested, which is
/// exactly the hole a second memo opens.
fn agree<'a>(
    daemon: &'a DaemonState,
    window: &WindowState,
    clock: Clock,
    what: &str,
) -> Vec<SidebarGroup<'a>> {
    let memoised = window.tree(daemon, clock);
    daemon.forget_folded();
    daemon.forget_dir_keys();
    let refolded = window.tree(daemon, clock);
    assert_eq!(
        memoised, refolded,
        "{what}: the memoised fold and a full re-derivation drew different sidebars"
    );
    refolded
}

/// A directory bucket's KEY must be derivable from the directory alone.
///
/// Deliberately distinct from `two_spellings_of_one_cwd_draw_one_directory_bucket`
/// above, which is the older guard and is NOT redundant with this one:
/// it asserts the bucket count, its label and its rows, and never looks at
/// [`GroupKey`]. A `directory_key` that computed a wrong but self-consistent
/// value would leave that test green, because every row would still land in
/// one correctly-labelled bucket carrying a key nothing else could name.
/// The key is exactly what changed when [`WindowState::bucket_by_directory`]
/// stopped canonicalising a path that had already been canonicalised, so it
/// needs a guard that reads it.
///
/// Locks out two things. Making [`inbox::project_key`] non-idempotent,
/// which would leave a bucket carrying a key that nothing else in the
/// window — not the collapse set, not `reveal`, not a band toggle — could
/// ever name. And splitting a bucket from its own key: `/tmp/x` and
/// `/tmp/x/` must not become two headers over one directory.
#[test]
fn a_directory_bucket_is_keyed_by_its_canonical_path_alone() {
    for raw in [
        std::env::temp_dir().to_string_lossy().into_owned(),
        format!("{}/", std::env::temp_dir().to_string_lossy()),
        "/tmp/vitrum-no-such-directory-4f21".to_string(),
        "/tmp/vitrum-no-such-directory-4f21/".to_string(),
    ] {
        let once = inbox::project_key(&raw);
        assert_eq!(
            inbox::project_key(&once),
            once,
            "project_key is not idempotent for {raw}, so the bucket and its \
             key would be computed from different strings"
        );
    }

    // Two sessions in one directory spelled two ways, and no project
    // record for it, so both take the bucket-per-cwd path.
    let plain = std::env::temp_dir().to_string_lossy().into_owned();
    let mut daemon = DaemonState::default();
    daemon.sessions = vec![
        row(10).project(99).cwd(&plain).waiting(Some(false)).build(),
        row(11)
            .project(99)
            .cwd(&format!("{plain}/"))
            .waiting(Some(false))
            .build(),
    ];
    let infos: Vec<SessionInfo> = daemon.sessions.iter().map(|r| r.info.clone()).collect();
    daemon.workspaces.adopt(infos.iter());

    let tree = window_on(DEFAULT_WORKSPACE).tree(&daemon, clock());
    assert_eq!(tree.len(), 1, "one directory drew more than one header");
    assert_eq!(
        tree[0].key,
        GroupKey::Directory(directory_key(&inbox::project_key(&plain))),
        "the bucket's key must be nameable from the directory alone"
    );
    // Same creation instant, so the static comparator falls through to the
    // id, ascending. What matters here is that both rows are in ONE band
    // of ONE bucket.
    assert_eq!(ids(&tree[0].bands.active), vec![10, 11]);
}

/// [`vitrum_model::ActiveOrder::Static`] is the default, and it means a
/// row never moves under the pointer because something about it changed.
/// An update that leaves a row in the inbox must leave the inbox in the
/// same order, newest first by creation.
///
/// Locks out two things. Reordering rows as a side effect of a derivation
/// change: a list that reshuffles on every daemon push turns clicking a
/// row into a race, and it is invisible in a screenshot because every
/// frame of it looks like a correct sidebar. And swapping the comparator
/// itself, which is why the rows below carry two DIFFERENT urgencies.
///
/// An earlier version of this test gave all six rows one status, and it
/// escaped a mutation that swapped `ActiveOrder::Static` for
/// `ActiveOrder::Urgency`. With one urgency and one attention priority
/// across the set, [`vitrum_model::order::UrgencyKey`] falls through to
/// creation time and produces the identical order, so the test could not
/// tell the two comparators apart and would have passed under either. The
/// Approval rows below outrank the Input rows, so Urgency interleaves them
/// as [15, 13, 11, 14, 12, 10] and only Static gives the run below.
#[test]
fn an_update_never_moves_a_row_that_stayed_in_the_inbox() {
    // Every row blocks on the operator, which pins its disposition to
    // Active whatever else moves. That is the point: this test is about
    // ORDER inside a band, and a row that legitimately changes band would
    // otherwise fail it for the wrong reason.
    let mut daemon = DaemonState::default();
    daemon.projects = vec![project(1, "one")];
    daemon.sessions = (0..6u64)
        .map(|i| {
            let built = row(10 + i)
                .project(1)
                .waiting(Some(true))
                .created_at_ms(NOW - HOUR + i * 1_000)
                .last_activity_ms(NOW - HOUR + i * 1_000);
            // Approval outranks Input, so the set spans two urgencies and
            // a urgency comparator cannot reproduce creation order.
            if i % 2 == 1 {
                built.hint(
                    HintState::Approval,
                    Some("approve this write?"),
                    NOW - 60_000,
                )
            } else {
                built
            }
            .build()
        })
        .collect();
    let infos: Vec<SessionInfo> = daemon.sessions.iter().map(|r| r.info.clone()).collect();
    daemon.workspaces.adopt(infos.iter());
    let window = window_on(DEFAULT_WORKSPACE);
    let clock = clock();

    let inbox_of = |daemon: &DaemonState| -> Vec<u64> {
        window
            .tree(daemon, clock)
            .iter()
            .flat_map(|g| g.bands.active.iter().map(|r| r.id().0).collect::<Vec<_>>())
            .collect()
    };

    // The two urgencies really are distinct, or the run below would prove
    // nothing about which comparator produced it.
    let urgencies: Vec<u8> = daemon
        .sessions
        .iter()
        .map(|r| r.status().urgency())
        .collect();
    assert_eq!(
        urgencies,
        vec![1, 4, 1, 4, 1, 4],
        "two distinct urgencies, so a urgency comparator cannot reproduce \
         creation order by falling through to it"
    );

    assert_eq!(
        inbox_of(&daemon),
        vec![15, 14, 13, 12, 11, 10],
        "the inbox is creation order, newest first, NOT urgency order"
    );

    // Move every fact the row carries except the one that decides its
    // band: activity, unread and the bell, in an order that would sort
    // the list backwards under any recency comparator.
    for i in 0..6u64 {
        let mut info = daemon.sessions[i as usize].info.clone();
        info.last_activity_ms = NOW - i * 60_000;
        info.unread = i % 2 == 0;
        info.attention.bell = i % 3 == 0;
        info.attention.idle_ms = (6 - i) * 10_000;
        daemon.apply(ServerMsg::SessionUpdated(info));
    }

    assert_eq!(
        inbox_of(&daemon),
        vec![15, 14, 13, 12, 11, 10],
        "a row moved in the inbox because its activity or its badge changed"
    );
}
#[test]
fn fine_grained_selectors_and_memoized_tree() {
    let mut st = UiState::default();
    let initial_rev = st.state_revision();

    assert_eq!(st.select_sessions().len(), 0);
    assert_eq!(st.select_workspace_id(), DEFAULT_WORKSPACE);
    assert_eq!(st.select_filter_query(), "");
    assert!(!st.has_changed_since(initial_rev));

    // Bumping revisions
    st.daemon.sessions_revision = st.daemon.sessions_revision.wrapping_add(1);
    assert!(st.has_changed_since(initial_rev));
    assert_ne!(st.state_revision(), initial_rev);

    // Initial tree computation populates memo
    let c = Clock::utc(NOW);
    let tree1 = st.tree(c);
    assert!(st.window.tree_memo.borrow().is_some());

    // Subsequent tree call with unchanged state revision returns cached tree
    let tree2 = st.tree(c);
    assert_eq!(tree1.len(), tree2.len());

    // Changing window filter invalidates tree memo key
    st.window.filter = "test".to_string();
    st.window.filter_revision = st.window.filter_revision.wrapping_add(1);
    let tree3 = st.tree(c);
    assert_eq!(st.select_filter_query(), "test");
    assert_eq!(tree3.len(), 0);
}
