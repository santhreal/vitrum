//! What the pointer has selected, and what copying it produces.
//!
//! A selection is two points in the session's row space and a mode that says
//! how to grow them. Row space is absolute: row zero is the oldest retained
//! row, not the top of the viewport, so a selection survives the output that
//! arrives while it is held and survives paging away and back. A selection in
//! viewport coordinates would slide up the screen every time a line was
//! printed, which is the same defect as a viewport that does not compensate
//! for growing history, and it is the reason none of the coordinates here are
//! screen coordinates.
//!
//! # The four modes
//!
//! - **Character**: exactly the cells between the two points.
//! - **Word**: both ends grown outward to word boundaries, and dragging keeps
//!   whole words at both ends.
//! - **Line**: whole rows.
//! - **Block**: the rectangle the two points bound, one span per row at the
//!   same columns. This is the only mode where a row's span does not run to
//!   the end of the row, and it is what makes a column of a table copyable.
//!
//! # Trailing blanks
//!
//! A terminal row is a fixed number of cells and most of them are blank. Every
//! row is that wide, so copying a wrapped paragraph without trimming produces
//! text padded to the pane's width, which pastes into another program as
//! ragged columns. Character and line selections trim the run of blanks at the
//! end of each row. Block selections do not: the operator drew a rectangle and
//! the blanks inside it are part of the rectangle.

/// Characters that continue a word beyond letters and digits.
///
/// Not just `_`. What an operator double-clicks in an agent transcript is a
/// path, a flag, a version or an identifier, and a word rule that stops at the
/// first dot turns one double-click into four. This is a starting point the
/// operator can change, not a constant: a language whose identifiers use other
/// punctuation needs a different set, and the pane cannot know which.
pub(crate) const DEFAULT_WORD_CHARS: &str = "_-./?&=%+@~:#";

/// A cell, in the session's absolute row space.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub(crate) struct Point {
    /// Rows from the oldest retained row.
    pub row: usize,
    /// Columns from the left of the grid.
    pub col: u16,
}

/// How a drag grows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Mode {
    /// Cell by cell.
    #[default]
    Character,
    /// Whole words at both ends.
    Word,
    /// Whole rows.
    Line,
    /// A rectangle.
    Block,
}

impl Mode {
    /// The mode a click of this many rapid repeats starts.
    ///
    /// One click is a character drag, two is a word, three is a line. Four
    /// starts over at a character rather than inventing a fourth meaning,
    /// which is what every other terminal does and what a hand resting on a
    /// mouse produces by accident.
    pub(crate) const fn for_click_count(count: u32) -> Self {
        match count % 3 {
            2 => Self::Word,
            0 => Self::Line,
            _ => Self::Character,
        }
    }
}

/// One row's worth of selected cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Span {
    /// Absolute row.
    pub row: usize,
    /// First selected column.
    pub start: u16,
    /// One past the last selected column.
    pub end: u16,
}

impl Span {
    /// Columns in this span.
    pub(crate) const fn len(self) -> u16 {
        self.end.saturating_sub(self.start)
    }
}

/// A live selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Selection {
    /// Where the drag started, already grown for the mode.
    anchor: (Point, Point),
    /// Where the pointer is now, already grown for the mode.
    head: (Point, Point),
    /// How it grows.
    mode: Mode,
    /// Columns in a row, which bounds every span.
    cols: u16,
}

impl Selection {
    /// Start a selection at `at`.
    ///
    /// `word` resolves a word's bounds on a row, and is called only for word
    /// mode. It is a closure rather than the row text because a word selection
    /// on a wrapped line has to be able to read the row it lands on without
    /// the caller materialising every row first.
    pub(crate) fn start(
        at: Point,
        mode: Mode,
        cols: u16,
        word: impl Fn(Point) -> (u16, u16),
    ) -> Self {
        let grown = grow(at, mode, cols, &word);
        Self {
            anchor: grown,
            head: grown,
            mode,
            cols,
        }
    }

    /// Move the loose end.
    pub(crate) fn drag_to(&mut self, at: Point, word: impl Fn(Point) -> (u16, u16)) {
        self.head = grow(at, self.mode, self.cols, &word);
    }

    /// How this selection grows.
    pub(crate) const fn mode(&self) -> Mode {
        self.mode
    }

    /// Whether the selection covers no cell at all.
    ///
    /// A bare click is this: pressed and released without moving, in character
    /// mode. It must not put an empty string on the clipboard and must not
    /// paint a highlight, because both would make a click look like a bug.
    pub(crate) fn is_empty(&self) -> bool {
        self.spans().all(|s| s.len() == 0)
    }

