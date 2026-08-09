//! Cursor addressing, origin mode, tab stops, save and restore.

use vitrum_grid::Attrs;

use crate::palette::Palette;
use crate::tests::support::{GHOSTTY_ANSI, linear, rows_of};

/// `CUP` is one-based, row first, and an absent parameter means one.
///
/// The bug: zero-based addressing. Every `CSI H` in the world would then land one row
/// and one column off, which is exactly the failure that makes a TUI's borders sit
/// half off the screen.
#[test]
fn cup_is_one_based_and_row_first() {
    let screen = linear(8, 4, b"\x1b[3;5HX");
    assert_eq!(rows_of(&screen), vec!["", "", "    X", ""]);

    let home = linear(8, 4, b"\x1b[9;9H\x1b[HX");
    assert_eq!(rows_of(&home)[0], "X", "CSI H with no parameters is home");
}

/// `CUP` past the edges clamps rather than wrapping or panicking.
#[test]
fn cup_past_the_edges_clamps() {
    let screen = linear(4, 3, b"\x1b[99;99HX");
    assert_eq!(rows_of(&screen), vec!["", "", "   X"]);
}

/// `CSI 0 ; 0 H` is treated as `CSI 1 ; 1 H`.
///
/// Zero is not a valid coordinate, and terminals read it as one. Treating it as index
/// zero-minus-one is where the off-by-one underflows.
#[test]
fn cup_with_zero_parameters_means_home() {
    let screen = linear(4, 3, b"\x1b[2;2H\x1b[0;0HX");
    assert_eq!(rows_of(&screen)[0], "X");
}

/// `CHA` and `VPA` set one axis and leave the other alone.
#[test]
fn cha_and_vpa_each_move_one_axis() {
    let screen = linear(8, 4, b"\x1b[3;5H\x1b[2GX");
    assert_eq!(rows_of(&screen)[2], " X", "CHA kept row 3");

    let vertical = linear(8, 4, b"\x1b[3;5H\x1b[2dX");
    assert_eq!(rows_of(&vertical)[1], "    X", "VPA kept column 5");
}

/// Origin mode makes addressing relative to the scroll region, and clamps to it.
///
/// The bug: ignoring DECOM. A program that sets a region and turns origin mode on then
/// addresses row 1 meaning the top of its pane; absolute addressing puts that output in
/// the header instead.
#[test]
fn origin_mode_addresses_relative_to_the_scroll_region() {
    let screen = linear(6, 5, b"\x1b[2;4r\x1b[?6h\x1b[1;1HA\x1b[99;1HB");
    assert_eq!(
        rows_of(&screen),
        vec!["", "A", "", "B", ""],
        "row 1 is the top of the region and row 99 clamps to its bottom"
    );
}

/// `CUU` and `CUD` clamp to the scroll region when the cursor starts inside it, and
/// never scroll.
///
/// The bug: letting `CUU` scroll like `RI` does. A program stepping the cursor up a
/// column would then drag the whole pane down with it.
#[test]
fn cuu_and_cud_clamp_to_the_region_without_scrolling() {
    let screen = linear(6, 5, b"header\r\n\x1b[2;4r\x1b[3;1Hmid\x1b[99AX\x1b[99BY");
    assert_eq!(
        rows_of(&screen),
        vec!["header", "   X", "mid", "    Y", ""],
        "the cursor stopped at rows 2 and 4, kept its column, and the header never moved"
    );
}

/// `HTS` adds a stop, `TBC` removes one, and `CSI 3 g` removes them all.
///
/// The bug: hardcoded eight-column tabs. Anything that lays out a table with custom
/// stops, which is what `expand` and every column-aligned TUI does, lands in the wrong
/// column on every tab.
#[test]
fn tab_stops_can_be_set_cleared_and_wiped() {
    // Clear all stops, set one at column 3, then tab to it.
    let screen = linear(12, 2, b"\x1b[3g\x1b[1;4H\x1bH\x1b[1;1Ha\tb");
    assert_eq!(rows_of(&screen)[0], "a  b");

    // With every stop gone, a tab runs to the last column.
    let none = linear(12, 2, b"\x1b[3ga\tb");
    assert_eq!(none.cursor().col, 11);
    assert_eq!(rows_of(&none)[0], "a          b");

    // Clearing just the stop at column 8 makes the first tab reach column 16.
    let one_gone = linear(24, 2, b"\x1b[1;9H\x1b[0g\x1b[1;1Ha\tb");
    assert_eq!(rows_of(&one_gone)[0], "a               b");

    // Ghostty ignores `TBC` with the parameter omitted, where ECMA-48 says an absent
    // parameter is 0. This records the defect rather than hiding it: the assertion is
    // that the bare form does nothing, so the day Ghostty starts honouring it this
    // test goes red and someone deletes this block instead of discovering the change
    // through a replay that suddenly tabs to a different column.
    let bare = linear(24, 2, b"\x1b[1;9H\x1b[g\x1b[1;1Ha\tb");
    assert_eq!(
        rows_of(&bare)[0],
        "a       b",
        "CSI g with no parameter is a no-op in Ghostty, so the column-8 stop survives"
    );
}

/// `CHT` and `CBT` move several stops at a time.
#[test]
fn cht_and_cbt_move_several_stops() {
    let forward = linear(40, 2, b"\x1b[3IX");
    assert_eq!(forward.cursor().col, 25, "three eight-column stops from zero");

    let back = linear(40, 2, b"\x1b[1;30H\x1b[2ZX");
    assert_eq!(back.cursor().col, 17, "back two stops from column 29");
}

