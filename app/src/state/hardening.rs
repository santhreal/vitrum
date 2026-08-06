//! What the model does when the input is nonsense.
//!
//! Every value folded here arrives from one of two places nobody validates:
//! the daemon, over a socket, and `ui.json`, which an operator may have opened
//! in an editor. These are the guards that keep such a value from taking the
//! window down or leaving the state inconsistent, held together rather than
//! scattered because they share one rule: refuse or clamp, never panic.

use super::*;
use crate::testkit::{HOUR, NOW, project, row};

fn clock() -> Clock {
    Clock::utc(NOW)
}

/// The bucket key for the project daemon id `p` is rooted at.
fn pk(p: u64) -> GroupKey {
    GroupKey::Project(ProjectId(inbox::fnv1a(
        inbox::project_key(&format!("/src/p{p}")).as_bytes(),
    )))
}

/// WHY: `ScrollbackChunk`'s end offset was `from_seq + data.len()`, a plain
/// `u64` add on a number the DAEMON chooses. A debug build panics on overflow,
/// so a malformed or hostile chunk header took the whole window down. The
/// offset saturates instead: no real stream reaches 2^64 bytes, so the clamp
/// is unreachable in practice and fatal only if it is missing.
#[test]
fn a_scrollback_chunk_at_the_end_of_the_offset_space_does_not_panic() {
    let mut st = UiState::default();
    st.daemon.sessions = vec![row(10).build()];
    st.open(SessionId(10), NOW);

    let reaction = st.apply(
        ServerMsg::ScrollbackChunk {
            session: SessionId(10),
            from_seq: u64::MAX - 1,
            data: vec![b'a', b'b', b'c'],
            more: false,
        },
        NOW,
    );

    let Reaction::Backfill {
        from_seq,
        resume_seq,
        bytes,
        ..
    } = reaction
    else {
        panic!("a chunk for the focused session must paint: {reaction:?}");
    };
    assert_eq!(from_seq, u64::MAX - 1);
    assert_eq!(
        resume_seq,
        u64::MAX,
        "the resume point saturates at the top of the space"
    );
    assert_eq!(bytes, b"abc");
    assert_eq!(st.window.history.span, 3);
}

/// WHY: the bulk context menu counted the whole SELECTION while acting on the
/// rows the tree is currently showing, and those two sets differ the moment a
/// bucket is collapsed under a live selection. Two consequences, one cosmetic
/// and one fatal: every label promised more rows than the action would touch,
/// and the refusal hint computed `targets - snoozable` on a `usize`, which
/// underflows and panics as soon as a snoozable row is off screen.
///
/// Five selected rows, two of them hidden behind a collapsed bucket and one of
/// the three visible ones blocked on the operator, is the smallest shape that
/// produces it: `snoozable` is four and the target list is three.
#[test]
fn the_bulk_menu_counts_the_rows_it_will_act_on_and_not_the_whole_selection() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "p1"), project(2, "p2")];
    st.daemon.sessions = [(10u64, 1u64), (11, 1), (12, 1), (20, 2), (21, 2)]
        .into_iter()
        .map(|(id, pid)| {
            row(id)
                .project(pid)
                // Working, so nothing is settleable and everything except the
                // blocked row below is snoozable.
                .waiting(Some(false))
                .created_at_ms(NOW - HOUR + id)
                .last_activity_ms(NOW - HOUR + id)
                .build()
        })
        .collect();
    // The one row that refuses a snooze, and it is in the VISIBLE bucket.
    st.daemon.sessions[2].info.attention.waiting = Some(true);
    st.daemon.sessions[2].info.hint = Some(vitrum_proto::AgentHint {
        state: vitrum_proto::HintState::Approval,
        label: None,
        received_at_ms: NOW,
    });

    st.select_all_visible(clock());
    assert_eq!(st.window.selection.len(), 5);

    st.window.collapsed.insert(pk(2));

    let targets = st.menu_targets(SessionId(10), clock());
    assert_eq!(
        targets,
        vec![SessionId(12), SessionId(11), SessionId(10)],
        "the collapsed bucket's rows are not targets"
    );

    let items = st.menu_items(SessionId(10), clock());
    let snooze = items
        .iter()
        .find(|item| item.action == MenuAction::SnoozeHeader)
        .expect("a multi-row menu offers a snooze header");
    assert_eq!(snooze.label, "Snooze (3)");
    assert!(!snooze.enabled, "one target refuses, so the bulk park refuses");
    assert_eq!(
        snooze.hint.as_deref(),
        Some("1 blocked on you"),
        "the refusal names how many of the TARGETS refuse"
    );

    let settle = items
        .iter()
        .find(|item| item.action == MenuAction::Settle)
        .expect("a multi-row menu offers settle");
    assert_eq!(settle.label, "Settle (3)");
    assert_eq!(settle.hint.as_deref(), Some("3 not finished"));
}

/// WHY: `parse_ui_state` reported an unsupported version as `v as u32`, which
/// truncates. A file claiming version 4294967297 therefore came back as
/// `Unsupported { version: 1 }` and told the operator "workspace file is
/// version 1, this build understands 1", which is both wrong and unactionable.
#[test]
fn a_version_past_the_end_of_a_u32_is_reported_as_out_of_range() {
    assert_eq!(
        parse_ui_state(r#"{"version":4294967297}"#),
        UiStateLoad::Unsupported {
            version: u32::MAX
        },
        "a version this build cannot name must not wrap into one it can"
    );
    assert_eq!(
        parse_ui_state(r#"{"version":2}"#),
        UiStateLoad::Unsupported { version: 2 },
        "a version that does fit is still reported exactly"
    );
}
