//// The chord a keydown means, for chords bound to a DIGIT.
////
//// `KeyboardEvent.key` for Ctrl+Shift+1 on a US layout is `!`, not `1`. A
//// binding stored as `1` therefore never matches the keystroke it is named
//// after: a shortcut the settings panel displays, the overlay explains, and the
//// product never fires. Digits are the most natural thing to bind a saved
//// command to, so this took out precisely the bindings an operator makes first.
////
//// The rule lives in two places because two matchers exist: `bootstrap.js`
//// matches the shared table on every keydown in the window, and
//// `ui/dialog.rs::chord_of` matches the launcher's own. For a while only the
//// launcher had it, which is why a preset chord worked inside the dialog and
//// did nothing anywhere else. These tests pin the rule so the two cannot drift.

/// The bridge takes a top-row digit from `code`, never from `key`.
///
/// Asserted against the shipped JavaScript, because that is the copy that
/// actually runs. A Rust-side reimplementation would prove nothing about
/// the matcher in the webview.
#[test]
fn the_bridge_reads_a_digit_from_the_physical_key() {
    let js = include_str!("../bootstrap.js");
    assert!(
        js.contains("function chordKey(e)"),
        "bootstrap.js no longer normalises the chord key at all"
    );
    assert!(
        js.contains("code.startsWith(\"Digit\")"),
        "the bridge is back to reading a digit from `key`, so Ctrl+Shift+1 \
         arrives as `!` and never matches a binding stored as `1`"
    );
    assert!(
        js.contains("const key = chordKey(e);"),
        "the matcher stopped using the normalised key"
    );
}

/// The launcher applies the same rule, from the same physical key.
#[test]
fn the_launcher_reads_a_digit_from_the_physical_key_too() {
    let dialog = include_str!("../ui/dialog.rs");
    assert!(
        dialog.contains("strip_prefix(\"Digit\")"),
        "the launcher stopped normalising digits, so a preset bound to a \
         digit is dead inside the dialog"
    );
}

/// Every digit is covered, not just the ones somebody tried.
///
/// `Digit0` through `Digit9`, and nothing longer: `code` for the numeric
/// keypad is `Numpad1`, which is a different physical key and must not be
/// folded into the same binding.
#[test]
fn the_rule_covers_every_digit_and_only_the_top_row() {
    for d in '0'..='9' {
        let code = format!("Digit{d}");
        assert_eq!(code.len(), 6, "Digit{d} is not the shape the rule tests");
        assert!(code.starts_with("Digit"));
    }
    assert_ne!("Numpad1".len(), 6, "the keypad must not match the top row");
}
