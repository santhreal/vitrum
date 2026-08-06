//! Counted nouns: `1 session`, `2 sessions`, `20 agents`.
//!
//! English pluralisation is not a rule, it is a lookup, so the plural form is
//! always supplied by the caller rather than guessed. [`count`] takes both
//! forms; [`count_s`] is the shorthand for the regular case where the plural is
//! the singular plus `s`. Nothing here tries to be clever about `-y`/`-ies` or
//! `-s`/`-es`, because a formatter that silently produces `2 branchs` is worse
//! than one that made the caller type the word.
//!
//! Zero takes the plural (`0 sessions`), which is correct English and the only
//! form that reads right in an empty state.
//!
//! Numbers are grouped in thousands with `,` so a scrollback line count is
//! legible at a glance: `12,480 lines`. The separator is fixed rather than
//! locale-derived, so the same session reads identically on every machine that
//! attaches to it.

use std::fmt::{self, Write as _};

/// The correct noun form for `n`, without the number.
#[must_use]
pub fn plural<'a>(n: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if n == 1 { singular } else { plural }
}

/// `n` followed by the correct noun form: `1 session`, `2 sessions`.
#[must_use]
pub fn count(n: u64, singular: &str, plural_form: &str) -> String {
    format!("{} {}", Grouped(n), plural(n, singular, plural_form))
}

/// [`count`] for regular nouns, where the plural is the singular plus `s`.
#[must_use]
pub fn count_s(n: u64, singular: &str) -> String {
    if n == 1 {
        format!("{} {singular}", Grouped(n))
    } else {
        format!("{} {singular}s", Grouped(n))
    }
}

/// A count where zero is worth naming: `no sessions`, `1 session`.
///
/// Empty states read better as words than as a digit.
#[must_use]
pub fn count_or_none(n: u64, singular: &str, plural_form: &str) -> String {
    if n == 0 {
        format!("no {plural_form}")
    } else {
        count(n, singular, plural_form)
    }
}

/// A number that renders grouped in threes: `0`, `999`, `1,000`, `12,480`.
///
/// A [`Display`](fmt::Display) rather than a function returning a `String`, so
/// splicing a count into a sentence costs the sentence's buffer and nothing
/// else. The digits go into a stack array, so this allocates nothing at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Grouped(pub u64);

impl fmt::Display for Grouped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 20 digits is the widest `u64`.
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        let mut value = self.0;
        loop {
            at -= 1;
            digits[at] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        let digits = &digits[at..];
        for (index, digit) in digits.iter().enumerate() {
            // Modulo rather than `is_multiple_of`, which this crate's declared
            // rust-version predates.
            if index > 0 && (digits.len() - index) % 3 == 0 {
                f.write_char(',')?;
            }
            f.write_char(char::from(*digit))?;
        }
        Ok(())
    }
}

/// Decimal digits grouped in threes: `0`, `999`, `1,000`, `12,480`.
#[must_use]
pub fn grouped(n: u64) -> String {
    Grouped(n).to_string()
}
