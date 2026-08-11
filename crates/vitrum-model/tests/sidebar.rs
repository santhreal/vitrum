//! End-to-end exercise of the sidebar model through its PUBLIC surface only.
//!
//! The unit tests inside the crate can reach private items and each other's
//! helpers. This one is compiled as a separate crate, so it also proves the
//! public API is complete and usable: a missing re-export or an over-private
//! field fails here and nowhere else.
//!
//! The scenario is the one the product is built for: twenty concurrent agents
//! in three projects, of which exactly one wants the operator right now.

use vitrum_model::{
    ActiveOrder, Clock, Direction, Disposition, DispositionPolicy, HintParser, ProjectGroup,
    Section, SectionCounts, Selection, SelectionFacts, SessionView, SidebarStatus, Snooze,
    StatusSource, Wrap, adjacent_matching, arrange, context_menu, rollup_all, visible_session_ids,
    wake_description,
};
use vitrum_proto::{
    AgentHint, Attention, HintState, ProjectId, SessionId, SessionInfo, SessionStatus,
};

const NOW: u64 = 1_772_580_600_000;
const HOUR: u64 = 3_600_000;

fn clock() -> Clock {
    Clock::utc(NOW)
}

fn policy() -> DispositionPolicy {
    DispositionPolicy::manual()
}

fn session(id: u64, project: u64, created_at_ms: u64) -> SessionInfo {
    SessionInfo {
        id: SessionId(id),
        project_id: ProjectId(project),
        title: format!("agent {id}"),
        cwd: "/srv/work".to_string(),
        command: "claude".to_string(),
        args: Vec::new(),
        status: SessionStatus::Running,
        created_at_ms,
        last_activity_ms: NOW - 1_000,
        cols: 120,
        rows: 40,
        git_branch: Some("main".to_string()),
        worktree: None,
        unread: false,
        attention: Attention {
            bell: false,
            idle_ms: 0,
            failed: false,
            // Every agent is computing unless a case below says otherwise.
            waiting: Some(false),
        },
        hint: None,
        term_title: None,
    }
}

/// Twenty agents, three projects, one approval buried in the middle.
fn fleet() -> Vec<SessionView> {
    let mut rows: Vec<SessionView> = (1..=20)
        .map(|id| SessionView::new(session(id, 1 + id % 3, 1_000 + id * 10)))
        .collect();

    // 7 is blocked asking for approval, and it is the only one.
    rows[6].info.attention.waiting = Some(true);
    rows[6].info.hint = Some(AgentHint {
        state: HintState::Approval,
        label: Some("git push --force origin main".to_string()),
        received_at_ms: NOW - 30_000,
    });

    // 12 crashed a minute ago and nobody has looked.
    rows[11].info.status = SessionStatus::Exited { code: Some(101) };
    rows[11].info.attention.failed = true;
    rows[11].info.attention.waiting = None;
    rows[11].info.last_activity_ms = NOW - 60_000;
    rows[11].info.unread = true;

    // 3 finished cleanly and was read; drained history.
    rows[2].info.status = SessionStatus::Exited { code: Some(0) };
    rows[2].info.attention.waiting = None;
    rows[2].info.last_activity_ms = NOW - 4 * HOUR;
    rows[2].last_visited_ms = Some(NOW - 3 * HOUR);

    // 15 is parked until the morning.
    rows[14].info.last_activity_ms = NOW - 2 * HOUR;
    rows[14].snooze = Some(Snooze {
        snoozed_at_ms: NOW - 2 * HOUR,
        wake_at_ms: NOW + 10 * HOUR,
    });

    // 19 is at its prompt, unhinted: the kernel says the next move is ours.
    rows[18].info.attention.waiting = Some(true);
    rows[18].info.unread = true;

    rows
}

/// The headline claim: with twenty agents running, the model can name the one
/// that wants a human, without the operator scanning anything.
#[test]
fn one_blocked_agent_among_twenty_is_findable_without_scanning() {
    let rows = fleet();
    let blocked: Vec<u64> = rows
        .iter()
        .filter(|row| row.status() == SidebarStatus::Approval)
        .map(|row| row.id().0)
        .collect();
    assert_eq!(blocked, vec![7]);

    let working = rows
        .iter()
        .filter(|row| row.status() == SidebarStatus::Working)
        .count();
    assert_eq!(working, 16);

    let resolution = rows[6].resolve_status();
    assert_eq!(resolution.status, SidebarStatus::Approval);
    assert_eq!(resolution.source, StatusSource::Hint);
    assert_eq!(rows[6].hint_label(), Some("git push --force origin main"));
}

