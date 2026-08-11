//! A pattern must not be able to spend the daemon's memory to compile.
//!
//! Compilation happens on the connection that asked, inside the process that
//! is also pumping every session's output, and the search box compiles on
//! every keystroke. The `regex` crate's default program ceiling is ten
//! megabytes, which a fourteen-character pattern reaches, so the default is a
//! ceiling on the wrong side of usable.
//!
//! The contract is not "small patterns work". It is that an expansion the
//! operator did not mean is refused by name, with the limit in the message,
//! while every pattern a person types still compiles.

use vitrum_search::{Matcher, Query};

/// Patterns whose compiled program is enormous relative to their source.
///
/// Each is a bounded repetition nested inside another, which the engine
/// expands rather than loops: the source is characters and the program is
/// hundreds of kilobytes.
///
/// Every one of these is chosen to sit in the band the fix actually moved.
/// The `regex` crate refuses `[\s\S]{2000}{2000}` on its own, so a test built
/// from patterns like that passes whether or not this crate sets a limit and
/// proves nothing. These three compile under the ten-megabyte default and are
/// refused under this crate's ceiling, which is the whole of the change.
const EXPANDING: &[&str] = &[
    r"(?:a{1000}){50}",
    r"(?:a{1000}){200}",
    r"(?:(?:ab|cd){100}){100}",
];

/// Patterns an operator types while looking for something in a scrollback.
///
/// These are the reason the limit is a limit and not a ban. Every one of them
/// must still compile, or the fix for a denial of service is a denial of
/// search.
const ORDINARY: &[&str] = &[
    r"error",
    r"^\s*(warning|error|fatal):",
    r"thread '.*' panicked at",
    r"\bE\d{4}\b",
    r"(?i)out of memory|oom.killer",
    r"0x[0-9a-f]{8,16}",
    r"a{1000}",
    r"[\w.-]+@[\w.-]+",
];

/// WHY: `RegexBuilder` ran with the default ten-megabyte program ceiling, so a
/// pattern that expands could make the daemon build and hold ten megabytes per
/// keystroke on a connection thread shared with every session's output pump.
///
/// Refused, by name, with the ceiling in the message so the operator knows the
/// pattern is the problem and not the machine.
///
/// What this does not catch: a pattern that compiles small and runs slowly.
/// `dfa_size_limit` bounds that one's memory but not its time.
#[test]
fn an_expanding_pattern_is_refused_rather_than_compiled() {
    for pattern in EXPANDING {
        let query = Query::regex(*pattern);
        let error = match Matcher::compile(&query) {
            Ok(_) => panic!("{pattern:?} compiled; the program ceiling is not applied"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains(&format!("{pattern:?}")),
            "the refusal must name the pattern: {error}"
        );
        assert!(
            error.contains("size limit"),
            "the refusal must name what was exceeded: {error}"
        );
    }
}

/// The limit must not cost the operator a pattern they would really type.
#[test]
fn every_ordinary_pattern_still_compiles() {
    for pattern in ORDINARY {
        assert!(
            Matcher::compile(&Query::regex(*pattern)).is_ok(),
            "{pattern:?} is a pattern a person types and it was refused"
        );
    }
}

/// A refused pattern must leave the matcher unusable rather than fall back to
/// a literal search.
///
/// Silently searching for the pattern's own text answers a different question
/// and looks like it succeeded, which is worse than refusing: the operator
/// reads "no results" as "my agents never mentioned this".
#[test]
fn a_refused_pattern_is_not_downgraded_to_a_literal_search() {
    let query = Query::regex(EXPANDING[0]);
    assert!(
        Matcher::compile(&query).is_err(),
        "a refused pattern must not produce a matcher at all"
    );
}
