//! `ED`, `EL`, `ECH`, and back-colour erase.
//!
//! Erasing is where a terminal decides what "blank" means. It is not "black": a cell
//! erased while a background colour is set keeps that colour, which is how a program
//! paints a coloured pane. Getting this wrong shows up as black holes in a full-screen
//! program the moment it clears anything.

use crate::tests::support::{GHOSTTY_ANSI, cell_at, linear, rows_of};

/// `CSI 0 K` erases from the cursor to the end of the row and leaves the head alone.
#[test]
fn el_zero_erases_to_the_end_of_the_row() {
    let screen = linear(8, 2, b"abcdefgh\x1b[1;4H\x1b[0K");

    assert_eq!(rows_of(&screen)[0], "abc");
}

/// `CSI 1 K` erases from the start of the row through the cursor cell inclusive.
///
/// The bug: erasing up to but not including the cursor. `CSI 1 K` is defined to include
/// it, and a prompt redraw that leaves one stale character behind is the visible result.
#[test]
fn el_one_erases_the_head_of_the_row_including_the_cursor_cell() {
    let screen = linear(8, 2, b"abcdefgh\x1b[1;4H\x1b[1K");

    assert_eq!(rows_of(&screen)[0], "    efgh");
}

/// `CSI 2 K` erases the whole row and does not move the cursor.
#[test]
fn el_two_erases_the_whole_row_and_leaves_the_cursor_where_it_was() {
    let screen = linear(8, 2, b"abcdefgh\x1b[1;4H\x1b[2K");

    assert_eq!(rows_of(&screen)[0], "");
    assert_eq!(screen.cursor().col, 3);
}

/// A bare `CSI K` is `CSI 0 K`.
///
/// An omitted parameter defaults to zero, and every shell prompt in existence writes the
/// bare form. Defaulting it to 2 instead would erase the prompt the user is typing at.
#[test]
fn a_bare_el_is_the_same_as_el_zero() {
    let bare = linear(8, 2, b"abcdefgh\x1b[1;4H\x1b[K");
    let explicit = linear(8, 2, b"abcdefgh\x1b[1;4H\x1b[0K");

    assert_eq!(bare, explicit);
}

/// `CSI 0 J` erases from the cursor to the end of the screen.
#[test]
fn ed_zero_erases_to_the_end_of_the_screen() {
    let screen = linear(4, 3, b"aaaa\r\nbbbb\r\ncccc\x1b[2;3H\x1b[0J");

    assert_eq!(rows_of(&screen), vec!["aaaa", "bb", ""]);
}

/// `CSI 1 J` erases from the start of the screen through the cursor cell.
#[test]
fn ed_one_erases_the_head_of_the_screen_including_the_cursor_cell() {
    let screen = linear(4, 3, b"aaaa\r\nbbbb\r\ncccc\x1b[2;3H\x1b[1J");

    assert_eq!(rows_of(&screen), vec!["", "   b", "cccc"]);
}

/// `CSI 2 J` erases the whole screen without moving the cursor.
///
/// The bug: homing the cursor as part of the erase. `clear` sends `CSI 2 J` followed by
/// `CSI H` precisely because the erase does not home; a parser that homes anyway
/// disagrees with every program that erases and then writes where it already was.
#[test]
fn ed_two_erases_everything_and_does_not_home_the_cursor() {
    let screen = linear(4, 3, b"aaaa\r\nbbbb\r\ncccc\x1b[2;3H\x1b[2J");

    assert_eq!(rows_of(&screen), vec!["", "", ""]);
    assert_eq!(screen.cursor().row, 1);
    assert_eq!(screen.cursor().col, 2);
}

/// `CSI X` erases `n` cells from the cursor without moving anything.
///
/// The bug: implementing `ECH` as `DCH`. Deleting would pull the tail of the row left,
/// so a program overwriting a field in place would have the rest of its line shift.
#[test]
fn ech_blanks_cells_in_place_without_pulling_the_tail_left() {
    let screen = linear(8, 2, b"abcdefgh\x1b[1;3H\x1b[3X");

    assert_eq!(rows_of(&screen)[0], "ab   fgh");
    assert_eq!(screen.cursor().col, 2, "ECH does not move the cursor");
}

/// `ECH` past the end of the row stops at the margin.
#[test]
fn ech_past_the_margin_stops_at_the_margin() {
    let screen = linear(8, 2, b"abcdefgh\x1b[1;7H\x1b[99X");

    assert_eq!(rows_of(&screen)[0], "abcdef");
}

/// An erase paints the current background, not the default one.
///
/// This is back-colour erase, and it is what makes a full-screen program's pane one
/// colour. Without it every clear punches the default background through the pane.
#[test]
fn an_erase_paints_the_current_background() {
    let screen = linear(4, 2, b"\x1b[44m\x1b[2J");

    assert_eq!(
        cell_at(&screen, 2, 1).bg,
        GHOSTTY_ANSI[4],
        "the erased cell kept the blue that was set when it was erased"
    );
}

/// An erase after the background is reset paints the default again.
///
/// The pair matters: a parser that latched the pane colour would keep painting blue
/// after the program went back to the default.
#[test]
fn an_erase_after_a_reset_paints_the_default_background() {
    let screen = linear(4, 2, b"\x1b[44m\x1b[2J\x1b[49m\x1b[2J");

    assert_eq!(
        cell_at(&screen, 2, 1).bg,
        crate::palette::Palette::DEFAULT.bg,
    );
}

/// The foreground of an erased cell is the default, not the colour that was set.
///
/// An erased cell has no glyph, so carrying a foreground into it would only show up
/// later, when something wrote a character there and inherited a colour nobody set.
#[test]
fn an_erased_cell_carries_no_foreground_of_its_own() {
    let screen = linear(4, 2, b"\x1b[31;44m\x1b[2J");

    assert_eq!(
        cell_at(&screen, 0, 0).fg,
        crate::palette::Palette::DEFAULT.fg
    );
}
