use super::*;
use vitrum_model::SessionView;
use vitrum_proto::{Attention, SessionInfo, SessionStatus};

fn session(id: u64, cwd: &str, command: &str, branch: Option<&str>) -> SessionView {
    SessionView::new(SessionInfo {
        id: SessionId(id),
        project_id: ProjectId(1),
        title: format!("s{id}"),
        cwd: cwd.to_string(),
        command: command.to_string(),
        args: Vec::new(),
        status: SessionStatus::Running,
        created_at_ms: id,
        last_activity_ms: id,
        cols: 80,
        rows: 24,
        git_branch: branch.map(str::to_string),
        unread: false,
        attention: Attention::default(),
        hint: None,
    })
}

fn project(id: u64, name: &str, root: &str) -> ProjectInfo {
    ProjectInfo {
        id: ProjectId(id),
        name: name.to_string(),
        root: root.to_string(),
    }
}

/// A history the store could really contain.
///
/// The line is SPLIT, because `launch::remember` is handed a program and
/// its arguments separately and never a whole line. Storing `cargo test`
/// in `command` produced `"cargo test"`, quoted, out of
/// `launch::join_command`, which is the exact shape a hand-built fixture
/// gets wrong and a real profile never does.
fn store_with(history: &[(&str, u32)]) -> LaunchStore {
    LaunchStore {
        history: history
            .iter()
            .map(|(line, count)| {
                let (command, args) =
                    launch::split_command(line).expect("a fixture line has a program");
                launch::HistoryEntry {
                    command,
                    args,
                    count: *count,
                    last_used_ms: 1_000,
                    icon: None,
                }
            })
            .collect(),
        ..LaunchStore::default()
    }
}

fn agent(label: &'static str, command: &'static str) -> Detected {
    Detected { label, command }
}

/// The list must have rows before a key is pressed.
///
/// The defect this locks out is the whole reason the surface exists: the
/// old dialog opened on three empty-ish fields and made the operator type
/// before it would show them anything. A launcher whose first paint is an
/// empty box is the same failure with a different shape.
#[test]
fn the_list_is_populated_before_any_keystroke() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    let rows = intents(
        &st,
        &store_with(&[("claude", 7)]),
        &[agent("Codex", "codex")],
        "/bin/bash",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    let shown: Vec<&str> = rows.iter().map(Intent::text).collect();
    assert_eq!(shown, vec!["claude", "codex", "/bin/bash"]);
    assert_eq!(rows[0].place, "vitrum");
    assert_eq!(rows[0].band, Band::Recent);
    assert_eq!(rows[1].band, Band::Agent);
    assert_eq!(rows[2].band, Band::Shell);
}

/// PATH discovery lands late on purpose, and must never move the row the
/// highlight is already sitting on.
///
/// The bug: agents ranked above recents, so the list re-ordered under the
/// operator's hand a few milliseconds after it opened and Enter launched
/// something they never chose.
#[test]
fn agents_arriving_late_never_displace_the_top_row() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    let store = store_with(&[("claude", 7)]);
    let first = intents(&st, &store, &[], "", "/src/vitrum", "/home/u", 2_000);
    let later = intents(
        &st,
        &store,
        &[agent("Codex", "codex"), agent("Gemini CLI", "gemini")],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    assert_eq!(first[0].text(), "claude");
    assert_eq!(later[0].text(), "claude");
    assert_eq!(later.len(), 3);
}

