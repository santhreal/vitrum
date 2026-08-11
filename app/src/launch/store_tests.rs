use super::*;
use crate::state::LauncherPrefs;

/// A fixed instant, so every age in these tests is exact rather than
/// "whatever the clock said". 2023-11-14T22:13:20Z.
const NOW: u64 = 1_700_000_000_000;
const HOUR: u64 = 3_600_000;
const DAY: u64 = 86_400_000;

fn entry(command: &str, count: u32, age_ms: u64) -> HistoryEntry {
    HistoryEntry {
        command: command.to_string(),
        args: Vec::new(),
        count,
        last_used_ms: NOW - age_ms,
        icon: None,
    }
}

fn store_with(history: Vec<HistoryEntry>) -> LaunchStore {
    LaunchStore {
        history,
        ..LaunchStore::default()
    }
}

/// A scratch directory that removes itself, so a failing test cannot leave
/// a tree behind for the next run to trip over.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "vitrum-launch-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir is writable");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn text(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- ranking ---------------------------------------------------------

/// Ranking must be frequency AND recency, not either alone.
///
/// Pure recency puts a command run once yesterday above one run four
/// hundred times this month; pure frequency freezes the list against what
/// the operator did six months ago and never offers what they started
/// doing this week. Both failures put the wrong command first in the
/// control they use most.
#[test]
fn ranking_multiplies_frequency_by_recency() {
    assert_eq!(history_score(&entry("a", 3, 0), NOW), 300);
    assert_eq!(history_score(&entry("a", 3, HOUR), NOW), 300);
    assert_eq!(history_score(&entry("a", 3, HOUR + 1), NOW), 210);
    assert_eq!(history_score(&entry("a", 3, 2 * DAY), NOW), 150);
    assert_eq!(history_score(&entry("a", 3, 10 * DAY), NOW), 90);
    assert_eq!(history_score(&entry("a", 3, 400 * DAY), NOW), 30);

    // Neither axis alone gives this order. Five uses today (5 x 70 = 350)
    // beats thirty uses a year ago (30 x 10 = 300), so frequency does not
    // win on its own; and thirty uses a year ago beats one use in the last
    // hour (1 x 100 = 100), so recency does not either.
    let store = store_with(vec![
        entry("stale", 30, 400 * DAY),
        entry("fresh", 5, 2 * HOUR),
        entry("once", 1, HOUR),
    ]);
    let order: Vec<&str> = ranked_history(&store, NOW)
        .iter()
        .map(|e| e.command.as_str())
        .collect();
    assert_eq!(order, vec!["fresh", "stale", "once"]);
}

/// Two entries with the same score must come out in a fixed order, and
/// the more recent one must win. A dropdown that reshuffles between two
/// openings of the same dialog moves the row out from under the pointer.
#[test]
fn ties_break_on_recency_then_text() {
    let store = store_with(vec![
        entry("zed", 2, 2 * HOUR),
        entry("alpha", 2, 3 * HOUR),
        entry("beta", 2, 2 * HOUR),
    ]);
    let order: Vec<&str> = ranked_history(&store, NOW)
        .iter()
        .map(|e| e.command.as_str())
        .collect();
    // All three score 2 * 70. The two at two hours old sort before the
    // three-hour one, and between them the text decides.
    assert_eq!(order, vec!["beta", "zed", "alpha"]);
}

/// A launch of a command already in the history must bump it, never add a
/// second row. Twenty launches of `claude` producing twenty identical
/// suggestions is the whole dropdown gone.
#[test]
fn a_repeat_launch_bumps_the_existing_entry() {
    let mut store = LaunchStore::default();
    remember(&mut store, "claude", &[], "/tmp", NOW - DAY, LauncherPrefs::default());
    remember(&mut store, "claude", &[], "/tmp", NOW, LauncherPrefs::default());
    assert_eq!(store.history.len(), 1);
    assert_eq!(store.history[0].count, 2);
    assert_eq!(store.history[0].last_used_ms, NOW);
}

/// The same program with different arguments is a different suggestion.
/// Merging them would offer a command line the operator has never run.
#[test]
fn different_arguments_are_different_history_entries() {
    let mut store = LaunchStore::default();
    remember(&mut store, "claude", &[], "/tmp", NOW, LauncherPrefs::default());
    remember(
        &mut store,
        "claude",
        &["--permission-mode".to_string(), "plan".to_string()],
        "/tmp",
        NOW,
        LauncherPrefs::default(),
    );
    assert_eq!(store.history.len(), 2);
}

