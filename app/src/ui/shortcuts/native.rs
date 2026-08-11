//! The keyboard reference, as a presented surface.
//!
//! Every row comes from [`crate::ui::settings::effective_help_rows`], which is
//! the shipped table folded with the operator's rebindings and is also what
//! key dispatch matches a press against. Rendering the shipped table instead
//! would advertise a chord that does nothing for every action the operator has
//! moved, and leave the live one documented nowhere.
//!
//! The Terminal section is separate and comes from
//! [`crate::keymap::PANE_CHORDS`]. Those chords reach the pane only because
//! dispatch finds no match for them, so they are documented apart from the
//! rows dispatch owns and the shell never claims one.
//!
//! # Why the sheet is built from a list rather than from the tables directly
//!
//! [`sections`] is what the surface draws, as data. The widget builder walks
//! it and does nothing else, so what the operator reads is checkable on a
//! machine with no display: "the Terminal section is exactly `PANE_CHORDS`"
//! and "every other chord is one dispatch fires on" are assertions about this
//! function rather than hopes about a widget tree.

use std::rc::Rc;

use gtk::prelude::*;

use super::{MODIFIER_NOTE, REBOUND_NOTE};
use crate::keymap::{GROUPS, PANE_CHORDS, PANE_SECTION_TITLE};
use crate::shell::Shell;
use crate::state::KeyboardPrefs;
use crate::ui::settings::effective_help_rows;
use crate::ui::sheet::{self, Sheet};

/// One heading and the chords under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section {
    pub(crate) heading: &'static str,
    /// Chord as it is drawn, and what it does.
    pub(crate) rows: Vec<(String, String)>,
}

/// Everything the sheet says, in the order it says it.
///
/// The shell's own groups in their fixed order, then the pane's. The pane's
/// section is last and comes from a different table on purpose: an entry for
/// one of those chords in the table dispatch matches would take the key away
/// from the pane silently.
pub(crate) fn sections(prefs: &KeyboardPrefs) -> Vec<Section> {
    let live = effective_help_rows(prefs);
    let mut out: Vec<Section> = GROUPS
        .iter()
        .map(|group| Section {
            heading: group.title(),
            rows: live
                .iter()
                .filter(|row| row.group == *group)
                .map(|row| (row.keys.clone(), row.what.to_string()))
                .collect(),
        })
        .collect();
    out.push(Section {
        heading: PANE_SECTION_TITLE,
        rows: PANE_CHORDS
            .iter()
            .map(|row| (row.keys.to_string(), row.what.to_string()))
            .collect(),
    });
    out
}

/// The reference sheet for `prefs`.
pub(crate) fn build(shell: &Shell, prefs: &KeyboardPrefs) -> Rc<Sheet> {
    let panel = sheet::column("rg-sheet__panel");
    panel.pack_start(&sheet::head(shell, "Keyboard"), false, false, 0);

    let keys = sheet::column("rg-keys");
    for section in sections(prefs) {
        let group = sheet::column("rg-keys__group");
        group.pack_start(
            &sheet::label("rg-keys__heading", section.heading),
            false,
            false,
            0,
        );
        for (chord, what) in &section.rows {
            group.pack_start(&entry(chord, what), false, false, 0);
        }
        keys.pack_start(&group, false, false, 0);
    }
    panel.pack_start(&keys, true, true, 0);

    // Only when something has been rebound, so a screenshot of an untouched
    // install is not captioned as though it were customised.
    if !prefs.overrides.is_empty() {
        panel.pack_start(
            &sheet::label("rg-sheet__note", REBOUND_NOTE),
            false,
            false,
            0,
        );
    }
    panel.pack_start(
        &sheet::label("rg-sheet__note", MODIFIER_NOTE),
        false,
        false,
        0,
    );

    Sheet::new(sheet::SHORTCUTS, sheet::DOCUMENT, &panel)
}

/// One documented chord and what it does.
///
/// One chip per alternative rather than one chip holding a slash. A chord chip
/// does not wrap, so a combined "Ctrl+Shift+Tab / Ctrl+Shift+PageUp" could
/// neither wrap nor shrink and drew over the description beside it. Split, the
/// row has a legal break between the chips and each chord stays intact.
fn entry(keys: &str, what: &str) -> gtk::Box {
    let row = sheet::row("rg-keys__row");
    let chords = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    for chord in keys.split(" / ") {
        chords.pack_start(&sheet::label("rg-keys__chord", chord), false, false, 0);
    }
    row.pack_start(&chords, false, false, 0);
    let what = sheet::label("rg-keys__what", what);
    what.set_hexpand(true);
    row.pack_start(&what, true, true, 0);
    row
}

/// How much room the reference wants, in rem.
///
/// Counted off [`sections`], so a chord added to `keymap.rs` moves this number
/// without anybody remembering to.
#[cfg(test)]
pub(crate) fn content(prefs: &KeyboardPrefs) -> (f64, f64) {
    let sections = sections(prefs);
    let lines: usize = sections.iter().map(|s| s.rows.len() + 1).sum();
    // The head, and the two notes under the table.
    (sheet::DOCUMENT.width, 8.0 + lines as f64 * 2.0)
}

#[cfg(test)]
mod tests;