/// A query must match on the directory alone.
///
/// The defect: a filter that only searched the command, so an operator who
/// remembered the place but not which agent was in it had no way to narrow
/// twenty rows.
#[test]
fn a_query_matches_on_the_directory_alone() {
    let mut st = UiState::default();
    st.daemon.projects = vec![
        project(1, "vitrum", "/src/vitrum"),
        project(2, "harness", "/src/harness"),
    ];
    st.daemon.sessions = vec![
        session(1, "/src/vitrum", "claude", None),
        session(2, "/src/harness", "claude", None),
    ];
    let rows = intents(
        &st,
        &LaunchStore::default(),
        &[],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    let hits = ranked(&rows, "harness");
    assert_eq!(hits.len(), 1);
    assert_eq!(rows[hits[0]].place, "harness");
    assert_eq!(rows[hits[0]].command, "claude");
}

/// A query must match on the branch alone.
///
/// Same defect, other field. `main` appears in no command and no path here,
/// so a filter that skipped the branch would return nothing.
#[test]
fn a_query_matches_on_the_branch_alone() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    st.daemon.sessions = vec![
        session(1, "/src/vitrum", "claude", Some("main")),
        session(2, "/src/vitrum/app", "codex", Some("wip/rewrite")),
    ];
    let rows = intents(
        &st,
        &LaunchStore::default(),
        &[],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    let hits = ranked(&rows, "main");
    assert_eq!(hits.len(), 1);
    assert_eq!(rows[hits[0]].branch.as_deref(), Some("main"));
    let wip = ranked(&rows, "rewrite");
    assert_eq!(wip.len(), 1);
    assert_eq!(rows[wip[0]].place, "vitrum/app");
}

/// Two terms narrow across two different fields at once.
///
/// The bug a single-term matcher has: `claude main` is treated as one
/// literal, matches nothing, and the operator concludes the search is
/// broken.
#[test]
fn two_terms_narrow_across_two_fields() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    st.daemon.sessions = vec![
        session(1, "/src/vitrum", "claude", Some("main")),
        session(2, "/src/vitrum", "codex", Some("main")),
    ];
    let rows = intents(
        &st,
        &LaunchStore::default(),
        &[],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    let hits = ranked(&rows, "codex main");
    assert_eq!(hits.len(), 1);
    assert_eq!(rows[hits[0]].command, "codex");
    assert!(ranked(&rows, "codex nosuchbranch").is_empty());
}

/// The number keys must address the rows as drawn, not the unfiltered
/// ranking behind them.
///
/// The defect: numbering the source list, so after typing a query Ctrl+2
/// launched a row that was no longer on screen.
#[test]
fn number_keys_map_to_the_visible_row_order() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    let rows = intents(
        &st,
        &store_with(&[("claude", 9), ("codex", 4), ("cargo test", 2)]),
        &[],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    let all: Vec<&str> = ranked(&rows, "").iter().map(|i| rows[*i].text()).collect();
    assert_eq!(all, vec!["claude", "codex", "cargo test"]);

    let narrowed = ranked(&rows, "c");
    let shown: Vec<&str> = narrowed.iter().map(|i| rows[*i].text()).collect();
    assert_eq!(shown, vec!["claude", "codex", "cargo test"]);

    let narrowed = ranked(&rows, "test");
    assert_eq!(narrowed.len(), 1);
    // Ctrl+1 on this list is row 0 of the FILTERED order.
    assert_eq!(rows[narrowed[0]].text(), "cargo test");
    assert_eq!(digit_of("Digit1"), Some(1));
    assert_eq!(digit_of("Digit9"), Some(9));
    assert_eq!(digit_of("Digit0"), None);
    assert_eq!(digit_of("KeyA"), None);
}

/// Never more rows than there are digits to address them with.
#[test]
fn the_list_never_draws_a_row_without_a_number() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    let history: Vec<(&str, u32)> = vec![
        ("a1", 1),
        ("a2", 2),
        ("a3", 3),
        ("a4", 4),
        ("a5", 5),
        ("a6", 6),
        ("a7", 7),
        ("a8", 8),
        ("a9", 9),
        ("a10", 10),
        ("a11", 11),
    ];
    let rows = intents(
        &st,
        &store_with(&history),
        &[],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    assert_eq!(rows.len(), 11);
    assert_eq!(ranked(&rows, "").len(), ROWS_MAX);
    assert_eq!(ROWS_MAX, 9);
}

/// A command that is not on this machine must be reported, not launched.
///
/// The defect: the old dialog painted a warning under the field and left
/// Launch fully armed, so one click sent a spawn the daemon could only
/// fail three seconds later. The first take now says the program's name;
/// only a second one runs it.
#[test]
fn an_unknown_command_is_reported_rather_than_launched() {
    let dir = if cfg!(windows) { "C:\\" } else { "/" };
    let ghost = Intent::new(
        "vitrum-no-such-command-9f3a --go".to_string(),
        dir.to_string(),
        "root".to_string(),
        None,
        Band::Typed,
        String::new(),
        None,
    );
    assert_eq!(
        attempt(&ghost, false),
        Attempt::Warn("vitrum-no-such-command-9f3a is not on this machine's PATH.".to_string())
    );
    match attempt(&ghost, true) {
        Attempt::Go(l) => {
            assert_eq!(l.command, "vitrum-no-such-command-9f3a");
            assert_eq!(l.args, vec!["--go".to_string()]);
        }
        other => panic!("armed take must go, got {other:?}"),
    }
}

/// A directory that does not exist can never be launched into, armed or
/// not. Arming is for "this might fail", never for "this cannot work".
#[test]
fn a_missing_directory_is_refused_even_when_armed() {
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let nowhere = Intent::new(
        shell.to_string(),
        "/vitrum-no-such-directory-9f3a".to_string(),
        "nope".to_string(),
        None,
        Band::Typed,
        String::new(),
        None,
    );
    assert_eq!(
        attempt(&nowhere, true),
        Attempt::Refuse(
            "/vitrum-no-such-directory-9f3a is not a directory on this machine.".to_string()
        )
    );
}

/// One click must launch, with no dialog, when the answer is known.
///
/// The defect this whole redesign exists for: every route to a new session
/// cost two clicks, because the first one only opened a form.
#[test]
fn the_primary_control_launches_the_last_command_here() {
    let dir = if cfg!(windows) { "C:\\" } else { "/" };
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "root", dir)];
    match primary_of(&st, &store_with(&[(shell, 12)]), dir, "/home/u", 2_000) {
        Primary::Ready(l) => {
            assert_eq!(l.command, shell);
            assert_eq!(l.cwd, launch::tidy_dir(dir));
            assert_eq!(l.title, None);
        }
        Primary::Choose(why) => panic!("should have launched, got {why:?}"),
    }
}

/// With no history the primary control must open the list and say why,
/// never guess an agent off PATH.
#[test]
fn with_no_history_the_primary_control_refuses_to_guess() {
    let dir = if cfg!(windows) { "C:\\" } else { "/" };
    let st = UiState::default();
    assert_eq!(
        primary_of(&st, &LaunchStore::default(), dir, "/home/u", 2_000),
        Primary::Choose("Nothing has been launched here yet.".to_string())
    );
}

