//! What a key press resolves to, and where it is allowed to fire.
//!
//! # The class these close
//!
//! One table is folded from two halves, the shipped chords and the operator's
//! saved commands, and for a while only one half had a caller. A preset
//! shortcut was stored, displayed in Settings, listed in the overlay, checked
//! for conflicts, and fired nothing. Testing the fold alone does not catch
//! that: the fold was correct and unused.
//!
//! So these drive the resolver the window actually calls, through the same
//! live bus a profile restore publishes on. Dropping either half of the fold,
//! or pointing the resolver back at the shipped table, turns them red.
//!
//! # What they do not catch
//!
//! Whether a key press REACHES the resolver. The pane's toolkit callback and
//! the shell's `onkeydown` are the two callers, and neither can be driven
//! without a display server. [`super::claim_in_pane`] is covered only for the
//! part below the toolkit: the chord it builds and the surface it claims for.

use super::*;

use crate::launch::SavedPreset;
use crate::state::Settings;

/// A chord as it arrives from a key press.
fn chord(key: &str, ctrl: bool, alt: bool, shift: bool) -> Chord {
    Chord {
        key: key.to_string(),
        ctrl,
        alt,
        shift,
    }
}

fn preset(id: u64, shortcut: &str) -> SavedPreset {
    SavedPreset {
        id,
        label: "Resume here".to_string(),
        command: "claude".to_string(),
        args: vec!["--resume".to_string()],
        cwd: Some("/src/vitrum".to_string()),
        shortcut: Some(shortcut.to_string()),
        icon: None,
    }
}

/// A saved command's shortcut fires it, from the pane, with no dialog first.
///
/// **Regression.** The preset half of the fold had no caller outside its own
/// tests. This goes through the live bus and the cached table, which is the
/// path a real key press takes, so it fails if the fold loses presets, if the
/// resolver reads the shipped table instead, or if the subscription is never
/// installed.
#[test]
fn a_saved_commands_shortcut_launches_it_from_inside_the_pane() {
    let _bus = crate::state::live::exclusive();
    watch_chords();
    crate::state::live::publish(&Settings::default());
    crate::state::live::publish_presets(&[preset(7, "ctrl+shift+j")]);

    assert_eq!(
        claim_live(&chord("j", true, false, true), Focus::Terminal, false),
        Some(Claim::Action(KeyAction::LaunchPreset(7))),
        "a preset shortcut did not resolve to launching the preset"
    );

    // And it stops being a shortcut the moment the operator deletes it,
    // rather than living on in a table folded once at startup.
    crate::state::live::publish_presets(&[]);
    assert_eq!(
        claim_live(&chord("j", true, false, true), Focus::Terminal, false),
        None,
        "a deleted preset's chord still fires"
    );
}

/// Nothing printable is ever taken from the agent.
///
/// The pane is where the operator types. A shell that claimed a bare key
/// would eat it, and the operator would see a keystroke go missing rather
/// than a shortcut misfire, which is far harder to report.
#[test]
fn a_bare_key_press_belongs_to_the_agent() {
    let _bus = crate::state::live::exclusive();
    watch_chords();
    crate::state::live::publish(&Settings::default());
    crate::state::live::publish_presets(&[]);

    for c in ('a'..='z').chain('0'..='9') {
        let key = c.to_string();
        for shift in [false, true] {
            assert_eq!(
                claim_live(&chord(&key, false, false, shift), Focus::Terminal, false),
                None,
                "the shell claimed {key:?} with shift {shift}, so the agent \
                 never receives it"
            );
        }
    }
    for named in ["enter", "tab", "backspace", "arrowup", "arrowdown", "escape"] {
        assert_eq!(
            claim_live(&chord(named, false, false, false), Focus::Terminal, false),
            None,
            "the shell claimed a bare {named} inside the pane"
        );
    }
}

/// Only a global chord is taken from the pane, for every chord that ships.
///
/// Derived from the live table at run time rather than from a list here, so a
/// chord added with a scope that steals keys from an agent turns this red
/// instead of shipping.
#[test]
fn the_pane_only_ever_loses_a_global_chord() {
    let _bus = crate::state::live::exclusive();
    watch_chords();
    crate::state::live::publish(&Settings::default());
    crate::state::live::publish_presets(&[]);

    let table = ui::settings::live_chords(&KeyboardPrefs::default(), &[]);
    assert!(!table.is_empty(), "the table is empty, so this checks nothing");
    for entry in &table {
        let pressed = chord(
            &entry.key,
            entry.ctrl,
            entry.alt,
            entry.shift == crate::keymap::Shift::On,
        );
        let claimed = claim_live(&pressed, Focus::Terminal, false).is_some();
        assert_eq!(
            claimed,
            entry.scope == Scope::Global,
            "{} is scoped {:?} and the pane {} it",
            entry.rendered(),
            entry.scope,
            if claimed { "loses" } else { "keeps" }
        );
    }
}

