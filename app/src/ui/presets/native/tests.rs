//! What the band contributes to the surface holding it.
//!
//! No widget is built: `gtk_init` needs a display. What is asserted is the
//! extent the launcher adds up when it declares its own natural size, because
//! an under-declared band is a launcher that reports it fits and then slices
//! its own list.

use super::*;

fn preset(id: u64) -> SavedPreset {
    SavedPreset {
        id,
        label: format!("preset {id}"),
        command: "claude".to_string(),
        ..Default::default()
    }
}

/// No presets is no band, so it costs the surface nothing. A heading with
/// nothing under it would take a row from the ranked list to say that the
/// operator has saved nothing, which they can already see.
#[test]
fn an_empty_band_asks_for_no_room() {
    assert_eq!(content(&[]), (0.0, 0.0));
}

/// The band's declared height rises with the number of chips. A fixed
/// estimate would under-declare on a full profile, which is the case where the
/// launcher has least room to spare.
#[test]
fn the_band_declares_more_room_for_more_presets() {
    let few = content(&[preset(1)]).1;
    let many = content(&(1..=9).map(preset).collect::<Vec<_>>()).1;
    assert!(
        many > few,
        "nine presets declared {many} rem and one declared {few}"
    );
}

/// The band never asks for width. It is content inside a sheet that already
/// has a cap, and a band that stated its own width would be the one element
/// able to push the launcher past that cap.
#[test]
fn the_band_leaves_the_width_to_the_sheet() {
    assert_eq!(content(&(1..=4).map(preset).collect::<Vec<_>>()).0, 0.0);
}
