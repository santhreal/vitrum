//! Regression test suite for `vitrum-model` state ordering, project rollups,
//! selection range retention, and attention alignment.

use vitrum_model::{
    arrange, arrange_sections, rollup_all, rollup_project, ActiveOrder, Clock, DispositionPolicy,
    ProjectRollup, Section, SectionSplit, Selection, SelectionFacts, SidebarStatus, Snooze,
    StatusSource, SessionView,
};
use vitrum_proto::{
    AgentHint, Attention, HintState, ProjectId, SessionId, SessionInfo, SessionStatus,
    IDLE_ATTENTION_MS,
};

const NOW: u64 = 1_772_580_600_000;
const HOUR: u64 = 3_600_000;

fn clock() -> Clock {
    Clock::utc(NOW)
}

fn policy() -> DispositionPolicy {
    DispositionPolicy::manual()
}

fn build_session(id: u64, project: u64, created_at_ms: u64) -> SessionView {
    SessionView::new(SessionInfo {
        id: SessionId(id),
        project_id: ProjectId(project),
        title: format!("session-{id}"),
        cwd: "/srv/work".to_string(),
        command: "claude".to_string(),
        args: Vec::new(),
        status: SessionStatus::Running,
        created_at_ms,
        last_activity_ms: NOW - 1_000,
        cols: 120,
        rows: 40,
        git_branch: Some("main".to_string()),
        unread: false,
        attention: Attention {
            bell: false,
            idle_ms: 0,
            failed: false,
            waiting: Some(false),
        },
        hint: None,
    })
}

fn session_ids(rows: &[SessionView]) -> Vec<u64> {
    rows.iter().map(|r| r.id().0).collect()
}

/// WHY: Defends single-pass bucket sorting within ActiveOrder::Urgency where two active
/// sessions have identical status urgency (SidebarStatus::Ready), verifying that
/// Attention::priority() resolves the tie before creation time or SessionId.
#[test]
fn test_arrange_urgency_order_status_tiebreaker_by_attention() {
    let mut session1 = build_session(1, 1, 2_000);
    session1.info.attention.waiting = Some(true); // Ready, priority = 3
    session1.info.attention.bell = false;

    let mut session2 = build_session(2, 1, 1_000);
    session2.info.attention.waiting = Some(false);
    session2.info.attention.bell = true; // Ready (via bell), priority = 2

    let mut session3 = build_session(3, 1, 500);
    session3.info.attention.failed = true; // Failed status (urgency = 2)

    let mut rows = vec![session2, session1, session3];
    let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Urgency);

    assert_eq!(split.active_len(), 3);
    // Highest urgency status first: Failed (Session 3, urgency 2).
    // Then Ready status (urgency 1): Session 1 (priority 3, waiting=true) outranks Session 2 (priority 2, bell=true) despite Session 1 being newer.
    assert_eq!(session_ids(&rows), vec![3, 1, 2]);
}

/// WHY: Defends single-pass bucket sorting tie-breakers when both status urgency and
/// Attention::priority() are equal, confirming newest creation timestamp wins, and when
/// creation timestamps match, SessionId numerical order acts as total deterministic tie-breaker.
#[test]
fn test_arrange_urgency_order_tiebreaker_by_creation_and_id() {
    // Case A: Equal urgency and attention, different creation timestamps.
    let mut s1 = build_session(1, 1, 1_000);
    s1.info.attention.waiting = Some(true); // urgency 1, attention priority 3

    let mut s2 = build_session(2, 1, 2_000);
    s2.info.attention.waiting = Some(true); // urgency 1, attention priority 3

    let mut rows_a = vec![s1, s2];
    arrange(&mut rows_a, clock(), policy(), ActiveOrder::Urgency);
    // Newest creation timestamp (2_000 -> s2) outranks older (1_000 -> s1).
    assert_eq!(session_ids(&rows_a), vec![2, 1]);

    // Case B: Equal urgency, attention, AND creation timestamp -> SessionId ascending tie-breaker.
    let mut s10 = build_session(10, 1, 1_000);
    s10.info.attention.waiting = Some(true);

    let mut s5 = build_session(5, 1, 1_000);
    s5.info.attention.waiting = Some(true);

    let mut rows_b = vec![s10, s5];
    arrange(&mut rows_b, clock(), policy(), ActiveOrder::Urgency);
    // SessionId ascending tiebreak: 5 comes before 10.
    assert_eq!(session_ids(&rows_b), vec![5, 10]);
}

