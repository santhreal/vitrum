//! What the frame promises.
//!
//! Two groups, and they close two different classes.
//!
//! - **What the pane is showing.** Nothing focused, nothing at all, a child
//!   that exited, a socket that went away. Which state a window is in is
//!   decided here, and every one of the four is reachable.
//! - **The bar.** Every string on the one permanent surface in the window,
//!   asserted without a display.

use super::*;

use crate::testkit::NOW;
use vitrum_model::SessionView;
use vitrum_proto::{Attention, ProjectId, SessionId, SessionInfo};


// ───────────────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────────────

fn session(id: u64, status: SessionStatus) -> SessionView {
    SessionView::new(SessionInfo {
        id: SessionId(id),
        project_id: ProjectId(1),
        title: "agent".into(),
        cwd: "/src/vitrum".into(),
        command: "codex".into(),
        args: Vec::new(),
        status,
        created_at_ms: 0,
        last_activity_ms: 0,
        cols: 120,
        rows: 40,
        git_branch: None,
        worktree: None,
        unread: false,
        attention: Attention::default(),
        hint: None,
        term_title: None,
    })
}

// ───────────────────────────────────────────────────────────────────────────
// What the pane is showing
// ───────────────────────────────────────────────────────────────────────────

/// A fresh window with no sessions must show the empty state, not the "pick a
/// session" state. The two say different things and only one of them is true
/// when there is nothing to pick.
#[test]
fn nothing_at_all_is_a_different_state_from_nothing_focused() {
    let st = UiState::default();
    assert_eq!(pane_state(&st), PaneState::Empty);

    let mut st = UiState::default();
    st.daemon.sessions = vec![session(1, SessionStatus::Running)];
    assert_eq!(pane_state(&st), PaneState::Unfocused);
}

/// A focused live session draws nothing over the grid. Any overlay here would
/// sit on top of the terminal the operator is typing into.
#[test]
fn a_focused_running_session_leaves_the_grid_alone() {
    let mut st = UiState::default();
    st.daemon.sessions = vec![session(1, SessionStatus::Running)];
    st.open(SessionId(1), NOW);
    assert_eq!(pane_state(&st), PaneState::Live);

    st.daemon.sessions[0].info.status = SessionStatus::Starting;
    assert_eq!(pane_state(&st), PaneState::Live);
}

/// A dead child is reported, and its output is not covered.
#[test]
fn an_exited_session_reports_its_code() {
    let mut st = UiState::default();
    st.daemon.sessions = vec![session(1, SessionStatus::Exited { code: Some(3) })];
    st.open(SessionId(1), NOW);
    assert_eq!(pane_state(&st), PaneState::Exited { code: Some(3) });
}

/// The three exit wordings must be distinct and must all promise the output is
/// still there. "Exited" and "killed" are different events, and an operator
/// who thinks the scrollback is gone will not go looking for it.
#[test]
fn exit_lines_distinguish_clean_failed_and_signalled() {
    assert_eq!(
        exit_line(Some(0)),
        "The agent exited cleanly. Its output is still here."
    );
    assert_eq!(
        exit_line(Some(137)),
        "The agent exited with code 137. Its output is still here."
    );
    assert_eq!(
        exit_line(None),
        "The agent was killed by a signal. Its output is still here."
    );
    for line in [exit_line(Some(0)), exit_line(Some(1)), exit_line(None)] {
        assert!(line.contains("still here"), "{line}");
    }
}

/// Focus pointing at a session that no longer exists must fall back to a real
/// state rather than panicking or drawing the live grid for a session that is
/// gone.
#[test]
fn focus_on_a_vanished_session_falls_back_to_a_real_state() {
    let mut st = UiState::default();
    st.daemon.sessions = vec![session(1, SessionStatus::Running)];
    st.window.focused = Some(SessionId(99));
    assert_eq!(pane_state(&st), PaneState::Unfocused);
}

// ───────────────────────────────────────────────────────────────────────────
// The bar
// ───────────────────────────────────────────────────────────────────────────

/// WHY: the window said nowhere where an agent was working.
///
/// The class, and it is three surfaces deep. The titlebar carries a session
/// title, which is renameable to anything. The sidebar row draws its
/// directory only when that directory says something the group header does
/// not, and the header carries a project NAME rather than a path, so a session
/// at its project root with no branch drew an empty line. And a session that
/// followed OSC 7 into a different directory changed nothing anybody could
/// see. Three places that could have said it, none that did.
///
/// The bar says it unconditionally, at the project root, at the home
/// directory, and after a move. There is no arm in which it is empty.
#[test]
fn the_bar_always_says_where_the_agent_is_working() {
    for cwd in [
        "/src/vitrum",
        "/src/vitrum/crates/vitrum-core",
        "/home/mk",
        "/",
    ] {
        let mut row = session(1, SessionStatus::Running);
        row.info.cwd = cwd.to_string();
        let bar = bar_of(&row, "/home/mk");
        assert!(
            !bar.place.trim().is_empty(),
            "the bar drew nothing for a session working in {cwd}"
        );
    }
}

