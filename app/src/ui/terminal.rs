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
use crate::ui::{dialog, firstrun};

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
    /// Start one session outright, with no layer at all: the directory, then
    /// the command line exactly as it should be split and spawned.
    ///
    /// The first-run pane's whole point. Every other route to a session from
    /// an empty window opened the launcher first, which asked an operator who
    /// had never seen this product to answer three questions before it would
    /// tell them what it was for. The caller validates and reports, because
    /// this pane owns no state and raises no flash of its own.
    pub on_start: EventHandler<(String, String)>,
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

    let st = props.state.read();
    let pane = pane_state(&st);
    let focused = st.window.focused;
    let offline = st.daemon.conn.is_retryable();
    let connecting = matches!(st.daemon.conn, ConnState::Connecting);
    let ready = st.server_ready();

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
        // Owned by JavaScript from here down. Do not add children.
        div {
            key: "{TERMINAL_KEY}",
            id: "rg-term",
            class: "rg-terminal",
        }

        match pane {
            PaneState::Live => rsx! {},
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
                    h2 { class: "rg-first__headline", "{firstrun::HEADLINE}" }
                    p { class: "rg-first__blurb", "{firstrun::BLURB}" }

                    if let Some((view, here)) = first {
                        if let Some(start) = view.start {
                            button {
                                class: "rg-btn rg-btn--primary rg-first__go",
                                r#type: "button",
                                // The one keystroke. On a window with nothing
                                // in it there is no grid to steal focus, so
                                // the second launch is Enter.
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
            },
            PaneState::Unfocused => rsx! {
                div { class: "rg-terminal__empty rg-terminal__empty--unfocused",
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
                            // Same wording as the row menu and the shortcut
                            // list: this stops drawing the transcript, and
                            // the session it belongs to has already exited.
                            "Stop viewing"
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

    // -----------------------------------------------------------------------
    // The pane's grid arithmetic
    //
    // Test-only, and that is the honest shape of it. The measurement happens
    // in the webview, because the pane's pixel box is the DOM's and nothing on
    // this side can see it; `bootstrap.js::paneGrid` runs this arithmetic on
    // the live box and reports the result as `BridgeEvent::Resize`. What lives
    // here is the definition, written where a test can run it over sizes no
    // window will ever be dragged to, against a bridge that cannot be run at
    // all without a display.
    //
    // The seam is real and is not papered over: this cannot prove the bridge
    // divides the way it says it does, only that the terms are there.
    // `the_bridge_measures_the_pane_the_way_this_module_does` is that half,
    // and it is as much as source text can carry. The seam closes when the
    // native pane in `app/src/pane` hosts a session and the box is Rust's.
    // -----------------------------------------------------------------------

    /// Fewest columns a pane is ever handed.
    ///
    /// xterm's own floor. A one-column grid is not a terminal, and a child
    /// told it has one wraps every line into a stripe.
    const MIN_COLS: u16 = 2;

    /// Fewest rows a pane is ever handed. One, because a child with zero rows
    /// has nowhere to draw and the emulator rejects the resize.
    const MIN_ROWS: u16 = 1;

    /// Whole cells that fit along one axis of a pane.
    ///
    /// `box_px` is the axis of the box the pane occupies. `chrome_px` is the
    /// AXIS SUM of everything inside that box a cell cannot occupy: the
    /// container's padding, plus a scrollbar gutter on the horizontal axis. A
    /// sum and not two sides, because that is the whole of what the arithmetic
    /// can see, and taking two would invite a caller to believe the
    /// distribution matters.
    ///
    /// Floor, never round and never ceil. A row the window edge cuts in half
    /// is not a row anybody can read, and counting it puts the last line of a
    /// full-screen TUI under the frame.
    fn cells_across(box_px: f64, chrome_px: f64, cell_px: f64) -> u32 {
        if !(cell_px > 0.0) || !box_px.is_finite() || !chrome_px.is_finite() {
            return 0;
        }
        let whole = ((box_px - chrome_px) / cell_px).floor();
        if whole.is_finite() && whole > 0.0 {
            whole as u32
        } else {
            0
        }
    }

    /// The grid a pane's pixel box can actually show.
    ///
    /// The one place this arithmetic is written down. `bootstrap.js` measures
    /// the live box and computes the same two numbers the same way, and
    /// `the_bridge_measures_the_pane_the_way_this_module_does` holds the two
    /// together, because what comes out of here leaves the process as
    /// `ClientMsg::Resize` and an agent redraws to it.
    ///
    /// The defect this replaced was one subtraction. The pane delegated to
    /// xterm's fit addon, which reads `getComputedStyle(container).height`;
    /// that resolves to the BORDER box under `box-sizing: border-box`, which
    /// `.rg-app *` sets on every element in the window, and the addon then
    /// subtracts the padding of the inner `.xterm` element, which has none,
    /// rather than the container's. `.rg-terminal` carries 24px above and 8px
    /// below, so the child was told it had two rows the window could not show
    /// and four columns off the right edge.
    fn pane_grid(
        box_w: f64,
        box_h: f64,
        chrome_x: f64,
        chrome_y: f64,
        cell_w: f64,
        cell_h: f64,
    ) -> (u16, u16) {
        let axis = |n: u32, min: u16| n.clamp(u32::from(min), u32::from(u16::MAX)) as u16;
        (
            axis(cells_across(box_w, chrome_x, cell_w), MIN_COLS),
            axis(cells_across(box_h, chrome_y, cell_h), MIN_ROWS),
        )
    }

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

    /// WHY: a pane's padding is a visual decision that silently resizes the
    /// PTY.
    ///
    /// The class: `.rg-terminal`'s padding is not decoration. It comes out of
    /// the box before [`pane_grid`] divides, so the grid is
    /// `floor((paneHeight - padTop - padBottom) / cellHeight)` by
    /// `floor((paneWidth - padLeft - padRight - scrollbar) / cellWidth)`, and
    /// that grid goes out as a `Resize` on the wire. An agent redraws to it.
    /// Somebody nudging the transcript down from the titlebar therefore takes
    /// a row away from the agent, on some window heights and not others, and
    /// it is discovered as a wrapped line in a diff three days later, never as
    /// a padding change, because nothing connects the two.
    ///
    /// The invariant that makes such an edit safe is narrower than "the
    /// padding is these numbers": the fit arithmetic reads only the SUM per
    /// axis, so any redistribution WITHIN an axis is provably invisible to
    /// the child, and any change to a sum is provably visible. This asserts
    /// the sums, and then asserts the thing the sums are a proxy for, by
    /// running the fit both ways over a sweep of pane and cell sizes.
    ///
    /// It also asserts WHERE the numbers come from. Every side of the
    /// shorthand has to be a `var(--rg-*)` out of the spacing scale rather
    /// than a bare length, because a literal here is the only kind of edit
    /// that can move an axis sum without anybody reading the scale first,
    /// and this is the one declaration in that file the PTY can feel.
    ///
    /// Both sides are read out of the stylesheet, so this test holds no copy
    /// of 24, 16 or 8 to go stale: the shipped value is the `.rg-terminal`
    /// declaration, and the baseline it must match is the four-sided
    /// `--rg-inset` that declaration replaced.
    ///
    /// The mutations this catches: `padding: 1.5rem var(--rg-inset) 0.5rem`
    /// (same pixels, literal lengths, so the scale no longer owns them);
    /// `padding: var(--rg-space-7) var(--rg-inset) var(--rg-space-2)` (the
    /// vertical sum goes 32 -> 40 and the agent loses a row on some heights);
    /// and `padding: var(--rg-space-4)` (both sums change at once).
    ///
    /// What it does NOT catch: a border or a scrollbar gutter on the same
    /// element, which also come out of the box, and whether the bridge
    /// measures the box correctly in the first place, which is
    /// [`the_bridge_measures_the_pane_the_way_this_module_does`].
    #[test]
    fn pane_padding_never_changes_the_grid_the_pty_gets() {
        let css = include_str!("../../assets/parts/10-spacing.css");
        let sides = padding_sides(css, ".rg-terminal");
        for side in &sides {
            assert!(
                side.starts_with("var(--rg-"),
                "`.rg-terminal`'s padding spells one side as `{side}`, which \
                 is a literal length rather than a spacing token. The scale \
                 in this file is where a pane's air is decided, and this is \
                 the one declaration in it the PTY can feel: a number free \
                 to be anything is a row the agent loses without a review. \
                 The shorthand reads {sides:?}."
            );
        }
        let [top, right, bottom, left] = padding_of(css, ".rg-terminal");
        let inset = length_px(css, "var(--rg-inset)", css);

        // The declaration this replaced: one inset, four sides.
        let base_x = inset * 2.0;
        let base_y = inset * 2.0;
        assert_eq!(
            (left + right, top + bottom),
            (base_x, base_y),
            "`.rg-terminal` is {top}/{right}/{bottom}/{left}, whose axis sums \
             are {}x{} against the {base_x}x{base_y} the four-sided \
             --rg-inset had. An axis sum moved, so the pane now hands the \
             child a different grid than it did before this padding existed.",
            left + right,
            top + bottom
        );

        // The sums are only a proxy. This is the property itself: over every
        // pane and cell size an operator can produce, the shipped padding
        // and the baseline propose the SAME grid. A fractional cell height
        // is the interesting case, because that is where a floor boundary
        // sits between two paddings that differ by 8px.
        for pane_w in [480.0, 640.0, 903.0, 1280.0, 1920.0, 2560.0] {
            for pane_h in (200..=1400).step_by(7).map(f64::from) {
                for (cell_w, cell_h) in [(8.0, 17.0), (9.6, 19.2), (7.0, 15.0), (10.0, 21.5)] {
                    let shipped =
                        pane_grid(pane_w, pane_h, left + right, top + bottom, cell_w, cell_h);
                    let before = pane_grid(pane_w, pane_h, base_x, base_y, cell_w, cell_h);
                    assert_eq!(
                        shipped, before,
                        "a {pane_w}x{pane_h} pane of {cell_w}x{cell_h} cells \
                         fits {shipped:?} with the shipped padding and \
                         {before:?} without it: the padding resized the \
                         terminal, and the agent redraws to the smaller grid"
                    );
                }
            }
        }
    }

    /// WHY: the number of rows the child is told it has must be the number of
    /// rows the operator can see.
    ///
    /// The class this closes is off-by-a-partial-cell in either direction. A
    /// grid one row too tall hides the bottom line of a full-screen TUI under
    /// the window edge, which is where an approval prompt puts its last
    /// option; a grid one row too short leaves a dead band the child will
    /// never draw in. Both are invisible in a screenshot taken at a size that
    /// happens to divide evenly, so the table is deliberately built out of
    /// sizes that do not.
    ///
    /// The shipped defect is the `chrome_px` column being zero: xterm's fit
    /// addon read `getComputedStyle(container).height`, which is the BORDER
    /// box under the `box-sizing: border-box` this window sets on every
    /// element, and then subtracted the padding of the inner `.xterm`, which
    /// has none. With `.rg-terminal`'s 32px per axis that is the last column
    /// of every row below: four columns and two rows the operator does not
    /// have.
    ///
    /// What it does NOT catch: whether the live box is measured correctly,
    /// which is [`the_bridge_measures_the_pane_the_way_this_module_does`].
    #[test]
    fn a_pane_is_only_told_about_cells_it_can_show_whole() {
        // box w, box h, chrome x, chrome y, cell w, cell h, cols, rows, and
        // the grid the pre-fix arithmetic proposed for the same pane.
        let table = [
            (1280.0, 800.0, 32.0, 32.0, 8.0, 17.0, 156, 45, (160, 47)),
            (1281.0, 807.0, 32.0, 32.0, 9.6, 19.2, 130, 40, (133, 42)),
            (1920.0, 1440.0, 32.0, 32.0, 8.0, 19.2, 236, 73, (240, 75)),
            (640.0, 400.0, 32.0, 32.0, 7.0, 21.5, 86, 17, (91, 18)),
            // The boundary pair. 797 is exactly 32 of chrome plus 45 whole
            // rows; 796 is one pixel short of the 45th and must report 44.
            (903.0, 797.0, 32.0, 32.0, 10.0, 17.0, 87, 45, (90, 46)),
            (903.0, 796.0, 32.0, 32.0, 10.0, 17.0, 87, 44, (90, 46)),
            // Below one cell on both axes. The child still needs a grid it
            // can address, so the floor is xterm's and not the arithmetic's.
            (10.0, 10.0, 32.0, 32.0, 8.0, 17.0, 2, 1, (2, 1)),
        ];
        for (w, h, cx, cy, cell_w, cell_h, cols, rows, before) in table {
            assert_eq!(
                pane_grid(w, h, cx, cy, cell_w, cell_h),
                (cols, rows),
                "a {w}x{h} pane with {cx}x{cy} of chrome and {cell_w}x{cell_h} \
                 cells shows {cols}x{rows} whole cells; the arithmetic that \
                 ignored the chrome proposed {before:?}"
            );
        }

        // The property the table is a sample of, asserted before the floor
        // that xterm imposes: over every pane height an operator can drag to,
        // the rows handed out plus the chrome fit inside the box, and one
        // more row does not.
        for cell_h in [15.0, 17.0, 19.2, 21.5] {
            for chrome_y in [0.0, 8.0, 32.0, 40.0] {
                for h in (200..=1400).map(f64::from) {
                    let rows = cells_across(h, chrome_y, cell_h);
                    let painted = f64::from(rows) * cell_h + chrome_y;
                    assert!(
                        painted <= h,
                        "a {h}px pane with {chrome_y}px of chrome was told it \
                         has {rows} rows of {cell_h}px, which paints \
                         {painted}px: the last row is under the window edge"
                    );
                    assert!(
                        painted + cell_h > h,
                        "a {h}px pane with {chrome_y}px of chrome was told it \
                         has {rows} rows of {cell_h}px, leaving room for \
                         another whole one the child will never draw in"
                    );
                }
            }
        }
    }

    /// WHY: the arithmetic in [`pane_grid`] is not what runs.
    ///
    /// Rust computes no geometry at runtime. The measurement happens in the
    /// webview, where the pane's box lives, so a correct function here and a
    /// bridge that still delegates to xterm's fit addon is the defect with a
    /// test in front of it. This pins the three things the bridge must do:
    /// measure the container's own padding, subtract it, and floor. The addon
    /// is named as banned because it is the specific wrong answer, and it is
    /// one `loadAddon` away from coming back.
    ///
    /// Source text is the only surface this side has. It cannot see whether
    /// the numbers are combined correctly, only that the terms are present,
    /// so the arithmetic itself stays in [`pane_grid`] where a test can run
    /// it.
    #[test]
    fn the_bridge_measures_the_pane_the_way_this_module_does() {
        let js = crate::BOOTSTRAP_JS;
        assert!(
            !js.contains("FitAddon"),
            "the bridge is back on xterm's fit addon, which measures the \
             container's BORDER box and subtracts the inner element's \
             padding: the pane's own padding is then counted as usable, and \
             the child is handed rows the window edge cuts off"
        );
        assert!(
            js.contains("function paneGrid("),
            "the bridge no longer has a named grid measurement, so nothing \
             here can say what it hands xterm"
        );
        for term in [
            "paddingTop",
            "paddingBottom",
            "paddingLeft",
            "paddingRight",
            "Math.floor",
        ] {
            assert!(
                js.contains(term),
                "the bridge's grid measurement dropped `{term}`, so the \
                 pane's chrome is either not subtracted or not floored"
            );
        }
    }

    /// WHY: a measurement that could not be taken must not count as one.
    ///
    /// The pane refits when the observer sees its box change, and it skips
    /// the work when the box is the one the last fit saw. That cache is
    /// written inside `refit`, and writing it before the grid has actually
    /// been measured is a specific, silent failure: the synchronous fit at
    /// mount runs before the engine has necessarily measured the font, so it
    /// proposes nothing, and the observer's first delivery then matches the
    /// recorded box and returns early. The pane stays at xterm's default
    /// 80x24 for the life of the window, which is a small grid in the corner
    /// of a large pane with a dead band under it.
    ///
    /// The ordering IS the invariant, so the ordering is what this reads.
    #[test]
    fn a_measurement_that_could_not_be_taken_does_not_count_as_one() {
        let body = crate::BOOTSTRAP_JS
            .split_once("function refit(")
            .expect("the bridge has no refit")
            .1;
        let body = body.split_once("\n  }").expect("refit never closes").0;
        let measured = body.find("paneGrid()").expect("refit measures nothing");
        let recorded = body.find("fitW =").expect("refit records no box");
        assert!(
            recorded > measured,
            "refit records the box it saw before it has a grid to show for \
             it, so a fit that could not run still suppresses the next one \
             and the pane never leaves its default size:\n{body}"
        );
    }

    /// One rule's `padding` shorthand, as the source spells each side.
    ///
    /// Unresolved on purpose: the token-origin half of the guard has to see
    /// `var(--rg-space-6)` and `1.5rem` as different things, and by the time
    /// a length has been resolved to 24 they are the same thing.
    fn padding_sides(css: &str, selector: &str) -> Vec<String> {
        let (_, rest) = css
            .split_once(&format!("\n{selector} {{"))
            .unwrap_or_else(|| panic!("no stylesheet rule for {selector}"));
        let (block, _) = rest
            .split_once('}')
            .unwrap_or_else(|| panic!("{selector}'s rule never closes"));
        let (_, value) = block
            .split_once("padding:")
            .unwrap_or_else(|| panic!("{selector} declares no padding: {block}"));
        let (value, _) = value
            .split_once(';')
            .unwrap_or_else(|| panic!("{selector}'s padding never ends: {block}"));
        // `var(--x)` contains no space, so whitespace splits the shorthand.
        value.split_whitespace().map(str::to_string).collect()
    }

    /// One rule's `padding` shorthand, expanded to top/right/bottom/left px.
    fn padding_of(css: &str, selector: &str) -> [f64; 4] {
        let raw = padding_sides(css, selector);
        let sides: Vec<f64> = raw.iter().map(|part| length_px(css, part, css)).collect();
        match sides[..] {
            [all] => [all, all, all, all],
            [y, x] => [y, x, y, x],
            [t, x, b] => [t, x, b, x],
            [t, r, b, l] => [t, r, b, l],
            _ => panic!("{selector}'s padding is not a 1-to-4 value shorthand: {raw:?}"),
        }
    }

    /// One CSS length in 1x pixels, resolving a single `var()` indirection.
    ///
    /// 16px to the rem is the root the app never overrides, and the same
    /// figure `sidebar/tests.rs::token_px` uses.
    fn length_px(css: &str, value: &str, sheet: &str) -> f64 {
        let value = value.trim();
        if let Some(name) = value.strip_prefix("var(").and_then(|v| v.strip_suffix(')')) {
            let (_, rest) = sheet
                .split_once(&format!("{}:", name.trim()))
                .unwrap_or_else(|| panic!("no stylesheet declares {name}"));
            let (declared, _) = rest.split_once(';').expect("a declaration ends in ;");
            return length_px(css, declared, sheet);
        }
        if let Some(rem) = value.strip_suffix("rem") {
            return rem.trim().parse::<f64>().expect("a rem length is a number") * 16.0;
        }
        if let Some(px) = value.strip_suffix("px") {
            return px.trim().parse::<f64>().expect("a px length is a number");
        }
        if value == "0" {
            return 0.0;
        }
        panic!("`{value}` is not a length this guard can resolve");
    }
}
