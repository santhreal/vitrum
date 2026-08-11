//! The alternate screen, and the cursor stash `1049` adds to it.
//!
//! Every full-screen program a session runs (a pager, an editor, a TUI agent front end)
//! enters the alternate screen on the way in and leaves it on the way out. If the
//! primary screen does not come back exactly as it was, the operator loses the shell
//! output they were reading, which is the single most visible way a terminal can be
//! wrong.

use crate::tests::support::{GHOSTTY_ANSI, cell_at, linear, rows_of};

/// `smcup` and `rmcup` as a terminal database actually emits them for `1049`.
const ENTER: &[u8] = b"\x1b[?1049h";
const LEAVE: &[u8] = b"\x1b[?1049l";

/// Feed `bytes` with the alternate screen entered and left around the middle section.
fn excursion(primary: &[u8], alt: &[u8], after: &[u8]) -> crate::screen::Screen {
    let mut input = Vec::new();
    input.extend_from_slice(primary);
    input.extend_from_slice(ENTER);
    input.extend_from_slice(alt);
    input.extend_from_slice(LEAVE);
    input.extend_from_slice(after);
    linear(8, 4, &input)
}

/// Entering the alternate screen shows a blank screen.
///
/// The bug: switching a flag without switching the buffer. The program's first frame
/// then draws on top of the shell's output, and every cell it does not paint shows the
/// shell underneath.
#[test]
fn entering_the_alternate_screen_shows_a_blank_one() {
    let mut input = b"one\r\ntwo".to_vec();
    input.extend_from_slice(ENTER);
    let screen = linear(8, 4, &input);

    assert_eq!(rows_of(&screen), vec!["", "", "", ""]);
}

/// Leaving restores the primary screen cell for cell.
#[test]
fn leaving_restores_the_primary_screen_exactly() {
    let restored = excursion(b"one\r\ntwo\r\nthree", b"\x1b[HPAGER", b"");
    let untouched = linear(8, 4, b"one\r\ntwo\r\nthree");

    assert_eq!(rows_of(&restored), rows_of(&untouched));
    assert_eq!(restored, untouched, "including the cursor");
}

/// `1049` saves the cursor on the way in and restores it on the way out.
///
/// This is the whole difference between `1049` and `47`. A shell prompt sits mid-row
/// when a pager starts; without the stash the prompt is redrawn at whatever position the
/// pager happened to leave the cursor in.
#[test]
fn the_cursor_comes_back_where_it_was_left() {
    let screen = excursion(b"\x1b[3;5H", b"\x1b[1;1Hxxxx", b"");

    assert_eq!((screen.cursor().row, screen.cursor().col), (2, 4));
}

/// Writing on the alternate screen leaves the primary untouched.
#[test]
fn work_done_on_the_alternate_screen_does_not_reach_the_primary() {
    let screen = excursion(b"keep", b"\x1b[2Jgone\r\nalso gone", b"");

    assert_eq!(rows_of(&screen)[0], "keep");
}

/// Output after leaving continues on the primary screen where the cursor was.
#[test]
fn output_after_leaving_continues_on_the_primary_screen() {
    let screen = excursion(b"ab", b"zzz", b"cd");

    assert_eq!(rows_of(&screen)[0], "abcd");
}

/// Entering twice is not a second save, and the primary survives.
///
/// The bug: keeping a stack. A program that sends `smcup` again after a resize would
/// push a second copy, and the shell's screen would come back only after two `rmcup`s,
/// which nothing sends.
#[test]
fn entering_twice_still_leaves_in_one_step() {
    let mut input = b"keep\x1b[2;3H".to_vec();
    input.extend_from_slice(ENTER);
    input.extend_from_slice(b"alt");
    input.extend_from_slice(ENTER);
    input.extend_from_slice(b"more");
    input.extend_from_slice(LEAVE);
    let screen = linear(8, 4, &input);

    assert_eq!(rows_of(&screen)[0], "keep");
    assert_eq!((screen.cursor().row, screen.cursor().col), (1, 2));
}

/// Leaving when never entered does not blank the primary screen.
///
/// `rmcup` at start-up is a common defensive emission. Treating it as a switch would
/// blank the shell's screen the moment a program started.
///
/// It is not a no-op: `1049 l` restores the stashed cursor, and with nothing stashed
/// that is the home position. The screen is what must survive, and it does.
#[test]
fn leaving_without_entering_does_not_blank_the_screen() {
    let mut input = b"keep".to_vec();
    input.extend_from_slice(LEAVE);
    let screen = linear(8, 4, &input);

    assert_eq!(rows_of(&screen)[0], "keep");
    assert_eq!((screen.cursor().row, screen.cursor().col), (0, 0));
}

/// `47` switches the buffer and does not touch the cursor.
///
/// The older mode. A program using it saves the cursor itself with `DECSC`, so a parser
/// that stashed the cursor here as well would restore twice and land in the wrong place.
#[test]
fn mode_forty_seven_switches_the_buffer_without_stashing_the_cursor() {
    let screen = linear(8, 4, b"keep\x1b[3;5H\x1b[?47h\x1b[1;1Halt\x1b[?47l");

    assert_eq!(rows_of(&screen)[0], "keep");
    assert_eq!(
        (screen.cursor().row, screen.cursor().col),
        (0, 3),
        "the cursor stayed where the alternate screen left it"
    );
}

/// `1048` stashes and restores the cursor without switching buffers.
#[test]
fn mode_ten_forty_eight_stashes_the_cursor_without_switching_buffers() {
    let screen = linear(8, 4, b"\x1b[3;5H\x1b[?1048h\x1b[1;1Hx\x1b[?1048lY");

    assert_eq!(rows_of(&screen)[0], "x", "no buffer switch happened");
    assert_eq!(cell_at(&screen, 4, 2).ch, 'Y', "the cursor came back");
}

/// A scroll region set on the alternate screen is still set on the primary.
///
/// The margins are terminal state, not screen state: `1049` stashes and restores the
/// cursor and its rendition, and nothing else. That is why `rmcup` in a terminal
/// database is a mode reset and why every full-screen program sends `CSI r` of its own
/// on the way out.
///
/// It is asserted rather than merely tolerated because it is the difference between a
/// shell that scrolls in the whole pane afterwards and one that scrolls in the pager's
/// window, and because a caller that resets the region gets the whole screen back.
#[test]
fn the_scroll_region_is_terminal_state_and_survives_the_excursion() {
    let survived = excursion(b"", b"\x1b[1;2r", b"a\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(
        rows_of(&survived),
        vec!["d", "e", "", ""],
        "output after the excursion scrolled inside the two-row region"
    );

    let reset = excursion(b"", b"\x1b[1;2r", b"\x1b[ra\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(
        rows_of(&reset),
        vec!["b", "c", "d", "e"],
        "CSI r put the whole screen back"
    );
}

/// A colour set on the alternate screen does not leak back to the primary.
#[test]
fn a_colour_set_on_the_alternate_screen_does_not_leak_back() {
    let screen = excursion(b"", b"\x1b[41m", b"X");

    assert_eq!(
        cell_at(&screen, 0, 0).bg,
        crate::palette::Palette::DEFAULT.bg
    );
}

/// The alternate screen still honours back-colour erase in its own right.
#[test]
fn the_alternate_screen_erases_in_its_own_background() {
    let mut input = Vec::new();
    input.extend_from_slice(ENTER);
    input.extend_from_slice(b"\x1b[42m\x1b[2J");
    let screen = linear(8, 4, &input);

    assert_eq!(cell_at(&screen, 3, 2).bg, GHOSTTY_ANSI[2]);
}
