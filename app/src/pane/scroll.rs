//! Where the viewport sits in the scrollback.
//!
//! The emulator holds the history and the pane decides which part of it is on
//! screen. That decision is arithmetic over three numbers and nothing else:
//! how many rows of history exist, how many rows the viewport shows, and how
//! far back from the live edge it is. Keeping it here, with no emulator handle
//! in sight, is what lets the whole of the paging behaviour be tested without
//! a terminal.
//!
//! # Following the live edge
//!
//! A viewport at the bottom follows new output. A viewport scrolled back does
//! not, and holding it still is not the absence of a behaviour: history grows
//! underneath it, so the offset from the bottom has to grow by exactly as much
//! or the text the operator was reading slides upward while they read it. That
//! is the flapping an operator sees as a pane that will not stay where it is
//! put.
//!
//! # One line of overlap
//!
//! A page is one row less than the viewport. The last line of the old page is
//! the first line of the new one, which is what makes a paged read continuous
//! rather than a sequence of disjoint screens with a line lost at every seam.

use vitrum_vt::ScrollViewport;

/// Shortest a scrollbar thumb may be drawn, in pixels.
///
/// A thumb proportional to the viewport's share of a hundred thousand lines of
/// history is under a pixel, which is a scrollbar nobody can grab and nobody
/// can see. Below this it stops being proportional and starts being a control.
const MIN_THUMB_PX: u32 = 24;

/// The viewport's position in a session's history.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Viewport {
    /// Rows of history above the active area.
    history: usize,
    /// Rows the viewport shows.
    rows: u16,
    /// Rows back from the live edge. Zero is live.
    offset: usize,
}

impl Viewport {
    /// A viewport of `rows` rows, live, over `history` rows of scrollback.
    pub(crate) fn new(rows: u16, history: usize) -> Self {
        Self {
            history,
            rows,
            offset: 0,
        }
    }

    /// Rows back from the live edge. Zero means the viewport is live.
    pub(crate) const fn offset(self) -> usize {
        self.offset
    }

    /// Rows of history above the active area.
    pub(crate) const fn history(self) -> usize {
        self.history
    }

    /// Whether the viewport is at the live edge and following output.
    pub(crate) const fn is_live(self) -> bool {
        self.offset == 0
    }

    /// Whether the viewport is showing the oldest row the emulator still has.
    ///
    /// The gesture behind a request for more history: arriving here means
    /// there is nothing further back to show, and the only way to see more is
    /// to ask the daemon for it. A viewport over no history at all is at the
    /// top and at the bottom at once, which is correct and is why this is not
    /// the negation of [`Self::is_live`].
    pub(crate) const fn at_top(self) -> bool {
        self.offset >= self.history
    }

    /// The furthest back the viewport can go.
    ///
    /// Exactly the history: scrolling back past the first retained row would
    /// show rows that were never written.
    pub(crate) const fn max_offset(self) -> usize {
        self.history
    }

    /// Rows moved per page. One less than the viewport, never zero.
    pub(crate) const fn page_rows(self) -> usize {
        if self.rows > 1 {
            self.rows as usize - 1
        } else {
            1
        }
    }

    /// Follow the widget to a new row count.
    ///
    /// The offset is held rather than the top row, because the operator's
    /// place in the text is the bottom of what they are reading: growing the
    /// window taller must reveal more above, not scroll what they were reading
    /// off the top.
    pub(crate) fn set_rows(&mut self, rows: u16) {
        self.rows = rows;
        self.clamp();
    }

    /// Record that the history is now this deep.
    ///
    /// A viewport at the live edge stays there and shows the new output. A
    /// viewport scrolled back keeps showing the same rows, which means its
    /// offset grows by however much history grew.
    pub(crate) fn history_grew_to(&mut self, history: usize) {
        let added = history.saturating_sub(self.history);
        self.history = history;
        if !self.is_live() {
            self.offset = self.offset.saturating_add(added);
        }
        self.clamp();
    }

