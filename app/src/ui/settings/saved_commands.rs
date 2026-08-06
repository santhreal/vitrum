//// The saved-command editor, which is the only writer of `launch.json`'s
//// preset list.
////
//// Every test here defends one invariant the new-session dialog is entitled to
//// assume, because it consumes this list and cannot re-validate it: labels are
//// unique and non-empty, ids are unique, a stored shortcut is one the matcher
//// can match, and a stored working directory is either a real string or
//// absent. A refused edit leaves the list byte-identical, so a validation
//// failure can never be a partial write.

use super::*;
use crate::launch::{
    LaunchStore, PresetFault, SavedPreset, encode_launch_store, parse_launch_store, preset_fault,
};

/// A command name no machine has, used where the point is that the lookup
/// fails. Suffixed rather than plausible, so a real binary appearing on a
/// test machine cannot make this pass for the wrong reason.
const ABSENT: &str = "vitrum-no-such-command-9f3a2b";

fn one(label: &str, line: &str) -> (Vec<SavedPreset>, u64) {
    let mut list = Vec::new();
    let id = create(&mut list, label, line).expect("the fixture must be accepted");
    (list, id)
}

/// Locks out: the editor storing two rows with one name, which leaves the
/// picker offering the same word twice with nothing to tell them apart.
/// Case-insensitively, because the picker is read by a person and `Claude`
/// and `claude` are one name to them.
#[test]
fn a_label_already_in_use_is_refused_whatever_its_case() {
    let (mut list, _) = one("Claude", "claude");
    let refusal = create(&mut list, "  claude  ", "codex").unwrap_err();

    assert_eq!(refusal, PresetRefusal::DuplicateLabel("claude".to_string()));
    assert_eq!(list.len(), 1, "the refused row must not have been pushed");
    assert_eq!(list[0].command, "claude");
}

/// Locks out: a refused edit landing half of itself. `revise` validates
/// before it assigns, so the list after a refusal has to be identical, not
/// merely the same length.
#[test]
fn a_refused_edit_changes_nothing_at_all() {
    let mut list = Vec::new();
    create(&mut list, "Claude", "claude --resume").unwrap();
    let second = create(&mut list, "Codex", "codex").unwrap();
    let before = list.clone();

    let refusal = revise(&mut list, second, PresetField::Label, "CLAUDE").unwrap_err();

    assert_eq!(refusal, PresetRefusal::DuplicateLabel("CLAUDE".to_string()));
    assert_eq!(list, before);
}

/// Locks out: the tab writing a shape the store cannot read back, which
/// would present as saved commands that vanish on the next launch. This is
/// the whole persistence contract with `crate::launch`, asserted on exact
/// values rather than on the list being non-empty.
#[test]
fn a_saved_command_survives_the_store_codec_unchanged() {
    let (mut list, id) = one("Claude", "claude --resume \"my project\"");
    revise(&mut list, id, PresetField::Cwd, " /tmp/vitrum ").unwrap();
    revise(&mut list, id, PresetField::Shortcut, "ctrl+shift+k").unwrap();

    let store = LaunchStore {
        presets: list.clone(),
        ..LaunchStore::default()
    };
    let back = parse_launch_store(&encode_launch_store(&store));

    assert_eq!(back.presets, list);
    let row = &back.presets[0];
    assert_eq!(row.label, "Claude");
    assert_eq!(row.command, "claude");
    assert_eq!(
        row.args,
        vec!["--resume".to_string(), "my project".to_string()]
    );
    assert_eq!(row.cwd.as_deref(), Some("/tmp/vitrum"));
    assert_eq!(row.shortcut.as_deref(), Some("Ctrl+Shift+K"));
    assert_eq!(row.id, id);
}

