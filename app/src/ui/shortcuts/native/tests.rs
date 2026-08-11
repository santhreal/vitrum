//! What the reference sheet says, and that the window edge never takes any of
//! it away.

use super::*;
use crate::keymap::KeyAction;
use crate::ui::settings::{Binding, live_chords, set_override};

/// **The fit rule for this surface.** The reference is the tallest thing this
/// module presents: every documented chord in the product, on one sheet. It
/// still fits a window smaller than itself, on every axis, down to the frame a
/// workspace switch hands a client.
#[test]
fn the_whole_reference_fits_a_window_smaller_than_it_is() {
    let prefs = KeyboardPrefs::default();
    sheet::assert_fits(sheet::SHORTCUTS, sheet::DOCUMENT, content(&prefs));
}

/// **The Terminal section is exactly the pane's table.** Not a subset, not a
/// superset, and in the table's own order. The section exists to tell an
/// operator which keys the shell will not take, so a chord missing from it is
/// a key the operator will believe the shell has claimed.
#[test]
fn the_terminal_section_is_the_pane_table_and_nothing_else() {
    let sections = sections(&KeyboardPrefs::default());
    let terminal = sections.last().expect("the sheet has sections");
    assert_eq!(terminal.heading, PANE_SECTION_TITLE);
    let drawn: Vec<&str> = terminal.rows.iter().map(|(keys, _)| keys.as_str()).collect();
    let table: Vec<&str> = PANE_CHORDS.iter().map(|row| row.keys).collect();
    assert_eq!(drawn, table);
}

/// **The documented chord is the chord that fires.** Every row outside the
/// Terminal section names a chord that is genuinely in the table dispatch
/// matches against, after rebinding.
///
/// Positional rows are one row for nine chords written as a range, are
/// excluded from rebinding for that reason, and have nothing to check.
#[test]
fn every_drawn_chord_is_one_dispatch_fires_on() {
    let mut prefs = KeyboardPrefs::default();
    set_override(
        &mut prefs,
        KeyAction::NewSession,
        &Binding {
            key: "9".to_string(),
            ctrl: true,
            alt: true,
            shift: true,
        },
    );
    let live = live_chords(&prefs, &[]);
    let sections = sections(&prefs);
    for section in sections.iter().take(sections.len() - 1) {
        for (keys, what) in &section.rows {
            if keys.contains(" - ") {
                continue;
            }
            for alternative in keys.split(" / ") {
                assert!(
                    live.iter().any(|chord| chord.rendered() == alternative),
                    "the sheet documents {alternative} for \"{what}\" and no chord fires on it"
                );
            }
        }
    }
    assert!(
        sections
            .iter()
            .any(|s| s.rows.iter().any(|(keys, _)| keys == "Ctrl+Alt+Shift+9")),
        "the rebound chord never reached the sheet"
    );
}

/// A section with no rows would paint a heading over blank space, which reads
/// as a rendering fault rather than as an empty category.
#[test]
fn no_section_is_a_heading_over_nothing() {
    for section in sections(&KeyboardPrefs::default()) {
        assert!(
            !section.rows.is_empty(),
            "the {} section has no rows",
            section.heading
        );
    }
}

/// Rebinding changes what is drawn, so the sheet cannot be a static picture of
/// the shipped table. Without this, every other assertion here would still
/// pass on a sheet rendered from `keymap::CHORDS`.
#[test]
fn rebinding_changes_what_the_sheet_draws() {
    let before = sections(&KeyboardPrefs::default());
    let mut prefs = KeyboardPrefs::default();
    set_override(
        &mut prefs,
        KeyAction::ToggleSidebar,
        &Binding {
            key: "j".to_string(),
            ctrl: true,
            alt: true,
            shift: false,
        },
    );
    let after = sections(&prefs);
    assert_ne!(before, after);
    assert!(
        after
            .iter()
            .any(|s| s.rows.iter().any(|(keys, _)| keys == "Ctrl+Alt+J")),
        "the new chord is not documented"
    );
    assert!(
        !after
            .iter()
            .any(|s| s.rows.iter().any(|(keys, _)| keys == "Ctrl+B")),
        "the old chord is still advertised as live"
    );
}

/// A longer table is a taller sheet. A content measurement that ignored its
/// own rows would make the fit test above assert nothing.
#[test]
fn the_height_the_sheet_asks_for_follows_its_rows() {
    let prefs = KeyboardPrefs::default();
    let rows: usize = sections(&prefs).iter().map(|s| s.rows.len()).sum();
    assert!(rows > 0);
    assert!(content(&prefs).1 > rows as f64);
}