/// The reason must name the binary, because "cannot launch" tells the
/// operator nothing they can act on.
#[test]
fn a_command_that_left_path_is_named_in_the_reason() {
    let dir = if cfg!(windows) { "C:\\" } else { "/" };
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "root", dir)];
    assert_eq!(
        primary_of(
            &st,
            &store_with(&[("vitrum-gone-9f3a", 3)]),
            dir,
            "/home/u",
            2_000
        ),
        Primary::Choose("vitrum-gone-9f3a is not on this machine's PATH.".to_string())
    );
}

/// The control wears the name of what it will start, and says "New
/// session" only when it is about to open the list instead.
#[test]
fn the_primary_control_is_labelled_with_what_it_will_do() {
    // The word is the program, never the path it was found at and never
    // the arguments: `+ /usr/local/bin/claude --permission-mode plan` does
    // not fit a 224px sidebar and names nothing the operator chose.
    assert_eq!(basename("/usr/local/bin/claude"), "claude");
    assert_eq!(basename("claude"), "claude");
    assert_eq!(basename("C:\\tools\\codex.exe"), "codex.exe");

    let store = store_with(&[("/usr/local/bin/claude --permission-mode plan", 4)]);
    assert_eq!(top_word(&store, 2_000).as_deref(), Some("claude"));
    assert_eq!(top_word(&LaunchStore::default(), 2_000), None);

    assert_eq!(go_label(Some("claude"), true), "+ claude");
    assert_eq!(go_label(Some("claude"), false), "+");
    assert_eq!(go_label(None, true), "New session");
    assert_eq!(go_label(None, false), "+");

    assert_eq!(
        go_tip(Some("claude"), "vitrum"),
        "Start claude in vitrum. Ctrl+Shift+N to choose something else."
    );
    assert_eq!(
        go_tip(Some("claude"), ""),
        "Start claude. Ctrl+Shift+N to choose something else."
    );
    assert_eq!(
        go_tip(None, "vitrum"),
        "Choose what to start (Ctrl+Shift+N)."
    );
}

/// With no history but a saved preset the control wears the preset's own
/// name, because a preset is a choice the operator already made rather
/// than a guess off PATH.
#[test]
fn a_saved_preset_names_the_control_when_there_is_no_history() {
    let store = LaunchStore {
        presets: vec![SavedPreset {
            id: 1,
            label: "Plan mode".into(),
            command: "claude".into(),
            args: vec!["--permission-mode".into(), "plan".into()],
            cwd: None,
            shortcut: None,
            icon: None,
        }],
        ..LaunchStore::default()
    };
    assert_eq!(top_word(&store, 2_000).as_deref(), Some("Plan mode"));
}

/// The place is project-relative and the absolute path never reaches the
/// row's text.
///
/// The defect: a 47-character absolute path sitting in the primary field
/// of the old dialog, which is a string nobody reads and the single
/// clearest reason that surface looked like configuration.
#[test]
fn the_place_is_project_relative_never_absolute() {
    let projects = vec![project(1, "vitrum", "/src/vitrum")];
    assert_eq!(place_of(&projects, "/src/vitrum", "/home/u"), "vitrum");
    assert_eq!(
        place_of(&projects, "/src/vitrum/app", "/home/u"),
        "vitrum/app"
    );
    assert_eq!(
        place_of(&projects, "/src/vitrum/crates/vitrum-model", "/home/u"),
        "vitrum/crates/vitrum-model"
    );
    // Nothing known about it: the last two components, still not the path.
    assert_eq!(
        place_of(&projects, "/media/santh/software", "/home/u"),
        "santh/software"
    );
    assert_eq!(place_of(&[], "/tmp/scratch", "/home/u"), "tmp/scratch");
    assert_eq!(place_of(&[], "/", "/home/u"), "/");
}

/// The deepest project wins, so a session under `repo/crates/foo` is named
/// after `repo` rather than minting a second place for a subdirectory.
#[test]
fn the_deepest_project_names_the_place() {
    let projects = vec![
        project(1, "outer", "/src"),
        project(2, "vitrum", "/src/vitrum"),
    ];
    assert_eq!(
        place_of(&projects, "/src/vitrum/app", "/home/u"),
        "vitrum/app"
    );
    assert_eq!(place_of(&projects, "/src/other", "/home/u"), "outer/other");
}

/// The row draws the place and keeps the absolute path in the title, which
/// is the only place it is allowed to appear.
#[test]
fn a_row_shows_the_place_and_hides_the_path_in_the_title() {
    let i = Intent::new(
        "claude".to_string(),
        "/src/vitrum/app".to_string(),
        "vitrum/app".to_string(),
        Some("main".to_string()),
        Band::Recent,
        "used 7 times".to_string(),
        None,
    );
    let v = view(&Pick::Go(i), "/home/op");
    assert_eq!(v.text, "claude");
    assert_eq!(
        v.place,
        Some(("vitrum/app".to_string(), "/src/vitrum/app".to_string()))
    );
    assert_eq!(v.branch.as_deref(), Some("main"));
    assert_eq!(v.tip, "claude in /src/vitrum/app, used 7 times");
}

