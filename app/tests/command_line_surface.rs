//! What a stranger sees when they get the command line wrong.
//!
//! WHY THIS EXISTS
//!
//! Four surfaces write diagnostics — the option parser, `update`, `hint` and
//! `icons` — and they disagreed about every part of it. `icons` named itself
//! and `update` did not, so half the lines on a shared stderr were
//! unattributable. Each of them printed the WHOLE manual after the sentence
//! naming the mistake, so a mistyped flag produced forty lines and the
//! sentence was off the top of a short terminal before the prompt came back.
//! One of them rendered a Rust range operator at the operator: `--ui-scale 99
//! is outside 1..=3`.
//!
//! None of that is visible from inside the crate: `Options::parse` returns a
//! `CliExit` and every in-crate test asserted on its `message` field. What a
//! person sees is the process, which stream it wrote to and what it exited
//! with, so this runs the binary.
//!
//! WHAT IS PROVED HERE
//!
//! The shape of a diagnostic, at the choke point: for a table of wrong command
//! lines covering all four surfaces, every one names the command, shows how
//! the command is called, points at `--help`, goes to stderr and not stdout,
//! and exits [`Exit::Usage`]. Then the specific defects, one test each.
//!
//! The table is the important part. Each of these was fixed on one surface at
//! a time before, and the next surface kept the old shape.
//!
//! WHAT IS NOT
//!
//! Not the window, which no test here opens. Not `update`'s network paths: an
//! invocation that reaches GitHub is not a unit of this suite.

use std::process::{Command, Output};

