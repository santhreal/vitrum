//! What `vitrum hint` writes, checked with the parser that has to read it.
//!
//! Every assertion here round-trips through [`vitrum_model::hint::HintParser`]
//! rather than comparing strings, because a string comparison would pass on a
//! sequence the daemon rejects. That is exactly the failure this feature had:
//! a parser and a transport that both worked, and no producer.

use super::*;

use vitrum_model::hint::{HintDeclaration, HintParser};

/// Parse `bytes` the way the daemon does, one byte at a time.
fn read_back(bytes: &str) -> Vec<HintDeclaration> {
    let mut parser = HintParser::new();
    let mut out = Vec::new();
    for byte in bytes.as_bytes() {
        parser.feed(&[*byte], &mut out);
    }
    assert!(!parser.is_mid_sequence(), "sequence left the parser mid-flight");
    assert_eq!(parser.rejected(), 0, "the parser rejected our own output");
    out
}

fn only(bytes: &str) -> HintDeclaration {
    let mut parsed = read_back(bytes);
    assert_eq!(parsed.len(), 1, "expected exactly one declaration");
    parsed.pop().expect("one declaration")
}

/// The whole point of the module: bytes the real parser accepts.
///
/// A builder that emitted `ESC]7373:approval` or the wrong OSC number would
/// look perfectly reasonable in review and produce a sidebar that never
/// changes, because a rejected hint is indistinguishable from no hint.
#[test]
fn every_state_round_trips_through_the_real_parser() {
    for (state, built) in [
        (HintState::Approval, sequence(HintState::Approval, None)),
        (HintState::Input, sequence(HintState::Input, None)),
        (HintState::Working, sequence(HintState::Working, None)),
        (HintState::Ready, sequence(HintState::Ready, None)),
    ] {
        assert_eq!(
            only(&built),
            HintDeclaration { state, label: None },
            "{state:?} did not survive the parser"
        );
    }
}

/// The label is the half no observation could supply, so it has to arrive
/// intact, including the punctuation an approval question is made of.
#[test]
fn a_label_round_trips_verbatim() {
    for label in [
        "run `rm -rf build/`?",
        "overwrite src/main.rs; then push",
        "which file? a, b or c",
        "réécrire le fichier",
    ] {
        assert_eq!(
            only(&sequence(HintState::Approval, Some(label))),
            HintDeclaration {
                state: HintState::Approval,
                label: Some(label.to_string()),
            },
        );
    }
}

/// A label carrying a control byte must not take the whole hint down with it.
///
/// The parser drops any payload containing a C0 byte, and a label is agent
/// output: a stray newline or ESC in it is ordinary. Emitting it raw would
/// silently lose the state, which is the one thing that must never happen.
#[test]
fn control_characters_in_a_label_do_not_lose_the_declaration() {
    let parsed = only(&sequence(HintState::Input, Some("first line\nsecond\tline\x1b[0m")));
    assert_eq!(parsed.state, HintState::Input);
    assert_eq!(parsed.label.as_deref(), Some("first line second line [0m"));
}

/// A label that is only whitespace or control bytes must leave no separator.
///
/// An empty trailing field parses as no label anyway, but emitting one is a
/// byte of noise on every prompt redraw and invites a future reader to think
/// an empty label is a distinct thing from an absent one.
#[test]
fn an_empty_label_is_omitted_entirely() {
    for label in ["", "   ", "\n\t"] {
        let built = sequence(HintState::Ready, Some(label));
        assert!(
            !built.contains("ready;"),
            "an empty label still emitted a separator: {built:?}"
        );
        assert_eq!(only(&built).label, None);
    }
}

/// A long label must be truncated by us, not abandoned by the parser.
///
/// The parser drops any payload over `MAX_SEQUENCE_BYTES`, and that limit is
/// in bytes while its own label cap is in characters. A builder that respected
/// only the character cap would emit a 480-byte payload for 120 emoji and lose
/// the state completely.
#[test]
fn an_over_long_label_still_parses() {
    for label in ["x".repeat(4000), "é".repeat(4000), "🙂".repeat(4000)] {
        let built = sequence(HintState::Approval, Some(&label));
        let parsed = only(&built);
        assert_eq!(parsed.state, HintState::Approval);
        let kept = parsed.label.expect("a truncated label is still a label");
        assert!(kept.chars().count() <= MAX_LABEL_CHARS, "{} chars", kept.chars().count());
        // The parser buffers the payload only: `ESC ]` and `ESC \` are two
        // bytes each and never reach the buffer.
        let payload = built.len() - 4;
        assert!(
            payload <= MAX_SEQUENCE_BYTES,
            "the payload must fit the parser's buffer, got {payload}"
        );
        assert!(label.starts_with(&kept), "truncation changed the text");
    }
}