    /// The two ends, in reading order.
    fn ends(&self) -> (Point, Point) {
        let (a0, a1) = self.anchor;
        let (h0, h1) = self.head;
        if (h0, h1) < (a0, a1) {
            (h0, a1)
        } else {
            (a0, h1)
        }
    }

    /// The rows and columns covered, one span per row, in reading order.
    ///
    /// Empty spans are produced for rows a block selection covers with a
    /// zero-width rectangle, so a caller iterating spans sees the same rows the
    /// operator's rectangle touches.
    pub(crate) fn spans(&self) -> impl Iterator<Item = Span> + '_ {
        let (start, end) = self.ends();
        let cols = self.cols;
        let mode = self.mode;
        // A block's columns come from the two points regardless of which is
        // higher on screen, because the rectangle is bounded by both corners
        // and a drag to the left is still a rectangle.
        let (left, right) = if mode == Mode::Block {
            let a = self.anchor.0.col.min(self.head.0.col);
            let b = self.anchor.1.col.max(self.head.1.col);
            (a.min(b), a.max(b))
        } else {
            (0, cols)
        };

        (start.row..=end.row).map(move |row| match mode {
            Mode::Block => Span {
                row,
                start: left.min(cols),
                end: right.min(cols),
            },
            Mode::Line => Span {
                row,
                start: 0,
                end: cols,
            },
            Mode::Character | Mode::Word => {
                let first = if row == start.row { start.col } else { 0 };
                let last = if row == end.row { end.col } else { cols };
                Span {
                    row,
                    start: first.min(cols),
                    end: last.min(cols).max(first.min(cols)),
                }
            }
        })
    }

    /// The text this selection copies.
    ///
    /// `row_text` returns a row's characters, or `None` for a row that is no
    /// longer retained. A row that went away contributes nothing and does not
    /// abort the copy: history is trimmed while a selection is held, and
    /// refusing to copy the part that survives would be worse than copying it.
    pub(crate) fn text(&self, row_text: impl Fn(usize) -> Option<Vec<char>>) -> String {
        let trim = self.mode != Mode::Block;
        let mut out = String::new();
        let mut first = true;
        for span in self.spans() {
            let Some(chars) = row_text(span.row) else {
                continue;
            };
            if !first {
                out.push('\n');
            }
            first = false;

            let from = usize::from(span.start).min(chars.len());
            let to = usize::from(span.end).min(chars.len());
            let mut slice = &chars[from..to];
            if trim {
                while let Some((last, rest)) = slice.split_last() {
                    if *last == ' ' || *last == '\0' {
                        slice = rest;
                    } else {
                        break;
                    }
                }
            }
            out.extend(slice.iter().map(|c| if *c == '\0' { ' ' } else { *c }));
        }
        out
    }
}

/// Grow one point into the pair of points its mode covers.
fn grow(
    at: Point,
    mode: Mode,
    cols: u16,
    word: &impl Fn(Point) -> (u16, u16),
) -> (Point, Point) {
    match mode {
        Mode::Character | Mode::Block => (at, at),
        Mode::Line => (
            Point { row: at.row, col: 0 },
            Point {
                row: at.row,
                col: cols,
            },
        ),
        Mode::Word => {
            let (start, end) = word(at);
            (
                Point {
                    row: at.row,
                    col: start,
                },
                Point {
                    row: at.row,
                    col: end,
                },
            )
        }
    }
}

/// The kind of run a character belongs to.
///
/// Three kinds and not two, because a double-click on punctuation between two
/// words must select the punctuation rather than the whole line or nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    /// Blank.
    Space,
    /// Part of a word.
    Word,
    /// Punctuation that is not in the word set.
    Other,
}

fn classify(c: char, word_chars: &str) -> Class {
    if c == ' ' || c == '\0' || c == '\t' {
        Class::Space
    } else if c.is_alphanumeric() || word_chars.contains(c) {
        Class::Word
    } else {
        Class::Other
    }
}

/// The run of like characters around `col`.
///
/// Returns a half-open column range. A column past the end of the row selects
/// nothing, which is what a double-click in the blank right-hand part of a
/// short line should do.
pub(crate) fn word_bounds(chars: &[char], col: u16, word_chars: &str) -> (u16, u16) {
    let idx = usize::from(col);
    if idx >= chars.len() {
        return (col, col);
    }
    let class = classify(chars[idx], word_chars);
    let mut start = idx;
    while start > 0 && classify(chars[start - 1], word_chars) == class {
        start -= 1;
    }
    let mut end = idx + 1;
    while end < chars.len() && classify(chars[end], word_chars) == class {
        end += 1;
    }
    (start as u16, end as u16)
}