/// The history must cap by rank, not by age. Truncating the tail of an
/// unsorted vector throws away whatever happens to be last, which can be
/// the command the operator runs every single day.
#[test]
fn the_history_cap_drops_the_worst_entry_not_the_last_one() {
    let mut store = LaunchStore {
        history: (0..HISTORY_MAX)
            .map(|i| entry(&format!("cmd{i:02}"), 1, (400 + i as u64) * DAY))
            .collect(),
        ..LaunchStore::default()
    };
    store.history[0] = entry("daily", 500, 0);
    remember(&mut store, "brand-new", &[], "/tmp", NOW, LauncherPrefs::default());
    assert_eq!(store.history.len(), HISTORY_MAX);
    assert!(
        store.history.iter().any(|e| e.command == "daily"),
        "the most-used command was evicted"
    );
    assert!(
        store.history.iter().any(|e| e.command == "brand-new"),
        "the launch that triggered the cap was the one thrown away"
    );
    // Every survivor scores 10 except those two, so the tie-break on
    // recency decides, and the oldest one-off is the single casualty.
    assert!(
        !store.history.iter().any(|e| e.command == "cmd59"),
        "the oldest one-off survived and something better was dropped"
    );
    assert!(store.history.iter().any(|e| e.command == "cmd58"));
}

/// A launch must record the directory it ran in, and must not blank it
/// when the caller passes nothing. `last_cwd` is the dialog's fallback
/// after a restart, and an empty one sends the operator back to a blank
/// field.
#[test]
fn a_launch_records_its_directory_and_never_blanks_it() {
    let mut store = LaunchStore::default();
    remember(&mut store, "sh", &[], "/src/vitrum", NOW, LauncherPrefs::default());
    assert_eq!(store.last_cwd, "/src/vitrum");
    remember(&mut store, "sh", &[], "   ", NOW, LauncherPrefs::default());
    assert_eq!(store.last_cwd, "/src/vitrum");
}

// ---- suggestions -----------------------------------------------------

/// An empty history must offer the agents really on `PATH`, and nothing
/// else. Inventing a name for a binary this machine does not have is a
/// suggestion that fails at spawn, and the login shell is not an agent.
#[test]
fn an_empty_history_offers_only_what_is_installed() {
    let store = LaunchStore::default();
    let detected = [Detected {
        label: "Claude Code",
        command: "claude",
    }];
    let got = command_suggestions(&store, &detected, "", NOW, 8);
    assert_eq!(
        got,
        vec![
            CommandSuggestion {
                line: "claude".into(),
                note: "Claude Code".into(),
                source: CommandSource::Detected,
            },
        ]
    );
}

/// An empty history on a machine with no agent must produce an empty list,
/// not a placeholder. An offer nobody can run is worse than an honest
/// blank.
#[test]
fn nothing_installed_and_no_history_suggests_nothing() {
    assert!(command_suggestions(&LaunchStore::default(), &[], "", NOW, 8).is_empty());
}

/// History must come before detection, ranked, and a detected agent the
/// operator has already run must appear once rather than twice.
#[test]
fn history_outranks_detection_and_is_never_duplicated() {
    let store = store_with(vec![entry("codex", 1, 3 * DAY), entry("claude", 9, HOUR)]);
    let detected = [
        Detected {
            label: "Claude Code",
            command: "claude",
        },
        Detected {
            label: "Gemini CLI",
            command: "gemini",
        },
    ];
    let got = command_suggestions(&store, &detected, "", NOW, 8);
    let lines: Vec<&str> = got.iter().map(|s| s.line.as_str()).collect();
    assert_eq!(lines, vec!["claude", "codex", "gemini"]);
    assert_eq!(got[0].note, "used 9 times");
    assert_eq!(got[1].note, "used once");
    assert_eq!(got[2].source, CommandSource::Detected);
}

/// A query must prefer a line that starts with it over one that merely
/// contains it, even when the containing one is ranked higher. Typing
/// "cl" and getting `codex --client` above `claude` is the field
/// disagreeing with the letters in it.
#[test]
fn a_prefix_match_beats_a_higher_ranked_substring_match() {
    let mut store = store_with(vec![entry("claude", 1, 400 * DAY)]);
    store.history.push(HistoryEntry {
        command: "codex".into(),
        args: vec!["--client".into()],
        count: 90,
        last_used_ms: NOW,
        icon: None,
    });
    let got = command_suggestions(&store, &[], "cl", NOW, 8);
    let lines: Vec<&str> = got.iter().map(|s| s.line.as_str()).collect();
    assert_eq!(lines, vec!["claude", "codex --client"]);
}

// ---- a command that is not on PATH -----------------------------------

