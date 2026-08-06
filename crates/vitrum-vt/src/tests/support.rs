//! A terminal and a grid of the same size, driven together.
//!
//! Every test wants the same three steps (feed bytes, sync, look at cells), so
//! the fixture owns both halves and exposes the reads a test actually makes.
//! Nothing here fakes anything: it is the real engine and the real grid.

use vitrum_grid::CellGrid;
use vitrum_grid::cell::{Cell, Style};

use crate::{SyncStats, Vt, VtOptions};

/// A terminal wired to a grid.
pub struct Fixture {
    /// The engine under test.
    pub vt: Vt,
    /// The grid it projects onto.
    pub grid: CellGrid,
}

impl Fixture {
    /// A terminal of the given size with generous scrollback.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::with_scrollback(cols, rows, 1 << 20)
    }

    /// A terminal of the given size with an exact scrollback budget.
    pub fn with_scrollback(cols: u16, rows: u16, max_scrollback: usize) -> Self {
        let vt = Vt::new(VtOptions {
            cols,
            rows,
            max_scrollback,
        })
        .expect("engine starts");
        let grid = CellGrid::new(cols, rows, Style::DEFAULT).expect("grid allocates");
        Self { vt, grid }
    }

    /// Feed bytes and project them, returning what the projection cost.
    pub fn write(&mut self, data: &[u8]) -> SyncStats {
        self.vt.feed(data);
        self.sync()
    }

    /// Project without feeding anything.
    pub fn sync(&mut self) -> SyncStats {
        self.vt.sync(&mut self.grid).expect("sync succeeds")
    }

    /// The text of one row, trailing blanks removed.
    pub fn line(&self, row: u16) -> String {
        self.grid
            .row_text(row)
            .expect("row is in bounds")
            .trim_end()
            .to_owned()
    }

    /// Every row's text, trailing blanks removed.
    pub fn lines(&self) -> Vec<String> {
        (0..self.grid.rows()).map(|r| self.line(r)).collect()
    }

    /// One cell.
    pub fn cell(&self, col: u16, row: u16) -> Cell {
        self.grid.cell(col, row).expect("cell is in bounds")
    }
}