/// Three states are OBSERVED with no agent cooperation at all. This is the
/// difference from a shell that parses per-harness event streams: session 19
/// has never emitted a hint and still reports a proven state.
#[test]
fn the_observed_states_need_no_agent_cooperation() {
    let rows = fleet();

    let at_prompt = &rows[18];
    assert_eq!(at_prompt.info.hint, None);
    assert_eq!(at_prompt.status(), SidebarStatus::Ready);
    assert_eq!(at_prompt.resolve_status().source, StatusSource::Waiting);
    assert!(!at_prompt.resolve_status().source.is_inferred());

    let computing = &rows[0];
    assert_eq!(computing.info.hint, None);
    assert_eq!(computing.status(), SidebarStatus::Working);
    assert_eq!(computing.resolve_status().source, StatusSource::Foreground);

    let crashed = &rows[11];
    assert_eq!(crashed.status(), SidebarStatus::Failed);
    assert_eq!(crashed.resolve_status().source, StatusSource::Exit);
}

/// A platform that cannot answer the probe must say so rather than claim
/// certainty. Same row, same data, `waiting` blanked: the state is still
/// useful and is now marked inferred.
#[test]
fn a_platform_without_the_probe_degrades_to_marked_inference() {
    let mut row = fleet()[18].clone();
    row.info.attention.waiting = None;
    row.info.attention.idle_ms = 45_000;

    assert_eq!(row.status(), SidebarStatus::Ready);
    assert_eq!(row.resolve_status().source, StatusSource::Idle);
    assert!(
        row.resolve_status().source.is_inferred(),
        "Windows must be able to tell the operator this is a guess"
    );
}

/// The three bands, in order, with the fold doing the work. The defect this
/// prevents is a streaming session sitting below dead ones.
#[test]
fn the_fleet_arranges_into_three_bands() {
    let mut rows = fleet();
    let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Static);

    assert_eq!(split.active_len(), 18);
    assert_eq!(split.snoozed_len(), 1);
    assert_eq!(split.settled_len(rows.len()), 1);

    assert_eq!(rows[split.active_end].id(), SessionId(15));
    assert_eq!(rows[split.snoozed_end].id(), SessionId(3));
    assert_eq!(split.section_at(0), Section::Active);
    assert_eq!(split.section_at(18), Section::Snoozed);
    assert_eq!(split.section_at(19), Section::Settled);

    // Static order: newest first, and status does NOT move anything.
    let inbox: Vec<u64> = rows[..split.active_end].iter().map(|row| row.id().0).collect();
    assert_eq!(
        inbox,
        vec![20, 19, 18, 17, 16, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 2, 1]
    );

    let counts = SectionCounts::of(&rows, clock(), policy());
    assert_eq!(counts.inbox(), 18);
    assert_eq!(counts.snoozed, 1);
    assert_eq!(counts.settled, 1);
    assert_eq!(counts.woke, 0);
}

/// The opt-in ordering does put the blocked row on top, for a caller that
/// prefers that trade.
#[test]
fn the_urgency_ordering_lifts_the_blocked_row_to_the_top() {
    let mut rows = fleet();
    let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Urgency);
    let inbox: Vec<u64> = rows[..split.active_end].iter().map(|row| row.id().0).collect();
    assert_eq!(inbox[0], 7, "the approval");
    assert_eq!(inbox[1], 12, "the unseen crash");
    assert_eq!(inbox[2], 19, "at its prompt");
    assert_eq!(rows[3].status(), SidebarStatus::Working);
}

/// Rather than reordering the list under the operator's hands, one keypress
/// jumps to the next row that wants them. This is what pays for the static
/// order.
#[test]
fn one_keypress_reaches_every_row_that_wants_the_operator() {
    let mut rows = fleet();
    let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Static);
    let inbox = &rows[..split.active_end];

    let groups = vec![ProjectGroup::new(
        ProjectId(0),
        inbox.iter().map(SessionView::id).collect(),
    )];
    let visible = visible_session_ids(&groups);
    assert_eq!(visible.len(), 18);

    let wants_me = |id: SessionId| {
        inbox
            .iter()
            .find(|row| row.id() == id)
            .is_some_and(|row| row.status().wants_operator())
    };

    let mut cursor = None;
    let mut visited = Vec::new();
    for _ in 0..3 {
        cursor = adjacent_matching(&visible, cursor, Direction::Next, Wrap::Around, wants_me);
        visited.push(cursor.expect("three rows want the operator").0);
    }
    assert_eq!(visited, vec![19, 12, 7]);

    // And it cycles rather than stopping at the bottom.
    let wrapped = adjacent_matching(&visible, cursor, Direction::Next, Wrap::Around, wants_me);
    assert_eq!(wrapped, Some(SessionId(19)));
}

