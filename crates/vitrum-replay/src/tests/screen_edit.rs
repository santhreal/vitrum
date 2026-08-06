//! ICH and DCH: editing within one row.

use crate::tests::support::{linear, rows_of};

/// `CSI @` opens blank cells at the cursor and pushes the tail right.
#[test]
fn ich_opens_blank_cells_and_pushes_the_tail_right() {
    let screen = linear(10, 2, b"abcdef\r\x1b[3C\x1b[2@");
    assert_eq!(rows_of(&screen)[0], "abc  def");
}

/// `CSI @` drops the characters that fall off the right edge.
#[test]
fn ich_drops_what_falls_off_the_right_edge() {
    let screen = linear(6, 2, b"abcdef\r\x1b[2@");
    assert_eq!(rows_of(&screen)[0], "  abcd");
}

/// `CSI P` removes cells at the cursor and pulls the tail left, blanking the end.
#[test]
fn dch_pulls_the_tail_left_and_blanks_the_end() {
    let screen = linear(10, 2, b"abcdefghij\r\x1b[3C\x1b[2P");
    assert_eq!(rows_of(&screen)[0], "abcfghij");
    let last = screen.grid().cell(9, 0).expect("cell");
    assert_eq!(last.ch, ' ');
}

/// Neither `ICH` nor `DCH` moves the cursor.
///
/// The bug: advancing the cursor by the count. A line editor issues `CSI P` and then
/// writes the replacement at the same position; a moved cursor puts it one character
/// further right on every edit.
#[test]
fn ich_and_dch_leave_the_cursor_where_it_was() {
    let inserted = linear(10, 2, b"abcdef\r\x1b[3C\x1b[2@X");
    assert_eq!(rows_of(&inserted)[0], "abcX def");
    assert_eq!(inserted.cursor().col, 4);

    let deleted = linear(10, 2, b"abcdef\r\x1b[3C\x1b[2PX");
    assert_eq!(rows_of(&deleted)[0], "abcX");
    assert_eq!(deleted.cursor().col, 4);
}

/// A count larger than the row clears from the cursor to the end instead of reading
/// past it.
#[test]
fn a_count_larger_than_the_row_clamps() {
    let inserted = linear(6, 2, b"abcdef\r\x1b[2C\x1b[99@");
    assert_eq!(rows_of(&inserted)[0], "ab");

    let deleted = linear(6, 2, b"abcdef\r\x1b[2C\x1b[99P");
    assert_eq!(rows_of(&deleted)[0], "ab");
}

/// An absent count means one, for both.
#[test]
fn an_absent_count_means_one() {
    let inserted = linear(8, 2, b"abcdef\r\x1b[@");
    assert_eq!(rows_of(&inserted)[0], " abcdef");

    let deleted = linear(8, 2, b"abcdef\r\x1b[P");
    assert_eq!(rows_of(&deleted)[0], "bcdef");
}

/// Both cancel a pending wrap.
///
/// Editing the row the cursor is waiting to wrap off means the program is still on
/// that row, so the deferred wrap must not fire on the next character.
#[test]
fn editing_cancels_a_pending_wrap() {
    let screen = linear(6, 2, b"abcdef\x1b[PX");
    assert_eq!(rows_of(&screen)[0], "abcdeX");
    assert_eq!(rows_of(&screen)[1], "");
}
