//! What the first-run pane decides.
//!
//! Every case here builds its own [`Machine`], so nothing in this file can
//! pass or fail because of what happens to be installed on the machine
//! running it. That is the entire reason [`launch::agent_roster`] takes a
//! resolver: before it did, the only way to test detection was to assert that
//! it agreed with `on_path`, which is the same expression twice and cannot
//! catch a wrong answer.
//!
//! The variant space is enumerated from the shipped table at run time rather
//! than written out, so an agent added to `AGENTS` is covered here without
//! anyone remembering to add it.

use super::*;
use crate::launch::LaunchStore;

/// The place function every case uses: deterministic, and never a real path.
fn place(cwd: &str) -> String {
    cwd.trim_start_matches('/').to_string()
}

/// A machine on which exactly `installed` resolve.
fn machine(installed: &[&str]) -> Machine {
    Machine {
        roster: launch::agent_roster(|c| installed.contains(&c)),
        last: None,
        cwd: "/src/scratch".to_string(),
        home: "/home/mk".to_string(),
    }
}

/// One remembered launch.
fn last(command: &str, args: &[&str], cwd: &str) -> RecentEntry {
    RecentEntry {
        command: command.to_string(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
        cwd: cwd.to_string(),
        last_used_ms: 1_000,
        icon: None,
    }
}

/// Every agent vitrum ships knowledge of, read out of the table itself.
fn every() -> Vec<AgentAvailability> {
    launch::agent_roster(|_| true)
}

// ---- the resolver ---------------------------------------------------------

/// The roster is a pure function of the resolver: same length either way,
/// table order preserved, and `installed` is exactly what the resolver said.
///
/// This is the contract that makes every other case in this file honest. If
/// the roster ever consulted `PATH` behind the resolver's back, the all-false
/// case below would still report something installed on a developer machine.
#[test]
fn the_roster_is_exactly_what_the_resolver_says() {
    let all = launch::agent_roster(|_| true);
    let none = launch::agent_roster(|_| false);
    assert_eq!(all.len(), none.len(), "the table changed size under a resolver");
    assert!(!all.is_empty(), "vitrum ships no agent names at all");
    for (yes, no) in all.iter().zip(none.iter()) {
        assert_eq!(yes.command, no.command, "table order moved");
        assert_eq!(yes.label, no.label);
        assert!(yes.installed);
        assert!(!no.installed);
    }

    // A resolver that answers for one name answers for exactly one entry.
    for a in &all {
        let one = launch::agent_roster(|c| c == a.command);
        let on: Vec<&str> = one
            .iter()
            .filter(|e| e.installed)
            .map(|e| e.command)
            .collect();
        assert_eq!(on, vec![a.command], "resolver leaked onto another entry");
    }
}

/// `detected_in` is the installed half, in table order, and nothing else.
/// The launcher's suggestion list is built from it, so a missing agent
/// leaking through here is a row that fails at spawn.
#[test]
fn only_installed_entries_become_suggestions() {
    let all = every();
    let first = all[0].command;
    let roster = launch::agent_roster(|c| c == first);
    let got = launch::detected_in(&roster);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].command, first);
    assert!(launch::detected_in(&launch::agent_roster(|_| false)).is_empty());
    assert_eq!(
        launch::detected_in(&launch::agent_roster(|_| true)).len(),
        all.len()
    );
}

// ---- what the pane offers -------------------------------------------------

/// Missing agents are named, not hidden. A first-run operator has to be able
/// to see that vitrum knows about Codex before they can act on the fact that
/// they have not installed it; an empty list under a heading that promises
/// agents tells them nothing and looks broken.
#[test]
fn every_known_agent_is_named_whether_or_not_it_is_here() {
    let all = every();
    let installed = all[0].command;
    let got = first_run(&machine(&[installed]), "/src/vitrum", place);

    assert_eq!(got.offers.len(), all.len(), "an agent was hidden");
    for (offer, known) in got.offers.iter().zip(all.iter()) {
        assert_eq!(offer.command, known.command, "table order moved");
        assert_eq!(offer.label, known.label);
    }
    let here = &got.offers[0];
    assert!(here.installed);
    assert_eq!(here.note, installed, "an installed row must name its command");
    assert!(here.primary, "the control's own agent must be marked");
    for missing in got.offers.iter().skip(1) {
        assert!(!missing.installed);
        assert!(!missing.primary, "a missing agent was marked as the primary");
        assert_eq!(missing.note, "not installed");
    }
}

