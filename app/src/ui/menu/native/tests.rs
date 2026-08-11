//! The menu's size, and the sentences it reports a move with.

use super::*;

/// **The fit rule for this surface, and the approval case.** A menu is an
/// option list, the destructive entries are at the bottom, and the operator
/// right-clicked a row near the bottom edge. It scrolls; it is never sliced.
///
/// The longest menu this product builds is the bulk one: every disposition
/// entry, every snooze preset, every workspace and every folder. Twelve
/// workspaces and twelve folders is a menu no window can show at once.
#[test]
fn the_longest_menu_fits_a_window_smaller_than_it_is() {
    let entries = 8 + 5 + 12 + 12;
    sheet::assert_fits(sheet::MENU, sheet::LIST, content(entries, 4));
}

/// A short menu is a short box, and it is still not allowed to leave the
/// smallest frame a window manager can hand a client.
#[test]
fn a_short_menu_still_survives_the_smallest_frame() {
    let short = content(3, 1);
    assert!(short.1 < content(30, 4).1);
    let natural = sheet::natural(sheet::LIST, short);
    assert!(sheet::allocated(sheet::SMALLEST.1, natural.1) <= sheet::SMALLEST.1);
    assert!(sheet::allocated(sheet::SMALLEST.0, natural.0) <= sheet::SMALLEST.0);
}

/// **A partial move is not reported as a success.** Five rows asked for and
/// three placed says three, and says the rest were already there.
#[test]
fn a_move_that_placed_fewer_than_it_was_asked_says_so() {
    let all = moved(Ok(5), 5, "workspace");
    assert!(all.text.contains("Moved 5"), "{}", all.text);
    assert!(!all.text.contains("of 5"), "{}", all.text);

    let some = moved(Ok(3), 5, "workspace");
    assert!(some.text.contains("3 of 5"), "{}", some.text);
    assert!(some.text.contains("already there"), "{}", some.text);
}

/// A refused move is an error, not a notice. A notice retires itself, and a
/// failure that erases itself before it is read is worse than one that stays.
#[test]
fn a_refused_move_is_an_error_that_does_not_retire() {
    let failed = moved(Err(crate::state::WorkspaceError::Unknown), 2, "workspace");
    assert_eq!(failed.kind, crate::state::FlashKind::Error);
    assert_eq!(crate::ui::toast::life(&failed, Some(4_000)), None);
}

/// Separators add height. A measurement that ignored them would understate a
/// menu with four groups in it by four rules' worth, which is the difference
/// between the last entry being on screen and being under the edge.
#[test]
fn separators_count_toward_the_height() {
    assert!(content(10, 4).1 > content(10, 0).1);
}
