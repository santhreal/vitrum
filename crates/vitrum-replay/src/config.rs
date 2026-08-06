//! What a replay needs to know before it starts.
//!
//! Screen size is the only thing here with no sensible default. A terminal's
//! output is meaningless without it: the same bytes wrap at column 80 and at
//! column 200 into two different screens, and `CSI 24 ; 80 H` addresses a cell
//! that may not exist. The daemon knows the geometry (it set the PTY window size),
//! and an asciicast header carries it, so a caller always has it.

use vitrum_grid::{MAX_CELLS, MAX_COLS, MAX_ROWS};

use crate::error::{Error, Result};
use crate::palette::Palette;

/// Bytes between keyframes, by default.
///
/// 256 KiB is chosen against the cost of the two things it trades off. A seek
/// feeds at most one stride, and feeding 256 KiB of terminal output through the
/// parser and grid is well under a millisecond, so a scrubber stays interactive.
/// A 10 MiB ring then holds 40 keyframes, and at 80x24 a keyframe is about 31 KiB,
/// so the index costs roughly 1.2 MiB for a session whose bytes cost 10 MiB.
/// Halving the stride halves seek latency and doubles that memory.
pub const DEFAULT_KEYFRAME_STRIDE: usize = 256 * 1024;

/// How far past a stride boundary [`crate::KeyframeIndex::build`] will look for a
/// byte boundary the parser can be resumed from.
///
/// Ordinary output reaches one within a byte or two. The bound matters for the
/// pathological case: a program emitting one enormous OSC string, or a binary file
/// catted to the terminal, can run for a long way with no dispatch that returns to
/// ground. 4 KiB past the boundary is generous, and giving up costs one skipped
/// keyframe rather than an unbounded scan.
pub const DEFAULT_GROUND_SCAN: usize = 4096;

/// Screen size, colours, and how densely to keyframe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ReplayConfig {
    /// Screen width in columns.
    pub cols: u16,
    /// Screen height in rows.
    pub rows: u16,
    /// What indexed rendition resolves to. See [`Palette`].
    pub palette: Palette,
    /// Bytes between keyframes. See [`DEFAULT_KEYFRAME_STRIDE`].
    pub keyframe_stride: usize,
    /// Ground-boundary search bound. See [`DEFAULT_GROUND_SCAN`].
    pub ground_scan: usize,
}

impl ReplayConfig {
    /// A configuration for a `cols` x `rows` screen with every default.
    ///
    /// # Errors
    ///
    /// [`Error::Geometry`] when the size is not one [`vitrum_grid::CellGrid`]
    /// accepts, checked here rather than at first use so a caller finds out before
    /// building an index.
    pub fn new(cols: u16, rows: u16) -> Result<Self> {
        let config = Self {
            cols,
            rows,
            palette: Palette::XTERM,
            keyframe_stride: DEFAULT_KEYFRAME_STRIDE,
            ground_scan: DEFAULT_GROUND_SCAN,
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

    /// The same configuration with a different keyframe density.
    ///
    /// # Errors
    ///
    /// [`Error::ZeroStride`] for a stride of zero, which would ask for one
    /// full-screen snapshot per byte.
    pub const fn with_keyframe_stride(mut self, stride: usize) -> Result<Self> {
        if stride == 0 {
            return Err(Error::ZeroStride);
        }
        self.keyframe_stride = stride;
        Ok(self)
    }

    /// The same configuration with a different ground-boundary search bound.
    ///
    /// Zero is legal and disables keyframing altogether: no boundary is ever
    /// searched for, so no keyframe is ever taken and every seek replays from the
    /// start of the stream. That is a real choice for a caller with no memory to
    /// spare, and it is why the index reports
    /// [`crate::KeyframeIndex::skipped_boundaries`] rather than pretending.
    #[must_use]
    pub const fn with_ground_scan(mut self, bytes: usize) -> Self {
        self.ground_scan = bytes;
        self
    }

    /// Check the geometry and the stride.
    ///
    /// # Errors
    ///
    /// [`Error::Geometry`] or [`Error::ZeroStride`].
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
        if self.keyframe_stride == 0 {
            return Err(Error::ZeroStride);
        }
        Ok(())
    }
}
