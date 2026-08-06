//! Two things the profile has to survive: a file written before icons and
//! recents existed, and an operator who runs the same command all day.
//!
//! Every test here is pure. The writing half of the store touches the real
//! profile directory, so exercising it would edit the profile of whoever ran
//! the suite; `remember` takes the store by reference and takes the clock as
//! an argument, which is where all the behaviour worth proving lives.

use super::*;

const NOW: u64 = 1_700_000_000_000;
const MINUTE: u64 = 60_000;

fn run(store: &mut LaunchStore, line: &str, cwd: &str, at: u64) {
    let (command, args) = split_command(line).expect("a fixture line has a program");
    remember(store, &command, &args, cwd, at);
}

/// A profile written by a build without icons or recents must load whole. A
/// missing field that failed the parse would default the entire document and
/// silently delete every preset the operator saved.
#[test]
fn a_file_from_an_older_build_loads_with_no_icon_and_no_recents() {
    let doc = r#"{
        "version": 1,
        "presets": [{"id": 7, "label": "Shell", "command": "bash"}],
        "history": [{"command": "claude", "count": 3, "last_used_ms": 12}],
        "last_cwd": "/src/vitrum"
    }"#;
    let got = parse_launch_store(doc);
    assert_eq!(got.presets.len(), 1, "the preset was dropped: {got:?}");
    assert_eq!(got.presets[0].label, "Shell");
    assert_eq!(got.presets[0].icon, None);
    assert_eq!(got.history[0].icon, None);
    assert!(got.recents.is_empty());
    assert_eq!(got.last_cwd, "/src/vitrum");
}

/// A slug from a newer build, or one somebody typed by hand, must not cost
/// the operator the preset it is attached to. The store keeps the string and
/// the drawing side resolves it to the command's default.
#[test]
fn an_unknown_icon_slug_survives_the_load_and_draws_the_default() {
    let doc = r#"{"presets":[{"id":1,"label":"x","command":"git","icon":"no-such-icon"}]}"#;
    let got = parse_launch_store(doc);
    assert_eq!(got.presets.len(), 1, "the preset was dropped: {got:?}");
    assert_eq!(got.presets[0].icon.as_deref(), Some("no-such-icon"));
    assert_eq!(
        crate::ui::icons::resolve(got.presets[0].icon.as_deref(), "git").slug,
        "branch"
    );
}

/// A chosen icon must round trip, or the picker forgets on the next launch.
#[test]
fn a_chosen_icon_round_trips_through_the_file() {
    let mut store = LaunchStore::default();
    store.presets.push(SavedPreset {
        id: 1,
        label: "Plan".into(),
        command: "claude".into(),
        icon: Some("flask".into()),
        ..SavedPreset::default()
    });
    run(&mut store, "claude", "/src/vitrum", NOW);
    store.recents[0].icon = Some("bolt".into());

    let back = parse_launch_store(&encode_launch_store(&store));
    assert_eq!(back.presets[0].icon.as_deref(), Some("flask"));
    assert_eq!(back.recents[0].icon.as_deref(), Some("bolt"));
    assert_eq!(back, store);
}

/// Restarting the same agent in the same repo used to be one row per run,
/// which pushed everything else off the list within a morning.
#[test]
fn re_running_a_command_in_the_same_place_keeps_one_row() {
    let mut store = LaunchStore::default();
    for i in 0..5 {
        run(&mut store, "claude", "/src/vitrum", NOW + i * MINUTE);
    }
    assert_eq!(store.recents.len(), 1);
    assert_eq!(store.recents[0].last_used_ms, NOW + 4 * MINUTE);
}

/// The directory is part of the row's identity. The same agent in two
/// checkouts is two different things to go back to, and merging them would
/// send the operator to the wrong repo.
#[test]
fn the_same_command_in_two_places_is_two_rows() {
    let mut store = LaunchStore::default();
    run(&mut store, "claude", "/src/vitrum", NOW);
    run(&mut store, "claude", "/src/other", NOW + MINUTE);
    assert_eq!(store.recents.len(), 2);
    assert_eq!(store.recents[0].cwd, "/src/other");
    assert_eq!(store.recents[1].cwd, "/src/vitrum");
}

