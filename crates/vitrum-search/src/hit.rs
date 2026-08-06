//! What a search returns.
//!
//! # Two views of every line
//!
//! Each hit carries the matching line twice: [`Hit::line`] is the original
//! bytes with every escape sequence intact, and [`Hit::visible`] is the text
//! that was matched. That is not redundancy, it is the whole contract.
//!
//! The client renders `line`, so the result row looks exactly like the
//! scrollback it came from — red errors stay red. The matcher worked on
//! `visible`, so [`Hit::visible_range`] indexes into that and nothing else. And
//! [`Hit::match_seq`] is in the *original* coordinate system, because `seq` is
//! the data plane's cumulative byte offset and a client that wants to scroll
//! the session to this hit needs the offset of the byte the terminal actually
//! wrote.
//!
//! Getting those three coordinate systems confused is the bug this crate is
//! most likely to have, so they are named apart rather than left to convention.
//!
//! # Ordering
//!
//! [`SearchResults::hits`] is sorted by `(session, line_seq, match_seq)` and
//! that order does not depend on the order the haystacks were supplied. Two
//! identical searches over the same scrollback return byte-identical results,
//! which is what makes "page 2" of a result list mean anything.

use std::ops::Range;

/// One line of context around a hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextLine {
    /// Stream offset of this line's first byte.
    pub seq: u64,
    /// Zero-based line index within the session's searched scrollback.
    pub index: u64,
    /// Original bytes, escape sequences intact, newline excluded.
    pub bytes: Vec<u8>,
}

impl ContextLine {
    /// The line as text, with invalid UTF-8 replaced.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// One match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Session the match was found in.
    pub session: u64,
    /// Stream offset of the first byte of the matching line.
    pub line_seq: u64,
    /// Stream offset of the first byte of the match itself.
    ///
    /// In original bytes: if the match sits after three SGR sequences, this
    /// counts those sequences.
    pub match_seq: u64,
    /// The match's extent in original bytes, relative to the line start.
    ///
    /// Ends at the last matched byte, so a trailing reset sequence is excluded.
    pub original_range: Range<usize>,
    /// The match's extent within [`Hit::visible`].
    pub visible_range: Range<usize>,
    /// Zero-based line index within the session's searched scrollback.
    pub line_index: u64,
    /// The matching line, original bytes, newline excluded.
    pub line: Vec<u8>,
    /// The matching line with escape sequences removed: exactly the bytes the
    /// matcher ran against, and the coordinate system [`Hit::visible_range`]
    /// indexes.
    ///
    /// Bytes rather than a `String` on purpose. A PTY carries whatever the
    /// program wrote, so the visible text is not always valid UTF-8, and a
    /// lossy conversion changes lengths — one stray `0xFF` becomes a
    /// three-byte replacement character and every offset after it in the line
    /// shifts by two. Storing the string would silently invalidate
    /// `visible_range` on exactly the lines that are hardest to debug.
    /// [`Hit::visible_lossy`] does the conversion when a caller wants text.
    pub visible: Vec<u8>,
    /// Preceding context, oldest first.
    pub before: Vec<ContextLine>,
    /// Following context, oldest first.
    pub after: Vec<ContextLine>,
}

impl Hit {
    /// The matched text itself, lossily decoded from the visible line.
    pub fn matched_text(&self) -> String {
        String::from_utf8_lossy(self.matched_bytes()).into_owned()
    }

    /// The matched bytes, exactly as the matcher saw them.
    pub fn matched_bytes(&self) -> &[u8] {
        // Always in bounds for a hit this crate produced; `get` keeps a
        // hand-built `Hit` in a caller's test from panicking.
        self.visible
            .get(self.visible_range.clone())
            .unwrap_or_default()
    }

    /// The visible line as text, with invalid UTF-8 replaced.
    pub fn visible_lossy(&self) -> String {
        String::from_utf8_lossy(&self.visible).into_owned()
    }

    /// The matching line as text, with invalid UTF-8 replaced.
    pub fn line_to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.line).into_owned()
    }

    /// Sort key: session, then position in that session's stream.
    pub fn order_key(&self) -> (u64, u64, u64) {
        (self.session, self.line_seq, self.match_seq)
    }
}

/// Everything a search found, plus what it cost.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchResults {
    /// Matches, ordered by `(session, line_seq, match_seq)`.
    pub hits: Vec<Hit>,
    /// A cap stopped the search before it ran out of scrollback.
    ///
    /// When true, `hits` is a prefix of the full answer in the documented
    /// order, not an arbitrary subset.
    pub truncated: bool,
    /// Bytes actually examined. Less than the total when `truncated`.
    pub bytes_scanned: u64,
    /// Lines actually examined.
    pub lines_scanned: u64,
    /// Sessions that contributed at least one hit.
    pub sessions_hit: usize,
}

