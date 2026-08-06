//! Regression test suite for grid bounds, cell memory packing, 2048x2048 size limits,
//! ring buffer row scrolling rotation, unicode wide pair tails, and damage tracking.

use vitrum_grid::{
    Attrs, Cell, CellGrid, CellSlot, CharWidth, DamageSpan, GridError, Region, Rgba, Style,
    WriteError, char_width, MAX_CELLS, MAX_COLS, MAX_ROWS,
};

// ============================================================================
// 1. Cell Memory Packing & Layout Invariants
// ============================================================================

/// WHY: Defends the 16-byte memory footprint and alignment of Cell to guarantee flat
/// grid storage without per-cell heap allocations across large terminal grids.
#[test]
fn test_cell_memory_packing_and_alignment() {
    assert_eq!(
        std::mem::size_of::<Cell>(),
        16,
        "Cell struct must be exactly 16 bytes"
    );
    assert_eq!(
        std::mem::size_of::<Rgba>(),
        4,
        "Rgba struct must be 4 bytes"
    );
    assert_eq!(
        std::mem::size_of::<Attrs>(),
        1,
        "Attrs struct must be 1 byte"
    );
    assert_eq!(
        std::mem::size_of::<CellSlot>(),
        1,
        "CellSlot enum must be 1 byte"
    );

    // Test byte packing & bit mask truncation for Attrs
    let invalid_bits = Attrs::from_bits_truncate(0b1111_1111);
    assert_eq!(
        invalid_bits.bits(),
        Attrs::ALL.bits(),
        "from_bits_truncate must strip undefined bits above 0b1111"
    );

    // Test Rgba serialization to/from bytes for vertex attribute conversion
    let color = Rgba::rgba(12, 34, 56, 78);
    assert_eq!(color.to_bytes(), [12, 34, 56, 78]);
    assert_eq!(Rgba::from_bytes([12, 34, 56, 78]), color);
}

/// WHY: Guarantees correct reverse-video attribute color resolution and glyphless cell
/// classification to prevent redundant font atlas rasterization in GPU render paths.
#[test]
fn test_cell_resolved_colors_and_glyphless_invariants() {
    let fg = Rgba::rgb(255, 0, 0);
    let bg = Rgba::rgb(0, 0, 255);
    let style = Style {
        fg,
        bg,
        attrs: Attrs::REVERSE,
    };
    let cell = Cell::new('A', style);

    let (res_fg, res_bg) = cell.resolved_colors();
    assert_eq!(res_fg, bg, "REVERSE attribute must swap fg to bg");
    assert_eq!(res_bg, fg, "REVERSE attribute must swap bg to fg");

    // Glyphless cell checks
    let space_cell = Cell::blank(Style::DEFAULT);
    assert!(
        space_cell.is_glyphless(),
        "Space character cell must be classified as glyphless"
    );

    let null_cell = Cell {
        ch: '\0',
        fg: Rgba::WHITE,
        bg: Rgba::BLACK,
        attrs: Attrs::NONE,
        slot: CellSlot::WideTail,
    };
    assert!(
        null_cell.is_glyphless(),
        "WideTail null cell must be classified as glyphless"
    );

    let char_cell = Cell::new('Z', Style::DEFAULT);
    assert!(
        !char_cell.is_glyphless(),
        "Printable character cell must not be glyphless"
    );
}

// ============================================================================
// 2. 2048x2048 Size Limits & Boundary Rules
// ============================================================================

