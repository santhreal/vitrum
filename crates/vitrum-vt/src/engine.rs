//! The engine: bytes in, [`CellGrid`] out.

use std::rc::Rc;

use libghostty_vt::render::{CellIterator, Dirty, RowIterator};
use libghostty_vt::style::RgbColor;
use libghostty_vt::terminal::ScrollViewport;
use libghostty_vt::{RenderState, Terminal, TerminalOptions};
use vitrum_grid::CellGrid;
use vitrum_grid::cell::Rgba;
use vitrum_grid::grid::GridError;

use crate::bridge::{CursorShape, CursorState, GRAPHEME_BUF, SyncStats, cell_of, to_rgba};
use crate::events::Events;

/// Why an engine operation failed.
#[derive(Debug)]
pub enum VtError {
    /// The terminal engine refused the operation.
    Engine(libghostty_vt::Error),
    /// The cell grid refused the operation, which in practice means a size the
    /// grid will not represent.
    Grid(GridError),
}

impl core::fmt::Display for VtError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Engine(e) => write!(f, "terminal engine: {e}"),
            Self::Grid(e) => write!(f, "cell grid: {e}"),
        }
    }
}

impl core::error::Error for VtError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Engine(e) => Some(e),
            Self::Grid(e) => Some(e),
        }
    }
}

impl From<libghostty_vt::Error> for VtError {
    fn from(e: libghostty_vt::Error) -> Self {
        Self::Engine(e)
    }
}

impl From<GridError> for VtError {
    fn from(e: GridError) -> Self {
        Self::Grid(e)
    }
}

/// How to build a [`Vt`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VtOptions {
    /// Width in cells.
    pub cols: u16,
    /// Height in cells.
    pub rows: u16,
    /// Scrollback budget in bytes. Zero disables scrollback entirely.
    pub max_scrollback: usize,
}

impl Default for VtOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            // Ghostty's own default order of magnitude. Large enough that a
            // build log survives, small enough that twenty idle sessions do not
            // dominate the client's footprint.
            max_scrollback: 10_000_000,
        }
    }
}

/// A terminal: VT state, scrollback, and the projection onto a cell grid.
///
/// One `Vt` is one session. It is not [`Send`], because libghostty's callbacks
/// run on the thread that calls [`Vt::feed`] and the engine keeps thread-local
/// state, so a session belongs to whichever thread created it.
pub struct Vt {
    term: Terminal<'static, 'static>,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    events: Rc<Events>,
}

impl core::fmt::Debug for Vt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The engine handles are opaque pointers; printing them says nothing.
        // Size is the field a caller ever wants to see in a log line.
        f.debug_struct("Vt")
            .field("cols", &self.term.cols().unwrap_or(0))
            .field("rows", &self.term.rows().unwrap_or(0))
            .finish_non_exhaustive()
    }
}

impl Vt {
    /// Create a terminal of the given size.
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine cannot allocate the screen.
    pub fn new(opts: VtOptions) -> Result<Self, VtError> {
        let events = Rc::new(Events::new());
        let mut term = Terminal::new(TerminalOptions {
            cols: opts.cols,
            rows: opts.rows,
            max_scrollback: opts.max_scrollback,
        })?;

        // Each closure captures its own `Rc` clone, so the callbacks own their
        // sink and borrow nothing from `Self`. That is what keeps `Vt` a plain
        // struct rather than a self-referential one.
        term.on_pty_write({
            let events = Rc::clone(&events);
            move |_t, data| events.push_pty_write(data)
        })?
        .on_bell({
            let events = Rc::clone(&events);
            move |_t| events.push_bell()
        })?
        .on_title_changed({
            let events = Rc::clone(&events);
            move |t| {
                if let Ok(title) = t.title() {
                    events.set_title(title);
                }
            }
        })?
        .on_pwd_changed({
            let events = Rc::clone(&events);
            move |t| {
                if let Ok(pwd) = t.pwd() {
                    events.set_pwd(pwd);
                }
            }
        })?;

        Ok(Self {
            term,
            render: RenderState::new()?,
            rows: RowIterator::new()?,
            cells: CellIterator::new()?,
            events,
        })
    }

