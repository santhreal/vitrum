//! One session in a pane: the emulator, the grid, and everything painted over
//! them.
//!
//! Toolkit-free on purpose. Nothing here knows what a widget is, which is why
//! the selection, the viewport, the search overlay and the mode reading are
//! all testable against a real libghostty terminal with no display anywhere.
//! The widget half is [`super::host`] and it is small, because everything that
//! could be moved out of it was.
//!
//! # The order a frame is built in
//!
//! 1. Bytes arriving are fed to the emulator and nothing else happens. Feeding
//!    is cheap and syncing is not, so a socket read must not sync: at a 4K grid
//!    the projection is a millisecond and a burst of reads would spend it once
//!    per read for one frame's worth of change.
//! 2. On the frame clock, the overlay is lifted, the emulator is projected onto
//!    the grid, and the overlay is put back.
//!
//! Lifting the overlay first is not bookkeeping. The projection writes only the
//! cells whose value differs from what the grid holds, so a cell the pane
//! recoloured for a selection looks changed to it, and a cell the emulator
//! changed underneath a selection looks unchanged. Restoring the emulator's own
//! value before projecting makes both correct, and it is why the overlay
//! records the cell it replaced rather than a flag.

use vitrum_grid::CellGrid;
use vitrum_grid::cell::{Cell, CellSlot, Cursor, Rgba, Style};
use vitrum_vt::{CursorShape as VtCursorShape, Mode, ScrollViewport, SyncStats, Vt, VtError, VtOptions};

use super::find::{Find, RowHit};
use super::mouse::{ModeFlags, Modes};
use super::scroll::Viewport;
use super::select::{Mode as SelectMode, Point, Selection};
use super::theme::{CursorShape, PaneTheme};

/// Rows a scrollback search will walk before giving up.
///
/// A search is a keystroke in the find bar, so it happens on the thread that
/// draws. A hundred thousand rows at a 4K grid is a hundred million cells and
/// several seconds, which is a frozen window. The bound is generous against
/// any real session and firm against a runaway one, and a search that hits it
/// says so rather than silently returning half an answer.
pub(crate) const MAX_SEARCH_ROWS: usize = 200_000;

/// Everything one session needs to be on screen.
pub(crate) struct PaneSession {
    vt: Vt,
    grid: CellGrid,
    /// A second grid the scrollback search projects pages onto, so a search
    /// never disturbs the frame being drawn. Allocated on the first search and
    /// kept, because a search is a keystroke and reallocating per keystroke is
    /// the cost this avoids.
    scratch: Option<CellGrid>,
    theme: PaneTheme,
    viewport: Viewport,
    selection: Option<Selection>,
    find: Option<Find>,
    /// The find as the operator has typed it, while the pane's find is open.
    ///
    /// Held apart from the compiled find because an empty pattern and a regex
    /// that does not compile are both things a person types on the way to one
    /// that does. Closing the find under them would lose the characters they
    /// already got right.
    find_input: Option<String>,
    /// Cells the pane recoloured, and what the emulator had put there.
    overlay: Vec<(u16, u16, Cell)>,
    /// One cell in pixels, which the emulator needs for pixel-coordinate
    /// mouse reports and for a resize.
    cell_px: (u32, u32),
    /// The search hit the viewport is on, so it can be painted differently
    /// from the rest.
    current_hit: Option<RowHit>,
    /// Text an input method has not committed yet, drawn at the cursor and
    /// never sent to the child.
    preedit: String,
}

impl core::fmt::Debug for PaneSession {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PaneSession")
            .field("cols", &self.grid.cols())
            .field("rows", &self.grid.rows())
            .field("viewport", &self.viewport)
            .field("selecting", &self.selection.is_some())
            .finish()
    }
}

impl PaneSession {
    /// Build a session of `cols` by `rows` cells.
    ///
    /// # Errors
    ///
    /// The emulator refused the size, or the grid did.
    pub(crate) fn new(
        cols: u16,
        rows: u16,
        cell_px: (u32, u32),
        theme: PaneTheme,
    ) -> Result<Self, VtError> {
        let theme = theme.clamped();
        let mut vt = Vt::new(VtOptions {
            cols,
            rows,
            ..VtOptions::default()
        })?;
        vt.resize(cols, rows, cell_px)?;
        // The grid is constructed with the operator's colours rather than
        // constructed and then recoloured. A grid built with the default style
        // and corrected afterwards paints one frame in the wrong colour, which
        // is a flash on every window open.
        let grid = CellGrid::new(cols, rows, theme.default_style())
            .map_err(VtError::from)?;

        let mut session = Self {
            vt,
            grid,
            scratch: None,
            theme,
            viewport: Viewport::new(rows, 0),
            selection: None,
            find: None,
            find_input: None,
            overlay: Vec::new(),
            cell_px,
            current_hit: None,
            preedit: String::new(),
        };
        session.push_theme()?;
        Ok(session)
    }

    /// The grid a renderer paints.
    pub(crate) const fn grid(&self) -> &CellGrid {
        &self.grid
    }

    /// The grid, mutably, for the renderer's own damage bookkeeping.
    pub(crate) const fn grid_mut(&mut self) -> &mut CellGrid {
        &mut self.grid
    }

    /// The colours and type in force.
    pub(crate) const fn theme(&self) -> &PaneTheme {
        &self.theme
    }