/// WHY: Defends ActiveOrder::Static inbox ordering invariant, making active sessions
/// strictly sort by creation time (newest first) and SessionId tie-breaker, ignoring
/// status urgency and Attention priority so rows remain static during status changes.
#[test]
fn test_arrange_static_order_ignores_urgency_and_attention() {
    let mut s_approval = build_session(1, 1, 1_000);
    s_approval.info.attention.waiting = Some(true);
    s_approval.info.hint = Some(AgentHint {
        state: HintState::Approval,
        label: Some("Approval requested".to_string()),
        received_at_ms: NOW,
    }); // Status urgency = 4 (highest)

    let s_working_newer = build_session(2, 1, 2_000); // Status urgency = 0 (lowest), but created at 2000

    let mut rows = vec![s_approval, s_working_newer];
    arrange(&mut rows, clock(), policy(), ActiveOrder::Static);

    // ActiveOrder::Static ignores status urgency (4 vs 0) and puts s_working_newer (created 2000) first.
    assert_eq!(session_ids(&rows), vec![2, 1]);
}

/// WHY: Defends Snoozed section tie-breaker ordering in arrange(), verifying sessions
/// sort by ascending wake_at_ms timestamp (soonest wake first), falling back to SessionId
/// numerical order on identical wake timestamps.
#[test]
fn test_arrange_snoozed_section_tiebreaker_wake_at_and_id() {
    let mut s1 = build_session(1, 1, 1_000);
    s1.snooze = Some(Snooze {
        snoozed_at_ms: NOW,
        wake_at_ms: NOW + 10 * HOUR,
    });

    let mut s2 = build_session(2, 1, 1_000);
    s2.snooze = Some(Snooze {
        snoozed_at_ms: NOW,
        wake_at_ms: NOW + 2 * HOUR,
    });

    let mut s3 = build_session(3, 1, 1_000);
    s3.snooze = Some(Snooze {
        snoozed_at_ms: NOW,
        wake_at_ms: NOW + 2 * HOUR, // Same wake time as s2
    });

    let mut rows = vec![s1, s3, s2];
    let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Static);

    assert_eq!(split.active_len(), 0);
    assert_eq!(split.snoozed_len(), 3);
    // s2 (wake +2h, id 2) comes before s3 (wake +2h, id 3), followed by s1 (wake +10h, id 1).
    assert_eq!(session_ids(&rows), vec![2, 3, 1]);
}

/// WHY: Defends Settled section tie-breaker ordering in arrange(), verifying finished/settled
/// sessions sort by descending settled_at_ms timestamp (most recently settled first), falling
/// back to SessionId numerical order on ties.
#[test]
fn test_arrange_settled_section_tiebreaker_settled_at_and_id() {
    let mut s1 = build_session(1, 1, 1_000);
    s1.info.status = SessionStatus::Exited { code: Some(0) };
    s1.info.last_activity_ms = NOW - 10 * HOUR;
    s1.last_visited_ms = Some(NOW - 10 * HOUR); // settled_at = NOW - 10h

    let mut s2 = build_session(2, 1, 1_000);
    s2.info.status = SessionStatus::Exited { code: Some(0) };
    s2.info.last_activity_ms = NOW - 1 * HOUR;
    s2.last_visited_ms = Some(NOW - 1 * HOUR); // settled_at = NOW - 1h (more recent)

    let mut s3 = build_session(3, 1, 1_000);
    s3.info.status = SessionStatus::Exited { code: Some(0) };
    s3.info.last_activity_ms = NOW - 1 * HOUR;
    s3.last_visited_ms = Some(NOW - 1 * HOUR); // settled_at = NOW - 1h (same as s2, higher ID)

    let mut rows = vec![s1, s3, s2];
    let split = arrange(&mut rows, clock(), policy(), ActiveOrder::Static);

    assert_eq!(split.settled_len(rows.len()), 3);
    // Settled order: descending settled_at_ms, then ascending ID.
    // s2 (settled -1h, id 2), s3 (settled -1h, id 3), s1 (settled -10h, id 1).
    assert_eq!(session_ids(&rows), vec![2, 3, 1]);
}