    /// Parse `data` as a VT stream and apply it to the terminal state.
    ///
    /// Callbacks fire during this call, so pending events are complete by the
    /// time it returns.
    pub fn feed(&mut self, data: &[u8]) {
        self.term.vt_write(data);
    }

    /// Resize the terminal, reflowing scrollback.
    ///
    /// `cell_px` is the pixel size of one cell, which programs read through
    /// XTWINOPS. It does not affect the grid.
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine rejects the size.
    pub fn resize(&mut self, cols: u16, rows: u16, cell_px: (u32, u32)) -> Result<(), VtError> {
        self.term.resize(cols, rows, cell_px.0, cell_px.1)?;
        Ok(())
    }

    /// Full reset (RIS): screen, scrollback, modes, and styles.
    pub fn reset(&mut self) {
        self.term.reset();
    }

    /// Move the viewport within the scrollback.
    pub fn scroll(&mut self, scroll: ScrollViewport) {
        self.term.scroll_viewport(scroll);
    }

    /// Set the colours the terminal falls back to when a cell has none of its
    /// own. A program can still override these with OSC 10/11/12.
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine rejects a colour.
    pub fn set_theme(
        &mut self,
        fg: Rgba,
        bg: Rgba,
        cursor: Option<Rgba>,
    ) -> Result<(), VtError> {
        let rgb = |c: Rgba| RgbColor {
            r: c.r,
            g: c.g,
            b: c.b,
        };
        self.term
            .set_default_fg_color(Some(rgb(fg)))?
            .set_default_bg_color(Some(rgb(bg)))?
            .set_default_cursor_color(cursor.map(rgb))?;
        Ok(())
    }

    /// Width of the terminal in cells.
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine handle is unreadable.
    pub fn cols(&self) -> Result<u16, VtError> {
        Ok(self.term.cols()?)
    }

    /// Height of the terminal in cells.
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine handle is unreadable.
    pub fn rows(&self) -> Result<u16, VtError> {
        Ok(self.term.rows()?)
    }

    /// Rows held in scrollback, above the viewport.
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine handle is unreadable.
    pub fn scrollback_rows(&self) -> Result<usize, VtError> {
        Ok(self.term.scrollback_rows()?)
    }

    /// True when a program has enabled any mouse reporting mode, which means
    /// the host must forward mouse events instead of using them for selection.
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine handle is unreadable.
    pub fn mouse_tracking(&self) -> Result<bool, VtError> {
        Ok(self.term.is_mouse_tracking()?)
    }

    /// The event sink, for hosts that want to inspect it directly.
    #[must_use]
    pub fn events(&self) -> &Events {
        &self.events
    }

    /// Move every byte the terminal owes the PTY onto `out`.
    ///
    /// A host that never calls this hangs any program that issues a device
    /// query, because the program is blocked reading an answer that is sitting
    /// in this buffer.
    pub fn drain_pty_write(&self, out: &mut Vec<u8>) {
        self.events.drain_pty_write(out);
    }

