//! Counted and pluralised nouns.

use crate::count::{count, count_or_none, count_s, grouped, plural};

/// One takes the singular and everything else takes the plural.
///
/// `1 sessions` in a status line is the single most noticeable sign that
/// nobody looked at the product before shipping it.
#[test]
fn one_is_singular_and_everything_else_is_plural() {
    assert_eq!(count(1, "session", "sessions"), "1 session");
    assert_eq!(count(2, "session", "sessions"), "2 sessions");
    assert_eq!(count(20, "agent", "agents"), "20 agents");
    assert_eq!(plural(1, "session", "sessions"), "session");
    assert_eq!(plural(2, "session", "sessions"), "sessions");
}

/// Zero takes the plural.
///
/// "0 session" is not English. Zero is plural in English and the empty state is
/// the string a user sees most often on first launch.
#[test]
fn zero_takes_the_plural() {
    assert_eq!(count(0, "session", "sessions"), "0 sessions");
    assert_eq!(plural(0, "session", "sessions"), "sessions");
    assert_eq!(count_s(0, "agent"), "0 agents");
}

/// The regular shorthand appends `s` only when the count is not one.
///
/// Callers reach for the shorthand constantly, so its singular case has to be
/// right or the bug appears everywhere at once.
#[test]
fn the_regular_shorthand_pluralises_correctly() {
    assert_eq!(count_s(1, "session"), "1 session");
    assert_eq!(count_s(2, "session"), "2 sessions");
    assert_eq!(count_s(20, "agent"), "20 agents");
    assert_eq!(count_s(1, "project"), "1 project");
}

/// Irregular plurals are supplied by the caller, never guessed.
///
/// A formatter that appended `s` unconditionally would produce `2 branchs` and
/// `2 directorys`. The plural form is a lookup in English, not a rule, so the
/// caller who knows the word supplies it.
#[test]
fn irregular_plurals_are_supplied_by_the_caller() {
    assert_eq!(count(2, "branch", "branches"), "2 branches");
    assert_eq!(count(2, "directory", "directories"), "2 directories");
    assert_eq!(count(1, "branch", "branches"), "1 branch");
}

/// The named-zero form says `no sessions` rather than `0 sessions`.
///
/// An empty state reads better as a word. The digit form is still correct, so
/// this is a separate function rather than a change to `count`.
#[test]
fn the_named_zero_form_uses_a_word() {
    assert_eq!(count_or_none(0, "session", "sessions"), "no sessions");
    assert_eq!(count_or_none(1, "session", "sessions"), "1 session");
    assert_eq!(count_or_none(7, "session", "sessions"), "7 sessions");
}

/// Thousands are grouped with commas.
///
/// A scrollback line count is regularly five or six digits, and `128480` has to
/// be read digit by digit while `128,480` does not.
#[test]
fn thousands_are_grouped() {
    assert_eq!(grouped(0), "0");
    assert_eq!(grouped(1), "1");
    assert_eq!(grouped(999), "999");
    assert_eq!(grouped(1_000), "1,000");
    assert_eq!(grouped(12_480), "12,480");
    assert_eq!(grouped(128_480), "128,480");
    assert_eq!(grouped(1_000_000), "1,000,000");
}

/// Grouping is applied inside counted nouns too.
///
/// If `count` bypassed the grouping, a sidebar would show `12,480 lines` in one
/// place and `12480 lines` in another for the same number.
#[test]
fn counted_nouns_are_grouped_too() {
    assert_eq!(count_s(12_480, "line"), "12,480 lines");
    assert_eq!(count(1_000, "session", "sessions"), "1,000 sessions");
    assert_eq!(count_or_none(2_500, "byte", "bytes"), "2,500 bytes");
}

/// The largest count groups correctly and does not misplace a separator.
///
/// The group boundary is computed from the distance to the end of the string,
/// which is the version that stays correct for a digit count that is not a
/// multiple of three.
#[test]
fn the_largest_count_groups_correctly() {
    assert_eq!(grouped(u64::MAX), "18,446,744,073,709,551,615");
    assert_eq!(grouped(10_000), "10,000");
    assert_eq!(grouped(100_000), "100,000");
    assert!(!grouped(1_000).starts_with(','), "no leading separator");
}