/// A collapsed project takes its rows out of reach entirely, and its header
/// carries the single most urgent state inside it.
#[test]
fn collapsing_a_project_hides_its_rows_but_not_its_urgency() {
    let rows = fleet();
    let rollups = rollup_all(&rows, clock(), policy());
    assert_eq!(rollups.len(), 3);

    let with_approval = rollups
        .iter()
        .find(|rollup| rollup.counts.approval > 0)
        .expect("session 7 is somewhere");
    assert_eq!(with_approval.indicator, Some(SidebarStatus::Approval));
    assert_eq!(with_approval.project_id, ProjectId(1 + 7 % 3));

    let members: Vec<SessionId> = rows
        .iter()
        .filter(|row| row.project_id() == with_approval.project_id)
        .map(SessionView::id)
        .collect();
    assert!(members.contains(&SessionId(7)));

    let mut groups = vec![ProjectGroup::new(with_approval.project_id, members.clone())];
    assert_eq!(visible_session_ids(&groups), members);

    groups[0].collapsed = true;
    assert_eq!(visible_session_ids(&groups), Vec::new());
    assert_eq!(
        adjacent_matching(&[], None, Direction::Next, Wrap::Around, |_| true),
        None
    );
}

/// The inbox drains and refills purely from the clock, with nothing scheduled.
/// Parking a row, its wake, and the badge clearing on a visit, all derived.
#[test]
fn the_inbox_drains_and_refills_without_a_single_timer() {
    let mut rows = fleet();
    let parked = rows.iter().position(|row| row.id() == SessionId(15)).unwrap();

    assert_eq!(
        rows[parked].disposition(clock(), policy()),
        Disposition::Snoozed
    );
    // NOW is 2026-03-03T23:30Z, so a ten-hour park crosses midnight: the label
    // has to say tomorrow even though the wake is ten hours out, not a day.
    assert_eq!(
        wake_description(rows[parked].snooze.unwrap().wake_at_ms, clock()),
        "tomorrow 9:30"
    );

    // Ten hours later, with nothing mutated in between.
    let morning = Clock::utc(NOW + 10 * HOUR);
    assert_eq!(rows[parked].disposition(morning, policy()), Disposition::Woke);
    assert_eq!(rows[parked].woke_at(morning), Some(NOW + 10 * HOUR));

    // It returns to the inbox in its ORIGINAL position, badge doing the work.
    let split = arrange(&mut rows, morning, policy(), ActiveOrder::Static);
    let inbox: Vec<u64> = rows[..split.active_end].iter().map(|row| row.id().0).collect();
    assert_eq!(
        inbox,
        vec![20, 19, 18, 17, 16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 2, 1],
        "row 15 is back between 16 and 14, not on top"
    );

    // Looking at it clears the badge.
    let index = rows.iter().position(|row| row.id() == SessionId(15)).unwrap();
    rows[index].last_visited_ms = Some(NOW + 10 * HOUR + 1);
    assert_eq!(rows[index].disposition(morning, policy()), Disposition::Active);
}

/// A parked row comes back early when the agent needs a human, and the operator
/// is never offered a snooze that would hide a pending approval.
#[test]
fn a_parked_row_raises_its_hand_and_cannot_be_parked_while_blocked() {
    let mut rows = fleet();
    let index = rows.iter().position(|row| row.id() == SessionId(15)).unwrap();
    assert!(rows[index].can_snooze());

    rows[index].info.hint = Some(AgentHint {
        state: HintState::Approval,
        label: Some("delete 4 files".to_string()),
        received_at_ms: NOW - HOUR,
    });
    rows[index].info.attention.waiting = Some(true);

    assert!(rows[index].raised_hand());
    assert!(!rows[index].effective_snoozed(clock()));
    assert_eq!(rows[index].disposition(clock(), policy()), Disposition::Active);
    assert!(
        !rows[index].can_snooze(),
        "re-parking a row that is blocked on you must be refused"
    );
    assert_eq!(
        rows[index].snooze.unwrap().wake_at_ms,
        NOW + 10 * HOUR,
        "the snooze itself was never mutated"
    );
}

