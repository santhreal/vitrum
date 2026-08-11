//! What the frame promises.
//!
//! Three groups, and they close three different classes.
//!
//! - **Geometry.** [`super::pane_frame`] is the only description of where the
//!   pane is, and nothing measures the document to check it. So the tokens it
//!   reads are held against the stylesheet's, and the rectangle is held
//!   against the window at every scale the product offers.
//! - **The column.** A strip that takes a line from the pane resizes the PTY
//!   and makes every agent on screen repaint. The stylesheet makes that
//!   impossible by default; the guard here makes a deliberate exception
//!   visible.
//! - **The bar.** Every string on the one permanent surface in the window,
//!   asserted without a DOM, plus the rendered markup for the two claims that
//!   source text cannot carry: that the bar is emitted at every state, and
//!   that no state emits a second strip.

use super::*;

use crate::testkit::NOW;
use vitrum_model::SessionView;
use vitrum_proto::{Attention, ProjectId, SessionInfo};

const APP_CSS: &str = include_str!("../../app.css");
const SIDEBAR_CSS: &str = include_str!("../../../assets/sidebar.css");

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

/// One custom property's declared value, verbatim.
fn token<'a>(css: &'a str, name: &str) -> &'a str {
    let (_, rest) = css
        .split_once(&format!("{name}:"))
        .unwrap_or_else(|| panic!("no stylesheet declares {name}"));
    let (value, _) = rest
        .split_once(';')
        .unwrap_or_else(|| panic!("{name}'s declaration never ends"));
    value.trim()
}

/// One CSS length in rem, for the tokens this module also holds in Rust.
fn token_rem(css: &str, name: &str) -> f64 {
    let value = token(css, name);
    value
        .strip_suffix("rem")
        .unwrap_or_else(|| panic!("{name} is {value}, which this guard cannot resolve to rem"))
        .trim()
        .parse()
        .expect("a rem length is a number")
}

/// One rule's declaration block.
fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
    let (_, rest) = css
        .split_once(&format!("\n{selector} {{"))
        .unwrap_or_else(|| panic!("no stylesheet rule for {selector}"));
    let (block, _) = rest
        .split_once('}')
        .unwrap_or_else(|| panic!("{selector}'s rule never closes"));
    block
}

// ───────────────────────────────────────────────────────────────────────────
// Geometry
// ───────────────────────────────────────────────────────────────────────────

/// WHY: the rectangle is computed from numbers that also live in a stylesheet,
/// and nothing at run time compares them.
///
/// The class: the pane is a native widget positioned by arithmetic. That
/// arithmetic cannot read the document, so every token it uses is duplicated
/// in Rust. A token edited on one side only does not fail, warn, or look
/// wrong in a diff: the widget is placed a few pixels off the box the
/// stylesheet reserved, forever, and the operator sees a seam.
///
/// What it does not catch: a token this module does not use. That is
/// deliberate; the pair is the contract, not the file.
#[test]
fn the_frame_reads_the_same_tokens_the_stylesheet_does() {
    for (css, name, held) in [
        (APP_CSS, "--rg-titlebar-h", TITLEBAR_REM),
        (APP_CSS, "--rg-panebar-h", PANEBAR_REM),
        (APP_CSS, "--rg-pane-pad", PANE_PAD_REM),
        (
            SIDEBAR_CSS,
            "--rg-sidebar-width-collapsed",
            SIDEBAR_RAIL_REM,
        ),
    ] {
        assert_eq!(
            token_rem(css, name),
            held,
            "{name} is declared once in the stylesheet and once in \
             ui/terminal.rs, and the two no longer agree, so the native pane \
             is placed off the box the document reserved for it"
        );
    }
}

