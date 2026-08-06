//! Turning a [`Query`] into something that can be run against a line.
//!
//! # Two engines, one interface
//!
//! A plain case-sensitive literal — which is what "which agent mentioned OOM"
//! actually is — goes to [`memchr::memmem`], a SIMD substring search that runs
//! at memory bandwidth. Everything else goes to the `regex` crate.
//!
//! The split is drawn where it is for a reason. `regex` already recognises a
//! bare literal and prefilters with the same machinery, so the win from the
//! fast path is modest; but `Regex::find_at` also has to consult a thread-local
//! cache pool on every call, and a scan does one call per line. Over twenty
//! million lines that overhead is the measurement.
//!
//! Whole-word never takes the fast path, even for a literal. Word boundaries
//! must be Unicode-aware — `caf\u{e9}` must not match inside `caf\u{e9}s` — and
//! implementing that a second time next to `regex`'s `\b` is exactly how the
//! two definitions drift apart.
//!
//! # Bytes, not strings
//!
//! Everything here is [`regex::bytes`]. A PTY emits whatever the program wrote,
//! which includes invalid UTF-8: a binary accidentally `cat`ed, a latin-1 log,
//! a buffer cut mid-character by the ring. A `str`-based matcher would have to
//! either reject those lines or lossily transcode them, and lossy transcoding
//! changes byte offsets, which is the one thing a hit's `seq` cannot survive.

use memchr::memmem;
use regex::bytes::{Regex, RegexBuilder};

use crate::error::{Error, Result};
use crate::query::{Pattern, Query};

/// A compiled query, ready to run against lines.
///
/// The finder is boxed because `memmem::Finder` embeds a 256-byte shift table
/// for its Boyer-Moore fallback, which would make every `Matcher` 288 bytes
/// whether or not it is a literal. A `Matcher` is compiled once and used
/// through a reference for millions of lines, so the extra indirection is one
/// L1 hit per line and the smaller type is free everywhere it is stored.
#[derive(Debug)]
pub enum Matcher {
    /// SIMD substring search: literal, case-sensitive, no word boundaries.
    Literal(Box<memmem::Finder<'static>>),
    /// Everything else.
    Regex(Regex),
}

impl Matcher {
    /// Compile `query`.
    pub fn compile(query: &Query) -> Result<Self> {
        if query.pattern.is_empty() {
            return Err(Error::EmptyPattern);
        }

        let fast_path = matches!(query.pattern, Pattern::Literal(_))
            && !query.case_insensitive
            && !query.whole_word;
        if fast_path {
            let needle = query.pattern.text().as_bytes();
            return Ok(Matcher::Literal(Box::new(
                memmem::Finder::new(needle).into_owned(),
            )));
        }

        let body = match &query.pattern {
            // `regex::escape` is what makes a literal search for `a.b` mean
            // `a.b` rather than "a, any character, b".
            Pattern::Literal(text) => regex::escape(text),
            Pattern::Regex(text) => text.clone(),
        };
        // The non-capturing group matters: `\balpha|beta\b` binds the
        // alternation looser than the boundaries and would match `beta` inside
        // `alphabeta`.
        let source = if query.whole_word {
            format!(r"\b(?:{body})\b")
        } else {
            body
        };

        let regex = RegexBuilder::new(&source)
            .case_insensitive(query.case_insensitive)
            // A scrollback line has no newlines in it by construction, but a
            // pattern containing `.` should not be able to reach past one if a
            // caller ever hands us a multi-line buffer.
            .multi_line(false)
            .build()
            .map_err(|source| Error::BadPattern {
                pattern: query.pattern.text().to_string(),
                message: source.to_string(),
            })?;
        Ok(Matcher::Regex(regex))
    }

