use super::*;

use crate::launch::LaunchStore;
use crate::testkit::project;

const NOW: u64 = 1_700_000_000_000;
const MINUTE: u64 = 60_000;

fn store_after(runs: &[(&str, &str, u64)]) -> LaunchStore {
    let mut store = LaunchStore::default();
    for (line, cwd, at) in runs {
        let (command, args) = launch::split_command(line).expect("a fixture line has a program");
        launch::remember(&mut store, &command, &args, cwd, *at);
    }
    store
}

#[derive(Props, Clone, PartialEq)]
struct HarnessProps {
    entries: Vec<RecentEntry>,
}

/// The handler has to be built INSIDE a component: `EventHandler::new` needs
/// a live Dioxus runtime and panics when a test constructs the props up
/// front.
#[component]
fn Harness(props: HarnessProps) -> Element {
    rsx! {
        Recents {
            entries: props.entries.clone(),
            projects: vec![project(1, "vitrum")],
            home: "/home/u".to_string(),
            on_launch: move |_: Launch| {},
        }
    }
}

fn render(entries: Vec<RecentEntry>) -> String {
    let mut dom = VirtualDom::new_with_props(Harness, HarnessProps { entries });
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// The list is stored in draw order, so a component that re-sorted or
/// re-clocked it would disagree with the file on the next open.
#[test]
fn the_list_draws_newest_first_exactly_as_stored() {
    let store = store_after(&[
        ("claude", "/src/vitrum", NOW - 3 * MINUTE),
        ("bash", "/src/vitrum", NOW - 2 * MINUTE),
        ("git status", "/src/vitrum", NOW - MINUTE),
    ]);
    let html = render(launch::recents(&store).to_vec());
    let order: Vec<usize> = ["git status", "bash", "claude"]
        .iter()
        .map(|line| html.find(line).unwrap_or_else(|| panic!("{line} is not drawn: {html}")))
        .collect();
    assert!(
        order[0] < order[1] && order[1] < order[2],
        "the rows are not newest first: {html}"
    );
}

/// The place used to be the raw absolute path, which is a string nobody reads
/// and the reason the old dialog's first field looked like configuration.
#[test]
fn a_row_names_its_place_by_project_and_keeps_the_path_in_the_title() {
    let store = store_after(&[("claude", "/src/vitrum/app", NOW)]);
    let html = render(launch::recents(&store).to_vec());
    assert!(
        html.contains(">vitrum/app<"),
        "the place chip is not project-relative: {html}"
    );
    assert!(
        html.contains("title=\"/src/vitrum/app\""),
        "the absolute path is not on the chip's title: {html}"
    );
}

/// An entry whose icon slug this build does not have must still draw the
/// shape its command implies, not an empty box.
#[test]
fn an_unknown_icon_slug_still_draws_the_command_default() {
    let entry = RecentEntry {
        command: "git".into(),
        args: vec!["status".into()],
        cwd: "/src/vitrum".into(),
        last_used_ms: NOW,
        icon: Some("no-such-icon".into()),
    };
    let html = render(vec![entry]);
    let branch = crate::ui::icons::from_slug("branch").expect("branch is in the set");
    assert!(
        html.contains(branch.stroke),
        "an unknown slug drew no icon at all: {html}"
    );
}

/// An operator's chosen icon must win over the one the command implies, or
/// the picker is a control that does nothing.
#[test]
fn a_chosen_icon_overrides_the_command_default() {
    let entry = RecentEntry {
        command: "git".into(),
        args: vec!["status".into()],
        cwd: "/src/vitrum".into(),
        last_used_ms: NOW,
        icon: Some("flask".into()),
    };
    let html = render(vec![entry]);
    let flask = crate::ui::icons::from_slug("flask").expect("flask is in the set");
    let branch = crate::ui::icons::from_slug("branch").expect("branch is in the set");
    assert!(html.contains(flask.stroke), "the chosen icon is missing: {html}");
    assert!(
        !html.contains(branch.stroke),
        "the default icon was drawn over the chosen one: {html}"
    );
}

/// An empty list drew an empty `ul`, which on screen is a heading over
/// nothing and reads as a surface that failed to load.
#[test]
fn an_empty_list_says_so_instead_of_drawing_nothing() {
    let html = render(Vec::new());
    assert!(
        html.contains("Nothing started yet."),
        "an empty list drew no sentence: {html}"
    );
    assert!(
        !html.contains("rg-recents__list"),
        "an empty list still drew a listbox: {html}"
    );
}