/// The operator's login name is not the product's to publish. A directory
/// under home is drawn home-relative, which is shorter and says the same
/// thing.
#[test]
fn a_directory_under_home_is_drawn_home_relative() {
    let mut row = session(1, SessionStatus::Running);
    row.info.cwd = "/home/mk/src/vitrum".to_string();
    let bar = bar_of(&row, "/home/mk");
    assert_eq!(bar.place, "~/src/vitrum");
    assert!(!bar.place.contains("/home/"), "{}", bar.place);
}

/// WHY: a git worktree was invisible.
///
/// The class: a linked worktree lives beside its project rather than inside
/// it, on another branch, and the window drew a branch name with no hint that
/// the files were somewhere else at all. Two sessions on two worktrees of one
/// project were told apart by a branch name, which is precisely the case where
/// the branch is not the interesting difference.
///
/// A main working tree reports nothing, which is the other half: an element
/// that appeared on every row would say nothing on almost all of them.
#[test]
fn a_session_in_a_linked_worktree_says_which_worktree() {
    let mut row = session(1, SessionStatus::Running);
    row.info.git_branch = Some("review".into());
    row.info.worktree = Some("wt-review".into());
    let bar = bar_of(&row, "/home/mk");
    assert_eq!(bar.worktree.as_deref(), Some("wt-review"));
    assert!(
        bar_title(&bar).contains("worktree wt-review"),
        "{}",
        bar_title(&bar)
    );

    let main = session(1, SessionStatus::Running);
    assert_eq!(
        bar_of(&main, "/home/mk").worktree,
        None,
        "a main working tree is not a worktree, and drawing one on every \
         session would make the element say nothing"
    );

    // An empty string is what a daemon sends when it resolved nothing, and it
    // must not become an element with no text in it.
    let mut blank = session(1, SessionStatus::Running);
    blank.info.worktree = Some(String::new());
    assert_eq!(bar_of(&blank, "/home/mk").worktree, None);
}

/// WHY: two surfaces resolving the same state independently name it two ways.
///
/// The class: the status read Approval in one place and Ready in another while
/// the gate was up, and an operator watching them disagree cannot tell which
/// is lying. The bar takes its word from [`Pill::of`], which is the function
/// the sidebar row calls, so there is one resolution and one word.
#[test]
fn the_bar_and_the_row_name_one_state_once() {
    for status in [
        SessionStatus::Running,
        SessionStatus::Starting,
        SessionStatus::Exited { code: Some(0) },
        SessionStatus::Exited { code: Some(1) },
    ] {
        let row = session(1, status.clone());
        let bar = bar_of(&row, "/home/mk");
        assert_eq!(
            bar.state.as_ref().map(|p| p.word),
            Some(Pill::of(&row).word),
            "the bar resolved {status:?} to a different word than the row"
        );
    }
}

/// A dead child's report is a sentence in the bar, not a box of its own.
#[test]
fn an_exit_is_a_word_in_the_bar() {
    let live = bar_of(&session(1, SessionStatus::Running), "/home/mk");
    assert_eq!(live.exit, None);

    let dead = bar_of(&session(1, SessionStatus::Exited { code: Some(2) }), "/h");
    assert_eq!(dead.exit.as_deref(), Some(exit_line(Some(2)).as_str()));
}

/// A window with nothing focused still has one fact worth stating, and the bar
/// is one line tall either way, so it costs nothing to state it.
#[test]
fn an_idle_bar_says_whether_the_daemon_answered() {
    let st = UiState::default();
    let bar = pane_bar(&st, "/home/mk", "127.0.0.1:7737");
    assert!(!bar.place.is_empty());
    assert_eq!(bar.state, None, "there is no session to have a state");

    assert!(idle_place(&ConnState::Connecting, "127.0.0.1:7737").contains("Connecting"));
    assert!(
        idle_place(
            &ConnState::Live {
                server_version: "0.4.0".into()
            },
            "127.0.0.1:7737"
        )
        .contains("0.4.0")
    );
}

/// A path long enough to overflow the bar keeps its ends. The leaf says which
/// crate the agent is in and the root says which project; the middle is the
/// part nobody reads.
#[test]
fn a_long_path_keeps_both_ends() {
    let mut row = session(1, SessionStatus::Running);
    row.info.cwd = "/home/mk/src/vitrum/crates/vitrum-core/src/session/handlers/inner".to_string();
    let bar = bar_of(&row, "/home/mk");
    assert!(bar.place.starts_with('~'), "{}", bar.place);
    assert!(bar.place.ends_with("inner"), "{}", bar.place);
    assert!(
        bar.place_full.ends_with("inner"),
        "the hover detail must carry the whole path: {}",
        bar.place_full
    );
}
