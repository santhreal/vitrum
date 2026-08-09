//! The terminal pane.
//!
//! The one rule this module exists to enforce: Dioxus must never diff the
//! terminal grid.
//!
//! [`TerminalPane`] renders a single `div#rg-term` with a stable key and no
//! children in the RSX. Because the template has no child nodes, the virtual
//! DOM has nothing to diff beneath it and emits no mutations for anything
//! xterm.js puts there. The node is rendered unconditionally, never inside a
//! conditional, so it is created once at mount and lives for the process.
//!
//! Every state the pane can be in is a *sibling* of that node, never a child.
//! Showing and hiding one is an ordinary diff that cannot reach the grid.

use dioxus::prelude::*;
use vitrum_proto::{SessionId, SessionStatus};

use crate::state::{ConnState, UiState};

/// Stable key for the terminal container.
///
/// The key is what tells Dioxus this is the same node across every render, so
/// the element is created once at mount and never torn down and rebuilt. A
/// rebuild would drop the xterm.js canvas and its WebGL context on the floor.
const TERMINAL_KEY: &str = "rg-terminal-root";

/// What the pane should say over or under the grid.
///
/// Returned as data so the exact wording of every state is pinned by a test.
/// The states are the ones an operator actually hits: nothing open yet, a
/// session picked but nothing to show, a child that exited, and a socket that
/// went away while the child kept running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneState {
    /// A live session is streaming. Nothing is drawn over the grid.
    Live,
    /// No session is focused and none exist.
    Empty,
    /// No session is focused but some exist.
    Unfocused,
    /// The focused child exited. `code` is `None` when it was signalled.
    Exited { code: Option<i32> },
}

/// Resolve what the pane is showing.
pub fn pane_state(st: &UiState) -> PaneState {
    match st.window.focused.and_then(|id| st.session(id)) {
        None if st.daemon.sessions.is_empty() => PaneState::Empty,
        None => PaneState::Unfocused,
        Some(info) => match info.status {
            SessionStatus::Exited { code } => PaneState::Exited { code },
            SessionStatus::Starting | SessionStatus::Running => PaneState::Live,
        },
    }
}

