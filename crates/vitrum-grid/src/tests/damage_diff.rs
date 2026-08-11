//! The artefact suite: an incremental frame compared byte for byte against the
//! frame a full unconditional repaint of the same logical state would produce.
//!
//! # Why a second model and not just a second render
//!
//! The renderer clears its attachment and draws every instance each frame, so a
//! cell whose damage was missed is not stale *pixels*, it is a stale *instance*
//! carried over in the persistent instance buffer. Rendering the same
//! [`CellGrid`] twice would therefore find missed damage and nothing else, and
//! the grid can also be wrong about its own cells: an op that writes the correct
//! value into the wrong physical row leaves both renders agreeing on the same
//! corruption.
//!
//! So the reference side is a [`Model`]: a flat `cols * rows` array of cells in
//! logical row order with no scroll indirection, no damage tracking and no
//! reuse, implementing each operation the way the [`CellGrid`] contract says it
//! behaves. Each step renders
//!
//! 1. the real grid through the renderer that has been running since the
//!    scenario started, which is the incremental frame, and
//! 2. a grid built from scratch out of the model through a renderer that is
//!    invalidated first, which is the full-repaint frame,
//!
//! and asserts the two images are identical bytes. A difference is either a
//! missed damage mark or a grid that corrupted its own cells, and the failure
//! message says which by diffing the cell arrays as well as the pixels.
//!
//! # Where the cases come from
//!
//! [`SCENARIOS`] is a list in this source file, and the test walks it. Adding a
//! situation is adding an entry, not adding a screenshot, and every entry runs
//! against every step boundary rather than only at the end: an artefact that
//! appears for one frame and is papered over by the next step is exactly the
//! flicker this exists to catch.

use crate::cell::{Attrs, Cell, CellSlot, CharWidth, Cursor, CursorShape, Rgba, Style, char_width};
use crate::grid::{CellGrid, Region};
use crate::{GridRenderer, HeadlessTarget, Image};

use super::support::{TEST_PX, gpu, renderer_with};

/// Grid the scenarios run on unless they resize.
///
/// Wide enough that a right-edge case is a different column from a boundary
/// case, short enough that a scenario is a handful of milliseconds of readback.
const COLS: u16 = 24;
const ROWS: u16 = 8;

/// Pixel size the targets are allocated at: the largest grid any scenario grows
/// to, so one pair of targets serves every step.
const MAX_COLS: u16 = 32;
const MAX_ROWS: u16 = 12;

const WHITE: Rgba = Rgba::WHITE;
const BLACK: Rgba = Rgba::BLACK;
const BLUE: Rgba = Rgba::new(0x33, 0x66, 0xcc, 0xff);
const AMBER: Rgba = Rgba::new(0xcc, 0x99, 0x33, 0xff);

const PLAIN: Style = Style {
    fg: WHITE,
    bg: BLACK,
    attrs: Attrs::NONE,
};

/// The style a selection overlay paints in.
const SELECTED: Style = Style {
    fg: BLACK,
    bg: BLUE,
    attrs: Attrs::NONE,
};

/// One thing a session does to its grid.
///
/// Every variant is an operation the pane or the VT front end actually issues.
/// Nothing here is a renderer call: the renderer is driven by the harness, once
/// per step, so a step is exactly one frame boundary.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// Write text at a cell, as [`CellGrid::write_str`] does.
    Write {
        col: u16,
        row: u16,
        text: &'static str,
        style: Style,
    },
    /// Overwrite one cell verbatim, as an overlay does.
    SetCell { col: u16, row: u16, cell: Cell },
    /// Recolour a run without touching its characters, which is what an SGR
    /// change over existing text and a selection overlay both look like.
    Restyle {
        row: u16,
        start: u16,
        end: u16,
        style: Style,
    },
    /// Blank a run to the default style, which is what erase-in-line does.
    EraseTail { row: u16, from: u16 },
    /// Blank the whole grid, as an alternate-screen switch does.
    Clear,
    /// Scroll a region up, as output at the bottom of a full screen does.
    ScrollUp { top: u16, bottom: u16, count: u16 },
    /// Scroll a region down, as a reverse index does.
    ScrollDown { top: u16, bottom: u16, count: u16 },
    /// Move, restyle, hide or show the caret.
    SetCursor(Option<Cursor>),
    /// Change the colour new blanks are painted in, which also moves the
    /// renderer's clear colour.
    DefaultStyle(Style),
    /// Follow the widget to a new cell count.
    Resize { cols: u16, rows: u16 },
    /// Fill a rectangle, as a DECALN or a region erase does.
    Fill { region: Region, cell: Cell },
}

const fn cursor_at(col: u16, row: u16, shape: CursorShape) -> Option<Cursor> {
    Some(Cursor {
        col,
        row,
        shape,
        color: AMBER,
    })
}

/// A named sequence of steps, run from a fresh grid at `cols` x `rows`.
struct Scenario {
    name: &'static str,
    cols: u16,
    rows: u16,
    steps: &'static [Step],
}