/// WHY: a pane handed a box larger than the window puts its last rows behind
/// the window edge.
///
/// The class: an approval prompt's option list sliced off at the bottom is
/// this defect and nothing else. The pane divides the box it is given by the
/// cell height, so a box that is 28px too tall is one to two rows the operator
/// cannot see, and those rows are at the BOTTOM, which is exactly where a TUI
/// puts the thing it is waiting for an answer to.
///
/// Every scale and text scale, because the arithmetic rounds and a rounding
/// that is safe at 1.0 can overhang at 1.25.
#[test]
fn the_frame_never_reaches_the_window_edge() {
    for &(w, h) in &[
        (1920u32, 1080u32),
        (3840, 2160),
        (1280, 720),
        (800, 600),
        // Narrower and shorter than the product's own minimum, because a
        // window manager can and does hand a client a size it never asked
        // for during a workspace switch.
        (200, 120),
    ] {
        for &scale in &[1.0f64, 1.25, 1.5, 2.0] {
            for &pct in &[75u16, 100, 150] {
                let rem = crate::ui::settings::ROOT_FONT_PX * f64::from(pct) / 100.0;
                let l = PaneLayout {
                    window_w: w,
                    window_h: h,
                    scale,
                    rem_px: rem,
                    sidebar_css: 256.0,
                };
                let f = pane_frame(&l);
                let pad = (PANE_PAD_REM * rem * scale).round();
                let bar = (PANEBAR_REM * rem * scale).round();

                assert!(
                    f.right() <= i64::from(w),
                    "the pane hangs {} device px past the right edge at \
                     {w}x{h} scale {scale} text {pct}%",
                    f.right() - i64::from(w)
                );
                assert!(
                    f.bottom() + bar as i64 + pad as i64 <= i64::from(h),
                    "the pane's last row is behind the bar at {w}x{h} scale \
                     {scale} text {pct}%: frame ends at {}, and the bar plus \
                     padding needs {} of the {h} available",
                    f.bottom(),
                    bar + pad
                );
                assert!(f.x >= 0 && f.y >= 0, "the pane starts off-window");
            }
        }
    }
}

/// WHY: a pane placed at the window origin covers the sidebar and the
/// titlebar.
///
/// The pane is a sibling widget over the webview, not a child of anything the
/// document lays out, so nothing stops it being drawn on top of the panel. The
/// only thing keeping it inside its region is this arithmetic.
#[test]
fn the_frame_starts_after_the_chrome_beside_it() {
    let l = PaneLayout {
        window_w: 1920,
        window_h: 1080,
        scale: 1.0,
        rem_px: 16.0,
        sidebar_css: 256.0,
    };
    let f = pane_frame(&l);
    assert_eq!(f.x, 256 + 8, "the pane starts inside the sidebar");
    assert_eq!(f.y, 36 + 8, "the pane starts under the titlebar");
    assert_eq!(f.width, 1920 - 256 - 16);
    assert_eq!(f.height, 1080 - 36 - 28 - 16);
}

/// A window dragged smaller than its own chrome gets a zero-sized pane, not a
/// negative one and not a panic. `u32` cannot hold a negative width, so the
/// alternative to clamping is a wrap to four billion.
#[test]
fn a_window_smaller_than_its_chrome_gets_an_empty_pane() {
    let f = pane_frame(&PaneLayout {
        window_w: 100,
        window_h: 40,
        scale: 1.0,
        rem_px: 16.0,
        sidebar_css: 256.0,
    });
    assert_eq!(f.width, 0);
    assert_eq!(f.height, 0);
}

/// WHY: the shell places the native pane at whatever this rectangle says and
/// then checks it against the window through [`PaneFrame::right`] and
/// [`PaneFrame::bottom`]. A frame that ends outside the window is a surface
/// the compositor draws and the operator cannot see, which is where an
/// approval prompt loses its last option.
///
/// The case is not hypothetical. A window manager hands a client whatever
/// size it likes during a workspace switch, and that size is routinely
/// smaller on BOTH axes than the sidebar beside the pane and the titlebar and
/// bar around it. Clamping one axis and not the other leaves the origin past
/// the far edge, so both are asserted at every scale the product offers.
#[test]
fn a_window_smaller_than_its_chrome_keeps_the_frame_inside_it_on_both_axes() {
    for scale in [1.0, 1.25, 2.0] {
        for (w, h) in [(0u32, 0u32), (1, 1), (100, 40), (300, 30), (40, 900)] {
            let f = pane_frame(&PaneLayout {
                window_w: w,
                window_h: h,
                scale,
                rem_px: 16.0,
                sidebar_css: 256.0,
            });
            assert!(
                f.x >= 0 && f.y >= 0,
                "{w}x{h} at {scale} put the origin at {},{}",
                f.x,
                f.y
            );
            assert!(
                f.right() <= i64::from(w),
                "{w}x{h} at {scale} ended at x={} past the right edge",
                f.right()
            );
            assert!(
                f.bottom() <= i64::from(h),
                "{w}x{h} at {scale} ended at y={} past the bottom edge",
                f.bottom()
            );
        }
    }
}

