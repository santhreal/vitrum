//! What to look for, and how much of the answer to keep.
//!
//! A query is data, not a compiled object: it is what a client sends over the
//! wire and what a test writes by hand. Compilation happens once per search in
//! [`crate::matcher`].
//!
//! # Caps
//!
//! Two caps, because one is not enough. [`Query::max_hits`] bounds the whole
//! answer so a search for `e` across twenty sessions cannot return two million
//! rows. [`Query::max_hits_per_session`] bounds each session's share, so one
//! chatty session cannot fill the entire budget before the others are looked
//! at. Without the second cap, "which of my agents mentioned OOM" answers
//! "session 3, four hundred times" and never reaches session 4.

/// What to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Match these exact characters. Regex metacharacters are literal.
    Literal(String),
    /// Match this regular expression, in the `regex` crate's syntax.
    Regex(String),
}

impl Pattern {
    /// The pattern text, whichever kind it is.
    pub fn text(&self) -> &str {
        match self {
            Pattern::Literal(text) | Pattern::Regex(text) => text,
        }
    }

    /// Is this pattern empty?
    ///
    /// An empty literal matches at every position and an empty regex matches
    /// every line; both are rejected rather than returning the entire
    /// scrollback of every session.
    pub fn is_empty(&self) -> bool {
        self.text().is_empty()
    }
}

/// Longest context run this crate will carry, in lines, on each side.
///
/// The before-context ring is sized from this, and a client that asks for a
/// thousand lines of context is asking for the scrollback, not for a search.
pub const MAX_CONTEXT: usize = 64;

/// Line text one search may return, in bytes, across every hit.
///
/// The hit and context caps bound how many rows come back; they do not bound
/// how big a row is. A session may hold ten megabytes of one-kilobyte lines,
/// and a pattern matching every one of them then returns ten thousand hits
/// each carrying its line twice plus sixty-four context lines on each side:
/// measured at 1.27 GB of heap from a 10 MiB ring, before the answer is
/// projected onto the wire, which copies it again. Every factor in that
/// product is chosen by the client, so the product needs a cap of its own.
///
/// Eight megabytes is far more than a result list anyone reads and far less
/// than one session's ring. A sweep that reaches it stops and reports
/// `truncated`, exactly as it does for the hit cap.
pub const DEFAULT_MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;

/// A search request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// What to look for.
    pub pattern: Pattern,
    /// Fold case. Applies to the pattern and the scrollback alike.
    pub case_insensitive: bool,
    /// Require a word boundary on both sides of the match.
    ///
    /// Boundaries are Unicode-aware, so `caf\u{e9}` does not match inside
    /// `caf\u{e9}s`.
    pub whole_word: bool,
    /// Lines of context to return before each hit. Clamped to [`MAX_CONTEXT`].
    pub context_before: usize,
    /// Lines of context to return after each hit. Clamped to [`MAX_CONTEXT`].
    pub context_after: usize,
    /// Total hits to return across every session.
    pub max_hits: usize,
    /// Hits to return from any one session.
    pub max_hits_per_session: usize,
    /// Report every match on a line, not just the first.
    ///
    /// Off by default: a log line that says `error` six times is one finding,
    /// and six rows of identical context is noise.
    pub all_matches_per_line: bool,
    /// Line text this search may return in total, in bytes.
    ///
    /// The other caps count rows. This one counts what the rows weigh, which
    /// is the only quantity the client does not also choose. Defaults to
    /// [`DEFAULT_MAX_ANSWER_BYTES`].
    pub max_answer_bytes: usize,
}

impl Query {
    /// A literal search with sensible defaults.
    pub fn literal(text: impl Into<String>) -> Self {
        Self::new(Pattern::Literal(text.into()))
    }

    /// A regex search with sensible defaults.
    pub fn regex(text: impl Into<String>) -> Self {
        Self::new(Pattern::Regex(text.into()))
    }

    /// A query with the given pattern and default everything else.
    pub fn new(pattern: Pattern) -> Self {
        Self {
            pattern,
            case_insensitive: false,
            whole_word: false,
            context_before: 2,
            context_after: 2,
            max_hits: 1_000,
            max_hits_per_session: 200,
            all_matches_per_line: false,
            max_answer_bytes: DEFAULT_MAX_ANSWER_BYTES,
        }
    }

    pub fn case_insensitive(mut self, yes: bool) -> Self {
        self.case_insensitive = yes;
        self
    }

    pub fn whole_word(mut self, yes: bool) -> Self {
        self.whole_word = yes;
        self
    }

