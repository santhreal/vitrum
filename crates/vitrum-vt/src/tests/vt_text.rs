//! What a byte stream puts on the screen.
//!
//! These are the sequences a shell session actually emits, and each one is
//! checked through the grid rather than through the engine, because the grid is
//! what a user sees. A parser that is right and a projection that is wrong look
//! identical from inside the engine.

use super::support::Fixture;

#[test]
fn plain_text_lands_on_the_first_row() {
    let mut fx = Fixture::new(20, 3);
    fx.write(b"hello");
    assert_eq!(fx.line(0), "hello");
}

#[test]
fn a_newline_moves_to_the_next_row() {
    let mut fx = Fixture::new(20, 3);
    fx.write(b"one\r\ntwo\r\nthree");
    assert_eq!(fx.lines(), ["one", "two", "three"]);
}

#[test]
fn a_bare_newline_does_not_return_the_carriage() {
    // Without CR the cursor keeps its column, so the second word starts under
    // the end of the first. A terminal that "helpfully" returns the carriage
    // breaks every program that draws with absolute positioning.
    let mut fx = Fixture::new(20, 3);
    fx.write(b"one\ntwo");
    assert_eq!(fx.lines(), ["one", "   two", ""]);
}

#[test]
fn text_past_the_last_column_wraps_to_the_next_row() {
    let mut fx = Fixture::new(5, 3);
    fx.write(b"abcdefgh");
    assert_eq!(fx.lines(), ["abcde", "fgh", ""]);
}

#[test]
fn cursor_addressing_places_text_exactly() {
    let mut fx = Fixture::new(10, 3);
    // CUP is 1-based; row 2 column 3 is grid (2, 1).
    fx.write(b"\x1b[2;3Hx");
    assert_eq!(fx.lines(), ["", "  x", ""]);
}

#[test]
fn erase_in_line_clears_what_was_there() {
    let mut fx = Fixture::new(10, 2);
    fx.write(b"stale text");
    assert_eq!(fx.line(0), "stale text");

    fx.write(b"\r\x1b[2K");
    assert_eq!(fx.line(0), "");
}

#[test]
fn erase_in_display_clears_every_row() {
    let mut fx = Fixture::new(10, 3);
    fx.write(b"one\r\ntwo\r\nthree");
    fx.write(b"\x1b[2J");
    assert_eq!(fx.lines(), ["", "", ""]);
}

#[test]
fn a_backspace_moves_left_without_erasing() {
    // BS is a cursor move. The character is removed by the space that a shell
    // sends after it, not by BS itself, and a terminal that erases on BS
    // corrupts progress bars that back up to redraw.
    let mut fx = Fixture::new(10, 1);
    fx.write(b"abc\x08");
    assert_eq!(fx.line(0), "abc");
    fx.write(b" ");
    assert_eq!(fx.line(0), "ab");
}

#[test]
fn a_tab_advances_to_the_next_stop() {
    let mut fx = Fixture::new(20, 1);
    fx.write(b"a\tb");
    assert_eq!(fx.line(0), "a       b");
}

#[test]
fn scrolling_off_the_top_keeps_the_last_rows() {
    let mut fx = Fixture::new(10, 3);
    fx.write(b"one\r\ntwo\r\nthree\r\nfour");
    assert_eq!(fx.lines(), ["two", "three", "four"]);
}

#[test]
fn a_shrinking_line_does_not_leave_its_old_tail() {
    // The engine reports every column of a row, blanks included, so a cleared
    // tail arrives as blank cells rather than as missing ones. This pins that
    // invariant: if it ever stopped holding, "long line" overwritten by "hi"
    // would read as "hi   line" and nothing else in the suite would notice.
    let mut fx = Fixture::new(12, 1);
    fx.write(b"long line xx");
    assert_eq!(fx.line(0), "long line xx");

    fx.write(b"\r\x1b[2Khi");
    assert_eq!(fx.line(0), "hi");
}

#[test]
fn utf8_split_across_two_writes_still_forms_one_character() {
    // A PTY read can end anywhere, including the middle of a multi-byte
    // character. The engine must hold the partial sequence rather than emit a
    // replacement character.
    let mut fx = Fixture::new(10, 1);
    let bytes = "é".as_bytes();
    fx.write(&bytes[..1]);
    fx.write(&bytes[1..]);
    assert_eq!(fx.line(0), "é");
}

#[test]
fn the_alternate_screen_hides_the_primary_one() {
    let mut fx = Fixture::new(10, 2);
    fx.write(b"primary");
    fx.write(b"\x1b[?1049h");
    assert_eq!(fx.line(0), "");

    // DECSET 1049 saves the cursor and clears the alternate screen, but it does
    // not home the cursor, so the text lands at the column the shell left it on.
    // A full-screen program positions itself explicitly, which is what this
    // does before checking the content.
    fx.write(b"\x1b[1;1Halt");
    assert_eq!(fx.line(0), "alt");

    fx.write(b"\x1b[?1049l");
    assert_eq!(fx.line(0), "primary");
}
