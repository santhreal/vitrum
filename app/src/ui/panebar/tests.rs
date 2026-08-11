//! What the strip under the pane says, at every state.
//!
//! # The rule these defend
//!
//! The bar is the only thing below the pane that occupies space, so a change
//! in what it contains is a change in the pane's rectangle, which is a resize
//! of the PTY, which is every agent on screen redrawing. Every test here is
//! ultimately the same assertion: the strip's element list does not depend on
//! what is happening to a session.
//!
//! Folded rather than rendered, so none of it needs a display.

use super::*;

use crate::testkit::NOW;
use crate::ui::sidebar::tree::Kind;
use vitrum_model::SessionView;
use vitrum_proto::{Attention, ProjectId, SessionId, SessionInfo, SessionStatus};

const HOME: &str = "/home/mk";
const SERVER: &str = "127.0.0.1:7737";

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

/// Every state the bar can be in, named, so a test that walks them cannot
/// quietly stop covering one.
fn states() -> Vec<(&'static str, UiState)> {
    let empty = UiState::default();

    let mut unfocused = UiState::default();
    unfocused.daemon.sessions = vec![session(1, SessionStatus::Running)];

    let mut running = UiState::default();
    running.daemon.sessions = vec![session(1, SessionStatus::Running)];
    running.open(SessionId(1), NOW);

    let mut exited = UiState::default();
    exited.daemon.sessions = vec![session(1, SessionStatus::Exited { code: Some(3) })];
    exited.open(SessionId(1), NOW);

    let mut repo = UiState::default();
    let mut view = session(1, SessionStatus::Running);
    view.info.git_branch = Some("main".into());
    view.info.worktree = Some("review".into());
    repo.daemon.sessions = vec![view];
    repo.open(SessionId(1), NOW);

    let mut down = UiState::default();
    down.daemon.conn = crate::state::ConnState::Failed {
        detail: "connection refused".into(),
    };

    vec![
        ("nothing at all", empty),
        ("a session nobody focused", unfocused),
        ("a running agent", running),
        ("an agent that exited", exited),
        ("an agent in a worktree", repo),
        ("a daemon that is not answering", down),
    ]
}