/// A directory row names the folder and puts its parent, home-shortened,
/// where the place chip goes.
#[test]
fn a_directory_row_names_the_folder_and_shortens_its_parent() {
    let v = view(&Pick::Cd("/home/op/src/vitrum".to_string()), "/home/op");
    assert_eq!(v.text, "vitrum");
    assert_eq!(
        v.place,
        Some(("~/src".to_string(), "/home/op/src/vitrum".to_string()))
    );
    assert_eq!(v.tip, "/home/op/src/vitrum");
    // Outside home it stays as it is rather than growing a wrong `~`.
    let v = view(&Pick::Cd("/srv/build".to_string()), "/home/op");
    assert_eq!(
        v.place,
        Some(("/srv".to_string(), "/srv/build".to_string()))
    );
}

/// A query that looks like a path turns the list into directories; a bare
/// word never does, or typing an agent name would lose the agent list.
#[test]
fn only_a_rooted_query_becomes_a_directory_search() {
    assert!(looks_like_path("/src"));
    assert!(looks_like_path("~/src"));
    assert!(looks_like_path("./app"));
    assert!(looks_like_path("../app"));
    assert!(looks_like_path("  /src"));
    assert!(!looks_like_path("claude"));
    assert!(!looks_like_path("src"));
    assert!(!looks_like_path(""));
    assert!(!looks_like_path("cargo test"));
}

/// The free-text escape hatch: a command nobody has run is still one Enter
/// away, because the old dialog's command field was free text and losing
/// that would make the launcher less capable than the form it replaces.
#[test]
fn anything_typed_is_still_launchable() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    let rows = intents(
        &st,
        &store_with(&[("claude", 3)]),
        &[],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    let extra =
        typed_intent(&rows, &st, "/src/vitrum", "make test", "/home/u").expect("a typed row");
    assert_eq!(extra.command, "make test");
    assert_eq!(extra.place, "vitrum");
    assert_eq!(extra.band, Band::Typed);
    // Not offered when it merely repeats a row that already exists.
    assert!(typed_intent(&rows, &st, "/src/vitrum", "claude", "/home/u").is_none());
    // Nor when the query is a path, which the directory list answers.
    assert!(typed_intent(&rows, &st, "/src/vitrum", "/src/o", "/home/u").is_none());
    assert!(typed_intent(&rows, &st, "/src/vitrum", "   ", "/home/u").is_none());
}

/// Recents in this project outrank agents merely present on PATH.
///
/// The defect: ranking by what the machine has installed rather than by
/// what this operator launches, which buries the one command they use
/// twenty times a day under four they have never run.
#[test]
fn recents_here_outrank_agents_on_path() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    let rows = intents(
        &st,
        &store_with(&[("codex", 2)]),
        &[agent("Claude Code", "claude"), agent("Codex", "codex")],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    let shown: Vec<&str> = rows.iter().map(Intent::text).collect();
    assert_eq!(shown, vec!["codex", "claude"]);
    assert_eq!(rows[0].band, Band::Recent);
}

/// Sessions running elsewhere are offered, but below everything in this
/// project, and they carry their own place rather than the current one.
#[test]
fn what_runs_elsewhere_ranks_below_what_runs_here() {
    let mut st = UiState::default();
    st.daemon.projects = vec![
        project(1, "vitrum", "/src/vitrum"),
        project(2, "harness", "/src/harness"),
    ];
    st.daemon.sessions = vec![
        session(1, "/src/harness", "gemini", Some("main")),
        session(2, "/src/vitrum/app", "codex", None),
    ];
    let rows = intents(
        &st,
        &LaunchStore::default(),
        &[agent("Claude Code", "claude")],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    let shown: Vec<(&str, &str)> = rows.iter().map(|i| (i.text(), i.place.as_str())).collect();
    assert_eq!(
        shown,
        vec![
            ("codex", "vitrum/app"),
            ("claude", "vitrum"),
            ("gemini", "harness"),
        ]
    );
    assert_eq!(rows[2].band, Band::Elsewhere);
    assert_eq!(rows[2].branch.as_deref(), Some("main"));
}

/// One row per launchable thing. Twenty agents in one repo used to be
/// twenty identical rows, each of which did the same thing.
#[test]
fn a_repeated_command_in_one_place_is_offered_once() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    st.daemon.sessions = vec![
        session(1, "/src/vitrum", "claude", None),
        session(2, "/src/vitrum", "claude", None),
        session(3, "/src/vitrum", "claude", None),
    ];
    let rows = intents(
        &st,
        &LaunchStore::default(),
        &[],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].text(), "claude");
}

/// A saved preset is a row, ranked above bare PATH discovery and named
/// with the label the operator gave it.
#[test]
fn a_saved_preset_is_a_row_with_its_own_name() {
    let mut st = UiState::default();
    st.daemon.projects = vec![project(1, "vitrum", "/src/vitrum")];
    let store = LaunchStore {
        presets: vec![SavedPreset {
            id: 1,
            label: "Plan mode".into(),
            command: "claude".into(),
            args: vec!["--permission-mode".into(), "plan".into()],
            cwd: None,
            shortcut: None,
            icon: None,
        }],
        ..LaunchStore::default()
    };
    let rows = intents(
        &st,
        &store,
        &[agent("Claude Code", "claude")],
        "",
        "/src/vitrum",
        "/home/u",
        2_000,
    );
    assert_eq!(rows[0].text(), "Plan mode");
    assert_eq!(rows[0].command, "claude --permission-mode plan");
    assert_eq!(rows[0].band, Band::Preset);
    assert_eq!(rows[1].text(), "claude");
    // Its own label is searchable, not only the command line behind it.
    assert_eq!(ranked(&rows, "plan mode"), vec![0]);
}

