//! The release-notes sheet's size, and what happens when it is bigger than
//! the window.

use super::*;
use crate::ui::whatsnew::{Group, parse_changelog};

/// The real changelog. A synthetic release would let this test pass on a file
/// nobody ships; the notes that reach an operator are the ones compiled in.
fn shipped() -> Vec<Release> {
    parse_changelog(crate::ui::whatsnew::CHANGELOG)
}

/// **The fit rule for this surface.** The whole shipped changelog is taller
/// than any window this product opens, and it is still reachable.
#[test]
fn the_whole_changelog_fits_a_window_smaller_than_it_is() {
    let releases = shipped();
    assert!(
        !releases.is_empty(),
        "the compiled-in changelog parses to nothing, so this test proves nothing"
    );
    sheet::assert_fits(sheet::WHATSNEW, sheet::DOCUMENT, content(&releases));
}

/// The one-pixel frame a workspace switch hands a client, on the largest
/// content this surface can hold. Called out separately from the general case
/// because it is the allocation that used to leave a sheet permanently
/// truncated for the rest of its life.
#[test]
fn a_one_pixel_allocation_scrolls_rather_than_truncating() {
    let content = content(&shipped());
    let natural = sheet::natural(sheet::DOCUMENT, content);
    assert!(sheet::scrolls(sheet::SMALLEST.1, natural.1));
    assert_eq!(sheet::allocated(sheet::SMALLEST.1, natural.1), 1);
}

/// A single short release is a short sheet. A surface whose height did not
/// depend on its content would be a half-screen box for one bullet, which is
/// the "centred on its own content box" complaint in reverse.
#[test]
fn one_short_release_asks_for_less_room_than_the_whole_file() {
    let one = vec![Release {
        version: "0.1.0".parse().expect("a semver literal"),
        date: "2026-01-01".to_string(),
        groups: vec![Group {
            heading: "Fixed".to_string(),
            entries: vec!["One thing.".to_string()],
        }],
    }];
    assert!(content(&one).1 < content(&shipped()).1);
}
