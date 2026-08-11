//! The frame around the pane.
//!
//! The pane itself is not here and is not markup. It is a GTK drawing area
//! with a wgpu swapchain on it, packed above the webview by [`crate::pane`],
//! positioned by a rectangle this module computes. Nothing in this file parses
//! an escape sequence, holds a cell, or draws a glyph of a session's output.
//!
//! # What the frame owns
//!
//! - **The rectangle.** [`pane_frame`] turns the window's client size, the
//!   display scale, the text scale and the sidebar's width into the pane's
//!   padding box in device pixels. It is arithmetic over layout tokens, so it
//!   is exact before anything is painted and testable without a display. No
//!   element is ever measured to obtain it.
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

#[cfg(test)]
use crate::pane::geometry;

use vitrum_fmt::path;
use vitrum_model::{AgentKind, SessionView};
use vitrum_proto::SessionStatus;

use crate::agent::{AgentMark, AgentMarks};
use crate::inbox::Pill;
use crate::state::{ConnState, UiState};

// ───────────────────────────────────────────────────────────────────────────
// Geometry
//
// Every number below has a twin in the stylesheet, and
// `tests::the_frame_reads_the_same_tokens_the_stylesheet_does` holds the pairs
// together. They are duplicated because the rectangle has to exist BEFORE
// layout: the widget is packed and placed by GTK, which has never seen the
// document, and asking the document would mean measuring it, which is the
// thing this product no longer does.
// ───────────────────────────────────────────────────────────────────────────

/// Titlebar height, in rem. `--rg-titlebar-h` in `app.css`.
#[cfg(test)]
pub const TITLEBAR_REM: f64 = 2.25;

/// Height of the bar under the pane, in rem. `--rg-panebar-h` in `app.css`.
///
/// One line of `--rg-text-xs` on a 28px band. It is a constant and not a
/// measurement precisely so that it cannot change: see the module note on why
/// a bar whose height follows its content resizes every agent on screen.
#[cfg(test)]
pub const PANEBAR_REM: f64 = 1.75;

/// Space between the pane's grid and the chrome around it, in rem, on all
/// four sides. `--rg-pane-pad` in `app.css`.
///
/// Four equal sides, which the pane did not have. It carried 4px above, 8px
/// left and nothing right or below, so a full-screen TUI's last row sat
/// against the window edge while its first sat 4px in, and the grid's optical
/// centre was 4px up and 4px left of the box's. That asymmetry is the
/// "nothing is centered" report at its largest surface.
#[cfg(test)]
pub const PANE_PAD_REM: f64 = 0.5;

/// Everything outside the pane that decides where the pane is.
///
/// Device pixels for the window, CSS pixels for everything the stylesheet
/// also knows about, and one scale to convert between them. Taken as a value
/// rather than read out of [`UiState`] because the window's client size and
/// the display scale are the platform's and are not in the model.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg(test)]
pub struct PaneLayout {
    /// Window client area width, in device pixels.
    pub window_w: u32,
    /// Window client area height, in device pixels.
    pub window_h: u32,
    /// Device pixels per CSS pixel.
    pub scale: f64,
    /// Root font size in CSS pixels, after the operator's text scale.
    pub rem_px: f64,
    /// Sidebar width in CSS pixels, as the panel is actually drawn.
    pub sidebar_css: f64,
}

/// The pane's padding box, in device pixels, relative to the client area.
///
/// The PADDING box and not the border box. Handing the pane its border box is
/// how an approval prompt's last option ends up behind the window edge: the
/// pane divides the box it is given by the cell height, so every pixel of
/// chrome left in that box buys a row the operator cannot see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub struct PaneFrame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
impl PaneFrame {
    /// Bottom edge, so a caller can assert the frame ends inside the window.
    #[must_use]
    #[cfg(test)]
    pub fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// Right edge.
    #[must_use]
    #[cfg(test)]
    pub fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }
}