/// A preset the operator bound to Ctrl+3 owns Ctrl+3 on this surface, so
/// row three must not draw a 3 it will never honour.
#[test]
fn a_digit_a_preset_owns_draws_no_badge() {
    let bound = vec![SavedPreset {
        id: 1,
        label: "Plan".into(),
        command: "claude".into(),
        args: Vec::new(),
        cwd: None,
        shortcut: Some("Ctrl+3".into()),
        icon: None,
    }];
    assert!(digit_free(&bound, 1));
    assert!(digit_free(&bound, 2));
    assert!(!digit_free(&bound, 3));
    assert!(digit_free(&[], 3));

    // The slot is still drawn, empty, so the rows above and below keep one
    // left edge. An element that disappears pulls its row's text 24px in.
    assert_eq!(key_of(&bound, 0), "1");
    assert_eq!(key_of(&bound, 2), "");
    assert_eq!(key_of(&[], 2), "3");
    assert_eq!(key_of(&[], 8), "9");
    assert_eq!(key_of(&[], 9), "");
}

/// The launcher says something only when the state is unusual, and the
/// permanent captions the old dialog carried are gone.
///
/// The fourth one, "This directory is not a project yet", is locked out
/// here by name: it fired on every directory that was not already a
/// project, which is the state every machine starts in, so it was on
/// screen by default and told the operator nothing they could act on.
#[test]
fn only_an_unusual_state_earns_a_line() {
    assert_eq!(
        note(None, 3, ""),
        None,
        "a launcher with rows in it and nothing wrong must say nothing"
    );
    assert_eq!(
        note(None, 0, "zzzz"),
        Some("Nothing matches \u{201c}zzzz\u{201d}.".to_string())
    );
    assert_eq!(
        note(None, 0, ""),
        Some("Nothing launched before, and no agent found on PATH.".to_string())
    );
    // Whatever the last take reported wins over all of it.
    assert_eq!(
        note(Some("claude is not on this machine's PATH."), 3, ""),
        Some("claude is not on this machine's PATH.".to_string())
    );
}

/// No caption may explain daemon mechanics. These three sentences were on
/// the old dialog permanently and each one described how the server names
/// or groups a session rather than anything the operator could change.
#[test]
fn the_daemon_mechanics_captions_are_gone() {
    let src = include_str!("../dialog.rs");
    let markup = src
        .split_once("#[cfg(test)]")
        .map_or(src, |(before, _)| before);
    for gone in [
        "Joins project",
        "No launch history yet, so these are the agents found on PATH.",
        "Left blank, the daemon names it after the command.",
        "Starts a new project rooted here.",
        "Ranked by what you launch most, and most recently.",
    ] {
        assert!(
            !markup.contains(gone),
            "the launcher still carries the caption {gone:?}"
        );
    }
}

/// A preset that cannot run must say so in its tooltip rather than
/// advertising a command line that will fail at spawn.
#[test]
fn a_broken_preset_shows_its_fault_not_its_command() {
    let broken = SavedPreset {
        id: 1,
        label: "Ghost".into(),
        command: "vitrum-no-such-command-9f3a".into(),
        args: vec!["--go".into()],
        cwd: None,
        shortcut: None,
        icon: None,
    };
    assert_eq!(
        preset_tip(&broken),
        "vitrum-no-such-command-9f3a is not on this machine's PATH."
    );
    assert_eq!(
        attempt(
            &Intent::new(
                "vitrum-no-such-command-9f3a --go".to_string(),
                "/".to_string(),
                "root".to_string(),
                None,
                Band::Preset,
                "saved".to_string(),
                Some(broken),
            ),
            true
        ),
        Attempt::Refuse("vitrum-no-such-command-9f3a is not on this machine's PATH.".to_string())
    );
}

/// A working preset's tooltip must show the exact line it will run, with
/// the pinned directory when it has one.
#[test]
fn a_working_preset_shows_the_line_it_will_run() {
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let good = SavedPreset {
        id: 2,
        label: "Shell here".into(),
        command: shell.into(),
        args: vec!["-c".into(), "echo hi".into()],
        cwd: Some(root.into()),
        shortcut: None,
        icon: None,
    };
    assert_eq!(
        preset_tip(&good),
        format!("{shell} -c \"echo hi\" in {root}")
    );
}

/// A `code` that merely starts with Digit but is not one key must be left
/// alone rather than half-parsed into a row number nothing can produce.
#[test]
fn a_malformed_code_is_not_a_row_number() {
    assert_eq!(digit_of("Digit12"), None);
    assert_eq!(digit_of("Digit"), None);
}

