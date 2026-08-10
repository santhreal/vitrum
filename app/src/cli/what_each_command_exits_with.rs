//! The exit-code table is one table, and every command tells the truth about
//! which part of it applies to it.
//!
//! Two things go wrong with documented exit codes, and both have happened
//! here. A command grows a new failure and returns a number its `--help` never
//! mentions, so a script's `case` falls through to the default branch. Or a
//! command's help lists a number it can no longer produce, so somebody writes
//! a branch that never runs. Neither shows up in a build or in a manual test,
//! because both look exactly like working software.
//!
//! So the set is not written down twice. Each command declares the codes it
//! returns as a `const`, renders its own `exit status:` block from that
//! declaration through [`vitrum_proto::exit::status_lines`], and the tests here
//! check the declaration against what the command's SOURCE actually returns.
//! Adding a `return Exit::Offline.code()` to a command that never reached the
//! network turns this red until the declaration is updated, which updates the
//! help in the same edit.

use super::*;

/// Every `Exit::` variant a source file mentions, in numeric order.
///
/// Reads the source rather than a list, which is the whole point: a hand
/// maintained list of "codes this command returns" is exactly the thing that
/// goes stale in silence.
///
/// Lines declaring a table are skipped, so the declaration cannot vouch for
/// itself. That matters most in this file's own module: `cli.rs` declares
/// `HINT_EXIT_CODES` for a command implemented in `hint.rs`, and without the
/// skip the top-level parser would appear to return a code it never does.
fn codes_returned_by(src: &str) -> Vec<Exit> {
    let mut found: Vec<Exit> = Vec::new();
    for line in src.lines() {
        if line.contains("EXIT_CODES") {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("Exit::") {
            rest = &rest[at + "Exit::".len()..];
            let end = rest
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(rest.len());
            let name = &rest[..end];
            let code = match name {
                "Ok" => Exit::Ok,
                "Failed" => Exit::Failed,
                "Usage" => Exit::Usage,
                "Unavailable" => Exit::Unavailable,
                "Offline" => Exit::Offline,
                "Corrupt" => Exit::Corrupt,
                // `Exit::ALL`, `Exit::code`, and anything else that is not a
                // variant. Not an error: this scan is looking for which codes
                // are produced, not auditing every mention of the type.
                _ => continue,
            };
            if !found.contains(&code) {
                found.push(code);
            }
        }
    }
    found.sort();
    found
}

/// One command's claim, and where to check it.
struct Command {
    /// What an operator types.
    name: &'static str,
    /// The source implementing it.
    source: &'static str,
    /// What it declares it returns.
    declared: &'static [Exit],
    /// Its `--help`.
    help: String,
}

fn commands() -> Vec<Command> {
    vec![
        Command {
            name: "vitrum",
            // `CliExit` and the option parser together: the parser never names
            // a code directly, it names `CliExit::misuse` or `CliExit::asked`,
            // and those two constructors are where the code is chosen. Stopping
            // at `usage()` leaves out the subcommand tables further down, which
            // belong to `hint`, `icons` and `update`.
            source: include_str!("../cli.rs")
                .split("impl CliExit {")
                .nth(1)
                .and_then(|rest| rest.split("pub(crate) fn usage()").next())
                .expect("cli.rs declares CliExit, then the parser, then usage"),
            declared: EXIT_CODES,
            help: usage(),
        },
        Command {
            name: "vitrum hint",
            source: include_str!("../hint.rs"),
            declared: HINT_EXIT_CODES,
            help: hint_usage(),
        },
        Command {
            name: "vitrum icons",
            source: include_str!("../icons.rs"),
            declared: crate::icons::EXIT_CODES,
            help: crate::icons::icons_usage(),
        },
        Command {
            name: "vitrum update",
            source: include_str!("../update.rs"),
            declared: crate::update::EXIT_CODES,
            help: crate::update::update_usage(),
        },
    ]
}

/// What each command declares is what its code actually returns.
#[test]
fn the_declared_codes_are_the_codes_the_source_returns() {
    for command in commands() {
        let returned = codes_returned_by(command.source);
        let mut declared = command.declared.to_vec();
        declared.sort();
        assert_eq!(
            returned, declared,
            "{} returns {returned:?} and declares {declared:?}",
            command.name
        );
        assert!(
            !returned.is_empty(),
            "{} returns nothing at all, so the scan is broken rather than the \
             command being infallible",
            command.name
        );
    }
}

/// Every code a command can return appears in its own `--help`, with a meaning.
///
/// A number with no meaning beside it is not documentation. The meanings come
/// from the shared table, so `vitrum update` and `vitrum-server` cannot end up
/// describing 3 differently.
#[test]
fn every_help_documents_every_code_it_can_return() {
    for command in commands() {
        assert!(
            command.help.contains("exit status:"),
            "{} has no exit status section:\n{}",
            command.name,
            command.help
        );
        for code in command.declared {
            let line = format!("  {}", code.code());
            assert!(
                command.help.contains(&line),
                "{} can exit {} and its help never says so:\n{}",
                command.name,
                code.code(),
                command.help
            );
            assert!(
                command.help.contains(code.meaning()),
                "{} documents {} with no meaning beside it:\n{}",
                command.name,
                code.code(),
                command.help
            );
        }
    }
}

/// The exit-status block lists exactly the codes the command returns.
///
/// The other half of the contract, and the one a reader gets wrong: a branch
/// written for a code that never arrives is dead and looks handled.
#[test]
fn no_help_claims_a_code_the_command_cannot_return() {
    for command in commands() {
        let block = command
            .help
            .split("exit status:\n")
            .nth(1)
            .expect("every help has the section");
        let listed: Vec<&str> = block
            .lines()
            .filter(|l| l.starts_with("  "))
            .map(|l| l.trim_start().split_whitespace().next().unwrap_or(""))
            .collect();
        let mut codes = command.declared.to_vec();
        codes.sort();
        let expected: Vec<String> = codes.iter().map(|c| c.code().to_string()).collect();
        assert_eq!(
            listed, expected,
            "{} lists the wrong codes:\n{}",
            command.name, command.help
        );
    }
}

/// Help is asked for, so it goes to stdout and exits zero. A wrong flag is not,
/// so it goes to stderr and exits [`Exit::Usage`].
///
/// This is the defect the type exists for. `vitrum --bogus` printed its usage
/// to STDOUT and returned normally, so every caller that branched on the status
/// treated a typo as a successful launch, and every caller reading stdout got a
/// page of help mixed into its data.
#[test]
fn a_wrong_flag_is_a_failure_and_help_is_not() {
    let bad = Options::parse(vec!["--bogus".to_string()]).expect_err("must not parse");
    assert_eq!(bad.exit, Exit::Usage);
    assert_ne!(bad.exit.code(), 0, "a typo exited successfully");
    assert_eq!(
        bad.message.lines().next(),
        Some("vitrum: unknown argument --bogus"),
        "{bad}"
    );

    for asked in [vec!["--help"], vec!["-h"], vec!["--version"], vec!["-V"]] {
        let told = Options::parse(asked.iter().map(|s| s.to_string()))
            .expect_err("these do not produce options");
        assert_eq!(told.exit, Exit::Ok, "{asked:?} was treated as a mistake");
    }
}

/// Every value error on the command line is a usage error, not a silent
/// default and not a generic failure.
///
/// A script that passes `--ui-scale $VAR` with an empty `VAR` has to be able to
/// tell "you typed that wrong" from "the machine could not do it", because only
/// the first is worth failing the script over.
#[test]
fn every_bad_value_is_a_usage_error() {
    let cases: Vec<Vec<&str>> = vec![
        vec!["--server"],
        vec!["--server", "http://127.0.0.1:7737"],
        vec!["--renderer"],
        vec!["--renderer", "vulkan"],
        vec!["--ui-scale"],
        vec!["--ui-scale", "huge"],
        vec!["--ui-scale", "99"],
        vec!["--nope"],
    ];
    for case in cases {
        let told = Options::parse(case.iter().map(|s| s.to_string()))
            .expect_err("every case here is a bad command line");
        assert_eq!(told.exit, Exit::Usage, "{case:?} -> {}", told.message);
        assert!(
            told.message.contains("usage: vitrum"),
            "{case:?} failed without showing how to call it: {}",
            told.message
        );
    }
}
