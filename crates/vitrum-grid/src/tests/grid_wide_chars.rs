//! Double-width characters: how they occupy two columns and what happens when
//! something splits the pair.

use crate::cell::{Cell, CellSlot, Rgba, Style};
use crate::grid::{CellGrid, WriteError};

fn head(grid: &CellGrid, col: u16, row: u16) -> Cell {
    grid.cell(col, row).expect("coordinate must be in bounds")
}

/// A wide character must claim two columns: a head carrying the character and a
/// tail carrying nothing.
///
/// If both columns carried the character, the renderer would draw the glyph
/// twice, once at each column, and CJK text would appear doubled. If only one
/// column were claimed, the next character would be written over the right half
/// of the glyph.
#[test]
fn wide_character_occupies_a_head_and_a_tail_column() {
    let mut grid = CellGrid::new(6, 1, Style::DEFAULT).unwrap();
    let advance = grid.write_char(1, 0, '漢', Style::DEFAULT).unwrap();
    assert_eq!(advance, 2, "the caller must advance the cursor by two");

    let h = head(&grid, 1, 0);
    assert_eq!(h.ch, '漢');
    assert_eq!(h.slot, CellSlot::WideHead);

    let t = head(&grid, 2, 0);
    assert_eq!(t.ch, '\0', "the tail must carry no character");
    assert_eq!(t.slot, CellSlot::WideTail);

    assert_eq!(head(&grid, 0, 0).slot, CellSlot::Single);
    assert_eq!(head(&grid, 3, 0).slot, CellSlot::Single);
}

/// A narrow character must claim exactly one column and advance by one.
///
/// The counterpart to the test above: a classification bug that made every
/// character wide would halve the usable width of the terminal.
#[test]
fn narrow_character_claims_one_column() {
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    assert_eq!(grid.write_char(0, 0, 'a', Style::DEFAULT).unwrap(), 1);
    assert_eq!(head(&grid, 0, 0).slot, CellSlot::Single);
    assert_eq!(head(&grid, 1, 0).ch, ' ');
}

/// A wide character in the last column must be refused, not clipped.
///
/// Real terminals wrap it to the next line, but wrapping needs the cursor and
/// the wrap flag, which live in the VT front end. Silently writing only the
/// head here would leave a half-drawn glyph hanging off the right edge of every
/// line that ends in CJK text.
#[test]
fn wide_character_at_the_last_column_is_refused() {
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    assert_eq!(
        grid.write_char(3, 0, 'あ', Style::DEFAULT).unwrap_err(),
        WriteError::WideAtRowEnd { col: 3, ch: 'あ' }
    );
    assert_eq!(head(&grid, 3, 0).ch, ' ', "nothing may have been written");
    assert_eq!(head(&grid, 3, 0).slot, CellSlot::Single);

    // One column earlier it fits exactly.
    assert_eq!(grid.write_char(2, 0, 'あ', Style::DEFAULT).unwrap(), 2);
    assert_eq!(head(&grid, 2, 0).slot, CellSlot::WideHead);
    assert_eq!(head(&grid, 3, 0).slot, CellSlot::WideTail);
}

/// Overwriting the head of a pair must blank the orphaned tail.
///
/// A cursor-addressed write into the left half of a CJK character is common
/// (progress bars redrawing over previous output). Leaving the tail behind
/// would keep a `WideTail` next to a normal cell, and the renderer would draw
/// nothing at all in that column: a permanent black gap that only a full
/// repaint clears.
#[test]
fn overwriting_a_head_blanks_the_orphaned_tail() {
    let mut grid = CellGrid::new(5, 1, Style::DEFAULT).unwrap();
    grid.write_char(1, 0, '漢', Style::DEFAULT).unwrap();
    grid.write_char(1, 0, 'x', Style::DEFAULT).unwrap();

    assert_eq!(head(&grid, 1, 0).ch, 'x');
    assert_eq!(head(&grid, 1, 0).slot, CellSlot::Single);
    assert_eq!(head(&grid, 2, 0).ch, ' ', "the tail must become a blank");
    assert_eq!(head(&grid, 2, 0).slot, CellSlot::Single);
    assert_eq!(grid.row_text(0).unwrap(), " x   ");
}

