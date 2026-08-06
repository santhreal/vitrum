//! The bounded cell grid and its damage tracking.
//!
//! [`CellGrid`] is the whole input to the renderer. A VT parser drives it, the
//! renderer reads it, and nothing else touches it. It owns exactly one
//! allocation of `cols * rows` [`Cell`]s plus one damage span per row.
//!
//! # Damage
//!
//! Every mutation compares against the value already stored and records nothing
//! when they are equal. That is the whole reason an idle terminal costs zero:
//! a repaint that writes the same bytes back produces zero damage, and the
//! renderer then records no GPU commands at all.
//!
//! Damage is tracked as one inclusive-exclusive column span per row rather than
//! a per-cell bitmap. A span is what the renderer wants anyway (it uploads a
//! contiguous run of instances per span), and a row's span costs 4 bytes
//! instead of `cols` bits.

use core::ops::Range;

use crate::cell::{Cell, CellSlot, CharWidth, Style, char_width};

/// Largest grid width the type accepts. A terminal wider than this is not a
/// terminal, and the bound keeps `col + 1` from ever overflowing `u16`.
pub const MAX_COLS: u16 = 2048;
/// Largest grid height the type accepts.
pub const MAX_ROWS: u16 = 2048;
/// Largest total cell count. At 16 bytes per cell this caps one grid at 16 MiB.
pub const MAX_CELLS: usize = 1 << 20;

/// Why a grid operation was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GridError {
    /// The requested dimensions are zero or exceed [`MAX_COLS`], [`MAX_ROWS`],
    /// or [`MAX_CELLS`].
    InvalidSize {
        /// Requested column count.
        cols: u16,
        /// Requested row count.
        rows: u16,
    },
    /// A coordinate fell outside the grid.
    OutOfBounds {
        /// Requested column.
        col: u16,
        /// Requested row.
        row: u16,
    },
    /// A scroll or fill region was empty, inverted, or ran past the last row.
    InvalidRegion {
        /// First row of the region.
        top: u16,
        /// Last row of the region, inclusive.
        bottom: u16,
    },
}

impl core::fmt::Display for GridError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidSize { cols, rows } => write!(
                f,
                "grid size {cols}x{rows} is invalid: each side must be 1..={MAX_COLS}/{MAX_ROWS} \
                 and the product must not exceed {MAX_CELLS} cells"
            ),
            Self::OutOfBounds { col, row } => {
                write!(f, "cell ({col}, {row}) is outside the grid")
            }
            Self::InvalidRegion { top, bottom } => {
                write!(f, "row region {top}..={bottom} is empty or out of range")
            }
        }
    }
}

impl core::error::Error for GridError {}

/// Why a character could not be written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteError {
    /// The target coordinate is outside the grid.
    OutOfBounds {
        /// Requested column.
        col: u16,
        /// Requested row.
        row: u16,
    },
    /// The character is a C0/C1 control. Controls carry no printable form; the
    /// VT front end must interpret them instead of storing them.
    Control(char),
    /// The character is zero width (a combining mark). One `char` per cell
    /// cannot represent composition, so the front end must pre-compose.
    ZeroWidth(char),
    /// A double-width character was written to the last column, where its
    /// second half would not fit. Wrapping is the front end's decision, so the
    /// grid reports rather than guesses.
    WideAtRowEnd {
        /// The column that had no room.
        col: u16,
        /// The character that did not fit.
        ch: char,
    },
}

impl core::fmt::Display for WriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OutOfBounds { col, row } => {
                write!(f, "cell ({col}, {row}) is outside the grid")
            }
            Self::Control(ch) => write!(
                f,
                "U+{:04X} is a control character and has no cell representation",
                *ch as u32
            ),
            Self::ZeroWidth(ch) => write!(
                f,
                "U+{:04X} is zero width; compose it into the preceding character before writing",
                *ch as u32
            ),
            Self::WideAtRowEnd { col, ch } => write!(
                f,
                "U+{:04X} needs two columns but column {col} is the last one; wrap or pad first",
                *ch as u32
            ),
        }
    }
}

impl core::error::Error for WriteError {}

/// A contiguous run of changed columns on one row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DamageSpan {
    /// Row index.
    pub row: u16,
    /// First changed column.
    pub start: u16,
    /// One past the last changed column.
    pub end: u16,
}