/// The agent the control already fires is marked in the list, not offered a
/// second time. An action appears exactly once per state; a row that starts
/// the same agent in the same directory as the button above it is the screen
/// repeating itself, which is the defect the empty pane was rebuilt to remove.
#[test]
fn the_promoted_agent_is_marked_rather_than_offered_twice() {
    let all = every();
    let names: Vec<&str> = all.iter().map(|a| a.command).collect();
    let got = first_run(&machine(&names), "/src/vitrum", place);

    let marked: Vec<&str> = got
        .offers
        .iter()
        .filter(|o| o.primary)
        .map(|o| o.command)
        .collect();
    assert_eq!(
        marked,
        vec![all[0].command],
        "exactly one row may be the control's own"
    );

    // A remembered command vitrum does not know promotes nothing, so every
    // installed row stays takeable rather than one being silently marked.
    let mut m = machine(&names);
    m.last = Some(last("/opt/agents/houdini", &[], "/src/vitrum"));
    let got = first_run(&m, "/src/vitrum", place);
    assert!(
        got.offers.iter().all(|o| !o.primary),
        "an unrelated row was marked as the control's own"
    );

    // With nothing installed there is nothing to mark, whatever the control
    // is doing.
    let got = first_run(&machine(&[]), "/src/vitrum", place);
    assert!(got.offers.iter().all(|o| !o.primary));
}

/// The offer list may never contain a shell. vitrum manages coding agents,
/// and a first-run screen that offers `bash` argues it is a terminal
/// multiplexer; see `AGENTS.md`, "Demos show agents, not shell output". The
/// gate reads the shipped table at run time, so a shell added to `AGENTS`
/// turns this red rather than shipping on the first screen a new operator
/// sees.
#[test]
fn no_offer_is_a_shell() {
    use vitrum_model::AgentKind;

    let got = first_run(&machine(&[]), "/src/vitrum", place);
    assert!(!got.offers.is_empty());
    for o in &got.offers {
        assert_ne!(
            AgentKind::of(o.command),
            AgentKind::Shell,
            "the first-run pane offered a shell: {}",
            o.command
        );
        assert_ne!(AgentKind::of(o.label), AgentKind::Shell, "{}", o.label);
    }
}

// ---- the one action -------------------------------------------------------

/// With nothing remembered, the control is aimed at the first agent this
/// machine actually has, in the directory the window is pointing at, and it
/// says so. Every shipped agent is exercised as that one agent, so the rule
/// cannot be right for the entry somebody had in mind and wrong for its
/// siblings.
#[test]
fn a_fresh_profile_starts_the_detected_agent_here() {
    for a in every() {
        let got = first_run(&machine(&[a.command]), "/src/vitrum", place);
        let start = got.start.expect("an installed agent was not offered");
        assert_eq!(start.line, a.command);
        assert_eq!(start.cwd, "/src/vitrum");
        assert_eq!(start.place, "src/vitrum");
        assert_eq!(start.word, a.label);
        assert_eq!(start.label, format!("Start {} in src/vitrum", a.label));
        assert!(!start.remembered);
        assert_eq!(
            got.caption.as_deref(),
            Some(
                format!(
                    "{} is on this machine. Anything else is one key away.",
                    a.label
                )
                .as_str()
            )
        );
        assert!(got.nothing.is_none());
    }
}

/// Table order decides which of several installed agents is promoted, and it
/// is stable: the same machine offers the same agent every launch, so the
/// control is something an operator can build a habit around.
#[test]
fn the_first_installed_agent_in_table_order_wins() {
    let all = every();
    if all.len() < 2 {
        return;
    }
    let names: Vec<&str> = all.iter().map(|a| a.command).collect();
    let got = first_run(&machine(&names), "/src/vitrum", place);
    assert_eq!(got.start.expect("nothing offered").line, all[0].command);

    // Drop the head and the next one in table order takes over, rather than
    // whichever entry the iteration order happened to reach first.
    let tail = first_run(&machine(&names[1..]), "/src/vitrum", place);
    assert_eq!(tail.start.expect("nothing offered").line, all[1].command);
}

/// The second launch is the first thing under the pointer. The remembered
/// pair is the agent AND the project, so an operator who was working in one
/// checkout comes back to that checkout, not to wherever this window happens
/// to point.
#[test]
fn the_remembered_pair_beats_detection() {
    let all = every();
    let promoted = all[0].command;
    let mut m = machine(&[promoted, all[all.len() - 1].command]);
    let remembered = all[all.len() - 1];
    m.last = Some(last(remembered.command, &["--resume"], "/src/other"));

    let got = first_run(&m, "/src/vitrum", place);
    let start = got.start.expect("nothing offered");
    assert_eq!(start.line, format!("{} --resume", remembered.command));
    assert_eq!(start.cwd, "/src/other", "the remembered project was dropped");
    assert_eq!(start.place, "src/other");
    assert_eq!(start.word, remembered.label, "the flag reached the control");
    assert_eq!(
        start.label,
        format!("Start {} in src/other", remembered.label)
    );
    assert!(start.remembered);
    assert_eq!(got.caption.as_deref(), Some("Where you left off."));
}