    /// Record a history that shrank, which is what a reset or a trim does.
    ///
    /// Separate from [`Viewport::history_grew_to`] because the two are not the
    /// same operation read backwards: history that went away cannot be kept in
    /// view, so the offset clamps and the operator lands on the oldest row
    /// that still exists rather than on nothing.
    pub(crate) fn history_shrank_to(&mut self, history: usize) {
        self.history = history;
        self.clamp();
    }

    /// Move by whole lines. Positive is towards the live edge.
    pub(crate) fn by_lines(&mut self, delta: i64) {
        let offset = i128::from(self.offset as u64) - i128::from(delta);
        self.offset = offset.clamp(0, self.max_offset() as i128) as usize;
    }

    /// Move by whole pages. Positive is towards the live edge.
    pub(crate) fn by_pages(&mut self, pages: i64) {
        let rows = self.page_rows() as i64;
        self.by_lines(pages.saturating_mul(rows));
    }

    /// Move by wheel notches. Positive is towards the live edge.
    pub(crate) fn by_notches(&mut self, notches: i64, lines_per_notch: u16) {
        self.by_lines(notches.saturating_mul(i64::from(lines_per_notch.max(1))));
    }

    /// Jump to the oldest retained row.
    pub(crate) fn to_top(&mut self) {
        self.offset = self.max_offset();
    }

    /// Jump to the live edge and resume following output.
    pub(crate) fn to_bottom(&mut self) {
        self.offset = 0;
    }

    /// Put an absolute row on screen, counted from the oldest retained row.
    ///
    /// The row is placed a third of the way down rather than at the top,
    /// because a match at the very top of the viewport has no context above it
    /// and the operator has to page back to see what led to it.
    pub(crate) fn reveal(&mut self, row: usize) {
        let lead = self.rows as usize / 3;
        let want_top = row.saturating_sub(lead);
        // The top row of a live viewport is `history`, so an offset of
        // `history - want_top` puts `want_top` at the top.
        self.offset = self.history.saturating_sub(want_top);
        self.clamp();
    }

    /// The absolute index of the viewport's first row, counted from the oldest
    /// retained row.
    pub(crate) const fn top_row(self) -> usize {
        self.history.saturating_sub(self.offset)
    }

    /// How the emulator should be told to move.
    ///
    /// An absolute row rather than a delta. A delta assumes the emulator's
    /// idea of the current position matches this one, and the two drift the
    /// first time history is trimmed between a scroll and its application.
    pub(crate) const fn as_scroll(self) -> ScrollViewport {
        if self.offset == 0 {
            ScrollViewport::Bottom
        } else {
            ScrollViewport::Row(self.top_row())
        }
    }

    /// Where to draw the scrollbar thumb in a track `track_px` long.
    ///
    /// `None` when there is no history: a scrollbar over a session that fits
    /// on screen is a control with nothing to control, and drawing it makes a
    /// fresh pane look like it is hiding something.
    ///
    /// Returns the thumb's offset from the top of the track and its length,
    /// both in pixels. The thumb is proportional to the viewport's share of
    /// the whole document, floored so it stays grabbable, and the offset is
    /// scaled into the space the floor left rather than computed against the
    /// full track, which is what keeps the thumb inside the track at the
    /// bottom.
    pub(crate) fn thumb(self, track_px: u32) -> Option<(u32, u32)> {
        if self.history == 0 || track_px == 0 {
            return None;
        }
        let total = self.history as f64 + f64::from(self.rows);
        let visible = f64::from(self.rows) / total;
        let track = f64::from(track_px);
        let len = (track * visible).round().max(f64::from(MIN_THUMB_PX));
        let len = len.min(track);

        // Zero at the top of the history, one at the live edge.
        let progress = if self.max_offset() == 0 {
            1.0
        } else {
            1.0 - (self.offset as f64 / self.max_offset() as f64)
        };
        let travel = track - len;
        let top = (travel * progress).round();
        Some((top as u32, len as u32))
    }

