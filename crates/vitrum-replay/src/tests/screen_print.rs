//! Printing: wrapping, wide characters, combining marks, insert mode, tabs, and a
//! sequence cut in half by a chunk boundary.
//!
//! This is the parser's own conformance suite. There is one VT parser in the product
//! now, the engine behind [`crate::Emulator`], and the daemon paints a live pane with
//! it. So every assertion here is a statement about what an operator sees, not about
//! an implementation detail of a replay.

use vitrum_grid::CellSlot;

use crate::tests::support::{cell_at, linear, rows_of, small, split_feed};

/// Text prints left to right and the cursor ends one column past the last character.
#[test]
fn text_prints_across_the_row_and_leaves_the_cursor_after_it() {
    let screen = small(b"abc");

    assert_eq!(rows_of(&screen)[0], "abc");
    assert_eq!(screen.cursor().col, 3);
    assert_eq!(screen.cursor().row, 0);
}

/// A line longer than the screen wraps onto the next row.
///
/// The bug this stops: clamping at the right margin. Every command whose output is
/// wider than the pane would lose its tail, silently, and a user resizing narrower
/// would watch text disappear rather than reflow.
#[test]
fn a_line_longer_than_the_row_wraps_to_the_next_one() {
    let screen = linear(4, 3, b"abcdefg");

    assert_eq!(rows_of(&screen), vec!["abcd", "efg", ""]);
    assert_eq!(screen.cursor().row, 1);
    assert_eq!(screen.cursor().col, 3);
}

/// With autowrap off (`DECAWM` reset) the last column is overwritten instead.
///
/// The bug: implementing wrap unconditionally. `CSI ? 7 l` is how a program draws a
/// character in the bottom right corner without scrolling the screen; wrapping there
/// scrolls the whole pane by one row on every frame.
#[test]
fn with_autowrap_off_the_last_column_is_overwritten() {
    let screen = linear(4, 2, b"\x1b[?7labcdefg");

    assert_eq!(
        rows_of(&screen),
        vec!["abcg", ""],
        "each character past the margin replaced the one in the last column"
    );
}

/// The cursor pauses on the last column rather than wrapping the moment it is filled.
///
/// This is the pending-wrap state, and it is the difference between a program that can
/// fill the bottom-right cell and one that cannot. Writing the last column must not
/// move the cursor off the row; the next printable character is what wraps.
#[test]
fn filling_the_last_column_leaves_the_cursor_on_it_until_the_next_character() {
    let filled = linear(4, 3, b"abcd");
    assert_eq!(filled.cursor().row, 0, "the row must not advance yet");
    assert_eq!(filled.cursor().col, 3);

    let wrapped = linear(4, 3, b"abcde");
    assert_eq!(wrapped.cursor().row, 1);
    assert_eq!(wrapped.cursor().col, 1);
}

/// A double-width character claims two columns: a head that carries it and a tail that
/// draws nothing.
///
/// The bug: storing a wide character in one cell. Everything after it on the row shifts
/// one column left of where the session put it, so a CJK log line is misaligned from
/// its first character onwards.
#[test]
fn a_wide_character_occupies_a_head_and_a_tail() {
    let screen = linear(8, 2, "\u{65e5}x".as_bytes());

    let head = cell_at(&screen, 0, 0);
    let tail = cell_at(&screen, 1, 0);
    let after = cell_at(&screen, 2, 0);

    assert_eq!(head.ch, '\u{65e5}');
    assert_eq!(head.slot, CellSlot::WideHead);
    assert_eq!(tail.slot, CellSlot::WideTail);
    assert_eq!(after.ch, 'x', "the next character starts at column 2");
    assert_eq!(screen.cursor().col, 3);
}

/// A wide character that does not fit the last column moves to the next row whole.
///
/// The bug: splitting the pair across the wrap, which puts a head in the last column
/// and a tail in the first column of the next row. Nothing draws that correctly.
#[test]
fn a_wide_character_that_does_not_fit_moves_to_the_next_row_whole() {
    let screen = linear(4, 3, "abc\u{65e5}".as_bytes());

    assert_eq!(
        cell_at(&screen, 3, 0).ch,
        ' ',
        "the last column of row 0 is left blank rather than half a character"
    );
    assert_eq!(cell_at(&screen, 0, 1).ch, '\u{65e5}');
    assert_eq!(cell_at(&screen, 0, 1).slot, CellSlot::WideHead);
    assert_eq!(cell_at(&screen, 1, 1).slot, CellSlot::WideTail);
}

