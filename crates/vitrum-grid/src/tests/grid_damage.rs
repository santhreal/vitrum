//! Damage tracking: what marks a cell dirty, what must not, and what the
//! renderer sees.
//!
//! The zero-idle-CPU claim rests entirely on this module. A grid that reports
//! damage for an unchanged frame turns twenty idle agents into twenty
//! continuous GPU uploads.

use crate::cell::{Attrs, Cell, Rgba, Style};
use crate::grid::{CellGrid, DamageSpan, Region};

fn spans(grid: &CellGrid) -> Vec<DamageSpan> {
    grid.damage().collect()
}

/// A fresh grid must be fully damaged, because nothing has been uploaded yet.
///
/// If a new grid started clean, the first frame would skip the draw and the
/// terminal would open showing whatever was in the render target: usually
/// nothing, occasionally the previous window's pixels.
#[test]
fn a_new_grid_starts_fully_damaged() {
    let grid = CellGrid::new(8, 3, Style::DEFAULT).unwrap();
    assert!(grid.is_dirty());
    assert_eq!(grid.dirty_cells(), 24);
    assert_eq!(
        spans(&grid),
        vec![
            DamageSpan { row: 0, start: 0, end: 8 },
            DamageSpan { row: 1, start: 0, end: 8 },
            DamageSpan { row: 2, start: 0, end: 8 },
        ]
    );
}

/// After `clear_damage` the grid must report exactly zero dirty cells and no
/// spans.
///
/// This is the state an idle terminal sits in. Any nonzero residue here means
/// the renderer uploads and submits on every frame forever.
#[test]
fn clearing_damage_reports_exactly_zero_dirty_cells() {
    let mut grid = CellGrid::new(8, 3, Style::DEFAULT).unwrap();
    grid.clear_damage();
    assert!(!grid.is_dirty());
    assert_eq!(grid.dirty_cells(), 0);
    assert_eq!(spans(&grid), Vec::new());
}

/// Writing a value identical to the one already stored must record no damage.
///
/// A VT front end that repaints a status line every second usually writes the
/// same bytes. Treating that as damage would make a terminal showing a static
/// prompt as expensive as one streaming a build log. This comparison is the
/// single most load-bearing line in the crate.
#[test]
fn rewriting_an_identical_value_records_no_damage() {
    let mut grid = CellGrid::new(8, 2, Style::DEFAULT).unwrap();
    grid.write_str(0, 0, "status", Style::DEFAULT).unwrap();
    grid.clear_damage();

    grid.write_str(0, 0, "status", Style::DEFAULT).unwrap();
    assert_eq!(grid.dirty_cells(), 0);
    assert!(!grid.is_dirty());

    assert!(!grid.set_cell(0, 0, grid.cell(0, 0).unwrap()).unwrap());
    assert_eq!(grid.dirty_cells(), 0);

    assert_eq!(
        grid.fill(Region::all(&grid), Cell::blank(Style::DEFAULT)),
        6,
        "only the six written cells differ from a blank"
    );
    grid.clear_damage();
    assert_eq!(grid.fill(Region::all(&grid), Cell::blank(Style::DEFAULT)), 0);
    assert_eq!(grid.dirty_cells(), 0);
}

/// A colour-only change with the same character must still count as damage.
///
/// Syntax highlighting recolours text in place. A comparison that only looked
/// at `ch` would leave the old colours on screen and the change would appear
/// only after some unrelated edit forced a repaint.
#[test]
fn a_color_only_change_is_damage() {
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    grid.write_char(0, 0, 'a', Style::DEFAULT).unwrap();
    grid.clear_damage();

    let recoloured = Style::new(Rgba::rgb(0xff, 0x00, 0x00), Rgba::BLACK);
    assert!(grid.write_char(0, 0, 'a', recoloured).is_ok());
    assert_eq!(grid.dirty_cells(), 1);
    assert_eq!(
        spans(&grid),
        vec![DamageSpan { row: 0, start: 0, end: 1 }]
    );
}

