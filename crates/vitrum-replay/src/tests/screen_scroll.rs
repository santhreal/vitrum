//! Scroll regions, IL, DL, SU, SD, IND and RI.

use crate::tests::support::{linear, rows_of};

/// A line feed on the bottom row scrolls the screen up by one.
#[test]
fn a_line_feed_on_the_bottom_row_scrolls_the_screen() {
    let screen = linear(4, 3, b"a\r\nb\r\nc\r\nd");
    assert_eq!(rows_of(&screen), vec!["b", "c", "d"]);
}

/// `DECSTBM` confines scrolling to its rows and leaves everything outside fixed.
///
/// This is what makes a status bar a status bar. A program that pins a header at row 0
/// and a footer at the bottom sets the region between them; scrolling the whole screen
/// instead drags the header off the top on the first line of output.
#[test]
fn a_scroll_region_leaves_the_rows_outside_it_fixed() {
    // Rows 2..4 scroll (1-based 2;4). Row 0 is a header, row 4 a footer.
    let screen = linear(
        6,
        5,
        b"header\r\n\x1b[2;4r\x1b[5;1Hfooter\x1b[2;1Hone\r\ntwo\r\nthree\r\nfour",
    );
    assert_eq!(
        rows_of(&screen),
        vec!["header", "two", "three", "four", "footer"]
    );
}

/// `DECSTBM` homes the cursor.
///
/// Programs rely on this: `smcup` sequences set the region and then draw from the
/// top without addressing first.
#[test]
fn setting_a_scroll_region_homes_the_cursor() {
    let screen = linear(6, 4, b"\x1b[3;4Hxx\x1b[2;3rA");
    assert_eq!(rows_of(&screen)[0], "A");
    assert_eq!(screen.cursor().row, 0);
}

/// `CSI r` with no parameters restores the full-screen region.
///
/// `rmcup` sends it on the way out. A program that left the region set would leave
/// the shell scrolling inside a window in the middle of the screen.
#[test]
fn csi_r_with_no_parameters_restores_the_full_screen_region() {
    let screen = linear(4, 3, b"\x1b[2;2r\x1b[ra\r\nb\r\nc\r\nd");
    assert_eq!(
        rows_of(&screen),
        vec!["b", "c", "d"],
        "the whole screen scrolled, so the region really went back to 0..2"
    );
}