/// A remembered agent that has since been uninstalled is not offered. It
/// would spawn-fail three seconds after the click, which is the exact failure
/// the roster exists to prevent, and the machine has a working agent to offer
/// instead.
#[test]
fn a_remembered_agent_that_is_gone_falls_back_to_one_that_is_here() {
    let all = every();
    if all.len() < 2 {
        return;
    }
    let present = all[0];
    let gone = all[1];
    let mut m = machine(&[present.command]);
    m.last = Some(last(gone.command, &[], "/src/other"));

    let start = first_run(&m, "/src/vitrum", place)
        .start
        .expect("nothing offered");
    assert_eq!(start.line, present.command);
    assert_eq!(start.cwd, "/src/vitrum", "a dead memory kept its directory");
    assert!(!start.remembered);
}

/// A remembered command vitrum does not ship knowledge of is taken at face
/// value. The roster has no opinion on it, this operator really ran it, and
/// refusing it would make the surface useless to anyone running an agent that
/// is not on the list — which the launcher explicitly supports.
#[test]
fn a_remembered_command_outside_the_table_is_still_offered() {
    let mut m = machine(&[]);
    m.last = Some(last("/opt/agents/houdini", &[], "/src/vitrum"));

    let got = first_run(&m, "/src/other", place);
    let start = got.start.expect("a real past launch was refused");
    assert_eq!(start.line, "/opt/agents/houdini");
    assert_eq!(start.cwd, "/src/vitrum");
    assert_eq!(start.word, "houdini", "the whole path reached the control");
    assert!(start.remembered);
    assert!(got.nothing.is_none());
}

/// A remembered launch with no directory recorded runs where the window is
/// pointing, rather than nowhere.
#[test]
fn a_remembered_launch_without_a_directory_runs_here() {
    let all = every();
    let mut m = machine(&[all[0].command]);
    m.last = Some(last(all[0].command, &[], "   "));

    let start = first_run(&m, "/src/vitrum", place)
        .start
        .expect("nothing offered");
    assert_eq!(start.cwd, "/src/vitrum");
    assert!(start.remembered);
}

/// With no directory at all to name, the control still reads as a sentence
/// rather than as "Start Codex in ".
#[test]
fn a_control_with_nowhere_to_point_drops_the_place() {
    let all = every();
    let start = first_run(&machine(&[all[0].command]), "  ", place)
        .start
        .expect("nothing offered");
    assert_eq!(start.place, "");
    assert_eq!(start.label, format!("Start {}", all[0].label));
}

// ---- the empty machine ----------------------------------------------------

/// A machine with none of them and a profile with no history gets no control
/// and one honest sentence naming everything vitrum looked for. Inventing a
/// button here would be a button that cannot work, and offering the login
/// shell instead is the category error this product is trying not to make.
#[test]
fn nothing_installed_and_nothing_remembered_says_what_was_looked_for() {
    let got = first_run(&machine(&[]), "/src/vitrum", place);
    assert!(got.start.is_none(), "an unrunnable control was offered");
    assert!(got.caption.is_none());

    let said = got.nothing.expect("no explanation for an empty machine");
    for a in every() {
        assert!(
            said.contains(a.command),
            "{} was looked for and never named: {said}",
            a.command
        );
    }
    assert!(said.contains("Install one"));
}

/// The headline and the sentence under it are always present. They are what
/// makes this a product rather than a blank pane, and they do not depend on
/// any reading of the machine.
#[test]
fn the_pane_always_says_what_the_product_is() {
    for m in [machine(&[]), machine(&[every()[0].command])] {
        let got = first_run(&m, "/src/vitrum", place);
        assert_eq!(got.headline, "Run your coding agents here.");
        assert!(got.blurb.contains("closing this window does not stop them"));
        assert!(got.blurb.contains("sidebar"));
    }
}

// ---- what a real profile carries ------------------------------------------

/// The memory this surface reads is the one the launcher already writes.
/// `recents` is keyed on command, arguments and directory together and its
/// head is the last launch, so "remember the last project and agent" needs no
/// second copy of the same fact — and this pins that, because a future change
/// that stops writing `recents` would silently make every second launch a
/// fresh guess again.
#[test]
fn the_last_launch_is_the_head_of_recents() {
    let mut store = LaunchStore::default();
    launch::remember(&mut store, "codex", &[], "/src/vitrum", 1_000);
    launch::remember(&mut store, "claude", &["--resume".into()], "/src/other", 2_000);

    let head = store.recents.first().cloned().expect("no recent recorded");
    assert_eq!(head.command, "claude");
    assert_eq!(head.args, vec!["--resume".to_string()]);
    assert_eq!(head.cwd, "/src/other");

    let mut m = machine(&["claude", "codex"]);
    m.last = Some(head);
    let start = first_run(&m, "/src/elsewhere", place)
        .start
        .expect("nothing offered");
    assert_eq!(start.line, "claude --resume");
    assert_eq!(start.cwd, "/src/other");
}
