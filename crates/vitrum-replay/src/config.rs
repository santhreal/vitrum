//! What a replay needs to know before it starts.
//!
//! Screen size is the only thing here with no sensible default. A terminal's
//! output is meaningless without it: the same bytes wrap at column 80 and at
//! column 200 into two different screens, and `CSI 24 ; 80 H` addresses a cell
//! that may not exist. The daemon knows the geometry (it set the PTY window size),
//! and an asciicast header carries it, so a caller always has it.
//!
//! # What used to be here
//!
//! A keyframe stride and a ground-scan bound, both of which configured an index
//! of cloned screens that no longer exists. Ghostty's terminal state cannot be
//! cloned, so there is nothing to tune: a forward seek feeds the bytes it crosses
//! and a rewind replays from the base of the stream. See [`crate::replay`].

use vitrum_grid::{MAX_CELLS, MAX_COLS, MAX_ROWS};

use crate::error::{Error, Result};
use crate::palette::Palette;

/// Screen size and colours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReplayConfig {
    /// Screen width in columns.
    pub cols: u16,
    /// Screen height in rows.
    pub rows: u16,
    /// What the default foreground and background resolve to. See [`Palette`].
    pub palette: Palette,
}

impl ReplayConfig {
    /// A configuration for a `cols` x `rows` screen with every default.
    ///
    /// # Errors
    ///
    /// [`Error::Geometry`] when the size is not one [`vitrum_grid::CellGrid`]
    /// accepts, checked here rather than at first use so a caller finds out before
    /// building a replay.
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        let config = Self {
            cols,
            rows,
            palette: Palette::XTERM,
        };
        config.validate()?;
        Ok(config)
    }

    /// The same configuration with a different palette.
    #[must_use]
    pub const fn with_palette(mut self, palette: Palette) -> Self {
        self.palette = palette;
        self
    }

    /// Check the geometry.
    ///
    /// # Errors
    ///
    /// [`Error::Geometry`].
    pub const fn validate(&self) -> Result<()> {
        let cells = self.cols as usize * self.rows as usize;
        let ok = self.cols > 0
            && self.rows > 0
            && self.cols <= MAX_COLS
            && self.rows <= MAX_ROWS
            && cells <= MAX_CELLS;
        if !ok {
            return Err(Error::Geometry {
                cols: self.cols,
                rows: self.rows,
            });
        }
        Ok(())
    }
}