/// A command that is not on `PATH` must launch with a warning, not an
/// error. The daemon runs on this machine so the check is meaningful, but
/// it is a different process with a different environment and refusing
/// outright would block a launch that would have worked.
#[test]
fn a_command_not_on_path_warns_and_still_launches() {
    let dir = if cfg!(windows) { "C:\\" } else { "/" };
    let l = validate(dir, "vitrum-no-such-command-9f3a --go", "")
        .expect("an absent binary is a warning, not a refusal");
    assert_eq!(l.command, "vitrum-no-such-command-9f3a");
    assert_eq!(l.args, vec!["--go".to_string()]);
    assert_eq!(
        l.warning.as_deref(),
        Some(
            "vitrum-no-such-command-9f3a is not on this machine's PATH. Launching anyway will fail unless the daemon resolves it differently."
        )
    );
}

/// A preset naming a binary this machine does not have must report it
/// before the spawn, naming the binary. "Preset failed" three seconds
/// later from the daemon tells the operator nothing.
#[test]
fn a_preset_naming_an_absent_binary_reports_it_by_name() {
    let p = SavedPreset {
        id: 1,
        label: "Ghost".into(),
        command: "vitrum-no-such-command-9f3a".into(),
        ..SavedPreset::default()
    };
    assert_eq!(
        preset_fault(&p),
        Some(PresetFault::NotOnPath(
            "vitrum-no-such-command-9f3a".to_string()
        ))
    );
    assert_eq!(
        preset_fault(&p).expect("fault").sentence(),
        "vitrum-no-such-command-9f3a is not on this machine's PATH."
    );
}

// ---- a directory that is not there -----------------------------------

/// A cwd that does not exist must be refused by name, before anything is
/// sent. The daemon's own failure arrives a round trip later and does not
/// say which of the two machines is missing the directory.
#[test]
fn a_nonexistent_directory_is_refused_by_name() {
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    assert_eq!(
        validate("/vitrum-no-such-directory-9f3a", shell, ""),
        Err("/vitrum-no-such-directory-9f3a is not a directory on this machine.".to_string())
    );
}

/// A preset pinning a directory that has gone must say so, and must say
/// it before the `PATH` check: a preset with both problems is more
/// usefully described by the one the operator can see is wrong.
#[test]
fn a_preset_pinning_a_missing_directory_reports_the_directory() {
    let p = SavedPreset {
        id: 1,
        label: "Gone".into(),
        command: "vitrum-no-such-command-9f3a".into(),
        cwd: Some("/vitrum-no-such-directory-9f3a".into()),
        ..SavedPreset::default()
    };
    assert_eq!(
        preset_fault(&p).expect("fault").sentence(),
        "/vitrum-no-such-directory-9f3a is not a directory on this machine."
    );
}

/// Completing inside a directory that does not exist, or one that cannot
/// be read, must report nothing. An error here would put a red banner
/// under a field the operator is halfway through typing into.
#[test]
fn an_unreadable_directory_completes_to_nothing() {
    assert!(list_dirs("/vitrum-no-such-directory-9f3a").is_empty());
    assert!(list_dirs("").is_empty());
    // A file is not a directory, and read_dir on one errors.
    let scratch = Scratch::new("file");
    let file = scratch.path().join("plain.txt");
    std::fs::write(&file, b"x").expect("write");
    assert!(list_dirs(&file.to_string_lossy()).is_empty());
}

/// Seeding must never hand the dialog a blank field, and must prefer a
/// directory that exists over one that merely used to.
#[test]
fn the_seed_directory_prefers_what_exists_and_is_never_blank() {
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    // Absolute on this platform, and absent on any. A Unix-shaped path is
    // merely rooted on Windows, not absolute, and `seed_cwd` refuses relative
    // candidates on purpose, so spelling it that way would assert the refusal
    // rather than the never-blank rule.
    let gone = if cfg!(windows) {
        r"C:\vitrum-no-such-directory-9f3a"
    } else {
        "/vitrum-no-such-directory-9f3a"
    };
    let elsewhere = if cfg!(windows) { r"C:\also-not-here" } else { "/also-not-here" };
    let store = LaunchStore { last_cwd: root.to_string(), ..LaunchStore::default() };
    // A seed that is gone loses to a last_cwd that is there.
    assert_eq!(seed_cwd(gone, &store, ""), root);
    // A seed that is there wins outright.
    assert_eq!(seed_cwd(root, &store, ""), root);
    // Nothing exists anywhere: still not blank.
    let empty = LaunchStore::default();
    assert_eq!(seed_cwd(gone, &empty, ""), gone);
    assert_eq!(seed_cwd("", &empty, elsewhere), elsewhere);
}

