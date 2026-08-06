//! The reconstructed terminal state, and the operations VT commands perform on
//! it.
//!
//! A [`Screen`] owns a [`vitrum_grid::CellGrid`] and everything a grid
//! deliberately does not model: where the cursor is, which rows scroll, which
//! modes are on, where the tab stops are, and which character set is mapped.
//! [`crate::perform`] decodes VT bytes into calls on this type and does nothing
//! else, so the rules of the terminal live here and the syntax of the terminal
//! lives there.
//!
//! # Equality
//!
//! [`Screen`] compares by *state*, not by bookkeeping. Two screens are equal when
//! every cell, the cursor, the modes, the tab stops, the charsets and the title
//! match. The grid's damage spans are excluded on purpose: damage records which
//! cells a renderer still has to upload, and a screen restored from a keyframe
//! has a different upload history from one fed linearly while describing the
//! identical terminal. Comparing damage would make the seek-equivalence test
//! assert something no user can see.
//!
//! # Scrollback
//!
//! There is none. A screen is `rows` tall and a row that scrolls off the top is
//! gone, exactly as in a terminal with no scrollback. Nothing is lost, because
//! the session's scrollback is the byte stream being replayed; it just is not
//! stored twice.

use vitrum_grid::{Attrs, Cell, CellGrid, CellSlot, CharWidth, Region, Rgba, Style, char_width};

use crate::error::{Error, Result};
use crate::palette::Palette;

/// Where the cursor is, and whether it is waiting to wrap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cursor {
    /// Zero-based column.
    pub col: u16,
    /// Zero-based row.
    pub row: u16,
    /// The last printed character filled the final column, so the *next*
    /// printable character wraps.
    ///
    /// This deferred wrap is not a detail. A terminal that wraps eagerly, the
    /// instant the last column is filled, puts the cursor on the next row before
    /// it knows whether anything follows, and then a bare `\r` returns to the
    /// wrong line. Every real terminal defers, so replay must too.
    pub pending_wrap: bool,
    /// Whether DECTCEM (`CSI ? 25 h`) has the cursor shown.
    pub visible: bool,
}

impl Cursor {
    /// Home, visible, no pending wrap.
    pub const HOME: Self = Self {
        col: 0,
        row: 0,
        pending_wrap: false,
        visible: true,
    };
}

/// What DECSC (`ESC 7`) stores and DECRC (`ESC 8`) puts back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SavedCursor {
    /// Saved column.
    pub col: u16,
    /// Saved row.
    pub row: u16,
    /// Saved graphic rendition.
    pub pen: Style,
    /// Saved charset mapping.
    pub charsets: Charsets,
    /// Saved origin mode.
    pub origin: bool,
}

impl SavedCursor {
    /// The state DECRC restores when nothing was ever saved: home, defaults.
    #[must_use]
    pub fn initial(palette: &Palette) -> Self {
        Self {
            col: 0,
            row: 0,
            pen: palette.default_style(),
            charsets: Charsets::ASCII,
            origin: false,
        }
    }
}

/// The rows DECSTBM (`CSI top ; bottom r`) confines scrolling to, inclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScrollRegion {
    /// First scrolling row.
    pub top: u16,
    /// Last scrolling row.
    pub bottom: u16,
}

impl ScrollRegion {
    /// Height in rows.
    #[must_use]
    pub const fn height(&self) -> u16 {
        self.bottom - self.top + 1
    }
}

/// Which character set a byte in `0x20..=0x7e` is read through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Charset {
    /// Plain ASCII: every character means itself.
    Ascii,
    /// DEC Special Graphics: `q` is a horizontal rule, `x` a vertical one, and so
    /// on. Still emitted by ncurses and by anything drawing a box the portable
    /// way, so a replay that ignored it would show `lqqqk` where the session
    /// showed a box corner.
    DecSpecialGraphics,
}

impl Charset {
    /// The glyph this charset gives `ch`.
    #[must_use]
    pub const fn map(self, ch: char) -> char {
        match self {
            Self::Ascii => ch,
            Self::DecSpecialGraphics => {
                let code = ch as u32;
                if code >= 0x5f && code <= 0x7e {
                    DEC_SPECIAL_GRAPHICS[(code - 0x5f) as usize]
                } else {
                    ch
                }
            }
        }
    }
}

