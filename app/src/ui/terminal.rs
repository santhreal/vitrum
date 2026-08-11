//! The frame around the pane.
//!
//! The pane itself is not here. It is a GTK drawing area with a wgpu
//! swapchain on it, packed into the frame's content box by [`crate::pane`].
//! Nothing in this file parses an escape sequence, holds a cell, or draws a
//! glyph of a session's output.
//!
//! # What the frame owns
//!
//! - **Not the rectangle.** The pane's box is GTK's, allocated by the content
//!   box it is packed into. This module once computed it from layout tokens,
//!   because the pane was a separate surface positioned over a document that
//!   could not be asked where anything was. Nothing computes it now, which is
//!   why a dialog opening no longer resizes a pty.
//! - **The bar under it.** [`pane_bar`] resolves what the strip below the pane
//!   says: where the agent is working, on which branch, in which worktree, how
//!   large its grid is, and what state it is in.
//! - **The states over it.** Nothing focused, nothing at all, a child that
//!   exited, a socket that went away. Each is a sibling of the reserved box,
//!   never a child of it.
//!
//! # Two rules that keep the pane still
//!
//! **The frame's height never depends on content.** The bar is one line tall
//! at every state including "no session", so a string arriving does not resize
//! the pane, and a pane that does not resize does not resize the PTY, and a
//! PTY that does not resize does not make every agent on screen redraw. A
//! strip that appears and pushes the pane is the single loudest source of the
//! flashing this product was reported for.
//!
//! **Nothing above the pane takes layout space.** Flashes and notices float
//! over the top edge of the pane box and are removed from flow, so a transient
//! sentence cannot move a terminal grid.

use vitrum_fmt::path;
use vitrum_model::{AgentKind, SessionView};
use vitrum_proto::SessionStatus;

use crate::agent::{AgentMark, AgentMarks};
use crate::inbox::Pill;
use crate::state::{ConnState, UiState};

// ───────────────────────────────────────────────────────────────────────────
// What the pane is showing
// ───────────────────────────────────────────────────────────────────────────

