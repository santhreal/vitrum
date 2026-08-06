//! The two ways a search can be refused.
//!
//! Both are the caller's pattern being unusable, and both are detected once at
//! compile time rather than per line. Nothing that happens *during* a scan can
//! fail: the input is bytes, every byte sequence is searchable, and a haystack
//! that is empty or truncated or full of invalid UTF-8 is a legitimate haystack
//! with no matches in it. That is deliberate — a search across twenty sessions
//! must not be aborted because one of them holds a half-written binary.

use std::fmt;

/// This crate's result type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A search that could not be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The pattern was empty.
    ///
    /// An empty pattern matches at every position of every line, so honouring
    /// it would return the entire scrollback of every session — hundreds of
    /// megabytes, in answer to a keystroke in a search box that the user has
    /// not finished typing into.
    EmptyPattern,
    /// The regular expression did not compile.
    BadPattern { pattern: String, message: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::EmptyPattern => write!(f, "search pattern is empty"),
            Error::BadPattern { pattern, message } => {
                write!(f, "cannot compile {pattern:?}: {message}")
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks out a bad-pattern message that hides either the pattern or the
    /// reason. It is rendered under a search box while the user is typing, and
    /// both halves are what makes it actionable.
    #[test]
    fn bad_pattern_message_names_the_pattern_and_the_reason() {
        let error = Error::BadPattern {
            pattern: "(unclosed".to_string(),
            message: "unclosed group".to_string(),
        };
        assert_eq!(
            error.to_string(),
            "cannot compile \"(unclosed\": unclosed group"
        );
    }

    /// Locks out the empty-pattern refusal rendering as a blank or panicking
    /// message, which is what a search box shows before the first keystroke.
    #[test]
    fn empty_pattern_message_is_plain_and_short() {
        assert_eq!(Error::EmptyPattern.to_string(), "search pattern is empty");
    }
}
