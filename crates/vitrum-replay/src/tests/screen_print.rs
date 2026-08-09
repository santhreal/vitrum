//! Printing, wrapping, wide characters, and insert mode.

use vitrum_grid::CellSlot;

use crate::tests::support::{linear, rows_of, small};

/// Filling the last column leaves the cursor on that column with a wrap pending,
/// and a following `CR` returns to the start of the *same* row.
///
/// This is the deferred wrap, and getting it wrong is the single most visible
/// emulator bug there is. A terminal that wraps eagerly puts the cursor on the next
/// row the instant the last column is filled; the `\r` that a progress bar sends
/// then returns to the start of the wrong line, and every redraw walks one row down
/// the screen.
#[test]
fn filling_the_last_column_defers_the_wrap_so_a_following_cr_stays_on_the_row() {
    let screen = small(b"0123456789");
    assert_eq!(screen.cursor().col, 9, "the cursor stays on the last column");
    assert_eq!(screen.cursor().row, 0);

    let after_cr = small(b"0123456789\rX");
    assert_eq!(rows_of(&after_cr)[0], "X123456789");
    assert_eq!(rows_of(&after_cr)[1], "", "nothing wrapped onto row 1");
}

/// One more printable character after the row is full takes the deferred wrap.
#[test]
fn one_more_character_takes_the_deferred_wrap() {
    let screen = small(b"0123456789A");
    assert_eq!(rows_of(&screen)[0], "0123456789");
    assert_eq!(rows_of(&screen)[1], "A");
    assert_eq!((screen.cursor().col, screen.cursor().row), (1, 1));
    assert_eq!(rows_of(&screen)[2], "", "and only one row was taken");
}

/// With autowrap off, printing past the last column overwrites it in place.
///
/// The bug: wrapping regardless of DECAWM. A full-screen program that turns autowrap
/// off and draws a border relies on the last column absorbing overflow; wrapping
/// there scrolls its whole layout up by a row.
#[test]
fn autowrap_off_overwrites_the_last_column_instead_of_wrapping() {
    let screen = small(b"\x1b[?7l0123456789ABC");
    assert_eq!(rows_of(&screen)[0], "012345678C");
    assert_eq!(rows_of(&screen)[1], "", "nothing reached row 1");
    assert_eq!(screen.cursor().col, 9, "parked on the last column, not wrapped");
}

/// A double-width character with one column left moves whole to the next row, and
/// the abandoned column is blanked.
///
/// Splitting it across the row edge would put half a glyph in each row, which is
/// what a naive `col += 2` produces and what makes CJK output unreadable.
#[test]
fn a_wide_character_with_one_column_left_moves_whole_to_the_next_row() {
    let screen = small("012345678\u{65e5}".as_bytes());
    assert_eq!(rows_of(&screen)[0], "012345678", "column 9 was left blank");
    let head = screen.grid().cell(0, 1).expect("cell");
    let tail = screen.grid().cell(1, 1).expect("cell");
    assert_eq!(head.ch, '\u{65e5}');
    assert_eq!(head.slot, CellSlot::WideHead);
    assert_eq!(tail.slot, CellSlot::WideTail);
    assert_eq!(screen.cursor().col, 2);
    assert_eq!(screen.cursor().row, 1);
}

/// A wide character claims two cells, as a head and a tail.
#[test]
fn a_wide_character_occupies_two_cells() {
    let screen = small("\u{ff21}b".as_bytes());
    assert_eq!(
        screen.grid().cell(0, 0).expect("cell").slot,
        CellSlot::WideHead
    );
    assert_eq!(
        screen.grid().cell(1, 0).expect("cell").slot,
        CellSlot::WideTail
    );
    assert_eq!(screen.grid().cell(2, 0).expect("cell").ch, 'b');
}