/// The terminator must be ST, never BEL.
///
/// BEL is also how a program asks for the operator. The daemon knows a hint's
/// own BEL is not a bell, but every other terminal on the machine does not,
/// and a prompt hook that beeps once per redraw would be uninstalled the same
/// day.
#[test]
fn the_terminator_is_st_not_bel() {
    let built = sequence(HintState::Working, Some("compiling"));
    assert!(built.ends_with("\u{1b}\\"), "{built:?}");
    assert!(!built.contains('\u{7}'), "{built:?}");
}

/// The tokens are the wire format. `HintState`'s serde names are not.
///
/// If someone renames a serde variant and this module follows, hints stop
/// parsing; if they rename it and this module does not, nothing breaks. This
/// pins the direction that matters.
#[test]
fn every_token_parses_back_to_the_state_it_names() {
    for state in [
        HintState::Approval,
        HintState::Input,
        HintState::Working,
        HintState::Ready,
    ] {
        assert_eq!(HintState::parse(token(state)), Some(state));
    }
}

/// `--clear` must declare the one state the resolver retires by itself.
///
/// `ready` and the two blocking states are all pinned until something else
/// replaces them, so clearing with any of those would leave the row asserting
/// a hint forever rather than handing it back to observation.
#[test]
fn clearing_declares_working_with_no_label() {
    assert_eq!(
        only(&sequence(HintState::Working, None)),
        HintDeclaration {
            state: HintState::Working,
            label: None,
        }
    );
}

/// Two declarations in one stream must both arrive.
///
/// A prompt command emits one per redraw, so they arrive back to back with
/// ordinary output between them. A builder that left the parser mid-sequence
/// would swallow the next one.
#[test]
fn back_to_back_declarations_both_parse() {
    let stream = format!("{}some output\n{}", sequence(HintState::Working, Some("build")), sequence(HintState::Ready, None));
    assert_eq!(
        read_back(&stream),
        vec![
            HintDeclaration {
                state: HintState::Working,
                label: Some("build".to_string()),
            },
            HintDeclaration {
                state: HintState::Ready,
                label: None,
            },
        ]
    );
}

/// Success writes the sequence and only the sequence.
///
/// This is called from `$(...)` in a prompt command, where a stray newline or
/// a progress line would land in the shell's prompt string.
#[test]
fn the_command_writes_exactly_the_sequence() {
    let mut out = Vec::new();
    let code = hint_command(&args(&["approval", "may I push?"]), &mut out);
    assert_eq!(code, 0);
    assert_eq!(
        String::from_utf8(out).expect("utf-8"),
        sequence(HintState::Approval, Some("may I push?"))
    );
}

/// Bad usage exits 2 and writes nothing at all to stdout.
///
/// Both halves are load-bearing. A script branches on the code, and a partial
/// write would corrupt the pipeline the command was called from. Printing the
/// usage text to stdout on error would put a page of help inside the caller's
/// prompt.
#[test]
fn bad_usage_exits_two_and_writes_nothing() {
    for words in [
        vec![],
        vec!["approvel"],
        vec!["--nope"],
        vec!["ready", "one", "two"],
        vec!["--clear", "ready"],
        vec!["Approval"],
        vec![""],
    ] {
        let mut out = Vec::new();
        let code = hint_command(&args(&words), &mut out);
        assert_eq!(code, 2, "{words:?} should be a usage error");
        assert!(out.is_empty(), "{words:?} wrote {out:?} to stdout");
    }
}

/// Help is output, not a diagnostic, and succeeds.
#[test]
fn help_exits_zero_and_names_the_states() {
    for flag in ["-h", "--help"] {
        let mut out = Vec::new();
        assert_eq!(hint_command(&args(&[flag]), &mut out), 0);
        let text = String::from_utf8(out).expect("utf-8");
        for state in ["approval", "input", "working", "ready"] {
            assert!(text.contains(state), "help does not mention {state}");
        }
    }
}

pub(super) fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}