impl DamageSpan {
    /// Number of cells the span covers.
    #[must_use]
    pub const fn len(self) -> usize {
        (self.end - self.start) as usize
    }

    /// True when the span covers no cells. Never returned by
    /// [`CellGrid::damage`], which skips empty rows.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start >= self.end
    }

    /// The columns as a range.
    #[must_use]
    pub const fn columns(self) -> Range<u16> {
        self.start..self.end
    }
}

/// Per-row damage bookkeeping. `start >= end` means "clean".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct RowDamage {
    start: u16,
    end: u16,
}

impl RowDamage {
    const CLEAN: Self = Self {
        start: u16::MAX,
        end: 0,
    };

    const fn is_clean(self) -> bool {
        self.start >= self.end
    }

    const fn len(self) -> usize {
        if self.is_clean() {
            0
        } else {
            (self.end - self.start) as usize
        }
    }

    fn extend(&mut self, start: u16, end: u16) {
        if start >= end {
            return;
        }
        if self.start > start {
            self.start = start;
        }
        if self.end < end {
            self.end = end;
        }
    }
}

/// A rectangular block of cells, used by fills.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Region {
    /// Left column.
    pub col: u16,
    /// Top row.
    pub row: u16,
    /// Width in columns.
    pub cols: u16,
    /// Height in rows.
    pub rows: u16,
}

impl Region {
    /// A region covering the whole of `grid`.
    #[must_use]
    pub const fn all(grid: &CellGrid) -> Self {
        Self {
            col: 0,
            row: 0,
            cols: grid.cols,
            rows: grid.rows,
        }
    }
}

/// A bounded terminal cell grid with per-row damage tracking.
#[derive(Clone, Debug)]
pub struct CellGrid {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    damage: Vec<RowDamage>,
    default_style: Style,
}

impl CellGrid {
    /// Create a `cols` x `rows` grid of blanks painted in `default_style`.
    ///
    /// The grid starts fully damaged, because the renderer has never uploaded
    /// any of it.
    ///
    /// # Errors
    ///
    /// [`GridError::InvalidSize`] when either side is zero or the size exceeds
    /// [`MAX_COLS`], [`MAX_ROWS`], or [`MAX_CELLS`].
    pub fn new(cols: u16, rows: u16, default_style: Style) -> Result<Self, GridError> {
        check_size(cols, rows)?;
        let len = cols as usize * rows as usize;
        Ok(Self {
            cols,
            rows,
            cells: vec![Cell::blank(default_style); len],
            damage: vec![
                RowDamage {
                    start: 0,
                    end: cols
                };
                rows as usize
            ],
            default_style,
        })
    }