/// Overwriting half of a wide pair blanks the other half.
///
/// Leaving the orphan behind draws a double-width glyph in one column, overlapping
/// its neighbour. The grid repairs this, and this test proves the replay path
/// actually goes through the repairing write.
#[test]
fn overwriting_half_a_wide_pair_blanks_the_orphan() {
    let screen = small("\u{ff21}\r b".as_bytes());
    assert_eq!(rows_of(&screen)[0], " b");
    assert_eq!(screen.grid().cell(0, 0).expect("cell").ch, ' ');
    assert_eq!(
        screen.grid().cell(0, 0).expect("cell").slot,
        CellSlot::Single
    );
}

/// Insert mode shifts the rest of the row right instead of overwriting.
///
/// The bug: ignoring IRM. A line editor that inserts a character mid-line then
/// relies on the terminal to push the tail across would show the tail overwritten,
/// one character lost per keystroke.
#[test]
fn insert_mode_shifts_the_row_right_instead_of_overwriting() {
    let screen = small(b"abcdef\r\x1b[4hXY");
    assert_eq!(rows_of(&screen)[0], "XYabcdef");

    let without = small(b"abcdef\rXY");
    assert_eq!(rows_of(&without)[0], "XYcdef", "with IRM off it overwrites");
}

/// Insert mode drops what falls off the right edge rather than wrapping it.
#[test]
fn insert_mode_drops_what_falls_off_the_right_edge() {
    let screen = small(b"0123456789\r\x1b[4hAB");
    assert_eq!(rows_of(&screen)[0], "AB01234567");
    assert_eq!(rows_of(&screen)[1], "");
}

/// Backspace moves left and clears a pending wrap, and never leaves column zero.
///
/// A backspace at column zero that wrapped to the previous row's end would let a
/// prompt redraw walk backwards up the screen.
#[test]
fn backspace_stops_at_column_zero_and_clears_a_pending_wrap() {
    let screen = small(b"ab\x08\x08\x08\x08X");
    assert_eq!(rows_of(&screen)[0], "Xb");
    assert_eq!(screen.cursor().col, 1);

    let after_full_row = small(b"0123456789\x08X");
    assert_eq!(
        rows_of(&after_full_row)[0],
        "01234567X9",
        "the pending wrap was cancelled, so the write stayed on row 0"
    );
}

/// A byte that is not valid UTF-8 prints one replacement character and does not
/// consume the bytes around it.
///
/// This is the behaviour a real terminal has and the behaviour a replay must match,
/// because the fixture, and every `git log` of a repository with a Latin-1 commit
/// message, contains these bytes.
#[test]
fn an_invalid_utf8_byte_prints_one_replacement_character() {
    let screen = small(b"a\xffb");
    assert_eq!(rows_of(&screen)[0], "a\u{fffd}b");
}

/// A UTF-8 character split across two feeds is printed once, when it completes.
///
/// The PTY read boundary lands mid-character constantly. A replay that printed the
/// lead byte as a replacement character and then the continuation bytes as two more
/// would turn every accented word into mojibake.
#[test]
fn a_utf8_character_split_across_feeds_prints_once_when_it_completes() {
    use crate::emulator::Emulator;
    use crate::palette::Palette;

    let mut emulator = Emulator::new(10, 2, Palette::XTERM).expect("geometry");
    let bytes = "é".as_bytes();
    emulator.feed(&bytes[..1]).expect("engine readable");
    assert_eq!(
        emulator.screen().line(0).trim_end(),
        "",
        "nothing is drawn from half a character"
    );
    emulator.feed(&bytes[1..]).expect("engine readable");
    assert_eq!(emulator.screen().line(0).trim_end(), "é");
}

/// A combining mark is dropped, leaving the base character standing.
///
/// The grid holds one `char` per cell and cannot compose. Dropping the mark is the
/// closest available answer; printing it into its own cell would shift the whole rest
/// of the line right by one column.
#[test]
fn a_combining_mark_is_dropped_rather_than_taking_a_cell() {
    let screen = small("e\u{301}x".as_bytes());
    assert_eq!(rows_of(&screen)[0], "ex");
}

/// A tab moves to the next eight-column stop and never leaves the row.
#[test]
fn a_tab_moves_to_the_next_eight_column_stop() {
    let screen = linear(24, 2, b"a\tb\tc");
    assert_eq!(rows_of(&screen)[0], "a       b       c");
}
