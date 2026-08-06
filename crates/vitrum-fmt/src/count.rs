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

/// The correct noun form for `n`, without the number.
#[must_use]
pub fn plural<'a>(n: u64, singular: &'a str, plural: &'a str) -> &'a str {
    if n == 1 { singular } else { plural }
}

/// `n` followed by the correct noun form: `1 session`, `2 sessions`.
#[must_use]
pub fn count(n: u64, singular: &str, plural_form: &str) -> String {
    format!("{} {}", grouped(n), plural(n, singular, plural_form))
}

/// [`count`] for regular nouns, where the plural is the singular plus `s`.
#[must_use]
pub fn count_s(n: u64, singular: &str) -> String {
    if n == 1 {
        format!("{} {singular}", grouped(n))
    } else {
        format!("{} {singular}s", grouped(n))
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

/// Decimal digits grouped in threes: `0`, `999`, `1,000`, `12,480`.
#[must_use]
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(*byte));
    }
    out
}
