//! ED, EL, ECH, and back-colour erase.

use vitrum_grid::{Attrs, Rgba};

use crate::palette::Palette;
use crate::tests::support::{linear, rows_of};

/// `CSI K` with no parameter erases from the cursor to the end of the line and
/// leaves everything before it.
///
/// The bug: reading the absent parameter as one, which erases the *start* of the
/// line instead. Every progress bar in the fixture is `\r` followed by `CSI K`, so
/// getting this backwards erases the text the program is about to overwrite anyway
/// and keeps the stale tail it meant to remove.
#[test]
fn el_with_no_parameter_erases_from_the_cursor_forwards() {
    let screen = linear(10, 2, b"abcdefghij\r\x1b[3C\x1b[K");
    assert_eq!(rows_of(&screen)[0], "abc");
}

/// `CSI 1 K` erases from the start of the line through the cursor, inclusive.
#[test]
fn el_1_erases_through_the_cursor_inclusive() {
    let screen = linear(10, 2, b"abcdefghij\r\x1b[3C\x1b[1K");
    assert_eq!(rows_of(&screen)[0], "    efghij");
}

/// `CSI 2 K` erases the whole line and leaves the cursor where it was.
#[test]
fn el_2_erases_the_whole_line_without_moving_the_cursor() {
    let screen = linear(10, 2, b"abcdefghij\r\x1b[3C\x1b[2KX");
    assert_eq!(rows_of(&screen)[0], "   X");
    assert_eq!(screen.cursor().col, 4);
}

/// `CSI J` erases from the cursor to the bottom right, including the rest of the
/// cursor's own row.
///
/// The bug: erasing whole rows from the cursor's row down, which wipes the text to
/// the left of the cursor on that row. A shell that clears below its prompt would
/// erase the prompt.
#[test]
fn ed_0_erases_the_rest_of_the_row_and_every_row_below() {
    let screen = linear(6, 3, b"aaaaaa\r\nbbbbbb\r\ncccccc\x1b[2;4H\x1b[J");
    assert_eq!(rows_of(&screen), vec!["aaaaaa", "bbb", ""]);
}

/// `CSI 1 J` erases from the top left through the cursor, inclusive.
#[test]
fn ed_1_erases_from_the_top_through_the_cursor_inclusive() {
    let screen = linear(6, 3, b"aaaaaa\r\nbbbbbb\r\ncccccc\x1b[2;4H\x1b[1J");
    assert_eq!(rows_of(&screen), vec!["", "    bb", "cccccc"]);
}

/// `CSI 2 J` erases the screen and does not move the cursor.
///
/// The bug: homing the cursor as part of `ED 2`. Programs emit `CSI 2 J` followed by
/// `CSI H` precisely because `ED` does not move the cursor; one that homed would make
/// the following `CSI 5 ; 1 H` land in the wrong place for every program that clears
/// then addresses.
#[test]
fn ed_2_erases_everything_and_leaves_the_cursor_alone() {
    let screen = linear(6, 3, b"aaaaaa\r\nbbbbbb\x1b[2;4H\x1b[2JX");
    assert_eq!(rows_of(&screen), vec!["", "   X", ""]);
}

/// `CSI 3 J` erases the scrollback and leaves the visible screen untouched.
///
/// The bug that made this a test: treating any unrecognised `ED` parameter as "erase
/// everything". `clear` on many systems sends `CSI H CSI 2 J CSI 3 J`, and a plain
/// `CSI 3 J` also arrives on its own from tools that only want the scrollback gone.
/// Wiping the screen for it makes output vanish that the user was still reading.
#[test]
fn ed_3_leaves_the_visible_screen_alone() {
    let screen = linear(6, 2, b"aaaaaa\r\nbbbbbb\x1b[3J");
    assert_eq!(rows_of(&screen), vec!["aaaaaa", "bbbbbb"]);
}

/// `CSI X` blanks a run of cells from the cursor and does not move it or shift the
/// tail.
///
/// The bug: implementing ECH as DCH. `ECH` overwrites in place; `DCH` pulls the rest
/// of the line left. Confusing them corrupts every table a TUI redraws.
#[test]
fn ech_blanks_in_place_without_shifting_or_moving() {
    let screen = linear(10, 2, b"abcdefghij\r\x1b[2C\x1b[3X");
    assert_eq!(rows_of(&screen)[0], "ab   fghij");
    assert_eq!(screen.cursor().col, 2);
}

/// `CSI X` past the end of the row stops at the row and does not touch the next one.
#[test]
fn ech_clamps_at_the_end_of_the_row() {
    let screen = linear(6, 2, b"aaaaaa\r\nbbbbbb\x1b[2;3H\x1b[99X");
    assert_eq!(rows_of(&screen), vec!["aaaaaa", "bb"]);
}

/// An erase keeps the current background colour and drops the other rendition bits.
///
/// Back-colour erase is what lets a program paint a coloured panel and then clear part
/// of it without the cleared part turning black. Keeping the *other* bits would be
/// the opposite bug: an erase under an active underline would draw a rule across
/// empty space.
#[test]
fn an_erase_keeps_the_background_and_drops_the_other_attributes() {
    // Blue background, underline on, then erase the line.
    let screen = linear(6, 2, b"\x1b[44;4mxxxxxx\r\x1b[2K");
    let cell = screen.grid().cell(0, 0).expect("cell");
    assert_eq!(cell.ch, ' ');
    assert_eq!(cell.bg, Palette::XTERM.indexed(4), "the panel colour survived");
    assert_eq!(cell.fg, Palette::XTERM.fg, "the foreground went back to default");
    assert_eq!(cell.attrs, Attrs::NONE, "no underline across the blank");
}

/// An untouched screen and an erased screen are painted identically.
///
/// If they differed, a `clear` would leave a visible seam between the part of the
/// screen the program wrote and the part it never reached.
#[test]
fn an_erased_screen_matches_an_untouched_one() {
    let untouched = linear(6, 2, b"");
    let erased = linear(6, 2, b"abc\r\ndef\x1b[2J");
    let blank = untouched.grid().cell(0, 0).expect("cell");
    let cleared = erased.grid().cell(0, 0).expect("cell");
    assert_eq!(blank, cleared);
    assert_eq!(blank.bg, Rgba::rgb(0, 0, 0), "xterm's default background");
}