/// Locks out: a preset that cannot run looking exactly like one that can,
/// until the daemon answers with a spawn error three seconds after the
/// click. The fault is named at edit time, against the same PATH the
/// daemon will use, because the daemon is bound to loopback.
#[test]
fn a_command_that_is_not_on_path_is_named_at_edit_time() {
    let (list, _) = one("Absent", ABSENT);

    assert_eq!(
        preset_fault(&list[0]),
        Some(PresetFault::NotOnPath(ABSENT.to_string()))
    );

    let (runnable, _) = one("Shell", "/bin/sh -l");
    assert_eq!(
        preset_fault(&runnable[0]),
        None,
        "an absolute path to a real executable must not be reported as a fault"
    );
}

/// Locks out: storing `Some("")` for the working directory, which is a
/// directory the daemon will refuse to enter. "No opinion" has to be
/// absence, or every preset without a directory becomes a launch failure.
#[test]
fn clearing_the_working_directory_stores_absence_not_an_empty_string() {
    let (mut list, id) = one("Claude", "claude");
    revise(&mut list, id, PresetField::Cwd, "/tmp").unwrap();
    assert_eq!(list[0].cwd.as_deref(), Some("/tmp"));

    revise(&mut list, id, PresetField::Cwd, "   ").unwrap();
    assert_eq!(list[0].cwd, None);
}

/// Locks out: two spellings of one chord in the file, which would make the
/// dialog's matcher fold at match time or miss the binding entirely. The
/// editor stores the canonical form whatever was typed.
#[test]
fn a_shortcut_is_stored_in_one_spelling_however_it_was_typed() {
    let (mut list, id) = one("Claude", "claude");

    revise(&mut list, id, PresetField::Shortcut, "  ctrl+j ").unwrap();
    let first = list[0].shortcut.clone();
    revise(&mut list, id, PresetField::Shortcut, "CTRL+J").unwrap();

    assert_eq!(first, list[0].shortcut);
    assert_eq!(list[0].shortcut.as_deref(), Some("Ctrl+J"));
}

/// Locks out: a chord in the file that nothing can ever match, which is a
/// shortcut the settings tab displays and the dialog ignores.
#[test]
fn a_shortcut_the_matcher_cannot_match_is_refused_rather_than_stored() {
    let (mut list, id) = one("Claude", "claude");

    let refusal = revise(&mut list, id, PresetField::Shortcut, "meta+++").unwrap_err();

    assert_eq!(refusal, PresetRefusal::BadShortcut("meta+++".to_string()));
    assert_eq!(list[0].shortcut, None);
}

/// Locks out: a preset bound over a chord the shell already claims.
/// `bootstrap.js` listens on window in the capture phase and calls
/// stopPropagation, so the dialog's keydown never runs for one of those
/// and the shortcut would be a key this tab displays and the product never
/// fires. Alt+1 is the shell's "focus tab 1".
#[test]
fn a_shortcut_the_shell_already_claims_is_refused_with_the_owner_named() {
    let (mut list, id) = one("Claude", "claude");

    let refusal = revise(&mut list, id, PresetField::Shortcut, "Alt+1").unwrap_err();

    assert_eq!(
        refusal,
        PresetRefusal::ShortcutTaken("Alt+1 is already Focus session by position.".to_string())
    );
    assert_eq!(list[0].shortcut, None);
}

/// Locks out: two saved commands on one chord. The dialog takes the first
/// match in list order, so the second row would be unreachable by keyboard
/// with nothing on screen saying which of the two won. Case and spacing
/// must not hide the collision, so the check runs on the canonical form.
#[test]
fn two_saved_commands_cannot_share_one_chord() {
    let mut list = Vec::new();
    let first = create(&mut list, "Claude", "claude").unwrap();
    let second = create(&mut list, "Codex", "codex").unwrap();
    revise(&mut list, first, PresetField::Shortcut, "Ctrl+Shift+J").unwrap();

    let refusal = revise(&mut list, second, PresetField::Shortcut, " ctrl+shift+j ")
        .expect_err("the same chord in a different spelling must still collide");

    assert_eq!(
        refusal,
        PresetRefusal::ShortcutInUse("Ctrl+Shift+J".to_string(), "Claude".to_string())
    );
    assert_eq!(list[1].shortcut, None);
    assert_eq!(list[0].shortcut.as_deref(), Some("Ctrl+Shift+J"));

    // The row that owns it may re-type it: excluding self is what makes
    // an unrelated edit to the same row possible at all.
    revise(&mut list, first, PresetField::Shortcut, "ctrl+shift+j").unwrap();
    assert_eq!(list[0].shortcut.as_deref(), Some("Ctrl+Shift+J"));
}