/// An attribute-only change with identical text and colours must count as
/// damage.
///
/// Turning on underline changes nothing the character comparison would see.
/// Missing it means SGR 4 appears to do nothing until the text scrolls.
#[test]
fn an_attribute_only_change_is_damage() {
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    grid.write_char(2, 0, 'a', Style::DEFAULT).unwrap();
    grid.clear_damage();

    grid.write_char(2, 0, 'a', Style::DEFAULT.with_attrs(Attrs::UNDERLINE))
        .unwrap();
    assert_eq!(grid.dirty_cells(), 1);
    assert_eq!(
        spans(&grid),
        vec![DamageSpan { row: 0, start: 2, end: 3 }]
    );
}

/// A single changed cell must produce a one-cell span on that row alone.
///
/// The renderer turns each span into a `write_buffer`. A span that covered the
/// whole row for a one-character edit would upload 200 cells per keystroke.
#[test]
fn a_single_cell_change_damages_exactly_one_cell() {
    let mut grid = CellGrid::new(10, 4, Style::DEFAULT).unwrap();
    grid.clear_damage();
    grid.write_char(6, 2, 'k', Style::DEFAULT).unwrap();

    assert_eq!(grid.dirty_cells(), 1);
    assert_eq!(
        spans(&grid),
        vec![DamageSpan { row: 2, start: 6, end: 7 }]
    );
    assert_eq!(spans(&grid)[0].len(), 1);
    assert!(!spans(&grid)[0].is_empty());
    assert_eq!(spans(&grid)[0].columns(), 6..7);
}

/// Two separated changes on one row must merge into the span that covers both.
///
/// This is the documented trade: a span is cheaper to track and cheaper to
/// upload than two scattered writes. The test pins the exact reported number so
/// nobody "optimises" it into a per-cell bitmap without deciding to.
#[test]
fn separated_changes_on_one_row_merge_into_one_span() {
    let mut grid = CellGrid::new(20, 1, Style::DEFAULT).unwrap();
    grid.clear_damage();
    grid.write_char(3, 0, 'a', Style::DEFAULT).unwrap();
    grid.write_char(17, 0, 'b', Style::DEFAULT).unwrap();

    assert_eq!(
        spans(&grid),
        vec![DamageSpan { row: 0, start: 3, end: 18 }]
    );
    assert_eq!(grid.dirty_cells(), 15);
}

/// Damage on different rows must stay on those rows.
///
/// Rows are independent spans precisely so a change at the top of the screen
/// does not force the bottom to re-upload.
#[test]
fn damage_stays_on_the_rows_that_changed() {
    let mut grid = CellGrid::new(6, 5, Style::DEFAULT).unwrap();
    grid.clear_damage();
    grid.write_char(1, 1, 'x', Style::DEFAULT).unwrap();
    grid.write_char(4, 3, 'y', Style::DEFAULT).unwrap();

    assert_eq!(
        spans(&grid),
        vec![
            DamageSpan { row: 1, start: 1, end: 2 },
            DamageSpan { row: 3, start: 4, end: 5 },
        ]
    );
    assert_eq!(grid.dirty_cells(), 2);
}

/// Writing a wide character must damage both of its columns.
///
/// The head and the tail are separate instances in the buffer. Uploading only
/// the head would leave a stale tail whose span still claimed a column, and the
/// tail would paint its old background over the right half of the new glyph.
#[test]
fn writing_a_wide_character_damages_both_columns() {
    let mut grid = CellGrid::new(8, 1, Style::DEFAULT).unwrap();
    grid.clear_damage();
    grid.write_char(2, 0, '漢', Style::DEFAULT).unwrap();

    assert_eq!(
        spans(&grid),
        vec![DamageSpan { row: 0, start: 2, end: 4 }]
    );
    assert_eq!(grid.dirty_cells(), 2);
}