/// Every scope is decided for every surface, and the answers are the policy.
///
/// The matrix is written out because it IS the specification: a scope that
/// silently started answering `true` everywhere would still pass a test that
/// only asked about the case someone had in mind.
#[test]
fn the_scope_matrix_is_what_it_says_it_is() {
    use Focus::{SessionList, Shell, Terminal, TextInput};

    let cases = [
        (Scope::Global, Terminal, false, true),
        (Scope::Global, TextInput, false, true),
        (Scope::Global, SessionList, false, true),
        (Scope::Global, Shell, false, true),
        (Scope::Global, Shell, true, true),
        (Scope::NotTerminal, Terminal, false, false),
        (Scope::NotTerminal, TextInput, false, true),
        (Scope::NotTerminal, SessionList, false, true),
        (Scope::NotTerminal, Shell, false, true),
        (Scope::NotTextInput, Terminal, false, false),
        (Scope::NotTextInput, TextInput, false, false),
        (Scope::NotTextInput, SessionList, false, true),
        (Scope::NotTextInput, Shell, false, true),
        (Scope::LayerOnly, Terminal, false, false),
        (Scope::LayerOnly, Shell, false, false),
        (Scope::LayerOnly, Shell, true, true),
        (Scope::LayerOnly, Terminal, true, true),
        (Scope::SessionList, SessionList, false, true),
        (Scope::SessionList, SessionList, true, false),
        (Scope::SessionList, Shell, false, false),
        (Scope::SessionList, Terminal, false, false),
    ];
    for (scope, focus, layer_open, want) in cases {
        assert_eq!(
            allows(scope, focus, layer_open),
            want,
            "{scope:?} at {focus:?} with layer_open={layer_open}"
        );
    }
}

/// Escape reaches the agent unless something is open over it.
///
/// The one case an operator notices immediately: Escape is how you leave an
/// agent's own prompt, and a shell that ate it would make the product feel
/// broken in the first minute.
#[test]
fn escape_belongs_to_the_agent_until_a_layer_is_open() {
    let _bus = crate::state::live::exclusive();
    watch_chords();
    crate::state::live::publish(&Settings::default());

    let esc = chord("escape", false, false, false);
    assert_eq!(claim_live(&esc, Focus::Terminal, false), None);
    assert_eq!(
        claim_live(&esc, Focus::Shell, true),
        Some(Claim::Action(KeyAction::Dismiss)),
        "Escape did not close the open layer"
    );
}

/// The operator's own binding wins over the shipped chord on the same keys.
///
/// That is what rebinding means. A custom binding that merely ran as well as
/// the built-in would fire two actions from one key press.
#[test]
fn a_custom_binding_takes_the_chord_from_the_built_in() {
    let mut prefs = KeyboardPrefs::default();
    let table = ui::settings::live_chords(&prefs, &[]);
    let built_in = table
        .iter()
        .find(|c| c.action == KeyAction::ToggleSidebar)
        .expect("the sidebar toggle is a shipped chord")
        .clone();
    let pressed = chord(
        &built_in.key,
        built_in.ctrl,
        built_in.alt,
        built_in.shift == crate::keymap::Shift::On,
    );

    assert_eq!(
        claim(&pressed, &prefs, &table, Focus::Shell, false),
        Some(Claim::Action(KeyAction::ToggleSidebar)),
        "the shipped chord does not fire before anything is bound over it"
    );

    prefs.custom = vec![crate::keymap::CustomBinding {
        label: "My list".to_string(),
        chord: crate::launch::format_chord(&pressed),
        steps: vec![crate::keymap::Step::Action {
            action: "newSession".to_string(),
        }],
    }]
    .into();
    assert_eq!(
        claim(&pressed, &prefs, &table, Focus::Shell, false),
        Some(Claim::Custom(pressed.clone())),
        "the operator's binding lost to the chord it was bound over"
    );
}

/// A chord an operator moved fires at its new keys and not at its old ones.
#[test]
fn a_rebound_chord_moves_rather_than_being_added() {
    let mut prefs = KeyboardPrefs::default();
    let before = ui::settings::live_chords(&prefs, &[]);
    let old = before
        .iter()
        .find(|c| c.action == KeyAction::NewSession)
        .expect("New session is a shipped chord")
        .clone();

    ui::settings::set_override(
        &mut prefs,
        KeyAction::NewSession,
        &ui::settings::Binding {
            key: "9".to_string(),
            ctrl: true,
            alt: true,
            shift: true,
        },
    );
    let after = ui::settings::live_chords(&prefs, &[]);

    assert_eq!(
        claim(
            &chord("9", true, true, true),
            &prefs,
            &after,
            Focus::Terminal,
            false
        ),
        Some(Claim::Action(KeyAction::NewSession)),
        "the new chord does not fire"
    );
    let old_press = chord(
        &old.key,
        old.ctrl,
        old.alt,
        old.shift == crate::keymap::Shift::On,
    );
    assert_ne!(
        claim(&old_press, &prefs, &after, Focus::Terminal, false),
        Some(Claim::Action(KeyAction::NewSession)),
        "the chord still fires at the keys it was moved off"
    );
}
