//! Command lines that are not command lines.
//!
//! Everything here was found by feeding hostile strings to `split_command` and
//! looking at what came back, not by imagining what might. Each one produced a
//! value the rest of the program would have accepted and could never have run.

use super::*;

/// A lone quote is not a command.
///
/// It parses to one empty word, because a quote pair is a real empty
/// argument and the quote marks the word as present. The result was
/// `Some(("", []))`: a command whose program is the empty string, which
/// `preset_from_typed` would save and which can never launch. The operator
/// would find out the next time they pressed the key they bound to it.
#[test]
fn a_lone_quote_is_not_a_command() {
    assert_eq!(split_command("\""), None);
    assert_eq!(split_command("\"\""), None);
    assert_eq!(split_command("   \"\"   "), None);
}

/// A program containing NUL can never be executed.
///
/// `exec` takes a C string. A NUL in the program fails at spawn no matter
/// what is around it, so it is refused at the parse where the operator is
/// still looking at what they typed.
#[test]
fn a_nul_in_the_program_is_refused() {
    assert_eq!(split_command("\0"), None);
    assert_eq!(split_command("cla\0ude"), None);
}

/// A NUL in an ARGUMENT is left alone, deliberately.
///
/// It is the program that must be executable. An argument is the child's
/// business, and refusing one here would be this parser deciding what a
/// program may be passed.
#[test]
fn a_nul_in_an_argument_is_the_childs_problem() {
    let (p, a) = split_command("claude \0").expect("the program is fine");
    assert_eq!(p, "claude");
    assert_eq!(a, vec!["\0"]);
}

/// Whitespace-only input is nothing, as it always was.
#[test]
fn whitespace_is_not_a_command() {
    for s in ["", " ", "\t", "   \n  ", "\u{a0}"] {
        assert_eq!(split_command(s), None, "{s:?} parsed as a command");
    }
}

/// Ordinary lines still parse, including the awkward ones.
///
/// The refusal must not become "reject anything unusual": quoting exists
/// so a path with a space, or an argument with a quote in it, can be
/// launched at all.
#[test]
fn the_lines_people_actually_type_still_parse() {
    for (line, program, args) in [
        ("claude", "claude", vec![]),
        ("claude --resume", "claude", vec!["--resume"]),
        (
            "claude --resume \"my project\"",
            "claude",
            vec!["--resume", "my project"],
        ),
        ("/usr/bin/env bash -l", "/usr/bin/env", vec!["bash", "-l"]),
        ("\"/opt/my agents/claude\"", "/opt/my agents/claude", vec![]),
        ("claude \"\"", "claude", vec![""]),
    ] {
        let (p, a) = split_command(line).unwrap_or_else(|| panic!("`{line}` refused"));
        assert_eq!(p, program, "`{line}` program");
        assert_eq!(a, args, "`{line}` args");
    }
}

/// A refused line cannot become a saved preset.
///
/// The parse is where this is caught, so the thing that matters is that
/// the caller which persists to disk honours the refusal.
#[test]
fn an_unrunnable_line_never_reaches_the_store() {
    for line in ["\"", "\0", "   "] {
        assert!(
            preset_from_typed(line, "/src/vitrum", &[]).is_err(),
            "`{line}` was accepted as a saveable preset"
        );
    }
}
