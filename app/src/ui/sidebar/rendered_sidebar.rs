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
    /// What the update path is saying, so the panel's restart band can be
    /// put on screen and taken off it.
    standing: crate::update::Standing,
}

/// The signal has to be created INSIDE the component: `Signal::new` needs a
/// live Dioxus runtime and panics when a test builds one up front.
#[component]
fn Harness(props: HarnessProps) -> Element {
    let state = use_signal(|| props.initial.clone());
    let update_standing = use_signal(|| props.standing.clone());
    rsx! {
        Sidebar {
            state,
            clock: TimeFormat::new(vitrum_fmt::Timestamp::from_millis(NOW as i64), 0),
            home: "/home/u".to_string(),
            server: "127.0.0.1:7717",
            update_standing,
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
            on_restart: move |()| {},
        }
    }
}

fn render(st: UiState) -> String {
    render_standing(st, crate::update::Standing::Current)
}

fn render_standing(st: UiState, standing: crate::update::Standing) -> String {
    let mut dom = VirtualDom::new_with_props(
        Harness,
        HarnessProps {
            initial: st,
            standing,
        },
    );
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
/// With no sessions at all the surface still has to render and say why it is
/// empty, because "nothing has run yet" and "the panel is broken" look
/// identical otherwise, and that is what the operator saw.
///
/// So the assertion is on the NAMED element and on its words, not merely on
/// the panel existing. An empty state whose element ships with no prose in it
/// is a blank column with an extra div, which is the same picture the defect
/// produced. It also asserts the list really is empty rather than hidden:
/// zero rows AND an explanation, not one of the two.
///
/// The mutations this catches: dropping the `groups.is_empty()` arm, so the
/// panel renders a bare scroller; emitting `.rg-sidebar__empty` with no
/// `.rg-empty__hint` inside it; and an empty state that draws alongside
/// session rows rather than in place of them.
///
/// What it does NOT catch: the wording, which is `inbox::Empty`'s and is
/// asserted there, or whether the element is visible after the cascade.
#[test]
fn an_empty_panel_still_renders_and_explains_itself() {
    let html = render(state_with(0));
    assert!(
        html.contains("rg-sidebar"),
        "the panel did not render at all with no sessions: {html}"
    );
    assert!(
        html.contains("rg-sidebar__empty"),
        "an empty panel is an empty list, which is what a broken one looks \
         like: {html}"
    );
    let hint = html
        .split_once("class=\"rg-empty__hint\"")
        .and_then(|(_, rest)| rest.split_once('>'))
        .and_then(|(_, rest)| rest.split_once('<'))
        .map(|(text, _)| text.trim())
        .unwrap_or_default();
    assert!(
        !hint.is_empty(),
        "the empty state rendered with no words in it, so the panel still \
         says nothing about why it is blank: {html}"
    );
    assert!(
        !html.contains("rg-session__title"),
        "a panel with no sessions drew a session row: {html}"
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

/// WHY: the panel drew nothing across three quarters of its own height.
///
/// The defect class: a scroller whose content does not reach the bottom, and
/// a product whose widest column is therefore mostly void. Measured, three
/// sessions filled 190px of a 764px scroller. The fix gives the remainder to
/// a region that starts a session, which is the only thing that belongs
/// there.
///
/// This asserts the SPLIT, because that is where the bug can come back. The
/// floor and the empty state answer the same question — "why is this panel
/// blank" — and exactly one of them is right at any moment. Both on screen
/// is two affordances for one action stacked in a column with nothing else
/// in it; neither is a panel that still says nothing.
///
/// What it does NOT catch: the geometry. Whether the region actually takes
/// the free space is `flex: 1 1 auto` in the sheet and is asserted there.
#[test]
fn the_empty_state_and_the_floor_are_never_both_on_screen() {
    let bare = render(state_with(0));
    assert!(
        bare.contains("rg-sidebar__empty"),
        "a window with no sessions drew no empty state at all: {bare}"
    );
    assert!(
        !bare.contains("rg-sidebar__floor"),
        "the floor stacked itself under the empty state, so one blank panel \
         offers the same action twice: {bare}"
    );

    let full = render(state_with(1));
    assert!(
        full.contains("rg-sidebar__floor"),
        "one session, and the rest of the panel is void again: {full}"
    );
    assert!(
        full.contains("rg-sidebar__floor-label"),
        "the floor rendered with no words in it, which is a dashed box: {full}"
    );
    assert!(
        !full.contains("rg-sidebar__empty"),
        "a panel with a session in it still says it is empty: {full}"
    );
}

/// WHY: the setting is cosmetic BY CONTRACT, and a cosmetic setting is one
/// mistaken `if` away from being a kill switch for updates.
///
/// `Settings::show_restart_to_update` decides whether the panel draws the
/// restart band. It must decide nothing else. The failure this closes is the
/// obvious one to write by accident: an operator who turns off a piece of
/// chrome, and silently stops receiving updates for it.
///
/// Both halves are asserted. The affordance appears for a STAGED build and
/// disappears when the setting is off — and the update path's own reader
/// count is checked at the source, so a second `show_restart_to_update`
/// appearing anywhere in the update module fails here rather than shipping.
///
/// What it does NOT catch: a reader added under a different name, or one in
/// a crate below `app`.
#[test]
fn the_setting_hides_the_restart_offer_without_disabling_updates() {
    let staged = crate::update::Standing::Staged {
        version: semver::Version::new(0, 1, 2),
    };

    let mut on = state_with(1);
    on.daemon.settings.show_restart_to_update = true;
    let shown = render_standing(on, staged.clone());
    assert!(
        shown.contains("rg-sidebar__restart"),
        "a staged build is on disk and the panel never offers the restart \
         that would run it: {shown}"
    );
    assert!(
        shown.contains("0.1.2"),
        "the offer never names the version waiting: {shown}"
    );

    let mut off = state_with(1);
    off.daemon.settings.show_restart_to_update = false;
    let hidden = render_standing(off, staged.clone());
    assert!(
        !hidden.contains("rg-sidebar__restart"),
        "the setting is off and the band is still on screen: {hidden}"
    );

    // Available is not staged: nothing is on disk to restart into, so the
    // band must not appear even with the setting on. Otherwise the offer
    // restarts the window and changes nothing.
    let merely = render_standing(
        state_with(1),
        crate::update::Standing::Available {
            version: semver::Version::new(0, 1, 2),
        },
    );
    assert!(
        !merely.contains("rg-sidebar__restart"),
        "an update that is only AVAILABLE offered a restart into a build \
         that is not there: {merely}"
    );

    // The contract half. `restart_offer` is the setting's only reader, so
    // the check, the download and the on-start swap cannot consult it.
    let update_src = include_str!("../../update.rs");
    let readers: Vec<&str> = update_src
        .lines()
        .filter(|line| line.contains("show_restart_to_update"))
        .filter(|line| !line.trim_start().starts_with("///"))
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    assert!(
        readers.is_empty(),
        "the update path reads the display setting directly, so hiding the \
         affordance can now disable updating: {readers:?}"
    );
    for gate in ["fn apply_on_start", "fn stage", "fn check"] {
        if let Some((_, after)) = update_src.split_once(gate) {
            let body = after.split_once("\n}\n").map_or(after, |(b, _)| b);
            assert!(
                !body.contains("show_restart_to_update"),
                "`{gate}` consults the display setting"
            );
        }
    }
}


/// WHY: a reorder painted an opaque rectangle across the list.
///
/// `title` is not a request for a tooltip, it is a request for a WINDOW. The
/// engine hands it to the platform, which paints an override-redirect surface
/// anchored to the POINTER, in the desktop's tooltip colours, that no rule in
/// this stylesheet can reach. Reorder the list under a stationary cursor —
/// which is what happens whenever a bucket is pinned to the top or an agent
/// changes state — and that surface stays exactly where it was, over rows it
/// no longer describes, for the whole of the reorder.
///
/// `SessionRow` was fixed for this and the rest of the list was not, so the
/// rectangle survived on the elements most likely to be under the cursor when
/// a bucket moves: the project header, its rollup chips, the shelf heads and
/// the two "show the rest" buttons. All of them now carry
/// `.rg-project__tip`, a child of the element it describes, which the same
/// layout that moves the element moves with it.
///
/// Asserted twice, because neither half is enough on its own. The rendered
/// half proves the markup that actually reaches the webview is clean, and the
/// source half covers the branches this fixture cannot put on screen at once
/// — it reads the whole list region of `sidebar.rs` and forbids the attribute
/// outright, so a new control added inside the list with a `title` on it goes
/// red here rather than shipping the defect back.
///
/// What it does NOT catch: chrome OUTSIDE the list. The toolbar, the floor
/// button and the footer keep their `title` and should: none of them moves.
#[test]
fn nothing_in_the_reorderable_list_asks_the_platform_for_a_tooltip() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum"), project(2, "atlas")];
    let mut sessions = Vec::new();
    for i in 0..3 {
        sessions.push(
            row(10 + i)
                .project(1)
                .command("claude")
                .title(&format!("session {i}"))
                .waiting(Some(false))
                .build(),
        );
    }
    // A shelf under the Active band, so both shelf heads render: the static
    // Active caption and the Snoozed button.
    sessions.push(
        row(20)
            .project(1)
            .command("codex")
            .title("parked")
            .snooze(NOW - HOUR, NOW + HOUR)
            .build(),
    );
    for i in 0..3 {
        sessions.push(
            row(30 + i)
                .project(2)
                .command("gemini")
                .title(&format!("atlas {i}"))
                .waiting(Some(false))
                .build(),
        );
    }
    st.daemon.sessions = sessions;

    // Every bucket but the first is collapsed, which is the only state that
    // draws the rollup chips. The first stays open so its shelves render.
    let clock = TimeFormat::new(vitrum_fmt::Timestamp::from_millis(NOW as i64), 0);
    let shut: Vec<GroupKey> = st
        .tree(inbox::model_clock(clock))
        .iter()
        .skip(1)
        .map(|g| g.key)
        .collect();
    assert!(!shut.is_empty(), "the fixture must draw more than one bucket");
    for key in shut {
        st.window.collapsed.insert(key);
    }

    let html = render(st);
    for present in [
        "rg-project__header",
        "rg-rollup__chip",
        "rg-project__section-head",
        "rg-project__tip",
    ] {
        assert!(
            html.contains(present),
            "the fixture never drew {present}, so this proves nothing: {html}"
        );
    }

    // The list is everything between the scroller and the floor button. The
    // floor is the one control inside the scroller that does not move, and it
    // keeps its `title`.
    let from = html
        .find("rg-sidebar__body")
        .expect("the scroller is the list");
    let to = html
        .find("rg-sidebar__floor")
        .expect("the floor closes the scroller");
    let list = &html[from..to];
    assert!(
        !list.contains("title=\""),
        "something in the reorderable list still asks the platform for a \
         tooltip, which paints an opaque rectangle over the list for the \
         whole of a reorder: {list}"
    );

    // The source half: every branch, including the ones this fixture cannot
    // render at the same time.
    let src = include_str!("../sidebar.rs");
    let code = src.split_once("#[cfg(test)]").map_or(src, |(a, _)| a);
    let open = code
        .find("class: \"rg-sidebar__body\"")
        .expect("the scroller opens the list");
    let close = code
        .find("class: \"rg-sidebar__floor\"")
        .expect("the floor closes the list");
    let region = &code[open..close];
    assert!(
        !region.contains("title:"),
        "sidebar.rs puts a `title` attribute inside the list region; use a \
         .rg-project__tip child instead, so a reorder moves it"
    );

    // And the panel it moved to is really styled, rather than being an
    // always-visible paragraph stapled under every header.
    let css = include_str!("../../../assets/sidebar.css");
    assert!(
        css.contains(".rg-project__tip"),
        "nothing styles the replacement, so the strings render inline"
    );
    assert!(
        css.contains(".rg-project__header:hover > .rg-project__tip"),
        "the replacement never appears, so the fix deleted the tooltip \
         rather than moving it"
    );
}
