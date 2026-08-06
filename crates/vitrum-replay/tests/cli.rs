//! The binary's exit-code and stream contract.
//!
//! `vitrum-replay` is documented as exiting 0 on success, 1 when the file cannot be read
//! or replayed, and 2 when the command line is wrong, and as putting nothing on stdout
//! when it fails. A script that pipes `export` into a file relies on all four of those,
//! and none of them are visible to a library test.

use std::io::Write;
use std::process::{Command, Output};

/// A hint sequence, some output, and a second hint, exactly as a PTY would emit them.
const CAPTURE: &[u8] = b"\x1b]7373;working;compiling\x1b\\hello\r\nworld\x1b]7373;ready;done\x07";

/// Run the binary with `args` and hand back its output.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vitrum-replay"))
        .args(args)
        .output()
        .expect("the binary runs")
}

/// Write `CAPTURE` to a uniquely named file and return its path.
fn fixture(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("vitrum-replay-{name}-{}.raw", std::process::id()));
    let mut file = std::fs::File::create(&path).expect("create");
    file.write_all(CAPTURE).expect("write");
    path
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("the process exited normally")
}

/// A usage mistake exits 2 and puts the complaint on stderr, never on stdout.
///
/// The bug this stops: printing usage to stdout, or exiting 1 for it. A caller that
/// distinguishes "your file is broken" from "your command is broken" cannot, and a
/// pipeline receives a usage message as if it were a recording.
#[test]
fn a_usage_mistake_exits_two_with_an_empty_stdout() {
    let cases: [&[&str]; 5] = [
        &[],
        &["frobnicate", "x.raw"],
        &["info"],
        &["info", "--cols"],
        &["info", "--cols", "abc", "x.raw"],
    ];

    for args in cases {
        let output = run(args);
        assert_eq!(code(&output), 2, "exit code for {args:?}");
        assert!(output.stdout.is_empty(), "stdout was written for {args:?}");
        assert!(
            output.stderr.starts_with(b"vitrum-replay: "),
            "stderr for {args:?} did not name the program"
        );
    }
}

/// A file that cannot be read or replayed exits 1, not 2, and writes nothing to stdout.
#[test]
fn a_bad_input_exits_one_with_an_empty_stdout() {
    let missing = std::env::temp_dir().join("vitrum-replay-nothing-here.raw");
    let output = run(&["info", &missing.to_string_lossy()]);

    assert_eq!(code(&output), 1);
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot read"),
        "stderr did not say what failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A seek past the end of the stream is reported, not clamped.
///
/// Clamping would print a screen the caller did not ask for and exit 0, so a script
/// scrubbing to a computed position would silently show the wrong frame.
#[test]
fn a_seek_past_the_end_is_refused_rather_than_clamped() {
    let path = fixture("out-of-range");
    let output = run(&["screen", &path.to_string_lossy(), "--at", "999999"]);

    assert_eq!(code(&output), 1);
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("999999"),
        "the error did not name the seq asked for"
    );
    std::fs::remove_file(path).ok();
}

/// `--help` and `--version` exit 0 and print to stdout, so they can be piped.
#[test]
fn help_and_version_succeed_on_stdout() {
    for flag in ["-h", "--help"] {
        let output = run(&[flag]);
        assert_eq!(code(&output), 0, "exit code for {flag}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    }
    for flag in ["-V", "--version"] {
        let output = run(&[flag]);
        assert_eq!(code(&output), 0, "exit code for {flag}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            concat!("vitrum-replay ", env!("CARGO_PKG_VERSION"))
        );
    }
}

/// The four commands each succeed on a real capture and say something specific.
#[test]
fn every_command_runs_against_a_real_capture() {
    let path = fixture("commands");
    let file = path.to_string_lossy().into_owned();

    let info = run(&["info", &file]);
    assert_eq!(code(&info), 0);
    let text = String::from_utf8_lossy(&info.stdout).into_owned();
    assert!(text.contains("source        raw scrollback"), "{text}");
    assert!(
        text.contains(&format!("bytes         {}", CAPTURE.len())),
        "{text}"
    );
    assert!(text.contains("chapters      2"), "{text}");

    let markers = run(&["markers", &file]);
    assert_eq!(code(&markers), 0);
    let text = String::from_utf8_lossy(&markers.stdout).into_owned();
    assert!(text.contains("working     compiling"), "{text}");
    assert!(text.contains("ready       done"), "{text}");

    let screen = run(&["screen", &file, "--cols", "20", "--rows", "3"]);
    assert_eq!(code(&screen), 0);
    let rows: Vec<String> = String::from_utf8_lossy(&screen.stdout)
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect();
    assert_eq!(rows, vec!["hello", "world", ""]);

    let export = run(&["export", &file, "--title", "a run"]);
    assert_eq!(code(&export), 0);
    let text = String::from_utf8_lossy(&export.stdout).into_owned();
    assert!(text.starts_with("{\"version\":2,\"width\":80,\"height\":24"), "{text}");
    assert!(text.contains("\"title\":\"a run\""), "{text}");

    std::fs::remove_file(path).ok();
}

/// An exported recording feeds straight back in and shows the same screen.
///
/// This is the whole point of `export`, and it crosses the writer, the reader, and the
/// stdin path in one go.
#[test]
fn an_export_reads_back_through_stdin_to_the_same_screen() {
    let path = fixture("round-trip");
    let file = path.to_string_lossy().into_owned();

    let direct = run(&["screen", &file, "--cols", "20", "--rows", "3"]);
    // The recording carries its own geometry, and the header wins on reimport, so the
    // export has to be taken at the size it is compared at.
    let export = run(&["export", &file, "--cols", "20", "--rows", "3"]);
    assert_eq!(code(&export), 0);

    let mut child = Command::new(env!("CARGO_BIN_EXE_vitrum-replay"))
        .args(["screen", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&export.stdout)
        .expect("write");
    let reloaded = child.wait_with_output().expect("wait");

    assert_eq!(code(&reloaded), 0);
    assert_eq!(
        String::from_utf8_lossy(&reloaded.stdout),
        String::from_utf8_lossy(&direct.stdout),
        "the exported recording replays to a different screen"
    );
    std::fs::remove_file(path).ok();
}

/// Chapters come back from an export at the byte they happened at.
///
/// The bug this stops: an export that writes every marker after one big output event, so
/// a reimported recording stacks all its chapters on the last byte.
#[test]
fn exported_chapters_keep_their_positions() {
    let path = fixture("chapters");
    let file = path.to_string_lossy().into_owned();

    let before = run(&["markers", &file]);
    let export = run(&["export", &file]);
    let cast = path.with_extension("cast");
    std::fs::write(&cast, &export.stdout).expect("write");
    let after = run(&["markers", &cast.to_string_lossy()]);

    assert_eq!(code(&after), 0);
    let positions = |output: &Output| -> Vec<String> {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
            .collect()
    };
    assert_eq!(positions(&after), positions(&before));

    std::fs::remove_file(path).ok();
    std::fs::remove_file(cast).ok();
}