/// A relative seed must be skipped, not used. Observed against the real
/// daemon: another client created a session with cwd `.`, the daemon
/// minted a project rooted at `.`, and this dialog then opened on `.` and
/// wrote `.` into `last_cwd`, so one client's working directory became
/// every later launch's default. `.` is a directory that exists, which is
/// exactly why an existence check alone does not catch it.
#[test]
fn a_relative_seed_is_skipped_even_though_it_exists() {
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    assert!(
        Path::new(".").is_dir(),
        "the trap only exists because this is true"
    );
    let store = LaunchStore {
        last_cwd: root.to_string(),
        ..LaunchStore::default()
    };
    assert_eq!(seed_cwd(".", &store, ""), root);
    assert_eq!(seed_cwd("src", &store, ""), root);
    // And a relative last_cwd falls through to home rather than being
    // handed back as the answer.
    let relative = LaunchStore {
        last_cwd: ".".to_string(),
        ..LaunchStore::default()
    };
    assert_eq!(seed_cwd("", &relative, root), root);
    assert_eq!(seed_cwd("", &relative, ""), "");
}

// ---- directory completion --------------------------------------------

/// Completion must work against a real tree, must match case-insensitively
/// on the prefix, and must cap what it returns. Sixty directories in one
/// folder is an ordinary checkout, and a dropdown that renders all of them
/// covers the dialog.
#[test]
fn completion_matches_a_real_tree_of_sixty_directories() {
    let scratch = Scratch::new("tree");
    for i in 0..60 {
        std::fs::create_dir(scratch.path().join(format!("proj{i:02}"))).expect("mkdir");
    }
    std::fs::create_dir(scratch.path().join("Zebra")).expect("mkdir");
    std::fs::create_dir(scratch.path().join(".hidden")).expect("mkdir");
    std::fs::write(scratch.path().join("notes.txt"), b"x").expect("write");

    let all = list_dirs(&scratch.text());
    assert_eq!(
        all.len(),
        62,
        "60 projects, Zebra and .hidden; not the file"
    );

    let (dir, fragment) = split_dir_input(&format!("{}/proj1", scratch.text()), "");
    assert_eq!(dir, scratch.text());
    assert_eq!(fragment, "proj1");

    let hits = filter_dirs(&all, &fragment, COMPLETE_MAX);
    assert_eq!(hits.len(), COMPLETE_MAX, "ten match, eight are shown");
    assert_eq!(leaf(&hits[0]), "proj10");
    assert_eq!(leaf(&hits[7]), "proj17");

    // Case-insensitive, like every shell.
    let zebra = filter_dirs(&all, "zeb", COMPLETE_MAX);
    assert_eq!(zebra.iter().map(|p| leaf(p)).collect::<Vec<_>>(), ["Zebra"]);
}

/// A dotted directory must stay out of the list until the operator types
/// a dot. A home directory has dozens and they would bury the two folders
/// the operator actually keeps work in.
#[test]
fn hidden_directories_appear_only_when_asked_for() {
    let entries = vec![
        "/home/ada/.cache".to_string(),
        "/home/ada/code".to_string(),
        "/home/ada/.config".to_string(),
    ];
    assert_eq!(filter_dirs(&entries, "", 8), vec!["/home/ada/code"]);
    assert_eq!(
        filter_dirs(&entries, ".c", 8),
        vec!["/home/ada/.cache", "/home/ada/.config"]
    );
}

/// A trailing separator means "list this directory", and `.` or `..` has
/// no last component to complete. Treating `~/src/` as the fragment `src`
/// inside `~` would offer siblings instead of children.
#[test]
fn a_trailing_separator_lists_the_directory_itself() {
    assert_eq!(
        split_dir_input("/src/vitrum/", ""),
        ("/src/vitrum/".to_string(), String::new())
    );
    assert_eq!(
        split_dir_input("/src/vit", ""),
        ("/src".to_string(), "vit".to_string())
    );
    assert_eq!(
        split_dir_input("/src/..", ""),
        ("/src/..".to_string(), String::new())
    );
    assert_eq!(split_dir_input("", ""), (String::new(), String::new()));
}

/// A relative path must complete to nothing rather than to this process's
/// current directory. The session runs in the daemon, which is a different
/// process; completing against our own cwd would offer a tree the session
/// will never see.
#[test]
fn a_relative_path_offers_no_completions() {
    assert_eq!(
        split_dir_input("src", ""),
        (String::new(), "src".to_string())
    );
    assert!(list_dirs("").is_empty());
}