/// Bulk actions over a multi-selection, with the counts and guards the menu
/// needs. The count in every label is what stops a bulk close hitting twenty
/// rows instead of two.
#[test]
fn multi_select_drives_a_context_menu_with_honest_counts() {
    let mut rows = fleet();
    let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Static);
    let visible: Vec<SessionId> = rows[..split.active_end].iter().map(SessionView::id).collect();

    let mut selection = Selection::new();
    selection.select_one(visible[0]);
    selection.extend_to(&visible, visible[3]);
    assert_eq!(selection.len(), 4);
    assert_eq!(selection.ordered(&visible), visible[..4].to_vec());

    let facts = SelectionFacts::collect(&selection, &rows, clock(), policy());
    assert_eq!(facts.count, 4);
    let menu = context_menu(facts);
    assert!(menu.iter().any(|item| item.label == "Close (4, 4 running)"));
    assert!(menu.iter().any(|item| item.destructive));

    // Extend across the approval row: snoozing the group must be refused.
    let approval_index = visible.iter().position(|id| *id == SessionId(7)).unwrap();
    selection.extend_to(&visible, visible[approval_index]);
    let facts = SelectionFacts::collect(&selection, &rows, clock(), policy());
    assert_eq!(facts.count, approval_index + 1);
    let menu = context_menu(facts);
    let snooze = menu
        .iter()
        .find(|item| item.label.starts_with("Snooze"))
        .expect("snooze is always offered");
    assert!(
        snooze.disabled,
        "a selection containing a pending approval cannot be parked"
    );

    // Closing three of them prunes the selection rather than leaving ghosts.
    let closed: Vec<SessionId> = visible.iter().copied().skip(1).take(3).collect();
    let remaining: Vec<SessionId> = visible
        .iter()
        .copied()
        .filter(|id| !closed.contains(id))
        .collect();
    selection.retain_visible(&remaining);
    assert!(closed.iter().all(|id| !selection.contains(*id)));
    assert_eq!(
        SelectionFacts::collect(&selection, &rows, clock(), policy()).count,
        approval_index + 1 - 3
    );
}

/// A harness opts in mid-stream, split across PTY reads exactly as the kernel
/// would deliver it, and the row upgrades from an observed Ready to a specific
/// request with a label.
#[test]
fn an_opted_in_harness_upgrades_a_row_mid_stream() {
    let mut rows = fleet();
    let index = rows.iter().position(|row| row.id() == SessionId(19)).unwrap();
    assert_eq!(rows[index].status(), SidebarStatus::Ready);
    assert_eq!(rows[index].hint_label(), None);

    let mut parser = HintParser::new();
    let mut declarations = Vec::new();
    // A real read boundary in the middle of the sequence, with ordinary output
    // on both sides.
    parser.feed(b"running tests...\x1b]7373;appro", &mut declarations);
    assert!(declarations.is_empty());
    assert!(parser.is_mid_sequence());
    parser.feed(b"val;rm -rf target\x1b\\ok\r\n", &mut declarations);

    assert_eq!(declarations.len(), 1);
    assert_eq!(parser.accepted(), 1);
    assert_eq!(parser.rejected(), 0);

    rows[index].info.hint = Some(declarations.remove(0).into_hint(NOW));
    assert_eq!(rows[index].status(), SidebarStatus::Approval);
    assert_eq!(rows[index].hint_label(), Some("rm -rf target"));
    assert_eq!(rows[index].resolve_status().source, StatusSource::Hint);
    assert!(!rows[index].can_snooze());
}

/// Hostile output cannot manufacture a state. An agent printing another
/// program's captured output, or a malformed near-miss, must leave the row
/// exactly as observation found it.
#[test]
fn hostile_output_cannot_manufacture_a_hint() {
    let mut parser = HintParser::new();
    let mut declarations = Vec::new();

    parser.feed(b"\x1b]0;window title\x07", &mut declarations);
    parser.feed(b"\x1b]7373;PANIC\x07", &mut declarations);
    parser.feed(b"\x1b]7373;ready\nfake\x07", &mut declarations);
    parser.feed(b"\x1b]07373;approval\x07", &mut declarations);
    parser.feed(&[0x1b, b']', b'7', b'3', b'7', b'3', b';', b'r', 0xff, 0x07], &mut declarations);
    parser.feed(b"\x1b]7373;approval", &mut declarations);
    parser.feed(&vec![b'x'; 4096], &mut declarations);

    assert_eq!(declarations, Vec::new());
    assert_eq!(parser.accepted(), 0);
    assert_eq!(parser.rejected(), 6);
    assert_eq!(parser.pending_bytes(), 0);

    // Still working afterwards.
    parser.feed(b"\x1b]7373;ready;done\x07", &mut declarations);
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].state, HintState::Ready);
}