/// Exit codes, from `vitrum_proto::exit`. Repeated rather than imported
/// because an integration test links no library target of this crate, and the
/// numbers are the published contract either way.
const OK: i32 = 0;
const FAILED: i32 = 1;
const USAGE: i32 = 2;
const UNAVAILABLE: i32 = 3;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vitrum"))
        .args(args)
        // Nothing here should reach the network, but a machine whose profile
        // selects the nightly channel must not change what a usage error says.
        .env_remove("VITRUM_LOG")
        .output()
        .unwrap_or_else(|e| panic!("running vitrum {args:?}: {e}"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("vitrum was killed by a signal")
}

/// Every wrong command line this program can be given, and the surface it
/// lands on.
///
/// Written out rather than generated, because the point is coverage of the
/// four parsers by a person who listed what a stranger types.
const WRONG: &[(&[&str], &str)] = &[
    (&["bogus"], "vitrum"),
    (&["--bogus"], "vitrum"),
    (&["--server"], "vitrum"),
    (&["--server", "http://127.0.0.1:7737"], "vitrum"),
    (&["--renderer"], "vitrum"),
    (&["--renderer", "vulkan"], "vitrum"),
    (&["--ui-scale"], "vitrum"),
    (&["--ui-scale", "huge"], "vitrum"),
    (&["--ui-scale", "99"], "vitrum"),
    (&["--token-file"], "vitrum"),
    (&["update", "--bogus"], "vitrum update"),
    (&["update", "--channel"], "vitrum update"),
    (&["update", "--channel", "weekly"], "vitrum update"),
    (&["hint"], "vitrum hint"),
    (&["hint", "bogus"], "vitrum hint"),
    (&["hint", "--bogus"], "vitrum hint"),
    (&["hint", "ready", "one", "two"], "vitrum hint"),
    (&["icons"], "vitrum icons"),
    (&["icons", "--bogus"], "vitrum icons"),
    (&["icons", "/src/vitrum", "/src/other"], "vitrum icons"),
];

/// A diagnostic names the command that wrote it.
///
/// `update`, `hint` and the option parser wrote bare sentences: "unknown
/// channel weekly" on a stderr shared with a build script says nothing about
/// who rejected what.
#[test]
fn every_diagnostic_names_the_command_that_wrote_it() {
    for (args, command) in WRONG {
        let out = run(args);
        let text = stderr(&out);
        let first = text.lines().next().unwrap_or_default();
        assert!(
            first.starts_with(&format!("{command}: ")),
            "vitrum {args:?} wrote an unattributable diagnostic: {first:?}"
        );
    }
}

/// A diagnostic is short, shows the synopsis, and points at the manual.
///
/// It used to BE the manual. `vitrum --bogus` wrote forty lines, the last
/// thirty of which described options the operator had not asked about, and on
/// a terminal shorter than that the line naming the mistake was gone.
#[test]
fn a_diagnostic_is_not_the_whole_manual() {
    for (args, command) in WRONG {
        let out = run(args);
        let text = stderr(&out);
        let lines: Vec<&str> = text.lines().collect();

        assert!(
            lines.len() <= 6,
            "vitrum {args:?} answered a mistake with {} lines:\n{text}",
            lines.len()
        );
        assert!(
            lines.iter().any(|l| l.starts_with("usage: ")),
            "vitrum {args:?} failed without showing how to call it:\n{text}"
        );
        assert!(
            text.contains(&format!("Run '{command} --help'")),
            "vitrum {args:?} does not say where the rest of the manual is:\n{text}"
        );
    }
}

/// A mistake is a failure on stderr, and help is output on stdout.
///
/// Both halves. A usage dump on stdout is data in somebody's pipeline, and a
/// zero exit for a typo is a script that carries on as though the window had
/// opened.
#[test]
fn a_mistake_is_a_failure_and_help_is_output() {
    for (args, _) in WRONG {
        let out = run(args);
        assert_eq!(code(&out), USAGE, "vitrum {args:?} exited with the wrong code");
        assert!(
            stdout(&out).is_empty(),
            "vitrum {args:?} wrote a diagnostic to stdout: {}",
            stdout(&out)
        );
    }

    for args in [
        &["--help"][..],
        &["-h"],
        &["--version"],
        &["-V"],
        &["update", "--help"],
        &["hint", "--help"],
        &["icons", "--help"],
    ] {
        let out = run(args);
        assert_eq!(code(&out), OK, "vitrum {args:?} treated a request as a mistake");
        assert!(
            stderr(&out).is_empty(),
            "vitrum {args:?} wrote what was asked for to stderr"
        );
        assert!(!stdout(&out).is_empty(), "vitrum {args:?} answered nothing");
    }
}

/// Nothing printed at an operator is written in Rust.
///
/// `--ui-scale 99 is outside 1..=3` shipped. A range operator is not a
/// sentence, and it does not say what to pass instead.
#[test]
fn a_range_is_not_printed_as_rust_syntax() {
    let out = run(&["--ui-scale", "99"]);
    let text = stderr(&out);
    assert!(
        !text.contains("..="),
        "the bound is printed as a Rust range: {text}"
    );
    assert!(
        text.contains("outside 1 to 3"),
        "the bound is not stated in words: {text}"
    );
    assert!(
        text.contains("auto"),
        "the message does not say what to pass instead: {text}"
    );
}

/// A mistyped subcommand is answered by naming the subcommands, in the
/// diagnostic itself.
///
/// They were named thirty lines down a usage dump, under a `commands:`
/// heading, below every option. Somebody who typed `vitrum udpate` had the
/// answer on screen and no reason to read that far.
#[test]
fn a_mistyped_command_is_answered_with_the_commands() {
    let out = run(&["udpate"]);
    let text = stderr(&out);
    assert!(
        text.contains("udpate"),
        "the diagnostic does not repeat what was typed: {text}"
    );
    let synopsis: String = text
        .lines()
        .filter(|l| l.starts_with("usage: ") || l.starts_with("       vitrum "))
        .collect();
    for command in ["update", "hint", "icons"] {
        assert!(
            synopsis.contains(command),
            "the synopsis does not name the {command} command: {text}"
        );
    }
}

/// A destination that is not ready is [`UNAVAILABLE`], not a flat failure.
///
/// `vitrum icons <a file>` exited 1, which tells an installer script to stop.
/// The request was right and one argument was wrong, which is the same class
/// as a directory that does not exist or cannot be written, and every other
/// member of that class already exited 3.
#[test]
fn icons_into_something_that_is_not_a_directory_is_unavailable() {
    let scratch = std::env::temp_dir().join(format!("vitrum-icons-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("creating the scratch directory");
    let file = scratch.join("not-a-directory");
    std::fs::write(&file, b"x").expect("writing the file");

    let out = run(&["icons", file.to_str().expect("a UTF-8 temp path")]);
    let text = stderr(&out);
    assert_eq!(
        code(&out),
        UNAVAILABLE,
        "a destination that is not a directory was reported as a failure of the write: {text}"
    );
    assert!(
        text.starts_with("vitrum icons: "),
        "the failure does not name the command: {text}"
    );
    assert!(
        text.contains("Nothing was left behind"),
        "the failure does not say whether it half-wrote a set: {text}"
    );
    assert_ne!(code(&out), FAILED);

    std::fs::remove_dir_all(&scratch).ok();
}

/// A directory that cannot be written is the same class, and the icon set is
/// idempotent when it can be.
///
/// Both in one test because they share the scratch directory and the second
/// depends on the first having produced a complete set.
#[cfg(unix)]
#[test]
fn icons_is_idempotent_and_a_read_only_directory_is_unavailable() {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    let scratch = std::env::temp_dir().join(format!("vitrum-icons-ro-{}", std::process::id()));
    std::fs::remove_dir_all(&scratch).ok();
    let good = scratch.join("good");
    std::fs::create_dir_all(&good).expect("creating the scratch directory");

    let first = run(&["icons", good.to_str().expect("a UTF-8 temp path")]);
    assert_eq!(code(&first), OK, "{}", stderr(&first));
    let listing = stdout(&first);
    let written: Vec<&str> = listing.lines().collect();
    assert!(!written.is_empty(), "a successful run listed no files");
    let digest = |paths: &[&str]| -> Vec<(String, Vec<u8>)> {
        paths
            .iter()
            .map(|p| {
                (
                    (*p).to_string(),
                    std::fs::read(Path::new(p)).expect("reading a written icon"),
                )
            })
            .collect()
    };
    let before = digest(&written);

    // Running it again over a complete set writes the same bytes and says the
    // same thing. An installer that runs twice must not produce a second
    // answer.
    let second = run(&["icons", good.to_str().expect("a UTF-8 temp path")]);
    assert_eq!(code(&second), OK, "{}", stderr(&second));
    assert_eq!(stdout(&second), stdout(&first));
    assert_eq!(before, digest(&written), "a second run rewrote the set");

    // A partially written set is completed rather than refused.
    std::fs::remove_file(Path::new(written[0])).expect("removing one icon");
    let third = run(&["icons", good.to_str().expect("a UTF-8 temp path")]);
    assert_eq!(code(&third), OK, "{}", stderr(&third));
    assert!(
        Path::new(written[0]).exists(),
        "a missing icon was not written back"
    );

    let readonly = scratch.join("readonly");
    std::fs::create_dir_all(&readonly).expect("creating the read-only directory");
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500))
        .expect("making it read-only");
    let refused = run(&["icons", readonly.to_str().expect("a UTF-8 temp path")]);

    // Root ignores the mode bits, and the assertion would then prove nothing.
    if code(&refused) != OK {
        assert_eq!(
            code(&refused),
            UNAVAILABLE,
            "a directory that cannot be written was not reported as a destination problem: {}",
            stderr(&refused)
        );
        assert!(
            stderr(&refused).contains("Nothing was left behind"),
            "{}",
            stderr(&refused)
        );
    }

    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700)).ok();
    std::fs::remove_dir_all(&scratch).ok();
}

/// The version is the crate version and nothing else, on stdout, exit zero.
///
/// It is what an operator filing a report pastes, and what a script greps.
#[test]
fn the_version_is_one_line() {
    for flag in ["--version", "-V"] {
        let out = run(&[flag]);
        assert_eq!(code(&out), OK);
        let text = stdout(&out);
        assert_eq!(text.lines().count(), 1, "{flag} printed more than a version");
        assert!(
            text.starts_with("vitrum "),
            "{flag} does not name the program: {text}"
        );
    }
}

/// Every subcommand's help documents the exit codes it can produce, and the
/// help of the program itself points at each subcommand's.
///
/// A number with no meaning beside it is not documentation, and a subcommand
/// nobody can find is a feature nobody can reach.
#[test]
fn each_help_leads_somewhere() {
    let top = stdout(&run(&["--help"]));
    for command in ["update", "hint", "icons"] {
        assert!(
            top.contains(command),
            "the top-level help does not mention {command}"
        );
        let help = stdout(&run(&[command, "--help"]));
        assert!(
            help.contains("exit status:"),
            "vitrum {command} --help documents no exit codes"
        );
        assert!(
            help.contains(&format!("usage: vitrum {command}")),
            "vitrum {command} --help does not show how it is called"
        );
    }
}