/// What the pane should say over or under the grid.
///
/// Returned as data so the exact wording of every state is pinned by a test.
/// The states are the ones an operator actually hits: nothing open yet, a
/// session picked but nothing to show, a child that exited, and a socket that
/// went away while the child kept running.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
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
#[must_use]
#[cfg(test)]
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
#[must_use]
pub fn exit_line(code: Option<i32>) -> String {
    match code {
        Some(0) => "The agent exited cleanly. Its output is still here.".to_string(),
        Some(c) => format!("The agent exited with code {c}. Its output is still here."),
        None => "The agent was killed by a signal. Its output is still here.".to_string(),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The bar
// ───────────────────────────────────────────────────────────────────────────

/// Columns a working directory gets in the bar.
///
/// Wider than the sidebar row's eighteen, because the bar spans the pane
/// rather than a 224px column and the directory is the fact it exists to
/// carry. Past this the MIDDLE is elided: a path's leaf says which crate the
/// agent is in and its root says which project, and both survive.
const BAR_PATH_COLUMNS: usize = 56;

/// Columns a branch name gets in the bar.
const BAR_BRANCH_COLUMNS: usize = 28;

/// What the strip under the pane says.
///
/// Data, not markup, so every string on the busiest permanent surface in the
/// window is asserted without a DOM.
///
/// Every field is `Option` except `place` and `state`, and those two are the
/// contract: the bar always says where the focused agent is working and what
/// it is doing. "Where" was previously said nowhere in the window at all —
/// not in the titlebar, which carries a session title, and not reliably on the
/// row, which yields its directory to the branch when it sits at the project
/// root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneBar {
    /// The agent's mark, or `None` when nothing is focused.
    pub mark: Option<AgentMark>,
    /// The agent's name, for the accessible label on the mark.
    pub agent: Option<&'static str>,
    /// Working directory, home-relative and middle-elided. Never empty.
    pub place: String,
    /// The full directory, for the bar's title.
    pub place_full: String,
    /// Branch, when the directory is in a repository.
    pub branch: Option<String>,
    /// Linked worktree name, when the directory is one.
    pub worktree: Option<String>,
    /// Grid size as the daemon has it, `columns x rows`.
    pub grid: Option<String>,
    /// The state word, from the same resolution the sidebar row uses.
    pub state: Option<Pill>,
    /// How the child ended, when it has.
    pub exit: Option<String>,
}

/// What a bar with nothing focused says where the directory goes.
///
/// The connection is the only fact a window with no session has, and it is a
/// fact worth a line: an operator staring at an empty window wants to know
/// whether the daemon answered. The bar is one line tall whatever it says, so
/// this costs no space and moves nothing when a session arrives.
#[must_use]
pub fn idle_place(conn: &ConnState, server: &str) -> String {
    match conn {
        ConnState::Connecting => format!("Connecting to {server}"),
        ConnState::Live { server_version } => {
            format!("{server} \u{00b7} server {server_version}")
        }
        _ => format!("{server} is not answering"),
    }
}

/// Resolve the bar.
///
/// `home` is the operator's home directory, used to draw `~` rather than a
/// name that is theirs and nobody else's business.
#[must_use]
pub fn pane_bar(st: &UiState, home: &str, server: &str) -> PaneBar {
    let Some(row) = st.window.focused.and_then(|id| st.row(id)) else {
        return PaneBar {
            mark: None,
            agent: None,
            place: idle_place(&st.daemon.conn, server),
            place_full: String::new(),
            branch: None,
            worktree: None,
            grid: None,
            state: None,
            exit: None,
        };
    };
    bar_of(row, home)
}

/// The bar for one session.
///
/// Split out of [`pane_bar`] so a row can be handed to it directly, which is
/// what every test of the wording does.
#[must_use]
pub fn bar_of(row: &SessionView, home: &str) -> PaneBar {
    let info = &row.info;
    let kind = AgentKind::of(&info.command);
    let full = path::shorten_home_relative(&info.cwd, home, usize::MAX);
    PaneBar {
        mark: Some(kind.mark()),
        agent: Some(kind.label()),
        place: path::shorten_home_relative(&info.cwd, home, BAR_PATH_COLUMNS),
        place_full: full,
        branch: info
            .git_branch
            .as_deref()
            .filter(|b| !b.is_empty())
            .map(|b| path::shorten(b, BAR_BRANCH_COLUMNS)),
        worktree: worktree_of(row),
        grid: Some(format!("{}\u{00d7}{}", info.cols, info.rows)),
        state: Some(Pill::of(row)),
        exit: match info.status {
            SessionStatus::Exited { code } => Some(exit_line(code)),
            _ => None,
        },
    }
}

/// The linked worktree a session is in, if it is in one.
///
/// The daemon resolves it, because only the daemon can: a linked worktree's
/// `.git` is a FILE holding `gitdir: <path>/.git/worktrees/<name>`, and the
/// client has never had the session's filesystem. It arrives on
/// [`vitrum_proto::SessionInfo::worktree`] and is recomputed on the same OSC 7
/// path as the directory and the branch, so an agent that moves itself between
/// worktrees updates here.
///
/// A main working tree reports `None`, and that is the whole distinction the
/// element exists to draw. Before it, two sessions on two branches of one
/// project were told apart by their branch alone, which is exactly the case
/// where the files are somewhere else and the branch does not say so.
#[must_use]
pub fn worktree_of(row: &SessionView) -> Option<String> {
    row.info
        .worktree
        .as_deref()
        .filter(|w| !w.is_empty())
        .map(str::to_string)
}

/// The bar's hover detail, in one string.
///
/// One panel rather than five: the elements in the bar are 12px apart and a
/// platform tooltip is a window, so five of them overlap each other and the
/// pane below.
#[must_use]
#[cfg(test)]
pub fn bar_title(bar: &PaneBar) -> String {
    let mut text = String::new();
    if let Some(agent) = bar.agent {
        text.push_str(agent);
        text.push('\n');
    }
    if !bar.place_full.is_empty() {
        text.push_str(&bar.place_full);
    } else {
        text.push_str(&bar.place);
    }
    if let Some(w) = &bar.worktree {
        text.push_str("\nworktree ");
        text.push_str(w);
    }
    if let Some(b) = &bar.branch {
        text.push_str("\nbranch ");
        text.push_str(b);
    }
    if let Some(g) = &bar.grid {
        text.push('\n');
        text.push_str(g);
        text.push_str(" cells");
    }
    if let Some(e) = &bar.exit {
        text.push('\n');
        text.push_str(e);
    }
    text
}

// ───────────────────────────────────────────────────────────────────────────
// Markup
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests;