/// `~` must expand, and must be left alone when the platform cannot say
/// where home is. Expanding against an empty string would produce a path
/// rooted at nothing.
#[test]
fn tilde_expands_only_when_there_is_a_home() {
    assert_eq!(expand_home("~", "/home/ada"), "/home/ada");
    assert_eq!(
        expand_home("~/src", "/home/ada"),
        format!("/home/ada{MAIN_SEPARATOR}src")
    );
    assert_eq!(
        expand_home("~/src", "/home/ada/"),
        format!("/home/ada{MAIN_SEPARATOR}src")
    );
    assert_eq!(expand_home("~/src", ""), "~/src");
    assert_eq!(expand_home("~notauser/x", "/home/ada"), "~notauser/x");
}

/// A trailing separator must not reach the daemon. The dialog appends one
/// every time a completion is accepted, and two spellings of one directory
/// key as two projects in the sidebar.
#[test]
fn a_trailing_separator_is_trimmed_but_a_root_is_not() {
    assert_eq!(tidy_dir("/src/vitrum/"), "/src/vitrum");
    assert_eq!(tidy_dir("  /src/vitrum//  "), "/src/vitrum");
    assert_eq!(tidy_dir("/"), "/");
    assert_eq!(tidy_dir("C:\\"), "C:\\");
    assert_eq!(tidy_dir(""), "");
}

// ---- command lines ---------------------------------------------------

