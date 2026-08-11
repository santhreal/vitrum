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

use crate::pane::geometry;
use dioxus::prelude::*;
use vitrum_fmt::path;
use vitrum_model::{AgentKind, SessionView};
use vitrum_proto::{SessionId, SessionStatus};

use crate::agent::{AgentMark, AgentMarks};
use crate::inbox::Pill;
use crate::state::{ConnState, UiState};
use crate::ui::{dialog, firstrun};

/// Stable key for the reserved box.
///
/// The key is what tells Dioxus this is the same node across every render, so
/// the element is created once at mount and never torn down and rebuilt. The
/// box reserves the region the native pane is placed over; a rebuild would
/// leave the widget floating over a region the document no longer describes.
const TERMINAL_KEY: &str = "rg-terminal-root";

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
pub const TITLEBAR_REM: f64 = 2.25;

/// Height of the bar under the pane, in rem. `--rg-panebar-h` in `app.css`.
///
/// One line of `--rg-text-xs` on a 28px band. It is a constant and not a
/// measurement precisely so that it cannot change: see the module note on why
/// a bar whose height follows its content resizes every agent on screen.
pub const PANEBAR_REM: f64 = 1.75;

/// Space between the pane's grid and the chrome around it, in rem, on all
/// four sides. `--rg-pane-pad` in `app.css`.
///
/// Four equal sides, which the pane did not have. It carried 4px above, 8px
/// left and nothing right or below, so a full-screen TUI's last row sat
/// against the window edge while its first sat 4px in, and the grid's optical
/// centre was 4px up and 4px left of the box's. That asymmetry is the
/// "nothing is centered" report at its largest surface.
pub const PANE_PAD_REM: f64 = 0.5;

/// Collapsed sidebar rail width, in rem. `--rg-sidebar-width-collapsed`.
pub const SIDEBAR_RAIL_REM: f64 = 3.0;