    /// Set both context sides at once.
    pub fn context(mut self, lines: usize) -> Self {
        self.context_before = lines;
        self.context_after = lines;
        self
    }

    pub fn context_before(mut self, lines: usize) -> Self {
        self.context_before = lines;
        self
    }

    pub fn context_after(mut self, lines: usize) -> Self {
        self.context_after = lines;
        self
    }

    pub fn max_hits(mut self, hits: usize) -> Self {
        self.max_hits = hits;
        self
    }

    pub fn max_hits_per_session(mut self, hits: usize) -> Self {
        self.max_hits_per_session = hits;
        self
    }

    /// Cap the line text the answer may carry, in bytes.
    pub fn max_answer_bytes(mut self, bytes: usize) -> Self {
        self.max_answer_bytes = bytes;
        self
    }

    pub fn all_matches_per_line(mut self, yes: bool) -> Self {
        self.all_matches_per_line = yes;
        self
    }

    /// Context before, clamped to what this crate will carry.
    pub fn effective_context_before(&self) -> usize {
        self.context_before.min(MAX_CONTEXT)
    }

    /// Context after, clamped to what this crate will carry.
    pub fn effective_context_after(&self) -> usize {
        self.context_after.min(MAX_CONTEXT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks out a default query with unbounded results. A search dispatched by
    /// a client that set no caps must still be safe against twenty sessions of
    /// ten megabytes.
    #[test]
    fn defaults_are_bounded_on_every_axis() {
        let query = Query::literal("OOM");
        assert_eq!(query.max_hits, 1_000);
        assert_eq!(query.max_hits_per_session, 200);
        assert_eq!(query.context_before, 2);
        assert_eq!(query.context_after, 2);
        assert!(!query.case_insensitive);
        assert!(!query.whole_word);
        assert!(!query.all_matches_per_line);
    }

    /// Locks out an unclamped context request allocating a ring of whatever
    /// size the client asked for.
    #[test]
    fn context_is_clamped_to_the_documented_maximum() {
        let query = Query::literal("x").context(10_000);
        assert_eq!(query.context_before, 10_000);
        assert_eq!(query.effective_context_before(), MAX_CONTEXT);
        assert_eq!(query.effective_context_after(), MAX_CONTEXT);

        let small = Query::literal("x").context(3);
        assert_eq!(small.effective_context_before(), 3);
        assert_eq!(small.effective_context_after(), 3);
    }

    /// Locks out asymmetric context being collapsed. `-A 5 -B 0` is a normal
    /// thing to want and the two sides must stay independent.
    #[test]
    fn context_sides_are_independent() {
        let query = Query::literal("x").context_before(0).context_after(5);
        assert_eq!(query.effective_context_before(), 0);
        assert_eq!(query.effective_context_after(), 5);
    }

    /// Locks out a builder method quietly overwriting another, which would
    /// silently drop the caller's case-insensitivity or their cap.
    #[test]
    fn builder_methods_compose_without_clobbering() {
        let query = Query::regex("err(or)?")
            .case_insensitive(true)
            .whole_word(true)
            .context(4)
            .max_hits(50)
            .max_hits_per_session(7)
            .all_matches_per_line(true);
        assert_eq!(query.pattern, Pattern::Regex("err(or)?".to_string()));
        assert!(query.case_insensitive);
        assert!(query.whole_word);
        assert_eq!(query.context_before, 4);
        assert_eq!(query.context_after, 4);
        assert_eq!(query.max_hits, 50);
        assert_eq!(query.max_hits_per_session, 7);
        assert!(query.all_matches_per_line);
    }

    /// Locks out a literal being built as a regex or vice versa, which would
    /// make a search for `a.b` match `axb` — or make `a.b` as a regex be
    /// escaped into uselessness.
    #[test]
    fn constructors_pick_the_right_pattern_kind() {
        assert_eq!(
            Query::literal("a.b").pattern,
            Pattern::Literal("a.b".to_string())
        );
        assert_eq!(
            Query::regex("a.b").pattern,
            Pattern::Regex("a.b".to_string())
        );
    }

    /// Locks out an empty pattern slipping past the emptiness test, which is
    /// the guard that stops a search returning every line of every session.
    #[test]
    fn empty_patterns_report_as_empty() {
        assert!(Pattern::Literal(String::new()).is_empty());
        assert!(Pattern::Regex(String::new()).is_empty());
        assert!(!Pattern::Literal(" ".to_string()).is_empty());
        assert_eq!(Pattern::Regex("x+".to_string()).text(), "x+");
    }
}
