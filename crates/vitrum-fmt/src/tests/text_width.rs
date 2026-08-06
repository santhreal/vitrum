//! Column measurement. Everything else in the crate is built on `display_width`
//! being right, so a wrong answer here silently corrupts every layout.

use crate::text::{cluster_width, display_width, fits, truncate_end, truncate_middle};

/// ASCII must measure one column per byte.
///
/// The fast path in `cluster_width` short-circuits single printable ASCII bytes
/// without calling `unicode-width`. If that path ever disagreed with the
/// library, every English label in the product would be measured by a different
/// rule than every other label.
#[test]
fn ascii_measures_one_column_per_character() {
    assert_eq!(display_width("session"), 7);
    assert_eq!(display_width(""), 0);
    assert_eq!(display_width(" "), 1);
    assert_eq!(display_width("a b"), 3);
    assert_eq!(display_width("~/src/vitrum"), 12);
}

/// CJK characters must measure two columns each, not one.
///
/// This is the whole reason the crate measures columns instead of `chars()`. If
/// a Japanese session title were measured by character count, every truncation
/// budget would be overrun by exactly the number of CJK characters kept, and
/// the sidebar would push its own right edge off screen.
#[test]
fn cjk_characters_measure_two_columns_each() {
    assert_eq!(display_width("漢字"), 4);
    assert_eq!("漢字".chars().count(), 2, "two chars, four columns");
    assert_eq!(display_width("こんにちは"), 10);
    assert_eq!(display_width("セッション"), 10);
    assert_eq!(display_width("a漢b"), 4);
}

/// A ZWJ emoji sequence is one cluster of two columns, not one column per
/// codepoint.
///
/// `👨‍👩‍👧` is six codepoints and eighteen bytes. Summing per-codepoint widths
/// would call it six columns wide and a terminal draws it as two, so a status
/// line containing one would be padded four columns short and every column to
/// its right would be misaligned.
#[test]
fn zwj_emoji_sequence_measures_two_columns() {
    let family = "👨\u{200d}👩\u{200d}👧";
    assert_eq!(family.len(), 18, "eighteen bytes");
    assert_eq!(family.chars().count(), 5, "five codepoints");
    assert_eq!(display_width(family), 2);
    assert_eq!(cluster_width(family), 2);
}

/// A regional-indicator flag is one cluster of two columns.
///
/// Two codepoints that each measure two columns on their own, but together
/// draw as one flag. Anything that measures them independently reports four.
#[test]
fn regional_indicator_flag_measures_two_columns() {
    assert_eq!(display_width("🇯🇵"), 2);
    assert_eq!(display_width("🇯🇵🇩🇪"), 4);
}

/// A combining mark adds no columns and does not detach from its base.
///
/// `e` + U+0301 draws as one `é` in one column. Counting the mark as a
/// character would over-measure, and treating it as its own cluster would let
/// truncation strip the accent off the letter it belongs to.
#[test]
fn combining_mark_adds_no_columns() {
    let decomposed = "cafe\u{301}";
    assert_eq!(decomposed.chars().count(), 5);
    assert_eq!(display_width(decomposed), 4);
    assert_eq!(display_width("café"), 4, "composed form measures the same");
}

/// Control characters measure zero columns.
///
/// `unicode-width` charges a control character one column. A terminal advances
/// the cursor by nothing: an `ESC` opens a sequence, a `BEL` rings, `DEL` does
/// nothing. Charging them would make any title carrying a stray escape from an
/// OSC sequence measure wider than it draws, and the layout would under-fill.
#[test]
fn control_characters_measure_zero_columns() {
    assert_eq!(display_width("a\u{1b}b"), 2, "ESC");
    assert_eq!(display_width("a\tb"), 2, "TAB");
    assert_eq!(display_width("a\u{7f}b"), 2, "DEL");
    assert_eq!(display_width("a\u{85}b"), 2, "C1 NEL");
    assert_eq!(display_width("a\r\nb"), 2, "CRLF is one cluster, zero columns");
    assert_eq!(cluster_width("\u{1b}"), 0);
}

/// A zero-width space measures zero columns.
///
/// Not a control character, so the control-character branch cannot cover it;
/// this checks the `unicode-width` path is actually consulted for the general
/// case rather than everything falling through the fast path.
#[test]
fn zero_width_space_measures_zero_columns() {
    assert_eq!(display_width("ab\u{200b}cd"), 4);
    assert_eq!(cluster_width("\u{200b}"), 0);
}

/// `fits` is inclusive at the budget and exclusive one column past it.
///
/// An off-by-one here decides whether a label that exactly fills its cell gets
/// needlessly truncated, which is the single most visible formatting defect
/// there is.
#[test]
fn fits_is_inclusive_at_the_budget() {
    assert!(fits("abcde", 6));
    assert!(fits("abcde", 5), "exactly filling the cell fits");
    assert!(!fits("abcde", 4));
    assert!(fits("", 0));
    assert!(!fits("漢", 1), "a wide character does not fit one column");
    assert!(fits("漢", 2));
}
#[test]
fn ascii_fastpath_width_and_fits() {
    let s = "hello world";
    assert_eq!(display_width(s), 11);
    assert!(fits(s, 11));
    assert!(!fits(s, 10));

    let with_ctrl = "hello\x07world";
    assert_eq!(display_width(with_ctrl), 10);
    assert!(fits(with_ctrl, 10));
    assert!(!fits(with_ctrl, 9));
}

#[test]
fn ascii_truncate_matches_width_not_byte_length() {
    // More bytes than budget, but printable width fits: must not ellipsize.
    let padded = "hello\x07\x07\x07world";
    assert_eq!(display_width(padded), 10);
    assert_eq!(truncate_end(padded, 10), padded);
    assert_eq!(truncate_middle(padded, 10), padded);

    // Over budget on width still truncates like the grapheme path.
    assert_eq!(truncate_end("hello world", 8), "hello w\u{2026}");
    assert_eq!(display_width(&truncate_end("hello world", 8)), 8);
}
