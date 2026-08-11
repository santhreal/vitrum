//! Find, over the scrollback the emulator holds.
//!
//! # One definition of a match
//!
//! The daemon answers "which of my agents mentioned this" across every
//! session's retained bytes. The pane answers "where in this session" over the
//! rows the emulator has. Two implementations of "a match" would diverge the
//! first time either was touched, and the operator would get a hit in the
//! cross-session list that the pane cannot find. So there is one: this module
//! compiles [`vitrum_search::Matcher`] from the same [`vitrum_search::Query`]
//! the daemon compiles, and runs it. Literal against regex, case folding and
//! whole-word are therefore the same decision in both places by construction,
//! not by agreement.
//!
//! What the two cannot share is the text. The daemon scans the byte stream
//! with escape sequences stripped, and never replays cursor motion, so
//! `50%\r100%` reads there as `50%100%`. The pane scans the grid, where the
//! same bytes left `100%` in the cell. The pane's reading is the one on
//! screen, so where they differ the daemon has an extra hit and the pane does
//! not, never the other way round. That is the honest seam and it is not
//! closeable without giving the daemon a grid per session.
//!
//! # Columns, not bytes
//!
//! A hit in a pane is a run of CELLS to paint, and the matcher works in bytes.
//! One cell is one character and a character is one to four bytes, so the byte
//! range comes back through the row's own character indices. Doing this by
//! dividing, or by assuming ASCII, puts the highlight on the wrong cells the
//! first time an agent prints a box-drawing character.

use vitrum_search::{Matcher, Query};

/// A match, in the coordinates the pane paints in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct RowHit {
    /// Absolute row, counted from the oldest retained row.
    pub row: usize,
    /// First matched column.
    pub start: u16,
    /// One past the last matched column.
    pub end: u16,
}

/// A find in progress.
pub(crate) struct Find {
    query: Query,
    matcher: Matcher,
    hits: Vec<RowHit>,
    /// Index into `hits` of the one the viewport is on.
    current: Option<usize>,
}

impl core::fmt::Debug for Find {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Find")
            .field("pattern", &self.query.pattern.text())
            .field("hits", &self.hits.len())
            .field("current", &self.current)
            .finish()
    }
}

impl Find {
    /// Compile `query`.
    ///
    /// # Errors
    ///
    /// The pattern was empty or, for a regex, did not compile. Both are things
    /// an operator types, so both have to reach the find bar as text rather
    /// than as a silently empty result.
    pub(crate) fn new(query: Query) -> Result<Self, vitrum_search::Error> {
        let matcher = Matcher::compile(&query)?;
        Ok(Self {
            query,
            matcher,
            hits: Vec::new(),
            current: None,
        })
    }

    /// The query being run, which the find bar echoes back.
    pub(crate) const fn query(&self) -> &Query {
        &self.query
    }

    /// Every match found by the last [`Find::scan`], in reading order.
    pub(crate) fn hits(&self) -> &[RowHit] {
        &self.hits
    }

    /// The match the viewport is on, if any.
    pub(crate) fn current(&self) -> Option<RowHit> {
        self.current.and_then(|i| self.hits.get(i).copied())
    }

    /// Which match of how many, for the find bar's counter. One-based, because
    /// it is read by a person.
    pub(crate) fn position(&self) -> Option<(usize, usize)> {
        self.current.map(|i| (i + 1, self.hits.len()))
    }

    /// Run over `rows` and keep every match.
    ///
    /// `rows` yields absolute row indices and their text. Scanning is pulled
    /// rather than pushed so the caller can walk the emulator's scrollback a
    /// page at a time without materialising all of it.
    ///
    /// The current match is preserved across a rescan when the same row and
    /// column still matches, which is what stops the highlight jumping to the
    /// top of the session every time the child prints a line.
    pub(crate) fn scan<'a>(&mut self, rows: impl Iterator<Item = (usize, &'a str)>) {
        let was = self.current();
        self.hits.clear();
        for (row, text) in rows {
            scan_row(&self.matcher, row, text, self.query.all_matches_per_line, &mut self.hits);
        }
        self.current = was
            .and_then(|prev| self.hits.iter().position(|h| *h == prev))
            .or(if self.hits.is_empty() { None } else { Some(0) });
    }

    /// Step to the next match, wrapping at the end.
    ///
    /// Wrapping rather than stopping: a find that goes dead at the last match
    /// makes the operator scroll back to the top by hand, and every editor
    /// they use wraps.
    pub(crate) fn next(&mut self) -> Option<RowHit> {
        if self.hits.is_empty() {
            return None;
        }
        self.current = Some(match self.current {
            Some(i) if i + 1 < self.hits.len() => i + 1,
            _ => 0,
        });
        self.current()
    }

    /// Step to the previous match, wrapping at the start.
    pub(crate) fn previous(&mut self) -> Option<RowHit> {
        if self.hits.is_empty() {
            return None;
        }
        self.current = Some(match self.current {
            Some(0) | None => self.hits.len() - 1,
            Some(i) => i - 1,
        });
        self.current()
    }

    /// Put the cursor on the first match at or after `row`.
    ///
    /// Used when a find starts while the operator is somewhere in the history:
    /// the first match they want is the one in front of them, not the one at
    /// the top of a session that has been running for an hour.
    pub(crate) fn seek_from(&mut self, row: usize) {
        self.current = self
            .hits
            .iter()
            .position(|h| h.row >= row)
            .or(if self.hits.is_empty() { None } else { Some(0) });
    }
}