/// `0x5f` (`_`) through `0x7e` (`~`) under DEC Special Graphics.
const DEC_SPECIAL_GRAPHICS: [char; 32] = [
    ' ', '\u{25c6}', '\u{2592}', '\u{2409}', '\u{240c}', '\u{240d}', '\u{240a}', '\u{00b0}',
    '\u{00b1}', '\u{2424}', '\u{240b}', '\u{2518}', '\u{2510}', '\u{250c}', '\u{2514}', '\u{253c}',
    '\u{23ba}', '\u{23bb}', '\u{2500}', '\u{23bc}', '\u{23bd}', '\u{251c}', '\u{2524}', '\u{2534}',
    '\u{252c}', '\u{2502}', '\u{2264}', '\u{2265}', '\u{03c0}', '\u{2260}', '\u{00a3}', '\u{00b7}',
];

/// The two designated charsets and which one is currently shifted in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Charsets {
    /// The set `ESC ( x` designates, active after SI (`0x0f`).
    pub g0: Charset,
    /// The set `ESC ) x` designates, active after SO (`0x0e`).
    pub g1: Charset,
    /// `false` for G0, `true` for G1.
    pub shifted: bool,
}

impl Charsets {
    /// Both slots ASCII, G0 shifted in: what a terminal starts with and what RIS
    /// puts back.
    pub const ASCII: Self = Self {
        g0: Charset::Ascii,
        g1: Charset::Ascii,
        shifted: false,
    };

    /// The charset currently in effect.
    #[must_use]
    pub const fn active(&self) -> Charset {
        if self.shifted { self.g1 } else { self.g0 }
    }
}

/// The mode flags that change what printing does.
///
/// Modes that only change what the terminal *sends* (application cursor keys,
/// bracketed paste, mouse reporting) are not here: replay has no input channel,
/// so they cannot affect a reconstructed screen and tracking them would be state
/// nobody can observe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Modes {
    /// DECAWM (`CSI ? 7 h`): printing past the last column wraps.
    pub autowrap: bool,
    /// DECOM (`CSI ? 6 h`): cursor addressing is relative to the scroll region.
    pub origin: bool,
    /// IRM (`CSI 4 h`): printing shifts the rest of the row right.
    pub insert: bool,
}

impl Modes {
    /// Autowrap on, origin off, insert off: the power-on state.
    pub const DEFAULT: Self = Self {
        autowrap: true,
        origin: false,
        insert: false,
    };
}

/// Columns where a tab stops.
///
/// A bitset of [`vitrum_grid::grid::MAX_COLS`] bits, so it is 256 bytes, `Copy`,
/// and adds a fixed cost to a keyframe rather than a heap allocation per
/// snapshot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TabStops {
    words: [u64; 32],
}

impl TabStops {
    /// Stops every `step` columns from column 0, which is how a terminal powers
    /// on with `step` of 8.
    ///
    /// A `step` of 0 is read as 8 rather than looping forever.
    #[must_use]
    pub const fn every(step: u16) -> Self {
        let step = if step == 0 { 8 } else { step };
        let mut words = [0u64; 32];
        let mut col = 0u16;
        while (col as usize) < 32 * 64 {
            words[(col / 64) as usize] |= 1u64 << (col % 64);
            col += step;
        }
        Self { words }
    }

    /// Is there a stop at `col`?
    #[must_use]
    pub const fn is_stop(&self, col: u16) -> bool {
        if (col as usize) >= 32 * 64 {
            return false;
        }
        self.words[(col / 64) as usize] & (1u64 << (col % 64)) != 0
    }

    /// Put a stop at `col`.
    pub const fn set(&mut self, col: u16) {
        if (col as usize) < 32 * 64 {
            self.words[(col / 64) as usize] |= 1u64 << (col % 64);
        }
    }

    /// Remove the stop at `col`.
    pub const fn clear(&mut self, col: u16) {
        if (col as usize) < 32 * 64 {
            self.words[(col / 64) as usize] &= !(1u64 << (col % 64));
        }
    }

    /// Remove every stop, which is what `CSI 3 g` does.
    pub const fn clear_all(&mut self) {
        self.words = [0u64; 32];
    }