/// WHY: Enforces strict grid boundary limits (MAX_COLS=2048, MAX_ROWS=2048, MAX_CELLS=1<<20)
/// to prevent heap exhaustion and integer overflows in u16 coordinate index math.
#[test]
fn test_grid_size_limits_boundaries() {
    // Verify exported constants
    assert_eq!(MAX_CELLS, 1 << 20);
    assert_eq!(MAX_COLS, 2048);
    assert_eq!(MAX_ROWS, 2048);

    // Valid boundary sizes
    assert!(CellGrid::new(1, 1, Style::DEFAULT).is_ok());
    assert!(CellGrid::new(2048, 512, Style::DEFAULT).is_ok()); // exactly 1,048,576 cells (1<<20)
    assert!(CellGrid::new(1024, 1024, Style::DEFAULT).is_ok()); // 1,048,576 cells

    // Invalid zero dimensions
    assert_eq!(
        CellGrid::new(0, 10, Style::DEFAULT).unwrap_err(),
        GridError::InvalidSize { cols: 0, rows: 10 }
    );
    assert_eq!(
        CellGrid::new(10, 0, Style::DEFAULT).unwrap_err(),
        GridError::InvalidSize { cols: 10, rows: 0 }
    );

    // Exceeding MAX_COLS / MAX_ROWS
    assert_eq!(
        CellGrid::new(2049, 1, Style::DEFAULT).unwrap_err(),
        GridError::InvalidSize { cols: 2049, rows: 1 }
    );
    assert_eq!(
        CellGrid::new(1, 2049, Style::DEFAULT).unwrap_err(),
        GridError::InvalidSize { cols: 1, rows: 2049 }
    );

    // Exceeding MAX_CELLS limit (2048x2048 = 4,194,304 cells > 1,048,576)
    assert_eq!(
        CellGrid::new(MAX_COLS, MAX_ROWS, Style::DEFAULT).unwrap_err(),
        GridError::InvalidSize {
            cols: MAX_COLS,
            rows: MAX_ROWS
        }
    );

    // 1025x1025 = 1,050,625 > 1<<20
    assert_eq!(
        CellGrid::new(1025, 1025, Style::DEFAULT).unwrap_err(),
        GridError::InvalidSize {
            cols: 1025,
            rows: 1025
        }
    );
}

/// WHY: Verifies grid resizing truncates and expands safely while repairing wide character heads
/// orphaned by right-edge boundary truncation.
#[test]
fn test_grid_resize_boundary_and_straddle_cleanup() {
    let mut grid = CellGrid::new(10, 5, Style::DEFAULT).unwrap();

    // Resizing to same size is a clean no-op
    assert!(grid.resize(10, 5).is_ok());

    // Write wide character '字' at column 8 (occupies col 8 head, col 9 tail)
    grid.write_char(8, 0, '字', Style::DEFAULT).unwrap();
    assert_eq!(grid.cell(8, 0).unwrap().slot, CellSlot::WideHead);
    assert_eq!(grid.cell(9, 0).unwrap().slot, CellSlot::WideTail);

    // Resize grid down to cols = 9, truncating column 9 (the tail of '字')
    grid.resize(9, 5).unwrap();
    assert_eq!(grid.cols(), 9);

    // Column 8 was WideHead, but since tail at col 9 was truncated, col 8 must be repaired to blank
    let repaired_cell = grid.cell(8, 0).unwrap();
    assert_eq!(
        repaired_cell.slot,
        CellSlot::Single,
        "Orphaned WideHead at truncation boundary must be repaired to Single blank"
    );
    assert_eq!(repaired_cell.ch, ' ');

    // Resizing to invalid dimensions must return GridError::InvalidSize
    assert!(grid.resize(3000, 3000).is_err());
}

// ============================================================================
// 3. Ring Buffer Row Scrolling Rotation & Boundary Shifts
// ============================================================================