/// A combining mark attaches to the character before it and claims no column of its own.
///
/// A grid cell is sixteen bytes and holds one `char`, so the cluster is flattened to its
/// base codepoint. What must never happen is the mark taking a column: that would push
/// the rest of the line right by one for every accent in it.
#[test]
fn a_combining_mark_claims_no_column_of_its_own() {
    // "e" + COMBINING ACUTE ACCENT, then a plain "f".
    let screen = linear(8, 2, "e\u{0301}f".as_bytes());

    assert_eq!(cell_at(&screen, 0, 0).ch, 'e');
    assert_eq!(cell_at(&screen, 1, 0).ch, 'f', "the mark took no column");
    assert_eq!(screen.cursor().col, 2);
}

/// A combining mark arriving before any character does not create one.
#[test]
fn a_leading_combining_mark_does_not_create_a_cell() {
    let screen = linear(8, 2, "\u{0301}a".as_bytes());

    assert_eq!(rows_of(&screen)[0], "a");
}

/// `IRM` (`CSI 4 h`) pushes the rest of the row right instead of overwriting it.
#[test]
fn insert_mode_pushes_the_rest_of_the_row_right() {
    let screen = linear(8, 2, b"abcdef\x1b[1;1H\x1b[4hXY");

    assert_eq!(
        rows_of(&screen)[0],
        "XYabcdef",
        "the tail moved right rather than being overwritten"
    );
}

/// With `IRM` reset, which is the power-on state, printing overwrites.
#[test]
fn replace_mode_overwrites_the_row() {
    let screen = linear(8, 2, b"abcdef\x1b[1;1HXY");

    assert_eq!(rows_of(&screen)[0], "XYcdef");
}

/// `HT` moves to the next eight-column tab stop rather than printing anything.
#[test]
fn a_horizontal_tab_moves_to_the_next_stop() {
    let screen = linear(24, 2, b"a\tb\tc");

    assert_eq!(screen.cursor().col, 17);
    assert_eq!(cell_at(&screen, 0, 0).ch, 'a');
    assert_eq!(cell_at(&screen, 8, 0).ch, 'b');
    assert_eq!(cell_at(&screen, 16, 0).ch, 'c');
}

/// `BS` moves back one column without erasing, and stops at column zero.
#[test]
fn backspace_moves_left_without_erasing_and_stops_at_the_margin() {
    let screen = linear(8, 2, b"abc\x08\x08\x08\x08\x08");

    assert_eq!(rows_of(&screen)[0], "abc", "backspace erases nothing");
    assert_eq!(screen.cursor().col, 0);
}

/// A sequence cut in half by a chunk boundary is completed by the next chunk.
///
/// A PTY read returns whatever had arrived. The split is not rare, it is the normal
/// case for any output larger than a pipe buffer, and a parser that resets on a chunk
/// boundary prints escape bytes as text on a busy session. Every kind of split is here
/// because they fail separately: a CSI has parameters mid-flight, UTF-8 has a
/// continuation byte, and an OSC has a two-byte terminator.
#[test]
fn a_sequence_split_across_two_feeds_is_completed_by_the_second() {
    // CSI cut between the parameter and the final byte.
    let csi = split_feed(8, 2, &[b"\x1b[3", b"1mR"]);
    assert_eq!(rows_of(&csi)[0], "R");
    assert_eq!(
        cell_at(&csi, 0, 0).fg,
        crate::tests::support::GHOSTTY_ANSI[1],
        "the colour survived the split"
    );

    // A three-byte UTF-8 character cut after its first byte.
    let utf8 = split_feed(8, 2, &[b"a\xe6", b"\x97\xa5b"]);
    assert_eq!(cell_at(&utf8, 1, 0).ch, '\u{65e5}');
    assert_eq!(cell_at(&utf8, 3, 0).ch, 'b');

    // An OSC cut inside its payload, terminated by ST in the second chunk.
    let osc = split_feed(8, 2, &[b"\x1b]0;he", b"llo\x1b\\x"]);
    assert_eq!(osc.title(), "hello");
    assert_eq!(rows_of(&osc)[0], "x");

    // An escape cut from its intermediate and final bytes.
    let charset = split_feed(8, 2, &[b"\x1b", b"(0qqq"]);
    assert_eq!(rows_of(&charset)[0], "\u{2500}\u{2500}\u{2500}");
}

/// Splitting the whole capture one byte at a time produces the same screen as one feed.
///
/// The strongest form of the property above: there is no byte position at which a split
/// changes the outcome. The capture is real PTY output, so it puts the boundary inside
/// every construct it contains rather than inside the ones a fixture author thought of.
#[test]
fn feeding_the_capture_one_byte_at_a_time_matches_one_feed() {
    let whole = linear(80, 24, crate::tests::support::CAPTURED);

    let chunks: Vec<&[u8]> = crate::tests::support::CAPTURED
        .chunks(1)
        .collect();
    let byte_at_a_time = split_feed(80, 24, &chunks);

    assert_eq!(rows_of(&byte_at_a_time), rows_of(&whole));
    assert_eq!(byte_at_a_time, whole);
}