/// Does `css` carry a rule for exactly this class?
///
/// Boundary-checked, never a bare substring test, and that distinction is
/// not pedantry: `css.contains(".rg-launch__branch")` is ALSO satisfied by
/// a rule for `.rg-launch__branch2`. Found by mutation. Renaming the rule
/// out from under live markup, which is the exact regression this guard
/// exists to catch, left the naive check passing.
fn styled(css: &str, class: &str) -> bool {
    let needle = format!(".{class}");
    css.match_indices(&needle).any(|(at, _)| {
        css[at + needle.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    })
}

/// Every class this file emits must have a rule.
///
/// An unstyled class is not an error anywhere: the element renders with no
/// padding, no colour and no box, and reads as a layout bug rather than a
/// missing rule. The names are read out of the markup rather than a
/// hand-kept list, because a list is exactly the thing that silently stops
/// matching what is drawn.
#[test]
fn every_class_the_launcher_emits_is_styled() {
    let src = include_str!("../dialog.rs");
    let launcher = include_str!("../../../assets/parts/22-launcher.css");
    let app = include_str!("../../app.css");
    let markup = src
        .split_once("#[cfg(test)]")
        .map_or(src, |(before, _)| before);

    let mut seen: Vec<&str> = Vec::new();
    for (at, _) in markup.match_indices("class: \"") {
        let rest = &markup[at + 8..];
        let Some(end) = rest.find('"') else { continue };
        for token in rest[..end].split_whitespace() {
            if token.starts_with("rg-") && !token.contains('{') {
                seen.push(token);
            }
        }
    }
    seen.sort_unstable();
    seen.dedup();
    assert!(
        seen.len() >= 12,
        "only found {} classes; the extraction broke rather than the markup",
        seen.len()
    );
    for class in &seen {
        assert!(
            styled(launcher, class) || styled(app, class),
            "the launcher emits .{class} and no stylesheet has a rule for it"
        );
    }
    // The ones the launcher owns must be in the launcher's own sheet, not
    // borrowed from a rule somebody else may delete.
    for class in [
        "rg-launch__query",
        "rg-launch__list",
        "rg-launch__row",
        "rg-launch__row--on",
        "rg-launch__key",
        "rg-launch__text",
        "rg-launch__place",
        "rg-launch__branch",
        "rg-launch__note",
        "rg-sheet--launcher",
    ] {
        assert!(
            styled(launcher, class),
            "22-launcher.css has no rule for .{class}"
        );
    }
}

/// The stylesheet with its comments removed.
///
/// Every guard below reads CODE. The prose in this file argues about
/// transitions, animation and pixel counts, so a guard matching bare words
/// against the raw text would fail on its own documentation.
fn code_only(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Every row element must be able to give way, or the longest realistic
/// agent name, project and branch together push each other out of the row.
///
/// This is the P0 in GOAL.md: zero overlap, not unlikely overlap. A flex
/// child with `min-width: 0` and an ellipsis cannot overlap its neighbours
/// at any width; one without them can and does.
///
/// Derived from the stylesheet, NOT from a list. The version before this
/// walked the three parts that existed when it was written, so a fourth
/// row part added later was invisible to it: found by mutation, a new
/// `.rg-launch__tag` with no `min-width` passed. Declaring `flex-shrink`
/// is the honest trigger, because that is a box saying it will be squeezed,
/// and a box that will be squeezed has to be able to elide.
#[test]
fn every_row_part_can_yield_before_it_overlaps() {
    let css = code_only(include_str!("../../../assets/parts/22-launcher.css"));
    let mut checked: Vec<&str> = Vec::new();
    for (at, _) in css.match_indices(".rg-launch__") {
        let rest = &css[at + 1..];
        let Some(head) = rest.find(" {") else {
            continue;
        };
        let class = &rest[..head];
        if class.contains([' ', ',', ':', '.']) {
            continue;
        }
        let Some((_, body)) = rest.split_once(" {") else {
            continue;
        };
        let Some((rule, _)) = body.split_once('}') else {
            continue;
        };
        if !rule.contains("flex-shrink") {
            continue;
        }
        checked.push(class);
        assert!(
            rule.contains("min-width: 0"),
            ".{class} declares flex-shrink but cannot shrink below its text, \
             so a long one pushes its neighbours out"
        );
        assert!(
            rule.contains("text-overflow: ellipsis"),
            ".{class} declares flex-shrink but clips mid-glyph instead of eliding"
        );
    }
    checked.sort_unstable();
    assert_eq!(
        checked,
        ["rg-launch__branch", "rg-launch__place", "rg-launch__text"],
        "the row's shrinkable parts have changed; the three that carry a \
         row's meaning must all still be here"
    );
}

/// Nothing on this surface may animate. Idle CPU is the product's headline
/// number and a launcher is opened dozens of times an hour.
///
/// Matched on the property PREFIX, not on the shorthand spelling. The
/// version before this looked for `animation:` and `transition:`, so
/// `transition-property` and `animation-name` walked straight past it:
/// found by mutation, both longhands passed.
///
/// The list is still hand-kept, which is the shape that has cost this
/// channel most of its escapes today, so it names every way CSS can move
/// something rather than the two anybody remembers: `scroll-behavior:
/// smooth` and `@starting-style` are motion the earlier list would have
/// waved through.
#[test]
fn the_launcher_stylesheet_has_no_motion() {
    let css = code_only(include_str!("../../../assets/parts/22-launcher.css"));
    for banned in [
        "@keyframes",
        "@starting-style",
        "animation",
        "transition",
        "infinite",
        "will-change",
        "scroll-behavior",
        "view-transition",
        "offset-path",
    ] {
        assert!(
            !css.contains(banned),
            "22-launcher.css declares {banned:?}, which the motion budget does not allow here"
        );
    }
}

/// Every length in the launcher's sheet lands on the 4px grid.
///
/// Both units, and that is the repair. The version before this read `rem`
/// and nothing else, so `padding: 0 6px` and `height: 37px` were invisible
/// to a guard whose whole subject is the grid: found by mutation, both
/// passed. A rem value is authored at 1x, so any multiple of 0.25rem is on
/// the grid; a px literal must be a multiple of 4, except the 1px hairline
/// and the 2px focus outline, which 10-spacing.css documents as
/// device-resolution artefacts rather than design measurements.
///
/// The UNIT SET is derived, not listed. Checking two units and ignoring
/// the rest is the same hole one step out: `padding: 0.4em` is off the
/// grid and neither branch above would ever see it. So every number
/// followed by letters is collected and the set of units found must be
/// exactly the two this design system authors in. A percentage carries no
/// letters and is not a grid length, so `width: 100%` is untouched.
#[test]
fn every_length_in_the_launcher_is_on_the_four_pixel_grid() {
    let css = code_only(include_str!("../../../assets/parts/22-launcher.css"));
    let bytes = css.as_bytes();
    let mut units: Vec<(String, usize)> = Vec::new();
    for (n, line) in css.lines().enumerate() {
        let at_line = line.as_ptr() as usize - css.as_ptr() as usize;
        let mut i = 0usize;
        while i < line.len() {
            if !line.as_bytes()[i].is_ascii_alphabetic() {
                i += 1;
                continue;
            }
            let start = i;
            while i < line.len() && line.as_bytes()[i].is_ascii_alphabetic() {
                i += 1;
            }
            // Only a letter run glued to the end of a number is a unit.
            if start == 0 || !bytes[at_line + start - 1].is_ascii_digit() {
                continue;
            }
            let unit = &line[start..i];
            let head = &line[..start];
            let num_at = head
                .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
                .map_or(0, |k| k + 1);
            let Ok(value) = head[num_at..].parse::<f64>() else {
                continue;
            };
            units.push((unit.to_string(), n + 1));
            let px = match unit {
                "rem" => value * 16.0,
                // The hairline and the focus ring are not measurements.
                "px" if value == 1.0 || value == 2.0 => continue,
                "px" => value,
                other => panic!(
                    "line {}: {value}{other} uses a unit this design system does not \
                     author in; every length here is rem, or px for a hairline",
                    n + 1
                ),
            };
            assert!(
                (px / 4.0 - (px / 4.0).round()).abs() < 1e-9,
                "line {}: {value}{unit} is {px}px, which is off the 4px grid",
                n + 1
            );
        }
    }
    let mut kinds: Vec<&str> = units.iter().map(|(u, _)| u.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(
        kinds,
        ["px", "rem"],
        "the units in this stylesheet have changed; found {kinds:?}"
    );
    assert!(
        units.len() >= 8,
        "only {} lengths scanned; the extraction broke",
        units.len()
    );
}

/// Every ranked row reaches the markup. The list may not silently render
/// a subset of what it ranked.
///
/// This closes the one gap the pure tests above cannot see. `ranked` and
/// `intents` are asserted on their exact contents, so a truncation THERE
/// fails several tests; a `.take(1)` on the rsx loop that draws them fails
/// none of them, and the operator gets one row under a launcher that
/// ranked nine. That is the shape SearchUi found in the search results and
/// TabIcons found in a mark that stayed inside its box: a semantic
/// degradation every structural check passes.
///
/// Asserted on the source because rendering this component needs a
/// VirtualDom, three signals and two worker threads, and a guard that
/// heavy for ten lines of markup is its own failure surface. What it
/// checks is exact: the loop walks the whole of `views`, and nothing
/// between the ranking and the DOM narrows it.
#[test]
fn every_ranked_row_reaches_the_markup() {
    let src = include_str!("../dialog.rs");
    let markup = src
        .split_once("#[cfg(test)]")
        .map_or(src, |(before, _)| before);
    let (_, after) = markup
        .split_once("class: \"rg-launch__list\"")
        .expect("the launcher's list element");
    let loop_head = after
        .lines()
        .find(|l| l.trim_start().starts_with("for "))
        .expect("the list draws its rows in a loop");
    assert_eq!(
        loop_head.trim(),
        "for (i, v) in views.iter().enumerate() {",
        "the row loop no longer walks the whole of `views`; anything that \
         narrows it here renders fewer rows than the launcher ranked, and \
         every other test in this file still passes"
    );
    // `views` is a one-for-one map over the ranked rows.
    assert!(
        markup.contains(
            "let views: Vec<RowView> = rows.iter().map(|p| view(p, &home_now)).collect();"
        ),
        "`views` is no longer a one-for-one map over the ranked rows"
    );
}

/// Nothing on the open path may block, and nothing may run per keystroke
/// that runs per open.
///
/// Asserted on the source rather than on a stopwatch, because a timing
/// assertion on a shared machine is a flake and this is a claim about
/// STRUCTURE, not about a number. Four rules, each one a regression this
/// surface has already shipped once:
///
/// 1. The `PATH` walk is five lookups across every directory in `PATH`,
///    measured at 44.5us warm and unbounded on an automounted share. It
///    may appear exactly once, handed to `off_thread`.
/// 2. `read_dir` on a wedged network mount blocks in the kernel for as
///    long as the mount wants. Same rule.
/// 3. The profile is read once when the launcher opens, never per render.
/// 4. `launch::validate` is one `stat` plus one `PATH` walk. The dialog
///    this replaced called it in its render body, so every keystroke paid
///    for both; it is now reached only through `attempt`, on a click.
#[test]
fn the_open_path_never_walks_path_or_the_filesystem() {
    let src = include_str!("../dialog.rs");
    let markup = src
        .split_once("#[cfg(test)]")
        .map_or(src, |(before, _)| before);
    // CODE only. The comments below this component explain what
    // `off_thread` is for and name every function this test counts, so
    // scanning them would make the prose fail the test.
    let body: String = markup
        .split_once("pub fn NewSessionDialog(")
        .expect("the launcher component")
        .1
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.as_str();

    assert_eq!(
        body.matches("detected_agents").count(),
        1,
        "the PATH walk appears more than once in the launcher"
    );
    assert!(
        body.contains("off_thread(launch::detected_agents)"),
        "the PATH walk is on the UI thread; it must go through off_thread"
    );

    assert_eq!(
        body.matches("list_dirs").count(),
        1,
        "the directory walk appears more than once in the launcher"
    );
    assert!(
        body.contains("off_thread(move || launch::list_dirs("),
        "read_dir is on the UI thread; it must go through off_thread"
    );

    assert_eq!(
        body.matches("load_launch_store").count(),
        1,
        "the profile is read more than once; it belongs in a use_signal \
         initialiser and nowhere else"
    );
    assert!(
        !body.contains("launch::validate("),
        "validate is a stat plus a PATH walk and must not be reachable \
         from a render; it belongs behind `attempt`, on a click"
    );

    // Ranking is built once per open; a keystroke re-runs the filter only.
    assert_eq!(
        body.matches("intents(").count(),
        1,
        "the ranking is rebuilt somewhere other than its own memo, so a \
         keystroke pays for it"
    );
}

/// The one-click control must actually launch.
///
/// Every other test in this file proves the DECISION is right:
/// `primary_of` returns Ready or a concrete Choose, `intents` ranks, the
/// list draws what it ranked. Not one of them can see whether anything
/// calls it. Rewire `on_launch_now` to `open_new_session` and the launcher
/// silently becomes the two-click form this whole ticket replaced, with
/// all 39 tests green and the button still reading `+ claude`.
///
/// That is the defect this ticket exists to fix, restored one layer up and
/// invisible to the suite written to prevent it. ScrollbackTruth found the
/// same shape in a backfill budget the send site never passed, and
/// NotifyActivation in a handler installed by a function nobody calls.
/// Main asked for exactly this not to happen and said so in writing.
///
/// Reads the shipped `main.rs`, which this module does not own, because a
/// guard belongs with the contract it defends rather than with the code it
/// reads. Anchored on tokens, never on quote pairing, per the note in
/// GOAL.md about how source scans in this codebase go wrong.
#[test]
fn the_one_click_control_actually_launches() {
    let main = crate::testkit::shell();
    let main = main.as_str();
    // Anchored on the test MODULE, not on the first `#[cfg(test)]`.
    // main.rs carries `#[cfg(test)] mod testkit;` at the top of the file,
    // so the obvious `split_once("#[cfg(test)]")` truncates at line 26 and
    // hands back an empty scan that passes every `!contains` clause and
    // fails every `expect`. Caught by this guard failing on the first run,
    // which is the only reason it is written down.
    let markup = main
        .split_once("\n#[cfg(test)]\nmod tests {")
        .map_or(main, |(before, _)| before);
    assert!(
        markup.len() > main.len() / 2,
        "the markup scan collapsed to {} of {} bytes; the anchor moved",
        markup.len(),
        main.len()
    );
    // CODE only: the prose around these call sites names every function
    // this test looks for.
    let code: String = markup
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    // 1. The sidebar's primary half reaches the launch, not the layer.
    let (_, wired) = code
        .split_once("on_launch_now:")
        .expect("main.rs does not wire the sidebar's primary half at all");
    let handler = wired.lines().next().unwrap_or_default();
    assert!(
        handler.contains("launch_now("),
        "on_launch_now is wired to {handler:?}, which never reaches \
         dialog::primary_launch, so the primary control opens a layer and \
         the product is back to two clicks"
    );
    assert!(
        !handler.contains("open_new_session"),
        "on_launch_now opens the ranked list; that is the caret half's job"
    );

    // 2. The handler decides with `primary_launch`.
    let (_, rest) = code
        .split_once("fn launch_now(")
        .expect("main.rs does not define launch_now");
    let body = rest.split_once("\nfn ").map_or(rest, |(b, _)| b);
    assert!(
        body.contains("dialog::primary_launch("),
        "launch_now no longer asks dialog::primary_launch, so whatever it \
         starts is not the ranked intent this file computed"
    );

    // 3. Ready SENDS and Choose OPENS. Swapping them is the silent
    //    two-click regression with the decision still correct.
    let (_, ready) = body
        .split_once("Primary::Ready(")
        .expect("launch_now has no Ready arm");
    let ready = ready
        .split_once("Primary::Choose")
        .map_or(ready, |(b, _)| b);
    assert!(
        ready.contains("start_session("),
        "the Ready arm does not send; a confident launch must not open a layer"
    );
    assert!(
        !ready.contains("open_new_session"),
        "the Ready arm opens the ranked list, so one click is two again"
    );
    let (_, choose) = body
        .split_once("Primary::Choose")
        .expect("launch_now has no Choose arm");
    assert!(
        choose.contains("open_new_session("),
        "the Choose arm does not open the list, so the operator is told \
         nothing when the launcher declines to guess"
    );
}