    /// First match in `haystack` at or after `from`, as a byte range.
    #[inline]
    pub fn find_at(&self, haystack: &[u8], from: usize) -> Option<std::ops::Range<usize>> {
        if from > haystack.len() {
            return None;
        }
        match self {
            Matcher::Literal(finder) => {
                let found = finder.find(&haystack[from..])?;
                Some(from + found..from + found + finder.needle().len())
            }
            // `find_at` rather than slicing so that look-around and `^` see the
            // real start of the line. Searching `&haystack[from..]` would let
            // `^error` match in the middle of a line.
            Matcher::Regex(regex) => regex.find_at(haystack, from).map(|m| m.start()..m.end()),
        }
    }

    /// Does `haystack` contain a match at all?
    #[inline]
    pub fn is_match(&self, haystack: &[u8]) -> bool {
        match self {
            Matcher::Literal(finder) => finder.find(haystack).is_some(),
            Matcher::Regex(regex) => regex.is_match(haystack),
        }
    }

    /// Is this the SIMD fast path?
    pub fn is_fast_literal(&self) -> bool {
        matches!(self, Matcher::Literal(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_all(matcher: &Matcher, haystack: &[u8]) -> Vec<std::ops::Range<usize>> {
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(range) = matcher.find_at(haystack, from) {
            // Advance past a zero-width match so the loop terminates.
            from = if range.end > range.start {
                range.end
            } else {
                range.end + 1
            };
            out.push(range);
        }
        out
    }

    /// Locks out the fast path being taken when it would be wrong. A
    /// case-insensitive or whole-word search on the memmem path would silently
    /// ignore the option.
    #[test]
    fn fast_path_is_taken_only_for_a_plain_literal() {
        assert!(
            Matcher::compile(&Query::literal("OOM"))
                .unwrap()
                .is_fast_literal()
        );
        assert!(
            !Matcher::compile(&Query::literal("OOM").case_insensitive(true))
                .unwrap()
                .is_fast_literal()
        );
        assert!(
            !Matcher::compile(&Query::literal("OOM").whole_word(true))
                .unwrap()
                .is_fast_literal()
        );
        assert!(
            !Matcher::compile(&Query::regex("OOM"))
                .unwrap()
                .is_fast_literal()
        );
    }

    /// Locks out regex metacharacters being honoured in a literal search. A
    /// user searching for `a.b` in a log of version numbers must not match
    /// `a1b`, and one searching for `[ok]` must not get an empty result because
    /// it parsed as a character class.
    #[test]
    fn literal_patterns_do_not_honour_metacharacters() {
        let dotted = Matcher::compile(&Query::literal("a.b")).unwrap();
        assert!(dotted.is_match(b"xxa.byy"));
        assert!(!dotted.is_match(b"xxaXbyy"));

        // Also on the non-fast path, where escaping is what does the work.
        let folded = Matcher::compile(&Query::literal("a.b").case_insensitive(true)).unwrap();
        assert!(folded.is_match(b"A.B"));
        assert!(!folded.is_match(b"AXB"));

        let bracketed = Matcher::compile(&Query::literal("[ok]")).unwrap();
        assert!(bracketed.is_match(b"result [ok] done"));
        assert!(!bracketed.is_match(b"result o done"));
    }

    /// Locks out the literal fast path reporting the wrong end offset, which
    /// would make a hit highlight the wrong bytes and truncate the mapped
    /// original range.
    #[test]
    fn literal_ranges_cover_exactly_the_needle() {
        let matcher = Matcher::compile(&Query::literal("error")).unwrap();
        assert_eq!(
            find_all(&matcher, b"an error and another error here"),
            vec![3..8, 21..26]
        );
    }

    /// Locks out case folding being applied to only one side. `OOM` must find
    /// `oom` and `oom` must find `OOM`.
    #[test]
    fn case_insensitive_matches_in_both_directions() {
        let upper = Matcher::compile(&Query::literal("OOM").case_insensitive(true)).unwrap();
        assert!(upper.is_match(b"killed: oom"));
        assert!(upper.is_match(b"killed: OOM"));
        assert!(upper.is_match(b"killed: OoM"));

        let lower = Matcher::compile(&Query::literal("oom").case_insensitive(true)).unwrap();
        assert!(lower.is_match(b"OOM killer"));
    }

    /// Locks out case sensitivity being on when it was not asked for, which
    /// would flood a search for `Error` with every lowercase `error`.
    #[test]
    fn case_sensitive_is_the_default() {
        let matcher = Matcher::compile(&Query::literal("OOM")).unwrap();
        assert!(matcher.is_match(b"OOM"));
        assert!(!matcher.is_match(b"oom"));
    }

    /// Locks out whole-word matching inside a longer word, which is the entire
    /// point of the option: `cat` must not match `concatenate`.
    #[test]
    fn whole_word_rejects_matches_inside_longer_words() {
        let matcher = Matcher::compile(&Query::literal("cat").whole_word(true)).unwrap();
        assert!(matcher.is_match(b"the cat sat"));
        assert!(matcher.is_match(b"(cat)"));
        assert!(matcher.is_match(b"cat"));
        assert!(!matcher.is_match(b"concatenate"));
        assert!(!matcher.is_match(b"cats"));
        assert!(!matcher.is_match(b"scat"));
    }

    /// Locks out an underscore or a digit counting as a boundary. `err` must
    /// not match inside `err_code` or `err2`, which are identifiers a user
    /// searching for the word `err` does not mean.
    #[test]
    fn whole_word_treats_underscores_and_digits_as_word_characters() {
        let matcher = Matcher::compile(&Query::literal("err").whole_word(true)).unwrap();
        assert!(!matcher.is_match(b"err_code"));
        assert!(!matcher.is_match(b"err2"));
        assert!(!matcher.is_match(b"2err"));
        assert!(matcher.is_match(b"an err here"));
        assert!(matcher.is_match(b"err-code"));
    }

    /// Locks out ASCII-only word boundaries. With `(?-u:\b)` an accented word
    /// followed by a space has no boundary at all, so a whole-word search for
    /// a non-ASCII term silently returns nothing.
    #[test]
    fn whole_word_boundaries_are_unicode_aware() {
        let matcher = Matcher::compile(&Query::literal("caf\u{e9}").whole_word(true)).unwrap();
        assert!(matcher.is_match("le caf\u{e9} est".as_bytes()));
        assert!(matcher.is_match("caf\u{e9}".as_bytes()));
        assert!(!matcher.is_match("caf\u{e9}s".as_bytes()));
    }

    /// Locks out an alternation binding looser than the word boundaries. With
    /// `\balpha|beta\b` the pattern means "`\balpha`" or "`beta\b`", so
    /// `alphabeta` matches — which is precisely what whole-word forbids.
    #[test]
    fn whole_word_groups_an_alternation_before_anchoring_it() {
        let matcher = Matcher::compile(&Query::regex("alpha|beta").whole_word(true)).unwrap();
        assert!(matcher.is_match(b"an alpha here"));
        assert!(matcher.is_match(b"a beta here"));
        assert!(!matcher.is_match(b"alphabeta"));
        assert!(!matcher.is_match(b"xalphax"));
    }

    /// Locks out `find_at` searching a slice instead of using a real offset.
    /// Slicing makes `^` match in the middle of a line, so `^error` would fire
    /// on `an error` at the second match position.
    #[test]
    fn anchors_see_the_true_start_of_the_line() {
        let matcher = Matcher::compile(&Query::regex("^error")).unwrap();
        assert_eq!(matcher.find_at(b"error here", 0), Some(0..5));
        assert_eq!(matcher.find_at(b"an error here", 0), None);
        // Even when told to start after the beginning, `^` must not re-anchor.
        assert_eq!(matcher.find_at(b"an error here", 3), None);
    }

    /// Locks out a `$` anchor matching before the end. The stripper removes a
    /// trailing CR precisely so this works on CRLF output.
    #[test]
    fn end_anchors_match_at_the_end_of_the_line() {
        let matcher = Matcher::compile(&Query::regex(r"failed$")).unwrap();
        assert!(matcher.is_match(b"build failed"));
        assert!(!matcher.is_match(b"build failed later"));
    }

    /// Locks out an invalid regex panicking or being silently accepted as a
    /// literal. A user typing `(unclosed` must get a message, not a crash and
    /// not zero results.
    #[test]
    fn an_invalid_regex_reports_a_usable_error() {
        let error = Matcher::compile(&Query::regex("(unclosed")).expect_err("must reject");
        match error {
            Error::BadPattern { pattern, message } => {
                assert_eq!(pattern, "(unclosed");
                assert!(
                    message.contains("unclosed") || message.contains("group"),
                    "message should explain the problem: {message}"
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// Locks out an empty pattern being compiled, which matches at every
    /// position and would return the whole scrollback of every session.
    #[test]
    fn an_empty_pattern_is_rejected() {
        assert!(matches!(
            Matcher::compile(&Query::literal("")),
            Err(Error::EmptyPattern)
        ));
        assert!(matches!(
            Matcher::compile(&Query::regex("")),
            Err(Error::EmptyPattern)
        ));
    }

    /// Locks out invalid UTF-8 in the scrollback aborting a search. A PTY
    /// carries whatever the program wrote, including a truncated character at
    /// a ring boundary and the contents of an accidentally `cat`ed binary.
    #[test]
    fn invalid_utf8_in_the_haystack_is_searchable() {
        //                  0..6    7  8   9..16    17..21
        let haystack = b"before \xff\xfe broken error after";
        assert_eq!(&haystack[17..22], b"error");
        assert!(
            Matcher::compile(&Query::literal("error"))
                .unwrap()
                .is_match(haystack)
        );
        assert!(
            Matcher::compile(&Query::regex("err.r"))
                .unwrap()
                .is_match(haystack)
        );
        assert_eq!(
            Matcher::compile(&Query::literal("error"))
                .unwrap()
                .find_at(haystack, 0),
            Some(17..22)
        );
        // The regex engine must agree with the SIMD path on the offset, or a
        // hit's seq depends on which engine happened to run.
        assert_eq!(
            Matcher::compile(&Query::regex("err.r"))
                .unwrap()
                .find_at(haystack, 0),
            Some(17..22)
        );
    }

    /// Locks out `find_at` panicking when the resume offset is at or past the
    /// end of the line, which happens on the last match of a line.
    #[test]
    fn resuming_at_or_past_the_end_is_none_not_a_panic() {
        let matcher = Matcher::compile(&Query::literal("x")).unwrap();
        assert_eq!(matcher.find_at(b"abx", 3), None);
        assert_eq!(matcher.find_at(b"abx", 4), None);
        assert_eq!(matcher.find_at(b"", 0), None);

        let regex = Matcher::compile(&Query::regex("x")).unwrap();
        assert_eq!(regex.find_at(b"abx", 3), None);
        assert_eq!(regex.find_at(b"abx", 99), None);
    }

    /// Locks out overlapping literal matches being reported. `aa` in `aaaa` is
    /// two matches, not three, and a scan that reported three would loop
    /// differently from the regex path.
    #[test]
    fn repeated_literal_matches_do_not_overlap() {
        let matcher = Matcher::compile(&Query::literal("aa")).unwrap();
        assert_eq!(find_all(&matcher, b"aaaa"), vec![0..2, 2..4]);
    }

    /// Locks out a zero-width regex hanging the scan. `a*` matches empty
    /// everywhere and the caller's advance logic must survive it.
    #[test]
    fn zero_width_matches_terminate() {
        let matcher = Matcher::compile(&Query::regex("x*")).unwrap();
        let found = find_all(&matcher, b"abc");
        assert_eq!(found, vec![0..0, 1..1, 2..2, 3..3]);
    }
}