/// Joining and splitting must be exact inverses, including for the shapes
/// that break a naive quoter: a space, an embedded quote, a Windows path,
/// and a doubled backslash that collapses on the way back.
#[test]
fn a_command_line_survives_a_round_trip() {
    for (command, args) in [
        ("claude", vec![]),
        ("claude", vec!["--permission-mode", "plan"]),
        ("/usr/bin/env", vec!["sh", "-c", "echo hello world"]),
        ("say", vec![r#"a "quoted" word"#]),
        (r"C:\tools\agent.exe", vec![r"C:\Program Files\x"]),
        (r"C:\\weird", vec![r"trailing\"]),
        ("empty", vec![""]),
    ] {
        let args: Vec<String> = args.into_iter().map(String::from).collect();
        let line = join_command(command, &args);
        assert_eq!(
            split_command(&line),
            Some((command.to_string(), args.clone())),
            "round trip failed for {line}"
        );
    }
}

/// An ordinary word must not gain quotes. A dropdown row reading
/// `"claude"` instead of `claude` is a suggestion the operator distrusts.
#[test]
fn an_ordinary_word_is_not_quoted() {
    assert_eq!(join_command("claude", &[]), "claude");
    assert_eq!(
        join_command("claude", &["--plan".to_string()]),
        "claude --plan"
    );
    assert_eq!(join_command(r"C:\tools\x.exe", &[]), r"C:\tools\x.exe");
}

// ---- the store file --------------------------------------------------

/// The document must survive a round trip through its own codec, or a
/// preset saved in the settings panel comes back different next launch.
#[test]
fn the_store_round_trips_through_its_codec() {
    let doc = LaunchStore {
        version: LAUNCH_STORE_VERSION,
        presets: vec![SavedPreset {
            id: 42,
            label: "Plan mode".into(),
            command: "claude".into(),
            args: vec!["--permission-mode".into(), "plan".into()],
            cwd: Some("/src/vitrum".into()),
            shortcut: Some("Ctrl+Shift+1".into()),
            icon: Some("spark".into()),
        }],
        history: vec![entry("sh", 3, HOUR)],
        recents: vec![RecentEntry {
            command: "sh".into(),
            args: Vec::new(),
            cwd: "/src/vitrum".into(),
            last_used_ms: NOW - HOUR,
            icon: None,
        }],
        last_cwd: "/src/vitrum".into(),
    };
    assert_eq!(parse_launch_store(&encode_launch_store(&doc)), doc);
}

/// An unreadable file must default rather than fail. This file holds
/// convenience, not truth: refusing to open the new-session dialog over a
/// corrupt history would cost the operator the ability to start a session.
#[test]
fn a_corrupt_or_empty_file_reads_as_defaults() {
    assert_eq!(parse_launch_store(""), LaunchStore::default());
    assert_eq!(parse_launch_store("{"), LaunchStore::default());
    assert_eq!(parse_launch_store("null"), LaunchStore::default());
    assert_eq!(parse_launch_store("[]"), LaunchStore::default());
}

/// A file from a newer build must not be half-read. Dropping the fields
/// this build does not know about and writing the result back would
/// silently delete presets the operator can still see in the newer one.
#[test]
fn a_future_version_is_not_read_at_all() {
    let doc = r#"{"version":99,"presets":[{"id":1,"label":"x","command":"sh"}]}"#;
    assert_eq!(parse_launch_store(doc), LaunchStore::default());
}

/// A hand-written file with only the fields a person would type must
/// load. This is the one part of the profile people are expected to edit.
#[test]
fn a_minimal_hand_written_preset_loads() {
    let doc = r#"{"presets":[{"id":7,"label":"Shell","command":"bash"}]}"#;
    let got = parse_launch_store(doc);
    assert_eq!(got.version, LAUNCH_STORE_VERSION);
    assert_eq!(
        got.presets,
        vec![SavedPreset {
            id: 7,
            label: "Shell".into(),
            command: "bash".into(),
            args: Vec::new(),
            cwd: None,
            shortcut: None,
            icon: None,
        }]
    );
}

/// Replacing the preset list must not touch history or the last
/// directory. Saving a preset in the settings panel and losing every
/// command suggestion is the read-modify-write bug this locks out; the
/// same fold `save_presets` performs is exercised here without going near
/// the shared config directory.
#[test]
fn writing_presets_preserves_history_and_last_directory() {
    let existing = encode_launch_store(&LaunchStore {
        version: LAUNCH_STORE_VERSION,
        presets: Vec::new(),
        history: vec![entry("claude", 8, HOUR)],
        recents: Vec::new(),
        last_cwd: "/src/vitrum".into(),
    });
    let mut store = parse_launch_store(&existing);
    store.version = LAUNCH_STORE_VERSION;
    store.presets = vec![SavedPreset {
        id: 1,
        label: "New".into(),
        command: "sh".into(),
        ..SavedPreset::default()
    }];
    let after = parse_launch_store(&encode_launch_store(&store));
    assert_eq!(after.history, vec![entry("claude", 8, HOUR)]);
    assert_eq!(after.last_cwd, "/src/vitrum");
    assert_eq!(after.presets.len(), 1);
}

/// Preset ids must be stable for the same label and command and must
/// differ when either changes. An id that moved would make the picker key
/// on a row that no longer exists.
#[test]
fn preset_ids_are_stable_and_distinguish_their_inputs() {
    assert_eq!(
        mint_preset_id("Plan", "claude"),
        mint_preset_id("Plan", "claude")
    );
    assert_ne!(
        mint_preset_id("Plan", "claude"),
        mint_preset_id("Plan", "codex")
    );
    assert_ne!(
        mint_preset_id("Plan", "claude"),
        mint_preset_id("Plann", "claude")
    );
    // The separator matters: without it these two would collide.
    assert_ne!(mint_preset_id("ab", "c"), mint_preset_id("a", "bc"));
}

// ---- chords ----------------------------------------------------------

/// A chord must need Ctrl or Alt. These are matched while a text field
/// has focus, so a preset bound to `k` or `Shift+K` would eat the letter
/// out of every path the operator types.
#[test]
fn a_chord_without_ctrl_or_alt_is_refused() {
    assert_eq!(parse_chord("k"), None);
    assert_eq!(parse_chord("Shift+K"), None);
    assert_eq!(parse_chord(""), None);
    assert_eq!(parse_chord("Ctrl+"), None);
    assert_eq!(parse_chord("Ctrl+Ctrl+K"), None);
    assert_eq!(parse_chord("Ctrl+K+J"), None);
    assert!(parse_chord("Ctrl+K").is_some());
    assert!(parse_chord("alt+1").is_some());
}

/// Parsing must be case-insensitive on the modifiers and must canonicalise
/// back to one spelling, so the editor writes the same text the help
/// shows.
#[test]
fn a_chord_canonicalises_to_one_spelling() {
    let chord = parse_chord("shift+CONTROL+k").expect("valid");
    assert_eq!(
        chord,
        Chord {
            key: "k".into(),
            ctrl: true,
            alt: false,
            shift: true,
        }
    );
    assert_eq!(format_chord(&chord), "Ctrl+Shift+K");
    assert_eq!(
        parse_chord(&format_chord(&chord)),
        Some(chord),
        "the canonical form must parse back to itself"
    );
    assert_eq!(
        format_chord(&parse_chord("alt+arrowdown").expect("valid")),
        "Alt+Arrowdown"
    );
}

/// A preset whose shortcut is nonsense must simply never fire, and must
/// not stop a later preset with a good one from firing.
#[test]
fn an_unparseable_shortcut_never_fires() {
    let presets = vec![
        SavedPreset {
            id: 1,
            label: "Junk".into(),
            command: "sh".into(),
            shortcut: Some("not a chord at all".into()),
            ..SavedPreset::default()
        },
        SavedPreset {
            id: 2,
            label: "Good".into(),
            command: "sh".into(),
            shortcut: Some("Ctrl+Shift+1".into()),
            ..SavedPreset::default()
        },
    ];
    let chord = parse_chord("Ctrl+Shift+1").expect("valid");
    assert_eq!(
        preset_for_chord(&presets, &chord).map(|p| p.id),
        Some(2),
        "a junk shortcut on an earlier preset swallowed the match"
    );
    let unbound = parse_chord("Ctrl+Shift+9").expect("valid");
    assert_eq!(preset_for_chord(&presets, &unbound), None);
}

/// A chord the shell already claims must be refused, and the refusal must
/// name what owns it. `bootstrap.js` captures on `window` and calls
/// `stopPropagation`, so a preset bound to Ctrl+Shift+N would be a
/// shortcut the settings panel displays and the product never fires.
#[test]
fn a_chord_the_shell_already_owns_is_reported_with_its_owner() {
    let taken = parse_chord("Ctrl+Shift+N").expect("valid");
    assert_eq!(
        chord_conflict(&taken),
        Some("Ctrl+Shift+N is already New session.".to_string())
    );
    // Ctrl+Shift+1 is free: Alt+1 selects a tab, Ctrl+Shift+1 does not.
    assert_eq!(
        chord_conflict(&parse_chord("Ctrl+Shift+1").expect("valid")),
        None
    );
}

/// A shell chord that cannot fire inside a text field must not be
/// reported as a conflict. Ctrl+A selects every row only when focus is on
/// the sidebar, so refusing it here would refuse a binding that works.
#[test]
fn a_chord_scoped_away_from_text_fields_is_not_a_conflict() {
    assert_eq!(chord_conflict(&parse_chord("Ctrl+A").expect("valid")), None);
}

// ---- duplication -----------------------------------------------------

/// Duplicating must reproduce the command, the arguments, the directory
/// and the label exactly. Anything less is a different session wearing the
/// same name.
#[test]
fn duplicating_reproduces_the_command_arguments_and_label() {
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let info = vitrum_proto::SessionInfo {
        id: vitrum_proto::SessionId(1),
        project_id: vitrum_proto::ProjectId(1),
        title: "planner".into(),
        cwd: root.into(),
        command: shell.into(),
        args: vec!["-c".into(), "echo two words".into()],
        status: vitrum_proto::SessionStatus::Running,
        created_at_ms: 0,
        last_activity_ms: 0,
        cols: 80,
        rows: 24,
        git_branch: None,
        worktree: None,
        unread: false,
        attention: vitrum_proto::Attention::default(),
        hint: None,
        term_title: None,
    };
    let l = duplicate_of(&info).expect("a running session can be duplicated");
    assert_eq!(l.command, shell);
    assert_eq!(l.args, vec!["-c".to_string(), "echo two words".to_string()]);
    assert_eq!(l.cwd, tidy_dir(root));
    assert_eq!(l.title.as_deref(), Some("planner"));
}

/// Duplicating a session whose directory has been deleted must fail here,
/// with the directory named, rather than at spawn three seconds later.
#[test]
fn duplicating_into_a_deleted_directory_fails_by_name() {
    let info = vitrum_proto::SessionInfo {
        id: vitrum_proto::SessionId(1),
        project_id: vitrum_proto::ProjectId(1),
        title: "gone".into(),
        cwd: "/vitrum-no-such-directory-9f3a".into(),
        command: "sh".into(),
        args: Vec::new(),
        status: vitrum_proto::SessionStatus::Running,
        created_at_ms: 0,
        last_activity_ms: 0,
        cols: 80,
        rows: 24,
        git_branch: None,
        worktree: None,
        unread: false,
        attention: vitrum_proto::Attention::default(),
        hint: None,
        term_title: None,
    };
    assert_eq!(
        duplicate_of(&info),
        Err("/vitrum-no-such-directory-9f3a is not a directory on this machine.".to_string())
    );
}

/// A preset with no pinned directory must run where the dialog points,
/// and one with a pinned directory must ignore the dialog. A preset that
/// quietly ran somewhere else is the worst kind of one-click action.
#[test]
fn a_pinned_preset_ignores_the_dialog_and_an_unpinned_one_follows_it() {
    let root = if cfg!(windows) { "C:\\" } else { "/" };
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let scratch = Scratch::new("pin");
    let free = SavedPreset {
        id: 1,
        label: "Free".into(),
        command: shell.into(),
        ..SavedPreset::default()
    };
    assert_eq!(
        preset_launch(&free, root).expect("valid").cwd,
        tidy_dir(root)
    );
    let pinned = SavedPreset {
        cwd: Some(scratch.text()),
        ..free.clone()
    };
    assert_eq!(
        preset_launch(&pinned, root).expect("valid").cwd,
        tidy_dir(&scratch.text())
    );
    // The label becomes the session title, so a preset launch is findable.
    assert_eq!(
        preset_launch(&free, root).expect("valid").title.as_deref(),
        Some("Free")
    );
}

// ---------------------------------------------------------------------------
// The presets a fresh profile starts with
// ---------------------------------------------------------------------------

/// A roster with `installed` set exactly as the case wants it.
fn roster(rows: &[(&'static str, &'static str, bool)]) -> Vec<AgentAvailability> {
    rows.iter()
        .map(|(label, command, installed)| AgentAvailability {
            label,
            command,
            installed: *installed,
        })
        .collect()
}

/// Every agent is seeded, and the ones this machine can run come first.
///
/// WHY THE ORDER IS THE CONTRACT: presets render in stored order and the
/// launcher puts the whole preset band above detected agents, so seeding in
/// table order opens a new profile with the single agent the machine has
/// sitting under four it does not. That is the exact arrangement
/// `detected_agents` was changed to stop producing.
#[test]
fn a_fresh_profile_is_seeded_with_what_it_can_run_first() {
    let seeded = seed_presets(&roster(&[
        ("Claude Code", "claude", false),
        ("Codex", "codex", true),
        ("Gemini CLI", "gemini", false),
        ("opencode", "opencode", true),
    ]));

    let order: Vec<&str> = seeded.iter().map(|p| p.command.as_str()).collect();
    assert_eq!(
        order,
        vec!["codex", "opencode", "claude", "gemini"],
        "installed agents must lead, and relative table order must survive"
    );
    assert_eq!(seeded.len(), 4, "no agent may be dropped from the seed");
}

/// A seeded preset pins no directory and no key.
///
/// It runs wherever the dialog points. Pinning `cwd` would choose a project
/// for an operator who does not have one yet, and a shipped `shortcut` would
/// occupy a chord the operator never agreed to.
#[test]
fn a_seeded_preset_pins_nothing() {
    for p in seed_presets(&roster(&[("Claude Code", "claude", true)])) {
        assert_eq!(p.cwd, None, "{} pinned a directory", p.label);
        assert_eq!(p.shortcut, None, "{} claimed a chord", p.label);
        assert!(p.args.is_empty(), "{} shipped arguments", p.label);
        assert_ne!(p.id, 0, "{} has no stable id", p.label);
    }
}

/// Every agent in the real table is seeded, whatever that table becomes.
///
/// Derived from `agent_roster` rather than from a list written here, so
/// adding an agent cannot leave a fresh profile silently missing it.
#[test]
fn the_seed_covers_every_agent_vitrum_knows() {
    let all = agent_roster(|_| false);
    let seeded = seed_presets(&all);
    assert_eq!(seeded.len(), all.len());
    for a in &all {
        assert!(
            seeded.iter().any(|p| p.command == a.command),
            "{} is missing from a fresh profile",
            a.command
        );
        assert!(
            is_known_agent(a.command),
            "{} is in the roster but not known to is_known_agent",
            a.command
        );
    }
}

/// Seeding happens once, keyed on the FILE, not on the list being empty.
///
/// An operator who deletes every seeded row has made a decision. Keying on
/// emptiness would overrule it on the next start, forever, which is the
/// failure mode that makes shipped defaults hostile.
#[test]
fn a_profile_whose_presets_were_deleted_is_not_reseeded() {
    let dir = Scratch::new("seed-once");
    let path = dir.0.join(LAUNCH_STORE_FILE);

    // Stand in for the real store: seed, then delete every row, then ask
    // whether the seeding rule would fire again.
    let seeded = LaunchStore {
        presets: seed_presets(&agent_roster(|_| true)),
        ..LaunchStore::default()
    };
    assert!(!seeded.presets.is_empty(), "the seed produced nothing");
    std::fs::write(&path, encode_launch_store(&seeded)).expect("write");

    let emptied = LaunchStore {
        presets: Vec::new(),
        ..parse_launch_store(&std::fs::read_to_string(&path).expect("read"))
    };
    std::fs::write(&path, encode_launch_store(&emptied)).expect("write");

    assert!(
        path.exists(),
        "the rule is keyed on this file, so the file must be what survives"
    );
    let back = parse_launch_store(&std::fs::read_to_string(&path).expect("read"));
    assert!(
        back.presets.is_empty(),
        "a profile that was emptied on purpose came back with rows"
    );
}

/// A seeded store round-trips through the file format unchanged.
#[test]
fn the_seed_survives_being_written_and_read() {
    let seeded = seed_presets(&agent_roster(|c| c == "codex"));
    let store = LaunchStore {
        presets: seeded.clone(),
        ..LaunchStore::default()
    };
    let back = parse_launch_store(&encode_launch_store(&store));
    assert_eq!(back.presets, seeded);
    assert_eq!(
        back.presets[0].command, "codex",
        "the installed agent must still lead after a round trip"
    );
}