    /// First stop after `col`, or the last column when there is none.
    ///
    /// A tab never leaves the row, which is why the fallback is the last column
    /// rather than a wrap.
    #[must_use]
    pub const fn next_after(&self, col: u16, cols: u16) -> u16 {
        let mut probe = col + 1;
        while probe < cols {
            if self.is_stop(probe) {
                return probe;
            }
            probe += 1;
        }
        cols - 1
    }

    /// Last stop before `col`, or column 0 when there is none.
    #[must_use]
    pub const fn previous_before(&self, col: u16) -> u16 {
        let mut probe = col;
        while probe > 0 {
            probe -= 1;
            if self.is_stop(probe) {
                return probe;
            }
        }
        0
    }
}

impl Default for TabStops {
    fn default() -> Self {
        Self::every(8)
    }
}

/// The full reconstructed terminal state at one point in a session's stream.
#[derive(Clone, Debug)]
pub struct Screen {
    grid: CellGrid,
    /// The buffer parked while the other one is active. `Some` only once the
    /// session has actually switched to the alternate screen, so a session that
    /// never runs a full-screen program pays for one grid, not two.
    inactive: Option<CellGrid>,
    on_alt: bool,
    pen: Style,
    cursor: Cursor,
    saved: SavedCursor,
    /// Where `CSI ? 1049 h` stashed the primary buffer's cursor.
    saved_primary: SavedCursor,
    region: ScrollRegion,
    modes: Modes,
    tabs: TabStops,
    charsets: Charsets,
    title: String,
    palette: Palette,
    /// Set by [`crate::perform`] on any dispatch that leaves the VT parser in its
    /// ground state. Read and cleared by [`crate::Emulator::feed_byte`]; not part
    /// of terminal state and excluded from equality.
    pub(crate) ground: bool,
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
            inactive: None,
            on_alt: false,
            pen: palette.default_style(),
            cursor: Cursor::HOME,
            saved: SavedCursor::initial(&palette),
            saved_primary: SavedCursor::initial(&palette),
            region: ScrollRegion {
                top: 0,
                bottom: rows - 1,
            },
            modes: Modes::DEFAULT,
            tabs: TabStops::every(8),
            charsets: Charsets::ASCII,
            title: String::new(),
            palette,
            ground: false,
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

    /// The rendition the next printed character will use.
    #[must_use]
    pub const fn pen(&self) -> Style {
        self.pen
    }

    /// The active mode flags.
    #[must_use]
    pub const fn modes(&self) -> Modes {
        self.modes
    }

    /// The scrolling region.
    #[must_use]
    pub const fn region(&self) -> ScrollRegion {
        self.region
    }

    /// The designated charsets.
    #[must_use]
    pub const fn charsets(&self) -> Charsets {
        self.charsets
    }

    /// The tab stops.
    #[must_use]
    pub const fn tabs(&self) -> &TabStops {
        &self.tabs
    }

    /// The colours indexed rendition resolves through.
    #[must_use]
    pub const fn palette(&self) -> &Palette {
        &self.palette
    }

