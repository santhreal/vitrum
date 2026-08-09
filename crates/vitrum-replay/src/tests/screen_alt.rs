//! The alternate screen, and what 1049 stashes.
//!
//! The old parser kept the inactive buffer, the alternate-screen flag and the
//! 1049 cursor stash on [`crate::Screen`], and these tests read them directly.
//! Ghostty keeps them now and does not hand them back, so every assertion here is
//! against what the buffer swap *does*: which text is on screen, and where the
//! cursor and the pen land afterwards. That is what a user sees and what a bug in
//! the swap would break, and it is a strictly stronger thing to assert than a
//! boolean the parser set itself.

use crate::tests::support::{GHOSTTY_ANSI, linear, rows_of};

/// Entering the alternate screen hides the primary buffer's content, and leaving it
/// brings the content back untouched.
///
/// This is the whole point of the alternate screen and the thing a user notices
/// instantly: run `vim`, quit, and your shell history should still be there. A replay
/// that painted the full-screen program over the primary buffer would show the
/// session's scrollback destroyed at the moment any TUI ran.
#[test]
fn leaving_the_alternate_screen_restores_the_primary_content() {
    let screen = linear(
        12,
        3,
        b"shell line\r\n\x1b[?1049h\x1b[HFULL SCREEN\x1b[?1049lback\r\n",
    );
    assert_eq!(rows_of(&screen)[0], "shell line");
    assert_eq!(
        rows_of(&screen)[1],
        "back",
        "output after rmcup resumed where the shell left off"
    );
    assert!(
        !rows_of(&screen).iter().any(|row| row.contains("FULL")),
        "the TUI's paint is on the buffer that was put away"
    );
}

/// The alternate screen starts blank rather than showing what the primary had.
///
/// The bug: switching buffers without clearing. A TUI that only paints the parts it
/// cares about would show the shell's text bleeding through the gaps.
#[test]
fn the_alternate_screen_starts_blank() {
    let screen = linear(12, 3, b"aaaa\r\nbbbb\r\ncccc\x1b[?1049h");
    assert_eq!(rows_of(&screen), vec!["", "", ""]);
}

/// 1049 stashes the primary buffer's cursor and rendition and puts them back on exit.
///
/// That is the difference between 1049 and the older 1047, and it is why a program
/// using it leaves the shell prompt exactly where it found it. Without the stash the
/// prompt reappears wherever the TUI happened to leave the cursor.
#[test]
fn ten_forty_nine_restores_the_cursor_and_the_rendition() {
    let screen = linear(
        12,
        4,
        b"\x1b[2;5H\x1b[31m\x1b[?1049h\x1b[4;1Hdeep\x1b[32m\x1b[?1049lX",
    );
    assert_eq!(rows_of(&screen)[1], "    X", "back at row 2, column 5");
    let cell = screen.grid().cell(4, 1).expect("cell");
    assert_eq!(
        cell.fg, GHOSTTY_ANSI[1],
        "the red the shell was using came back, not the TUI's green"
    );
}

/// Entering with 1049 leaves the cursor where it was.
///
/// The old parser homed it, and that was wrong. xterm defines 1049 as 1047 plus 1048:
/// save the cursor, switch to the alternate buffer, clear it. Nothing in either half
/// homes. A program that paints its first line without addressing the cursor gets that
/// line wherever the shell prompt had left it, which is what a real terminal does and
/// what its author will have compensated for.
///
/// The bug this stops: reintroducing the home, which silently moves the first line of
/// every full-screen program's output in every replay.
#[test]
fn entering_with_ten_forty_nine_leaves_the_cursor_where_it_was() {
    let screen = linear(12, 3, b"\x1b[3;7H\x1b[?1049hX");
    assert_eq!(rows_of(&screen)[2], "      X", "still at row 3, column 7");
    assert_eq!(rows_of(&screen)[0], "", "and nothing was painted at the top");

    let older = linear(12, 3, b"\x1b[3;7H\x1b[?1047hX");
    assert_eq!(
        rows_of(&older)[2],
        "      X",
        "1047 is the half of 1049 that does the switch, and it does not home either"
    );
}

/// The older 47 and 1047 spellings swap the buffer and leave the cursor alone.
///
/// The bug: treating them as 1049. A program using `smcup`/`rmcup` built from 47 keeps
/// managing the cursor itself, and moving it under the program's feet puts its first
/// line of output in the wrong place.
#[test]
fn forty_seven_swaps_the_buffer_without_touching_the_cursor() {
    let screen = linear(12, 3, b"\x1b[2;4H\x1b[?47hX");
    assert_eq!(rows_of(&screen)[1], "   X", "still at row 2, column 4");
}

/// Entering when already on the alternate screen is a no-op.
///
/// The bug: stashing again, which overwrites the primary buffer's saved cursor with the
/// alternate buffer's, so exiting lands in the wrong place. Two `smcup`s in a row happen
/// whenever a program spawns another full-screen program.
#[test]
fn entering_twice_does_not_lose_the_primary_buffer() {
    let screen = linear(12, 3, b"shell\x1b[?1049h\x1b[?1049hinner\x1b[?1049lX");
    assert_eq!(rows_of(&screen)[0], "shellX");
}

/// Leaving when never entered is a no-op and does not blank the screen.
///
/// A stray `rmcup` from a crashed program must not wipe the shell's output.
#[test]
fn leaving_without_entering_changes_nothing() {
    let screen = linear(12, 2, b"shell output\x1b[?1049l");
    assert_eq!(rows_of(&screen)[0], "shell output");
}

/// The alternate buffer has its own content, kept separate from the primary.
///
/// The write into the alternate buffer is addressed with an explicit home, because
/// 1049 does not move the cursor and an unaddressed write would land at column 7
/// where `primary` ended and wrap.
#[test]
fn the_two_buffers_hold_separate_content() {
    let on_alt = linear(12, 2, b"primary\x1b[?1049h\x1b[Halternate");
    assert_eq!(rows_of(&on_alt)[0], "alternate");

    let back = linear(12, 2, b"primary\x1b[?1049h\x1b[Halternate\x1b[?1049l");
    assert_eq!(rows_of(&back)[0], "primary");

    let unaddressed = linear(12, 2, b"primary\x1b[?1049halternate");
    assert_eq!(
        rows_of(&unaddressed),
        vec!["       alter".to_string(), "nate".to_string()],
        "with no addressing the text starts where the primary cursor was and wraps"
    );
}

/// A screen costs one grid whether or not a full-screen program ever ran.
///
/// It used to cost two once a TUI appeared, because the inactive buffer was a second
/// [`crate::Screen`] field and a keyframe cloned both. The inactive buffer belongs to
/// the engine now, and a [`crate::Screen`] is the projection of whichever buffer is
/// showing, so the memory this crate reports is flat.
///
/// The bug this stops: a projection that starts keeping its own copy of the buffer
/// that is not on screen, which nothing would read and every screen would pay for.
#[test]
fn a_screen_costs_one_grid_whether_or_not_a_tui_ran() {
    let plain = linear(80, 24, b"just a shell\r\n");
    let with_alt = linear(80, 24, b"just a shell\r\n\x1b[?1049h");
    assert_eq!(
        plain.heap_bytes(),
        with_alt.heap_bytes(),
        "the alternate buffer is the engine's, not the projection's"
    );
    assert_eq!(
        plain.heap_bytes(),
        80 * 24 * 16 + 24 * 4,
        "one grid of cells plus one damage span per row"
    );
}