/// `DECSC` and `DECRC` carry the rendition and the charset, not just the position.
///
/// The bug: saving only the coordinates. A program that saves, changes colour, prints,
/// and restores expects its colour back too; without it every subsequent character
/// takes the colour of whatever the interruption used.
#[test]
fn decsc_and_decrc_carry_the_rendition_as_well_as_the_position() {
    let screen = linear(20, 3, b"\x1b[31m\x1b7\x1b[2;5H\x1b[32mgreen\x1b8red");
    assert_eq!(rows_of(&screen)[1], "    green");
    assert_eq!(rows_of(&screen)[0], "red");
    let cell = screen.grid().cell(0, 0).expect("cell");
    assert_eq!(cell.fg, GHOSTTY_ANSI[1], "red came back");
}

/// `CSI s` and `CSI u` save and restore too.
///
/// The fixture contains both, because that spelling is what a great deal of real
/// output uses.
#[test]
fn csi_s_and_csi_u_save_and_restore() {
    let screen = linear(20, 3, b"start\x1b[s\x1b[3;1Hbottom\x1b[u after");
    assert_eq!(rows_of(&screen)[0], "start after");
    assert_eq!(rows_of(&screen)[2], "bottom");
}

/// `DECRC` with nothing saved goes home with default rendition.
///
/// The bug: restoring uninitialised state. A program that restores before saving is
/// malformed, and the answer has to be deterministic rather than whatever was in the
/// field.
#[test]
fn decrc_with_nothing_saved_goes_home_in_default_rendition() {
    let screen = linear(8, 3, b"\x1b[2;3H\x1b[1;31m\x1b8X");
    assert_eq!(rows_of(&screen)[0], "X");
    let cell = screen.grid().cell(0, 0).expect("cell");
    assert_eq!(cell.fg, Palette::XTERM.fg);
    assert_eq!(cell.attrs, Attrs::NONE);
}

/// `DECTCEM` toggles cursor visibility and nothing else.
#[test]
fn dectcem_toggles_cursor_visibility() {
    assert!(linear(4, 2, b"").cursor().visible, "visible at power on");
    assert!(!linear(4, 2, b"\x1b[?25l").cursor().visible);
    assert!(linear(4, 2, b"\x1b[?25l\x1b[?25h").cursor().visible);
}

/// `RIS` puts everything back: screen, cursor, region, modes, tabs, charset. Not the
/// title.
///
/// The bug: a partial reset. `reset` in a shell sends `ESC c`, and anything left over
/// afterwards, a scroll region most of all, makes the shell behave as though it were
/// still inside the program that crashed.
///
/// The region, the modes and the charset are proved by what they would do rather
/// than by reading them off the screen, because Ghostty owns them and does not
/// report them. After the reset the trailer prints `qqq` (so the graphics set is
/// gone), fills past the last column (so autowrap is back and insert mode is not),
/// and reaches row 4 (so the two-row scroll region is gone).
#[test]
fn ris_resets_every_piece_of_state() {
    let screen = linear(
        8,
        4,
        b"\x1b]0;title\x07\x1b[2;3r\x1b[?7l\x1b[4h\x1b(0\x1b[31mtext\x1b[2;2H\x1bcX",
    );
    assert_eq!(rows_of(&screen), vec!["X", "", "", ""]);
    assert_eq!(
        screen.title(),
        "title",
        "the window title survives RIS: it is a window property set over OSC, not part \
         of the VT state Ghostty resets, and the old parser clearing it was a local \
         invention"
    );
    let cell = screen.grid().cell(0, 0).expect("cell");
    assert_eq!(cell.fg, Palette::XTERM.fg, "the pen went back to default");

    let after = linear(
        8,
        4,
        b"\x1b[2;3r\x1b[?7l\x1b[4h\x1b(0\x1b[31m\x1bcqqqqqqqqZ\x1b[4;1Hbottom",
    );
    assert_eq!(
        rows_of(&after),
        vec!["qqqqqqqq", "Z", "", "bottom"],
        "ascii again, wrapping again, and the whole screen scrollable again"
    );
}

/// `DECALN` fills the screen with `E` and homes the cursor.
///
/// The fastest way to prove a replay is addressing every cell, and the first thing
/// `vttest` does.
#[test]
fn decaln_fills_the_screen_with_e_and_homes() {
    let screen = linear(4, 3, b"\x1b#8");
    assert_eq!(rows_of(&screen), vec!["EEEE", "EEEE", "EEEE"]);
    assert_eq!((screen.cursor().col, screen.cursor().row), (0, 0));
}

/// An OSC 0 or OSC 2 sets the title; OSC 1 and other numbers do not.
#[test]
fn osc_0_and_2_set_the_title_and_nothing_else_does() {
    assert_eq!(linear(4, 2, b"\x1b]0;first\x07").title(), "first");
    assert_eq!(linear(4, 2, b"\x1b]2;second\x1b\\").title(), "second");
    assert_eq!(
        linear(4, 2, b"\x1b]1;icon\x07").title(),
        "",
        "OSC 1 is the icon name, which no vitrum surface shows"
    );
    assert_eq!(
        linear(4, 2, b"\x1b]7373;working\x07").title(),
        "",
        "an agent hint is not a title"
    );
}
