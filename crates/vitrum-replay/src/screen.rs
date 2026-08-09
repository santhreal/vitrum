//! The reconstructed terminal state, projected out of the engine.
//!
//! A [`Screen`] is what a caller can see of a replayed session at one position:
//! a [`vitrum_grid::CellGrid`] of cells, where the cursor is, and the window
//! title. It is a projection, not the model. The model — cursor arithmetic,
//! scroll regions, mode flags, tab stops, charset designations, the alternate
//! buffer, the saved cursor — lives inside Ghostty, reached through
//! [`crate::Emulator`], and this crate no longer keeps a second copy of it.
//!
//! # Why the state accessors are gone
//!
//! This type used to expose `pen`, `modes`, `region`, `charsets` and `tabs`,
//! because a hand-written parser in this crate, built on the `vte` crate, owned
//! them. Ghostty owns them now and libghostty's C API does not hand them back: it
//! reports the grid, the cursor and the DEC mode bits, and nothing else.
//! Publishing fields this crate could only guess at would be worse than not
//! publishing them, so the tests that used to read those fields assert the
//! behaviour they cause instead — a scroll region is proved by what scrolls, not
//! by two integers.
//!
//! # Equality
//!
//! [`Screen`] compares by *state*: every cell, the cursor, and the title. The
//! grid's damage spans are excluded on purpose: damage records which cells a
//! renderer still has to upload, and a screen produced by a seek has a different
//! upload history from one fed linearly while describing the identical terminal.
//! Comparing damage would make the seek-equivalence test assert something no
//! user can see.
//!
//! # Scrollback
//!
//! There is none. A screen is `rows` tall and a row that scrolls off the top is
//! gone, exactly as in a terminal with no scrollback. Nothing is lost, because
//! the session's scrollback is the byte stream being replayed; it just is not
//! stored twice.

use vitrum_grid::{Cell, CellGrid, CellSlot, Rgba};

use crate::error::{Error, Result};
use crate::palette::Palette;

/// Where the cursor is, and whether it is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cursor {
    /// Column, zero based.
    pub col: u16,
    /// Row, zero based.
    pub row: u16,
    /// False after `CSI ? 25 l` (DECTCEM).
    pub visible: bool,
}

impl Cursor {
    /// The top left, visible, which is where a terminal powers on.
    pub const HOME: Self = Self {
        col: 0,
        row: 0,
        visible: true,
    };
}

/// The visible terminal state at one point in a session's stream.
#[derive(Clone, Debug)]
pub struct Screen {
    grid: CellGrid,
    cursor: Cursor,
    title: String,
    palette: Palette,
}

impl Screen {
    /// A blank `cols` x `rows` screen painted in `palette`'s defaults.
    ///
    /// # Errors
    ///
    /// [`Error::Geometry`] when the size is not one [`CellGrid`] accepts.
    pub fn new(cols: u16, rows: u16, palette: Palette) -> Result<Self> {
        let grid = CellGrid::new(cols, rows, palette.default_style())
            .map_err(|_| Error::Geometry { cols, rows })?;
        Ok(Self {
            grid,
            cursor: Cursor::HOME,
            title: String::new(),
            palette,
        })
    }

    /// The visible cell grid, ready to hand to [`vitrum_grid::GridRenderer`].
    #[must_use]
    pub const fn grid(&self) -> &CellGrid {
        &self.grid
    }

    /// The visible cell grid, mutable, for a renderer that needs to clear damage
    /// after uploading a frame.
    pub const fn grid_mut(&mut self) -> &mut CellGrid {
        &mut self.grid
    }

    /// Column count.
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.grid.cols()
    }

    /// Row count.
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.grid.rows()
    }

    /// Where the cursor is.
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The colours this screen's blanks and defaults were painted in.
    #[must_use]
    pub const fn palette(&self) -> &Palette {
        &self.palette
    }

    /// The last title the session set with OSC 0 or OSC 2, or empty.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// One row's text, with trailing blanks kept.
    ///
    /// A row past the bottom of the screen has no text and yields an empty
    /// string, which keeps this usable as a display and test helper without a
    /// `Result` at every call site.
    #[must_use]
    pub fn line(&self, row: u16) -> String {
        self.grid.row_text(row).unwrap_or_default()
    }

    /// Every row, newline separated, each right-trimmed.
    ///
    /// Trimming is what makes two screens diffable in a test failure; the cells
    /// themselves are untouched and [`Screen::grid`] still reports the blanks.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        for row in 0..self.rows() {
            if row > 0 {
                out.push('\n');
            }
            out.push_str(self.line(row).trim_end());
        }
        out
    }

    /// Bytes this screen holds on the heap.
    ///
    /// This is the grid and the title, which is everything this crate allocates
    /// for a screen. The engine behind it has an arena of its own that
    /// libghostty does not report; see [`crate::KeyframeIndex::heap_bytes`].
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.grid.len() * core::mem::size_of::<Cell>()
            + self.grid.rows() as usize * core::mem::size_of::<u32>()
            + self.title.capacity()
    }

    /// Overwrite the projection from the engine.
    pub(crate) fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }

    /// Record a title the engine reported through OSC.
    pub(crate) fn set_title(&mut self, title: String) {
        self.title = title;
    }
}

impl PartialEq for Screen {
    /// Cells, cursor and title. Damage is bookkeeping, not state; see the module
    /// header.
    fn eq(&self, other: &Self) -> bool {
        // Row by row rather than buffer against buffer. The grid scrolls by
        // rotating an index rather than moving cells, so two screens showing
        // the same thing can hold it in a different physical order, and
        // comparing the raw buffers would call them different.
        self.cursor == other.cursor
            && self.title == other.title
            && self.grid.cols() == other.grid.cols()
            && self.grid.rows() == other.grid.rows()
            && (0..self.grid.rows()).all(|r| self.grid.row(r) == other.grid.row(r))
    }
}

impl Eq for Screen {}

/// Cells that hold a glyph, for tests and for a caller diffing two screens.
///
/// A [`CellSlot::WideTail`] is skipped, because it draws nothing and its
/// character is a placeholder.
#[must_use]
pub fn glyph_cells(grid: &CellGrid) -> Vec<(u16, u16, char, Rgba)> {
    let mut out = Vec::new();
    for row in 0..grid.rows() {
        for col in 0..grid.cols() {
            let Some(cell) = grid.cell(col, row) else {
                continue;
            };
            if cell.slot == CellSlot::WideTail || cell.ch == ' ' {
                continue;
            }
            out.push((col, row, cell.ch, cell.fg));
        }
    }
    out
}
