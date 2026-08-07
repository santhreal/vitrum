//! Construction, addressing, fills, scrolling, and resize.

use crate::cell::{Attrs, Cell, CellSlot, Rgba, Style};
use crate::grid::{CellGrid, GridError, MAX_CELLS, MAX_COLS, MAX_ROWS, Region, WriteError};

fn style(fg: u8, bg: u8) -> Style {
    Style::new(Rgba::rgb(fg, fg, fg), Rgba::rgb(bg, bg, bg))
}

/// A new grid must be exactly `cols * rows` blanks in the requested style.
///
/// The renderer allocates its instance buffer from `len()`. If construction
/// ever produced a different cell count than `cols * rows` the draw call would
/// read past the buffer, which is a GPU validation error at best and garbage
/// on screen at worst.
#[test]
fn new_grid_is_cols_times_rows_blanks_in_the_requested_style() {
    let s = style(0x40, 0x80);
    let grid = CellGrid::new(7, 3, s).expect("7x3 is a valid size");
    assert_eq!(grid.cols(), 7);
    assert_eq!(grid.rows(), 3);
    assert_eq!(grid.len(), 21);
    assert_eq!(grid.len(), 21);
    assert_eq!(grid.default_style(), s);
    for row in 0..grid.rows() {
        for cell in grid.row(row).unwrap() {
            assert_eq!(*cell, Cell::blank(s));
        }
    }
}

/// Sizes outside the documented bounds must be refused with the exact
/// dimensions that were rejected.
///
/// A zero-sided grid produces a zero-instance draw and a zero-byte buffer,
/// which wgpu rejects at validation time with a message that says nothing about
/// terminals. Catching it here means a window dragged to zero width reports a
/// grid error instead of killing the device.
#[test]
fn invalid_sizes_are_refused_with_the_offending_dimensions() {
    let s = Style::DEFAULT;
    assert_eq!(
        CellGrid::new(0, 10, s).unwrap_err(),
        GridError::InvalidSize { cols: 0, rows: 10 }
    );
    assert_eq!(
        CellGrid::new(10, 0, s).unwrap_err(),
        GridError::InvalidSize { cols: 10, rows: 0 }
    );
    assert_eq!(
        CellGrid::new(MAX_COLS + 1, 1, s).unwrap_err(),
        GridError::InvalidSize {
            cols: MAX_COLS + 1,
            rows: 1
        }
    );
    assert_eq!(
        CellGrid::new(1, MAX_ROWS + 1, s).unwrap_err(),
        GridError::InvalidSize {
            cols: 1,
            rows: MAX_ROWS + 1
        }
    );
    // Both sides legal, product over the cell cap.
    assert_eq!(MAX_COLS as usize * MAX_ROWS as usize, 4 * MAX_CELLS);
    assert_eq!(
        CellGrid::new(MAX_COLS, MAX_ROWS, s).unwrap_err(),
        GridError::InvalidSize {
            cols: MAX_COLS,
            rows: MAX_ROWS
        }
    );
    // Exactly at the cap is accepted.
    assert!(CellGrid::new(MAX_COLS, 512, s).is_ok());
}

/// Flat indexing must be row-major and must reject out-of-range coordinates.
///
/// The renderer converts a damage span into a buffer offset with the same
/// arithmetic. If the grid were column-major, uploads would land on the wrong
/// cells and the screen would look transposed in a way no single-cell test
/// would catch.
#[test]
fn indexing_is_row_major_and_bounded() {
    let grid = CellGrid::new(5, 4, Style::DEFAULT).unwrap();
    assert_eq!(grid.index(0, 0), Some(0));
    assert_eq!(grid.index(4, 0), Some(4));
    assert_eq!(grid.index(0, 1), Some(5));
    assert_eq!(grid.index(4, 3), Some(19));
    assert_eq!(grid.index(5, 0), None);
    assert_eq!(grid.index(0, 4), None);
    assert_eq!(grid.index(u16::MAX, u16::MAX), None);
    assert!(grid.cell(5, 0).is_none());
    assert!(grid.row(4).is_none());
    assert_eq!(grid.row(3).map(<[Cell]>::len), Some(5));
}