/// WHY: Defends single-pass bucket sorting partition boundaries (Active < Snoozed < Settled)
/// and idempotency across repeated arrange() executions on heterogeneous session lists.
#[test]
fn test_arrange_single_pass_partitioning_and_idempotency() {
    let mut active = build_session(1, 1, 1_000);
    
    let mut snoozed = build_session(2, 1, 1_000);
    snoozed.snooze = Some(Snooze {
        snoozed_at_ms: NOW,
        wake_at_ms: NOW + 5 * HOUR,
    });

    let mut settled = build_session(3, 1, 1_000);
    settled.info.status = SessionStatus::Exited { code: Some(0) };
    settled.info.last_activity_ms = NOW - 2 * HOUR;
    settled.last_visited_ms = Some(NOW - 1 * HOUR);
    let mut rows = vec![settled.clone(), active.clone(), snoozed.clone()];
    let split1 = arrange(&mut rows, clock(), policy(), ActiveOrder::Urgency);

    assert_eq!(split1.active_len(), 1);
    assert_eq!(split1.snoozed_len(), 1);
    assert_eq!(split1.settled_len(3), 1);
    assert_eq!(split1.section_at(0), Section::Active);
    assert_eq!(split1.section_at(1), Section::Snoozed);
    assert_eq!(split1.section_at(2), Section::Settled);

    let initial_order = session_ids(&rows);
    assert_eq!(initial_order, vec![1, 2, 3]);

    // Re-running arrange multiple times must produce identical split and row order (idempotent).
    for _ in 0..5 {
        let split_again = arrange(&mut rows, clock(), policy(), ActiveOrder::Urgency);
        assert_eq!(split_again, split1);
        assert_eq!(session_ids(&rows), initial_order);
    }

    // arrange_sections borrows the bands accurately according to the split.
    let arranged = arrange_sections(&mut rows, clock(), policy(), ActiveOrder::Urgency);
    assert_eq!(arranged.active.len(), 1);
    assert_eq!(arranged.snoozed.len(), 1);
    assert_eq!(arranged.settled.len(), 1);
    assert_eq!(arranged.active[0].id(), SessionId(1));
    assert_eq!(arranged.snoozed[0].id(), SessionId(2));
    assert_eq!(arranged.settled[0].id(), SessionId(3));
}

/// WHY: Defends workspace project rollup aggregation via rollup_all(), ensuring project groups
/// preserve input first-appearance order while accumulating per-project session metrics.
#[test]
fn test_fxhash_workspace_project_rollups_first_appearance_order() {
    let s_p30 = build_session(1, 30, 1_000);
    let s_p10 = build_session(2, 10, 1_000);
    let s_p20 = build_session(3, 20, 1_000);
    let s_p10_b = build_session(4, 10, 2_000);

    let rows = vec![s_p30, s_p10, s_p20, s_p10_b];
    let rollups = rollup_all(&rows, clock(), policy());

    assert_eq!(rollups.len(), 3);
    // Project order MUST match first appearance: 30, 10, 20.
    assert_eq!(rollups[0].project_id, ProjectId(30));
    assert_eq!(rollups[1].project_id, ProjectId(10));
    assert_eq!(rollups[2].project_id, ProjectId(20));

    // Project 10 accumulated both session 2 and session 4.
    assert_eq!(rollups[1].total, 2);
    assert_eq!(rollups[0].total, 1);
    assert_eq!(rollups[2].total, 1);
}

/// WHY: Defends the invariant that settled sessions in a project increase total and settled counts
/// but do not vote for the rollup status indicator, returning None indicator when all sessions are settled.
#[test]
fn test_fxhash_workspace_project_rollups_settled_non_voting_indicator() {
    let mut s1 = build_session(1, 100, 1_000);
    s1.info.status = SessionStatus::Exited { code: Some(0) };
    s1.info.last_activity_ms = NOW - 2 * HOUR;
    s1.last_visited_ms = Some(NOW - 1 * HOUR);

    let mut s2 = build_session(2, 100, 2_000);
    s2.info.status = SessionStatus::Exited { code: Some(0) };
    s2.info.last_activity_ms = NOW - 2 * HOUR;
    s2.last_visited_ms = Some(NOW - 1 * HOUR);
    let rows = vec![s1, s2];
    let rollup = rollup_project(ProjectId(100), &rows, clock(), policy());

    assert_eq!(rollup.total, 2);
    assert_eq!(rollup.settled, 2);
    assert_eq!(rollup.active(), 0);
    // Settled sessions do not vote for indicator -> indicator must be None.
    assert_eq!(rollup.indicator, None);
}