/// Overwriting the tail of a pair must blank the orphaned head.
///
/// The mirror case. A leftover `WideHead` would still draw a two-column glyph,
/// so the character the caller just wrote would be painted over by the old CJK
/// glyph's right half.
#[test]
fn overwriting_a_tail_blanks_the_orphaned_head() {
    let mut grid = CellGrid::new(5, 1, Style::DEFAULT).unwrap();
    grid.write_char(1, 0, '漢', Style::DEFAULT).unwrap();
    grid.write_char(2, 0, 'y', Style::DEFAULT).unwrap();

    assert_eq!(head(&grid, 1, 0).ch, ' ', "the head must become a blank");
    assert_eq!(head(&grid, 1, 0).slot, CellSlot::Single);
    assert_eq!(head(&grid, 2, 0).ch, 'y');
    assert_eq!(head(&grid, 2, 0).slot, CellSlot::Single);
    assert_eq!(grid.row_text(0).unwrap(), "  y  ");
}

/// A wide write landing between two existing pairs must repair both of them.
///
/// This is the adversarial case: the new pair's left edge splits one existing
/// pair and its right edge splits another. Repairing only one side leaves a
/// dangling half that renders as a gap or a doubled glyph.
#[test]
fn a_wide_write_between_two_pairs_repairs_both_neighbours() {
    let mut grid = CellGrid::new(6, 1, Style::DEFAULT).unwrap();
    grid.write_char(0, 0, 'あ', Style::DEFAULT).unwrap(); // columns 0,1
    grid.write_char(2, 0, 'い', Style::DEFAULT).unwrap(); // columns 2,3
    grid.write_char(4, 0, 'う', Style::DEFAULT).unwrap(); // columns 4,5

    // Land a new pair on columns 1,2: splits the first pair's tail and the
    // second pair's head.
    grid.write_char(1, 0, 'ん', Style::DEFAULT).unwrap();

    assert_eq!(head(&grid, 0, 0).ch, ' ', "first pair's head is orphaned");
    assert_eq!(head(&grid, 0, 0).slot, CellSlot::Single);
    assert_eq!(head(&grid, 1, 0).ch, 'ん');
    assert_eq!(head(&grid, 1, 0).slot, CellSlot::WideHead);
    assert_eq!(head(&grid, 2, 0).slot, CellSlot::WideTail);
    assert_eq!(head(&grid, 3, 0).ch, ' ', "second pair's tail is orphaned");
    assert_eq!(head(&grid, 3, 0).slot, CellSlot::Single);
    assert_eq!(head(&grid, 4, 0).ch, 'う', "third pair is untouched");
    assert_eq!(head(&grid, 5, 0).slot, CellSlot::WideTail);
    assert_eq!(grid.row_text(0).unwrap(), " ん う");
}

/// Repairing a broken pair must keep the neighbour's colours.
///
/// The orphaned half becomes a blank, but it must keep the background the
/// character had. Resetting it to the grid default would punch a
/// default-coloured hole into a coloured run every time a CJK character was
/// partially overwritten.
#[test]
fn repairing_a_pair_preserves_the_neighbours_colors() {
    let coloured = Style::new(Rgba::rgb(0xff, 0x00, 0x00), Rgba::rgb(0x00, 0x40, 0x00));
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    grid.write_char(0, 0, '漢', coloured).unwrap();
    grid.write_char(1, 0, 'z', Style::DEFAULT).unwrap();

    let orphan = head(&grid, 0, 0);
    assert_eq!(orphan.ch, ' ');
    assert_eq!(orphan.fg, coloured.fg, "orphan keeps the pair's foreground");
    assert_eq!(orphan.bg, coloured.bg, "orphan keeps the pair's background");
}