/// Out-of-bounds writes must report the coordinate, not silently clip.
///
/// A VT parser with an off-by-one cursor would otherwise scribble on column 0
/// of the next row and the corruption would look like a wrapping bug.
#[test]
fn out_of_bounds_writes_are_reported_not_clipped() {
    let mut grid = CellGrid::new(4, 2, Style::DEFAULT).unwrap();
    assert_eq!(
        grid.set_cell(4, 0, Cell::default()).unwrap_err(),
        GridError::OutOfBounds { col: 4, row: 0 }
    );
    assert_eq!(
        grid.write_char(0, 2, 'x', Style::DEFAULT).unwrap_err(),
        WriteError::OutOfBounds { col: 0, row: 2 }
    );
    assert_eq!(
        grid.write_str(9, 0, "hi", Style::DEFAULT).unwrap_err(),
        WriteError::OutOfBounds { col: 9, row: 0 }
    );
    assert_eq!(
        grid.clear_row(2).unwrap_err(),
        GridError::OutOfBounds { col: 0, row: 2 }
    );
    for row in 0..grid.rows() {
        for cell in grid.row(row).unwrap() {
            assert_eq!(*cell, Cell::default(), "nothing may have been written");
        }
    }
}

/// Controls and combining marks must be refused by name.
///
/// Storing a control would put an unprintable codepoint through the
/// rasteriser; storing a combining mark on its own cell would render a
/// free-floating accent one column to the right of the letter it belongs to.
/// Both are refused so the front end has to make an explicit decision.
#[test]
fn controls_and_combining_marks_are_refused_with_the_character() {
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    assert_eq!(
        grid.write_char(0, 0, '\u{7}', Style::DEFAULT).unwrap_err(),
        WriteError::Control('\u{7}')
    );
    assert_eq!(
        grid.write_char(0, 0, '\u{1b}', Style::DEFAULT).unwrap_err(),
        WriteError::Control('\u{1b}')
    );
    assert_eq!(
        grid.write_char(0, 0, '\u{301}', Style::DEFAULT).unwrap_err(),
        WriteError::ZeroWidth('\u{301}')
    );
    assert_eq!(grid.cell(0, 0).unwrap().ch, ' ');
}

/// `write_str` must skip unstorable characters and keep advancing.
///
/// A log line containing a stray BEL must not truncate the rest of the line.
/// The exact returned column matters because the caller uses it as the next
/// cursor position.
#[test]
fn write_str_skips_unstorable_characters_and_returns_the_next_column() {
    let mut grid = CellGrid::new(10, 1, Style::DEFAULT).unwrap();
    let end = grid
        .write_str(0, 0, "a\u{7}b\u{301}c", Style::DEFAULT)
        .unwrap();
    assert_eq!(end, 3, "three storable characters were written");
    assert_eq!(grid.row_text(0).unwrap(), "abc       ");
}

/// `write_str` must stop at the row edge rather than wrap or error.
///
/// Wrapping is a VT decision that needs the wrap flag; a grid that wrapped on
/// its own would double-print the tail of every long line the parser also
/// wrapped.
#[test]
fn write_str_stops_at_the_row_edge_without_wrapping() {
    let mut grid = CellGrid::new(4, 2, Style::DEFAULT).unwrap();
    let end = grid.write_str(2, 0, "abcdef", Style::DEFAULT).unwrap();
    assert_eq!(end, 4);
    assert_eq!(grid.row_text(0).unwrap(), "  ab");
    assert_eq!(grid.row_text(1).unwrap(), "    ", "row 1 must be untouched");
}

/// A fill must be clipped to the grid and must report the exact change count.
///
/// The count is what a caller uses to decide whether a repaint is worth doing.
/// Reporting cells that did not change would defeat the whole damage model.
#[test]
fn fill_clips_to_the_grid_and_counts_only_real_changes() {
    let mut grid = CellGrid::new(4, 3, Style::DEFAULT).unwrap();
    let marker = Cell::new('#', style(0xff, 0x10));

    let changed = grid.fill(
        Region {
            col: 2,
            row: 1,
            cols: 99,
            rows: 99,
        },
        marker,
    );
    assert_eq!(changed, 4, "columns 2..4 of rows 1..3");
    assert_eq!(grid.row_text(0).unwrap(), "    ");
    assert_eq!(grid.row_text(1).unwrap(), "  ##");
    assert_eq!(grid.row_text(2).unwrap(), "  ##");

    let again = grid.fill(
        Region {
            col: 2,
            row: 1,
            cols: 2,
            rows: 2,
        },
        marker,
    );
    assert_eq!(again, 0, "refilling identical cells changes nothing");
}