// Why the pane's own rectangle type is not this one.
//
// `PaneFrame` is a layout RESULT, computed from stylesheet tokens this module
// owns and nothing else. `crate::pane::PaneRect` is the platform's argument.
// Keeping them apart is what lets this module compile, and be tested, with no
// knowledge of GTK, wgpu, or which windowing system the build targets, and it
// is why every test below runs on a machine with no display.
//
// The shell converts, because the shell is the only place that holds both a
// window handle and a pane. This module hands the rectangle out through
// `on_frame` and never places anything itself.

/// One CSS length in device pixels, rounded to a whole pixel.
///
/// Rounded once, here, rather than accumulated as a float and rounded at the
/// end. A pane whose left edge is 8.4 and whose width is 1911.6 is placed at 8
/// with width 1912 and hangs one pixel over the right edge; rounding each
/// EDGE and subtracting keeps the box inside the window at every scale.
#[cfg(test)]
fn dev(css: f64, scale: f64) -> f64 {
    (css * scale).round()
}

/// One device pixel, expressed as a cell.
///
/// The frame is measured in device pixels, so dividing a box by a one-pixel
/// cell yields the pixels of that box. It exists so the frame can use the
/// grid arithmetic in [`crate::pane::geometry`] without pretending to know a
/// font's cell size, which this module has never been told and must not be.
#[cfg(test)]
const ONE_DEVICE_PIXEL: f64 = 1.0;

/// Where the pane goes.
///
/// Pure arithmetic over [`PaneLayout`]. The result is clamped into the window
/// on both axes and can be zero-sized, which is the honest answer for a window
/// dragged smaller than its own chrome; a zero-sized pane is placed and draws
/// nothing, and the caller does not have to special-case it.
#[must_use]
#[cfg(test)]
pub fn pane_frame(l: &PaneLayout) -> PaneFrame {
    let scale = if l.scale.is_finite() && l.scale > 0.0 {
        l.scale
    } else {
        1.0
    };
    let rem = if l.rem_px.is_finite() && l.rem_px > 0.0 {
        l.rem_px
    } else {
        16.0
    };
    let sidebar = if l.sidebar_css.is_finite() && l.sidebar_css > 0.0 {
        l.sidebar_css
    } else {
        0.0
    };

    let pad = dev(PANE_PAD_REM * rem, scale);
    let top = dev(TITLEBAR_REM * rem, scale) + pad;
    let bottom = dev(PANEBAR_REM * rem, scale) + pad;

    // The ORIGIN is clamped as well as the size. A window narrower than the
    // sidebar is not hypothetical: a window manager hands a client whatever
    // size it likes during a workspace switch, and this product's own
    // minimum does not bind what it is given. Clamping only the size leaves
    // a zero-width pane placed past the right edge, which is a surface the
    // compositor has to composite and the operator cannot see.
    let window_w = f64::from(l.window_w);
    let window_h = f64::from(l.window_h);
    // Clamped to where the trailing chrome starts, not to the window edge:
    // the pane's own right-hand padding and the bar under it are part of the
    // frame's contract, and an origin past them puts a zero-sized pane where
    // the bar is drawn.
    let left = (dev(sidebar, scale) + pad).min((window_w - pad).max(0.0));
    let top = top.min((window_h - bottom).max(0.0));

    // The chrome as an axis SUM, and the subtraction taken through the pane's
    // own door rather than written a second time here.
    //
    // `cells_across` takes a box, takes the chrome out of it, floors, and
    // answers zero for a box smaller than its own chrome. A device pixel as
    // the cell makes that exactly the pixel arithmetic this frame needs, and
    // it makes the shell's answer and the pane's answer the same expression.
    // Two copies of one subtraction disagree at a rounding boundary and
    // nowhere else, which reaches the operator as an approval prompt whose
    // last option is behind the window edge on one window size in a hundred.
    let w = geometry::cells_across(window_w, left + pad, ONE_DEVICE_PIXEL);
    let h = geometry::cells_across(window_h, top + bottom, ONE_DEVICE_PIXEL);

    PaneFrame {
        x: left as i32,
        y: top as i32,
        width: w,
        height: h,
    }
}

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