/// Breaking a wide pair must damage the orphaned neighbour too.
///
/// The repair writes a blank one column outside the span the caller asked for.
/// If that write escaped the damage record, the orphaned half would keep its
/// old instance and the renderer would still draw a two-column glyph there.
#[test]
fn breaking_a_wide_pair_damages_the_orphaned_neighbour() {
    let mut grid = CellGrid::new(8, 1, Style::DEFAULT).unwrap();
    grid.write_char(2, 0, '漢', Style::DEFAULT).unwrap();
    grid.clear_damage();

    grid.write_char(3, 0, 'q', Style::DEFAULT).unwrap();
    assert_eq!(
        spans(&grid),
        vec![DamageSpan { row: 0, start: 2, end: 4 }],
        "the orphaned head at column 2 must be in the span"
    );
    assert_eq!(grid.dirty_cells(), 2);
}

/// A scroll must damage every row of its region and nothing outside it.
///
/// Rows below a scrolling region hold a pager's status line. Damaging them
/// would re-upload the status line on every line of output, which is the
/// per-frame cost this design exists to avoid.
#[test]
fn scrolling_damages_only_the_region() {
    let mut grid = CellGrid::new(4, 6, Style::DEFAULT).unwrap();
    grid.clear_damage();
    grid.scroll_up(1, 3, 1, Cell::default()).unwrap();

    assert_eq!(
        spans(&grid),
        vec![
            DamageSpan { row: 1, start: 0, end: 4 },
            DamageSpan { row: 2, start: 0, end: 4 },
            DamageSpan { row: 3, start: 0, end: 4 },
        ]
    );
    assert_eq!(grid.dirty_cells(), 12);
}

/// A scroll by zero must record no damage at all.
///
/// A VT parser that emits a zero-count scroll (real output does) would
/// otherwise force a full-region upload for a no-op, which is exactly the
/// "re-serialize state on a timer" failure this renderer was designed against.
#[test]
fn scrolling_by_zero_records_no_damage() {
    let mut grid = CellGrid::new(4, 6, Style::DEFAULT).unwrap();
    grid.write_str(0, 2, "keep", Style::DEFAULT).unwrap();
    grid.clear_damage();

    grid.scroll_up(0, 5, 0, Cell::default()).unwrap();
    grid.scroll_down(0, 5, 0, Cell::default()).unwrap();
    assert_eq!(grid.dirty_cells(), 0);
    assert!(!grid.is_dirty());
    assert_eq!(grid.row_text(2).unwrap(), "keep");
}

/// A refused scroll must not leave damage behind.
///
/// An invalid DECSTBM followed by a scroll must be inert. Damage recorded
/// before the validation check would cause a pointless full-region upload for
/// an operation that never happened.
#[test]
fn a_refused_scroll_records_no_damage() {
    let mut grid = CellGrid::new(4, 6, Style::DEFAULT).unwrap();
    grid.clear_damage();
    assert!(grid.scroll_up(4, 2, 1, Cell::default()).is_err());
    assert!(grid.scroll_down(0, 99, 1, Cell::default()).is_err());
    assert_eq!(grid.dirty_cells(), 0);
}

/// `mark_all_damaged` must cover every cell at the current size.
///
/// The renderer calls this after a glyph atlas reset, when every cached atlas
/// coordinate has become meaningless. Covering less than the whole grid would
/// leave cells pointing at glyph rectangles that now hold a different
/// character.
#[test]
fn mark_all_damaged_covers_every_cell() {
    let mut grid = CellGrid::new(12, 4, Style::DEFAULT).unwrap();
    grid.clear_damage();
    grid.mark_all_damaged();

    assert_eq!(grid.dirty_cells(), 48);
    assert_eq!(spans(&grid).len(), 4);
    for span in spans(&grid) {
        assert_eq!(span.columns(), 0..12);
    }
}