/// `clear` must repaint in the current default style, not the original one.
///
/// A terminal that receives OSC 11 (set background) then a clear must show the
/// new background. Using a style captured at construction would leave the old
/// colour behind until every cell was individually rewritten.
#[test]
fn clear_uses_the_current_default_style() {
    let mut grid = CellGrid::new(3, 2, style(0xff, 0x00)).unwrap();
    grid.write_str(0, 0, "abc", style(0xff, 0x00)).unwrap();

    let new_default = style(0x20, 0x50);
    grid.set_default_style(new_default);
    let changed = grid.clear();
    assert_eq!(changed, 6, "3 written plus 3 blanks whose background moved");
    for row in 0..grid.rows() {
        for cell in grid.row(row).unwrap() {
            assert_eq!(*cell, Cell::blank(new_default));
        }
    }
}

/// Scrolling up must move rows by exactly `count` and blank the vacated ones.
///
/// This is the single hottest structural operation in a terminal. An off-by-one
/// here duplicates or eats a line on every newline at the bottom of the screen.
#[test]
fn scroll_up_moves_rows_by_count_and_blanks_the_bottom() {
    let mut grid = CellGrid::new(3, 5, Style::DEFAULT).unwrap();
    for row in 0..5u16 {
        grid.write_str(0, row, &format!("r{row}"), Style::DEFAULT)
            .unwrap();
    }
    grid.scroll_up(0, 4, 2, Cell::default()).unwrap();
    assert_eq!(grid.row_text(0).unwrap(), "r2 ");
    assert_eq!(grid.row_text(1).unwrap(), "r3 ");
    assert_eq!(grid.row_text(2).unwrap(), "r4 ");
    assert_eq!(grid.row_text(3).unwrap(), "   ");
    assert_eq!(grid.row_text(4).unwrap(), "   ");
}
#[test]
fn circular_ring_buffer_scrolling_rotates_row_slots() {
    let mut grid = CellGrid::new(4, 5, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "ROW0", Style::DEFAULT).unwrap();
    grid.write_str(0, 1, "ROW1", Style::DEFAULT).unwrap();
    grid.write_str(0, 2, "ROW2", Style::DEFAULT).unwrap();
    grid.write_str(0, 3, "ROW3", Style::DEFAULT).unwrap();
    grid.write_str(0, 4, "ROW4", Style::DEFAULT).unwrap();

    grid.scroll_up(0, 4, 1, Cell::default()).unwrap();
    assert_eq!(grid.row_text(0).unwrap(), "ROW1");
    assert_eq!(grid.row_text(1).unwrap(), "ROW2");
    assert_eq!(grid.row_text(2).unwrap(), "ROW3");
    assert_eq!(grid.row_text(3).unwrap(), "ROW4");
    assert_eq!(grid.row_text(4).unwrap(), "    ");
}

/// Scrolling down must move rows the other way and blank the top.
///
/// Reverse index (RI) at the top of a screen depends on this. Sharing the
/// implementation with `scroll_up` means a sign error would silently make both
/// directions scroll the same way.
#[test]
fn scroll_down_moves_rows_by_count_and_blanks_the_top() {
    let mut grid = CellGrid::new(3, 5, Style::DEFAULT).unwrap();
    for row in 0..5u16 {
        grid.write_str(0, row, &format!("r{row}"), Style::DEFAULT)
            .unwrap();
    }
    grid.scroll_down(0, 4, 2, Cell::default()).unwrap();
    assert_eq!(grid.row_text(0).unwrap(), "   ");
    assert_eq!(grid.row_text(1).unwrap(), "   ");
    assert_eq!(grid.row_text(2).unwrap(), "r0 ");
    assert_eq!(grid.row_text(3).unwrap(), "r1 ");
    assert_eq!(grid.row_text(4).unwrap(), "r2 ");
}

