//// A saved preset's chord is a SHORTCUT, not a dialog accelerator.
////
//// The distinction is the whole feature. `SavedPreset::shortcut` existed, the
//// new-session dialog matched it, and the settings panel refused conflicts,
//// which made it look finished. But the only matcher was the dialog's own
//// keydown handler, so firing a preset meant opening the dialog first: two
//// keystrokes to reach the thing whose entire purpose was to be one. The
//// design requires shortcuts that do complex things like open a session
//// in a named folder with a named command, and the shipped behaviour did not.
////
//// `bootstrap.js` matches exactly ONE table. These tests are about what is in
//// it.

use super::*;
use crate::launch::SavedPreset;

fn preset(id: u64, label: &str, shortcut: Option<&str>) -> SavedPreset {
    SavedPreset {
        id,
        label: label.to_string(),
        command: "claude".to_string(),
        args: vec!["--resume".to_string()],
        cwd: Some("/src/vitrum".to_string()),
        shortcut: shortcut.map(str::to_string),
        icon: None,
    }
}

/// A preset with a chord must reach the table the bridge matches.
///
/// This is the bug: without it the chord exists on disk, is shown in
/// settings, is checked for conflicts, and never fires from anywhere but
/// one dialog.
#[test]
fn a_presets_chord_reaches_the_live_table() {
    let prefs = KeyboardPrefs::default();
    let presets = vec![preset(7, "Resume here", Some("ctrl+shift+j"))];
    let live = live_chords(&prefs, &presets);
    let found = live
        .iter()
        .find(|c| c.action == KeyAction::LaunchPreset(7))
        .expect("the preset's chord never reached the table the bridge matches");
    assert_eq!(found.key, "j");
    assert!(found.ctrl, "ctrl was dropped");
    assert_eq!(found.shift, Shift::On, "shift was dropped");
}

/// It must be GLOBAL, or it is still a dialog accelerator.
///
/// `bootstrap.js` refuses to fire a chord whose scope does not match the
/// current surface, so a preset scoped to a layer would do nothing while
/// the operator is typing at an agent, which is where they are.
#[test]
fn a_presets_chord_fires_from_anywhere() {
    let live = live_chords(
        &KeyboardPrefs::default(),
        &[preset(7, "Resume here", Some("ctrl+shift+j"))],
    );
    let found = live
        .iter()
        .find(|c| c.action == KeyAction::LaunchPreset(7))
        .unwrap();
    assert_eq!(
        found.scope,
        Scope::Global,
        "a preset that only fires on one surface is not a shortcut"
    );
}

/// A preset with no chord contributes nothing.
///
/// Most presets have no shortcut. An entry with an empty key would match
/// a keydown whose key is empty, which is how a table starts eating
/// keystrokes nobody bound.
#[test]
fn a_preset_without_a_chord_adds_no_entry() {
    let base = live_chords(&KeyboardPrefs::default(), &[]).len();
    let with = live_chords(&KeyboardPrefs::default(), &[preset(7, "Plain", None)]);
    assert_eq!(
        with.len(),
        base,
        "a chordless preset still took a table slot"
    );
}

/// An unparseable chord contributes nothing, and does not poison the rest.
///
/// The string is operator-authored and survives across versions, so a
/// value this build cannot read is normal rather than exceptional. The
/// preset stays launchable from the dialog; only its accelerator is absent.
#[test]
fn an_unreadable_chord_is_skipped_and_the_others_still_load() {
    let presets = vec![
        preset(1, "Broken", Some("ctrl+shift+")),
        preset(2, "Fine", Some("ctrl+shift+m")),
    ];
    let live = live_chords(&KeyboardPrefs::default(), &presets);
    assert!(
        !live.iter().any(|c| c.action == KeyAction::LaunchPreset(1)),
        "an unparseable chord was admitted to the table"
    );
    assert!(
        live.iter().any(|c| c.action == KeyAction::LaunchPreset(2)),
        "one bad preset took out a good one"
    );
}

/// Presets come last, so one can never shadow a shipped chord.
///
/// `bootstrap.js` takes the FIRST matching entry. Settings refuses a
/// conflicting chord up front, but ordering is what makes that refusal
/// something we do not have to trust: a preset that somehow carried
/// Ctrl+Shift+N still loses to New session.
#[test]
fn a_preset_can_never_outrank_a_built_in_chord() {
    let prefs = KeyboardPrefs::default();
    let builtins = effective_chords(&prefs).len();
    let live = live_chords(&prefs, &[preset(7, "Sneaky", Some("ctrl+shift+n"))]);
    let at = live
        .iter()
        .position(|c| c.action == KeyAction::LaunchPreset(7))
        .expect("the preset is missing entirely");
    assert!(
        at >= builtins,
        "a preset sits at {at}, inside the {builtins} built-ins, so it can shadow one"
    );
    let new_session = live
        .iter()
        .position(|c| c.action == KeyAction::NewSession)
        .unwrap();
    assert!(new_session < at, "the preset would win over New session");
}

/// The table the webview receives must carry the preset's action id.
///
/// Everything above is about the Rust-side vector. This is the wire: if
/// the id does not survive serialisation the bridge sends a string Rust
/// cannot parse, and the chord silently does nothing.
#[test]
fn the_serialised_table_carries_the_presets_action() {
    let live = live_chords(
        &KeyboardPrefs::default(),
        &[preset(42, "Resume here", Some("ctrl+shift+j"))],
    );
    let json = keymap_json(&live);
    assert!(
        json.contains("\"preset:42\""),
        "the wire table has no preset action in it: {json}"
    );
    assert_eq!(
        KeyAction::parse("preset:42"),
        Some(KeyAction::LaunchPreset(42)),
        "the bridge would send a string this build cannot parse back"
    );
}

/// The id round-trips exactly, including values a positional scheme breaks.
///
/// Preset ids are minted and never renumbered, so they are large and
/// sparse. `SelectTab` shares the same `other =>` parse arm and IS range
/// checked, so a preset id must not be quietly clamped by it.
#[test]
fn a_large_preset_id_survives_the_wire() {
    for id in [0u64, 9, 10, 1_000_000, u64::MAX] {
        let wire = KeyAction::LaunchPreset(id).wire();
        assert_eq!(wire, format!("preset:{id}"));
        assert_eq!(KeyAction::parse(&wire), Some(KeyAction::LaunchPreset(id)));
    }
}