/// Everything outside the pane that decides where the pane is.
///
/// Device pixels for the window, CSS pixels for everything the stylesheet
/// also knows about, and one scale to convert between them. Taken as a value
/// rather than read out of [`UiState`] because the window's client size and
/// the display scale are the platform's and are not in the model.
#[derive(Debug, Clone, Copy, PartialEq)]
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
pub struct PaneFrame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl PaneFrame {
    /// Bottom edge, so a caller can assert the frame ends inside the window.
    #[must_use]
    pub fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    /// Right edge.
    #[must_use]
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
fn dev(css: f64, scale: f64) -> f64 {
    (css * scale).round()
}

/// One device pixel, expressed as a cell.
///
/// The frame is measured in device pixels, so dividing a box by a one-pixel
/// cell yields the pixels of that box. It exists so the frame can use the
/// grid arithmetic in [`crate::pane::geometry`] without pretending to know a
/// font's cell size, which this module has never been told and must not be.
const ONE_DEVICE_PIXEL: f64 = 1.0;

/// Where the pane goes.
///
/// Pure arithmetic over [`PaneLayout`]. The result is clamped into the window
/// on both axes and can be zero-sized, which is the honest answer for a window
/// dragged smaller than its own chrome; a zero-sized pane is placed and draws
/// nothing, and the caller does not have to special-case it.
#[must_use]
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

#[derive(Props, Clone, PartialEq)]
pub struct TerminalPaneProps {
    pub state: Signal<UiState>,
    /// Where the daemon is, for the bar's idle line.
    pub server: String,
    /// The operator's home directory, so the bar can draw `~`.
    pub home: String,
    pub on_new_session: EventHandler<()>,
    pub on_close_tab: EventHandler<SessionId>,
    pub on_retry: EventHandler<()>,
    /// Start one session outright, with no layer at all: the directory, then
    /// the command line exactly as it should be split and spawned.
    ///
    /// The first-run pane's whole point. Every other route to a session from
    /// an empty window opened the launcher first, which asked an operator who
    /// had never seen this product to answer three questions before it would
    /// tell them what it was for. The caller validates and reports, because
    /// this pane owns no state and raises no flash of its own.
    pub on_start: EventHandler<(String, String)>,
    /// Where the pane goes, whenever that changes.
    ///
    /// The rectangle is computed here because the layout is known here: the
    /// sidebar's width, the text scale and the two chrome bands are all this
    /// module's tokens. It is APPLIED by the shell, because the shell is the
    /// only place holding a window handle and a pane, and because a component
    /// that reached for either could not be rendered by a test.
    ///
    /// Fired on every render whose layout inputs moved, and only then;
    /// placing a widget where it already is costs one comparison in the pane.
    pub on_frame: EventHandler<PaneFrame>,
    /// The window's client area, in device pixels, and the display scale.
    ///
    /// From the platform, so it is a prop rather than something read out of
    /// [`UiState`]: the model has never held a pixel size and should not
    /// start. Zero means the window has not reported a size yet, and the
    /// frame is not fired.
    pub window_px: (u32, u32),
    pub scale: f64,
}

#[component]
pub fn TerminalPane(props: TerminalPaneProps) -> Element {
    // Hooks first, and unconditionally.
    //
    // Reading the machine costs one `PATH` walk per known agent plus one
    // profile read. It happens once, on a thread of its own, and only while
    // the pane is genuinely empty, so a window with a session in it pays
    // nothing and an idle window never rescans. An answer that arrives after
    // the first session started is dropped by `use_resource`, which has
    // already cancelled the future that would have received it.
    let state = props.state;
    let vacant = use_memo(move || matches!(pane_state(&state.read()), PaneState::Empty));
    let machine = use_resource(move || {
        let vacant = vacant();
        async move {
            if !vacant {
                return None;
            }
            Some(dialog::off_thread(firstrun::read_machine).await)
        }
    });

    // Where the pane goes, recomputed only when an input to it moved.
    //
    // A memo and not a bare call, so a daemon pushing output twenty times a
    // second does not hand the pane a rectangle twenty times a second. The
    // five inputs are the whole of what the arithmetic reads; everything else
    // that changes in this component leaves the pane exactly where it was.
    let window_px = props.window_px;
    let scale = props.scale;
    let frame = use_memo(move || {
        let st = state.read();
        // One rem for both lengths below, and the same one the document root
        // is set to. The rail is `3rem` in the stylesheet and the sidebar's
        // own width is a pixel count the operator set, so only the rail
        // follows the text scale; reading an unscaled rem for it while the
        // frame's rem is scaled puts the pane's left edge in the wrong place
        // by the whole difference.
        let rem_px = crate::ui::settings::rem_px(st.daemon.settings.text_scale_pct);
        pane_frame(&PaneLayout {
            window_w: window_px.0,
            window_h: window_px.1,
            scale,
            rem_px,
            sidebar_css: if st.window.sidebar_collapsed {
                SIDEBAR_RAIL_REM * rem_px
            } else {
                st.window.sidebar_width
            },
        })
    });
    // An effect and not a render step: placing a widget is not markup, and it
    // has to happen after the render that moved it. A window that has not
    // reported its size yet is not placed at all, because a zero-sized
    // rectangle handed to the pane is a resize the child would answer.
    let on_frame = props.on_frame;
    use_effect(move || {
        let f = frame();
        if f.width > 0 && f.height > 0 {
            on_frame.call(f);
        }
    });

    let st = props.state.read();
    let pane = pane_state(&st);
    let focused = st.window.focused;
    let offline = st.daemon.conn.is_retryable();
    let connecting = matches!(st.daemon.conn, ConnState::Connecting);
    let ready = st.server_ready();
    let bar = pane_bar(&st, &props.home, &props.server);
    let bar_tip = bar_title(&bar);

    // What the first-run pane offers, and the directory its rows launch in.
    // `None` for the few milliseconds before the reading lands, which is why
    // the headline and the sentence under it are constants: the product says
    // what it is immediately, and only the machine-dependent half waits.
    let read = machine.read();
    let first: Option<(firstrun::FirstRun, String)> = (*read)
        .as_ref()
        .and_then(|m| m.as_ref())
        .map(|m| {
            let seeded = crate::actions::seed_dir(&st, None);
            let here = if seeded.trim().is_empty() {
                m.cwd.clone()
            } else {
                seeded
            };
            let projects = &st.daemon.projects;
            let home = &m.home;
            let view = firstrun::first_run(m, &here, |cwd| dialog::place_of(projects, cwd, home));
            (view, here)
        });
    let chord = firstrun::other_way();

    rsx! {
        // The region the native pane is placed over. No children, ever: the
        // element exists to reserve space and to be the thing the frame
        // arithmetic describes, and it is never painted.
        div {
            key: "{TERMINAL_KEY}",
            id: "rg-term",
            class: "rg-terminal",
        }

        match pane {
            PaneState::Live | PaneState::Exited { .. } => rsx! {},
            // Connecting and offline are NOT empty states. A failure that
            // renders as "nothing here yet" is a lie, so each keeps its own
            // sentence and its own modifier, and neither is ever shown the
            // first-run copy: telling somebody what the product is for while
            // the thing that runs it is unreachable is the wrong sentence.
            PaneState::Empty if connecting => rsx! {
                div { class: "rg-terminal__empty rg-terminal__empty--connecting",
                    span { class: "rg-terminal__empty-hint",
                        "Connecting to the session daemon."
                    }
                }
            },
            PaneState::Empty if offline => rsx! {
                div { class: "rg-terminal__empty rg-terminal__empty--offline",
                    span { class: "rg-terminal__empty-hint",
                        "The session daemon is not answering."
                    }
                    button {
                        class: "rg-btn rg-btn--primary",
                        r#type: "button",
                        onclick: move |_| props.on_retry.call(()),
                        "Retry"
                    }
                }
            },
            // The first thirty seconds. One statement of what this is, one
            // aimed action, and the roster of agents this machine can and
            // cannot run. Everything on it is decided by
            // `firstrun::first_run`, so the rules are asserted without a DOM.
            PaneState::Empty => rsx! {
                div { class: "rg-terminal__empty rg-terminal__empty--first",
                    // ONE column, and it is a fixed one.
                    //
                    // The panel used to be a centred flex column with no
                    // width, so it was as wide as its widest child and every
                    // child was centred against a different axis: the
                    // headline against its own length, the agent list against
                    // the longest agent note. The list arrives after a PATH
                    // walk, so the whole panel changed width when it landed
                    // and every line in it moved sideways. A stated column
                    // centres once, against the pane, and nothing in it moves
                    // when the reading arrives.
                    div { class: "rg-first",
                        h2 { class: "rg-first__headline", "{firstrun::HEADLINE}" }
                        p { class: "rg-first__blurb", "{firstrun::BLURB}" }

                        if let Some((view, here)) = first {
                            if let Some(start) = view.start {
                                button {
                                    class: "rg-btn rg-btn--primary rg-first__go",
                                    r#type: "button",
                                    // The one keystroke. On a window with
                                    // nothing in it there is no grid to steal
                                    // focus, so the second launch is Enter.
                                    autofocus: true,
                                    disabled: !ready,
                                    onclick: {
                                        let cwd = start.cwd.clone();
                                        let line = start.line.clone();
                                        move |_| props.on_start.call((cwd.clone(), line.clone()))
                                    },
                                    "{start.label}"
                                }
                            }
                            if let Some(caption) = view.caption {
                                span { class: "rg-first__caption", "{caption}" }
                            }
                            if let Some(said) = view.nothing {
                                p { class: "rg-first__nothing", "{said}" }
                            }
                            ul { class: "rg-first__agents",
                                for offer in view.offers {
                                    li {
                                        key: "{offer.command}",
                                        class: if offer.primary {
                                            "rg-first__agent rg-first__agent--on"
                                        } else if offer.installed {
                                            "rg-first__agent"
                                        } else {
                                            "rg-first__agent rg-first__agent--missing"
                                        },
                                        if offer.installed && !offer.primary {
                                            button {
                                                class: "rg-first__pick",
                                                r#type: "button",
                                                disabled: !ready,
                                                onclick: {
                                                    let cwd = here.clone();
                                                    let line = offer.command.to_string();
                                                    move |_| props.on_start.call((cwd.clone(), line.clone()))
                                                },
                                                span { class: "rg-first__name", "{offer.label}" }
                                                span { class: "rg-first__note", "{offer.note}" }
                                            }
                                        } else {
                                            span { class: "rg-first__pick",
                                                span { class: "rg-first__name", "{offer.label}" }
                                                span { class: "rg-first__note", "{offer.note}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(chord) = chord {
                            span { class: "rg-terminal__empty-hint",
                                "Something else: "
                                kbd { "{chord}" }
                            }
                        }
                    }
                }
            },
            PaneState::Unfocused => rsx! {
                div { class: "rg-terminal__empty rg-terminal__empty--unfocused",
                    div { class: "rg-first",
                        span { class: "rg-terminal__empty-title", "No session focused" }
                        span { class: "rg-terminal__empty-hint",
                            "Pick a session in the sidebar, or press "
                            kbd { "Alt+1" }
                            " through "
                            kbd { "Alt+9" }
                            "."
                        }
                    }
                }
            },
        }

        // ONE line, at every state, forever.
        //
        // Outside the `match` above and outside every condition in this
        // module. The bar is the only child of `.rg-main` below the pane that
        // occupies space, and its height is a constant, so nothing that
        // happens to a session can change the pane's rectangle. An exit is a
        // WORD in this bar, not a strip that appears above it and shortens the
        // grid by 32px at the moment the operator is reading its last output.
        div { class: "rg-panebar", title: "{bar_tip}",
            if let Some(mark) = bar.mark {
                svg {
                    class: "rg-panebar__agent",
                    view_box: "0 0 16 16",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "1.25",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    "aria-hidden": "true",
                    path { d: "{mark.stroke}" }
                    if !mark.fill.is_empty() {
                        path { d: "{mark.fill}", fill: "currentColor", stroke: "none" }
                    }
                }
            }
            span { class: "rg-panebar__place", "{bar.place}" }
            if let Some(worktree) = bar.worktree {
                span { class: "rg-panebar__worktree",
                    span { class: "rg-panebar__key", "worktree" }
                    span { class: "rg-panebar__value", "{worktree}" }
                }
            }
            if let Some(branch) = bar.branch {
                span { class: "rg-panebar__branch", "{branch}" }
            }
            span { class: "rg-panebar__gap" }
            if let Some(exit) = bar.exit {
                span { class: "rg-panebar__exit", "{exit}" }
                if let Some(id) = focused {
                    button {
                        class: "rg-btn-inline",
                        r#type: "button",
                        onclick: move |_| props.on_close_tab.call(id),
                        // Same wording as the row menu and the shortcut list:
                        // this stops drawing the transcript, and the session
                        // it belongs to has already exited.
                        "Stop viewing"
                    }
                }
            }
            if let Some(grid) = bar.grid {
                span { class: "rg-panebar__grid", "{grid}" }
            }
            if let Some(pill) = bar.state {
                span { class: "rg-panebar__state {pill.class}",
                    span { class: "rg-pill__word", "{pill.word}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