/// A scale the platform reports as zero, or a text scale read from a corrupt
/// profile, must not divide the window by nothing. The frame falls back to the
/// defaults rather than producing a rectangle nobody can place.
#[test]
fn a_nonsense_scale_falls_back_rather_than_producing_a_nonsense_box() {
    let sane = pane_frame(&PaneLayout {
        window_w: 1920,
        window_h: 1080,
        scale: 1.0,
        rem_px: 16.0,
        sidebar_css: 0.0,
    });
    for bad in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        let f = pane_frame(&PaneLayout {
            window_w: 1920,
            window_h: 1080,
            scale: bad,
            rem_px: 16.0,
            sidebar_css: 0.0,
        });
        assert_eq!(f, sane, "scale {bad} produced a different box");
        let f = pane_frame(&PaneLayout {
            window_w: 1920,
            window_h: 1080,
            scale: 1.0,
            rem_px: bad,
            sidebar_css: 0.0,
        });
        assert_eq!(f, sane, "root font {bad} produced a different box");
    }
}

/// WHY: two places deriving one grid disagree by a row exactly at a rounding
/// boundary, and nowhere else.
///
/// The class: the shell subtracts the chrome and hands over a rectangle, the
/// pane divides that rectangle by a cell. If either side floors where the
/// other rounds, or accumulates a float where the other took a whole pixel,
/// the two answers differ for a narrow band of window sizes and agree
/// everywhere a developer would look. The operator sees an approval prompt
/// with its last option cut off on one window size and not on the next.
///
/// The invariant, at the boundary both sides cross: the whole window divided
/// by a cell with the chrome sum taken out equals the frame's own rectangle
/// divided by the same cell. It is asserted over the scales and text scales
/// the product offers, because a subtraction that is exact at 1.0 is
/// fractional at 1.25, and over several cell sizes, because an equality that
/// holds for one cell is not an equality.
#[test]
fn the_grid_the_frame_yields_is_the_grid_the_pane_derives() {
    use crate::pane::PaneRect;

    for (w, h) in [
        (1920u32, 1080u32),
        (3840, 2160),
        (1279, 719),
        (1281, 721),
        (800, 601),
        (200, 120),
        (100, 40),
        (1, 1),
    ] {
        for scale in [1.0f64, 1.25, 1.5, 2.0] {
            for pct in [75u16, 100, 150] {
                let rem = crate::ui::settings::ROOT_FONT_PX * f64::from(pct) / 100.0;
                let f = pane_frame(&PaneLayout {
                    window_w: w,
                    window_h: h,
                    scale,
                    rem_px: rem,
                    sidebar_css: 256.0,
                });
                // The chrome from the stylesheet tokens, derived here and
                // not read back off the frame. A chrome taken from the
                // frame's own edges cancels out of both sides of the
                // comparison, and a frame handed one pixel too many would
                // agree with itself. This is the independent reading the
                // equality needs to be worth asserting.
                let pad = (PANE_PAD_REM * rem * scale).round();
                let chrome_x = (256.0 * scale).round() + 2.0 * pad;
                let chrome_y =
                    (TITLEBAR_REM * rem * scale).round() + (PANEBAR_REM * rem * scale).round()
                        + 2.0 * pad;
                for (cw, ch) in [(6u32, 13u32), (8, 16), (9, 19), (14, 31)] {
                    let from_window = geometry::pane_grid(
                        f64::from(w),
                        f64::from(h),
                        chrome_x,
                        chrome_y,
                        f64::from(cw),
                        f64::from(ch),
                    );
                    let from_frame = PaneRect {
                        x: f.x,
                        y: f.y,
                        width: f.width,
                        height: f.height,
                    }
                    .grid((cw, ch));
                    assert_eq!(
                        from_window, from_frame,
                        "{w}x{h} at scale {scale} text {pct}% cell {cw}x{ch}: \
                         the window says {from_window:?} and the frame says \
                         {from_frame:?}"
                    );
                    // No cell may be counted that the rectangle cannot show.
                    // The floors are allowed to exceed it, because a grid
                    // below them is not a terminal, and only then.
                    let (cols, rows) = from_frame;
                    assert!(
                        u32::from(cols) * cw <= f.width || cols == geometry::MIN_COLS,
                        "{w}x{h} at {scale}: {cols} columns of {cw}px do not \
                         fit in {}px",
                        f.width
                    );
                    assert!(
                        u32::from(rows) * ch <= f.height || rows == geometry::MIN_ROWS,
                        "{w}x{h} at {scale}: {rows} rows of {ch}px do not fit \
                         in {}px",
                        f.height
                    );
                }
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The column
// ───────────────────────────────────────────────────────────────────────────

/// WHY: a strip that takes a line from the pane makes every agent on screen
/// repaint its whole viewport.
///
/// The class, and the chain that makes it expensive: an in-flow child of
/// `.rg-main` shortens the pane's box, a shorter box is fewer rows, fewer rows
/// is a PTY resize, and a resize makes a full-screen TUI reflow and repaint
/// everything it has drawn. So a two-line notice about ONE session flashes
/// EVERY agent in the window, twice.
///
/// The stylesheet makes the wrong thing the default: `.rg-main > *` is
/// absolutely positioned, so a strip added by anybody, in any file, is out of
/// flow whether or not they knew this rule. What this guard adds is the other
/// half: an opt-out is a decision, and a third one turns the suite red until
/// somebody records why the pane may be shortened for it.
///
/// What it does not catch: a child that opts out through a different
/// mechanism, such as its own `position: static` in a more specific rule.
/// Nothing in this tree does that and the default above makes it pointless.
#[test]
fn the_pane_column_holds_only_the_pane_and_its_bar() {
    assert!(
        APP_CSS.contains("\n.rg-main > * {"),
        "the default that puts every child of .rg-main out of flow is gone, \
         so a strip added later takes a line from the pane and reflows every \
         agent in the window"
    );
    assert!(
        rule(APP_CSS, ".rg-main > *").contains("position: absolute"),
        "the default no longer removes a child from flow"
    );

    // Derived from the file, not listed here: a fourth opt-out appears in this
    // set the moment somebody writes it, and the assertion below fails naming
    // it.
    let mut opted: Vec<&str> = APP_CSS
        .match_indices(".rg-main > .")
        .map(|(i, _)| {
            let rest = &APP_CSS[i + ".rg-main > ".len()..];
            let end = rest
                .find(|c: char| c == ',' || c == ' ' || c == '\n' || c == '{')
                .unwrap_or(rest.len());
            &rest[..end]
        })
        .collect();
    opted.sort_unstable();
    opted.dedup();
    assert_eq!(
        opted,
        vec![".rg-panebar", ".rg-terminal"],
        "exactly two things in the pane column may occupy space: the pane and \
         the bar under it. Anything else here shortens the pane and repaints \
         every agent on screen"
    );
}

/// WHY: a bar whose height follows its content is a bar that resizes the pane.
///
/// The whole reason the exit bar was folded into a permanent strip is that it
/// appeared and disappeared. A `min-height`, or a height left to the content,
/// puts that back the moment a two-line message is written into it.
#[test]
fn the_bar_is_the_same_box_empty_as_it_is_full() {
    let block = rule(APP_CSS, ".rg-panebar");
    assert!(
        block.contains("height: var(--rg-panebar-h)"),
        "the bar no longer states its height, so an empty bar and a bar with \
         an exit sentence in it are different boxes and the pane resizes \
         between them:\n{block}"
    );
    assert!(
        !block.contains("min-height"),
        "a min-height lets content grow the bar, which is the resize this \
         strip exists to remove:\n{block}"
    );
    assert!(
        block.contains("flex: 0 0 var(--rg-panebar-h)"),
        "the bar can be compressed or stretched by its siblings, so its \
         height is not the token after all:\n{block}"
    );
}

/// The exit bar is gone, in markup and in the stylesheet. A rule left behind
/// for a class nothing emits is dead weight that reads as a live surface.
#[test]
fn nothing_still_draws_a_separate_exit_bar() {
    assert!(
        !APP_CSS.contains("rg-exitbar"),
        "the exit bar's rules survive, so the strip that appeared under the \
         pane when a child died can come back with one line of markup"
    );
    assert!(
        !include_str!("../terminal.rs").contains("rg-exitbar"),
        "the frame still emits the exit bar"
    );
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
