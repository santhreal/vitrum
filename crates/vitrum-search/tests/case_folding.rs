//! The ASCII case-folding fast path must return exactly what `regex` returns.
//!
//! `Matcher` picks between a SIMD literal search, a byte-wise ASCII fold and
//! the `regex` engine. The operator cannot tell which one ran, so they have to
//! be indistinguishable. A fast path that returns fewer hits is the one failure
//! a search cannot have: you cannot notice the line you were not shown.

use vitrum_search::{Matcher, Query};

/// Corpus lines chosen to exercise the fold: mixed case, the two Unicode
/// characters that fold onto ASCII letters, and text around them.
const LINES: &[&str] = &[
    "Sink the ship",
    "sink the ship",
    "SINK THE SHIP",
    "\u{17f}ink the ship",
    "temperature 300\u{212a} today",
    "temperature 300K today",
    "kelvin",
    "KELVIN",
    "no match here",
    "eSSen",
    "essen",
    "",
    "s",
    "\u{17f}",
];

/// Every case-insensitive literal returns the same hits whichever engine ran.
#[test]
fn the_case_insensitive_fast_path_agrees_with_the_regex_engine() {
    for needle in [
        "sink", "SINK", "Sink", "kelvin", "K", "s", "ship", "the", "essen", "en", "300k",
    ] {
        let matcher = Matcher::compile(&Query::literal(needle).case_insensitive(true))
            .expect("the query compiles");
        let reference = regex::bytes::RegexBuilder::new(&regex::escape(needle))
            .case_insensitive(true)
            .build()
            .expect("the reference compiles");

        for line in LINES {
            let bytes = line.as_bytes();
            assert_eq!(
                matcher.is_match(bytes),
                reference.is_match(bytes),
                "{needle:?} against {line:?} disagrees with the regex engine"
            );
            assert_eq!(
                matcher.find_at(bytes, 0),
                reference.find(bytes).map(|m| m.start()..m.end()),
                "{needle:?} against {line:?} finds a different span"
            );
        }
    }
}

/// `s` and `k` are the whole list of ASCII letters a non-ASCII character folds
/// onto, and the fast path's guard is built on that being true.
///
/// Taken from `regex` itself rather than from a table written here, over every
/// scalar value, so a Unicode revision that adds a third pair fails this test
/// instead of silently narrowing what a search finds.
#[test]
fn only_s_and_k_have_non_ascii_case_fold_partners() {
    let mut found = Vec::new();
    for letter in b'a'..=b'z' {
        let re = regex::bytes::RegexBuilder::new(&(letter as char).to_string())
            .case_insensitive(true)
            .build()
            .expect("a one letter pattern compiles");
        let mut buf = [0u8; 4];
        let partner = (0u32..=0x10FFFF)
            .filter_map(char::from_u32)
            .filter(|ch| !ch.is_ascii())
            .any(|ch| re.is_match(ch.encode_utf8(&mut buf).as_bytes()));
        if partner {
            found.push(letter as char);
        }
    }
    assert_eq!(
        found,
        vec!['k', 's'],
        "the set of ASCII letters with non-ASCII case-fold partners changed"
    );
}
