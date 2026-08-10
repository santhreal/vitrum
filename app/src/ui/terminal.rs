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
    /// The class: `.rg-terminal`'s padding is not decoration. xterm's fit
    /// addon measures the element's CONTENT box, so the grid it proposes is
    /// `floor((paneHeight - padTop - padBottom) / cellHeight)` by
    /// `floor((paneWidth - padLeft - padRight) / cellWidth)`, and that grid
    /// goes out as a `Resize` on the wire. An agent redraws to it. Somebody
    /// nudging the transcript down from the titlebar therefore takes a row
    /// away from the agent, on some window heights and not others, and it is
    /// discovered as a wrapped line in a diff three days later — never as a
    /// padding change, because nothing connects the two.
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
    /// element, which also come out of the content box, and anything the fit
    /// addon does that is not this arithmetic.
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
                    let shipped = fit(pane_w, pane_h, cell_w, cell_h, left + right, top + bottom);
                    let before = fit(pane_w, pane_h, cell_w, cell_h, base_x, base_y);
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

    /// The grid xterm's fit addon proposes, in cells.
    ///
    /// `pad_x` and `pad_y` are AXIS SUMS and not four sides, because that is
    /// the whole of what the arithmetic can see; taking four would invite a
    /// caller to believe the distribution matters.
    fn fit(pane_w: f64, pane_h: f64, cell_w: f64, cell_h: f64, pad_x: f64, pad_y: f64) -> (u32, u32) {
        let cols = ((pane_w - pad_x) / cell_w).floor().max(2.0) as u32;
        let rows = ((pane_h - pad_y) / cell_h).floor().max(1.0) as u32;
        (cols, rows)
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