/// Arguments are part of the row too: `claude` and `claude
/// --permission-mode plan` are two different launches, and offering one when
/// the operator meant the other starts an agent with the wrong permissions.
#[test]
fn arguments_distinguish_two_rows() {
    let mut store = LaunchStore::default();
    run(&mut store, "claude", "/src/vitrum", NOW);
    run(
        &mut store,
        "claude --permission-mode plan",
        "/src/vitrum",
        NOW + MINUTE,
    );
    assert_eq!(store.recents.len(), 2);
    assert_eq!(
        recent_line(&store.recents[0]),
        "claude --permission-mode plan"
    );
}

/// A trailing separator is the spelling the launcher's directory completion
/// produces on every accepted Tab. Storing both spellings would put one
/// directory in the list twice.
#[test]
fn one_directory_spelled_two_ways_is_still_one_row() {
    let mut store = LaunchStore::default();
    run(&mut store, "claude", "/src/vitrum", NOW);
    run(&mut store, "claude", "/src/vitrum/", NOW + MINUTE);
    assert_eq!(store.recents.len(), 1);
    assert_eq!(store.recents[0].cwd, "/src/vitrum");
}

/// An uncapped list grows until the file is the size of the shell history,
/// and every read of it is a read of all of it.
#[test]
fn the_list_is_capped_and_drops_the_oldest() {
    let mut store = LaunchStore::default();
    for i in 0..(RECENTS_MAX as u64 + 4) {
        run(&mut store, &format!("cmd{i}"), "/src/vitrum", NOW + i);
    }
    assert_eq!(store.recents.len(), RECENTS_MAX);
    assert_eq!(store.recents[0].command, format!("cmd{}", RECENTS_MAX + 3));
    assert!(
        !store.recents.iter().any(|e| e.command == "cmd0"),
        "the oldest row survived the cap"
    );
}

/// Re-running something from the bottom of the list must move it to the top.
/// A list that only ever appends is a list where the thing you just did is
/// wherever it was last week.
#[test]
fn re_running_an_older_row_moves_it_to_the_front() {
    let mut store = LaunchStore::default();
    run(&mut store, "claude", "/src/vitrum", NOW);
    run(&mut store, "bash", "/src/vitrum", NOW + MINUTE);
    run(&mut store, "git status", "/src/vitrum", NOW + 2 * MINUTE);
    assert_eq!(recent_line(&store.recents[2]), "claude");

    run(&mut store, "claude", "/src/vitrum", NOW + 3 * MINUTE);
    let lines: Vec<String> = store.recents.iter().map(recent_line).collect();
    assert_eq!(lines, vec!["claude", "git status", "bash"]);
    assert_eq!(store.recents[0].last_used_ms, NOW + 3 * MINUTE);
}

/// The icon belongs to the command, not to the run. Losing it on the next
/// launch would make the picker look like a control that does not stick.
#[test]
fn bumping_a_row_keeps_the_icon_the_operator_chose() {
    let mut store = LaunchStore::default();
    run(&mut store, "claude", "/src/vitrum", NOW);
    store.recents[0].icon = Some("bolt".into());
    run(&mut store, "bash", "/src/vitrum", NOW + MINUTE);
    run(&mut store, "claude", "/src/vitrum", NOW + 2 * MINUTE);
    assert_eq!(store.recents[0].icon.as_deref(), Some("bolt"));
}

/// Recents and the ranked history are two different questions, and folding
/// one into the other would make the launcher's ranking depend on the cap.
#[test]
fn recording_a_launch_writes_both_lists() {
    let mut store = LaunchStore::default();
    run(&mut store, "claude", "/src/vitrum", NOW);
    run(&mut store, "claude", "/src/other", NOW + MINUTE);
    assert_eq!(store.history.len(), 1, "one command is one history entry");
    assert_eq!(store.history[0].count, 2);
    assert_eq!(store.recents.len(), 2, "two places are two recents");
    assert_eq!(store.last_cwd, "/src/other");
}