/// WHY: Verifies row scrolling via scroll_up and scroll_down shifts cell data in-place
/// without memory allocation while filling vacated rows.
#[test]
fn test_grid_scroll_up_down_ring_rotation() {
    let mut grid = CellGrid::new(10, 5, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "ROW_0", Style::DEFAULT).unwrap();
    grid.write_str(0, 1, "ROW_1", Style::DEFAULT).unwrap();
    grid.write_str(0, 2, "ROW_2", Style::DEFAULT).unwrap();
    grid.write_str(0, 3, "ROW_3", Style::DEFAULT).unwrap();
    grid.write_str(0, 4, "ROW_4", Style::DEFAULT).unwrap();

    // Scroll up whole grid by 2 rows
    let fill = Cell::new('~', Style::DEFAULT);
    grid.scroll_up(0, 4, 2, fill).unwrap();

    assert_eq!(grid.row_text(0).unwrap().trim_end(), "ROW_2");
    assert_eq!(grid.row_text(1).unwrap().trim_end(), "ROW_3");
    assert_eq!(grid.row_text(2).unwrap().trim_end(), "ROW_4");
    assert_eq!(grid.row_text(3).unwrap(), "~~~~~~~~~~");
    assert_eq!(grid.row_text(4).unwrap(), "~~~~~~~~~~");

    // Scroll down whole grid by 1 row
    let fill_down = Cell::new('#', Style::DEFAULT);
    grid.scroll_down(0, 4, 1, fill_down).unwrap();

    assert_eq!(grid.row_text(0).unwrap(), "##########");
    assert_eq!(grid.row_text(1).unwrap().trim_end(), "ROW_2");
    assert_eq!(grid.row_text(2).unwrap().trim_end(), "ROW_3");
    assert_eq!(grid.row_text(3).unwrap().trim_end(), "ROW_4");
    assert_eq!(grid.row_text(4).unwrap(), "~~~~~~~~~~");
}

/// WHY: Tests sub-region scrolling isolation and boundary behavior when scroll count
/// equals or exceeds region height or region parameters are invalid.
#[test]
fn test_grid_scroll_region_bounds_and_exceeding_count() {
    let mut grid = CellGrid::new(10, 5, Style::DEFAULT).unwrap();
    for r in 0..5 {
        grid.write_str(0, r, &format!("LINE_{r}"), Style::DEFAULT).unwrap();
    }

    // Scroll sub-region (rows 1..=3) up by 2 rows
    let fill = Cell::blank(Style::DEFAULT);
    grid.scroll_up(1, 3, 2, fill).unwrap();

    // Row 0 and Row 4 must remain untouched outside region
    assert_eq!(grid.row_text(0).unwrap().trim_end(), "LINE_0");
    assert_eq!(grid.row_text(1).unwrap().trim_end(), "LINE_3");
    assert_eq!(grid.row_text(4).unwrap().trim_end(), "LINE_4");

    // Scroll count >= region height clears whole sub-region
    grid.scroll_up(1, 3, 10, fill).unwrap();
    assert_eq!(grid.row_text(1).unwrap(), "          ");
    assert_eq!(grid.row_text(2).unwrap(), "          ");
    assert_eq!(grid.row_text(3).unwrap(), "          ");

    // Scroll count == 0 is a valid no-op
    assert!(grid.scroll_up(0, 4, 0, fill).is_ok());

    // Inverted or out-of-bounds regions return GridError::InvalidRegion
    assert_eq!(
        grid.scroll_up(3, 1, 1, fill).unwrap_err(),
        GridError::InvalidRegion { top: 3, bottom: 1 }
    );
    assert_eq!(
        grid.scroll_up(0, 5, 1, fill).unwrap_err(),
        GridError::InvalidRegion { top: 0, bottom: 5 }
    );
}

// ============================================================================
// 4. Unicode Wide Pair Tails & Straddling Repair
// ============================================================================

/// WHY: Ensures writing wide characters correctly creates paired WideHead and WideTail cells
/// and overwriting either half automatically detaches and repairs straddling wide pairs.
#[test]
fn test_unicode_wide_pair_head_tail_repair() {
    let mut grid = CellGrid::new(10, 2, Style::DEFAULT).unwrap();

    // Write wide character '🚀' at col 2
    let advance = grid.write_char(2, 0, '🚀', Style::DEFAULT).unwrap();
    assert_eq!(advance, 2);

    let head = grid.cell(2, 0).unwrap();
    let tail = grid.cell(3, 0).unwrap();

    assert_eq!(head.slot, CellSlot::WideHead);
    assert_eq!(head.ch, '🚀');
    assert_eq!(tail.slot, CellSlot::WideTail);
    assert_eq!(tail.ch, '\0');

    // Overwrite the WideTail cell at col 3 with narrow 'X'
    grid.write_char(3, 0, 'X', Style::DEFAULT).unwrap();
    let repaired_head = grid.cell(2, 0).unwrap();
    assert_eq!(
        repaired_head.slot,
        CellSlot::Single,
        "Overwriting WideTail must detach WideHead and reset it to blank"
    );
    assert_eq!(repaired_head.ch, ' ');

    // Write wide char '🌐' at col 5 (cols 5, 6)
    grid.write_char(5, 0, '🌐', Style::DEFAULT).unwrap();

    // Overwrite the WideHead at col 5 with narrow 'Y'
    grid.write_char(5, 0, 'Y', Style::DEFAULT).unwrap();
    let repaired_tail = grid.cell(6, 0).unwrap();
    assert_eq!(
        repaired_tail.slot,
        CellSlot::Single,
        "Overwriting WideHead must detach WideTail and reset it to blank"
    );
    assert_eq!(repaired_tail.ch, ' ');
}

