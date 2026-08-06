//! The alternate screen, and what 1049 stashes.

use crate::palette::Palette;
use crate::tests::support::{linear, rows_of};

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
    assert!(!screen.on_alt_screen());
    assert_eq!(
        rows_of(&screen)[1],
        "back",
        "output after rmcup resumed where the shell left off"
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
    assert!(screen.on_alt_screen());
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
        cell.fg,
        Palette::XTERM.indexed(1),
        "the red the shell was using came back, not the TUI's green"
    );
}

/// Entering with 1049 homes the cursor, so a program that paints without addressing
/// starts at the top left.
#[test]
fn entering_with_ten_forty_nine_homes_the_cursor() {
    let screen = linear(12, 3, b"\x1b[3;7H\x1b[?1049hX");
    assert_eq!(rows_of(&screen)[0], "X");
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
    assert!(screen.on_alt_screen());
}

/// Entering when already on the alternate screen is a no-op.
///
/// The bug: stashing again, which overwrites the primary buffer's saved cursor with the
/// alternate buffer's, so exiting lands in the wrong place. Two `smcup`s in a row happen
/// whenever a program spawns another full-screen program.
#[test]
fn entering_twice_does_not_lose_the_primary_buffer() {
    let screen = linear(
        12,
        3,
        b"shell\x1b[?1049h\x1b[?1049hinner\x1b[?1049lX",
    );
    assert_eq!(rows_of(&screen)[0], "shellX");
    assert!(!screen.on_alt_screen());
}

/// Leaving when never entered is a no-op and does not blank the screen.
///
/// A stray `rmcup` from a crashed program must not wipe the shell's output.
#[test]
fn leaving_without_entering_changes_nothing() {
    let screen = linear(12, 2, b"shell output\x1b[?1049l");
    assert_eq!(rows_of(&screen)[0], "shell output");
    assert!(!screen.on_alt_screen());
}

/// The alternate buffer has its own content, kept separate from the primary.
#[test]
fn the_two_buffers_hold_separate_content() {
    let on_alt = linear(12, 2, b"primary\x1b[?1049halternate");
    assert_eq!(rows_of(&on_alt)[0], "alternate");

    let back = linear(12, 2, b"primary\x1b[?1049halternate\x1b[?1049l");
    assert_eq!(rows_of(&back)[0], "primary");
}

/// A screen that never entered the alternate buffer never allocates one.
///
/// This is a memory contract, not a nicety: a keyframe is a screen clone, and doubling
/// every keyframe for a session that never runs a TUI would double the index's cost for
/// nothing.
#[test]
fn a_session_that_never_uses_the_alternate_buffer_pays_for_one_grid() {
    let plain = linear(80, 24, b"just a shell\r\n");
    let with_alt = linear(80, 24, b"just a shell\r\n\x1b[?1049h");
    assert!(
        with_alt.heap_bytes() > plain.heap_bytes(),
        "the second grid only appears once the alternate screen is used"
    );
    assert_eq!(
        plain.heap_bytes(),
        80 * 24 * 16 + 24 * 4,
        "one grid of cells plus one damage span per row"
    );
}