/// Locks out: an id minted from the label and command colliding with a row
/// that already holds it, which is the picker launching the wrong command.
/// Reachable by using a label, renaming it, and using it again.
#[test]
fn reusing_a_label_after_renaming_mints_a_free_id() {
    let (mut list, first) = one("Claude", "claude");
    revise(&mut list, first, PresetField::Label, "Claude, old").unwrap();

    let second = create(&mut list, "Claude", "claude").unwrap();

    assert_ne!(first, second);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, first);
    assert_eq!(list[1].id, second);
}

/// Locks out: a reorder arrow at the end of the list that visibly does
/// nothing. The move is refused, which is what the panel disables the
/// button on.
#[test]
fn a_move_past_either_end_is_refused_and_the_order_is_kept() {
    let mut list = Vec::new();
    let a = create(&mut list, "A", "claude").unwrap();
    let b = create(&mut list, "B", "codex").unwrap();

    assert!(!move_by(&mut list, a, -1), "the first row cannot move up");
    assert!(!move_by(&mut list, b, 1), "the last row cannot move down");
    assert_eq!(list.iter().map(|p| p.id).collect::<Vec<_>>(), vec![a, b]);

    assert!(move_by(&mut list, a, 1));
    assert_eq!(list.iter().map(|p| p.id).collect::<Vec<_>>(), vec![b, a]);
}

/// Locks out: an index-keyed edit rewriting the wrong row after a second
/// window deleted one. The edit is keyed by id, so a row that is gone is
/// reported as gone instead of silently hitting its neighbour.
#[test]
fn editing_a_row_that_was_deleted_elsewhere_is_refused_by_name() {
    let mut list = Vec::new();
    let a = create(&mut list, "A", "claude").unwrap();
    let b = create(&mut list, "B", "codex").unwrap();
    assert!(remove(&mut list, a));

    let refusal = revise(&mut list, a, PresetField::Label, "Renamed").unwrap_err();

    assert_eq!(refusal, PresetRefusal::Vanished);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, b);
    assert_eq!(list[0].label, "B");
}

/// Locks out: an empty or unrunnable record reaching the file. A row with
/// no label is invisible in the picker and a row with no command is a
/// button that cannot start anything.
#[test]
fn a_row_with_no_label_or_no_command_never_reaches_the_list() {
    let mut list = Vec::new();

    assert_eq!(
        create(&mut list, "   ", "claude").unwrap_err(),
        PresetRefusal::NoLabel
    );
    assert_eq!(
        create(&mut list, "Claude", "  ").unwrap_err(),
        PresetRefusal::NoCommand
    );
    assert_eq!(
        create(&mut list, "Claude", "\"   \"").unwrap_err(),
        PresetRefusal::NoCommand,
        "a quoted run of spaces is a non-empty line with no program in it"
    );
    assert!(list.is_empty());
}

/// Locks out: a label the picker can only show elided. The limit is
/// counted in characters, so a non-Latin label gets the same allowance.
#[test]
fn a_label_longer_than_the_picker_shows_is_refused_by_length() {
    let mut list = Vec::new();
    let long: String = "\u{3042}".repeat(PRESET_LABEL_MAX + 1);

    let refusal = create(&mut list, &long, "claude").unwrap_err();

    assert_eq!(refusal, PresetRefusal::LabelTooLong(PRESET_LABEL_MAX + 1));
    assert!(list.is_empty());

    let at_limit: String = "\u{3042}".repeat(PRESET_LABEL_MAX);
    assert!(create(&mut list, &at_limit, "claude").is_ok());
}