/// The element list, by class, in draw order, ignoring modifiers.
fn shape(node: &Node) -> Vec<String> {
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

fn walk(node: &Node, out: &mut Vec<String>) {
    let base = node
        .class
        .split_whitespace()
        .find(|c| !c.contains("--"))
        .unwrap_or_default();
    if !base.is_empty() {
        out.push(base.to_string());
    }
    for child in &node.children {
        walk(child, out);
    }
}

/// THE rule. Six states, one element list.
///
/// Not "roughly the same": identical, in order. An element that appears only
/// once a fact resolves is the defect this whole file exists for, and it is
/// invisible in a screenshot of any single state.
#[test]
fn the_bar_has_the_same_elements_at_every_state() {
    let mut seen: Option<(&str, Vec<String>)> = None;
    for (name, st) in states() {
        let shape = shape(&strip(&st, HOME, SERVER));
        match &seen {
            None => seen = Some((name, shape)),
            Some((first, expected)) => assert_eq!(
                &shape, expected,
                "the bar's elements differ between {first} and {name}"
            ),
        }
    }
}

/// The exit is a word in the bar and never an element beside it.
#[test]
fn an_exit_adds_a_word_and_not_a_row() {
    let states = states();
    let running = &states[2].1;
    let exited = &states[3].1;

    let before = strip(running, HOME, SERVER);
    let after = strip(exited, HOME, SERVER);
    assert_eq!(
        shape(&before),
        shape(&after),
        "an exit changed the bar's element list"
    );
    assert!(
        text_of(&after, "rg-panebar__exit-line").contains("code 3"),
        "the exit code is not in the bar: {:?}",
        text_of(&after, "rg-panebar__exit-line")
    );
    assert!(
        text_of(&before, "rg-panebar__exit-line").is_empty(),
        "a running agent's bar quotes an exit"
    );
}

/// The one control is offered only for a session that has actually ended.
#[test]
fn stop_viewing_is_refused_until_the_agent_exits() {
    let states = states();
    let running = strip(&states[2].1, HOME, SERVER);
    let exited = strip(&states[3].1, HOME, SERVER);

    assert!(
        !enabled(&running, Act::StopViewing),
        "a running agent offers a control that would abandon its transcript"
    );
    assert!(
        enabled(&exited, Act::StopViewing),
        "an exited agent's tab cannot be dropped from the bar"
    );
}

/// A window with no session still says whether the daemon answered.
#[test]
fn an_idle_bar_reports_the_connection() {
    let idle = strip(&UiState::default(), HOME, SERVER);
    let place = text_of(&idle, "rg-panebar__place");
    assert!(
        place.contains(SERVER),
        "an idle bar does not name the daemon: {place:?}"
    );

    let states = states();
    let down = strip(&states[5].1, HOME, SERVER);
    let place = text_of(&down, "rg-panebar__place");
    assert!(
        place.contains("not answering"),
        "a dead socket reads as normal: {place:?}"
    );
}

/// The worktree is named by git's name for it, with its caption, or by nothing
/// at all. Never by a path.
#[test]
fn a_worktree_is_a_named_pair_or_an_empty_one() {
    let states = states();
    let plain = strip(&states[2].1, HOME, SERVER);
    let linked = strip(&states[4].1, HOME, SERVER);

    assert_eq!(text_of(&plain, "rg-panebar__key"), "");
    assert_eq!(text_of(&plain, "rg-panebar__value"), "");
    assert_eq!(text_of(&linked, "rg-panebar__key"), "worktree");
    assert_eq!(text_of(&linked, "rg-panebar__value"), "review");
    assert!(
        !text_of(&linked, "rg-panebar__value").contains('/'),
        "the worktree is quoted as a path"
    );
}

/// A directory under the operator's home is drawn with a tilde.
///
/// The bar is on screen in every screenshot this project publishes, and a home
/// directory in it names the person who took it.
#[test]
fn the_place_never_spells_out_a_home_directory() {
    let mut st = UiState::default();
    let mut view = session(1, SessionStatus::Running);
    view.info.cwd = format!("{HOME}/src/vitrum");
    st.daemon.sessions = vec![view];
    st.open(SessionId(1), NOW);

    let place = text_of(&strip(&st, HOME, SERVER), "rg-panebar__place");
    assert!(
        !place.contains(HOME),
        "the bar spells out the home directory: {place:?}"
    );
    assert!(place.starts_with('~'), "the bar does not abbreviate: {place:?}");
}

/// Exactly one element grows, so everything after it is pinned to the right
/// edge whatever the ones before it say.
#[test]
fn one_element_takes_the_slack() {
    for (name, st) in states() {
        let node = strip(&st, HOME, SERVER);
        let growers: Vec<&str> = node
            .children
            .iter()
            .filter(|c| c.grow)
            .map(|c| c.class.as_str())
            .collect();
        assert_eq!(
            growers,
            vec!["rg-panebar__gap"],
            "the bar's slack is not in one place at {name}"
        );
    }
}

/// The mark is the focused agent's own, and there is exactly one place for it.
#[test]
fn the_agent_mark_is_reserved_before_a_session_exists() {
    let idle = strip(&UiState::default(), HOME, SERVER);
    let live = strip(&states()[2].1, HOME, SERVER);

    let idle_mark = find(&idle, "rg-panebar__agent").expect("no place for the mark");
    assert!(
        !matches!(idle_mark.kind, Kind::Mark(_)),
        "an empty window draws an agent mark"
    );
    let live_mark = find(&live, "rg-panebar__agent").expect("no mark for a focused agent");
    assert!(
        matches!(live_mark.kind, Kind::Mark(_)),
        "a focused agent has no mark"
    );
}

// ───────────────────────────────────────────────────────────────────────────

/// The first node wearing `class`, by token.
fn find<'a>(node: &'a Node, class: &str) -> Option<&'a Node> {
    if node.class.split_whitespace().any(|c| c == class) {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, class))
}

/// What the element wearing `class` says, or `""`.
fn text_of(node: &Node, class: &str) -> String {
    find(node, class).map(|n| n.text.clone()).unwrap_or_default()
}

/// Whether the control raising `act` accepts a press.
fn enabled(node: &Node, act: Act) -> bool {
    fn walk(node: &Node, act: Act) -> Option<bool> {
        if node.kind == Kind::Press(act) {
            return Some(node.enabled);
        }
        node.children.iter().find_map(|child| walk(child, act))
    }
    walk(node, act).unwrap_or(false)
}