/// WHY: Protects against malformed text writes by refusing C0/C1 control characters,
/// zero-width combining marks, and wide characters that lack space at row ends.
#[test]
fn test_unicode_wide_char_row_end_and_invalid_chars() {
    let mut grid = CellGrid::new(10, 2, Style::DEFAULT).unwrap();

    // Control character write refusal
    assert_eq!(
        grid.write_char(0, 0, '\u{0007}', Style::DEFAULT).unwrap_err(),
        WriteError::Control('\u{0007}')
    );

    // Zero-width combining mark refusal
    assert_eq!(
        grid.write_char(0, 0, '\u{0300}', Style::DEFAULT).unwrap_err(),
        WriteError::ZeroWidth('\u{0300}')
    );

    // Wide character at row end refusal (col 9 is last column, needs 2 columns)
    assert_eq!(
        grid.write_char(9, 0, '界', Style::DEFAULT).unwrap_err(),
        WriteError::WideAtRowEnd { col: 9, ch: '界' }
    );

    // Out of bounds write
    assert_eq!(
        grid.write_char(10, 0, 'A', Style::DEFAULT).unwrap_err(),
        WriteError::OutOfBounds { col: 10, row: 0 }
    );

    // Check classification function directly
    assert_eq!(char_width('\u{0007}'), CharWidth::Control);
    assert_eq!(char_width('\u{0300}'), CharWidth::ZeroWidth);
    assert_eq!(char_width('A'), CharWidth::Narrow);
    assert_eq!(char_width('界'), CharWidth::Wide);
}

/// WHY: Verifies row_text extraction skips WideTail cells so clipboard/text representations
/// match visible characters without emitting null bytes or duplicated glyphs.
#[test]
fn test_grid_row_text_wide_tail_skipping() {
    let mut grid = CellGrid::new(10, 1, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "Hi🚀Grid", Style::DEFAULT).unwrap();

    // "Hi" (2 cols) + '🚀' (2 cols: head+tail) + "Grid" (4 cols) = 8 cols written
    let text = grid.row_text(0).unwrap();
    assert_eq!(
        text.trim_end(),
        "Hi🚀Grid",
        "row_text must skip WideTail null bytes and preserve verbatim text"
    );
}

// ============================================================================
// 5. Bitmask / Span Damage Tracking & Optimization
// ============================================================================

