//! Cursor addressing, origin mode, tab stops, and save/restore.
//!
//! Every full-screen program moves the cursor far more often than it prints. An
//! off-by-one here is not a cosmetic fault: `CUP` is one-based and everything else in
//! this crate is zero-based, so a parser that forgets the conversion draws every screen
//! one row and one column out.

use crate::tests::support::{GHOSTTY_ANSI, cell_at, linear, rows_of};

/// `CUP` is one-based, and the reported cursor is zero-based.
#[test]
fn cup_is_one_based_and_the_reported_position_is_zero_based() {
    let screen = linear(8, 4, b"\x1b[3;5H");

    assert_eq!(screen.cursor().row, 2);
    assert_eq!(screen.cursor().col, 4);
}

/// A bare `CSI H` and `CSI 1 ; 1 H` both home the cursor.
#[test]
fn a_bare_cup_homes_the_cursor() {
    let screen = linear(8, 4, b"\x1b[4;8Hx\x1b[H");

    assert_eq!(screen.cursor().row, 0);
    assert_eq!(screen.cursor().col, 0);
}

/// An address past the edge of the screen is clamped to the edge.
///
/// The bug: trusting the parameter. A program that addresses row 50 on a 24-row screen
/// is common (it is how `tput cup` behaves after a resize the program has not seen yet),
/// and an unclamped index is an out-of-bounds write or a wrapped row.
#[test]
fn an_address_past_the_edge_is_clamped() {
    let screen = linear(8, 4, b"\x1b[99;99Hx");

    assert_eq!(screen.cursor().row, 3);
    assert_eq!(cell_at(&screen, 7, 3).ch, 'x');
}

/// `CUU`, `CUD`, `CUF` and `CUB` move by their parameter and stop at the edges.
#[test]
fn the_relative_moves_step_by_their_parameter_and_stop_at_the_edges() {
    let moved = linear(8, 5, b"\x1b[3;4H\x1b[2A\x1b[3C");
    assert_eq!((moved.cursor().row, moved.cursor().col), (0, 6));

    let clamped = linear(8, 5, b"\x1b[3;4H\x1b[99A\x1b[99D");
    assert_eq!((clamped.cursor().row, clamped.cursor().col), (0, 0));

    let down = linear(8, 5, b"\x1b[1;1H\x1b[99B\x1b[99C");
    assert_eq!((down.cursor().row, down.cursor().col), (4, 7));
}

/// A zero parameter on a relative move means one.
///
/// `CSI 0 A` is a move of one row, not of none. Programs emit the zero form through
/// naive parameter formatting, and treating it as a no-op leaves them drawing on top of
/// themselves.
#[test]
fn a_zero_parameter_on_a_relative_move_means_one() {
    let screen = linear(8, 5, b"\x1b[3;3H\x1b[0A\x1b[0C");

    assert_eq!((screen.cursor().row, screen.cursor().col), (1, 3));
}

/// `CHA` and `VPA` address one axis and leave the other alone.
#[test]
fn cha_and_vpa_address_one_axis_each() {
    let screen = linear(8, 5, b"\x1b[3;3H\x1b[6G");
    assert_eq!((screen.cursor().row, screen.cursor().col), (2, 5));

    let vertical = linear(8, 5, b"\x1b[3;3H\x1b[5d");
    assert_eq!((vertical.cursor().row, vertical.cursor().col), (4, 2));
}

/// With origin mode set, `CUP` addresses rows relative to the scroll region.
///
/// The bug: ignoring `DECOM`. A program that sets a region and then addresses row 1
/// means the top of its region; treating it as the top of the screen draws over the
/// header the region was set to protect.
#[test]
fn origin_mode_makes_cup_relative_to_the_scroll_region() {
    // Region rows 2..4 (1-based), origin mode on, then address row 1 of the region.
    let screen = linear(8, 5, b"\x1b[2;4r\x1b[?6h\x1b[1;1HX");

    assert_eq!(rows_of(&screen), vec!["", "X", "", "", ""]);
    assert_eq!(screen.cursor().row, 1);
}

/// With origin mode set, the cursor cannot leave the region.
#[test]
fn origin_mode_confines_the_cursor_to_the_region() {
    let screen = linear(8, 5, b"\x1b[2;4r\x1b[?6h\x1b[99;1HX");

    assert_eq!(
        rows_of(&screen),
        vec!["", "", "", "X", ""],
        "the clamp stopped at the bottom of the region, not the bottom of the screen"
    );
}

/// Resetting origin mode homes the cursor to the top of the screen again.
#[test]
fn resetting_origin_mode_returns_addressing_to_the_screen() {
    let screen = linear(8, 5, b"\x1b[2;4r\x1b[?6h\x1b[?6l\x1b[1;1HX");

    assert_eq!(rows_of(&screen)[0], "X");
}