impl SearchResults {
    /// How many matches were returned.
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// Did the search find anything?
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// Throughput helper: megabytes examined.
    pub fn megabytes_scanned(&self) -> f64 {
        self.bytes_scanned as f64 / (1024.0 * 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(session: u64, line_seq: u64, match_seq: u64) -> Hit {
        Hit {
            session,
            line_seq,
            match_seq,
            original_range: 0..5,
            visible_range: 0..5,
            line_index: 0,
            line: b"error".to_vec(),
            visible: b"error".to_vec(),
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    /// Locks out an order key that sorts by match position before session,
    /// which would interleave twenty agents' output and make the result list
    /// unreadable.
    #[test]
    fn order_key_puts_session_first() {
        let mut hits = [hit(2, 10, 10), hit(1, 900, 900), hit(2, 5, 5), hit(1, 3, 3)];
        hits.sort_by_key(Hit::order_key);
        assert_eq!(
            hits.iter()
                .map(|h| (h.session, h.line_seq))
                .collect::<Vec<_>>(),
            vec![(1, 3), (1, 900), (2, 5), (2, 10)]
        );
    }

    /// Locks out two matches on one line ordering by anything but their
    /// position in the line.
    #[test]
    fn order_key_breaks_line_ties_by_match_position() {
        let mut hits = [hit(1, 100, 140), hit(1, 100, 105)];
        hits.sort_by_key(Hit::order_key);
        assert_eq!(
            hits.iter().map(|h| h.match_seq).collect::<Vec<_>>(),
            vec![105, 140]
        );
    }

    /// Locks out `visible_range` being interpreted against a lossily decoded
    /// string. One `0xFF` in the line becomes a three-byte replacement
    /// character, and every offset after it shifts by two — so the reported
    /// match text would be the wrong substring, silently, on exactly the lines
    /// that are hardest to reason about.
    #[test]
    fn matched_bytes_are_exact_even_when_the_line_is_not_utf8() {
        let mut broken = hit(1, 0, 0);
        broken.visible = b"a\xffb OOM tail".to_vec();
        broken.visible_range = 4..7;
        assert_eq!(broken.matched_bytes(), b"OOM");
        assert_eq!(broken.matched_text(), "OOM");
        // The lossy rendering is three bytes longer, which is exactly why the
        // range must not be applied to it.
        assert_eq!(broken.visible_lossy(), "a\u{fffd}b OOM tail");
        assert_eq!(broken.visible_lossy().len(), broken.visible.len() + 2);
    }

    /// Locks out a hand-built `Hit` with an out-of-range slice panicking
    /// inside a caller's own test.
    #[test]
    fn an_out_of_range_visible_range_yields_empty_rather_than_panicking() {
        let mut broken = hit(1, 0, 0);
        broken.visible_range = 3..99;
        assert_eq!(broken.matched_bytes(), b"");
        assert_eq!(broken.matched_text(), "");
    }

    /// Locks out the matched text being taken from the whole line rather than
    /// the match, which would make every result row show the entire line twice.
    #[test]
    fn matched_text_is_the_match_not_the_line() {
        let mut good = hit(1, 0, 0);
        good.visible = b"an error here".to_vec();
        good.visible_range = 3..8;
        assert_eq!(good.matched_text(), "error");
        assert_eq!(good.visible_lossy(), "an error here");
    }

    /// Locks out a lossy renderer that drops bytes instead of replacing them,
    /// which would shift every column in the displayed line.
    #[test]
    fn lossy_rendering_replaces_rather_than_drops() {
        let line = ContextLine {
            seq: 0,
            index: 0,
            bytes: b"ok \xff done".to_vec(),
        };
        assert_eq!(line.to_string_lossy(), "ok \u{fffd} done");

        let mut broken = hit(1, 0, 0);
        broken.line = b"\xffbad".to_vec();
        assert_eq!(broken.line_to_string_lossy(), "\u{fffd}bad");
    }

    /// Locks out an empty result being reported as non-empty, which is what a
    /// "no matches" placeholder keys off.
    #[test]
    fn empty_results_report_as_empty() {
        let results = SearchResults::default();
        assert!(results.is_empty());
        assert_eq!(results.len(), 0);
        assert!(!results.truncated);
        assert_eq!(results.bytes_scanned, 0);
        assert_eq!(results.megabytes_scanned(), 0.0);
    }

    /// Locks out the megabyte conversion using 1000 rather than 1024, which
    /// would misreport the benchmark by 4.9%.
    #[test]
    fn megabytes_use_binary_units() {
        let results = SearchResults {
            bytes_scanned: 10 * 1024 * 1024,
            ..SearchResults::default()
        };
        assert_eq!(results.megabytes_scanned(), 10.0);
    }
}