/// A resize must leave the whole grid damaged at the new size.
///
/// After a resize the instance buffer is reallocated and holds nothing. If any
/// row came out clean, that row would draw from uninitialised instance memory.
#[test]
fn resizing_leaves_the_whole_grid_damaged_at_the_new_size() {
    let mut grid = CellGrid::new(4, 2, Style::DEFAULT).unwrap();
    grid.clear_damage();

    grid.resize(7, 5).unwrap();
    assert_eq!(grid.dirty_cells(), 35);
    assert_eq!(spans(&grid).len(), 5);
    for span in spans(&grid) {
        assert_eq!(span.columns(), 0..7);
    }

    grid.clear_damage();
    grid.resize(2, 2).unwrap();
    assert_eq!(grid.dirty_cells(), 4);
    assert_eq!(spans(&grid).len(), 2);
}

/// A failed write must not record damage.
///
/// Refusing a control character and then reporting the cell dirty would upload
/// an unchanged cell on the next frame, once per control byte in the stream.
/// A terminal streaming a build log sees thousands of those a second.
#[test]
fn refused_writes_record_no_damage() {
    let mut grid = CellGrid::new(4, 1, Style::DEFAULT).unwrap();
    grid.clear_damage();

    assert!(grid.write_char(0, 0, '\u{1b}', Style::DEFAULT).is_err());
    assert!(grid.write_char(0, 0, '\u{301}', Style::DEFAULT).is_err());
    assert!(grid.write_char(3, 0, '漢', Style::DEFAULT).is_err());
    assert!(grid.set_cell(9, 9, Cell::default()).is_err());
    assert_eq!(grid.dirty_cells(), 0);
    assert!(!grid.is_dirty());
}

/// `set_cell` must report whether the value actually changed.
///
/// Callers use the boolean to decide whether to schedule a frame. Always
/// returning `true` would schedule a frame per write and reintroduce the
/// unconditional-repaint cost this design removes.
#[test]
fn set_cell_reports_whether_the_value_changed() {
    let mut grid = CellGrid::new(3, 1, Style::DEFAULT).unwrap();
    let cell = Cell::new('m', Style::DEFAULT);
    assert!(grid.set_cell(1, 0, cell).unwrap(), "first write changes it");
    assert!(!grid.set_cell(1, 0, cell).unwrap(), "second write does not");
    assert!(
        grid.set_cell(1, 0, Cell::new('n', Style::DEFAULT)).unwrap(),
        "a different character does"
    );
}

/// A damage span must never be reported for a clean row.
///
/// The renderer's upload loop assumes every yielded span is non-empty; a
/// zero-length span would produce a zero-byte `write_buffer`, which wgpu
/// rejects.
#[test]
fn no_empty_spans_are_ever_yielded() {
    let mut grid = CellGrid::new(30, 20, Style::DEFAULT).unwrap();
    grid.clear_damage();
    grid.write_char(0, 0, 'a', Style::DEFAULT).unwrap();
    grid.write_char(29, 19, 'b', Style::DEFAULT).unwrap();

    let all = spans(&grid);
    assert_eq!(all.len(), 2, "only the two touched rows are reported");
    assert_eq!(all[0], DamageSpan { row: 0, start: 0, end: 1 });
    assert_eq!(all[1], DamageSpan { row: 19, start: 29, end: 30 });
    for span in all {
        assert!(!span.is_empty());
        assert_eq!(span.len(), 1, "each row had exactly one changed cell");
        assert!(span.start < span.end);
    }
}
#[test]
fn bitmask_damage_bookkeeping_tracks_dirty_columns() {
    let mut grid = CellGrid::new(100, 1, Style::DEFAULT).unwrap();
    grid.clear_damage();
    grid.write_char(10, 0, 'X', Style::DEFAULT).unwrap();
    grid.write_char(70, 0, 'Y', Style::DEFAULT).unwrap();

    let all = spans(&grid);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].start, 10);
    assert_eq!(all[0].end, 71);
}
