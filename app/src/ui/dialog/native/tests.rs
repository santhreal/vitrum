//! What the presented launcher and rename surfaces are sized for.
//!
//! No widget is built: `gtk_init` needs a display. Everything the launcher
//! DECIDES is decided by the pure functions in the module above and asserted
//! there. What is left for this file is the property only the presented
//! surface has, which is that neither sheet can be sliced by the window it is
//! in.

use super::*;
use crate::launch::{RecentEntry, SavedPreset};
use crate::ui::sheet;

fn preset(id: u64) -> SavedPreset {
    SavedPreset {
        id,
        label: format!("preset {id}"),
        command: "claude".to_string(),
        ..Default::default()
    }
}

fn recent(cwd: &str) -> RecentEntry {
    RecentEntry {
        command: "claude".to_string(),
        cwd: cwd.to_string(),
        ..Default::default()
    }
}

/// Every state the launcher can open in, from the emptiest to the fullest a
/// profile can produce.
///
/// The full case is the one that matters: nine presets, the longest recents
/// band `launcher.recentRows` can be set to, the maximum directory completions
/// and a full row list all on screen at once is far taller than any window,
/// and it is the state a configured profile opens into. The band length is the
/// setting's ceiling rather than its default, because the default proves
/// nothing about the list an operator can ask for.
fn states() -> Vec<(f64, f64)> {
    let presets: Vec<SavedPreset> = (1..=9).map(preset).collect();
    let recents: Vec<RecentEntry> = (0..usize::from(crate::state::RECENT_ROWS_MAX))
        .map(|i| recent(&format!("/src/project{i}")))
        .collect();
    vec![
        // A fresh profile: nothing saved, nothing run, nothing typed.
        content(&[], &[], 0, 0, true),
        // Typing, so the bands are gone and the ranked list is the answer.
        content(&presets, &recents, 0, ROWS_MAX, false),
        // Completing a path in the `in` field while the bands are still up.
        content(&presets, &recents, DIR_MAX, DIR_MAX, true),
        // The fullest surface this profile can produce.
        content(&presets, &recents, DIR_MAX, ROWS_MAX, true),
    ]
}

/// **The fit invariant, for the launcher.** At every window size a window
/// manager can hand a client, including the one-pixel frame a workspace switch
/// produces, the launcher is allocated no more than the window and everything
/// past that is reachable by scrolling.
#[test]
fn the_launcher_fits_every_window() {
    for state in states() {
        sheet::assert_fits(sheet::LAUNCHER, sheet::LIST, state);
    }
}

/// **The fit invariant, for the rename field.** A two-line sheet is the case
/// where a cap that also acted as a floor would be most obviously wrong, so it
/// is asserted on the same terms as the tall one.
#[test]
fn the_rename_field_fits_every_window() {
    sheet::assert_fits(sheet::RENAME, sheet::NARROW, rename_content());
}

/// A fuller profile asks for more room. A launcher whose declared height did
/// not move with its content would report that it fits and then slice the rows
/// the operator opened it to read.
#[test]
fn a_fuller_launcher_declares_more_room() {
    let empty = content(&[], &[], 0, 0, true).1;
    let full = content(
        &(1..=9).map(preset).collect::<Vec<_>>(),
        &(0..usize::from(crate::state::RECENT_ROWS_MAX))
            .map(|i| recent(&format!("/src/project{i}")))
            .collect::<Vec<_>>(),
        DIR_MAX,
        ROWS_MAX,
        true,
    )
    .1;
    assert!(
        full > empty,
        "a full profile declared {full} rem and an empty one {empty}"
    );
}

/// The bands cost nothing once the operator is typing. They are not drawn
/// then, and a height that counted them anyway would open the launcher on a
/// scrollbar it does not need.
#[test]
fn the_bands_cost_nothing_once_they_are_gone() {
    let presets: Vec<SavedPreset> = (1..=9).map(preset).collect();
    let recents: Vec<RecentEntry> = (0..4).map(|i| recent(&format!("/src/p{i}"))).collect();
    let with = content(&presets, &recents, 0, ROWS_MAX, true).1;
    let without = content(&presets, &recents, 0, ROWS_MAX, false).1;
    assert!(
        without < with,
        "hiding the bands declared {without} rem against {with} with them"
    );
}

/// The rename sheet counts its refusal line whether or not it is showing. A
/// sheet that grew when it refused would move the control the operator is
/// about to press at the moment they are reading why it did not work.
#[test]
fn the_rename_sheet_reserves_the_line_it_refuses_on() {
    assert!(rename_content().1 > HEAD_REM + FIELD_REM + FOOT_REM);
}
