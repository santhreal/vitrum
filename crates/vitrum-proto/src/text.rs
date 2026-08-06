//! Turning arbitrary bytes of the outside world into something safe to draw.
//!
//! Almost every string the client renders came from somewhere the operator
//! does not control: a branch name in a repository they cloned, a path handed
//! to `--remote`, a command a preset was built from, the reason a spawn failed.
//! Two properties have to hold before any of it reaches a label, a tooltip or
//! an error banner.
//!
//! **It must not be able to forge structure.** The sidebar tooltip is
//! newline-separated and the banner is one line, so a newline inside a value
//! lets that value write what looks like another field. Unicode bidi
//! formatting is the same attack without the newline: U+202E reverses
//! everything after it, so a branch holding one renders as something other
//! than the bytes that were stored, while comparing equal to them. Git permits
//! both in ref names, and a filesystem permits both in a path.
//!
//! **It must be bounded.** An error is a sentence for a person to read. When
//! it is built by formatting the input back at the operator, the input decides
//! how long it is: a 100,000 character command produced a 200,991 character
//! error, measured, which is not a banner but a denial of the banner.

use std::fmt::Write as _;

/// Strip everything that could forge structure, keep everything else.
///
/// Deliberately narrow. Control characters and the bidi formatting range go;
/// every other codepoint stays, because a branch called `функция` or `機能` or
/// `café` is an ordinary branch and a sanitiser that mangles it is its own
/// defect.
pub fn display_safe(s: &str) -> String {
    // Sized up front: the filtered iterator reports a lower bound of zero, so
    // `collect` would grow a long message through a chain of reallocations.
    let mut out = String::with_capacity(s.len());
    out.extend(s.chars().filter(|c| is_display_safe(*c)));
    out
}

