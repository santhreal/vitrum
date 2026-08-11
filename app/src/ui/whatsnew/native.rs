//! The post-update release notes, as a presented surface.
//!
//! Shown once. The recording of "seen" is hung off dismissal rather than off
//! the footer control, because a sheet that only records itself when the
//! operator presses the button comes back on the next launch for everyone who
//! pressed Escape or clicked the scrim, and a release note that will not stay
//! dismissed is worse than none.

use std::rc::Rc;

use gtk::prelude::*;

use super::{Release, intro, title};
use crate::shell::Shell;
use crate::ui::sheet::{self, Sheet};

/// The sheet for the releases this profile has not seen.
///
/// `on_seen` runs when the sheet goes away, however it went away.
pub(crate) fn build(
    shell: &Shell,
    releases: &[Release],
    on_seen: impl Fn() + 'static,
) -> Rc<Sheet> {
    let panel = sheet::column("rg-sheet__panel");
    panel.pack_start(&sheet::head(shell, &title(releases)), false, false, 0);

    let body = sheet::column("rg-sheet__body");
    body.pack_start(
        &sheet::label("rg-whatsnew__intro", &intro(releases)),
        false,
        false,
        0,
    );

    for release in releases {
        let block = sheet::column("rg-whatsnew__release");
        let version = sheet::row("rg-whatsnew__version");
        version.pack_start(
            &sheet::label("rg-whatsnew__number", &release.version.to_string()),
            false,
            false,
            0,
        );
        if !release.date.is_empty() {
            version.pack_end(
                &sheet::label("rg-whatsnew__date", &release.date),
                false,
                false,
                0,
            );
        }
        block.pack_start(&version, false, false, 0);

        for group in &release.groups {
            let section = sheet::column("rg-whatsnew__group");
            if !group.heading.is_empty() {
                section.pack_start(
                    &sheet::label("rg-whatsnew__heading", &group.heading),
                    false,
                    false,
                    0,
                );
            }
            let entries = sheet::column("rg-whatsnew__entries");
            for entry in &group.entries {
                entries.pack_start(
                    &sheet::label("rg-whatsnew__entry", entry),
                    false,
                    false,
                    0,
                );
            }
            section.pack_start(&entries, false, false, 0);
            block.pack_start(&section, false, false, 0);
        }
        body.pack_start(&block, false, false, 0);
    }
    panel.pack_start(&body, true, true, 0);

    let foot = sheet::row("rg-sheet__foot");
    let done = gtk::Button::with_label("Got it");
    done.style_context().add_class("rg-btn");
    done.style_context().add_class("rg-btn--primary");
    let shell = shell.clone();
    done.connect_clicked(move |_| shell.dismiss());
    foot.pack_end(&done, false, false, 0);
    panel.pack_start(&foot, false, false, 0);

    let sheet = Sheet::new(sheet::WHATSNEW, sheet::DOCUMENT, &panel);
    sheet.on_dismiss(on_seen);
    sheet
}

/// How much room the notes want, in rem, for the releases being shown.
///
/// Reported rather than measured, so the fit rule can be checked without a
/// display. The width is the cap: release notes are prose and prose gets the
/// column it was written for. The height is what the content adds up to, which
/// is what decides whether the sheet scrolls.
#[cfg(test)]
pub(crate) fn content(releases: &[Release]) -> (f64, f64) {
    // Head, intro and footer. Everything below is per release.
    let mut lines = 6.0;
    for release in releases {
        lines += 2.0;
        for group in &release.groups {
            lines += if group.heading.is_empty() { 0.0 } else { 2.0 };
            lines += group.entries.len() as f64 * 2.0;
        }
    }
    (sheet::DOCUMENT.width, lines)
}

#[cfg(test)]
mod tests;
