//// Saving a command from the launcher, where the operator just typed it.
////
//// Presets used to be creatable only in `Settings > Presets`, so the moment a
//// command proved worth keeping was the moment you had to leave the surface
//// you were on and retype it, directory included. Almost nobody does that,
//// which is why the launcher's preset band was empty on every machine.
////
//// Every test here targets [`preset_from_typed`], which takes the existing
//// list as an argument and touches no file. The writing half is one `load`,
//// this call, and one `save`.

use super::*;

/// An empty line is refused, and says what to do instead.
///
/// Ctrl+S on an empty field is an ordinary mistake, and a silent no-op
/// there is indistinguishable from the key not being bound.
#[test]
fn an_empty_command_is_refused_with_an_instruction() {
    let err = preset_from_typed("   ", "/src/vitrum", &[]).unwrap_err();
    assert!(
        err.contains("Type a command first"),
        "the refusal does not say what to do: {err}"
    );
}

/// The label is the line that was typed, and the command and arguments are
/// stored split.
///
/// Split, so a preset's meaning cannot change under a future edit to the
/// quoting rules. The label keeps the line because asking for a name is a
/// second question at the exact moment the operator wanted to start work.
#[test]
fn the_label_is_the_line_and_the_command_is_stored_split() {
    let p = preset_from_typed("claude --resume \"my project\"", "/src/vitrum", &[])
        .expect("saves");
    assert_eq!(p.label, "claude --resume \"my project\"");
    assert_eq!(p.command, "claude");
    assert_eq!(p.args, vec!["--resume", "my project"]);
    assert_eq!(p.cwd.as_deref(), Some("/src/vitrum"));
    assert!(p.shortcut.is_none(), "a new preset binds no key by itself");
}

/// The same command in the same directory is refused, not duplicated.
///
/// The launcher lists every saved preset, so a second copy would put two
/// identical rows in the list the operator is looking at.
#[test]
fn saving_the_same_thing_twice_is_refused() {
    let first = preset_from_typed("claude", "/src/vitrum", &[]).expect("saves");
    let err = preset_from_typed("claude", "/src/vitrum", &[first.clone()]).unwrap_err();
    assert!(err.contains("already saved"), "{err}");
}

/// The SAME command in a DIFFERENT directory is a different preset.
///
/// Running one agent in two checkouts is the normal case, and refusing the
/// second would make the feature useless to anybody with more than one
/// repository.
#[test]
fn the_same_command_elsewhere_is_a_separate_preset() {
    let first = preset_from_typed("claude", "/src/vitrum", &[]).expect("saves");
    let second = preset_from_typed("claude", "/src/other", &[first.clone()])
        .expect("a different directory is a different preset");
    assert_ne!(first.cwd, second.cwd);
}

/// The id is stable for one label and line, and differs across them.
///
/// A chord is bound to an id. If saving the same thing twice minted two
/// ids, a rebind would point at a preset the launcher no longer lists.
#[test]
fn a_presets_id_is_derived_from_what_it_is() {
    let a = mint_preset_id("claude", "claude --resume");
    assert_eq!(a, mint_preset_id("claude", "claude --resume"));
    assert_ne!(a, mint_preset_id("claude", "claude --continue"));
    assert_ne!(a, mint_preset_id("codex", "claude --resume"));
}

/// A saved preset round-trips its command line.
///
/// The launcher renders `join_command(command, args)`, so a line that does
/// not survive the split-and-join renames itself the moment it is saved.
#[test]
fn a_saved_line_round_trips_through_the_split() {
    for line in [
        "claude",
        "claude --resume",
        "claude --resume \"my project\"",
        "/bin/bash -l",
    ] {
        let p = preset_from_typed(line, "/src/vitrum", &[]).expect("saves");
        assert_eq!(
            join_command(&p.command, &p.args),
            line,
            "`{line}` renamed itself when saved"
        );
    }
}