/// Whether one character is safe to place in a rendered line.
pub fn is_display_safe(c: char) -> bool {
    !c.is_control()
        && !matches!(
            c,
            '\u{200e}' | '\u{200f}' | '\u{061c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        )
}

/// Longest error text that reaches a client, in characters.
///
/// Sized for the banner rather than for a log: past roughly this much the
/// operator is reading a wall, and the useful part has already scrolled out of
/// the width they have.
pub const MAX_ERROR_CHARS: usize = 240;

/// Sanitise an error and bound it, keeping both ends.
///
/// The elision is in the middle on purpose. These messages are built by
/// context chaining, so the shape is `doing X: doing Y: the actual reason`.
/// Cutting the tail throws away the reason, which is the only part that tells
/// the operator what to change; cutting the head throws away what was being
/// attempted. Removing the middle is the only cut that keeps both.
pub fn error_text(s: &str) -> String {
    let safe = display_safe(s);
    let total = safe.chars().count();
    if total <= MAX_ERROR_CHARS {
        return safe;
    }
    // Weighted towards the tail: the reason is worth more than the preamble.
    let head = MAX_ERROR_CHARS / 3;
    let tail = MAX_ERROR_CHARS - head;
    let dropped = total - head - tail;
    // Both ends are copied as whole slices and the marker is written in place,
    // so a 200 000 character error costs one buffer instead of three.
    let head_end = char_offset(&safe, head);
    let tail_start = char_offset(&safe, total - tail);
    let mut out = String::with_capacity(head_end + 40 + (safe.len() - tail_start));
    out.push_str(&safe[..head_end]);
    write!(out, " … {dropped} more characters … ").expect("writing to a String cannot fail");
    out.push_str(&safe[tail_start..]);
    out
}

/// Byte offset of character number `chars`, or the end of the string.
fn char_offset(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map_or(s.len(), |(at, _)| at)
}

#[cfg(test)]
mod an_error_is_a_sentence_not_a_channel {
    use super::*;

    /// A 100,000 character command produced a 200,991 character error.
    ///
    /// Measured against the running daemon: `createSession` with a command of
    /// 100,000 `A`s came back as an error containing it twice, over the wire
    /// and into a one-line banner. The bound is on the rendered text, not on
    /// the input, because every future error built by formatting is the same
    /// bug again.
    #[test]
    fn a_vast_error_is_cut_down_to_a_banner() {
        let huge = format!("spawning {} in a pty: no such file", "A".repeat(100_000));
        let out = error_text(&huge);
        assert!(
            out.chars().count() < MAX_ERROR_CHARS + 40,
            "error was {} characters",
            out.chars().count()
        );
    }

    /// Cutting the tail would throw away the only actionable part.
    ///
    /// These strings are built by context chaining, so the reason lives at the
    /// end. An operator who cannot see `no such file or directory` has been
    /// told that something failed and nothing else.
    #[test]
    fn both_what_failed_and_why_survive_the_cut() {
        let huge = format!(
            "spawning {} in a pty: no such file or directory",
            "A".repeat(100_000)
        );
        let out = error_text(&huge);
        assert!(out.starts_with("spawning AAA"), "lost what was attempted: {out}");
        assert!(
            out.ends_with("no such file or directory"),
            "lost the reason: {out}"
        );
        assert!(out.contains("more characters"), "no sign of the cut: {out}");
    }

    /// A short error is passed through untouched.
    ///
    /// The bound must be invisible in the case that happens every day, or it
    /// is a second thing to reason about when reading a message.
    #[test]
    fn an_ordinary_error_is_not_touched() {
        let m = "cwd /tmp/nope is not a directory";
        assert_eq!(error_text(m), m);
    }

    /// A path can forge a line in the banner.
    ///
    /// Measured live: a `cwd` containing a newline came back as
    /// `cwd /tmp/evil\nStatus: connected … is not a directory`, and the client
    /// draws that second line as if the daemon had written it.
    #[test]
    fn a_path_cannot_write_its_own_status_line() {
        let out = error_text("cwd /tmp/evil\nStatus: connected is not a directory");
        assert!(!out.contains('\n'), "newline survived: {out:?}");
        assert_eq!(out, "cwd /tmp/evilStatus: connected is not a directory");
    }

    /// A path can reverse the banner.
    ///
    /// U+202E survived into the error text in the same measurement. It is
    /// legal in a filesystem path and in a git ref, so it arrives through
    /// ordinary use of the product, not only through an attack.
    #[test]
    fn a_path_cannot_reverse_the_banner() {
        let out = error_text("spawning ba\u{202e}sh in a pty: nope");
        assert!(!out.contains('\u{202e}'), "override survived: {out:?}");
        assert_eq!(out, "spawning bash in a pty: nope");
        for c in [
            '\u{200e}', '\u{200f}', '\u{061c}', '\u{202a}', '\u{202d}', '\u{2066}', '\u{2069}',
        ] {
            let s = format!("a{c}b");
            assert_eq!(display_safe(&s), "ab", "{c:?} survived");
        }
    }

    /// Sanitising must not mangle the languages people work in.
    ///
    /// The failure mode of a filter written in a hurry is to keep ASCII and
    /// drop the rest, which silently corrupts every non-English branch name
    /// and path in the product.
    #[test]
    fn ordinary_unicode_is_left_alone() {
        for s in [
            "функция",
            "機能/新しい",
            "café-au-lait",
            "emoji 🚀 branch",
            "naïve",
            "ß-straße",
        ] {
            assert_eq!(display_safe(s), s, "mangled {s}");
        }
    }

    /// The cut must never split a character.
    ///
    /// This is the exact bug already fixed once in `git_branch`, where a byte
    /// slice at index 7 landed mid-character and panicked. A bound expressed
    /// in bytes would reintroduce it here, where the input is even less
    /// trusted.
    #[test]
    fn a_multibyte_error_is_cut_without_panicking() {
        for filler in ["é", "機", "🚀", "\u{10FFFF}"] {
            let huge = format!("spawning {} in a pty: nope", filler.repeat(50_000));
            let out = error_text(&huge);
            assert!(out.ends_with("nope"), "{filler} lost the reason");
            assert!(out.chars().count() < MAX_ERROR_CHARS + 40);
        }
    }

    /// Exactly at the bound, and one past it.
    #[test]
    fn the_boundary_itself_behaves() {
        let at = "x".repeat(MAX_ERROR_CHARS);
        assert_eq!(error_text(&at), at);
        let past = "x".repeat(MAX_ERROR_CHARS + 1);
        let out = error_text(&past);
        assert!(out.contains("1 more characters"), "{out}");
        assert!(out.chars().count() < MAX_ERROR_CHARS + 40);
    }

    /// An error made entirely of removed characters becomes empty, not a lie.
    #[test]
    fn an_error_of_pure_control_characters_is_empty() {
        assert_eq!(error_text("\u{202e}\u{200f}\u{0007}\u{0000}"), "");
    }
}
