//! `vitrum hint`'s argument grammar and the help that describes it.

use super::*;

fn parse(words: &[&str]) -> Result<HintRequest, String> {
    parse_hint(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
}

fn declare(state: HintState, label: Option<&str>) -> Result<HintRequest, String> {
    Ok(HintRequest::Declare {
        state,
        label: label.map(str::to_string),
    })
}

/// Each state token has to reach the builder as the state it names.
///
/// A parser that mapped `input` to `Working` would produce a valid sequence
/// carrying the wrong claim, which no round-trip test downstream can catch
/// because the bytes are perfectly well formed.
#[test]
fn every_state_token_is_accepted_as_itself() {
    assert_eq!(parse(&["approval"]), declare(HintState::Approval, None));
    assert_eq!(parse(&["input"]), declare(HintState::Input, None));
    assert_eq!(parse(&["working"]), declare(HintState::Working, None));
    assert_eq!(parse(&["ready"]), declare(HintState::Ready, None));
}

/// An unknown state must be refused, never rounded to a known one.
///
/// The wire parser has the same rule for the same reason: a future `paused`
/// declared as `ready` is a badge that says the opposite of the truth.
#[test]
fn an_unknown_state_is_an_error() {
    for bad in ["approve", "Approval", "APPROVAL", "", "idle", "42"] {
        let err = parse(&[bad]).expect_err("{bad} must not parse");
        assert!(err.contains("approval, input, working or ready"), "{err}");
    }
}

/// The label is one argument, and a caller who forgot to quote it must be told
/// so rather than having the first word silently used.
#[test]
fn the_label_is_a_single_argument() {
    assert_eq!(
        parse(&["approval", "run rm -rf build/?"]),
        declare(HintState::Approval, Some("run rm -rf build/?"))
    );
    let err = parse(&["approval", "run", "rm"]).expect_err("unquoted label must fail");
    assert!(err.contains("one argument"), "{err}");
}

/// A label may start with a dash. Reading it as an option would make every
/// "-y to continue" prompt unreportable.
#[test]
fn a_label_may_look_like_an_option() {
    assert_eq!(
        parse(&["input", "--force?"]),
        declare(HintState::Input, Some("--force?"))
    );
}

/// An unknown option before the state is a typo, and must not be taken as one.
#[test]
fn an_unknown_option_is_an_error() {
    let err = parse(&["--verbose", "ready"]).expect_err("must fail");
    assert!(err.contains("unknown option --verbose"), "{err}");
}

/// `--clear` is the documented way back to the observed status, and it must
/// mean `working` with nothing attached.
#[test]
fn clear_declares_working_alone() {
    assert_eq!(parse(&["--clear"]), declare(HintState::Working, None));
    let err = parse(&["--clear", "ready"]).expect_err("must fail");
    assert!(err.contains("no other arguments"), "{err}");
}

/// No arguments is a usage error, not a silent no-op.
#[test]
fn a_missing_state_is_an_error() {
    let err = parse(&[]).expect_err("must fail");
    assert!(err.contains("needs a state"), "{err}");
}

/// Help is asked for, so it is not an error, whichever flag is used and
/// wherever it appears.
#[test]
fn help_wins_over_everything_else() {
    for words in [vec!["-h"], vec!["--help"], vec!["approval", "--help"]] {
        assert_eq!(parse(&words), Ok(HintRequest::Help), "{words:?}");
    }
}

/// The subcommand has to be discoverable from `vitrum --help`.
///
/// Approval and Input are unreachable until an operator knows this command
/// exists, and a feature nobody can find is the finding this closes.
#[test]
fn the_top_level_help_names_the_hint_command() {
    let text = usage();
    assert!(text.contains("hint"), "{text}");
    assert!(text.contains("Approval and Input"), "{text}");
}

/// The subcommand's own help must say what it is for and what it costs to skip.
#[test]
fn the_hint_help_explains_the_sequence_and_the_exit_codes() {
    let text = hint_usage();
    assert!(text.contains("7373"), "the OSC number is the wire contract: {text}");
    assert!(text.contains("--clear"), "{text}");
    assert!(text.contains("2  "), "the failing exit code is not documented: {text}");
    assert!(!text.contains("%%"), "help ships an unrendered escape: {text}");
}