/// A scroll must stay inside its region and leave the rest of the screen alone.
///
/// DECSTBM sets a scrolling region; a `less` pager relies on the status line
/// below it never moving. A scroll that ignored the region would drag the
/// status line up with the text.
#[test]
fn scroll_respects_the_region_boundaries() {
    let mut grid = CellGrid::new(3, 5, Style::DEFAULT).unwrap();
    for row in 0..5u16 {
        grid.write_str(0, row, &format!("r{row}"), Style::DEFAULT)
            .unwrap();
    }
    grid.scroll_up(1, 3, 1, Cell::default()).unwrap();
    assert_eq!(grid.row_text(0).unwrap(), "r0 ", "above the region");
    assert_eq!(grid.row_text(1).unwrap(), "r2 ");
    assert_eq!(grid.row_text(2).unwrap(), "r3 ");
    assert_eq!(grid.row_text(3).unwrap(), "   ");
    assert_eq!(grid.row_text(4).unwrap(), "r4 ", "below the region");
}

/// A scroll count at or past the region height must clear the whole region.
///
/// `clear` sequences are often implemented as a scroll by the screen height.
/// An implementation that computed `height - count` as an unsigned subtraction
/// would panic or wrap to a gigantic copy length.
#[test]
fn scroll_count_at_or_past_the_region_height_clears_it() {
    let mut grid = CellGrid::new(3, 4, Style::DEFAULT).unwrap();
    for row in 0..4u16 {
        grid.write_str(0, row, "xxx", Style::DEFAULT).unwrap();
    }
    grid.scroll_up(0, 3, 4, Cell::default()).unwrap();
    for row in 0..4u16 {
        assert_eq!(grid.row_text(row).unwrap(), "   ");
    }

    for row in 0..4u16 {
        grid.write_str(0, row, "yyy", Style::DEFAULT).unwrap();
    }
    grid.scroll_down(0, 3, 9999, Cell::default()).unwrap();
    for row in 0..4u16 {
        assert_eq!(grid.row_text(row).unwrap(), "   ");
    }
}

/// An inverted or out-of-range scroll region must be refused.
///
/// DECSTBM with a bottom above the top is a real thing hostile output sends. A
/// grid that trusted it would compute a negative length and panic in
/// `copy_within`, taking the whole client down over one escape sequence.
#[test]
fn inverted_or_overflowing_scroll_regions_are_refused() {
    let mut grid = CellGrid::new(3, 4, Style::DEFAULT).unwrap();
    assert_eq!(
        grid.scroll_up(3, 1, 1, Cell::default()).unwrap_err(),
        GridError::InvalidRegion { top: 3, bottom: 1 }
    );
    assert_eq!(
        grid.scroll_down(0, 4, 1, Cell::default()).unwrap_err(),
        GridError::InvalidRegion { top: 0, bottom: 4 }
    );
    assert_eq!(
        grid.scroll_up(0, u16::MAX, 1, Cell::default()).unwrap_err(),
        GridError::InvalidRegion {
            top: 0,
            bottom: u16::MAX
        }
    );
}

/// Growing must keep every existing cell at the same coordinate and blank the
/// new space.
///
/// This is the window-maximise path. Content that shifted would make every
/// resize look like the terminal lost a line.
#[test]
fn growing_preserves_content_at_the_same_coordinates() {
    let mut grid = CellGrid::new(3, 2, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "abc", Style::DEFAULT).unwrap();
    grid.write_str(0, 1, "def", Style::DEFAULT).unwrap();

    grid.resize(5, 4).unwrap();
    assert_eq!(grid.cols(), 5);
    assert_eq!(grid.rows(), 4);
    assert_eq!(grid.row_text(0).unwrap(), "abc  ");
    assert_eq!(grid.row_text(1).unwrap(), "def  ");
    assert_eq!(grid.row_text(2).unwrap(), "     ");
    assert_eq!(grid.row_text(3).unwrap(), "     ");
}

/// Shrinking must truncate from the right and bottom, keeping the top-left.
///
/// The alternative (anchoring bottom-left, as a scrollback-aware reflow would)
/// needs wrap flags this type does not have. Truncation is the documented
/// contract and the renderer's instance layout assumes it.
#[test]
fn shrinking_truncates_from_the_right_and_bottom() {
    let mut grid = CellGrid::new(5, 4, Style::DEFAULT).unwrap();
    for row in 0..4u16 {
        grid.write_str(0, row, "ABCDE", Style::DEFAULT).unwrap();
    }
    grid.resize(3, 2).unwrap();
    assert_eq!(grid.cols(), 3);
    assert_eq!(grid.rows(), 2);
    assert_eq!(grid.row_text(0).unwrap(), "ABC");
    assert_eq!(grid.row_text(1).unwrap(), "ABC");
    assert_eq!(grid.len(), 6);
}

