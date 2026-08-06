//! The WHOLE sidebar, rendered.
//!
//! The panel is the product. It had 1,926 tests and not one of them built it:
//! they asserted CSS substrings, pure-function returns and source text, so a
//! green suite sat beside a screenshot of a project header with no sessions
//! under it. Everything here starts from a `UiState` with real sessions in it
//! and looks at the HTML that would reach the webview.

use super::*;
use crate::testkit::{HOUR, NOW, project, row};

/// A window holding `n` running sessions in one project.
fn state_with(n: u64) -> UiState {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum")];
    st.daemon.sessions = (0..n)
        .map(|i| {
            row(10 + i)
                .project(1)
                .command("claude")
                .title(&format!("session {i}"))
                .waiting(Some(false))
                .created_at_ms(NOW - HOUR + i)
                .last_activity_ms(NOW - HOUR + i)
                .build()
        })
        .collect();
    st
}

#[derive(Props, Clone, PartialEq)]
struct HarnessProps {
    initial: UiState,
}

/// The signal has to be created INSIDE the component: `Signal::new` needs a
/// live Dioxus runtime and panics when a test builds one up front.
#[component]
fn Harness(props: HarnessProps) -> Element {
    let state = use_signal(|| props.initial.clone());
    rsx! {
        Sidebar {
            state,
            clock: TimeFormat::new(vitrum_fmt::Timestamp::from_millis(NOW as i64), 0),
            home: "/home/u".to_string(),
            server: "127.0.0.1:7717",
            on_select: move |_: (SessionId, Click)| {},
            on_close_session: move |_: SessionId| {},
            on_toggle_project: move |_: GroupKey| {},
            on_toggle_section: move |_: (GroupKey, Section)| {},
            on_toggle_preview: move |_: GroupKey| {},
            on_toggle_settled_tail: move |_: GroupKey| {},
            on_toggle_sidebar: move |()| {},
            on_retry: move |()| {},
            on_jump: move |()| {},
            on_new_session: move |_: Option<ProjectId>| {},
            on_launch_now: move |()| {},
            on_filter: move |_: String| {},
            on_menu: move |_: (f64, f64, SessionId)| {},
            on_resize_start: move |_: f64| {},
            on_resize_nudge: move |_: f64| {},
            on_settings: move |()| {},
        }
    }
}

fn render(st: UiState) -> String {
    let mut dom = VirtualDom::new_with_props(Harness, HarnessProps { initial: st });
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// Sessions must actually appear under their project.
///
/// This is the defect that was on screen: a header reading `vitrum` with
/// the session list empty beneath it. Every state-layer test passed while
/// it did, because they all asked "does the model hold three sessions"
/// and never "does the panel draw three rows".
#[test]
fn every_session_in_the_model_reaches_the_panel() {
    let html = render(state_with(3));
    for i in 0..3 {
        assert!(
            html.contains(&format!("session {i}")),
            "session {i} is in the model and not in the panel: {html}"
        );
    }
    assert_eq!(
        html.matches("class=\"rg-session__title\"").count(),
        3,
        "the panel drew a different number of rows than the model holds"
    );
}

/// A project header must never state the same fact twice.
///
/// It shipped reading `vitrum  (dot) 2  2`: the rollup chip said "2
/// running" and a plain total said "2", side by side, and the operator
/// asked what the second number meant. It meant nothing the first did
/// not. A bare total is never the best answer available: collapsed, the
/// rollup breaks it down by status; expanded, the rows are right there.
#[test]
fn a_project_header_never_prints_the_same_number_twice() {
    let html = render(state_with(2));
    assert!(
        !html.contains("rg-project__count"),
        "the header is back to printing a bare total beside the rollup: {html}"
    );
}

/// The panel must carry no tab strip and no second switcher.
///
/// The strip was a duplicate of this surface that cost a whole chrome band
/// above the terminal. Nothing in the sidebar may quietly grow one back.
#[test]
fn the_panel_holds_no_tab_strip_markup() {
    let html = render(state_with(4));
    for banned in ["rg-tabs", "rg-tab ", "rg-overflow"] {
        assert!(
            !html.contains(banned),
            "the panel emits `{banned}`, which is tab strip markup"
        );
    }
}

/// An empty panel must not be indistinguishable from a broken one.
///
/// With no sessions at all the surface still has to render and say why it
/// is empty, because "nothing has run yet" and "the panel is broken" look
/// identical otherwise, and that is what the operator saw.
#[test]
fn an_empty_panel_still_renders_and_explains_itself() {
    let html = render(state_with(0));
    assert!(
        html.contains("rg-sidebar"),
        "the panel did not render at all with no sessions: {html}"
    );
    assert!(
        html.contains("rg-empty") || html.contains("rg-project__empty"),
        "an empty panel says nothing about why it is empty: {html}"
    );
}

/// The agent mark reaches the panel, not just the row component.
///
/// `rendered_row` proves `SessionRow` draws it. This proves the sidebar
/// actually renders that component on the real path, which is the link
/// that was missing every other time this product shipped a dead surface.
#[test]
fn the_agent_mark_survives_the_whole_panel() {
    let html = render(state_with(2));
    assert_eq!(
        html.matches("class=\"rg-session__agent").count(),
        2,
        "the panel drew the wrong number of agent marks: {html}"
    );
    assert!(
        html.contains(AgentKind::Claude.mark().stroke),
        "a panel full of `claude` sessions draws no Claude mark: {html}"
    );
}