/// A wide pair straddling the new right edge must lose its head on shrink.
///
/// After `resize(cols, _)` the tail at the old column `cols` is gone. A head
/// left in the last column would still ask the renderer for a two-column quad,
/// which would be clipped by the viewport and draw a half glyph on the screen
/// edge for as long as the window stayed that size.
#[test]
fn shrinking_removes_a_head_whose_tail_was_truncated() {
    let mut grid = CellGrid::new(6, 2, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "ab漢cd", Style::DEFAULT).unwrap();
    grid.write_str(0, 1, "漢漢漢", Style::DEFAULT).unwrap();
    assert_eq!(head(&grid, 2, 0).slot, CellSlot::WideHead);

    grid.resize(3, 2).unwrap();

    // Row 0: the pair started at column 2, so its tail is gone.
    assert_eq!(head(&grid, 2, 0).ch, ' ');
    assert_eq!(head(&grid, 2, 0).slot, CellSlot::Single);
    assert_eq!(grid.row_text(0).unwrap(), "ab ");

    // Row 1: the pair at columns 2..4 lost its tail; the pair at 0..2 survives.
    assert_eq!(head(&grid, 0, 1).slot, CellSlot::WideHead);
    assert_eq!(head(&grid, 1, 1).slot, CellSlot::WideTail);
    assert_eq!(head(&grid, 2, 1).ch, ' ');
    assert_eq!(head(&grid, 2, 1).slot, CellSlot::Single);
}

/// Shrinking must not disturb a pair that fits entirely inside the new width.
///
/// The truncation repair only inspects the final column. If it were sloppier
/// and swept the whole row, every surviving CJK character would be erased on
/// every window narrow.
#[test]
fn shrinking_keeps_pairs_that_still_fit() {
    let mut grid = CellGrid::new(8, 1, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "漢字test", Style::DEFAULT).unwrap();
    grid.resize(4, 1).unwrap();

    assert_eq!(head(&grid, 0, 0).ch, '漢');
    assert_eq!(head(&grid, 0, 0).slot, CellSlot::WideHead);
    assert_eq!(head(&grid, 1, 0).slot, CellSlot::WideTail);
    assert_eq!(head(&grid, 2, 0).ch, '字');
    assert_eq!(head(&grid, 2, 0).slot, CellSlot::WideHead);
    assert_eq!(head(&grid, 3, 0).slot, CellSlot::WideTail);
    assert_eq!(grid.row_text(0).unwrap(), "漢字");
}

/// A row of wide characters must consume exactly half as many columns as it has
/// characters, and `write_str` must report the right end column.
///
/// The cursor position after a run of CJK is what the VT front end uses for the
/// next write. Advancing by one per character would put every subsequent
/// character on top of the previous one.
#[test]
fn a_run_of_wide_characters_advances_two_columns_each() {
    let mut grid = CellGrid::new(10, 1, Style::DEFAULT).unwrap();
    let end = grid.write_str(0, 0, "漢字仮名", Style::DEFAULT).unwrap();
    assert_eq!(end, 8, "four wide characters occupy eight columns");
    for pair in 0..4u16 {
        assert_eq!(head(&grid, pair * 2, 0).slot, CellSlot::WideHead);
        assert_eq!(head(&grid, pair * 2 + 1, 0).slot, CellSlot::WideTail);
    }
    assert_eq!(head(&grid, 8, 0).slot, CellSlot::Single);
    assert_eq!(grid.row_text(0).unwrap(), "漢字仮名  ");
}

/// An emoji must be treated as wide, like CJK.
///
/// Agents print emoji in status lines constantly. Classifying a grinning face
/// as narrow shifts every subsequent column on the line by one and the
/// misalignment persists until the line is redrawn.
#[test]
fn emoji_occupies_two_columns() {
    let mut grid = CellGrid::new(6, 1, Style::DEFAULT).unwrap();
    assert_eq!(
        grid.write_char(0, 0, '\u{1f600}', Style::DEFAULT).unwrap(),
        2
    );
    assert_eq!(head(&grid, 0, 0).slot, CellSlot::WideHead);
    assert_eq!(head(&grid, 1, 0).slot, CellSlot::WideTail);
}

/// Writing the identical wide character twice must be a no-op.
///
/// The repair step runs before the write and could easily blank the pair's own
/// tail on the way to rewriting it, turning an idempotent repaint into two
/// damaged cells and a flicker. Both cells must come out unchanged.
#[test]
fn rewriting_the_same_wide_character_changes_nothing() {
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    grid.write_char(0, 0, '漢', Style::DEFAULT).unwrap();
    let before: Vec<Cell> = grid.row(0).unwrap().to_vec();
    grid.clear_damage();

    grid.write_char(0, 0, '漢', Style::DEFAULT).unwrap();
    assert_eq!(grid.row(0).unwrap(), before.as_slice());
    assert_eq!(
        grid.dirty_cells(),
        0,
        "an identical rewrite must record no damage"
    );
}