/// An inverted or degenerate region is rejected and the full screen is used.
///
/// `CSI 5 ; 2 r` is malformed. Accepting it would leave `top > bottom`, and every
/// later scroll would compute a negative height.
///
/// Ghostty owns the region and does not report it, so the assertion is what the
/// region does: four rows of output scroll the whole screen by one, which only
/// happens when the region is the full screen.
#[test]
fn an_inverted_region_falls_back_to_the_full_screen() {
    let inverted = linear(4, 4, b"\x1b[4;2ra\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(rows_of(&inverted), vec!["b", "c", "d", "e"]);

    let single = linear(4, 4, b"\x1b[2;2ra\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(
        rows_of(&single),
        vec!["b", "c", "d", "e"],
        "a one-row region cannot scroll, so it is refused too"
    );
}

/// `RI` on the top row of the region scrolls the region down.
///
/// This is how a pager scrolls backwards. Moving the cursor up out of the region
/// instead would draw the previous page over the header.
#[test]
fn reverse_index_on_the_top_row_scrolls_the_region_down() {
    let screen = linear(4, 3, b"a\r\nb\r\nc\x1b[H\x1bMX");
    assert_eq!(rows_of(&screen), vec!["X", "a", "b"]);
}

/// `IND` moves down without returning to column zero, and `NEL` does both.
///
/// The bug: implementing `IND` as `NEL`. `ESC D` is a bare line feed; a program using
/// it to step down a column of a table would have every cell pushed to column zero.
#[test]
fn index_moves_down_only_and_nel_also_returns_to_column_zero() {
    let index = linear(6, 3, b"abc\x1bDX");
    assert_eq!(rows_of(&index), vec!["abc", "   X", ""]);

    let nel = linear(6, 3, b"abc\x1bEX");
    assert_eq!(rows_of(&nel), vec!["abc", "X", ""]);
}

/// `CSI L` inserts blank rows at the cursor and pushes the rest of the region down,
/// dropping what falls off the bottom of the region.
#[test]
fn insert_lines_pushes_the_region_down_and_drops_the_overflow() {
    let screen = linear(4, 4, b"a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[2L");
    assert_eq!(rows_of(&screen), vec!["a", "", "", "b"]);
}

/// `CSI M` deletes rows at the cursor and pulls the rest of the region up.
#[test]
fn delete_lines_pulls_the_region_up() {
    let screen = linear(4, 4, b"a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[2M");
    assert_eq!(rows_of(&screen), vec!["a", "d", "", ""]);
}

/// `IL` and `DL` outside the scroll region do nothing at all.
///
/// The bug: editing rows the region excludes, which lets a program with a pinned
/// header corrupt the header by inserting a line while the cursor happens to be on it.
#[test]
fn insert_and_delete_lines_outside_the_region_do_nothing() {
    let screen = linear(4, 4, b"a\r\nb\r\nc\r\nd\x1b[2;3r\x1b[1;1H\x1b[2L\x1b[4;1H\x1b[2M");
    assert_eq!(rows_of(&screen), vec!["a", "b", "c", "d"]);
}

/// `CSI S` and `CSI T` scroll the region without moving the cursor.
#[test]
fn su_and_sd_scroll_the_region_and_leave_the_cursor_alone() {
    let up = linear(4, 3, b"a\r\nb\r\nc\x1b[2;2H\x1b[SX");
    assert_eq!(rows_of(&up), vec!["b", "cX", ""]);

    let down = linear(4, 3, b"a\r\nb\r\nc\x1b[2;2H\x1b[TX");
    assert_eq!(rows_of(&down), vec!["", "aX", "b"]);
}

/// A scroll past the height of the region clears it rather than reading out of bounds.
#[test]
fn a_scroll_larger_than_the_region_clears_it() {
    let screen = linear(4, 3, b"a\r\nb\r\nc\x1b[99S");
    assert_eq!(rows_of(&screen), vec!["", "", ""]);
}

/// Rows scrolled into view use the current background, not black.
///
/// A program painting a coloured pane and then scrolling it would otherwise get a
/// black stripe at the edge on every scroll.
#[test]
fn scrolled_in_rows_use_the_current_background() {
    let screen = linear(4, 3, b"\x1b[41ma\r\nb\r\nc\r\nd");
    let cell = screen.grid().cell(0, 2).expect("cell");
    assert_eq!(cell.ch, 'd');
    let vacated = screen.grid().cell(3, 2).expect("cell");
    assert_eq!(
        vacated.bg,
        crate::tests::support::GHOSTTY_ANSI[1],
        "the vacated row was filled in the pane colour"
    );
}

/// Two screens showing the same thing are the same screen, however they got there.
///
/// The grid scrolls by rotating an index rather than moving cells, so a screen
/// that has scrolled holds its rows in a different physical order from one that
/// has not. Comparing the raw cell buffer would call these two different, which
/// would make every seek that lands after a scroll disagree with the linear
/// replay it is supposed to match.
#[test]
fn how_a_screen_scrolled_into_place_does_not_change_what_it_equals() {
    // Three rows of content reached by scrolling two lines off the top.
    let scrolled = linear(4, 3, b"x\r\ny\r\na\r\nb\r\nc");
    // The same three rows, written where they sit, with the cursor left in the
    // same place by the same final keystrokes.
    let direct = linear(4, 3, b"a\r\nb\r\nc");

    assert_eq!(rows_of(&scrolled), rows_of(&direct), "the fixture must agree");
    assert_eq!(
        scrolled, direct,
        "a screen that scrolled must equal one that did not"
    );
}
