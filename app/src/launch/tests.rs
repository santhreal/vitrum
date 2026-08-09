use super::*;

/// A bare program name must produce no arguments. An empty `args` vec is
/// what the daemon expects; a vec holding one empty string makes the child
/// see `argv[1] == ""`, which breaks argument parsing in most agents.
#[test]
fn a_bare_command_has_no_arguments() {
    assert_eq!(split_command("claude"), Some(("claude".into(), vec![])));
    assert_eq!(
        split_command("  /bin/bash  "),
        Some(("/bin/bash".into(), vec![]))
    );
}

/// Arguments split on runs of whitespace, not on single spaces. A pasted
/// command line with a tab or a double space would otherwise gain empty
/// arguments the child then has to ignore.
#[test]
fn arguments_split_on_whitespace_runs() {
    assert_eq!(
        split_command("codex  exec\t--full-auto"),
        Some((
            "codex".into(),
            vec!["exec".to_string(), "--full-auto".to_string()]
        ))
    );
}

/// Quotes group, which is the entire reason this is not `split_whitespace`.
/// A project under "My Documents" is the common case on Windows and the
/// one that breaks first.
#[test]
fn quotes_group_an_argument_with_spaces() {
    assert_eq!(
        split_command(r#"claude --add-dir "/home/a b/c" --print"#),
        Some((
            "claude".into(),
            vec![
                "--add-dir".to_string(),
                "/home/a b/c".to_string(),
                "--print".to_string()
            ]
        ))
    );
}

/// A quoted Windows path must survive with its separators intact. This is
/// the case a naive "backslash escapes anything" splitter destroys:
/// `C:\Program Files\agent.exe` becomes `C:Program Filesagent.exe`, and
/// the daemon then reports a spawn failure for a path the user never
/// typed.
#[test]
fn a_quoted_windows_path_keeps_its_separators() {
    assert_eq!(
        split_command(r#""C:\Program Files\agent.exe" --once"#),
        Some((
            r"C:\Program Files\agent.exe".into(),
            vec!["--once".to_string()]
        ))
    );
}

/// An unquoted backslash is a literal backslash, not an escape. Spaces are
/// grouped with quotes instead, which is the only convention that can be
/// right on both Windows and Unix at once.
#[test]
fn an_unquoted_backslash_is_literal() {
    assert_eq!(
        split_command(r"agent -m say\ hi"),
        Some((
            "agent".into(),
            vec!["-m".to_string(), r"say\".to_string(), "hi".to_string()]
        ))
    );
}

/// A backslash before a quote or another backslash still escapes, because
/// there is otherwise no way to pass a literal double quote to an agent,
/// and a prompt containing one is not exotic.
#[test]
fn backslash_escapes_a_quote_or_a_backslash() {
    assert_eq!(
        split_command(r#"agent -m "a \"b\" c""#),
        Some((
            "agent".into(),
            vec!["-m".to_string(), r#"a "b" c"#.to_string()]
        ))
    );
    assert_eq!(
        split_command(r#"agent "back\\slash""#),
        Some(("agent".into(), vec![r"back\slash".to_string()]))
    );
}

/// An explicit empty argument must survive. `agent ""` is how you pass an
/// empty string, and dropping it silently shifts every later argument by
/// one position.
#[test]
fn an_explicitly_empty_argument_is_kept() {
    assert_eq!(
        split_command(r#"agent "" x"#),
        Some(("agent".into(), vec![String::new(), "x".to_string()]))
    );
}

/// Nothing to run must be `None`, never an empty command. A
/// `CreateSession` with an empty `command` reaches the daemon and fails
/// there, one round trip later, with a much worse message.
#[test]
fn an_empty_line_has_no_program() {
    assert_eq!(split_command(""), None);
    assert_eq!(split_command("   \t "), None);
}

/// Defect class: the product offering a shell as a thing to launch.
///
/// vitrum manages coding agents. A suggestion list that ends in `/bin/bash`
/// tells a first-run operator with no agent installed that this is a
/// terminal multiplexer, and any screenshot of the launcher ships that
/// claim (`AGENTS.md`, "Demos show agents, not shell output"). The
/// invariant is at the choke point: NOTHING `command_suggestions` invents
/// may resolve to [`AgentKind::Shell`], for any input. History is exempt
/// by construction — it holds only lines this operator really launched —
/// and is checked here to prove the rule is about what the product offers,
/// not about what an operator may run.
///
/// The shipped table is enumerated at run time, so adding an agent to
/// `AGENTS` puts it under this gate without anyone remembering to.
#[test]
fn no_invented_suggestion_is_a_shell() {
    use vitrum_model::AgentKind;

    let every: Vec<Detected> = AGENTS
        .iter()
        .map(|(label, command)| Detected { label, command })
        .collect();
    let got = command_suggestions(&LaunchStore::default(), &every, "", 1_000, 64);
    assert_eq!(got.len(), AGENTS.len(), "every shipped agent must be offered");
    for s in &got {
        assert_ne!(
            s.source,
            CommandSource::History,
            "{} came from an empty history",
            s.line
        );
        assert_ne!(
            AgentKind::of(&s.line),
            AgentKind::Shell,
            "the launcher invented a shell row: {}",
            s.line
        );
    }

    // An operator who has really launched a shell still gets it back. The
    // gate is on invention, not on what this machine may run.
    let mut store = LaunchStore::default();
    store.history.push(HistoryEntry {
        command: "/bin/bash".into(),
        args: vec![],
        count: 1,
        last_used_ms: 900,
        icon: None,
    });
    let got = command_suggestions(&store, &[], "", 1_000, 64);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].source, CommandSource::History);
    assert_eq!(got[0].line, "/bin/bash");
}

/// Detection must report only what is installed, in table order, and must
/// never invent a name. A greyed-out row for a binary this machine does
/// not have is four wrong answers in front of one right one.
#[test]
fn detection_reports_only_installed_agents_in_table_order() {
    let got = detected_agents();
    let expected: Vec<Detected> = AGENTS
        .iter()
        .filter(|(_, cmd)| on_path(cmd))
        .map(|(label, command)| Detected { label, command })
        .collect();
    assert_eq!(got, expected);
    for d in &got {
        assert!(
            on_path(d.command),
            "{} was offered but is absent",
            d.command
        );
    }
}

/// A real executable on `PATH` must resolve and a made-up one must not.
/// This is the check the dialog's "not on PATH" warning rests on; if it
/// answered true for everything the warning would never appear, and if it
/// answered false for everything every launch would carry a false alarm.
#[test]
fn path_lookup_finds_real_commands_only() {
    let real = if cfg!(windows) { "cmd" } else { "sh" };
    assert!(on_path(real), "{real} must be resolvable");
    assert!(!on_path("vitrum-no-such-command-9f3a"));
    assert!(!on_path(""));
}

/// An absolute path must be checked as a path, not looked up in `PATH`.
/// Otherwise `/usr/bin/env` would be searched for as a directory entry
/// literally named "/usr/bin/env" inside each `PATH` component.
#[test]
fn an_absolute_path_is_checked_directly() {
    if cfg!(unix) {
        assert!(on_path("/bin/sh") || on_path("/usr/bin/sh"));
        assert!(!on_path("/nonexistent/agent"));
    }
}

/// A directory on `PATH` is not a command. Without the file check,
/// `on_path("bin")` would answer true on any machine with a `bin`
/// directory inside a `PATH` component.
#[test]
fn a_directory_is_not_executable() {
    let dir = std::env::temp_dir();
    assert!(dir.is_dir());
    assert!(!is_executable(&dir));
}

/// An empty working directory must be rejected before anything is sent.
/// The daemon would spawn in its own cwd, which is not where the user
/// thinks the agent is running.
#[test]
fn a_missing_directory_is_rejected_by_name() {
    assert_eq!(
        validate("", "sh", ""),
        Err("Pick a project or type a working directory.".to_string())
    );
    assert_eq!(
        validate("   ", "sh", ""),
        Err("Pick a project or type a working directory.".to_string())
    );
}

/// A directory that does not exist must be named in the error. "Invalid
/// path" would leave the user guessing whether they typo'd the path or the
/// daemon is on another machine.
#[test]
fn a_nonexistent_directory_names_itself() {
    assert_eq!(
        validate("/no/such/dir/vitrum-test", "sh", ""),
        Err("/no/such/dir/vitrum-test is not a directory on this machine.".to_string())
    );
}

/// An empty command must be rejected with a message that says what to do.
#[test]
fn an_empty_command_is_rejected_with_guidance() {
    let dir = std::env::temp_dir();
    assert_eq!(
        validate(dir.to_str().unwrap(), "  ", ""),
        Err("Type a command to run, or pick one above.".to_string())
    );
}

/// A valid launch must carry the split command, the trimmed title, and no
/// warning when the program exists.
#[test]
fn a_valid_launch_carries_split_arguments_and_no_warning() {
    let dir = std::env::temp_dir();
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let got = validate(
        dir.to_str().unwrap(),
        &format!("{shell} -c \"echo hi\""),
        "  review  ",
    )
    .expect("valid");
    assert_eq!(got.command, shell);
    assert_eq!(got.args, vec!["-c".to_string(), "echo hi".to_string()]);
    assert_eq!(got.title.as_deref(), Some("review"));
    assert_eq!(got.warning, None);
    // As a path, not as text: Windows' `temp_dir` ends in a separator and
    // `validate` trims it, and `Path` compares components rather than bytes.
    assert_eq!(std::path::Path::new(&got.cwd), dir.as_path());
}

/// A blank title must become `None`, not `Some("")`. An empty title makes
/// the daemon name the session after nothing, and the sidebar row renders
/// as a blank line with a status dot.
#[test]
fn a_blank_title_becomes_none() {
    let dir = std::env::temp_dir();
    let got = validate(dir.to_str().unwrap(), "sh", "   ").expect("valid");
    assert_eq!(got.title, None);
}

/// A command that is not on `PATH` must still be launchable, but must warn
/// first and name the command. Refusing outright would break the case
/// where the daemon has a different environment; saying nothing would turn
/// a typo into a mystery.
#[test]
fn an_unresolvable_command_warns_but_is_allowed() {
    let dir = std::env::temp_dir();
    let got = validate(
        dir.to_str().unwrap(),
        "vitrum-no-such-command-9f3a --go",
        "",
    )
    .expect("still a legal launch");
    assert_eq!(got.command, "vitrum-no-such-command-9f3a");
    assert_eq!(got.args, vec!["--go".to_string()]);
    assert_eq!(
        got.warning.as_deref(),
        Some(
            "vitrum-no-such-command-9f3a is not on this machine's PATH. Launching anyway will fail unless the daemon resolves it differently."
        )
    );
}