    /// Where the viewport is.
    pub(crate) const fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Hand bytes to the emulator. Nothing is projected and nothing is drawn.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        self.vt.feed(bytes);
    }

    /// Show, or stop showing, an input method's uncommitted text.
    ///
    /// Returns whether anything changed, so a host does not mark a frame for
    /// a composition that is where it was.
    pub(crate) fn set_preedit(&mut self, text: &str) -> bool {
        if self.preedit == text {
            return false;
        }
        self.preedit.clear();
        self.preedit.push_str(text);
        self.repaint_overlay();
        true
    }

    /// Discard the session's whole state, which is what a backfill replay
    /// starts with.
    pub(crate) fn reset(&mut self) {
        self.vt.reset();
        self.selection = None;
        self.overlay.clear();
        self.find = None;
        self.find_input = None;
        self.current_hit = None;
        self.viewport = Viewport::new(self.grid.rows(), 0);
        self.grid.mark_all_damaged();
    }

    /// Move every byte the emulator owes the child onto `out`.
    ///
    /// A host that never calls this hangs any program that issues a device
    /// query, because the program is blocked reading an answer sitting in this
    /// buffer.
    pub(crate) fn drain_pty_write(&self, out: &mut Vec<u8>) {
        self.vt.drain_pty_write(out);
    }

    /// Project the emulator onto the grid and repaint the pane's own overlay.
    ///
    /// # Errors
    ///
    /// The emulator handle was unreadable, or the grid refused a size.
    pub(crate) fn sync(&mut self) -> Result<SyncStats, VtError> {
        self.lift_overlay();
        let stats = self.vt.sync(&mut self.grid)?;

        let history = self.vt.scrollback_rows()?;
        if history >= self.viewport.history() {
            self.viewport.history_grew_to(history);
        } else {
            self.viewport.history_shrank_to(history);
        }
        self.viewport.set_rows(self.grid.rows());

        let cursor = self.vt.cursor()?;
        // A cursor is drawn only on the live view. Scrolled back, the cell it
        // would sit on holds text from an hour ago and a caret there says the
        // operator is typing into it.
        let want = (cursor.visible && self.viewport.is_live()).then(|| Cursor {
            col: cursor.col.min(self.grid.cols().saturating_sub(1)),
            row: cursor.row.min(self.grid.rows().saturating_sub(1)),
            shape: grid_shape(cursor.shape),
            color: cursor.color,
        });
        self.grid.set_cursor(want).map_err(VtError::from)?;

        self.lay_overlay();
        Ok(stats)
    }

    /// Follow the widget to a new cell count.
    ///
    /// # Errors
    ///
    /// The emulator refused the size.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16, cell_px: (u32, u32)) -> Result<(), VtError> {
        if (cols, rows) == (self.grid.cols(), self.grid.rows()) && cell_px == self.cell_px {
            return Ok(());
        }
        self.cell_px = cell_px;
        // The overlay's coordinates are about to mean something else, and a
        // selection that survives a reflow selects text nobody picked.
        self.lift_overlay();
        self.selection = None;
        self.current_hit = None;
        self.vt.resize(cols, rows, cell_px)?;
        self.viewport.set_rows(rows);
        self.grid.mark_all_damaged();
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Theme
    // -----------------------------------------------------------------------

    /// Adopt a new theme, in full, while the window is open.
    ///
    /// # Errors
    ///
    /// The emulator refused a colour or a cursor style.
    pub(crate) fn set_theme(&mut self, theme: PaneTheme) -> Result<(), VtError> {
        let theme = theme.clamped();
        if theme == self.theme {
            return Ok(());
        }
        self.theme = theme;
        self.push_theme()?;
        // Every blank cell in the grid still carries the old background, and
        // the renderer's clear colour comes from the default style, so both
        // have to move together or the pane clears to the new colour and
        // paints the old one over it.
        self.grid.set_default_style(self.theme.default_style());
        self.grid.clear();
        self.grid.mark_all_damaged();
        Ok(())
    }

    /// Push the colour half of the theme into the emulator.
    fn push_theme(&mut self) -> Result<(), VtError> {
        let p = &self.theme.palette;
        self.vt
            .set_theme(p.foreground, p.background, Some(p.cursor))?;
        self.vt.set_palette(&self.theme.ansi_overrides())?;
        let shape = match self.theme.cursor_shape {
            CursorShape::Block => VtCursorShape::Block,
            CursorShape::Bar => VtCursorShape::Bar,
            CursorShape::Underline => VtCursorShape::Underline,
        };
        self.vt
            .set_cursor_default(Some(shape), Some(self.theme.cursor_blink))
    }

    // -----------------------------------------------------------------------
    // Modes
    // -----------------------------------------------------------------------

    /// The mode state a pointer event and a paste are functions of.
    ///
    /// Read from the emulator every time rather than cached. A cache would be
    /// wrong for exactly one frame after a program changes a mode, and the
    /// frame after a program enables mouse tracking is the frame the operator
    /// clicks in.
    pub(crate) fn modes(&self) -> Modes {
        let on = |m: Mode| self.vt.mode(m).unwrap_or(false);
        Modes::from_flags(ModeFlags {
            x10_mouse: on(Mode::X10_MOUSE),
            normal_mouse: on(Mode::NORMAL_MOUSE),
            button_mouse: on(Mode::BUTTON_MOUSE),
            any_mouse: on(Mode::ANY_MOUSE),
            utf8_mouse: on(Mode::UTF8_MOUSE),
            sgr_mouse: on(Mode::SGR_MOUSE),
            urxvt_mouse: on(Mode::URXVT_MOUSE),
            sgr_pixels_mouse: on(Mode::SGR_PIXELS_MOUSE),
            alt_scroll: on(Mode::ALT_SCROLL),
            alt_screen: on(Mode::ALT_SCREEN) || on(Mode::ALT_SCREEN_SAVE),
        })
    }

    /// Whether a paste must be bracketed, as the child asked.
    pub(crate) fn bracketed_paste(&self) -> bool {
        self.vt.mode(Mode::BRACKETED_PASTE).unwrap_or(false)
    }

    /// Whether the cursor keys are in their application form.
    pub(crate) fn application_cursor(&self) -> bool {
        self.vt.mode(Mode::DECCKM).unwrap_or(false)
    }

    // -----------------------------------------------------------------------
    // Viewport
    // -----------------------------------------------------------------------

    /// Move the viewport and tell the emulator where it now is.
    ///
    /// Returns whether anything moved, so a caller does not mark a frame for a
    /// wheel notch at the end of the history.
    pub(crate) fn scroll(&mut self, f: impl FnOnce(&mut Viewport)) -> bool {
        let before = self.viewport;
        f(&mut self.viewport);
        if self.viewport == before {
            return false;
        }
        self.vt.scroll(self.viewport.as_scroll());
        self.grid.mark_all_damaged();
        true
    }

    /// Put the viewport back at the live edge, which is what typing does.
    pub(crate) fn scroll_to_bottom(&mut self) -> bool {
        self.scroll(Viewport::to_bottom)
    }

    /// Put the viewport a fixed number of rows back from the live edge.
    ///
    /// What a thumb dragged to a position means. Expressed as a move from
    /// where the viewport is rather than as an assignment, so the one place
    /// that clamps an offset against the history stays [`Viewport::by_lines`].
    pub(crate) fn scroll_to_offset(&mut self, offset: usize) -> bool {
        self.scroll(|v| {
            let now = i64::try_from(v.offset()).unwrap_or(i64::MAX);
            let want = i64::try_from(offset).unwrap_or(i64::MAX);
            v.by_lines(now.saturating_sub(want));
        })
    }

    // -----------------------------------------------------------------------
    // Selection
    // -----------------------------------------------------------------------

    /// Begin a selection at an absolute cell.
    pub(crate) fn select_start(&mut self, at: Point, mode: SelectMode) {
        let cols = self.grid.cols();
        let selection = Selection::start(at, mode, cols, |p| self.word_at(p));
        self.selection = Some(selection);
        self.repaint_overlay();
    }

    /// Extend the live selection.
    pub(crate) fn select_drag(&mut self, to: Point) {
        let Some(mut selection) = self.selection else {
            return;
        };
        selection.drag_to(to, |p| self.word_at(p));
        self.selection = Some(selection);
        self.repaint_overlay();
    }

    /// Drop the selection.
    pub(crate) fn select_clear(&mut self) -> bool {
        if self.selection.take().is_none() {
            return false;
        }
        self.repaint_overlay();
        true
    }

    /// How the live selection grows, or `None` when nothing is selected.
    ///
    /// A host extending a selection has to know what it is extending: a
    /// shift-click after a double click grows the existing word selection by
    /// words, and a pointer sample that cannot change a line selection is a
    /// repaint of every selected span for nothing.
    pub(crate) fn selection_mode(&self) -> Option<SelectMode> {
        self.selection.map(|s| s.mode())
    }

    /// The text the live selection copies, or `None` when nothing is selected.
    pub(crate) fn selection_text(&mut self) -> Option<String> {
        let selection = self.selection?;
        if selection.is_empty() {
            return None;
        }
        // Rows are read one at a time through the same page walk the search
        // uses, because a selection may cover history that is not on screen.
        let rows: Vec<usize> = selection.spans().map(|s| s.row).collect();
        let text = self.rows_text(&rows);
        let out = selection.text(|row| {
            text.get(&row)
                .map(|line| line.chars().collect::<Vec<char>>())
        });
        (!out.is_empty()).then_some(out)
    }

    /// The word around a point, for a double-click.
    fn word_at(&self, at: Point) -> (u16, u16) {
        let Some(row) = self.viewport_row(at.row) else {
            return (at.col, at.col);
        };
        let Some(text) = self.grid.row_text(row) else {
            return (at.col, at.col);
        };
        let chars: Vec<char> = text.chars().collect();
        super::select::word_bounds(&chars, at.col, &self.theme.word_chars)
    }

    /// The grid row an absolute row is showing at, if it is on screen.
    fn viewport_row(&self, absolute: usize) -> Option<u16> {
        let top = self.viewport.top_row();
        let offset = absolute.checked_sub(top)?;
        (offset < usize::from(self.grid.rows())).then(|| offset as u16)
    }

    // -----------------------------------------------------------------------
    // Find
    // -----------------------------------------------------------------------

    /// Start or replace the find, and scan the retained scrollback.
    ///
    /// Returns how many matches there are. `None` means the pattern could not
    /// be compiled, which the find bar reports as text.
    ///
    /// # Errors
    ///
    /// The emulator handle was unreadable during the page walk.
    pub(crate) fn find_start(
        &mut self,
        query: vitrum_search::Query,
    ) -> Result<Option<usize>, VtError> {
        let Ok(mut find) = Find::new(query) else {
            self.find = None;
            self.current_hit = None;
            self.repaint_overlay();
            return Ok(None);
        };
        let corpus = self.whole_scrollback()?;
        find.scan(corpus.iter().map(|(row, text)| (*row, text.as_str())));
        find.seek_from(self.viewport.top_row());
        let count = find.hits().len();
        self.current_hit = find.current();
        self.find = Some(find);
        self.reveal_current_hit();
        Ok(Some(count))
    }

    /// Close the find bar.
    pub(crate) fn find_clear(&mut self) {
        self.find = None;
        self.find_input = None;
        self.current_hit = None;
        self.repaint_overlay();
    }

    /// Whether the pane's find is open.
    pub(crate) const fn find_is_open(&self) -> bool {
        self.find_input.is_some()
    }

    /// The query as it has been typed so far.
    pub(crate) fn find_input(&self) -> Option<&str> {
        self.find_input.as_deref()
    }

    /// Open the find with an empty query.
    pub(crate) fn find_open(&mut self) {
        if self.find_input.is_none() {
            self.find_input = Some(String::new());
            self.repaint_overlay();
        }
    }

    /// Replace the query and rescan the retained scrollback.
    ///
    /// Returns how many matches there are. `None` is an empty pattern or one
    /// that did not compile, and neither closes the find.
    ///
    /// # Errors
    ///
    /// The emulator handle was unreadable during the page walk.
    pub(crate) fn find_type(&mut self, query: &str) -> Result<Option<usize>, VtError> {
        self.find_input = Some(query.to_owned());
        if query.is_empty() {
            self.find = None;
            self.current_hit = None;
            self.repaint_overlay();
            return Ok(None);
        }
        self.find_start(vitrum_search::Query::literal(query))
    }

    /// Step to the next or previous match and bring it on screen.
    pub(crate) fn find_step(&mut self, forward: bool) -> Option<RowHit> {
        let find = self.find.as_mut()?;
        let hit = if forward { find.next() } else { find.previous() };
        self.current_hit = hit;
        self.reveal_current_hit();
        hit
    }

    /// Which match of how many, for the find bar's counter.
    pub(crate) fn find_position(&self) -> Option<(usize, usize)> {
        self.find.as_ref().and_then(Find::position)
    }

    /// Scroll so the current match is on screen with context above it.
    fn reveal_current_hit(&mut self) {
        let Some(hit) = self.current_hit else {
            self.repaint_overlay();
            return;
        };
        self.scroll(|v| v.reveal(hit.row));
        self.repaint_overlay();
    }

    /// Every retained row as text, oldest first.
    ///
    /// The page walk. The emulator holds the history and will only show one
    /// viewport of it at a time, so the viewport is moved across the whole
    /// history and each page projected onto the scratch grid. The live
    /// viewport is put back before returning, and the live grid is never
    /// touched.
    fn whole_scrollback(&mut self) -> Result<Vec<(usize, String)>, VtError> {
        let (cols, rows) = (self.grid.cols(), self.grid.rows());
        let history = self.vt.scrollback_rows()?;
        let total = history + usize::from(rows);
        let mut out = Vec::with_capacity(total.min(MAX_SEARCH_ROWS));

        let scratch = match self.scratch.as_mut() {
            Some(g) => g,
            None => {
                self.scratch = Some(CellGrid::new(cols, rows, Style::DEFAULT).map_err(VtError::from)?);
                self.scratch.as_mut().expect("just inserted")
            }
        };

        // The absolute row the walk still owes. A page is requested by its top
        // row, and the emulator clamps a request past the last page to the
        // last page, so the final step overlaps the one before it. Tracking
        // what has been emitted rather than where the viewport was asked to go
        // is what keeps the overlap from being read as repeated history.
        let mut next = 0usize;
        while next < total && out.len() < MAX_SEARCH_ROWS {
            let page_top = next.min(history);
            self.vt.scroll(ScrollViewport::Row(page_top));
            // A projection reads only the rows the engine reports as dirty,
            // and a scrolled viewport is not guaranteed to report any. Forcing
            // a size disagreement makes the projection treat the page as a
            // resize and read every row of it, which is the only way to be
            // sure the page in hand is the page that was asked for.
            scratch
                .resize(cols, rows.saturating_sub(1).max(1))
                .map_err(VtError::from)?;
            self.vt.sync(scratch)?;

            for r in 0..rows {
                let absolute = page_top + usize::from(r);
                if absolute < next {
                    continue;
                }
                if absolute >= total || out.len() >= MAX_SEARCH_ROWS {
                    break;
                }
                let text = scratch.row_text(r).unwrap_or_default();
                out.push((absolute, text.trim_end().to_owned()));
                next = absolute + 1;
            }
            if page_top == history {
                // That was the live screen, which is the end of the buffer.
                break;
            }
        }

        // Put the operator's viewport back before anybody draws.
        self.vt.scroll(self.viewport.as_scroll());
        self.grid.mark_all_damaged();
        Ok(out)
    }

    /// The text of specific absolute rows, by the same walk.
    fn rows_text(&mut self, wanted: &[usize]) -> std::collections::BTreeMap<usize, String> {
        let mut out = std::collections::BTreeMap::new();
        let Ok(all) = self.whole_scrollback() else {
            return out;
        };
        for (row, text) in all {
            if wanted.contains(&row) {
                out.insert(row, text);
            }
        }
        out
    }

    // -----------------------------------------------------------------------
    // Overlay
    // -----------------------------------------------------------------------

    /// Put every cell the pane recoloured back to what the emulator had.
    ///
    /// Last recorded is restored first. Two overlays can cover one cell: a
    /// selection over a search hit, or the find bar over the scrollbar thumb in
    /// the bottom-right corner. Each records the value it displaced, so the
    /// second one records the first one's colour, and restoring in the order
    /// they were recorded puts the emulator's cell back and then paints the
    /// intermediate colour over it again. The cell then keeps a highlight after
    /// everything that asked for it is gone, and nothing repaints it because
    /// nothing thinks it changed.
    fn lift_overlay(&mut self) {
        while let Some((col, row, cell)) = self.overlay.pop() {
            let _ = self.grid.set_cell(col, row, cell);
        }
    }

    /// Recolour the cells the selection and the search cover.
    fn lay_overlay(&mut self) {
        let cols = self.grid.cols();
        let rows = self.grid.rows();
        let top = self.viewport.top_row();

        if let Some(find) = self.find.as_ref() {
            let current = self.current_hit;
            let hits: Vec<RowHit> = find.hits().to_vec();
            for hit in hits {
                let Some(row) = row_on_screen(hit.row, top, rows) else {
                    continue;
                };
                let (bg, fg) = self.theme.match_colours(Some(hit) == current);
                self.paint(row, hit.start.min(cols), hit.end.min(cols), bg, fg);
            }
        }

        // The selection is painted after the matches, because a selected match
        // is selected: the operator's own gesture wins over a highlight the
        // pane put there.
        if let Some(selection) = self.selection {
            let (bg, fg) = self.theme.selection_colours();
            let spans: Vec<_> = selection.spans().collect();
            for span in spans {
                let Some(row) = row_on_screen(span.row, top, rows) else {
                    continue;
                };
                self.paint(row, span.start.min(cols), span.end.min(cols), bg, fg);
            }
        }

        // The composition last, over everything, because it is the thing the
        // operator is looking at while they type it. It is drawn and never
        // sent: a half-composed character handed to a program arrives as
        // three keystrokes it will act on separately.
        if !self.preedit.is_empty()
            && let Some(cursor) = self.grid.cursor()
        {
            let (bg, fg) = self.theme.selection_colours();
            let text: Vec<char> = self.preedit.chars().collect();
            let mut col = cursor.col;
            let mut row = cursor.row;
            for ch in text {
                if col >= cols {
                    col = 0;
                    row += 1;
                }
                if row >= rows {
                    break;
                }
                self.paint_char(col, row, ch, bg, fg, true);
                col += 1;
            }
        }
        self.lay_thumb();
        self.lay_find_bar();
    }

    /// Draw the scrollbar thumb over the last column.
    ///
    /// Only while the viewport is off the live edge. A permanent gutter would
    /// cost the child a column for a control that has nothing to point at for
    /// most of a session, and a column taken away from a running agent is a
    /// full-screen redraw.
    ///
    /// The track is the height the cells cover, not the height of the box.
    /// Measuring against the box would drift the thumb away from the row it
    /// names by up to one cell at the bottom, which is exactly where the
    /// operator drags it.
    ///
    /// The cursor's colour, not the selection's. A thumb painted in the
    /// selection colour is indistinguishable from a one-cell selection at the
    /// right edge, and both mark a position, so the operator reads the wrong
    /// one as the other.
    fn lay_thumb(&mut self) {
        if self.viewport.is_live() {
            return;
        }
        let rows = self.grid.rows();
        let cols = self.grid.cols();
        let cell_h = self.cell_px.1.max(1);
        let Some((top_px, len_px)) = self.viewport.thumb(u32::from(rows) * cell_h) else {
            return;
        };
        let first = (top_px / cell_h).min(u32::from(rows.saturating_sub(1)));
        let last = (top_px + len_px)
            .div_ceil(cell_h)
            .clamp(first + 1, u32::from(rows));
        let bg = self.theme.palette.cursor;
        let fg = self.theme.palette.background;
        let col = cols.saturating_sub(1);
        for row in first..last {
            self.paint_char(col, row as u16, ' ', bg, fg, false);
        }
    }

    /// Draw the find bar on the bottom row of the grid.
    ///
    /// Inside the grid rather than above it. A strip above the pane takes a
    /// line of layout, resizes the pty, and makes every agent on screen
    /// repaint its whole transcript the moment the find opens.
    ///
    /// The pattern shown is the compiled one where there is a compiled one,
    /// so what the bar says is what the matcher is running rather than what
    /// the buffer happens to hold.
    fn lay_find_bar(&mut self) {
        let Some(input) = self.find_input.as_deref() else {
            return;
        };
        let pattern = self
            .find
            .as_ref()
            .map_or(input, |f| f.query().pattern.text());
        let text = match (self.find.is_some(), self.find_position()) {
            (_, Some((at, of))) => format!("find: {pattern}  {at}/{of}"),
            (true, None) => format!("find: {pattern}  no matches"),
            (false, None) => format!("find: {pattern}"),
        };
        let row = self.grid.rows().saturating_sub(1);
        let cols = self.grid.cols();
        let (bg, fg) = self.theme.match_colours(true);
        let mut col = 0u16;
        for ch in text.chars() {
            if col >= cols {
                break;
            }
            self.paint_char(col, row, ch, bg, fg, false);
            col += 1;
        }
        // The rest of the row belongs to the bar too, so whatever the child
        // left there does not read as part of what was typed.
        while col < cols {
            self.paint_char(col, row, ' ', bg, fg, false);
            col += 1;
        }
    }

    /// Lift and re-lay in one step, for a change that only moves the overlay.
    fn repaint_overlay(&mut self) {
        self.lift_overlay();
        self.lay_overlay();
    }

    /// Recolour one run of cells, remembering what was there.
    fn paint(&mut self, row: u16, start: u16, end: u16, bg: Rgba, fg: Rgba) {
        for col in start..end {
            let Some(was) = self.grid.cell(col, row) else {
                continue;
            };
            let painted = Cell {
                fg,
                bg,
                ..was
            };
            if painted == was {
                continue;
            }
            if self.grid.set_cell(col, row, painted).is_ok() {
                self.overlay.push((col, row, was));
            }
        }
    }

    /// Replace one cell's character and colours, remembering what was there.
    ///
    /// `underline` marks a composition, which is the one thing painted here
    /// that the operator has not committed yet.
    ///
    /// The painted cell is a single-column one whatever the cell held before.
    /// A double-width character occupies two cells, the head draws both columns
    /// and the tail draws none, so a bar or a thumb that wrote its character
    /// into a tail and kept the slot would draw nothing at all: a gap in the
    /// find bar, or a thumb that disappears on every row whose last column is
    /// the right half of a wide glyph.
    fn paint_char(&mut self, col: u16, row: u16, ch: char, bg: Rgba, fg: Rgba, underline: bool) {
        let Some(was) = self.grid.cell(col, row) else {
            return;
        };
        self.detach_half_pair(col, row, was);
        let painted = Cell {
            ch,
            fg,
            bg,
            attrs: if underline {
                was.attrs | vitrum_grid::cell::Attrs::UNDERLINE
            } else {
                was.attrs
            },
            slot: CellSlot::Single,
        };
        if painted == was {
            return;
        }
        if self.grid.set_cell(col, row, painted).is_ok() {
            self.overlay.push((col, row, was));
        }
    }

    /// Blank the other half of the wide pair `(col, row)` is part of, recording
    /// it in the overlay so lifting puts the whole pair back.
    ///
    /// Recorded before the cell that displaced it, because the overlay is
    /// restored last first: the pair's two halves then go back in the order
    /// that leaves both of them holding the emulator's own cells.
    fn detach_half_pair(&mut self, col: u16, row: u16, was: Cell) {
        let other = match was.slot {
            CellSlot::WideHead => Some(col + 1).filter(|c| *c < self.grid.cols()),
            CellSlot::WideTail => col.checked_sub(1),
            CellSlot::Single => None,
        };
        let Some(other) = other else {
            return;
        };
        let Some(had) = self.grid.cell(other, row) else {
            return;
        };
        let blank = Cell::blank(had.style());
        if blank == had {
            return;
        }
        if self.grid.set_cell(other, row, blank).is_ok() {
            self.overlay.push((other, row, had));
        }
    }
}

