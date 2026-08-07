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

/// The ASCII fast paths are a second implementation of the grapheme rules, so
/// they have to be pinned to the first one. Examples cannot do that: a fast
/// path breaks on the input nobody thought to write down. This walks the whole
/// one and two byte ASCII space and asserts both implementations agree, so any
/// future edit to either side that changes a single codepoint fails here.
#[test]
fn the_ascii_fast_path_agrees_with_the_grapheme_path() {
    use unicode_segmentation::UnicodeSegmentation;

    fn by_grapheme(text: &str) -> usize {
        text.graphemes(true).map(cluster_width).sum()
    }

    use crate::text::{
        display_width_by_cluster, fits_by_cluster, truncate_end_by_cluster,
        truncate_middle_by_cluster,
    };

    // Every fast path is held against the implementation it shortcuts, output
    // for output. Asserting only that a result fits its budget would accept a
    // fast path that truncates somewhere else entirely.
    fn agree(text: &str, budget: usize, what: &str) {
        assert_eq!(
            display_width(text),
            display_width_by_cluster(text),
            "display_width disagrees for {what}"
        );
        assert_eq!(
            fits(text, budget),
            fits_by_cluster(text, budget),
            "fits disagrees for {what} at {budget}"
        );
        assert_eq!(
            truncate_end(text, budget),
            truncate_end_by_cluster(text, budget),
            "truncate_end disagrees for {what} at {budget}"
        );
        assert_eq!(
            truncate_middle(text, budget),
            truncate_middle_by_cluster(text, budget),
            "truncate_middle disagrees for {what} at {budget}"
        );
    }

    let mut checked = 0usize;
    let mut buf = String::with_capacity(2);
    for a in 0u8..128 {
        buf.clear();
        buf.push(a as char);
        assert_eq!(display_width(&buf), by_grapheme(&buf), "width of {a:#04x}");
        for b in 0u8..128 {
            buf.clear();
            buf.push(a as char);
            buf.push(b as char);
            let width = by_grapheme(&buf);
            assert_eq!(display_width(&buf), width, "width of {a:#04x}{b:#04x}");
            for budget in 0..=3 {
                assert_eq!(
                    fits(&buf, budget),
                    width <= budget,
                    "fits({a:#04x}{b:#04x}, {budget})"
                );
                // Truncation may drop columns but must never claim more than
                // it was given, which is the invariant the caller lays out to.
                assert!(
                    by_grapheme(&truncate_end(&buf, budget)) <= budget,
                    "truncate_end({a:#04x}{b:#04x}, {budget}) overflows"
                );
            }
            for budget in 0..=4 {
                agree(&buf, budget, &format!("{a:#04x}{b:#04x}"));
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 128 * 128);

    // Long enough to be over budget, so the middle split actually runs. A
    // string that fits is returned whole and never reaches the head and tail
    // walks, which is where the two implementations have the most room to
    // disagree.
    for a in 0u8..128 {
        for b in 0u8..128 {
            let text = format!("AB{}{}YZ", a as char, b as char);
            for budget in 2..=5 {
                agree(&text, budget, &format!("AB{a:#04x}{b:#04x}YZ"));
            }
        }
    }
}
