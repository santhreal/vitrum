//! What the band contributes to the surface holding it.
//!
//! No widget is built: `gtk_init` needs a display. What is asserted is the
//! extent the launcher adds up when it declares its natural size.

use super::*;

fn entry(cwd: &str) -> RecentEntry {
    RecentEntry {
        command: "claude".to_string(),
        cwd: cwd.to_string(),
        ..Default::default()
    }
}

/// The empty band still costs a line, because it still says something. An
/// extent of zero here would under-declare the one case where the launcher has
/// nothing else on screen either.
#[test]
fn the_empty_band_still_declares_the_line_it_says() {
    assert!(content(&[]).1 > 0.0);
}

/// The declared height rises with the number of rows, so a full recents list
/// is not silently taller than the launcher claimed.
#[test]
fn the_band_declares_more_room_for_more_rows() {
    let one = content(&[entry("/src/vitrum")]).1;
    let many = content(&[
        entry("/src/vitrum"),
        entry("/src/vitrum/app"),
        entry("/src/other"),
    ])
    .1;
    assert!(many > one, "three rows declared {many} rem and one {one}");
}

/// The band never states its own width. The sheet's cap is what bounds the
/// launcher, and a band with an opinion about width could push past it.
#[test]
fn the_band_leaves_the_width_to_the_sheet() {
    assert_eq!(content(&[entry("/src/vitrum")]).0, 0.0);
    assert_eq!(content(&[]).0, 0.0);
}
