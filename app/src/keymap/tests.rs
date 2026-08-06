use super::*;
use std::collections::BTreeSet;

/// The acceptance criterion, as a test: no chord may be undiscoverable.
///
/// The table's convention is that a help row documents itself and every
/// help-less chord that follows it, up to the next help row, and that a
/// row covering more than one chord must spell its coverage out with an
/// explicit `keys` override. So a chord is documented when it carries its
/// own row, or when the row governing it is an explicit multi-chord row.
///
/// Without this, adding a binding and forgetting its `help` produces a
/// shortcut a user can only find by reading the source, which is the exact
/// failure this table exists to prevent.
#[test]
fn every_chord_is_documented() {
    let mut governing: Option<(Help, KeyAction)> = None;
    for ch in CHORDS {
        if let Some(h) = ch.help {
            governing = Some((h, ch.action));
            continue;
        }
        let (h, owner) = governing.expect("the first chord must carry its own help row");
        assert!(
            h.keys.is_some(),
            "chord {} is governed by the row for {:?}, which documents only itself",
            ch.rendered(),
            owner
        );
    }
}

/// A multi-chord row must actually cover the chords that follow it: either
/// by naming one literally, or by being a written range that shares the
/// chord's modifier prefix. A row reading "Ctrl+Tab / Ctrl+PageDown" that
/// governed a chord bound to Alt+7 would document the wrong key.
#[test]
fn multi_chord_rows_cover_what_follows_them() {
    let mut governing: Option<&'static str> = None;
    for ch in CHORDS {
        if let Some(h) = ch.help {
            governing = h.keys;
            continue;
        }
        let keys = governing.expect("checked by every_chord_is_documented");
        let rendered = ch.rendered();
        let named = keys.contains(&rendered);
        let prefix = rendered
            .rfind('+')
            .map(|i| &rendered[..=i])
            .unwrap_or(rendered.as_str());
        let ranged = keys.contains(" - ") && keys.contains(prefix);
        assert!(named || ranged, "row {keys:?} does not cover {rendered}");
    }
}

/// An alias row must actually contain the chord it claims to document. A
/// row reading "Ctrl+Tab / Ctrl+PageDown" that sits on a chord bound to
/// Ctrl+Home would be worse than no row at all.
#[test]
fn override_rows_contain_their_own_chord() {
    for ch in CHORDS {
        let Some(h) = ch.help else { continue };
        let Some(keys) = h.keys else { continue };
        assert!(
            keys.contains(&ch.rendered()),
            "overlay row {keys:?} does not contain its own chord {}",
            ch.rendered()
        );
    }
}

/// Every action the bridge can send must round-trip. A typo in either
/// direction makes a key silently do nothing, which is indistinguishable
/// from a broken keyboard.
#[test]
fn actions_round_trip_through_the_wire() {
    for ch in CHORDS {
        let wire = ch.action.wire();
        assert_eq!(
            KeyAction::parse(&wire),
            Some(ch.action),
            "action {wire} does not parse back"
        );
    }
}

/// Two chords must not match the same event, or which one wins depends on
/// table order and changes when a line is moved. Shift::Any collides with
/// both Shift::On and Shift::Off on the same key and modifiers.
#[test]
fn no_two_chords_match_the_same_event() {
    for (i, a) in CHORDS.iter().enumerate() {
        for b in &CHORDS[i + 1..] {
            if a.key != b.key || a.ctrl != b.ctrl || a.alt != b.alt {
                continue;
            }
            let shift_overlaps =
                a.shift == b.shift || a.shift == Shift::Any || b.shift == Shift::Any;
            assert!(
                !shift_overlaps,
                "chords {:?} and {:?} both match key {} ctrl={} alt={}",
                a.action, b.action, a.key, a.ctrl, a.alt
            );
        }
    }
}

