//! The saved presets band, as GTK chips.
//!
//! The rule this band exists for is in the module above it: a command the
//! operator deliberately saved is shown unconditionally, in the order they
//! saved it, rather than made to compete for one of nine ranked rows.
//!
//! Chips wrap across the width, which is what a [`gtk::FlowBox`] does and what
//! a column of rows does not. Nine presets in a column would push the ranked
//! list, which is still the primary surface, off the bottom of the sheet.
//!
//! Validation stays on the click. [`launch::preset_fault`] is a `stat` and a
//! `PATH` walk, and this band is rebuilt whenever the launcher's query empties,
//! so validating while building would put both back on the typing path the
//! launcher spent a rewrite getting them off.

use gtk::prelude::*;

use crate::launch::{self, SavedPreset};
use crate::shell::Shell;
use crate::ui::sheet;
use crate::ui::{glyph, icons};

/// Build the band, or nothing at all when there are no presets.
///
/// `None` rather than an empty box with a heading: a heading that teaches
/// presets exist while offering no way to make one is a dead end, and the run
/// field's own Save control is what teaches that at the moment there is
/// something worth saving.
pub(crate) fn band(shell: &Shell, presets: &[SavedPreset], here: &str) -> Option<gtk::Box> {
    if presets.is_empty() {
        return None;
    }
    let band = sheet::column("rg-presets");
    band.pack_start(&sheet::label("rg-presets__head", "Saved"), false, false, 0);

    let note = sheet::label("rg-presets__note", "");
    note.set_no_show_all(true);

    let list = gtk::FlowBox::new();
    list.style_context().add_class("rg-presets__list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_row_spacing(0);
    list.set_column_spacing(0);
    for preset in presets {
        list.add(&chip(shell, preset, here, &note));
    }
    band.pack_start(&list, false, false, 0);
    band.pack_start(&note, false, false, 0);
    Some(band)
}

/// One preset, as the control that starts it.
fn chip(shell: &Shell, preset: &SavedPreset, here: &str, note: &gtk::Label) -> gtk::Button {
    let line = launch::join_command(&preset.command, &preset.args);
    let icon = *icons::resolve(preset.icon.as_deref(), &line);

    let body = sheet::row("rg-presets__chip");
    body.pack_start(&glyph::mark(icon.stroke, icon.fill, "rg-presets__icon"), false, false, 0);
    body.pack_start(&sheet::label("rg-presets__text", &preset.label), false, false, 0);
    if let Some(keys) = &preset.shortcut {
        body.pack_end(&sheet::label("rg-presets__chord", keys), false, false, 0);
    }

    let button = gtk::Button::new();
    button.add(&body);
    // The chip is a short label by design, so the whole truth lives here: the
    // exact line that will run, or the reason it will not.
    button.set_tooltip_text(Some(&crate::ui::dialog::preset_tip(preset)));

    let shell = shell.clone();
    let taken = preset.clone();
    let here = here.to_string();
    let note = note.clone();
    button.connect_clicked(move |_| match launch::preset_fault(&taken) {
        Some(fault) => say(&note, &fault.sentence()),
        None => match launch::preset_launch(&taken, &here) {
            Ok(l) => {
                say(&note, "");
                crate::ui::dialog::native::go(&shell, l);
            }
            Err(why) => say(&note, &why),
        },
    });
    button
}

/// Show `text` under the band, or take the line away when there is nothing to
/// say.
///
/// Hidden rather than emptied. An empty label still occupies a row, and a band
/// that reserves a line for a sentence it usually has none of moves the ranked
/// list down for nothing.
fn say(note: &gtk::Label, text: &str) {
    note.set_text(text);
    note.set_visible(!text.is_empty());
}

/// How much room the band wants, in rem, at its tallest.
///
/// Chips wrap, so the real height depends on the width they are given. The
/// upper bound is one chip per row, which is what a window narrow enough to
/// need the fit rule actually produces, and a bound is what the fit test
/// wants: a surface that fits its worst case fits every case above it.
#[cfg(test)]
pub(crate) fn content(presets: &[SavedPreset]) -> (f64, f64) {
    if presets.is_empty() {
        return (0.0, 0.0);
    }
    (0.0, HEAD_REM + presets.len() as f64 * CHIP_REM)
}

/// The heading above the chips, in rem.
#[cfg(test)]
const HEAD_REM: f64 = 1.5;

/// One row of chips, in rem.
#[cfg(test)]
const CHIP_REM: f64 = 2.0;

#[cfg(test)]
mod tests;