/// WHY: Defends rollup status indicator resolution across project active sessions, ensuring
/// the single most urgent active status (Approval > Input > Failed > Ready > Working) becomes the group indicator.
#[test]
fn test_fxhash_workspace_project_rollups_indicator_urgency_hierarchy() {
    let p = ProjectId(42);

    let s_working = build_session(1, 42, 1_000); // Working

    let mut s_ready = build_session(2, 42, 2_000);
    s_ready.info.attention.waiting = Some(true); // Ready

    let mut s_failed = build_session(3, 42, 3_000);
    s_failed.info.status = SessionStatus::Exited { code: Some(1) };
    s_failed.info.attention.failed = true;
    s_failed.info.unread = true; // Failed

    let mut s_approval = build_session(4, 42, 4_000);
    s_approval.info.attention.waiting = Some(true);
    s_approval.info.hint = Some(AgentHint {
        state: HintState::Approval,
        label: Some("Deploy to production?".to_string()),
        received_at_ms: NOW,
    }); // Approval

    let rows = vec![s_working, s_ready, s_failed, s_approval];
    let rollup = rollup_project(p, &rows, clock(), policy());

    assert_eq!(rollup.total, 4);
    assert_eq!(rollup.counts.working, 1);
    assert_eq!(rollup.counts.ready, 1);
    assert_eq!(rollup.counts.failed, 1);
    assert_eq!(rollup.counts.approval, 1);
    // Approval is highest urgency (weight 4) -> indicator MUST be Approval.
    assert_eq!(rollup.indicator, Some(SidebarStatus::Approval));
}

/// WHY: Defends zero-allocation Selection::retain_visible range retention, ensuring off-screen
/// selected IDs are dropped in-place while resetting stale anchor and lead pointers when they leave the visible window.
#[test]
fn test_zero_allocation_selection_retain_visible_bounds() {
    let mut selection = Selection::new();
    let s1 = SessionId(1);
    let s2 = SessionId(2);
    let s3 = SessionId(3);
    let s4 = SessionId(4);

    let visible_initial = vec![s1, s2, s3, s4];
    selection.select_one(s2); // anchor = s2, lead = s2
    selection.extend_to(&visible_initial, s4); // selected = {s2, s3, s4}, anchor = s2, lead = s4

    assert_eq!(selection.len(), 3);
    assert_eq!(selection.anchor(), Some(s2));
    assert_eq!(selection.lead(), Some(s4));

    // Screen filter changes: s2 and s4 disappear; only s1 and s3 remain visible.
    let visible_next = vec![s1, s3];
    selection.retain_visible(&visible_next);

    // s2 and s4 were pruned; s3 remains.
    assert_eq!(selection.len(), 1);
    assert!(selection.contains(s3));
    assert!(!selection.contains(s2));
    assert!(!selection.contains(s4));

    // Stale anchor (s2) and stale lead (s4) must be cleared since they left the visible list.
    assert_eq!(selection.anchor(), None);
    assert_eq!(selection.lead(), None);
}

/// WHY: Defends anchored selection set range expansion via extend_to() and extend_to_additive(),
/// testing shift-click range retention over visible ordering and fallback behaviors when anchor or target is invalid.
#[test]
fn test_zero_allocation_selection_extend_to_range_fallback() {
    let s10 = SessionId(10);
    let s20 = SessionId(20);
    let s30 = SessionId(30);
    let s40 = SessionId(40);
    let visible = vec![s10, s20, s30, s40];

    let mut selection = Selection::single(s20); // anchor = 20
    assert_eq!(selection.anchor(), Some(s20));

    // Shift-click s40: selects inclusive range s20..=s40 (20, 30, 40). Anchor stays 20, lead becomes 40.
    selection.extend_to(&visible, s40);
    assert_eq!(selection.ordered(&visible), vec![s20, s30, s40]);
    assert_eq!(selection.anchor(), Some(s20));
    assert_eq!(selection.lead(), Some(s40));

    // Shift-click an off-screen / missing ID (s99): falls back to single select on s99.
    let s99 = SessionId(99);
    selection.extend_to(&visible, s99);
    assert_eq!(selection.len(), 1);
    assert!(selection.contains(s99));
    assert_eq!(selection.anchor(), Some(s99));

    // Additive extend_to_additive: union anchored range.
    let mut sel_add = Selection::single(s10); // anchor = 10
    sel_add.toggle(s40); // anchor = 40, selected = {10, 40}
    sel_add.extend_to_additive(&visible, s20); // range between anchor 40 and 20 -> {20, 30, 40}, unioned with {10, 40} -> {10, 20, 30, 40}
    assert_eq!(sel_add.len(), 4);
    assert_eq!(sel_add.lead(), Some(s20));
}