/// A vertical-only resize must keep the columns byte for byte.
///
/// This path takes the in-place `Vec::resize` shortcut rather than rebuilding
/// row by row. If the shortcut ever stopped matching the general path, dragging
/// a window's bottom edge would corrupt the text while dragging its corner
/// would not, which is a maddening bug to chase.
#[test]
fn vertical_only_resize_matches_the_general_path() {
    let mut a = CellGrid::new(4, 3, Style::DEFAULT).unwrap();
    for row in 0..3u16 {
        a.write_str(0, row, "wxyz", Style::DEFAULT).unwrap();
    }
    let mut b = a.clone();

    a.resize(4, 6).unwrap();
    b.resize(5, 6).unwrap();
    b.resize(4, 6).unwrap();

    for row in 0..a.rows() {
        assert_eq!(a.row(row), b.row(row), "row {row} must match");
    }
    assert_eq!(a.row_text(2).unwrap(), "wxyz");
    assert_eq!(a.row_text(3).unwrap(), "    ");
}

/// Resizing to the current size must change nothing and record no damage.
///
/// Compositors deliver spurious resize events at the same size on every
/// workspace switch. Treating those as a real resize would force a full grid
/// re-upload each time and turn an idle terminal into a busy one.
#[test]
fn resize_to_the_same_size_is_a_no_op_with_no_damage() {
    let mut grid = CellGrid::new(6, 3, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "hello", Style::DEFAULT).unwrap();
    grid.clear_damage();

    grid.resize(6, 3).unwrap();
    assert_eq!(grid.dirty_cells(), 0);
    assert!(!grid.is_dirty());
    assert_eq!(grid.row_text(0).unwrap(), "hello ");
}

/// Resize must refuse invalid sizes and leave the grid untouched.
///
/// A failed resize that had already dropped the old cells would leave the
/// renderer drawing from a buffer sized for a grid that no longer exists.
#[test]
fn failed_resize_leaves_the_grid_intact() {
    let mut grid = CellGrid::new(4, 2, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "keep", Style::DEFAULT).unwrap();
    assert_eq!(
        grid.resize(0, 5).unwrap_err(),
        GridError::InvalidSize { cols: 0, rows: 5 }
    );
    assert_eq!(grid.cols(), 4);
    assert_eq!(grid.rows(), 2);
    assert_eq!(grid.row_text(0).unwrap(), "keep");
}

/// `row_text` must skip wide tails so extracted text has no phantom columns.
///
/// Clipboard copy and the test suite both read through this. Including the
/// tail's NUL would put a stray `\0` after every CJK character in a copied
/// line.
#[test]
fn row_text_skips_wide_tails() {
    let mut grid = CellGrid::new(6, 1, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "a漢b", Style::DEFAULT).unwrap();
    assert_eq!(grid.row_text(0).unwrap(), "a漢b  ");
    assert_eq!(
        grid.row_text(0).unwrap().chars().count(),
        5,
        "one narrow, one wide, one narrow, two trailing blanks"
    );
}

/// Attributes and colours written into a cell must survive a read verbatim.
///
/// Everything downstream (face selection, the reverse swap, the underline flag)
/// reads these back. A style that mutated in storage would make SGR state
/// non-deterministic.
#[test]
fn styles_round_trip_through_grid_storage() {
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    let s = Style {
        fg: Rgba::new(0x11, 0x22, 0x33, 0x44),
        bg: Rgba::new(0x55, 0x66, 0x77, 0x88),
        attrs: Attrs::BOLD | Attrs::ITALIC | Attrs::UNDERLINE | Attrs::REVERSE,
    };
    grid.write_char(1, 0, 'Q', s).unwrap();

    let cell = grid.cell(1, 0).unwrap();
    assert_eq!(cell.ch, 'Q');
    assert_eq!(cell.fg, s.fg);
    assert_eq!(cell.bg, s.bg);
    assert_eq!(cell.attrs, s.attrs);
    assert_eq!(cell.slot, CellSlot::Single);
    assert_eq!(cell.style(), s);
}