/// The emulator's cursor shape as the renderer's.
///
/// Two enums rather than one shared type, because the shapes a VT can request
/// and the shapes a shader can draw are different questions that happen to
/// have the same four answers today. The translation is exhaustive so a shape
/// added on either side stops compiling here rather than being drawn as a
/// block.
const fn grid_shape(shape: VtCursorShape) -> vitrum_grid::cell::CursorShape {
    use vitrum_grid::cell::CursorShape as Grid;
    match shape {
        VtCursorShape::Block => Grid::Block,
        VtCursorShape::HollowBlock => Grid::HollowBlock,
        VtCursorShape::Bar => Grid::Bar,
        VtCursorShape::Underline => Grid::Underline,
    }
}

/// The grid row an absolute row is on, if the viewport is showing it.
const fn row_on_screen(absolute: usize, top: usize, rows: u16) -> Option<u16> {
    if absolute < top {
        return None;
    }
    let offset = absolute - top;
    if offset < rows as usize {
        Some(offset as u16)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitrum_search::Query;

    fn session() -> PaneSession {
        PaneSession::new(40, 8, (10, 20), PaneTheme::default()).expect("a session is buildable")
    }

    fn feed_sync(s: &mut PaneSession, bytes: &[u8]) {
        s.feed(bytes);
        s.sync().expect("sync");
    }

    /// WHY: a pane painted black. One of the two causes
    /// is here: a grid built with the renderer's default style and recoloured
    /// afterwards paints its first frame in the wrong colour, and on a window
    /// that opens dark that first frame is black-on-black.
    ///
    /// The invariant: before a single byte is fed, every cell already carries
    /// the operator's own colours, and the grid is fully damaged so the first
    /// frame actually paints them.
    #[test]
    fn a_session_is_the_operators_colours_before_the_first_byte() {
        let mut theme = PaneTheme::default();
        theme.palette.background = Rgba::rgb(0x20, 0x10, 0x30);
        theme.palette.foreground = Rgba::rgb(0xd0, 0xd0, 0xd0);

        let s = PaneSession::new(20, 4, (10, 20), theme.clone()).unwrap();
        assert!(s.grid().is_dirty(), "the first frame would be skipped");
        for row in 0..s.grid().rows() {
            for col in 0..s.grid().cols() {
                let cell = s.grid().cell(col, row).unwrap();
                assert_eq!(cell.bg, theme.background_with_opacity(), "({col},{row})");
                assert_eq!(cell.fg, theme.palette.foreground, "({col},{row})");
            }
        }
    }

    /// WHY: a theme change while the window is open must repaint every blank
    /// cell as well as the clear colour. Moving only one of them leaves the
    /// pane clearing to the new background and painting the old one over it,
    /// which is a two-colour pane until the child happens to rewrite a row.
    #[test]
    fn changing_the_theme_repaints_the_cells_nobody_wrote() {
        let mut s = session();
        feed_sync(&mut s, b"hello");

        let mut theme = PaneTheme::default();
        theme.palette.background = Rgba::rgb(1, 2, 3);
        s.set_theme(theme.clone()).unwrap();

        assert_eq!(s.grid().default_style().bg, theme.background_with_opacity());
        let blank = s.grid().cell(30, 3).unwrap();
        assert_eq!(blank.bg, theme.background_with_opacity());
        assert!(s.grid().is_dirty(), "the change would not be drawn");
    }

    /// WHY: the mode state is what decides whether the pointer belongs to the
    /// child, how a report is encoded, and whether a paste is bracketed. A
    /// pane that guesses gets all three wrong in the programs this product
    /// manages, which are exactly the programs that set these modes.
    ///
    /// Read from a real libghostty terminal, one mode at a time, so the
    /// reading is proved against the engine rather than against a mock.
    #[test]
    fn every_mode_the_pane_acts_on_is_read_from_the_emulator() {
        let mut s = session();
        let idle = s.modes();
        assert_eq!(idle.tracking, super::super::mouse::Tracking::Off);
        assert_eq!(idle.protocol, super::super::mouse::Protocol::Legacy);
        assert!(!idle.alt_screen);
        // Mode 1007 is set by default in this engine, so a pane that assumed
        // it off would send arrow keys for a wheel notch the moment a program
        // switched to the alternate screen.
        assert!(idle.alt_scroll);
        assert!(!s.bracketed_paste());
        assert!(!s.application_cursor());

        s.feed(b"\x1b[?1000h");
        assert_eq!(s.modes().tracking, super::super::mouse::Tracking::Normal);
        s.feed(b"\x1b[?1002h");
        assert_eq!(s.modes().tracking, super::super::mouse::Tracking::Button);
        s.feed(b"\x1b[?1003h");
        assert_eq!(s.modes().tracking, super::super::mouse::Tracking::Any);
        s.feed(b"\x1b[?1006h");
        assert_eq!(s.modes().protocol, super::super::mouse::Protocol::Sgr);
        s.feed(b"\x1b[?1016h");
        assert_eq!(s.modes().protocol, super::super::mouse::Protocol::SgrPixels);

        s.feed(b"\x1b[?2004h");
        assert!(s.bracketed_paste());
        s.feed(b"\x1b[?2004l");
        assert!(!s.bracketed_paste());

        s.feed(b"\x1b[?1h");
        assert!(s.application_cursor());
        s.feed(b"\x1b[?1l");
        assert!(!s.application_cursor());

        s.feed(b"\x1b[?1049h");
        assert!(s.modes().alt_screen);
        s.feed(b"\x1b[?1049l");
        assert!(!s.modes().alt_screen);

        // The wheel belongs to the program only on the alternate screen with
        // 1007 still set. Clearing it hands the wheel back to the pane, which
        // is the whole point of the mode.
        s.feed(b"\x1b[?1049h\x1b[?1007l");
        assert!(!s.modes().alt_scroll);
        assert!(s.modes().alt_screen);
    }

    /// WHY: the overlay is a recolour of cells the emulator also writes. If it
    /// is not lifted before the projection, the projection sees the pane's own
    /// colour as the current value and skips a cell the child changed, and the
    /// pane shows stale text under a selection forever.
    ///
    /// The invariant: with a selection held, the text under it still updates,
    /// and clearing the selection leaves the emulator's own colours behind
    /// with nothing of the pane's left.
    #[test]
    fn output_under_a_selection_still_updates_and_leaves_no_residue() {
        let mut s = session();
        feed_sync(&mut s, b"\x1b[H\x1b[31moriginal text\x1b[0m");
        let before = s.grid().cell(0, 0).unwrap();

        s.select_start(Point { row: 0, col: 0 }, SelectMode::Line);
        s.select_drag(Point { row: 0, col: 12 });
        let selected = s.grid().cell(0, 0).unwrap();
        assert_ne!(selected.bg, before.bg, "the selection painted nothing");

        // The child rewrites the row while the selection is held.
        feed_sync(&mut s, b"\x1b[H\x1b[32mreplaced\x1b[0m");
        assert_eq!(
            s.grid().cell(0, 0).unwrap().ch,
            'r',
            "the text under the selection did not update"
        );
        assert_eq!(
            s.grid().cell(0, 0).unwrap().bg,
            selected.bg,
            "the selection stopped being painted"
        );

        s.select_clear();
        let after = s.grid().cell(0, 0).unwrap();
        assert_eq!(after.ch, 'r');
        assert_ne!(after.bg, selected.bg, "the selection left residue behind");
    }

    /// WHY: a selection is in absolute rows, so it has to stop being painted
    /// when the operator scrolls away from it and start again when they come
    /// back. Painting it at a fixed grid row would highlight whatever text
    /// happens to be there.
    #[test]
    fn a_selection_is_painted_only_where_the_viewport_is_showing_it() {
        let mut s = session();
        for i in 0..40 {
            s.feed(format!("line {i}\r\n").as_bytes());
        }
        s.sync().unwrap();

        let top = s.viewport().top_row();
        s.select_start(Point { row: top, col: 0 }, SelectMode::Line);
        s.select_drag(Point { row: top, col: 5 });
        s.sync().unwrap();
        let painted = s.grid().cell(0, 0).unwrap().bg;
        assert_eq!(painted, s.theme().selection_colours().0);

        // Scroll away: the selected row is no longer on screen, so nothing on
        // screen may carry the selection's colour.
        s.scroll(|v| v.by_pages(-3));
        s.sync().unwrap();
        for row in 0..s.grid().rows() {
            for col in 0..s.grid().cols() {
                assert_ne!(
                    s.grid().cell(col, row).unwrap().bg,
                    painted,
                    "a selection off screen was painted at ({col},{row})"
                );
            }
        }

        // And back.
        s.scroll_to_bottom();
        s.sync().unwrap();
        assert_eq!(s.grid().cell(0, 0).unwrap().bg, painted);
    }

    /// WHY: the page walk is the only way to read history the emulator is not
    /// showing, and it depends on the projection reading rows the engine may
    /// consider unchanged. If the walk silently returns the live viewport for
    /// every page, a find over a long session finds only what is on screen and
    /// reports it as the whole answer.
    ///
    /// The invariant is stated on content: every line ever printed is found
    /// exactly once, in order, including lines far outside the viewport.
    #[test]
    fn the_page_walk_reads_history_the_viewport_is_not_showing() {
        let mut s = session();
        for i in 0..200 {
            s.feed(format!("line {i:03}\r\n").as_bytes());
        }
        s.sync().unwrap();

        let rows = s.whole_scrollback().unwrap();
        let joined: Vec<&str> = rows.iter().map(|(_, t)| t.as_str()).collect();
        for i in 0..200 {
            let want = format!("line {i:03}");
            assert_eq!(
                joined.iter().filter(|t| **t == want).count(),
                1,
                "{want} was not read exactly once from the history"
            );
        }
        // In order, and with the absolute row indices contiguous from zero.
        for (i, (row, _)) in rows.iter().enumerate() {
            assert_eq!(*row, i, "the walk skipped or repeated a row");
        }
    }

    /// WHY: a search must not move the operator's viewport as a side effect,
    /// and must not disturb the frame being drawn.
    #[test]
    fn a_search_puts_the_viewport_back_where_it_found_it() {
        let mut s = session();
        for i in 0..200 {
            s.feed(format!("line {i:03}\r\n").as_bytes());
        }
        s.sync().unwrap();
        s.scroll(|v| v.by_pages(-4));
        let before = s.viewport();

        let _ = s.whole_scrollback().unwrap();
        assert_eq!(s.viewport(), before, "the walk moved the viewport");
    }

    /// WHY: a find that only searches the visible screen is a find that
    /// answers the wrong question, and the operator cannot tell: it reports a
    /// count and the count looks like an answer.
    #[test]
    fn a_find_reaches_matches_far_outside_the_viewport() {
        let mut s = session();
        s.feed(b"needle at the very start\r\n");
        for i in 0..300 {
            s.feed(format!("filler {i}\r\n").as_bytes());
        }
        s.feed(b"needle at the very end\r\n");
        s.sync().unwrap();

        let count = s
            .find_start(Query::literal("needle"))
            .unwrap()
            .expect("the pattern compiles");
        assert_eq!(count, 2, "the find did not reach the whole history");

        // Stepping brings each one on screen.
        let first = s.find_step(true).expect("a match");
        s.sync().unwrap();
        assert!(
            s.viewport().top_row() <= first.row
                && first.row < s.viewport().top_row() + usize::from(s.grid().rows()),
            "the match was not brought on screen"
        );
    }

    /// WHY: a highlight painted at the wrong columns points at the wrong text,
    /// which is worse than not highlighting at all.
    #[test]
    fn a_search_match_is_highlighted_on_the_cells_it_covers() {
        let mut s = session();
        feed_sync(&mut s, b"\x1b[Halpha needle omega");

        s.find_start(Query::literal("needle")).unwrap().unwrap();
        s.sync().unwrap();

        let (bg, _) = s.theme().match_colours(true);
        for col in 6..12 {
            assert_eq!(
                s.grid().cell(col, 0).unwrap().bg,
                bg,
                "column {col} was not highlighted"
            );
        }
        assert_ne!(s.grid().cell(5, 0).unwrap().bg, bg, "the space before it");
        assert_ne!(s.grid().cell(12, 0).unwrap().bg, bg, "the space after it");

        s.find_clear();
        s.sync().unwrap();
        for col in 0..20 {
            assert_ne!(s.grid().cell(col, 0).unwrap().bg, bg, "residue at {col}");
        }
    }

    /// WHY: a caret drawn while the operator is reading history says they are
    /// typing into a row from an hour ago.
    #[test]
    fn the_caret_is_drawn_only_on_the_live_view() {
        let mut s = session();
        for i in 0..60 {
            s.feed(format!("line {i}\r\n").as_bytes());
        }
        s.sync().unwrap();
        assert!(s.grid().cursor().is_some(), "the live view has no caret");

        s.scroll(|v| v.by_pages(-2));
        s.sync().unwrap();
        assert!(s.grid().cursor().is_none(), "a caret was drawn in history");

        s.scroll_to_bottom();
        s.sync().unwrap();
        assert!(s.grid().cursor().is_some());
    }

    /// WHY: output arriving while the operator reads history must not drag
    /// them back to the bottom, and a viewport at the bottom must follow.
    /// This is the pane's half of the same rule the viewport model states.
    #[test]
    fn output_moves_a_live_viewport_and_leaves_a_scrolled_one() {
        let mut s = session();
        for i in 0..60 {
            s.feed(format!("line {i}\r\n").as_bytes());
        }
        s.sync().unwrap();
        assert!(s.viewport().is_live());

        s.scroll(|v| v.by_pages(-2));
        let pinned = s.viewport().top_row();

        for i in 60..90 {
            s.feed(format!("line {i}\r\n").as_bytes());
        }
        s.sync().unwrap();
        assert_eq!(s.viewport().top_row(), pinned, "output moved the reader");
        assert!(!s.viewport().is_live());

        s.scroll_to_bottom();
        s.sync().unwrap();
        assert!(s.viewport().is_live());
    }

    /// WHY: a resize reflows every row, so a selection made before it covers
    /// text nobody picked afterwards. Keeping it is worse than dropping it,
    /// because the operator then copies the wrong thing.
    #[test]
    fn a_resize_drops_a_selection_rather_than_moving_it() {
        let mut s = session();
        feed_sync(&mut s, b"\x1b[Hsome text here");
        s.select_start(Point { row: 0, col: 0 }, SelectMode::Line);
        s.select_drag(Point { row: 0, col: 9 });
        assert!(s.selection_text().is_some());

        s.resize(60, 12, (10, 20)).unwrap();
        assert!(s.selection_text().is_none(), "a selection survived a reflow");
        assert_eq!(s.grid().cols(), 40, "the grid follows the next projection");
        s.sync().unwrap();
        assert_eq!((s.grid().cols(), s.grid().rows()), (60, 12));
    }

    /// WHY: a device query the pane never answers hangs the program that
    /// issued it, and the program sits there looking like it crashed.
    #[test]
    fn a_device_query_gets_its_answer_back() {
        let mut s = session();
        // Primary device attributes: every program that probes a terminal
        // sends one of these before deciding what it may use.
        s.feed(b"\x1b[c");
        let mut out = Vec::new();
        s.drain_pty_write(&mut out);
        assert!(!out.is_empty(), "the emulator's answer was never collected");
        assert_eq!(out[0], 0x1b, "the answer is an escape sequence");
    }

    /// WHY: a composition has to be visible where it is being typed and must
    /// never reach the child. A pane that sends each intermediate character
    /// gives a program three keystrokes for one glyph, and a pane that draws
    /// nothing leaves the operator composing blind.
    ///
    /// The invariant: the uncommitted text is in the grid at the cursor, the
    /// emulator never saw it, and clearing it puts the cells back.
    #[test]
    fn a_composition_is_drawn_at_the_cursor_and_never_sent() {
        let mut s = session();
        feed_sync(&mut s, b"\x1b[H> ");
        let cursor = s.grid().cursor().expect("a live view has a caret");
        let (col, row) = (cursor.col, cursor.row);
        let before: Vec<char> = (0..4)
            .map(|i| s.grid().cell(col + i, row).unwrap().ch)
            .collect();

        assert!(s.set_preedit("ばか"));
        assert!(!s.set_preedit("ばか"), "the same composition marked a frame");
        assert_eq!(s.grid().cell(col, row).unwrap().ch, 'ば');
        assert_eq!(s.grid().cell(col + 1, row).unwrap().ch, 'か');

        // Nothing was handed to the child, and nothing was handed to the
        // emulator either: a sync must not wipe the composition out.
        let mut out = Vec::new();
        s.drain_pty_write(&mut out);
        assert!(out.is_empty(), "a composition reached the child");
        s.sync().unwrap();
        assert_eq!(s.grid().cell(col, row).unwrap().ch, 'ば');

        assert!(s.set_preedit(""));
        for (i, ch) in before.iter().enumerate() {
            assert_eq!(
                s.grid().cell(col + i as u16, row).unwrap().ch,
                *ch,
                "the composition left residue at column {}",
                col + i as u16
            );
        }
    }

    /// WHY: the find the operator can reach is the one driven by keystrokes,
    /// and every part of it can be present and still not joined up. This
    /// drives the whole path: open, type, highlight, count, step, close.
    ///
    /// Cutting any wire in it fails here. A `find_type` that does not compile
    /// the query leaves the cells unhighlighted, one that does not keep the
    /// text leaves the bar blank, and one that does not seek leaves the
    /// counter at nothing.
    #[test]
    fn typing_a_query_highlights_the_matches_and_says_how_many() {
        let mut s = session();
        feed_sync(&mut s, b"\x1b[Halpha needle omega\r\nsecond needle here");

        s.find_open();
        assert!(s.find_is_open());
        assert_eq!(s.find_input(), Some(""));

        // Typed one character at a time, which is how it arrives.
        let mut typed = String::new();
        for ch in "needle".chars() {
            typed.push(ch);
            s.find_type(&typed).expect("the scrollback is readable");
        }
        s.sync().unwrap();

        assert_eq!(s.find_input(), Some("needle"));
        assert_eq!(s.find_position(), Some((1, 2)));

        let (current, _) = s.theme().match_colours(true);
        for col in 6..12 {
            assert_eq!(
                s.grid().cell(col, 0).unwrap().bg,
                current,
                "column {col} of the current match was not highlighted"
            );
        }

        // The bar reads back what is being matched and where in the set the
        // operator is.
        let bar = s.grid().row_text(s.grid().rows() - 1).unwrap();
        assert!(bar.starts_with("find: needle  1/2"), "the bar reads {bar:?}");

        s.find_step(true);
        s.sync().unwrap();
        assert_eq!(s.find_position(), Some((2, 2)));
        let bar = s.grid().row_text(s.grid().rows() - 1).unwrap();
        assert!(bar.starts_with("find: needle  2/2"), "the bar reads {bar:?}");

        // Backspacing to nothing keeps the find open, because the operator is
        // still standing in it.
        s.find_type("").expect("the scrollback is readable");
        s.sync().unwrap();
        assert!(s.find_is_open());
        assert_eq!(s.find_position(), None);

        s.find_clear();
        s.sync().unwrap();
        assert!(!s.find_is_open());
        let bar = s.grid().row_text(s.grid().rows() - 1).unwrap();
        assert!(!bar.contains("find:"), "the bar left residue: {bar:?}");
    }

    /// WHY: a thumb the operator can see but not act on is a decoration. The
    /// position the scrollbar reports and the offset a drag to that position
    /// produces have to be the same number, or the view jumps away from the
    /// thumb the moment it is grabbed.
    #[test]
    fn a_thumb_dragged_to_a_position_scrolls_to_the_offset_it_names() {
        let mut s = session();
        for i in 0..400 {
            s.feed(format!("line {i:03}\r\n").as_bytes());
        }
        s.sync().unwrap();

        let track = u32::from(s.grid().rows()) * 20;
        let max = s.viewport().max_offset();
        assert!(max > 8, "the session kept no history to scroll through");

        for target in [0usize, 1, 40, max / 2, max] {
            s.scroll_to_offset(target);
            assert_eq!(s.viewport().offset(), target, "asked for {target}");

            // Grabbing the thumb where it is drawn and letting go without
            // moving must leave it where it was. The offset it reads back as
            // is coarser than a row, because a track has fewer pixels than
            // the history has rows, so the invariant is on the pixel and not
            // on the row: the thumb does not jump out from under the pointer.
            let (top, _) = s.viewport().thumb(track).expect("history has a thumb");
            let round = s.viewport().offset_for_thumb(top, track);
            s.scroll_to_offset(round);
            let (again, _) = s.viewport().thumb(track).expect("history has a thumb");
            assert_eq!(
                again, top,
                "a thumb grabbed at {top}px read back as offset {round} and \
                 redrew at {again}px"
            );
        }

        // Past the end clamps rather than running off the oldest row.
        s.scroll_to_offset(usize::MAX);
        assert_eq!(s.viewport().offset(), s.viewport().max_offset());
    }

    /// WHY: a permanent gutter costs the child a column for a control with
    /// nothing to point at, and a thumb that never appears is a scrollback
    /// the operator cannot see the shape of.
    #[test]
    fn the_thumb_is_drawn_only_while_the_viewport_is_off_the_live_edge() {
        let mut s = session();
        for i in 0..200 {
            s.feed(format!("line {i:03}\r\n").as_bytes());
        }
        s.sync().unwrap();

        let thumb = s.theme().palette.cursor;
        let last = s.grid().cols() - 1;
        for row in 0..s.grid().rows() {
            assert_ne!(
                s.grid().cell(last, row).unwrap().bg,
                thumb,
                "a live viewport drew a thumb at row {row}"
            );
        }

        s.scroll(|v| v.by_pages(-4));
        s.sync().unwrap();
        let drawn = (0..s.grid().rows())
            .filter(|row| s.grid().cell(last, *row).unwrap().bg == thumb)
            .count();
        assert!(drawn > 0, "a scrolled viewport drew no thumb");
        assert!(
            drawn < usize::from(s.grid().rows()),
            "the thumb filled the whole track, so it says nothing"
        );

        s.scroll_to_bottom();
        s.sync().unwrap();
        for row in 0..s.grid().rows() {
            assert_ne!(
                s.grid().cell(last, row).unwrap().bg,
                thumb,
                "the thumb left residue at row {row}"
            );
        }
    }

    /// WHY: copy is the one gesture whose result leaves the process, so a
    /// selection that paints correctly and copies the wrong rows is a defect
    /// the operator carries into another program.
    #[test]
    fn a_selection_copies_exactly_the_rows_it_covers() {
        let mut s = session();
        feed_sync(&mut s, b"\x1b[Hfirst line\r\nsecond line\r\nthird line");

        let top = s.viewport().top_row();
        assert_eq!(s.selection_text(), None, "nothing is selected yet");

        s.select_start(Point { row: top, col: 0 }, SelectMode::Line);
        s.select_drag(Point { row: top + 1, col: 0 });
        assert_eq!(s.selection_mode(), Some(SelectMode::Line));
        assert_eq!(
            s.selection_text().as_deref(),
            Some("first line\nsecond line")
        );

        // A bare click covers no cell and must not put an empty string on the
        // clipboard: a click that clears what was copied is a click nobody
        // makes on purpose.
        s.select_start(Point { row: top, col: 3 }, SelectMode::Character);
        assert_eq!(s.selection_text(), None);

        assert!(s.select_clear());
        assert!(!s.select_clear(), "clearing twice reported a second change");
    }

    /// WHY: the overlay is a stack of whole-cell substitutions, and two of them
    /// can land on the same cell: a selection over a search hit, or the find
    /// bar over the scrollbar thumb on the bottom-right cell. Restoring them in
    /// the order they were recorded puts the first one back and then the second
    /// one back over it, so the cell keeps the highlight after everything that
    /// asked for it is gone. That is a coloured cell nothing on screen explains
    /// and nothing will repaint.
    ///
    /// The invariant: after every overlay is dropped, every cell holds exactly
    /// what the emulator put there.
    #[test]
    fn overlapping_overlays_leave_no_colour_behind() {
        let mut s = session();
        feed_sync(&mut s, b"\x1b[Habcabc");

        let clean: Vec<Cell> = (0..s.grid().cols())
            .map(|col| s.grid().cell(col, 0).expect("row zero is on screen"))
            .collect();

        let top = s.viewport().top_row();
        s.find_type("abc").expect("a literal find compiles");
        s.select_start(Point { row: top, col: 0 }, SelectMode::Character);
        s.select_drag(Point { row: top, col: 6 });
        assert!(
            (0..6).any(|col| s.grid().cell(col, 0).unwrap() != clean[col as usize]),
            "neither overlay painted, so the ordering is not under test"
        );

        s.select_clear();
        s.find_clear();

        for col in 0..s.grid().cols() {
            assert_eq!(
                s.grid().cell(col, 0).unwrap(),
                clean[col as usize],
                "column {col} kept an overlay colour after every overlay was dropped"
            );
        }
    }

    /// WHY: an overlay writes a whole cell, and a cell can be half of a
    /// double-width character. The tail of a pair draws nothing, because the
    /// head's quad already covers both columns, so an overlay that writes a
    /// character into a tail and leaves the slot alone produces a hole: a find
    /// bar with a gap in it, or a scrollbar thumb that vanishes on every row
    /// whose last column is the tail of a wide glyph.
    ///
    /// The invariant: a cell an overlay wrote draws itself, and the pair it
    /// broke does not survive as an orphaned half.
    #[test]
    fn an_overlay_over_a_wide_pair_draws_what_it_wrote() {
        use vitrum_grid::cell::CellSlot;

        let mut s = session();
        // A pair whose tail is the last column, which is where the thumb goes.
        let cols = s.grid().cols();
        feed_sync(&mut s, format!("\x1b[1;{}H\u{3042}", cols - 1).as_bytes());
        assert_eq!(
            s.grid().cell(cols - 2, 0).unwrap().slot,
            CellSlot::WideHead,
            "the emulator did not lay a wide pair at the right edge"
        );

        s.find_open();
        s.find_type("nothing-matches-this").ok();

        let last_row = s.grid().rows() - 1;
        // A pair on the find bar's own row, so the bar has to write over it.
        feed_sync(&mut s, format!("\x1b[{};1H\u{3042}\u{3044}", last_row + 1).as_bytes());

        for col in 0..cols {
            let cell = s.grid().cell(col, last_row).expect("the bar row is on screen");
            assert_eq!(
                cell.slot,
                CellSlot::Single,
                "the find bar left column {col} as {:?}, which draws nothing",
                cell.slot
            );
        }
    }

    // -----------------------------------------------------------------------
    // The artefact suite
    // -----------------------------------------------------------------------

    /// One thing an operator or a child program does to a live session.
    ///
    /// Every variant goes through the same public entry point the pane's widget
    /// calls, so a scenario is a recording of real use rather than a poke at
    /// internal state.
    #[derive(Clone, Copy, Debug)]
    enum Act {
        /// Bytes from the child.
        Feed(&'static str),
        /// Wheel or key paging, in rows, positive being back into history.
        Scroll(i64),
        /// Jump to the live edge, which is what typing does.
        ToBottom,
        /// Press at a cell, relative to the top row on screen.
        SelectStart { row: usize, col: u16, mode: SelectMode },
        /// Drag to a cell, relative to the top row on screen.
        SelectDrag { row: usize, col: u16 },
        /// Release the selection.
        SelectClear,
        /// Type into the find bar.
        FindType(&'static str),
        /// Close the find bar.
        FindClear,
        /// Follow the widget to a new cell count.
        Resize { cols: u16, rows: u16 },
        /// Text an input method has not committed.
        Preedit(&'static str),
    }

    struct ActScenario {
        name: &'static str,
        acts: &'static [Act],
    }

    /// Every situation the session-level artefact suite drives.
    ///
    /// The list is the test's input, so closing a regression means adding the
    /// sequence that produced it rather than adding an assertion elsewhere.
    static ACT_SCENARIOS: &[ActScenario] = &[
        ActScenario {
            name: "output arrives at the bottom while the viewport is scrolled back",
            acts: &[
                Act::Feed("line one\r\nline two\r\nline three\r\nline four\r\nline five\r\n"),
                Act::Feed("six\r\nseven\r\neight\r\nnine\r\nten\r\neleven\r\ntwelve\r\n"),
                Act::Scroll(4),
                Act::Feed("printed while scrolled back\r\n"),
                Act::Feed("and again\r\n"),
                Act::Scroll(2),
                Act::Scroll(-3),
                Act::ToBottom,
            ],
        },
        ActScenario {
            name: "a line is rewritten shorter than it was",
            acts: &[
                Act::Feed("\x1b[H\x1b[2Ja long line of output here"),
                Act::Feed("\r\x1b[Kshort"),
                Act::Feed("\r\nsecond\r\x1b[K"),
            ],
        },
        ActScenario {
            name: "an SGR change repaints a region without changing its characters",
            acts: &[
                Act::Feed("\x1b[H\x1b[2Jplain text on a row"),
                Act::Feed("\x1b[H\x1b[31mplain text on a row"),
                Act::Feed("\x1b[H\x1b[1;4;7mplain text on a row"),
                Act::Feed("\x1b[H\x1b[mplain text on a row"),
            ],
        },
        ActScenario {
            name: "wide characters and combining marks at a cell boundary",
            acts: &[
                Act::Feed("\x1b[H\x1b[2J\u{3042}\u{3044}\u{3046}\u{3048}"),
                Act::Feed("\x1b[1;2Hn"),
                Act::Feed("\x1b[1;3Hm"),
                Act::Feed("\x1b[2;1He\u{0301}fg"),
                // A pair that ends on the last column, then one that cannot fit.
                Act::Feed("\x1b[3;39H\u{3042}"),
                Act::Feed("\x1b[4;40H\u{3042}"),
                Act::Feed("\x1b[3;39Hz"),
            ],
        },
        ActScenario {
            name: "the alternate screen is entered and left",
            acts: &[
                Act::Feed("primary one\r\nprimary two\r\n"),
                Act::Feed("\x1b[?1049h"),
                Act::Feed("\x1b[2J\x1b[H\x1b[7m editor \x1b[m\r\nbuffer body"),
                Act::Feed("\x1b[?25l"),
                Act::Feed("\x1b[?25h"),
                Act::Feed("\x1b[?1049l"),
                Act::Feed("back on the primary\r\n"),
            ],
        },
        ActScenario {
            name: "the caret moves, changes shape, hides and shows",
            acts: &[
                Act::Feed("\x1b[H\x1b[2Jcaret work"),
                Act::Feed("\x1b[1;1H"),
                Act::Feed("\x1b[1;5H"),
                Act::Feed("\x1b[5 q"),
                Act::Feed("\x1b[3 q"),
                Act::Feed("\x1b[1 q"),
                Act::Feed("\x1b[?25l"),
                Act::Feed("\x1b[?25h"),
                Act::Feed("\x1b[8;20H"),
                Act::Scroll(3),
                Act::ToBottom,
            ],
        },
        ActScenario {
            name: "a selection is made, extended, and cleared over a search",
            acts: &[
                Act::Feed("\x1b[H\x1b[2Jselect this and this and this\r\nsecond row of text"),
                Act::SelectStart { row: 0, col: 0, mode: SelectMode::Character },
                Act::SelectDrag { row: 0, col: 11 },
                Act::SelectDrag { row: 1, col: 6 },
                Act::FindType("this"),
                Act::SelectDrag { row: 1, col: 18 },
                Act::SelectClear,
                Act::FindClear,
                Act::Feed("\r\nthird row"),
            ],
        },
        ActScenario {
            name: "the find bar and the scrollbar thumb share the corner cell",
            acts: &[
                Act::Feed("filler\r\n"),
                Act::Feed("more\r\nmore\r\nmore\r\nmore\r\nmore\r\nmore\r\nmore\r\nmore\r\n"),
                Act::Scroll(3),
                Act::FindType("more"),
                Act::Scroll(1),
                Act::FindClear,
                Act::ToBottom,
            ],
        },
        ActScenario {
            name: "a composition is typed at the caret and committed",
            acts: &[
                Act::Feed("\x1b[H\x1b[2J$ "),
                Act::Preedit("ni"),
                Act::Preedit("nih"),
                Act::Preedit("\u{4f60}\u{597d}"),
                Act::Preedit(""),
                Act::Feed("\u{4f60}\u{597d}"),
            ],
        },
        ActScenario {
            name: "a resize reflows a session that has scrolled",
            acts: &[
                Act::Feed("one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\n"),
                Act::Feed("a line long enough that a narrower grid has to wrap it somewhere\r\n"),
                Act::Scroll(3),
                Act::Resize { cols: 24, rows: 6 },
                Act::Feed("after the shrink\r\n"),
                Act::Resize { cols: 40, rows: 12 },
                Act::Feed("after the grow\r\n"),
                Act::Resize { cols: 40, rows: 8 },
                Act::ToBottom,
            ],
        },
    ];

    fn act(s: &mut PaneSession, a: Act) {
        match a {
            Act::Feed(bytes) => s.feed(bytes.as_bytes()),
            Act::Scroll(rows) => {
                s.scroll(|v| v.by_lines(rows));
            }
            Act::ToBottom => {
                s.scroll_to_bottom();
            }
            Act::SelectStart { row, col, mode } => {
                let top = s.viewport().top_row();
                s.select_start(Point { row: top + row, col }, mode);
            }
            Act::SelectDrag { row, col } => {
                let top = s.viewport().top_row();
                s.select_drag(Point { row: top + row, col });
            }
            Act::SelectClear => {
                s.select_clear();
            }
            Act::FindType(pattern) => {
                s.find_open();
                s.find_type(pattern).expect("a literal find compiles");
            }
            Act::FindClear => s.find_clear(),
            Act::Resize { cols, rows } => {
                s.resize(cols, rows, (10, 20)).expect("the emulator accepts the size");
            }
            Act::Preedit(text) => {
                s.set_preedit(text);
            }
        }
    }

    /// WHY: `paint()` is change-gated, so a cell whose damage was never marked
    /// is a cell the renderer never re-uploads. The instance it already holds is
    /// drawn again on every frame, so the wrong content stays on screen until
    /// something unrelated happens to damage that row. That is the whole family
    /// of static glitches, and none of its members is
    /// visible from the grid's own state: the grid is correct and the marks are
    /// not.
    ///
    /// The invariant, checked after every act of every scenario: the frame the
    /// incremental renderer produces is byte for byte the frame a full
    /// unconditional repaint of the same grid would produce. The reference is a
    /// clone of the same grid marked wholly damaged and drawn through a
    /// renderer that remembers nothing, so any difference is a damage mark that
    /// was owed and not made.
    ///
    /// Driven through a real libghostty session rather than a hand-built grid,
    /// because the marks that go missing are the ones the emulator's own
    /// projection and the pane's overlay disagree about.
    #[test]
    fn every_frame_a_session_produces_matches_a_full_repaint() {
        let mut rig = Differ::new();
        for scenario in ACT_SCENARIOS {
            let mut s =
                PaneSession::new(40, 8, (10, 20), PaneTheme::default()).expect("a session builds");
            rig.restart();

            for (n, a_step) in scenario.acts.iter().copied().enumerate() {
                act(&mut s, a_step);
                s.sync().expect("the emulator projects onto the grid");
                if let Some(what) = rig.compare(s.grid_mut()) {
                    panic!(
                        "{}: act {n} ({a_step:?}) left an undamaged cell on screen\n  {what}",
                        scenario.name
                    );
                }
            }
        }
    }

    /// The comparison must be able to see an artefact, or it proves nothing.
    ///
    /// WHY: a differential test whose two sides can never disagree is green
    /// forever and says nothing about the code it names. Dropping the damage
    /// marks without uploading them is exactly what every bug this suite hunts
    /// does by accident, so the comparison has to fail on it.
    #[test]
    fn the_session_comparison_detects_a_dropped_damage_mark() {
        let mut rig = Differ::new();
        let mut s = PaneSession::new(40, 8, (10, 20), PaneTheme::default()).expect("a session builds");
        s.feed(b"first frame");
        s.sync().unwrap();
        assert_eq!(rig.compare(s.grid_mut()), None, "the first frame already differs");

        s.feed(b"\r\nan artefact nobody marked");
        s.sync().unwrap();
        s.grid_mut().clear_damage();
        assert!(
            rig.compare(s.grid_mut()).is_some(),
            "a dropped damage mark did not show up as a pixel difference"
        );
    }

    /// The incremental renderer, the full-repaint renderer, and the two targets
    /// they draw into.
    struct Differ {
        gpu: vitrum_grid::GpuContext,
        incremental: vitrum_grid::GridRenderer,
        reference: vitrum_grid::GridRenderer,
        a: vitrum_grid::HeadlessTarget,
        b: vitrum_grid::HeadlessTarget,
        cell: (u32, u32),
    }

    impl Differ {
        fn new() -> Self {
            use vitrum_grid::{GpuContext, GridRenderer, HeadlessTarget, RendererConfig};

            let gpu = GpuContext::headless().expect("the artefact suite needs a wgpu adapter");
            let config = RendererConfig {
                format: HeadlessTarget::FORMAT,
                ..RendererConfig::default()
            };
            let incremental = GridRenderer::new(gpu.device(), &config)
                .expect("a monospace face must be available");
            let reference = GridRenderer::new(gpu.device(), &config)
                .expect("a monospace face must be available");
            // The largest grid any scenario reaches, so one pair of targets
            // serves them all and a resize never reallocates mid-comparison.
            let cell = incremental.cell_size();
            let a = HeadlessTarget::new(gpu.device(), cell.0 * 40, cell.1 * 12);
            let b = HeadlessTarget::new(gpu.device(), cell.0 * 40, cell.1 * 12);
            Self { gpu, incremental, reference, a, b, cell }
        }

        /// Forget the last scenario, so a scenario's first frame is a genuine
        /// first frame rather than a continuation of the one before it.
        fn restart(&mut self) {
            self.incremental.invalidate();
        }

        /// Draw `grid` incrementally and against a full repaint of the same
        /// cells, and describe the first pixel where the two disagree.
        fn compare(&mut self, grid: &mut CellGrid) -> Option<String> {
            let (cw, ch) = self.cell;
            let viewport = (cw * u32::from(grid.cols()), ch * u32::from(grid.rows()));
            let mut want = grid.clone();

            self.incremental
                .render(self.gpu.device(), self.gpu.queue(), grid, self.a.view(), viewport)
                .expect("the incremental frame draws");
            let live = self.a.read(self.gpu.device(), self.gpu.queue());

            want.mark_all_damaged();
            self.reference.invalidate();
            self.reference
                .render(self.gpu.device(), self.gpu.queue(), &mut want, self.b.view(), viewport)
                .expect("the full repaint draws");
            let full = self.b.read(self.gpu.device(), self.gpu.queue());

            if live.as_bytes() == full.as_bytes() {
                return None;
            }
            let (x, y) = (0..live.height())
                .flat_map(|y| (0..live.width()).map(move |x| (x, y)))
                .find(|(x, y)| live.pixel(*x, *y) != full.pixel(*x, *y))
                .expect("the images differ, so some pixel differs");
            Some(format!(
                "pixel ({x}, {y}) in cell ({}, {}): incremental {:?}, full repaint {:?}",
                x / cw,
                y / ch,
                live.pixel(x, y),
                full.pixel(x, y)
            ))
        }
    }
}