/// Every situation the artefact suite drives.
///
/// The list is the test's input. A regression is closed by adding the sequence
/// that produced it here, not by adding an assertion somewhere else.
static SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "output at the bottom of a scrolled screen",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 0, text: "first line", style: PLAIN },
            Step::Write { col: 0, row: 1, text: "second line", style: PLAIN },
            Step::Write { col: 0, row: ROWS - 1, text: "prompt$", style: PLAIN },
            Step::ScrollUp { top: 0, bottom: ROWS - 1, count: 1 },
            Step::Write { col: 0, row: ROWS - 1, text: "after scroll", style: PLAIN },
            Step::ScrollUp { top: 0, bottom: ROWS - 1, count: 3 },
            Step::Write { col: 0, row: ROWS - 1, text: "again", style: PLAIN },
        ],
    },
    Scenario {
        name: "scroll reveals rows in both directions",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 0, text: "aaaa", style: PLAIN },
            Step::Write { col: 0, row: 1, text: "bbbb", style: PLAIN },
            Step::Write { col: 0, row: 2, text: "cccc", style: PLAIN },
            Step::Write { col: 0, row: 3, text: "dddd", style: PLAIN },
            Step::ScrollUp { top: 0, bottom: 3, count: 2 },
            Step::ScrollDown { top: 0, bottom: 3, count: 1 },
            Step::ScrollDown { top: 1, bottom: ROWS - 1, count: 2 },
            Step::ScrollUp { top: 2, bottom: 5, count: 4 },
            Step::Write { col: 2, row: 2, text: "revealed", style: PLAIN },
        ],
    },
    Scenario {
        name: "a write next to a wide pair after the rows have rotated",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 0, text: "keep me", style: PLAIN },
            Step::Write { col: 4, row: 3, text: "\u{3042}", style: PLAIN },
            Step::ScrollUp { top: 0, bottom: ROWS - 1, count: 1 },
            // The pair is now on logical row 2, and its physical row is not 2.
            Step::Write { col: 5, row: 2, text: "x", style: PLAIN },
            Step::Write { col: 4, row: 2, text: "\u{3042}", style: PLAIN },
            Step::Write { col: 4, row: 2, text: "y", style: PLAIN },
        ],
    },
    Scenario {
        name: "the caret moves, changes shape, and leaves its last cell",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 0, text: "caret trail", style: PLAIN },
            Step::SetCursor(cursor_at(0, 0, CursorShape::Block)),
            Step::SetCursor(cursor_at(1, 0, CursorShape::Block)),
            Step::SetCursor(cursor_at(1, 0, CursorShape::Bar)),
            Step::SetCursor(cursor_at(1, 0, CursorShape::Underline)),
            Step::SetCursor(cursor_at(1, 0, CursorShape::HollowBlock)),
            Step::SetCursor(cursor_at(10, 4, CursorShape::Block)),
            // Hidden and shown again is what a program toggling DECTCEM does,
            // and it is also what a blink would be if this renderer blinked.
            Step::SetCursor(None),
            Step::SetCursor(cursor_at(10, 4, CursorShape::Block)),
            Step::SetCursor(None),
        ],
    },
    Scenario {
        name: "the caret survives a scroll under it",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 5, text: "under the caret", style: PLAIN },
            Step::SetCursor(cursor_at(3, 5, CursorShape::Block)),
            Step::ScrollUp { top: 0, bottom: ROWS - 1, count: 1 },
            Step::SetCursor(cursor_at(3, 4, CursorShape::Block)),
            Step::ScrollDown { top: 0, bottom: ROWS - 1, count: 2 },
        ],
    },
    Scenario {
        name: "a selection is made, extended, and cleared",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 1, text: "select this text", style: PLAIN },
            Step::Write { col: 0, row: 2, text: "and this line too", style: PLAIN },
            Step::Restyle { row: 1, start: 0, end: 6, style: SELECTED },
            Step::Restyle { row: 1, start: 6, end: 16, style: SELECTED },
            Step::Restyle { row: 2, start: 0, end: 17, style: SELECTED },
            // Cleared by putting the emulator's own colours back, which is what
            // lifting the overlay does.
            Step::Restyle { row: 1, start: 0, end: 16, style: PLAIN },
            Step::Restyle { row: 2, start: 0, end: 17, style: PLAIN },
        ],
    },
    Scenario {
        name: "an SGR change repaints a region without changing its characters",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 0, text: "unchanged characters", style: PLAIN },
            Step::Restyle {
                row: 0,
                start: 0,
                end: 20,
                style: Style { fg: AMBER, bg: BLACK, attrs: Attrs::NONE },
            },
            Step::Restyle {
                row: 0,
                start: 0,
                end: 20,
                style: Style { fg: AMBER, bg: BLACK, attrs: Attrs::UNDERLINE },
            },
            Step::Restyle {
                row: 0,
                start: 0,
                end: 20,
                style: Style { fg: AMBER, bg: BLACK, attrs: Attrs::REVERSE },
            },
            Step::Restyle {
                row: 0,
                start: 0,
                end: 20,
                style: Style { fg: AMBER, bg: BLACK, attrs: Attrs::BOLD },
            },
            Step::DefaultStyle(Style { fg: WHITE, bg: BLUE, attrs: Attrs::NONE }),
            Step::EraseTail { row: 1, from: 0 },
        ],
    },
    Scenario {
        name: "wide characters at a boundary and at the right edge",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 0, text: "\u{3042}\u{3044}\u{3046}", style: PLAIN },
            // Land a narrow character on the tail of the first pair.
            Step::Write { col: 1, row: 0, text: "n", style: PLAIN },
            // Land one on the head of the second pair.
            Step::Write { col: 2, row: 0, text: "m", style: PLAIN },
            // A pair that ends exactly on the last column.
            Step::Write { col: COLS - 2, row: 1, text: "\u{3042}", style: PLAIN },
            // A pair that does not fit, which must leave the row alone.
            Step::Write { col: COLS - 1, row: 2, text: "\u{3042}", style: PLAIN },
            // Break the edge pair from its left.
            Step::Write { col: COLS - 2, row: 1, text: "z", style: PLAIN },
            // A combining mark is refused by the grid and must not move a cell.
            Step::Write { col: 3, row: 3, text: "e\u{0301}f", style: PLAIN },
            Step::Write { col: 0, row: 4, text: "\u{0301}", style: PLAIN },
            Step::SetCursor(cursor_at(0, 0, CursorShape::Block)),
            Step::SetCursor(cursor_at(1, 0, CursorShape::Block)),
        ],
    },
    Scenario {
        name: "a line is rewritten shorter than it was",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 2, text: "a long line of output", style: PLAIN },
            Step::Write { col: 0, row: 2, text: "short", style: PLAIN },
            Step::EraseTail { row: 2, from: 5 },
            Step::Write { col: 0, row: 3, text: "\u{3042}\u{3044}\u{3046}\u{3048}", style: PLAIN },
            Step::Write { col: 0, row: 3, text: "ab", style: PLAIN },
            Step::EraseTail { row: 3, from: 2 },
        ],
    },
    Scenario {
        name: "alternate screen out and back",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 0, text: "primary line one", style: PLAIN },
            Step::Write { col: 0, row: 1, text: "primary line two", style: PLAIN },
            Step::SetCursor(cursor_at(4, 1, CursorShape::Bar)),
            // Into the alternate screen: cleared, then a full-screen program.
            Step::Clear,
            Step::Fill {
                region: Region { col: 0, row: 0, cols: COLS, rows: 1 },
                cell: Cell { ch: ' ', fg: BLACK, bg: AMBER, attrs: Attrs::NONE, slot: CellSlot::Single },
            },
            Step::Write { col: 1, row: 0, text: "editor", style: Style { fg: BLACK, bg: AMBER, attrs: Attrs::NONE } },
            Step::Write { col: 0, row: 3, text: "buffer contents", style: PLAIN },
            Step::SetCursor(cursor_at(0, 3, CursorShape::Block)),
            // Back to the primary screen: cleared, then restored.
            Step::Clear,
            Step::Write { col: 0, row: 0, text: "primary line one", style: PLAIN },
            Step::Write { col: 0, row: 1, text: "primary line two", style: PLAIN },
            Step::SetCursor(cursor_at(4, 1, CursorShape::Bar)),
        ],
    },
    Scenario {
        name: "a resize after the rows have rotated",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 0, row: 0, text: "row zero", style: PLAIN },
            Step::Write { col: 0, row: 1, text: "row one", style: PLAIN },
            Step::Write { col: 0, row: 2, text: "row two", style: PLAIN },
            Step::Write { col: 0, row: 3, text: "row three", style: PLAIN },
            Step::SetCursor(cursor_at(2, 3, CursorShape::Block)),
            Step::ScrollUp { top: 0, bottom: ROWS - 1, count: 2 },
            // Same column count, fewer rows: the fast path in `resize`.
            Step::Resize { cols: COLS, rows: ROWS - 2 },
            Step::Write { col: 0, row: 0, text: "after", style: PLAIN },
            // Same column count, more rows.
            Step::Resize { cols: COLS, rows: MAX_ROWS },
            Step::ScrollUp { top: 0, bottom: MAX_ROWS - 1, count: 3 },
            // A width change, which takes the copying path.
            Step::Resize { cols: MAX_COLS, rows: MAX_ROWS },
            Step::Resize { cols: COLS, rows: ROWS },
        ],
    },
    Scenario {
        name: "a resize that cuts a wide pair in half",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: COLS - 4, row: 0, text: "\u{3042}\u{3044}", style: PLAIN },
            Step::Write { col: COLS - 4, row: 1, text: "\u{3042}\u{3044}", style: PLAIN },
            Step::ScrollUp { top: 0, bottom: ROWS - 1, count: 1 },
            Step::Resize { cols: COLS - 3, rows: ROWS },
            Step::Resize { cols: COLS, rows: ROWS },
            Step::Write { col: COLS - 4, row: 0, text: "ok", style: PLAIN },
        ],
    },
    Scenario {
        name: "an overlay cell written over a wide pair",
        cols: COLS,
        rows: ROWS,
        steps: &[
            Step::Write { col: 3, row: 1, text: "\u{3042}", style: PLAIN },
            // What a find bar or a scrollbar thumb does: a whole-cell write
            // that lands on one half of a pair.
            Step::SetCell {
                col: 4,
                row: 1,
                cell: Cell { ch: '#', fg: BLACK, bg: BLUE, attrs: Attrs::NONE, slot: CellSlot::Single },
            },
            Step::SetCell {
                col: 3,
                row: 1,
                cell: Cell { ch: ' ', fg: WHITE, bg: BLACK, attrs: Attrs::NONE, slot: CellSlot::Single },
            },
            Step::Write { col: 3, row: 1, text: "\u{3042}", style: PLAIN },
            Step::SetCell {
                col: 3,
                row: 1,
                cell: Cell { ch: '#', fg: BLACK, bg: BLUE, attrs: Attrs::NONE, slot: CellSlot::Single },
            },
            Step::SetCell {
                col: 4,
                row: 1,
                cell: Cell { ch: ' ', fg: WHITE, bg: BLACK, attrs: Attrs::NONE, slot: CellSlot::Single },
            },
        ],
    },
];

