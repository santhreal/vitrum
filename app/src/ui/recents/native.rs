//! The recents band, as GTK rows.
//!
//! Same content and the same order as the module above: newest first, keyed on
//! the command AND the directory, no ranking and no clock reading. The row is
//! a button rather than a list item because taking one is the only thing that
//! can be done to it, and a selectable list would add a selection nothing
//! reads.
//!
//! Validation stays on the click, for the reason [`super::native`]'s sibling
//! band gives: [`launch::recent_launch`] is a `stat` and a `PATH` walk, and
//! this band is rebuilt whenever the launcher's query empties.

use gtk::prelude::*;
use vitrum_proto::ProjectInfo;

use crate::launch::{self, RecentEntry};
use crate::shell::Shell;
use crate::ui::dialog::place_of;
use crate::ui::sheet;
use crate::ui::{glyph, icons};

/// Build the band. Always a widget, because the empty case has something to
/// say: nothing has been started yet.
pub(crate) fn band(
    shell: &Shell,
    entries: &[RecentEntry],
    projects: &[ProjectInfo],
    home: &str,
) -> gtk::Box {
    let band = sheet::column("rg-recents");
    let note = sheet::label("rg-recents__note", "");
    note.set_no_show_all(true);

    if entries.is_empty() {
        note.set_text("Nothing started yet.");
        note.set_visible(true);
        band.pack_start(&note, false, false, 0);
        return band;
    }

    let list = sheet::column("rg-recents__list");
    for (i, entry) in entries.iter().enumerate() {
        list.pack_start(&row(shell, entry, i, projects, home, &note), false, false, 0);
    }
    band.pack_start(&list, false, false, 0);
    band.pack_start(&note, false, false, 0);
    band
}

/// One recent command, as the control that starts it again.
fn row(
    shell: &Shell,
    entry: &RecentEntry,
    index: usize,
    projects: &[ProjectInfo],
    home: &str,
    note: &gtk::Label,
) -> gtk::Button {
    let line = launch::recent_line(entry);
    let icon = *icons::resolve(entry.icon.as_deref(), &line);
    let place = place_of(projects, &entry.cwd, home);

    let body = sheet::row("rg-recents__row");
    // The ordinal is drawn, not bound. It is the row's position in a list the
    // operator is reading, and binding it would collide with the launcher's
    // own Ctrl+digit, which numbers the ranked rows below this band.
    body.pack_start(
        &sheet::label("rg-recents__key", &(index + 1).to_string()),
        false,
        false,
        0,
    );
    body.pack_start(&glyph::mark(icon.stroke, icon.fill, "rg-recents__icon"), false, false, 0);
    let text = sheet::label("rg-recents__text", &line);
    text.set_hexpand(true);
    body.pack_start(&text, true, true, 0);
    body.pack_end(&sheet::label("rg-recents__place", &place), false, false, 0);

    let button = gtk::Button::new();
    button.add(&body);
    // The place chip is project-relative, so the absolute directory lives
    // here, where it can be read without spending a line of the row on it.
    button.set_tooltip_text(Some(&format!("{line} in {}", entry.cwd)));

    let shell = shell.clone();
    let taken = entry.clone();
    let note = note.clone();
    button.connect_clicked(move |_| match launch::recent_launch(&taken) {
        Ok(l) => {
            note.set_visible(false);
            crate::ui::dialog::native::go(&shell, l);
        }
        // A row whose directory has gone since it ran does not vanish and does
        // not launch. Hiding it would leave the operator wondering where it
        // went.
        Err(why) => {
            note.set_text(&why);
            note.set_visible(true);
        }
    });
    button
}

/// How much room the band wants, in rem.
///
/// The empty case is one line, not nothing: it says so out loud.
#[cfg(test)]
pub(crate) fn content(entries: &[RecentEntry]) -> (f64, f64) {
    if entries.is_empty() {
        return (0.0, NOTE_REM);
    }
    (0.0, entries.len() as f64 * ROW_REM)
}

/// One row, in rem.
#[cfg(test)]
const ROW_REM: f64 = 2.0;

/// The line the empty band says instead of a list, in rem.
#[cfg(test)]
const NOTE_REM: f64 = 1.5;

#[cfg(test)]
mod tests;