/// Every match on one row, appended to `out`.
///
/// Separate from [`Find::scan`] so the column mapping is testable on its own,
/// which is the part that goes wrong.
fn scan_row(
    matcher: &Matcher,
    row: usize,
    text: &str,
    all: bool,
    out: &mut Vec<RowHit>,
) {
    let bytes = text.as_bytes();
    // The prefilter exists so a row with no chance of a match costs a memchr
    // rather than a full search. A pane rescans on every frame that changes
    // while a find is open, so this is the difference between a find bar that
    // is free and one that is felt.
    if !matcher.is_possible_match(bytes) {
        return;
    }

    let mut from = 0;
    while let Some(range) = matcher.find_at(bytes, from) {
        // An empty match cannot be advanced past by its own length, and a
        // pattern that can match empty would spin here forever. The matcher
        // refuses an empty pattern, but a regex like `x*` matches empty at
        // every position, so the guard is on the range and not on the pattern.
        let step = if range.end > range.start {
            range.end
        } else {
            range.start + 1
        };

        let start = char_index(text, range.start);
        let end = char_index(text, range.end);
        out.push(RowHit {
            row,
            start: clamp_col(start),
            end: clamp_col(end),
        });

        if !all || step >= bytes.len() {
            break;
        }
        from = step;
    }
}

/// The character index a byte offset falls in.
///
/// A byte offset inside a multi-byte character resolves to that character,
/// because a highlight cannot start in the middle of a cell.
fn char_index(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.chars().count();
    }
    text.char_indices()
        .position(|(i, c)| i <= byte && byte < i + c.len_utf8())
        .unwrap_or_else(|| text.chars().count())
}