/// Single-character keys must be stored lowercase. `KeyboardEvent.key` is
/// "W" when shift is held, so the bridge lowercases before comparing; a
/// table entry of "W" would then never match anything.
#[test]
fn character_keys_are_lowercase() {
    for ch in CHORDS {
        if ch.key.chars().count() != 1 {
            continue;
        }
        assert_eq!(
            ch.key.to_lowercase(),
            ch.key,
            "chord key {:?} is not lowercase",
            ch.key
        );
    }
}

/// Alt+1 through Alt+9 must cover exactly the nine positional slots, in
/// order. A gap here means one tab position is unreachable by keyboard and
/// nothing else notices.
#[test]
fn alt_digits_cover_nine_slots_in_order() {
    let slots: Vec<usize> = CHORDS
        .iter()
        .filter_map(|ch| match ch.action {
            KeyAction::SelectTab(i) => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(slots, (0..TAB_SLOTS).collect::<Vec<_>>());
    for (i, ch) in CHORDS
        .iter()
        .filter(|ch| matches!(ch.action, KeyAction::SelectTab(_)))
        .enumerate()
    {
        assert_eq!(ch.key, (i + 1).to_string(), "Alt+{} is misbound", i + 1);
        assert!(ch.alt && !ch.ctrl, "tab slot {i} is not an Alt chord");
    }
}

/// Out-of-range tab indices must not parse. `tab:9` with nine slots would
/// address a tenth position that no chord can produce and no strip has.
#[test]
fn tab_index_is_bounded() {
    assert_eq!(KeyAction::parse("tab:8"), Some(KeyAction::SelectTab(8)));
    assert_eq!(KeyAction::parse("tab:9"), None);
    assert_eq!(KeyAction::parse("tab:"), None);
    assert_eq!(KeyAction::parse("tab:x"), None);
    assert_eq!(KeyAction::parse("tab:-1"), None);
}

/// Unknown wire strings must be rejected, never defaulted. A bridge that
/// starts sending an action this build does not know about must do
/// nothing, not switch tabs.
#[test]
fn unknown_actions_are_rejected() {
    assert_eq!(KeyAction::parse(""), None);
    assert_eq!(KeyAction::parse("quit"), None);
    assert_eq!(KeyAction::parse("NEXT"), None);
}

/// Escape must be the only `LayerOnly` chord, and it must be the only way
/// Escape is claimed. Claiming Escape unconditionally would break every
/// vim-mode agent running in the terminal.
#[test]
fn escape_is_claimed_only_while_a_layer_is_open() {
    let escapes: Vec<&Chord> = CHORDS.iter().filter(|ch| ch.key == "escape").collect();
    assert_eq!(escapes.len(), 1);
    assert_eq!(escapes[0].scope, Scope::LayerOnly);
    assert_eq!(escapes[0].action, KeyAction::Dismiss);
}

/// Ctrl+K must stay out of the terminal. Inside readline it is
/// kill-to-end-of-line, which agents rely on; stealing it globally would
/// make the shell actively worse than a plain terminal.
#[test]
fn ctrl_k_yields_to_the_terminal() {
    let ch = CHORDS
        .iter()
        .find(|ch| ch.key == "k" && ch.ctrl)
        .expect("Ctrl+K is bound");
    assert_eq!(ch.scope, Scope::NotTerminal);
    assert_eq!(ch.action, KeyAction::FocusSearch);
}

/// A bare `?` must not fire inside any text entry. xterm.js reads keys
/// through a hidden textarea, so `NotTextInput` is what keeps a question
/// mark typed at an agent from opening the help overlay instead.
#[test]
fn bare_question_mark_yields_to_text_entry() {
    let ch = CHORDS
        .iter()
        .find(|ch| ch.key == "?" && !ch.ctrl)
        .expect("bare ? is bound");
    assert_eq!(ch.scope, Scope::NotTextInput);
    assert_eq!(ch.shift, Shift::Any);
}

/// Every group in [`GROUPS`] must have at least one row, and every row's
/// group must be in [`GROUPS`]. An empty section renders as a heading with
/// nothing under it; a missing section hides its chords entirely.
#[test]
fn overlay_sections_are_all_populated() {
    let used: BTreeSet<&str> = help_rows().iter().map(|r| r.group.title()).collect();
    let declared: BTreeSet<&str> = GROUPS.iter().map(|g| g.title()).collect();
    assert_eq!(used, declared);
    for g in GROUPS {
        assert!(
            !help_rows_for(g).is_empty(),
            "overlay section {} is empty",
            g.title()
        );
    }
}

/// Exact rendering of the four modifier shapes. The overlay is the only
/// place a user learns a chord, so "Ctrl+Shift+W" must not drift into
/// "Shift+Ctrl+W" or "ctrl+shift+w" between builds.
#[test]
fn chords_render_in_a_fixed_order() {
    let close = CHORDS
        .iter()
        .find(|ch| ch.action == KeyAction::CloseTab)
        .unwrap();
    assert_eq!(close.rendered(), "Ctrl+Shift+W");
    let alt1 = CHORDS
        .iter()
        .find(|ch| ch.action == KeyAction::SelectTab(0))
        .unwrap();
    assert_eq!(alt1.rendered(), "Alt+1");
    let f1 = CHORDS.iter().find(|ch| ch.key == "f1").unwrap();
    assert_eq!(f1.rendered(), "F1");
    let down = CHORDS
        .iter()
        .find(|ch| ch.action == KeyAction::NextAttention)
        .unwrap();
    assert_eq!(down.rendered(), "Ctrl+Shift+Down");
}

/// The exact overlay text for the switching section, so a reworded row is
/// a deliberate change and not a side effect of editing the table.
///
/// It also locks out the wording regression the tab strip left behind: with
/// no strip on screen, a row reading "Close tab" names a control the
/// operator cannot see.
#[test]
fn switching_section_rows_are_exact() {
    let rows: Vec<(String, &str)> = help_rows_for(Group::Switching)
        .into_iter()
        .map(|r| (r.keys, r.what))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("Ctrl+Tab / Ctrl+PageDown".to_string(), "Next session"),
            (
                "Ctrl+Shift+Tab / Ctrl+Shift+PageUp".to_string(),
                "Previous session"
            ),
            ("Alt+1 - Alt+9".to_string(), "Focus session by position"),
            (
                "Ctrl+Shift+W".to_string(),
                "Stop viewing; the session keeps running"
            ),
        ]
    );
}
#[test]
fn packed_key_chord_bitfield_and_matching() {
    let p1 = PackedKeyChord::pack(true, false, true, false, 1, 42);
    assert!(p1.ctrl());
    assert!(!p1.alt());
    assert!(p1.shift());
    assert!(!p1.meta());

    let parsed = PackedKeyChord::from_str_fast("Ctrl+Shift+x");
    assert!(parsed.ctrl());
    assert!(parsed.shift());
    assert!(!parsed.alt());

    let chord = CHORDS[0];
    let packed = chord.packed();
    assert_eq!(packed.ctrl(), chord.ctrl);
}

#[test]
fn frame_aligned_microtask_debouncer() {
    let mut debouncer = crate::keys::FrameKeyDebouncer::new(16);
    let action = KeyAction::NextRow;

    // First keypress at t=10ms -> processed immediately
    let res1 = debouncer.process(action, 10);
    assert_eq!(res1, Some(crate::keys::DebouncedKeyAction { action, coalesced_repeat_count: 1 }));

    // Rapid repeat keypress at t=15ms (< 16ms budget) -> debounced (coalesced)
    let res2 = debouncer.process(action, 15);
    assert_eq!(res2, None);

    // Keypress at t=30ms (>= 16ms budget) -> processes and returns coalesced count = 2
    let res3 = debouncer.process(action, 30);
    assert_eq!(res3, Some(crate::keys::DebouncedKeyAction { action, coalesced_repeat_count: 2 }));
}