/// `HTS` sets a tab stop at the cursor and `TBC` clears them.
///
/// The bug: hardcoding stops every eight columns. `HTS` is how a program lays out a
/// table with columns that are not multiples of eight, and a hardcoded ruler puts every
/// field in the wrong place.
#[test]
fn hts_sets_a_stop_and_tbc_clears_every_stop() {
    let set = linear(24, 2, b"\x1b[3G\x1bH\x1b[1G\ta");
    assert_eq!(cell_at(&set, 2, 0).ch, 'a', "the tab landed on column 3");

    let cleared = linear(24, 2, b"\x1b[3g\x1b[1G\ta");
    assert_eq!(
        cleared.cursor().col,
        23,
        "with no stops left the tab runs to the right margin"
    );
}

/// `CSI 0 g` clears only the stop under the cursor.
#[test]
fn tbc_zero_clears_only_the_stop_under_the_cursor() {
    let screen = linear(32, 2, b"\x1b[9G\x1b[0g\x1b[1G\ta");

    assert_eq!(
        cell_at(&screen, 16, 0).ch,
        'a',
        "column 9 was cleared so the tab ran on to column 17"
    );
}

/// `CBT` steps backwards through tab stops.
#[test]
fn cbt_steps_back_to_the_previous_stop() {
    let screen = linear(32, 2, b"\x1b[20G\x1b[2Za");

    assert_eq!(screen.cursor().col, 9, "back two stops from column 20");
    assert_eq!(cell_at(&screen, 8, 0).ch, 'a');
}

/// `DECSC` and `DECRC` carry the position and the rendition together.
///
/// The bug: saving the position only. A program that saves, changes colour to draw a
/// status line, and restores would come back with the status colour still set and paint
/// the rest of its output in it.
#[test]
fn decsc_and_decrc_carry_the_position_and_the_rendition() {
    let screen = linear(8, 3, b"\x1b[2;2H\x1b[31m\x1b7\x1b[3;6H\x1b[32mG\x1b8R");

    assert_eq!(cell_at(&screen, 5, 2).ch, 'G');
    assert_eq!(cell_at(&screen, 5, 2).fg, GHOSTTY_ANSI[2]);
    assert_eq!(cell_at(&screen, 1, 1).ch, 'R', "the position came back");
    assert_eq!(
        cell_at(&screen, 1, 1).fg,
        GHOSTTY_ANSI[1],
        "the rendition came back with it"
    );
}

/// `CSI s` and `CSI u` are the same save and restore.
#[test]
fn csi_s_and_csi_u_save_and_restore_the_same_state() {
    let ansi = linear(8, 3, b"\x1b[2;2H\x1b[31m\x1b[s\x1b[3;6HG\x1b[uR");
    let dec = linear(8, 3, b"\x1b[2;2H\x1b[31m\x1b7\x1b[3;6HG\x1b8R");

    assert_eq!(ansi, dec);
}

/// A restore with nothing saved homes the cursor and resets the rendition.
///
/// The alternative is restoring uninitialised state, which is how a program that sends
/// `CSI u` defensively at start-up ends up drawing somewhere unpredictable.
#[test]
fn a_restore_with_nothing_saved_homes_the_cursor() {
    let screen = linear(8, 3, b"\x1b[3;6H\x1b[31m\x1b8X");

    assert_eq!((screen.cursor().row, screen.cursor().col), (0, 1));
    assert_eq!(
        cell_at(&screen, 0, 0).fg,
        crate::palette::Palette::DEFAULT.fg
    );
}

/// `DECTCEM` hides and shows the cursor.
///
/// The renderer draws a caret only when the cursor is visible, so this flag is the
/// difference between a full-screen program that flickers a caret across the screen as
/// it redraws and one that does not.
#[test]
fn dectcem_hides_and_shows_the_cursor() {
    assert!(linear(8, 3, b"").cursor().visible, "visible at power on");
    assert!(!linear(8, 3, b"\x1b[?25l").cursor().visible);
    assert!(linear(8, 3, b"\x1b[?25l\x1b[?25h").cursor().visible);
}

/// `RIS` puts the cursor home, clears the screen, and drops the rendition.
#[test]
fn ris_resets_the_cursor_and_the_screen() {
    let screen = linear(8, 3, b"\x1b[31mabc\r\ndef\x1bcX");

    assert_eq!(rows_of(&screen), vec!["X", "", ""]);
    assert_eq!(
        cell_at(&screen, 0, 0).fg,
        crate::palette::Palette::DEFAULT.fg
    );
}
