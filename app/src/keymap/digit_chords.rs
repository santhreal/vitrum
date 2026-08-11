//! The chord a keydown means, for chords bound to a DIGIT.
//!
//! The key name for Ctrl+Shift+1 on a US layout is `!`, not `1`. A binding
//! stored as `1` therefore never matches the keystroke it is named after: a
//! shortcut the settings panel displays, the overlay explains, and the product
//! never fires. Digits are the most natural thing to bind a saved command to,
//! so this took out precisely the bindings an operator makes first.
//!
//! The rule used to live in two matchers and only one of them had it, which is
//! why a preset chord worked inside the launcher and did nothing anywhere
//! else. There is one matcher now, [`super::chord_from_event`], and these
//! tests pin its behaviour rather than its text.

use super::chord_from_event;
use crate::launch::{Chord, parse_chord};

/// A top-row digit comes from the physical key, whatever Shift did to the name.
///
/// The shifted names are the US layout's, which is the layout the defect was
/// reported on. The rule does not depend on them: it never looks at the name
/// when the code is a top-row digit, so a layout that produces some other
/// symbol resolves to the same digit.
#[test]
fn a_top_row_digit_resolves_to_the_digit_and_not_to_its_shifted_name() {
    for (digit, shifted) in [
        ('1', "!"),
        ('2', "@"),
        ('3', "#"),
        ('4', "$"),
        ('5', "%"),
        ('6', "^"),
        ('7', "&"),
        ('8', "*"),
        ('9', "("),
        ('0', ")"),
    ] {
        let code = format!("Digit{digit}");
        let got = chord_from_event(shifted, &code, true, false, true);
        assert_eq!(
            got,
            Chord {
                key: digit.to_string(),
                ctrl: true,
                alt: false,
                shift: true,
            },
            "Ctrl+Shift+{digit} arrived as {shifted:?} and did not resolve to \
             the digit it is bound as"
        );
    }
}

/// A binding an operator saved matches the keystroke it names.
///
/// The round trip is the actual contract: `parse_chord` is what stores a
/// preset's shortcut and `chord_from_event` is what a key press resolves to,
/// and the defect was that the two disagreed for exactly one class of key.
#[test]
fn a_saved_digit_binding_equals_the_keystroke_it_was_named_after() {
    let saved = parse_chord("Ctrl+Shift+1").expect("Ctrl+Shift+1 is a valid binding");
    let pressed = chord_from_event("!", "Digit1", true, false, true);
    assert_eq!(
        saved, pressed,
        "the stored binding and the keystroke resolve differently, which is \
         a shortcut that can be saved and can never fire"
    );
}

/// The keypad is a different physical key and keeps its own name.
///
/// Folding `Numpad1` into `1` would bind two keys to one shortcut, and the
/// keypad's digits are already the arrow and editing cluster under NumLock.
#[test]
fn the_keypad_is_not_folded_into_the_top_row() {
    let pressed = chord_from_event("1", "Numpad1", true, false, false);
    assert_eq!(
        pressed.key, "1",
        "the keypad's name is what it resolves to, unchanged"
    );
    let end = chord_from_event("End", "Numpad1", true, false, false);
    assert_eq!(
        end.key, "end",
        "the keypad under NumLock off reported End and the rule renamed it a \
         digit, so Ctrl+End would fire a digit binding"
    );
}

/// Everything that is not a top-row digit comes from the name, lowercased.
///
/// A letter's code is `KeyK` rather than `k`, so taking the code for
/// everything would store every letter binding under a name nothing matches.
#[test]
fn every_other_key_comes_from_its_name() {
    for (name, code, want) in [
        ("K", "KeyK", "k"),
        ("k", "KeyK", "k"),
        ("ArrowDown", "ArrowDown", "arrowdown"),
        ("Escape", "Escape", "escape"),
        ("F1", "F1", "f1"),
        ("/", "Slash", "/"),
    ] {
        assert_eq!(
            chord_from_event(name, code, true, false, false).key,
            want,
            "{name:?} on {code:?} resolved to the wrong binding name"
        );
    }
}

/// The modifiers are carried through untouched.
///
/// Cheap to assert and it is the half a digit-focused rewrite is most likely
/// to drop: a resolver that returned the right key with the modifiers cleared
/// would make every chord match a bare key press.
#[test]
fn the_modifiers_survive_the_rule() {
    for (ctrl, alt, shift) in [
        (true, false, false),
        (false, true, false),
        (true, true, true),
        (false, false, true),
    ] {
        let got = chord_from_event("!", "Digit1", ctrl, alt, shift);
        assert_eq!((got.ctrl, got.alt, got.shift), (ctrl, alt, shift));
    }
}