    /// Column count.
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// Row count.
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// Total cell count.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.cols as usize * self.rows as usize
    }

    /// Always false: a grid with a zero side cannot be constructed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// The style new blanks are painted in.
    #[must_use]
    pub const fn default_style(&self) -> Style {
        self.default_style
    }

    /// Change the style new blanks are painted in. Existing cells keep their
    /// own colours, so this records no damage.
    pub const fn set_default_style(&mut self, style: Style) {
        self.default_style = style;
    }

    /// Every cell, row-major.
    #[must_use]
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Flat index of `(col, row)`, or `None` when out of bounds.
    #[must_use]
    pub const fn index(&self, col: u16, row: u16) -> Option<usize> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        Some(row as usize * self.cols as usize + col as usize)
    }

    /// The cell at `(col, row)`, or `None` when out of bounds.
    #[must_use]
    pub fn cell(&self, col: u16, row: u16) -> Option<Cell> {
        self.index(col, row).map(|i| self.cells[i])
    }

    /// One row of cells, or `None` when `row` is out of bounds.
    #[must_use]
    pub fn row(&self, row: u16) -> Option<&[Cell]> {
        if row >= self.rows {
            return None;
        }
        let w = self.cols as usize;
        let start = row as usize * w;
        Some(&self.cells[start..start + w])
    }

    /// The printable text of one row, skipping wide-pair tails.
    ///
    /// This is for tests and for clipboard extraction; it is not on any render
    /// path and it allocates.
    #[must_use]
    pub fn row_text(&self, row: u16) -> Option<String> {
        let cells = self.row(row)?;
        let mut out = String::with_capacity(cells.len());
        for cell in cells {
            if cell.slot == CellSlot::WideTail {
                continue;
            }
            out.push(cell.ch);
        }
        Some(out)
    }

    /// Overwrite one cell verbatim.
    ///
    /// Returns `true` when the stored value actually changed. Writing the value
    /// already present is a no-op and records no damage, which is what makes an
    /// idle repaint free.
    ///
    /// This does not repair wide pairs. Use [`CellGrid::write_char`] for VT
    /// text; use this for whole-cell restores where the caller already knows
    /// the slot layout is consistent.
    ///
    /// # Errors
    ///
    /// [`GridError::OutOfBounds`] when `(col, row)` is outside the grid.
    pub fn set_cell(&mut self, col: u16, row: u16, cell: Cell) -> Result<bool, GridError> {
        let Some(idx) = self.index(col, row) else {
            return Err(GridError::OutOfBounds { col, row });
        };
        Ok(self.store(idx, col, row, cell))
    }

    /// Write `ch` at `(col, row)` in `style`, repairing any wide pair it breaks.
    ///
    /// Returns the number of columns consumed: 1 for a narrow character, 2 for
    /// a double-width one. The caller advances its cursor by that amount.
    ///
    /// # Errors
    ///
    /// See [`WriteError`]. Controls and zero-width characters are refused
    /// rather than silently dropped, and a wide character with no room is
    /// reported rather than wrapped, because wrapping is a VT-level decision.
    pub fn write_char(
        &mut self,
        col: u16,
        row: u16,
        ch: char,
        style: Style,
    ) -> Result<u16, WriteError> {
        if col >= self.cols || row >= self.rows {
            return Err(WriteError::OutOfBounds { col, row });
        }
        let width = match char_width(ch) {
            CharWidth::Control => return Err(WriteError::Control(ch)),
            CharWidth::ZeroWidth => return Err(WriteError::ZeroWidth(ch)),
            CharWidth::Narrow => 1u16,
            CharWidth::Wide => 2u16,
        };
        if col + width > self.cols {
            return Err(WriteError::WideAtRowEnd { col, ch });
        }

        self.detach_straddling_pairs(col, row, width);

        let base = row as usize * self.cols as usize;
        if width == 1 {
            self.store(
                base + col as usize,
                col,
                row,
                Cell {
                    ch,
                    fg: style.fg,
                    bg: style.bg,
                    attrs: style.attrs,
                    slot: CellSlot::Single,
                },
            );
        } else {
            self.store(
                base + col as usize,
                col,
                row,
                Cell {
                    ch,
                    fg: style.fg,
                    bg: style.bg,
                    attrs: style.attrs,
                    slot: CellSlot::WideHead,
                },
            );
            self.store(
                base + col as usize + 1,
                col + 1,
                row,
                Cell {
                    ch: '\0',
                    fg: style.fg,
                    bg: style.bg,
                    attrs: style.attrs,
                    slot: CellSlot::WideTail,
                },
            );
        }
        Ok(width)
    }

    /// Write `text` starting at `(col, row)`, stopping at the end of the row.
    ///
    /// Returns the column just past the last character written. Characters that
    /// [`CellGrid::write_char`] refuses (controls, combining marks) are skipped;
    /// a wide character that does not fit ends the write, leaving the trailing
    /// column untouched.
    ///
    /// # Errors
    ///
    /// [`WriteError::OutOfBounds`] when the starting coordinate is outside the
    /// grid. A row that simply runs out of columns is not an error.
    pub fn write_str(
        &mut self,
        col: u16,
        row: u16,
        text: &str,
        style: Style,
    ) -> Result<u16, WriteError> {
        if col >= self.cols || row >= self.rows {
            return Err(WriteError::OutOfBounds { col, row });
        }
        let mut cursor = col;
        for ch in text.chars() {
            if cursor >= self.cols {
                break;
            }
            match self.write_char(cursor, row, ch, style) {
                Ok(advance) => cursor += advance,
                Err(WriteError::WideAtRowEnd { .. }) => break,
                Err(WriteError::Control(_) | WriteError::ZeroWidth(_)) => {}
                Err(err @ WriteError::OutOfBounds { .. }) => return Err(err),
            }
        }
        Ok(cursor)
    }

    /// Fill `region` with `cell`, clipped to the grid.
    ///
    /// Returns the number of cells whose value actually changed.
    pub fn fill(&mut self, region: Region, cell: Cell) -> usize {
        let col_end = region.col.saturating_add(region.cols).min(self.cols);
        let row_end = region.row.saturating_add(region.rows).min(self.rows);
        let mut changed = 0;
        for row in region.row..row_end {
            let base = row as usize * self.cols as usize;
            for col in region.col..col_end {
                if self.store(base + col as usize, col, row, cell) {
                    changed += 1;
                }
            }
        }
        changed
    }

    /// Reset every cell to a blank in the current default style.
    ///
    /// Returns the number of cells that changed.
    pub fn clear(&mut self) -> usize {
        let blank = Cell::blank(self.default_style);
        self.fill(Region::all(self), blank)
    }

    /// Reset one row to blanks in the current default style.
    ///
    /// # Errors
    ///
    /// [`GridError::OutOfBounds`] when `row` is outside the grid.
    pub fn clear_row(&mut self, row: u16) -> Result<usize, GridError> {
        if row >= self.rows {
            return Err(GridError::OutOfBounds { col: 0, row });
        }
        let blank = Cell::blank(self.default_style);
        Ok(self.fill(
            Region {
                col: 0,
                row,
                cols: self.cols,
                rows: 1,
            },
            blank,
        ))
    }

    /// Move rows `top..=bottom` up by `count`, filling the vacated rows at the
    /// bottom of the region with `fill`.
    ///
    /// `count == 0` is a no-op and records no damage. A `count` at or beyond the
    /// region height clears the whole region. The move is a single
    /// `copy_within` on the flat cell array; nothing is allocated.
    ///
    /// # Errors
    ///
    /// [`GridError::InvalidRegion`] when the region is inverted or runs past
    /// the last row.
    pub fn scroll_up(
        &mut self,
        top: u16,
        bottom: u16,
        count: u16,
        fill: Cell,
    ) -> Result<(), GridError> {
        self.scroll(top, bottom, count, fill, true)
    }

    /// Move rows `top..=bottom` down by `count`, filling the vacated rows at the
    /// top of the region with `fill`. See [`CellGrid::scroll_up`].
    ///
    /// # Errors
    ///
    /// [`GridError::InvalidRegion`] when the region is inverted or runs past
    /// the last row.
    pub fn scroll_down(
        &mut self,
        top: u16,
        bottom: u16,
        count: u16,
        fill: Cell,
    ) -> Result<(), GridError> {
        self.scroll(top, bottom, count, fill, false)
    }

    fn scroll(
        &mut self,
        top: u16,
        bottom: u16,
        count: u16,
        fill: Cell,
        up: bool,
    ) -> Result<(), GridError> {
        if top > bottom || bottom >= self.rows {
            return Err(GridError::InvalidRegion { top, bottom });
        }
        if count == 0 {
            return Ok(());
        }
        let height = bottom - top + 1;
        let w = self.cols as usize;
        let moved = height.saturating_sub(count);
        if moved > 0 {
            let n = moved as usize * w;
            let (src, dst) = if up {
                ((top + count) as usize * w, top as usize * w)
            } else {
                (top as usize * w, (top + count) as usize * w)
            };
            self.cells.copy_within(src..src + n, dst);
        }
        let blank_rows = if up {
            (top + moved)..=bottom
        } else {
            top..=(bottom - moved)
        };
        for row in blank_rows {
            let base = row as usize * w;
            self.cells[base..base + w].fill(fill);
        }
        for row in top..=bottom {
            self.damage[row as usize].extend(0, self.cols);
        }
        Ok(())
    }

    /// Resize to `cols` x `rows`.
    ///
    /// Content is anchored at the top-left: growing appends blanks on the right
    /// and at the bottom, shrinking truncates. Reflow is deliberately not done
    /// here, because reflow needs the wrap flags only a VT front end knows.
    ///
    /// A wide pair cut in half by a narrower grid has its orphaned head
    /// replaced with a blank, so a double-width glyph can never be left
    /// straddling the right edge.
    ///
    /// Resizing to the current size is a no-op and records no damage. Any real
    /// resize marks the whole grid damaged, because the renderer's instance
    /// buffer has to be rebuilt anyway.
    ///
    /// # Errors
    ///
    /// [`GridError::InvalidSize`] on the same conditions as [`CellGrid::new`].
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), GridError> {
        check_size(cols, rows)?;
        if cols == self.cols && rows == self.rows {
            return Ok(());
        }
        let blank = Cell::blank(self.default_style);

        if cols == self.cols {
            self.cells.resize(cols as usize * rows as usize, blank);
        } else {
            let mut next = vec![blank; cols as usize * rows as usize];
            let copy_cols = cols.min(self.cols) as usize;
            let copy_rows = rows.min(self.rows) as usize;
            for row in 0..copy_rows {
                let src = row * self.cols as usize;
                let dst = row * cols as usize;
                next[dst..dst + copy_cols].copy_from_slice(&self.cells[src..src + copy_cols]);
            }
            self.cells = next;
            // A head left in the final column lost its tail to the truncation.
            if cols < self.cols {
                let last = cols as usize - 1;
                for row in 0..copy_rows {
                    let idx = row * cols as usize + last;
                    if self.cells[idx].slot == CellSlot::WideHead {
                        self.cells[idx] = Cell::blank(self.cells[idx].style());
                    }
                }
            }
        }

        self.cols = cols;
        self.rows = rows;
        self.damage.clear();
        self.damage.resize(
            rows as usize,
            RowDamage {
                start: 0,
                end: cols,
            },
        );
        Ok(())
    }

    /// Damaged spans, in row order, skipping clean rows.
    pub fn damage(&self) -> impl Iterator<Item = DamageSpan> + '_ {
        self.damage
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.is_clean())
            .map(|(row, d)| DamageSpan {
                row: row as u16,
                start: d.start,
                end: d.end,
            })
    }

    /// Number of cells covered by damaged spans.
    ///
    /// This is a span total, not a changed-cell total: a row where only columns
    /// 0 and 199 changed reports 200. The renderer wants the span anyway
    /// because a contiguous upload beats two scattered ones, and the number
    /// that matters (zero on an unchanged frame) is exact.
    #[must_use]
    pub fn dirty_cells(&self) -> usize {
        self.damage.iter().map(|d| d.len()).sum()
    }

    /// True when any row has damage.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.damage.iter().any(|d| !d.is_clean())
    }

    /// Drop all damage. The renderer calls this once it has uploaded a frame.
    pub fn clear_damage(&mut self) {
        self.damage.fill(RowDamage::CLEAN);
    }

    /// Mark every cell damaged. Used when something outside the grid (a resize,
    /// a glyph atlas reset) invalidated the renderer's copy.
    pub fn mark_all_damaged(&mut self) {
        self.damage.fill(RowDamage {
            start: 0,
            end: self.cols,
        });
    }

    /// Store `cell` at flat index `idx`, recording damage only on a real change.
    fn store(&mut self, idx: usize, col: u16, row: u16, cell: Cell) -> bool {
        if self.cells[idx] == cell {
            return false;
        }
        self.cells[idx] = cell;
        self.damage[row as usize].extend(col, col + 1);
        true
    }

    /// Blank the surviving half of any wide pair that straddles the edge of the
    /// range `[col, col + width)` about to be overwritten.
    fn detach_straddling_pairs(&mut self, col: u16, row: u16, width: u16) {
        let base = row as usize * self.cols as usize;
        if col > 0 && self.cells[base + col as usize - 1].slot == CellSlot::WideHead {
            let idx = base + col as usize - 1;
            let blank = Cell::blank(self.cells[idx].style());
            self.store(idx, col - 1, row, blank);
        }
        let last = col + width - 1;
        if last + 1 < self.cols && self.cells[base + last as usize].slot == CellSlot::WideHead {
            let idx = base + last as usize + 1;
            let blank = Cell::blank(self.cells[idx].style());
            self.store(idx, last + 1, row, blank);
        }
    }
}

fn check_size(cols: u16, rows: u16) -> Result<(), GridError> {
    let ok = cols > 0
        && rows > 0
        && cols <= MAX_COLS
        && rows <= MAX_ROWS
        && cols as usize * rows as usize <= MAX_CELLS;
    if ok {
        Ok(())
    } else {
        Err(GridError::InvalidSize { cols, rows })
    }
}