/// Rows to scroll per tick while a drag is held outside the pane.
///
/// Zero inside the pane, which is the case that must cost nothing: a drag in
/// the middle of the pane calls this on every motion sample and must not start
/// a timer.
///
/// The rate ramps with distance rather than being constant. A constant rate
/// forces the operator to hold a drag for as long as the selection is deep,
/// which over ten thousand rows is not a gesture anybody completes; a ramp
/// lets a small excursion creep one row at a time and a large one cover a page
/// per tick.
pub(crate) fn autoscroll_rows(pointer_y: i32, pane_height: i32, page_rows: u16) -> i32 {
    /// Pixels of overshoot per extra row per tick.
    const RAMP_PX: i32 = 12;

    let over = if pointer_y < 0 {
        pointer_y
    } else if pointer_y >= pane_height {
        pointer_y - pane_height + 1
    } else {
        return 0;
    };

    let rows = (over.abs() + RAMP_PX - 1) / RAMP_PX;
    let rows = rows.clamp(1, i32::from(page_rows).max(1));
    if over < 0 { -rows } else { rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    /// A row of a transcript wide enough to have blanks at the end, which is
    /// every row a terminal ever holds.
    fn padded(s: &str, cols: usize) -> Vec<char> {
        let mut v = chars(s);
        v.resize(cols, ' ');
        v
    }

    const COLS: u16 = 40;

    fn no_words(_: Point) -> (u16, u16) {
        (0, 0)
    }

    /// WHY: a selection held in viewport coordinates slides up the screen
    /// every time the child prints a line, and the operator watches their
    /// selection walk away from the text they picked.
    ///
    /// Absolute rows are the fix, and the observable form of it is that
    /// nothing in this module takes a viewport. The test that this is true is
    /// that a selection's spans do not change when the viewport does, which is
    /// asserted by there being no viewport to change: what is asserted here is
    /// the consequence, that spans name absolute rows and are stable.
    #[test]
    fn spans_name_absolute_rows_and_do_not_move() {
        let mut s = Selection::start(
            Point { row: 900, col: 4 },
            Mode::Character,
            COLS,
            no_words,
        );
        s.drag_to(Point { row: 902, col: 9 }, no_words);
        let before: Vec<_> = s.spans().collect();
        assert_eq!(before[0].row, 900);
        assert_eq!(before[2].row, 902);
        let after: Vec<_> = s.spans().collect();
        assert_eq!(before, after);
    }

    /// WHY: a drag upward is the same selection as the drag downward that
    /// covers it, and a pane that produces an empty selection for one of them
    /// is a pane where selecting backwards does nothing.
    #[test]
    fn dragging_backwards_selects_the_same_cells_as_dragging_forwards() {
        let a = Point { row: 10, col: 5 };
        let b = Point { row: 12, col: 20 };

        let mut down = Selection::start(a, Mode::Character, COLS, no_words);
        down.drag_to(b, no_words);
        let mut up = Selection::start(b, Mode::Character, COLS, no_words);
        up.drag_to(a, no_words);

        assert_eq!(
            down.spans().collect::<Vec<_>>(),
            up.spans().collect::<Vec<_>>()
        );
    }

    /// WHY: a bare click must not clear the clipboard, and must not paint a
    /// highlight over the cell it landed on. Both look like a bug and one of
    /// them destroys what the operator copied a moment earlier.
    #[test]
    fn a_click_that_never_moved_selects_nothing() {
        let s = Selection::start(Point { row: 3, col: 7 }, Mode::Character, COLS, no_words);
        assert!(s.is_empty());
        assert_eq!(s.text(|_| Some(padded("hello", 40))), "");

        let mut moved = s;
        moved.drag_to(Point { row: 3, col: 8 }, no_words);
        assert!(!moved.is_empty());
    }

    /// WHY: a rectangle is the only reason block mode exists, and the failure
    /// is that a drag to the left produces a backwards rectangle covering
    /// nothing.
    ///
    /// Both corners bound the rectangle regardless of drag direction, and
    /// every covered row carries the same columns. That second half is what
    /// makes a column of a table copyable and is what distinguishes this from
    /// every other mode.
    #[test]
    fn a_block_selection_is_the_same_rectangle_from_any_corner() {
        let corners = [
            (Point { row: 4, col: 10 }, Point { row: 8, col: 20 }),
            (Point { row: 8, col: 20 }, Point { row: 4, col: 10 }),
            (Point { row: 4, col: 20 }, Point { row: 8, col: 10 }),
            (Point { row: 8, col: 10 }, Point { row: 4, col: 20 }),
        ];
        for (from, to) in corners {
            let mut s = Selection::start(from, Mode::Block, COLS, no_words);
            s.drag_to(to, no_words);
            let spans: Vec<_> = s.spans().collect();
            assert_eq!(spans.len(), 5, "{from:?} to {to:?}");
            for span in &spans {
                assert_eq!((span.start, span.end), (10, 20), "{from:?} to {to:?}");
            }
            assert_eq!(spans[0].row, 4);
            assert_eq!(spans[4].row, 8);
        }
    }

    /// WHY: a terminal row is padded to the pane's width, so copying without
    /// trimming pastes a wrapped paragraph as ragged columns of spaces. A
    /// block selection must NOT trim, because the operator drew a rectangle
    /// and the blanks in it are the rectangle.
    ///
    /// The pair is the point: one rule for both is wrong in one direction or
    /// the other, and only running both catches a change that fixes one by
    /// breaking the other.
    #[test]
    fn line_and_character_copies_trim_padding_and_a_block_copy_does_not() {
        let rows = |row: usize| -> Option<Vec<char>> {
            Some(match row {
                0 => padded("first line", 40),
                1 => padded("second", 40),
                _ => padded("third line here", 40),
            })
        };

        let mut line = Selection::start(Point { row: 0, col: 0 }, Mode::Line, COLS, no_words);
        line.drag_to(Point { row: 2, col: 0 }, no_words);
        assert_eq!(line.text(rows), "first line\nsecond\nthird line here");

        let mut character =
            Selection::start(Point { row: 0, col: 0 }, Mode::Character, COLS, no_words);
        character.drag_to(Point { row: 1, col: 40 }, no_words);
        assert_eq!(character.text(rows), "first line\nsecond");

        let mut block = Selection::start(Point { row: 0, col: 2 }, Mode::Block, COLS, no_words);
        block.drag_to(Point { row: 2, col: 12 }, no_words);
        assert_eq!(
            block.text(rows),
            "rst line  \ncond      \nird line h",
            "a rectangle keeps the blanks inside it"
        );
    }

    /// WHY: history is trimmed while a selection is held, and a copy that
    /// aborts because one row went away loses the part that is still there.
    #[test]
    fn a_row_that_is_no_longer_retained_is_skipped_rather_than_aborting() {
        let mut s = Selection::start(Point { row: 0, col: 0 }, Mode::Line, COLS, no_words);
        s.drag_to(Point { row: 3, col: 0 }, no_words);
        let text = s.text(|row| match row {
            1 => None,
            other => Some(padded(&format!("row {other}"), 40)),
        });
        assert_eq!(text, "row 0\nrow 2\nrow 3");
    }

    /// WHY: what an operator double-clicks in an agent transcript is a path, a
    /// flag, a version or an identifier. A word rule that stops at the first
    /// dot or slash turns one gesture into four and makes double-click
    /// useless in exactly this product's content.
    ///
    /// Each case names the run a double-click on the marked column must
    /// select. The punctuation case is the one that is easy to get wrong in
    /// the other direction: a run of characters in neither class is its own
    /// run, not the whole line.
    #[test]
    fn a_double_click_selects_the_whole_token_an_operator_meant() {
        let cases: &[(&str, u16, &str)] = &[
            ("run src/main.rs now", 5, "src/main.rs"),
            ("run src/main.rs now", 4, "src/main.rs"),
            ("cargo --release build", 8, "--release"),
            ("version 1.2.3-rc.1 ok", 10, "1.2.3-rc.1"),
            ("call foo_bar(x)", 6, "foo_bar"),
            ("call foo_bar(x)", 12, "("),
            ("a == b", 2, "=="),
            ("word  gap", 5, "  "),
            ("user@host:~/src", 4, "user@host:~/src"),
        ];
        for &(line, col, want) in cases {
            let row = chars(line);
            let (start, end) = word_bounds(&row, col, DEFAULT_WORD_CHARS);
            let got: String = row[usize::from(start)..usize::from(end)].iter().collect();
            assert_eq!(got, want, "double-click at {col} in {line:?}");
        }
    }

    /// WHY: the word set is the operator's, and a pane that hardcodes it
    /// cannot serve a language whose identifiers use other punctuation.
    #[test]
    fn the_word_set_changes_what_a_double_click_takes() {
        let row = chars("a.b.c");
        assert_eq!(word_bounds(&row, 0, DEFAULT_WORD_CHARS), (0, 5));
        assert_eq!(word_bounds(&row, 0, ""), (0, 1), "a bare set stops at the dot");
        assert_eq!(word_bounds(&row, 1, ""), (1, 2), "and the dot is its own run");
    }

    /// WHY: a double-click in the blank part of a short line must select
    /// nothing rather than the run of padding to the pane's edge.
    #[test]
    fn a_double_click_past_the_end_of_a_row_selects_nothing() {
        let row = chars("short");
        for col in [5u16, 6, 39, 400] {
            assert_eq!(word_bounds(&row, col, DEFAULT_WORD_CHARS), (col, col));
        }
    }

    /// WHY: a word drag that does not keep whole words at both ends is a word
    /// selection in name only: the first character of the drag is a word and
    /// every one after it is a character.
    #[test]
    fn dragging_a_word_selection_keeps_whole_words_at_both_ends() {
        let rows: &[&str] = &["alpha beta gamma", "delta epsilon zeta"];
        let word = |p: Point| {
            let row = chars(rows[p.row]);
            word_bounds(&row, p.col, DEFAULT_WORD_CHARS)
        };

        // Start inside "beta", drag into "epsilon".
        let mut s = Selection::start(Point { row: 0, col: 7 }, Mode::Word, COLS, word);
        let spans: Vec<_> = s.spans().collect();
        assert_eq!((spans[0].start, spans[0].end), (6, 10), "whole word at rest");

        s.drag_to(Point { row: 1, col: 8 }, word);
        let spans: Vec<_> = s.spans().collect();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start, 6, "the anchor kept its whole word");
        assert_eq!(spans[1].end, 13, "the head grew to a whole word");
    }

    /// WHY: click counts come from the toolkit and keep counting. A fourth
    /// click must start over rather than fall through to an unhandled mode.
    #[test]
    fn click_counts_cycle_through_character_word_and_line() {
        let want = [
            (1u32, Mode::Character),
            (2, Mode::Word),
            (3, Mode::Line),
            (4, Mode::Character),
            (5, Mode::Word),
            (6, Mode::Line),
            (7, Mode::Character),
            (300, Mode::Line),
        ];
        for (count, mode) in want {
            assert_eq!(Mode::for_click_count(count), mode, "{count} clicks");
        }
    }

    /// WHY: a drag inside the pane must not start a timer. Autoscroll that
    /// fires while the pointer is in the middle of the pane is a pane that
    /// scrolls whenever anybody selects anything.
    ///
    /// And the ramp: a constant rate over ten thousand rows is a gesture
    /// nobody completes, so the rate has to grow with the overshoot and stay
    /// bounded by a page so a flick does not jump the whole history.
    #[test]
    fn autoscroll_is_off_inside_the_pane_and_ramps_outside_it() {
        const H: i32 = 600;
        for y in [0, 1, 200, 599] {
            assert_eq!(autoscroll_rows(y, H, 50), 0, "y={y} inside the pane");
        }

        assert_eq!(autoscroll_rows(-1, H, 50), -1, "just above");
        assert_eq!(autoscroll_rows(H, H, 50), 1, "just below");

        let mut last = 0;
        for over in [1, 12, 24, 60, 120, 600, 6_000] {
            let rows = autoscroll_rows(H + over, H, 50);
            assert!(rows >= last, "the ramp went backwards at {over}px");
            assert!(rows <= 50, "{rows} rows exceeds a page");
            last = rows;
        }
        assert_eq!(last, 50, "a large overshoot must reach the page bound");

        // Symmetric: dragging above the pane scrolls back at the same rate.
        for over in [1, 24, 600] {
            assert_eq!(
                autoscroll_rows(-over, H, 50),
                -autoscroll_rows(H + over - 1, H, 50),
                "the two directions disagree at {over}px"
            );
        }
    }

    /// WHY: a page bound of zero would make autoscroll clamp to nothing and
    /// freeze a drag at the pane's edge with no error.
    #[test]
    fn autoscroll_still_moves_when_the_pane_is_one_row_tall() {
        assert_eq!(autoscroll_rows(1_000, 20, 0), 1);
        assert_eq!(autoscroll_rows(-1_000, 20, 0), -1);
    }
}