/// A column, held inside the width a grid can be.
const fn clamp_col(index: usize) -> u16 {
    if index > u16::MAX as usize {
        u16::MAX
    } else {
        index as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitrum_search::{Haystack, Pattern};

    fn find(q: Query) -> Find {
        Find::new(q).expect("pattern compiles")
    }

    /// The rows of a transcript, as the grid holds them and as the daemon
    /// holds them. Identical, so the only thing that can make the two
    /// disagree is the matcher.
    const ROWS: &[&str] = &[
        "cargo build",
        "error: linker killed",
        "warning: unused import",
        "Error in module",
        "exit 137",
        "the errors are: error, ERROR, erroneous",
    ];

    /// WHY: two definitions of "a match" diverge the first time either is
    /// touched, and the operator gets a hit in the cross-session list that the
    /// pane cannot find. That is the defect this module exists to prevent, and
    /// the only proof is running both.
    ///
    /// The invariant: over the same text, for every query shape the find bar
    /// can produce, the pane's matched substrings are exactly the daemon's.
    /// The query space is walked here rather than exemplified, so a new
    /// matcher path added to `vitrum_search` that the pane does not exercise
    /// is a missing row somebody has to add.
    ///
    /// Does not catch: the seam named in the module doc. The daemon does not
    /// replay cursor motion and the pane does, so text that was overwritten in
    /// place reads differently in the two. That is a difference in the TEXT,
    /// not in the definition of a match, and it is asserted separately by
    /// `the_pane_reads_the_screen_and_the_daemon_reads_the_stream`.
    #[test]
    fn the_pane_and_the_daemon_agree_on_what_a_match_is() {
        let queries = [
            Query::literal("error"),
            Query::literal("error").case_insensitive(true),
            Query::literal("error").whole_word(true),
            Query::literal("error")
                .case_insensitive(true)
                .whole_word(true),
            Query::regex("err(or|oneous)"),
            Query::regex("^e").case_insensitive(true),
            Query::regex(r"\d+"),
            Query::literal("ERROR").case_insensitive(true),
            Query::literal("no such text"),
        ];

        let stream = ROWS.join("\n");
        let bytes = stream.as_bytes();

        for query in queries {
            // Every match on every line, both sides, so a difference in how
            // many are reported per line also shows up.
            let query = query.all_matches_per_line(true).max_hits(10_000);

            let daemon = vitrum_search::search(
                &query,
                &[Haystack {
                    session: 1,
                    base_seq: 0,
                    chunks: std::slice::from_ref(&bytes),
                }],
            )
            .expect("the daemon compiles the same pattern");
            let daemon_texts: Vec<String> =
                daemon.hits.iter().map(vitrum_search::Hit::matched_text).collect();

            let mut pane = find(query.clone());
            pane.scan(ROWS.iter().enumerate().map(|(i, r)| (i, *r)));
            let pane_texts: Vec<String> = pane
                .hits()
                .iter()
                .map(|h| {
                    ROWS[h.row]
                        .chars()
                        .skip(usize::from(h.start))
                        .take(usize::from(h.end - h.start))
                        .collect()
                })
                .collect();

            assert_eq!(
                pane_texts,
                daemon_texts,
                "{:?} disagreed",
                query.pattern.text()
            );
        }
    }

    /// WHY: the seam between the two, stated as a test so nobody closes it by
    /// accident and nobody is surprised by it.
    ///
    /// The daemon scans the byte stream and does not replay cursor motion, so
    /// a progress line that overwrote itself is still searchable there. The
    /// pane scans the grid, where only the last value survives. The direction
    /// matters: the daemon may have a hit the pane does not, never the
    /// reverse, because the pane's text is a subsequence of what was written.
    #[test]
    fn the_pane_reads_the_screen_and_the_daemon_reads_the_stream() {
        let written: &[u8] = b"50%\r100%\n";
        let on_screen = "100%";

        let query = Query::literal("50%");
        let daemon = vitrum_search::search(
            &query,
            &[Haystack {
                session: 1,
                base_seq: 0,
                chunks: std::slice::from_ref(&written),
            }],
        )
        .unwrap();
        assert_eq!(daemon.len(), 1, "the stream still holds the overwritten text");

        let mut pane = find(query);
        pane.scan(core::iter::once((0usize, on_screen)));
        assert!(
            pane.hits().is_empty(),
            "the pane must not find text that is not on the screen"
        );
    }

    /// WHY: a hit is a run of cells to paint, and a pane that divides bytes by
    /// one puts the highlight on the wrong cells the first time an agent
    /// prints a box-drawing character or an emoji, which is every agent.
    ///
    /// The invariant is stated in the grid's own coordinates: the characters
    /// between the reported columns are the matched text.
    #[test]
    fn a_hit_names_the_cells_the_match_is_in_and_not_the_bytes() {
        let cases: &[(&str, &str)] = &[
            ("plain error here", "error"),
            ("\u{2502} error \u{2502}", "error"),
            ("\u{4e2d}\u{6587}error\u{4e2d}\u{6587}", "error"),
            ("\u{1f600}\u{1f600}error", "error"),
            ("\u{e9}\u{e9}\u{e9}error", "error"),
        ];
        for &(row, needle) in cases {
            let mut f = find(Query::literal(needle));
            f.scan(core::iter::once((7usize, row)));
            let hit = f.hits()[0];
            assert_eq!(hit.row, 7);
            let got: String = row
                .chars()
                .skip(usize::from(hit.start))
                .take(usize::from(hit.end - hit.start))
                .collect();
            assert_eq!(got, needle, "in {row:?}");
        }
    }

    /// WHY: a find that goes dead at the last match makes the operator scroll
    /// to the top by hand. Every editor wraps, and a pane that does not is a
    /// pane whose find bar is worse than the one in the text editor beside it.
    #[test]
    fn stepping_wraps_at_both_ends() {
        let mut f = find(
            Query::literal("error")
                .case_insensitive(true)
                .all_matches_per_line(true),
        );
        f.scan(ROWS.iter().enumerate().map(|(i, r)| (i, *r)));
        let n = f.hits().len();
        assert!(n >= 4, "the fixture must have several matches, got {n}");

        assert_eq!(f.position(), Some((1, n)));
        for step in 2..=n {
            f.next();
            assert_eq!(f.position(), Some((step, n)));
        }
        f.next();
        assert_eq!(f.position(), Some((1, n)), "next did not wrap");

        f.previous();
        assert_eq!(f.position(), Some((n, n)), "previous did not wrap");
        f.previous();
        assert_eq!(f.position(), Some((n - 1, n)));
    }

    /// WHY: a rescan happens on every frame that changes while the find bar is
    /// open. Resetting the current match to the top on each one makes the
    /// highlight jump away from what the operator is reading every time the
    /// agent prints a line, which is the flapping seen in
    /// another surface and would be the same defect here.
    #[test]
    fn output_arriving_does_not_move_the_current_match() {
        let mut f = find(Query::literal("error").case_insensitive(true));
        f.scan(ROWS.iter().enumerate().map(|(i, r)| (i, *r)));
        f.next();
        f.next();
        let held = f.current().unwrap();
        let before = f.hits().len();

        // The child prints two more lines; every earlier row is unchanged.
        let mut grown: Vec<&str> = ROWS.to_vec();
        grown.push("still working");
        grown.push("another error appeared");
        f.scan(grown.iter().enumerate().map(|(i, r)| (i, *r)));

        assert_eq!(f.current(), Some(held), "the highlight moved under output");
        assert_eq!(
            f.hits().len(),
            before + 1,
            "the match on the new row was not found"
        );
    }

    /// WHY: a match that scrolls out of retention leaves the cursor pointing
    /// at a hit that no longer exists, and the next step has to land somewhere
    /// real rather than panic or go dead.
    #[test]
    fn losing_the_current_match_falls_back_to_the_first() {
        let mut f = find(Query::literal("error"));
        f.scan(ROWS.iter().enumerate().map(|(i, r)| (i, *r)));
        f.next();
        assert!(f.current().is_some());

        // Only the last row survives, and it does not hold the old hit.
        f.scan(core::iter::once((99usize, ROWS[5])));
        let now = f.current().expect("a surviving match becomes current");
        assert_eq!(now.row, 99);
        assert_eq!(f.position(), Some((1, f.hits().len())));

        // Nothing survives at all.
        f.scan(core::iter::once((0usize, "nothing here")));
        assert!(f.current().is_none());
        assert!(f.position().is_none());
        assert!(f.next().is_none(), "stepping an empty find must be inert");
        assert!(f.previous().is_none());
    }

    /// WHY: a find opened while the operator is deep in history must land on
    /// the match in front of them, not on one from an hour ago.
    #[test]
    fn a_find_starts_from_where_the_operator_is_looking() {
        let mut f = find(Query::literal("error").case_insensitive(true));
        f.scan(ROWS.iter().enumerate().map(|(i, r)| (i, *r)));

        f.seek_from(3);
        assert_eq!(f.current().unwrap().row, 3);
        f.seek_from(5);
        assert_eq!(f.current().unwrap().row, 5);
        f.seek_from(0);
        assert_eq!(f.current().unwrap().row, 1);
        // Past every match, the search wraps to the first rather than going
        // dead, which is the same rule stepping follows.
        f.seek_from(1_000);
        assert_eq!(f.current().unwrap().row, 1);
    }

    /// WHY: a pattern that can match the empty string matches at every
    /// position, and advancing by the match length does not advance at all.
    /// The result is a find bar that hangs the UI thread on a keystroke.
    ///
    /// Assert termination, with a bound: the hits on one row cannot exceed its
    /// characters plus one.
    #[test]
    fn a_pattern_that_matches_nothing_still_terminates() {
        let row = "aaa bbb";
        for pattern in ["a*", "b?", "(?:)|x", "^", "$", r"\b"] {
            let Ok(mut f) = Find::new(
                Query::new(Pattern::Regex(pattern.into())).all_matches_per_line(true),
            ) else {
                continue;
            };
            f.scan(core::iter::once((0usize, row)));
            assert!(
                f.hits().len() <= row.chars().count() + 1,
                "{pattern:?} produced {} hits over {} characters",
                f.hits().len(),
                row.chars().count()
            );
        }
    }

    /// WHY: an operator types into the find bar, so an empty pattern and a
    /// broken regex are ordinary inputs. Both must arrive as text they can act
    /// on rather than as an empty result that looks like "no matches".
    #[test]
    fn an_unusable_pattern_is_refused_rather_than_finding_nothing() {
        assert!(Find::new(Query::literal("")).is_err());
        assert!(Find::new(Query::regex("(unclosed")).is_err());
        assert!(Find::new(Query::regex("[a-")).is_err());
        assert!(Find::new(Query::literal("(unclosed")).is_ok(), "a literal is literal");
    }

    /// WHY: the setting that decides whether a row reports one match or all of
    /// them is the query's, and a pane that always reports one disagrees with
    /// the daemon's count on exactly the rows an operator is looking at.
    #[test]
    fn a_row_reports_one_match_or_all_of_them_as_the_query_asked() {
        let row = "error error error";

        let mut one = find(Query::literal("error"));
        one.scan(core::iter::once((0usize, row)));
        assert_eq!(one.hits().len(), 1);

        let mut all = find(Query::literal("error").all_matches_per_line(true));
        all.scan(core::iter::once((0usize, row)));
        assert_eq!(all.hits().len(), 3);
        assert_eq!(all.hits()[0].start, 0);
        assert_eq!(all.hits()[1].start, 6);
        assert_eq!(all.hits()[2].start, 12);
    }
}