    /// The offset a thumb dragged to `top_px` in a `track_px` track means.
    ///
    /// The inverse of [`Viewport::thumb`] over the same floor, so grabbing the
    /// thumb and putting it back where it was does not move the view.
    pub(crate) fn offset_for_thumb(self, top_px: u32, track_px: u32) -> usize {
        if self.max_offset() == 0 || track_px == 0 {
            return 0;
        }
        let Some((_, len)) = self.thumb(track_px) else {
            return 0;
        };
        let travel = f64::from(track_px.saturating_sub(len));
        if travel <= 0.0 {
            return 0;
        }
        let progress = (f64::from(top_px) / travel).clamp(0.0, 1.0);
        ((1.0 - progress) * self.max_offset() as f64).round() as usize
    }

    /// Hold the offset inside the history that exists.
    fn clamp(&mut self) {
        let max = self.max_offset();
        if self.offset > max {
            self.offset = max;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Viewport heights a real window produces, from a very short pane to a 4K
    /// one at a small type size.
    const ROWS: &[u16] = &[1, 2, 3, 24, 43, 50, 67, 101, 158, 240];

    /// History depths, including none and the default retention.
    const HISTORY: &[usize] = &[0, 1, 7, 50, 999, 1_000, 10_000, 100_000];

    /// WHY: paging past either end is the defect that presents as a pane that
    /// scrolls into blankness, or one that refuses to come back to the live
    /// edge.
    ///
    /// The invariant is a bound, not a value: whatever sequence of moves is
    /// applied, the offset is between zero and the history. Asserting it after
    /// every move over every size is what closes the class, rather than
    /// asserting one page-up lands where it should.
    #[test]
    fn no_sequence_of_moves_leaves_the_viewport_outside_its_history() {
        for &rows in ROWS {
            for &history in HISTORY {
                let mut v = Viewport::new(rows, history);
                let moves: &[fn(&mut Viewport)] = &[
                    |v| v.by_pages(-1),
                    |v| v.by_pages(1),
                    |v| v.by_pages(-1_000_000),
                    |v| v.by_pages(1_000_000),
                    |v| v.by_lines(i64::MIN / 2),
                    |v| v.by_lines(i64::MAX / 2),
                    |v| v.by_notches(-3, 3),
                    |v| v.by_notches(3, 3),
                    Viewport::to_top,
                    Viewport::to_bottom,
                    |v| v.reveal(0),
                    |v| v.reveal(usize::MAX / 2),
                ];
                for step in moves {
                    step(&mut v);
                    assert!(
                        v.offset() <= v.max_offset(),
                        "offset {} past history {history} at {rows} rows",
                        v.offset()
                    );
                }
            }
        }
    }

    /// WHY: output arriving while the operator is reading history is the
    /// commonest thing that happens in this product, and a viewport that does
    /// not compensate slides the text upward under their eyes. That is what
    /// the operator saw as a pane that will not stay where it is put.
    ///
    /// The invariant: while scrolled back, the absolute top row on screen does
    /// not change no matter how much history arrives. While live, it tracks
    /// the live edge.
    #[test]
    fn output_arriving_does_not_move_a_viewport_the_operator_scrolled_back() {
        let mut v = Viewport::new(50, 1_000);
        v.by_pages(-2);
        let pinned = v.top_row();
        assert!(!v.is_live());

        for added in [1usize, 1, 5, 100, 4_000] {
            let before = v.history();
            v.history_grew_to(before + added);
            assert_eq!(
                v.top_row(),
                pinned,
                "{added} rows of output moved the operator's view"
            );
            assert!(!v.is_live());
        }

        let mut live = Viewport::new(50, 1_000);
        assert!(live.is_live());
        for added in [1usize, 100, 4_000] {
            let before = live.history();
            live.history_grew_to(before + added);
            assert!(live.is_live(), "a live viewport stopped following output");
            assert_eq!(live.top_row(), live.history());
        }
    }

    /// WHY: history that goes away cannot be kept in view, and an offset left
    /// pointing past the oldest retained row is a viewport showing nothing.
    #[test]
    fn a_history_that_shrinks_lands_the_viewport_on_a_row_that_exists() {
        let mut v = Viewport::new(30, 5_000);
        v.to_top();
        assert_eq!(v.offset(), 5_000);

        v.history_shrank_to(100);
        assert_eq!(v.offset(), 100);
        assert_eq!(v.top_row(), 0);

        v.history_shrank_to(0);
        assert_eq!(v.offset(), 0);
        assert!(v.is_live(), "an emptied history leaves the viewport live");
    }

    /// WHY: a page with no overlap loses a line at every seam, and the line it
    /// loses is the one the operator was about to read.
    ///
    /// The invariant is stated as coverage rather than as a row count: paging
    /// back from the live edge and forward again returns to the live edge, and
    /// every page shares exactly one row with the one before it.
    #[test]
    fn a_page_overlaps_the_previous_page_by_exactly_one_row() {
        for &rows in ROWS {
            let mut v = Viewport::new(rows, 10_000);
            let first_top = v.top_row();
            v.by_pages(-1);
            let second_top = v.top_row();
            assert_eq!(
                first_top - second_top,
                v.page_rows(),
                "a page at {rows} rows moved the wrong distance"
            );
            if rows > 1 {
                assert_eq!(
                    v.page_rows(),
                    rows as usize - 1,
                    "a page must be one row short of the viewport"
                );
            } else {
                assert_eq!(v.page_rows(), 1, "a one-row viewport still pages");
            }
            v.by_pages(1);
            assert!(v.is_live(), "paging forward did not return to the live edge");
        }
    }

    /// WHY: a match revealed at the very top of the viewport has no context
    /// above it, and the operator pages back to find out what led to it. This
    /// is the difference between a search that answers a question and one that
    /// asks another.
    #[test]
    fn revealing_a_row_leaves_context_above_it() {
        for &rows in &[24u16, 50, 101] {
            let mut v = Viewport::new(rows, 10_000);
            v.reveal(5_000);
            let lead = rows as usize / 3;
            assert_eq!(v.top_row(), 5_000 - lead, "at {rows} rows");
            assert!(v.top_row() <= 5_000);
            assert!(5_000 < v.top_row() + rows as usize, "the row is off screen");
        }

        // A row near the top of history cannot have a third of a screen above
        // it, and the viewport lands at the top rather than refusing.
        let mut v = Viewport::new(50, 10_000);
        v.reveal(3);
        assert_eq!(v.top_row(), 0);
    }

    /// WHY: a thumb that leaves the track, or that is a hairline over a deep
    /// history, is a control the operator cannot use.
    ///
    /// The bound is geometric and holds at every position: the thumb starts
    /// inside the track, ends inside the track, and is never shorter than the
    /// floor.
    #[test]
    fn the_thumb_stays_inside_its_track_at_every_position_and_depth() {
        for &track in &[40u32, 200, 617, 2_160] {
            for &rows in ROWS {
                for &history in HISTORY {
                    let mut v = Viewport::new(rows, history);
                    for offset in [0usize, 1, history / 3, history / 2, history] {
                        v.to_bottom();
                        v.by_lines(-(offset as i64));
                        let Some((top, len)) = v.thumb(track) else {
                            assert_eq!(history, 0, "no thumb over a real history");
                            continue;
                        };
                        assert!(len > 0, "a zero-length thumb");
                        assert!(len <= track, "thumb {len} longer than track {track}");
                        assert!(
                            len >= MIN_THUMB_PX.min(track),
                            "thumb {len} under the floor at {history} rows"
                        );
                        assert!(
                            top + len <= track,
                            "thumb {top}+{len} runs past track {track}"
                        );
                    }
                }
            }
        }
    }

    /// WHY: grabbing a thumb and putting it back must not move the view, and
    /// the round trip is where an off-by-one in the floor shows up.
    #[test]
    fn dragging_the_thumb_and_releasing_it_lands_where_it_was_grabbed() {
        for &track in &[200u32, 617, 2_160] {
            for &history in &[50usize, 999, 10_000] {
                let mut v = Viewport::new(50, history);
                for offset in [0usize, 1, history / 4, history / 2, history] {
                    v.to_bottom();
                    v.by_lines(-(offset as i64));
                    let (top, _) = v.thumb(track).unwrap();
                    let round = v.offset_for_thumb(top, track);
                    let drift = round.abs_diff(v.offset());
                    // One row of drift per pixel of track is the resolution
                    // the track has; more than that is arithmetic, not
                    // quantisation.
                    let resolution = (history / track.max(1) as usize) + 1;
                    assert!(
                        drift <= resolution,
                        "{drift} rows of drift over {history} rows in a {track}px track"
                    );
                }
            }
        }
    }

    /// WHY: the emulator is told an absolute row, not a delta, and a viewport
    /// at the live edge has to say so rather than naming the last row. A row
    /// that happens to be the bottom stops following output the moment one
    /// more line arrives.
    #[test]
    fn a_live_viewport_asks_for_the_bottom_and_not_for_a_row() {
        let mut v = Viewport::new(50, 1_000);
        assert_eq!(v.as_scroll(), ScrollViewport::Bottom);

        v.by_lines(-1);
        assert_eq!(v.as_scroll(), ScrollViewport::Row(999));

        v.to_top();
        assert_eq!(v.as_scroll(), ScrollViewport::Row(0));

        v.to_bottom();
        assert_eq!(v.as_scroll(), ScrollViewport::Bottom);
    }

    /// WHY: a window resized taller must reveal more history above, not scroll
    /// the text the operator is reading off the top of the pane.
    #[test]
    fn growing_the_pane_reveals_history_above_rather_than_moving_the_text() {
        let mut v = Viewport::new(24, 1_000);
        v.by_pages(-4);
        let offset = v.offset();

        v.set_rows(50);
        assert_eq!(v.offset(), offset, "a resize moved the viewport");
        assert_eq!(v.top_row(), 1_000 - offset);

        // Shrinking below the history is still legal; the offset is unchanged
        // because the offset is measured from the bottom.
        v.set_rows(2);
        assert_eq!(v.offset(), offset);
    }

    /// WHY: the wheel setting is the operator's, and a zero would freeze the
    /// wheel with no error anywhere.
    #[test]
    fn a_wheel_notch_always_moves_at_least_one_line() {
        let mut v = Viewport::new(50, 1_000);
        v.by_notches(-1, 0);
        assert_eq!(v.offset(), 1, "a zero setting must not freeze the wheel");

        v.to_bottom();
        v.by_notches(-1, 7);
        assert_eq!(v.offset(), 7);
    }

    /// WHY: arrival at the oldest retained row is the gesture that asks the
    /// daemon for more history. A predicate that is true one row early sends a
    /// request per wheel notch near the top; one that is true one row late
    /// never sends one at all, and the operator sees a buffer that stops.
    ///
    /// Also pinned: a session with no history is at the top already, so a pane
    /// that has just opened asks once rather than never.
    #[test]
    fn the_top_is_the_oldest_retained_row_and_nowhere_else() {
        let empty = Viewport::new(24, 0);
        assert!(empty.at_top(), "a pane with no history has nothing above it");
        assert!(empty.is_live(), "and it is also at the live edge");

        let mut v = Viewport::new(24, 100);
        assert!(!v.at_top());
        v.by_lines(-99);
        assert!(!v.at_top(), "one row short of the top is not the top");
        v.by_lines(-1);
        assert!(v.at_top());
        // Asking to go further back cannot move past it.
        v.by_pages(-10);
        assert!(v.at_top());
        assert_eq!(v.offset(), 100);

        // Output arriving adds rows at the live edge, below the reader. The
        // oldest row is still the oldest row, so the viewport is still at the
        // top and must not ask for a second page because the child spoke.
        v.history_grew_to(200);
        assert!(v.at_top());
        assert_eq!(v.top_row(), 0, "the reader was dragged off the oldest row");
    }
}