    /// Project the current screen onto `grid`.
    ///
    /// The grid is resized to match the terminal when they disagree, so a
    /// caller may hand in a grid of any size and get a correct frame.
    ///
    /// Rows the terminal reports as unchanged are not read at all, and rows
    /// that are read still only damage the cells whose value actually differs.
    /// An idle terminal therefore produces [`SyncStats::is_noop`].
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine handle is unreadable, or
    /// [`VtError::Grid`] when the terminal size is one the grid refuses.
    pub fn sync(&mut self, grid: &mut CellGrid) -> Result<SyncStats, VtError> {
        let mut stats = SyncStats::default();

        let snapshot = self.render.update(&self.term)?;
        let dirty = snapshot.dirty()?;
        let cols = snapshot.cols()?;
        let rows = snapshot.rows()?;
        let colors = snapshot.colors()?;

        // A resize invalidates every cell the renderer holds, so the size check
        // has to happen before the dirty check: the terminal can consider a row
        // unchanged while the grid has never seen it.
        let resized = grid.cols() != cols || grid.rows() != rows;
        if resized {
            grid.resize(cols, rows)?;
            stats.resized = true;
        }

        if dirty == Dirty::Clean && !resized {
            stats.rows_skipped = rows;
            return Ok(stats);
        }

        // A partially dirty frame is the common case: one row of shell output
        // changed and the other forty-nine did not. A full frame ignores the
        // per-row flags because global state (a palette change, a screen swap)
        // affects rows that never reported themselves dirty.
        let per_row = dirty == Dirty::Partial && !resized;

        let mut buf = ['\0'; GRAPHEME_BUF];
        let mut row_iter = self.rows.update(&snapshot)?;
        let mut y: u16 = 0;

        while let Some(row) = row_iter.next() {
            if y >= rows {
                break;
            }
            if per_row && !row.dirty()? {
                stats.rows_skipped += 1;
                y += 1;
                continue;
            }

            let mut cell_iter = self.cells.update(row)?;
            let mut x: u16 = 0;
            while let Some(cell) = cell_iter.next() {
                if x >= cols {
                    break;
                }

                let len = cell.graphemes_len()?;
                let ch = if len == 0 {
                    None
                } else {
                    let take = len.min(GRAPHEME_BUF);
                    cell.graphemes_buf(&mut buf[..take])?;
                    if len > 1 {
                        stats.graphemes_flattened += 1;
                    }
                    Some(buf[0])
                };

                let raw = cell.raw_cell()?;
                let projected = cell_of(
                    ch,
                    cell.fg_color()?,
                    cell.bg_color()?,
                    &cell.style()?,
                    raw.wide()?,
                    &colors,
                );

                if grid.set_cell(x, y, projected)? {
                    stats.cells_changed += 1;
                }
                x += 1;
            }

            row.set_dirty(false)?;
            stats.rows_synced += 1;
            y += 1;
        }

        // Both dirty layers are independent, and clearing one does not clear
        // the other. The per-row flags were cleared above as each row was read.
        snapshot.set_dirty(Dirty::Clean)?;

        Ok(stats)
    }

    /// Where the cursor is and how to draw it.
    ///
    /// Read after [`Vt::sync`] so the renderer draws the cursor on the frame it
    /// belongs to.
    ///
    /// # Errors
    ///
    /// [`VtError::Engine`] when the engine handle is unreadable.
    pub fn cursor(&mut self) -> Result<CursorState, VtError> {
        use libghostty_vt::render::CursorVisualStyle;

        let snapshot = self.render.update(&self.term)?;
        let colors = snapshot.colors()?;
        let visible = snapshot.cursor_visible()?;
        let viewport = snapshot.cursor_viewport()?;
        let shape = match snapshot.cursor_visual_style()? {
            CursorVisualStyle::Block => CursorShape::Block,
            CursorVisualStyle::BlockHollow => CursorShape::HollowBlock,
            CursorVisualStyle::Bar => CursorShape::Bar,
            CursorVisualStyle::Underline => CursorShape::Underline,
            // The enum is non-exhaustive upstream. A shape this build does not
            // know is drawn as a block, because an unknown shape must still
            // show the user where the cursor is.
            _ => CursorShape::Block,
        };

        // Reading the cursor must not consume the frame's dirty state: `sync`
        // owns that, and a caller is free to call these in either order.
        let (col, row, at_wide_tail) = viewport.map_or((0, 0, false), |c| (c.x, c.y, c.at_wide_tail));

        Ok(CursorState {
            col,
            row,
            // A cursor outside the viewport (the user scrolled up) is not drawn.
            visible: visible && viewport.is_some(),
            at_wide_tail,
            color: to_rgba(colors.cursor.unwrap_or(colors.foreground)),
            shape,
        })
    }
}
