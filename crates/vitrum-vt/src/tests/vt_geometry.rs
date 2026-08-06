//! Resize, scrollback, and the cursor.

use vitrum_grid::cell::Rgba;

use crate::{CursorShape, ScrollViewport};

use super::support::Fixture;

#[test]
fn a_resize_reflows_wrapped_text() {
    // Reflow is the capability the webview path never had: widening a window
    // there left every wrapped line broken where it was.
    let mut fx = Fixture::new(5, 4);
    fx.write(b"abcdefgh");
    assert_eq!(fx.lines()[..2], ["abcde", "fgh"]);

    fx.vt.resize(10, 4, (8, 16)).expect("resize succeeds");
    fx.sync();
    assert_eq!(fx.line(0), "abcdefgh");
}

#[test]
fn the_terminal_reports_its_new_size() {
    let mut fx = Fixture::new(20, 5);
    fx.vt.resize(40, 10, (8, 16)).expect("resize succeeds");

    assert_eq!(fx.vt.cols().expect("readable"), 40);
    assert_eq!(fx.vt.rows().expect("readable"), 10);
}

#[test]
fn rows_that_scroll_off_go_to_scrollback() {
    let mut fx = Fixture::new(10, 2);
    assert_eq!(fx.vt.scrollback_rows().expect("readable"), 0);

    fx.write(b"one\r\ntwo\r\nthree\r\nfour");
    assert!(fx.vt.scrollback_rows().expect("readable") >= 2);
}

#[test]
fn scrollback_can_be_disabled_entirely() {
    // Zero scrollback is what a session that only ever shows a status line
    // should cost, and it must really be zero rather than a small default.
    let mut fx = Fixture::with_scrollback(10, 2, 0);
    fx.write(b"one\r\ntwo\r\nthree\r\nfour");

    assert_eq!(fx.vt.scrollback_rows().expect("readable"), 0);
}

#[test]
fn scrolling_up_shows_older_rows() {
    let mut fx = Fixture::new(10, 2);
    fx.write(b"one\r\ntwo\r\nthree\r\nfour");
    assert_eq!(fx.lines(), ["three", "four"]);

    fx.vt.scroll(ScrollViewport::Top);
    fx.sync();
    assert_eq!(fx.line(0), "one");

    fx.vt.scroll(ScrollViewport::Bottom);
    fx.sync();
    assert_eq!(fx.lines(), ["three", "four"]);
}

#[test]
fn the_cursor_follows_the_text() {
    let mut fx = Fixture::new(10, 3);
    fx.write(b"ab");

    let cursor = fx.vt.cursor().expect("readable");
    assert_eq!((cursor.col, cursor.row), (2, 0));
    assert!(cursor.visible);
}

#[test]
fn the_cursor_can_be_hidden_and_shown() {
    let mut fx = Fixture::new(10, 3);
    fx.write(b"\x1b[?25l");
    assert!(!fx.vt.cursor().expect("readable").visible);

    fx.write(b"\x1b[?25h");
    assert!(fx.vt.cursor().expect("readable").visible);
}

#[test]
fn a_program_can_choose_the_cursor_shape() {
    let mut fx = Fixture::new(10, 1);
    assert_eq!(fx.vt.cursor().expect("readable").shape, CursorShape::Block);

    fx.write(b"\x1b[5 q");
    assert_eq!(fx.vt.cursor().expect("readable").shape, CursorShape::Bar);

    fx.write(b"\x1b[3 q");
    assert_eq!(fx.vt.cursor().expect("readable").shape, CursorShape::Underline);
}

#[test]
fn the_cursor_takes_the_configured_colour() {
    let mut fx = Fixture::new(10, 1);
    fx.vt
        .set_theme(Rgba::WHITE, Rgba::BLACK, Some(Rgba::rgb(1, 2, 3)))
        .expect("theme applies");

    assert_eq!(fx.vt.cursor().expect("readable").color, Rgba::rgb(1, 2, 3));
}

#[test]
fn reading_the_cursor_does_not_consume_the_frame() {
    // `sync` owns the dirty state. If reading the cursor cleared it, a caller
    // that asked for the cursor first would render a blank screen.
    let mut fx = Fixture::new(10, 2);
    fx.vt.feed(b"hello");

    let _ = fx.vt.cursor().expect("readable");
    let stats = fx.sync();

    assert!(stats.cells_changed > 0, "the frame survived the cursor read");
    assert_eq!(fx.line(0), "hello");
}

#[test]
fn a_reset_clears_the_screen_and_the_cursor() {
    let mut fx = Fixture::new(10, 2);
    fx.write(b"\x1b[31mtext");

    fx.vt.reset();
    fx.sync();

    assert_eq!(fx.lines(), ["", ""]);
    let cursor = fx.vt.cursor().expect("readable");
    assert_eq!((cursor.col, cursor.row), (0, 0));
}