    /// The last title the session set with OSC 0 or OSC 2, or empty.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Is a full-screen program's alternate buffer showing?
    #[must_use]
    pub const fn on_alt_screen(&self) -> bool {
        self.on_alt
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
    /// This is what one keyframe costs, so a caller sizing an index multiplies it
    /// by the number of keyframes and gets the real number.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        let grid = |g: &CellGrid| {
            g.len() * core::mem::size_of::<Cell>() + g.rows() as usize * core::mem::size_of::<u32>()
        };
        grid(&self.grid) + self.inactive.as_ref().map_or(0, grid) + self.title.capacity()
    }

    /// Resize the screen, anchoring content at the top left.
    ///
    /// The cursor and the scroll region are clamped into the new size. No reflow:
    /// see [`CellGrid::resize`].
    ///
    /// # Errors
    ///
    /// [`Error::Geometry`] when the size is not one [`CellGrid`] accepts.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.grid
            .resize(cols, rows)
            .map_err(|_| Error::Geometry { cols, rows })?;
        if let Some(other) = self.inactive.as_mut() {
            other
                .resize(cols, rows)
                .map_err(|_| Error::Geometry { cols, rows })?;
        }
        self.region = ScrollRegion {
            top: self.region.top.min(rows - 1),
            bottom: self.region.bottom.min(rows - 1),
        };
        if self.region.top > self.region.bottom {
            self.region = ScrollRegion {
                top: 0,
                bottom: rows - 1,
            };
        }
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.cursor.pending_wrap = false;
        Ok(())
    }

    // ---- state mutation, driven by `crate::perform` ----

    /// The cell an erase writes.
    ///
    /// Back-colour erase: the current background is kept, because a program that
    /// paints a coloured panel and then clears part of it expects the panel
    /// colour, not black. Foreground and the rendition bits are *not* kept, so an
    /// erase under an active underline does not draw a rule across empty space.
    pub(crate) fn erase_cell(&self) -> Cell {
        Cell::blank(Style {
            fg: self.palette.fg,
            bg: self.pen.bg,
            attrs: Attrs::NONE,
        })
    }

    pub(crate) const fn pen_mut(&mut self) -> &mut Style {
        &mut self.pen
    }

    pub(crate) const fn charsets_mut(&mut self) -> &mut Charsets {
        &mut self.charsets
    }

    pub(crate) const fn modes_mut(&mut self) -> &mut Modes {
        &mut self.modes
    }

    pub(crate) const fn tabs_mut(&mut self) -> &mut TabStops {
        &mut self.tabs
    }

    pub(crate) const fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    pub(crate) fn set_title(&mut self, title: &str) {
        self.title.clear();
        self.title.push_str(title);
    }

    /// Print one character at the cursor, wrapping and advancing.
    ///
    /// Named `print_char` rather than `print` so it cannot be confused with, or
    /// accidentally shadow, [`vte::Perform::print`] in [`crate::perform`].
    pub(crate) fn print_char(&mut self, ch: char) {
        let ch = self.charsets.active().map(ch);
        let width = match char_width(ch) {
            // A control reaching `print` means the parser decoded it as text,
            // which only happens for a C1 byte in a non-UTF-8 stream. Nothing to
            // draw and nothing to move.
            CharWidth::Control => return,
            // The grid stores one `char` per cell and cannot compose, so a
            // combining mark has nowhere to go. Dropping it leaves the base
            // character standing, which is the closest a non-composing grid gets.
            CharWidth::ZeroWidth => return,
            CharWidth::Narrow => 1u16,
            CharWidth::Wide => 2u16,
        };
        let cols = self.cols();

        if self.cursor.pending_wrap {
            self.wrap_now();
        }
        // A double-width character with one column left cannot be split. A real
        // terminal blanks the last column and puts the character on the next row.
        if width == 2 && self.cursor.col + 1 >= cols {
            if !self.modes.autowrap {
                return;
            }
            let cell = self.erase_cell();
            let _ = self.grid.set_cell(cols - 1, self.cursor.row, cell);
            self.wrap_now();
        }

        if self.modes.insert {
            self.insert_chars(width);
        }
        if self.grid
            .write_char(self.cursor.col, self.cursor.row, ch, self.pen)
            .is_err()
        {
            return;
        }
        if self.cursor.col + width >= cols {
            self.cursor.col = cols - 1;
            self.cursor.pending_wrap = self.modes.autowrap;
        } else {
            self.cursor.col += width;
        }
    }

    /// Take the deferred wrap: down one row, back to column 0.
    fn wrap_now(&mut self) {
        self.cursor.pending_wrap = false;
        self.cursor.col = 0;
        self.line_feed();
    }

    /// Down one row, scrolling the region when already at its bottom.
    pub(crate) fn line_feed(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.region.bottom {
            self.scroll_region_up(1);
        } else if self.cursor.row + 1 < self.rows() {
            self.cursor.row += 1;
        }
    }

    /// Up one row, scrolling the region down when already at its top.
    pub(crate) fn reverse_index(&mut self) {
        self.cursor.pending_wrap = false;
        if self.cursor.row == self.region.top {
            self.scroll_region_down(1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }

    pub(crate) fn carriage_return(&mut self) {
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn backspace(&mut self) {
        self.cursor.pending_wrap = false;
        self.cursor.col = self.cursor.col.saturating_sub(1);
    }

    pub(crate) fn tab_forward(&mut self, count: u16) {
        self.cursor.pending_wrap = false;
        let cols = self.cols();
        for _ in 0..count.max(1) {
            self.cursor.col = self.tabs.next_after(self.cursor.col, cols);
        }
    }

    pub(crate) fn tab_backward(&mut self, count: u16) {
        self.cursor.pending_wrap = false;
        for _ in 0..count.max(1) {
            self.cursor.col = self.tabs.previous_before(self.cursor.col);
        }
    }

    /// Absolute cursor addressing, honouring origin mode.
    pub(crate) fn move_to(&mut self, col: u16, row: u16) {
        let (row_base, row_limit) = if self.modes.origin {
            (self.region.top, self.region.bottom)
        } else {
            (0, self.rows() - 1)
        };
        self.cursor.row = row.saturating_add(row_base).min(row_limit);
        self.cursor.col = col.min(self.cols() - 1);
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn move_col(&mut self, col: u16) {
        self.cursor.col = col.min(self.cols() - 1);
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn move_row(&mut self, row: u16) {
        let (row_base, row_limit) = if self.modes.origin {
            (self.region.top, self.region.bottom)
        } else {
            (0, self.rows() - 1)
        };
        self.cursor.row = row.saturating_add(row_base).min(row_limit);
        self.cursor.pending_wrap = false;
    }

    /// Relative cursor movement, clamped to the screen.
    ///
    /// Vertical movement is additionally clamped to the scrolling region when the
    /// cursor starts inside it, so `CUU` cannot walk a full-screen program's
    /// cursor out of its own pane.
    pub(crate) fn move_by(&mut self, dcol: i32, drow: i32) {
        self.cursor.pending_wrap = false;
        let col = i32::from(self.cursor.col) + dcol;
        self.cursor.col = col.clamp(0, i32::from(self.cols()) - 1) as u16;

        let inside = self.cursor.row >= self.region.top && self.cursor.row <= self.region.bottom;
        let (lo, hi) = if inside {
            (i32::from(self.region.top), i32::from(self.region.bottom))
        } else {
            (0, i32::from(self.rows()) - 1)
        };
        let row = i32::from(self.cursor.row) + drow;
        self.cursor.row = row.clamp(lo, hi) as u16;
    }

    pub(crate) fn set_region(&mut self, top: u16, bottom: u16) {
        let last = self.rows() - 1;
        let top = top.min(last);
        let bottom = bottom.min(last);
        if top < bottom {
            self.region = ScrollRegion { top, bottom };
        } else {
            self.region = ScrollRegion { top: 0, bottom: last };
        }
        // DECSTBM homes the cursor, and with origin mode on "home" is the top of
        // the new region rather than the top of the screen.
        self.move_to(0, 0);
    }

    pub(crate) fn scroll_region_up(&mut self, count: u16) {
        let cell = self.erase_cell();
        let _ = self
            .grid
            .scroll_up(self.region.top, self.region.bottom, count, cell);
    }

    pub(crate) fn scroll_region_down(&mut self, count: u16) {
        let cell = self.erase_cell();
        let _ = self
            .grid
            .scroll_down(self.region.top, self.region.bottom, count, cell);
    }

    /// Insert `count` blank rows at the cursor, pushing the rest of the region
    /// down. Outside the region this does nothing, which is what xterm does.
    pub(crate) fn insert_lines(&mut self, count: u16) {
        if self.cursor.row < self.region.top || self.cursor.row > self.region.bottom {
            return;
        }
        let cell = self.erase_cell();
        let _ = self
            .grid
            .scroll_down(self.cursor.row, self.region.bottom, count, cell);
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
    }

    /// Delete `count` rows at the cursor, pulling the rest of the region up.
    pub(crate) fn delete_lines(&mut self, count: u16) {
        if self.cursor.row < self.region.top || self.cursor.row > self.region.bottom {
            return;
        }
        let cell = self.erase_cell();
        let _ = self
            .grid
            .scroll_up(self.cursor.row, self.region.bottom, count, cell);
        self.cursor.col = 0;
        self.cursor.pending_wrap = false;
    }

    /// Shift the row right from the cursor by `count`, dropping what falls off.
    pub(crate) fn insert_chars(&mut self, count: u16) {
        let cols = self.cols();
        let row = self.cursor.row;
        let at = self.cursor.col;
        let count = count.max(1).min(cols - at);
        let cell = self.erase_cell();
        let mut col = cols;
        while col > at + count {
            col -= 1;
            let source = self.cell_at(col - count, row);
            let _ = self.grid.set_cell(col, row, source);
        }
        for col in at..at + count {
            let _ = self.grid.set_cell(col, row, cell);
        }
        self.cursor.pending_wrap = false;
    }

    /// Pull the row left onto the cursor by `count`, blanking the tail.
    pub(crate) fn delete_chars(&mut self, count: u16) {
        let cols = self.cols();
        let row = self.cursor.row;
        let at = self.cursor.col;
        let count = count.max(1).min(cols - at);
        let cell = self.erase_cell();
        for col in at..cols - count {
            let source = self.cell_at(col + count, row);
            let _ = self.grid.set_cell(col, row, source);
        }
        for col in cols - count..cols {
            let _ = self.grid.set_cell(col, row, cell);
        }
        self.cursor.pending_wrap = false;
    }

    /// Blank `count` cells from the cursor without moving it.
    pub(crate) fn erase_chars(&mut self, count: u16) {
        let cols = self.cols();
        let count = count.max(1).min(cols - self.cursor.col);
        let cell = self.erase_cell();
        self.grid.fill(
            Region {
                col: self.cursor.col,
                row: self.cursor.row,
                cols: count,
                rows: 1,
            },
            cell,
        );
    }

    /// `CSI J`: 0 cursor to end, 1 start to cursor, 2 everything.
    pub(crate) fn erase_display(&mut self, mode: u16) {
        let cell = self.erase_cell();
        let cols = self.cols();
        let rows = self.rows();
        let (row, col) = (self.cursor.row, self.cursor.col);
        match mode {
            0 => {
                self.grid.fill(
                    Region {
                        col,
                        row,
                        cols: cols - col,
                        rows: 1,
                    },
                    cell,
                );
                if row + 1 < rows {
                    self.grid.fill(
                        Region {
                            col: 0,
                            row: row + 1,
                            cols,
                            rows: rows - row - 1,
                        },
                        cell,
                    );
                }
            }
            1 => {
                if row > 0 {
                    self.grid.fill(
                        Region {
                            col: 0,
                            row: 0,
                            cols,
                            rows: row,
                        },
                        cell,
                    );
                }
                self.grid.fill(
                    Region {
                        col: 0,
                        row,
                        cols: col + 1,
                        rows: 1,
                    },
                    cell,
                );
            }
            2 => {
                self.grid.fill(
                    Region {
                        col: 0,
                        row: 0,
                        cols,
                        rows,
                    },
                    cell,
                );
            }
            // 3 erases the scrollback and leaves the screen alone. This screen
            // has no scrollback of its own, so there is nothing to erase, and
            // clearing the visible rows here would be a bug a user would see as
            // their output vanishing.
            _ => {}
        }
        self.cursor.pending_wrap = false;
    }

    /// `CSI K`: 0 cursor to end of line, 1 start of line to cursor, 2 the line.
    pub(crate) fn erase_line(&mut self, mode: u16) {
        let cell = self.erase_cell();
        let cols = self.cols();
        let (row, col) = (self.cursor.row, self.cursor.col);
        let (from, count) = match mode {
            0 => (col, cols - col),
            1 => (0, col + 1),
            _ => (0, cols),
        };
        self.grid.fill(
            Region {
                col: from,
                row,
                cols: count,
                rows: 1,
            },
            cell,
        );
        self.cursor.pending_wrap = false;
    }

    pub(crate) fn save_cursor(&mut self) {
        self.saved = SavedCursor {
            col: self.cursor.col,
            row: self.cursor.row,
            pen: self.pen,
            charsets: self.charsets,
            origin: self.modes.origin,
        };
    }

    pub(crate) fn restore_cursor(&mut self) {
        let saved = self.saved;
        self.pen = saved.pen;
        self.charsets = saved.charsets;
        self.modes.origin = saved.origin;
        self.cursor.pending_wrap = false;
        self.cursor.row = saved.row.min(self.rows() - 1);
        self.cursor.col = saved.col.min(self.cols() - 1);
    }

    /// `CSI ? 1049 h` / `l`, and the older 47 and 1047 spellings.
    ///
    /// `save_restore` is true for 1049, which additionally stashes the primary
    /// buffer's cursor on the way in and puts it back on the way out. That is the
    /// whole reason 1049 exists and why a program that uses it does not leave the
    /// shell prompt in the wrong place.
    pub(crate) fn set_alt_screen(&mut self, on: bool, save_restore: bool) {
        if on == self.on_alt {
            return;
        }
        if on {
            if save_restore {
                self.saved_primary = SavedCursor {
                    col: self.cursor.col,
                    row: self.cursor.row,
                    pen: self.pen,
                    charsets: self.charsets,
                    origin: self.modes.origin,
                };
            }
            let mut alt = self.grid.clone();
            let cell = self.erase_cell();
            alt.fill(
                Region {
                    col: 0,
                    row: 0,
                    cols: alt.cols(),
                    rows: alt.rows(),
                },
                cell,
            );
            self.inactive = Some(core::mem::replace(&mut self.grid, alt));
            self.on_alt = true;
            if save_restore {
                self.cursor = Cursor {
                    col: 0,
                    row: 0,
                    pending_wrap: false,
                    visible: self.cursor.visible,
                };
            }
        } else {
            if let Some(primary) = self.inactive.take() {
                self.grid = primary;
                self.grid.mark_all_damaged();
            }
            self.on_alt = false;
            if save_restore {
                let saved = self.saved_primary;
                self.pen = saved.pen;
                self.charsets = saved.charsets;
                self.modes.origin = saved.origin;
                self.cursor = Cursor {
                    col: saved.col.min(self.cols() - 1),
                    row: saved.row.min(self.rows() - 1),
                    pending_wrap: false,
                    visible: self.cursor.visible,
                };
            }
        }
    }

    /// `ESC c`: back to power-on state, keeping only the geometry and the palette.
    pub(crate) fn reset(&mut self) {
        self.set_alt_screen(false, false);
        self.pen = self.palette.default_style();
        self.grid.set_default_style(self.palette.default_style());
        let cell = Cell::blank(self.palette.default_style());
        let cols = self.cols();
        let rows = self.rows();
        self.grid.fill(
            Region {
                col: 0,
                row: 0,
                cols,
                rows,
            },
            cell,
        );
        self.cursor = Cursor::HOME;
        self.saved = SavedCursor::initial(&self.palette);
        self.saved_primary = self.saved;
        self.region = ScrollRegion {
            top: 0,
            bottom: rows - 1,
        };
        self.modes = Modes::DEFAULT;
        self.tabs = TabStops::every(8);
        self.charsets = Charsets::ASCII;
        self.title.clear();
    }

    /// `ESC # 8`: fill the screen with `E`. A test pattern, and the fastest way
    /// to tell whether a replay is addressing cells correctly.
    pub(crate) fn decaln(&mut self) {
        let cell = Cell::new('E', self.pen);
        let cols = self.cols();
        let rows = self.rows();
        self.grid.fill(
            Region {
                col: 0,
                row: 0,
                cols,
                rows,
            },
            cell,
        );
        self.cursor = Cursor {
            col: 0,
            row: 0,
            pending_wrap: false,
            visible: self.cursor.visible,
        };
    }

    fn cell_at(&self, col: u16, row: u16) -> Cell {
        self.grid
            .cell(col, row)
            .unwrap_or_else(|| Cell::blank(self.palette.default_style()))
    }
}

impl PartialEq for Screen {
    /// Compares terminal state and ignores renderer bookkeeping. See the module
    /// header for why damage spans are excluded.
    fn eq(&self, other: &Self) -> bool {
        fn same_grid(a: &CellGrid, b: &CellGrid) -> bool {
            a.cols() == b.cols() && a.rows() == b.rows() && a.cells() == b.cells()
        }
        fn same_inactive(a: &Option<CellGrid>, b: &Option<CellGrid>) -> bool {
            match (a, b) {
                (None, None) => true,
                (Some(a), Some(b)) => same_grid(a, b),
                _ => false,
            }
        }
        same_grid(&self.grid, &other.grid)
            && same_inactive(&self.inactive, &other.inactive)
            && self.on_alt == other.on_alt
            && self.pen == other.pen
            && self.cursor == other.cursor
            && self.saved == other.saved
            && self.saved_primary == other.saved_primary
            && self.region == other.region
            && self.modes == other.modes
            && self.tabs == other.tabs
            && self.charsets == other.charsets
            && self.title == other.title
            && self.palette == other.palette
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