/// One line describing how a child ended.
pub fn exit_line(code: Option<i32>) -> String {
    match code {
        Some(0) => "The agent exited cleanly. Its output is still here.".to_string(),
        Some(c) => format!("The agent exited with code {c}. Its output is still here."),
        None => "The agent was killed by a signal. Its output is still here.".to_string(),
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct TerminalPaneProps {
    pub state: Signal<UiState>,
    pub on_new_session: EventHandler<()>,
    pub on_close_tab: EventHandler<SessionId>,
    pub on_retry: EventHandler<()>,
}

#[component]
pub fn TerminalPane(props: TerminalPaneProps) -> Element {
    let st = props.state.read();
    let pane = pane_state(&st);
    let focused = st.window.focused;
    let offline = st.daemon.conn.is_retryable();
    let connecting = matches!(st.daemon.conn, ConnState::Connecting);
    let ready = st.server_ready();

    rsx! {
        // Owned by JavaScript from here down. Do not add children.
        div {
            key: "{TERMINAL_KEY}",
            id: "rg-term",
            class: "rg-terminal",
        }

        match pane {
            PaneState::Live => rsx! {},
            PaneState::Empty => rsx! {
                div { class: "rg-terminal__empty",
                    // Nothing is said here that the button does not already
                    // say. A heading reading "No sessions yet" above a
                    // sentence reading "Start one and it appears in the
                    // sidebar" above a button reading "New session" is the
                    // same statement three times, and the shortcut line under
                    // it explained a control 24px away. Four stacked bands,
                    // separated by gaps of 23, 15 and 17px, to say one thing.
                    //
                    // Connecting and offline are NOT empty states. A failure
                    // that renders as "nothing here yet" is a lie, so those
                    // two keep their sentence.
                    if connecting {
                        span { class: "rg-terminal__empty-hint",
                            "Connecting to the session daemon."
                        }
                    } else if offline {
                        span { class: "rg-terminal__empty-hint",
                            "The session daemon is not answering."
                        }
                        button {
                            class: "rg-btn rg-btn--primary",
                            r#type: "button",
                            onclick: move |_| props.on_retry.call(()),
                            "Retry"
                        }
                    } else {
                        button {
                            class: "rg-btn rg-btn--primary",
                            r#type: "button",
                            disabled: !ready,
                            onclick: move |_| props.on_new_session.call(()),
                            "New session"
                        }
                        span { class: "rg-terminal__empty-hint",
                            kbd { "Ctrl+Shift+N" }
                        }
                    }
                }
            },
            PaneState::Unfocused => rsx! {
                div { class: "rg-terminal__empty",
                    span { class: "rg-terminal__empty-title", "No session focused" }
                    span { class: "rg-terminal__empty-hint",
                        "Pick a session in the sidebar, or press "
                        kbd { "Alt+1" }
                        " through "
                        kbd { "Alt+9" }
                        "."
                    }
                }
            },
            PaneState::Exited { code } => rsx! {
                div { class: "rg-exitbar",
                    span { class: "rg-exitbar__text", "{exit_line(code)}" }
                    if let Some(id) = focused {
                        button {
                            class: "rg-btn-inline",
                            r#type: "button",
                            onclick: move |_| props.on_close_tab.call(id),
                            "Close tab"
                        }
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::NOW;
    use vitrum_model::SessionView;
    use vitrum_proto::{Attention, ProjectId, SessionInfo};

    fn session(id: u64, status: SessionStatus) -> SessionView {
        SessionView::new(SessionInfo {
            id: SessionId(id),
            project_id: ProjectId(1),
            title: "agent".into(),
            cwd: "/src".into(),
            command: "sh".into(),
            args: Vec::new(),
            status,
            created_at_ms: 0,
            last_activity_ms: 0,
            cols: 80,
            rows: 24,
            git_branch: None,
            unread: false,
            attention: Attention::default(),
            hint: None,
            term_title: None,
        })
    }

    /// A fresh window with no sessions must show the empty state, not the
    /// "pick a session" state. The two say different things and only one of
    /// them is true when there is nothing to pick.
    #[test]
    fn nothing_at_all_is_a_different_state_from_nothing_focused() {
        let st = UiState::default();
        assert_eq!(pane_state(&st), PaneState::Empty);

        let mut st = UiState::default();
        st.daemon.sessions = vec![session(1, SessionStatus::Running)];
        assert_eq!(pane_state(&st), PaneState::Unfocused);
    }

    /// A focused live session draws nothing over the grid. Any overlay here
    /// would sit on top of the terminal the user is typing into.
    #[test]
    fn a_focused_running_session_leaves_the_grid_alone() {
        let mut st = UiState::default();
        st.daemon.sessions = vec![session(1, SessionStatus::Running)];
        st.open(SessionId(1), NOW);
        assert_eq!(pane_state(&st), PaneState::Live);

        st.daemon.sessions[0].info.status = SessionStatus::Starting;
        assert_eq!(pane_state(&st), PaneState::Live);
    }

    /// A dead child gets a bar, not an overlay. The output it left behind is
    /// the most valuable thing on screen at that moment, and covering it to
    /// announce the exit would be exactly the wrong trade.
    #[test]
    fn an_exited_session_reports_its_code() {
        let mut st = UiState::default();
        st.daemon.sessions = vec![session(1, SessionStatus::Exited { code: Some(3) })];
        st.open(SessionId(1), NOW);
        assert_eq!(pane_state(&st), PaneState::Exited { code: Some(3) });
    }

    /// The three exit wordings must be distinct and must all promise the
    /// output is still there. "Exited" and "killed" are different events, and
    /// a user who thinks the scrollback is gone will not go looking for it.
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

    /// Focus pointing at a session that no longer exists must fall back to a
    /// real state rather than panicking or drawing the live grid for a
    /// session that is gone.
    #[test]
    fn focus_on_a_vanished_session_falls_back_to_a_real_state() {
        let mut st = UiState::default();
        st.daemon.sessions = vec![session(1, SessionStatus::Running)];
        st.window.focused = Some(SessionId(99));
        assert_eq!(pane_state(&st), PaneState::Unfocused);
    }
}