/// WHY: Defends damage tracking accuracy, confirming identical cell writes generate zero
/// damage while non-identical writes expand RowDamage spans correctly.
#[test]
fn test_grid_damage_span_coalescing_and_noop_suppression() {
    let mut grid = CellGrid::new(20, 5, Style::DEFAULT).unwrap();

    // Fresh grid starts fully damaged
    assert!(grid.is_dirty());
    assert_eq!(grid.dirty_cells(), 100); // 20 * 5

    // Clear damage
    grid.clear_damage();
    assert!(!grid.is_dirty());
    assert_eq!(grid.dirty_cells(), 0);

    // Writing identical cell (blank) to (5, 1) returns changed=false and records NO damage
    let default_blank = Cell::blank(Style::DEFAULT);
    assert!(!grid.set_cell(5, 1, default_blank).unwrap());
    assert!(!grid.is_dirty());

    // Mutating cell at (5, 1) with new fg color marks row 1 damaged (start: 5, end: 6)
    let red_cell = Cell::new('X', Style::new(Rgba::rgb(255, 0, 0), Rgba::BLACK));
    assert!(grid.set_cell(5, 1, red_cell).unwrap());
    assert!(grid.is_dirty());

    let damage: Vec<DamageSpan> = grid.damage().collect();
    assert_eq!(damage.len(), 1);
    assert_eq!(damage[0], DamageSpan { row: 1, start: 5, end: 6 });
    assert_eq!(grid.dirty_cells(), 1);

    // Mutating cell at (2, 1) expands row 1 damage span to cover start: 2, end: 6
    let blue_cell = Cell::new('Y', Style::new(Rgba::rgb(0, 255, 0), Rgba::BLACK));
    assert!(grid.set_cell(2, 1, blue_cell).unwrap());

    let damage2: Vec<DamageSpan> = grid.damage().collect();
    assert_eq!(damage2.len(), 1);
    assert_eq!(damage2[0], DamageSpan { row: 1, start: 2, end: 6 });
    assert_eq!(damage2[0].len(), 4);
    assert_eq!(grid.dirty_cells(), 4);
}

/// WHY: Ensures scroll operations and explicit mark_all_damaged trigger full-width row
/// damage spans so renderer GPU instance buffers stay fully in sync.
#[test]
fn test_grid_scroll_and_mark_all_damaged_invariants() {
    let mut grid = CellGrid::new(10, 4, Style::DEFAULT).unwrap();
    grid.clear_damage();
    assert!(!grid.is_dirty());

    // mark_all_damaged sets all 4 rows to damaged spanning 0..10
    grid.mark_all_damaged();
    assert!(grid.is_dirty());
    assert_eq!(grid.dirty_cells(), 40);

    grid.clear_damage();

    // Scroll up rows 1..=2 by 1 row
    grid.scroll_up(1, 2, 1, Cell::blank(Style::DEFAULT)).unwrap();

    let damaged_rows: Vec<u16> = grid.damage().map(|d| d.row).collect();
    assert_eq!(
        damaged_rows,
        vec![1, 2],
        "Only scrolled rows within sub-region top..=bottom should be marked damaged"
    );

    for span in grid.damage() {
        assert_eq!(span.start, 0);
        assert_eq!(span.end, 10);
    }
}

/// WHY: Tests rectangular region fills with clipping to grid boundaries and verifies
/// returned changed-cell counts and damage span calculations.
#[test]
fn test_grid_fill_and_region_clipping_damage() {
    let mut grid = CellGrid::new(15, 6, Style::DEFAULT).unwrap();
    grid.clear_damage();

    let fill_cell = Cell::new('*', Style::DEFAULT);

    // Fill 5x3 region at col 2, row 1
    let region = Region {
        col: 2,
        row: 1,
        cols: 5,
        rows: 3,
    };
    let changed = grid.fill(region, fill_cell);
    assert_eq!(changed, 15); // 5 cols * 3 rows

    let damage: Vec<DamageSpan> = grid.damage().collect();
    assert_eq!(damage.len(), 3);
    for (i, span) in damage.iter().enumerate() {
        assert_eq!(span.row, (i + 1) as u16);
        assert_eq!(span.start, 2);
        assert_eq!(span.end, 7);
    }

    // Filling exact same region again returns changed = 0 and adds no new damage
    grid.clear_damage();
    let changed_second = grid.fill(region, fill_cell);
    assert_eq!(changed_second, 0);
    assert!(!grid.is_dirty());

    // Fill region extending out-of-bounds (col 12, row 4 with size 10x10 on 15x6 grid)
    let oob_region = Region {
        col: 12,
        row: 4,
        cols: 10,
        rows: 10,
    };
    let oob_changed = grid.fill(oob_region, fill_cell);
    // Clipped region is cols 12..15 (3 cols), rows 4..6 (2 rows) => 6 cells
    assert_eq!(
        oob_changed, 6,
        "Fill out-of-bounds region must clip safely to grid dimensions"
    );
}