/// The reference grid: logical rows, no indirection, no damage, no reuse.
///
/// This is the specification the [`CellGrid`] is measured against, so every
/// method here is written from the documented behaviour rather than from the
/// implementation. Where the two disagree the harness fails, and which of them
/// is wrong is a judgement the failure message supports rather than makes.
#[derive(Clone)]
struct Model {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    default_style: Style,
    cursor: Option<Cursor>,
}

impl Model {
    fn new(cols: u16, rows: u16, default_style: Style) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::blank(default_style); cols as usize * rows as usize],
            default_style,
            cursor: None,
        }
    }

    fn idx(&self, col: u16, row: u16) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    fn get(&self, col: u16, row: u16) -> Cell {
        self.cells[self.idx(col, row)]
    }

    fn put(&mut self, col: u16, row: u16, cell: Cell) {
        let i = self.idx(col, row);
        self.cells[i] = cell;
    }

    /// Blank the surviving half of a pair the range `[col, col + width)` breaks.
    fn detach(&mut self, col: u16, row: u16, width: u16) {
        if col > 0 && self.get(col - 1, row).slot == CellSlot::WideHead {
            let style = self.get(col - 1, row).style();
            self.put(col - 1, row, Cell::blank(style));
        }
        let last = col + width - 1;
        if last + 1 < self.cols && self.get(last, row).slot == CellSlot::WideHead {
            let style = self.get(last + 1, row).style();
            self.put(last + 1, row, Cell::blank(style));
        }
    }

    fn write_char(&mut self, col: u16, row: u16, ch: char, style: Style) -> Option<u16> {
        let width = match char_width(ch) {
            CharWidth::Control | CharWidth::ZeroWidth => return None,
            CharWidth::Narrow => 1u16,
            CharWidth::Wide => 2u16,
        };
        if col + width > self.cols {
            return Some(0);
        }
        self.detach(col, row, width);
        if width == 1 {
            self.put(
                col,
                row,
                Cell { ch, fg: style.fg, bg: style.bg, attrs: style.attrs, slot: CellSlot::Single },
            );
        } else {
            self.put(
                col,
                row,
                Cell { ch, fg: style.fg, bg: style.bg, attrs: style.attrs, slot: CellSlot::WideHead },
            );
            self.put(
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
        Some(width)
    }

    fn write_str(&mut self, col: u16, row: u16, text: &str, style: Style) {
        if col >= self.cols || row >= self.rows {
            return;
        }
        let mut cursor = col;
        for ch in text.chars() {
            if cursor >= self.cols {
                break;
            }
            match self.write_char(cursor, row, ch, style) {
                // A wide character with no room ends the write.
                Some(0) => break,
                Some(advance) => cursor += advance,
                // A control or a combining mark is skipped.
                None => {}
            }
        }
    }

    fn fill(&mut self, region: Region, cell: Cell) {
        let col_end = region.col.saturating_add(region.cols).min(self.cols);
        let row_end = region.row.saturating_add(region.rows).min(self.rows);
        for row in region.row..row_end {
            for col in region.col..col_end {
                self.put(col, row, cell);
            }
        }
    }

    fn scroll_up(&mut self, top: u16, bottom: u16, count: u16, fill: Cell) {
        if count == 0 || top > bottom || bottom >= self.rows {
            return;
        }
        let height = bottom - top + 1;
        let count = count.min(height);
        let w = self.cols as usize;
        let rows: Vec<Vec<Cell>> = (top..=bottom)
            .map(|r| self.cells[r as usize * w..r as usize * w + w].to_vec())
            .collect();
        for (i, r) in (top..=bottom).enumerate() {
            let src = i + count as usize;
            let base = r as usize * w;
            if src < rows.len() {
                self.cells[base..base + w].copy_from_slice(&rows[src]);
            } else {
                self.cells[base..base + w].fill(fill);
            }
        }
    }

    fn scroll_down(&mut self, top: u16, bottom: u16, count: u16, fill: Cell) {
        if count == 0 || top > bottom || bottom >= self.rows {
            return;
        }
        let height = bottom - top + 1;
        let count = count.min(height) as usize;
        let w = self.cols as usize;
        let rows: Vec<Vec<Cell>> = (top..=bottom)
            .map(|r| self.cells[r as usize * w..r as usize * w + w].to_vec())
            .collect();
        for (i, r) in (top..=bottom).enumerate() {
            let base = r as usize * w;
            if i >= count {
                self.cells[base..base + w].copy_from_slice(&rows[i - count]);
            } else {
                self.cells[base..base + w].fill(fill);
            }
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        let blank = Cell::blank(self.default_style);
        let mut next = vec![blank; cols as usize * rows as usize];
        let copy_cols = cols.min(self.cols) as usize;
        let copy_rows = rows.min(self.rows) as usize;
        for row in 0..copy_rows {
            let src = row * self.cols as usize;
            let dst = row * cols as usize;
            next[dst..dst + copy_cols].copy_from_slice(&self.cells[src..src + copy_cols]);
        }
        if cols < self.cols {
            let last = cols as usize - 1;
            for row in 0..copy_rows {
                let idx = row * cols as usize + last;
                if next[idx].slot == CellSlot::WideHead {
                    next[idx] = Cell::blank(next[idx].style());
                }
            }
        }
        self.cells = next;
        self.cols = cols;
        self.rows = rows;
        if let Some(c) = self.cursor
            && (c.col >= cols || c.row >= rows)
        {
            self.cursor = None;
        }
    }

    fn apply(&mut self, step: Step) {
        match step {
            Step::Write { col, row, text, style } => self.write_str(col, row, text, style),
            Step::SetCell { col, row, cell } => {
                if col < self.cols && row < self.rows {
                    self.put(col, row, cell);
                }
            }
            Step::Restyle { row, start, end, style } => {
                for col in start..end.min(self.cols) {
                    let was = self.get(col, row);
                    self.put(
                        col,
                        row,
                        Cell { fg: style.fg, bg: style.bg, attrs: style.attrs, ..was },
                    );
                }
            }
            Step::EraseTail { row, from } => {
                let blank = Cell::blank(self.default_style);
                self.fill(
                    Region { col: from, row, cols: self.cols - from, rows: 1 },
                    blank,
                );
            }
            Step::Clear => {
                let blank = Cell::blank(self.default_style);
                self.fill(
                    Region { col: 0, row: 0, cols: self.cols, rows: self.rows },
                    blank,
                );
            }
            Step::ScrollUp { top, bottom, count } => {
                let blank = Cell::blank(self.default_style);
                self.scroll_up(top, bottom, count, blank);
            }
            Step::ScrollDown { top, bottom, count } => {
                let blank = Cell::blank(self.default_style);
                self.scroll_down(top, bottom, count, blank);
            }
            Step::SetCursor(cursor) => {
                let ok = cursor.is_none_or(|c| c.col < self.cols && c.row < self.rows);
                if ok {
                    self.cursor = cursor;
                }
            }
            Step::DefaultStyle(style) => self.default_style = style,
            Step::Resize { cols, rows } => self.resize(cols, rows),
            Step::Fill { region, cell } => self.fill(region, cell),
        }
    }

    /// A grid holding exactly this model's cells, freshly built so no scroll
    /// indirection and no damage history can carry into it.
    fn to_grid(&self) -> CellGrid {
        let mut grid = CellGrid::new(self.cols, self.rows, self.default_style)
            .expect("model dimensions must be valid");
        for row in 0..self.rows {
            for col in 0..self.cols {
                grid.set_cell(col, row, self.get(col, row))
                    .expect("model coordinate must be inside its own grid");
            }
        }
        grid.set_cursor(self.cursor)
            .expect("model caret must be inside its own grid");
        grid.mark_all_damaged();
        grid
    }
}

/// Apply one step to the real grid, ignoring the refusals the model also
/// ignores. A refusal is a legitimate outcome the two sides must agree on, and
/// the agreement is checked by the pixel comparison rather than by asserting on
/// the error here.
fn apply_to_grid(grid: &mut CellGrid, step: Step) {
    match step {
        Step::Write { col, row, text, style } => {
            let _ = grid.write_str(col, row, text, style);
        }
        Step::SetCell { col, row, cell } => {
            let _ = grid.set_cell(col, row, cell);
        }
        Step::Restyle { row, start, end, style } => {
            for col in start..end.min(grid.cols()) {
                let Some(was) = grid.cell(col, row) else {
                    continue;
                };
                let _ = grid.set_cell(
                    col,
                    row,
                    Cell { fg: style.fg, bg: style.bg, attrs: style.attrs, ..was },
                );
            }
        }
        Step::EraseTail { row, from } => {
            let blank = Cell::blank(grid.default_style());
            grid.fill(
                Region { col: from, row, cols: grid.cols() - from, rows: 1 },
                blank,
            );
        }
        Step::Clear => {
            grid.clear();
        }
        Step::ScrollUp { top, bottom, count } => {
            let blank = Cell::blank(grid.default_style());
            let _ = grid.scroll_up(top, bottom, count, blank);
        }
        Step::ScrollDown { top, bottom, count } => {
            let blank = Cell::blank(grid.default_style());
            let _ = grid.scroll_down(top, bottom, count, blank);
        }
        Step::SetCursor(cursor) => {
            let _ = grid.set_cursor(cursor);
        }
        Step::DefaultStyle(style) => grid.set_default_style(style),
        Step::Resize { cols, rows } => {
            grid.resize(cols, rows).expect("scenario resize must be valid");
        }
        Step::Fill { region, cell } => {
            grid.fill(region, cell);
        }
    }
}

/// The first cell whose value differs between the live grid and the model.
fn first_cell_difference(grid: &CellGrid, model: &Model) -> Option<String> {
    if (grid.cols(), grid.rows()) != (model.cols, model.rows) {
        return Some(format!(
            "grid is {}x{} but the model is {}x{}",
            grid.cols(),
            grid.rows(),
            model.cols,
            model.rows
        ));
    }
    if grid.cursor() != model.cursor {
        return Some(format!(
            "caret: grid {:?}, model {:?}",
            grid.cursor(),
            model.cursor
        ));
    }
    for row in 0..model.rows {
        for col in 0..model.cols {
            let live = grid.cell(col, row).expect("in-bounds cell");
            let want = model.get(col, row);
            if live != want {
                return Some(format!(
                    "cell ({col}, {row}): grid {live:?}, model {want:?}"
                ));
            }
        }
    }
    None
}

/// The first pixel where two images differ, named by the cell it falls in.
fn first_pixel_difference(
    incremental: &Image,
    full: &Image,
    cell_px: (u32, u32),
) -> Option<String> {
    if incremental.as_bytes() == full.as_bytes() {
        return None;
    }
    for y in 0..incremental.height() {
        for x in 0..incremental.width() {
            let a = incremental.pixel(x, y);
            let b = full.pixel(x, y);
            if a != b {
                let (cw, ch) = cell_px;
                return Some(format!(
                    "pixel ({x}, {y}) in cell ({}, {}): incremental {a:?}, full repaint {b:?}",
                    x / cw.max(1),
                    y / ch.max(1)
                ));
            }
        }
    }
    // Sizes differ, so no coordinate is common to both.
    Some(format!(
        "image sizes differ: incremental {}x{}, full repaint {}x{}",
        incremental.width(),
        incremental.height(),
        full.width(),
        full.height()
    ))
}

/// A pair of renderers over one device, plus the targets they draw into.
struct Harness {
    incremental: GridRenderer,
    reference: GridRenderer,
    a: HeadlessTarget,
    b: HeadlessTarget,
}

impl Harness {
    fn new() -> Self {
        // Two renderers so the incremental one keeps every scrap of state it
        // accumulated while the reference one is invalidated on every frame.
        // They share a device, a queue, and a font database, and nothing else.
        let incremental = renderer_with(TEST_PX, crate::DEFAULT_ATLAS_DIM);
        let reference = renderer_with(TEST_PX, crate::DEFAULT_ATLAS_DIM);
        let (cw, ch) = incremental.cell_size();
        let (w, h) = (cw * u32::from(MAX_COLS), ch * u32::from(MAX_ROWS));
        Self {
            incremental,
            reference,
            a: HeadlessTarget::new(gpu().device(), w, h),
            b: HeadlessTarget::new(gpu().device(), w, h),
        }
    }

    /// The pixel viewport a `cols` x `rows` grid occupies, which is what both
    /// sides are rendered at so a smaller grid does not compare its unused
    /// margin against a differently sized one.
    fn viewport(&self, cols: u16, rows: u16) -> (u32, u32) {
        self.incremental.pixel_size_for(cols, rows)
    }

    fn draw_incremental(&mut self, grid: &mut CellGrid) -> Image {
        let viewport = self.viewport(grid.cols(), grid.rows());
        self.incremental
            .render(gpu().device(), gpu().queue(), grid, self.a.view(), viewport)
            .expect("incremental render must succeed");
        self.a.read(gpu().device(), gpu().queue())
    }

    fn draw_full(&mut self, grid: &mut CellGrid) -> Image {
        let viewport = self.viewport(grid.cols(), grid.rows());
        // Nothing this renderer believes about the previous frame may carry
        // into this one: that is what makes it the full-repaint reference.
        self.reference.invalidate();
        self.reference
            .render(gpu().device(), gpu().queue(), grid, self.b.view(), viewport)
            .expect("full repaint must succeed");
        self.b.read(gpu().device(), gpu().queue())
    }
}

/// Every scenario, compared frame by frame against a full repaint.
///
/// This is the artefact gate. It goes red when an incremental frame differs
/// from the frame a full unconditional repaint of the same logical state
/// produces, which is the definition of a rendering artefact that will not
/// repair itself.
#[test]
fn incremental_frames_match_a_full_repaint() {
    let mut harness = Harness::new();
    let cell_px = harness.incremental.cell_size();

    for scenario in SCENARIOS {
        let mut grid = CellGrid::new(scenario.cols, scenario.rows, PLAIN)
            .expect("scenario dimensions must be valid");
        let mut model = Model::new(scenario.cols, scenario.rows, PLAIN);

        // The renderer starts each scenario with no memory of the last one, so
        // a scenario's first frame is a genuine first frame.
        harness.incremental.invalidate();
        let _ = harness.draw_incremental(&mut grid);

        for (n, step) in scenario.steps.iter().copied().enumerate() {
            apply_to_grid(&mut grid, step);
            model.apply(step);

            let incremental = harness.draw_incremental(&mut grid);
            let mut reference = model.to_grid();
            let full = harness.draw_full(&mut reference);

            if let Some(what) = first_pixel_difference(&incremental, &full, cell_px) {
                let cells = first_cell_difference(&grid, &model)
                    .unwrap_or_else(|| "cells agree, so the damage marks do not".to_owned());
                panic!(
                    "{}: step {n} ({step:?}) left the incremental frame different \
                     from a full repaint\n  {what}\n  {cells}",
                    scenario.name
                );
            }
        }
    }
}

/// The suite must be able to see an artefact, or it proves nothing.
///
/// A grid whose damage is dropped without being uploaded is exactly what a
/// missed damage mark is, and the comparison the real test runs has to fail on
/// it. Without this, a harness that compared an image against itself would look
/// green forever.
#[test]
fn the_comparison_detects_a_dropped_damage_mark() {
    let mut harness = Harness::new();
    let cell_px = harness.incremental.cell_size();

    let mut grid = CellGrid::new(COLS, ROWS, PLAIN).expect("dimensions must be valid");
    let mut model = Model::new(COLS, ROWS, PLAIN);
    harness.incremental.invalidate();
    let _ = harness.draw_incremental(&mut grid);

    let step = Step::Write { col: 2, row: 2, text: "artefact", style: PLAIN };
    apply_to_grid(&mut grid, step);
    model.apply(step);
    // Drop the marks without uploading them, which is what every bug this
    // suite hunts does by accident.
    grid.clear_damage();

    let incremental = harness.draw_incremental(&mut grid);
    let mut reference = model.to_grid();
    let full = harness.draw_full(&mut reference);
    assert!(
        first_pixel_difference(&incremental, &full, cell_px).is_some(),
        "a dropped damage mark must show up as a pixel difference"
    );
}

/// An atlas that fills and resets must not leave a cell pointing at texels
/// another glyph has since been written over.
///
/// The atlas rewinds its packer and empties its entry map rather than growing,
/// and it does not repaint the texture: a glyph placed after a reset overwrites
/// the pixels an earlier glyph occupied. Every instance already uploaded this
/// frame, and every instance still resident from an earlier frame, holds the
/// old coordinates, so a reset is only safe if the renderer rebuilds all of
/// them. This drives a small atlas through more distinct glyphs than it can
/// hold and compares each frame against a full repaint through an atlas large
/// enough never to reset.
#[test]
fn an_atlas_reset_repaints_every_cell_that_referenced_it() {
    // 256 is the floor `GlyphAtlas::new` clamps to, and it holds a couple of
    // hundred glyph boxes at this font size. One frame's live set fits; the
    // running total across frames does not, so the atlas resets between frames
    // while the top row's instances still point into it.
    const A_COLS: u16 = 16;
    const A_ROWS: u16 = 4;
    const FRAMES: u32 = 16;

    let mut incremental = renderer_with(TEST_PX, 256);
    let mut reference = renderer_with(TEST_PX, crate::DEFAULT_ATLAS_DIM);
    let (cw, ch) = incremental.cell_size();
    let a = HeadlessTarget::new(gpu().device(), cw * u32::from(A_COLS), ch * u32::from(A_ROWS));
    let b = HeadlessTarget::new(gpu().device(), cw * u32::from(A_COLS), ch * u32::from(A_ROWS));
    let viewport = (a.width(), a.height());

    let mut grid = CellGrid::new(A_COLS, A_ROWS, PLAIN).expect("dimensions must be valid");
    let mut model = Model::new(A_COLS, A_ROWS, PLAIN);

    // CJK, because each one is a distinct glyph box and one contiguous block
    // holds far more of them than this atlas can.
    let mut next = 0x4e00u32;
    let mut take = |count: u16| -> String {
        (0..count)
            .map(|_| {
                let ch = char::from_u32(next).expect("CJK block is all valid scalars");
                next += 1;
                ch
            })
            .collect()
    };

    // Row zero is written once and never touched again. Nothing damages it
    // after this frame, so it repaints correctly only if a reset rebuilds it.
    let fixed = take(A_COLS / 2);
    let _ = grid.write_str(0, 0, &fixed, PLAIN);
    model.write_str(0, 0, &fixed, PLAIN);

    for frame in 0..FRAMES {
        for row in 1..A_ROWS {
            let text = take(A_COLS / 2);
            let _ = grid.write_str(0, row, &text, PLAIN);
            model.write_str(0, row, &text, PLAIN);
        }

        incremental
            .render(gpu().device(), gpu().queue(), &mut grid, a.view(), viewport)
            .expect("incremental render must succeed");
        let inc = a.read(gpu().device(), gpu().queue());

        let mut want = model.to_grid();
        reference.invalidate();
        reference
            .render(gpu().device(), gpu().queue(), &mut want, b.view(), viewport)
            .expect("full repaint must succeed");
        let full = b.read(gpu().device(), gpu().queue());

        if let Some(what) = first_pixel_difference(&inc, &full, (cw, ch)) {
            panic!("frame {frame} differs from a full repaint: {what}");
        }
    }
    assert!(
        incremental.atlas().generation() > 0,
        "the small atlas never reset, so this test proved nothing about a reset"
    );
}

/// A host must not gate its paint on the grid's damage alone.
///
/// WHY: the pane skipped a frame whenever no cell had changed. That is right
/// while the renderer still owns the images it drew into, and wrong the moment
/// the swapchain is reconfigured: a resize inside one cell, a present-mode
/// change and a font rebuild all hand back a fresh set of images with
/// undefined contents, and none of them touches a cell. The pane then showed
/// whatever was in that memory until the child happened to write something,
/// which is a pane that goes to garbage on a window drag and stays there.
///
/// The invariant: when what is on screen is no longer the frame this renderer
/// drew, the renderer says so, and repainting a clean grid reproduces the frame
/// exactly. The second target stands in for the new swapchain image, and it is
/// never written by anything else, so a skipped repaint leaves it blank.
#[test]
fn a_clean_grid_still_owes_a_frame_after_the_target_is_replaced() {
    let mut renderer = renderer_with(TEST_PX, crate::DEFAULT_ATLAS_DIM);
    let (cw, ch) = renderer.cell_size();
    let first = HeadlessTarget::new(gpu().device(), cw * u32::from(COLS), ch * u32::from(ROWS));
    let replacement =
        HeadlessTarget::new(gpu().device(), cw * u32::from(COLS), ch * u32::from(ROWS));
    let viewport = (first.width(), first.height());

    let mut grid = CellGrid::new(COLS, ROWS, PLAIN).expect("dimensions must be valid");
    let _ = grid.write_str(0, 1, "on screen", PLAIN);
    renderer
        .render(gpu().device(), gpu().queue(), &mut grid, first.view(), viewport)
        .expect("the first frame must draw");
    let drawn = first.read(gpu().device(), gpu().queue());

    // Steady state: nothing changed and nothing is owed, which is the case the
    // change gate exists for.
    assert!(!grid.is_dirty());
    assert!(!renderer.needs_rebuild());
    let idle = renderer
        .render(gpu().device(), gpu().queue(), &mut grid, first.view(), viewport)
        .expect("an idle frame must not fail");
    assert!(!idle.gpu_work, "an unchanged frame did GPU work");

    // The host reconfigured its swapchain. No cell changed.
    renderer.invalidate();
    assert!(
        !grid.is_dirty(),
        "the grid alone cannot tell a host that a frame is owed here"
    );
    assert!(
        renderer.needs_rebuild(),
        "the renderer did not say the previous frame is gone, so a host gating \
         on the grid would skip this frame and show an unpainted image"
    );

    let stats = renderer
        .render(
            gpu().device(),
            gpu().queue(),
            &mut grid,
            replacement.view(),
            viewport,
        )
        .expect("the repaint must draw");
    assert!(stats.gpu_work, "the repaint recorded no GPU work");
    assert!(stats.full_rebuild, "the repaint was not a full rebuild");
    assert_eq!(
        replacement.read(gpu().device(), gpu().queue()).as_bytes(),
        drawn.as_bytes(),
        "the repainted image is not the frame that was on screen"
    );
    assert!(
        !renderer.needs_rebuild(),
        "the renderer still owes a frame after drawing one"
    );
}