/// WHY: Defends SelectionFacts::collect isolation against stale selections, verifying that selected
/// IDs not present in the visible session slice are safely ignored without inflating aggregate count metrics.
#[test]
fn test_zero_allocation_selection_facts_collection_isolation() {
    let s1 = build_session(1, 1, 1_000); // live
    let mut s2 = build_session(2, 1, 1_000);
    s2.snooze = Some(Snooze {
        snoozed_at_ms: NOW,
        wake_at_ms: NOW + 1 * HOUR,
    }); // snoozed

    let rows = vec![s1, s2.clone()];

    let mut selection = Selection::single(SessionId(1));
    selection.toggle(SessionId(2));
    selection.toggle(SessionId(999)); // Stale ID 999 not present in rows!

    let facts = SelectionFacts::collect(&selection, &rows, clock(), policy());

    // Stale ID 999 must be ignored -> count = 2 (not 3).
    assert_eq!(facts.count, 2);
    assert_eq!(facts.snoozed, 1);
    assert_eq!(facts.live, 2);
}
/// WHY: Defends Attention alignment with SidebarStatus and StatusSource, verifying that OS syscall
/// waiting probes produce Ready (Waiting) while agent hints align to Approval/Input (Hint) and process exits override hints.
#[test]
fn test_attention_alignment_syscall_waiting_vs_hint_precedence() {
    // 1. Syscall waiting probe = Ready with StatusSource::Waiting
    let mut s_waiting = build_session(1, 1, 1_000);
    s_waiting.info.attention.waiting = Some(true);
    let res1 = s_waiting.resolve_status();
    assert_eq!(res1.status, SidebarStatus::Ready);
    assert_eq!(res1.source, StatusSource::Waiting);

    // 2. HintState::Approval = Approval with StatusSource::Hint
    let mut s_hint = build_session(2, 1, 1_000);
    s_hint.info.attention.waiting = Some(true);
    s_hint.info.hint = Some(AgentHint {
        state: HintState::Approval,
        label: Some("Permit rm?".to_string()),
        received_at_ms: NOW,
    });
    let res2 = s_hint.resolve_status();
    assert_eq!(res2.status, SidebarStatus::Approval);
    assert_eq!(res2.source, StatusSource::Hint);

    // 3. Process exit overrides stale approval hint -> Failed with StatusSource::Exit
    let mut s_exited = build_session(3, 1, 1_000);
    s_exited.info.status = SessionStatus::Exited { code: Some(1) };
    s_exited.info.attention.failed = true;
    s_exited.info.hint = Some(AgentHint {
        state: HintState::Approval,
        label: Some("Stale hint".to_string()),
        received_at_ms: NOW,
    });
    let res3 = s_exited.resolve_status();
    assert_eq!(res3.status, SidebarStatus::Failed);
    assert_eq!(res3.source, StatusSource::Exit);
}

/// WHY: Defends Attention::priority() transport signal hierarchy (failed > waiting > bell > idle > default)
/// and its alignment with status resolution and sorting weight.
#[test]
fn test_attention_priority_hierarchy_ladder() {
    let att_failed = Attention {
        failed: true,
        waiting: Some(true),
        bell: true,
        idle_ms: 100_000,
    };
    assert_eq!(att_failed.priority(), 4);

    let att_waiting = Attention {
        failed: false,
        waiting: Some(true),
        bell: true,
        idle_ms: 100_000,
    };
    assert_eq!(att_waiting.priority(), 3);

    let att_bell = Attention {
        failed: false,
        waiting: Some(false),
        bell: true,
        idle_ms: 100_000,
    };
    assert_eq!(att_bell.priority(), 2);

    let att_idle = Attention {
        failed: false,
        waiting: Some(false),
        bell: false,
        idle_ms: IDLE_ATTENTION_MS + 10,
    };
    assert_eq!(att_idle.priority(), 1);

    let att_default = Attention {
        failed: false,
        waiting: Some(false),
        bell: false,
        idle_ms: 0,
    };
    assert_eq!(att_default.priority(), 0);

    // Verify priority ranking: 4 > 3 > 2 > 1 > 0
    assert!(att_failed.priority() > att_waiting.priority());
    assert!(att_waiting.priority() > att_bell.priority());
    assert!(att_bell.priority() > att_idle.priority());
    assert!(att_idle.priority() > att_default.priority());
}
