//! End and middle truncation: budgets, ellipsis placement, and the rule that a
//! double-width glyph is never cut in half.

use crate::text::{ELLIPSIS, display_width, pad_end, truncate_end, truncate_middle};

/// A string that fits must come back byte-for-byte, with no ellipsis.
///
/// Appending an ellipsis to something that already fits is the most common
/// truncation bug and it is immediately visible on every short label.
#[test]
fn text_that_fits_is_returned_unchanged() {
    assert_eq!(truncate_end("session", 10), "session");
    assert_eq!(truncate_end("session", 7), "session", "exactly at budget");
    assert_eq!(truncate_middle("session", 7), "session");
    assert_eq!(truncate_end("", 0), "");
    assert!(!truncate_end("session", 7).contains(ELLIPSIS));
}

/// One column over budget triggers an ellipsis that itself costs one column.
///
/// The result must be exactly the budget, not the budget plus one: a
/// truncation that forgets to pay for its own ellipsis overflows by one column
/// on every truncated row.
#[test]
fn truncate_end_pays_for_its_own_ellipsis() {
    assert_eq!(truncate_end("session-name", 11), "session-na…");
    assert_eq!(display_width(&truncate_end("session-name", 11)), 11);
    assert_eq!(truncate_end("abcdef", 3), "ab…");
    assert_eq!(truncate_end("abcdef", 1), "…");
}

/// A zero budget yields an empty string, never a bare ellipsis.
///
/// A collapsed column must render nothing. Emitting `…` into a zero-width cell
/// would overflow it and push the whole row one column right.
#[test]
fn zero_budget_yields_nothing() {
    assert_eq!(truncate_end("session", 0), "");
    assert_eq!(truncate_middle("session", 0), "");
    assert_eq!(display_width(&truncate_end("session", 0)), 0);
}

/// A one-column budget yields the ellipsis alone from the middle truncator.
///
/// There is no room for any content, but the row still has to signal that
/// something was elided rather than that the field is empty.
#[test]
fn one_column_budget_yields_only_an_ellipsis() {
    assert_eq!(truncate_middle("session", 1), "…");
    assert_eq!(display_width(&truncate_middle("session", 1)), 1);
}

/// A double-width character is dropped rather than half-drawn when only one
/// column of budget is left.
///
/// This is the case the acceptance criteria call out: `漢字漢字` is eight
/// columns, and in a four-column cell the ellipsis leaves three, which cannot
/// hold two wide characters. Splitting one would emit a byte range that ends
/// mid-glyph; a terminal draws garbage and the string may not even be valid
/// UTF-8. The correct answer is three columns of output in a four-column cell.
#[test]
fn truncation_never_splits_a_wide_character() {
    assert_eq!(truncate_end("漢字漢字", 5), "漢字…");
    assert_eq!(display_width(&truncate_end("漢字漢字", 5)), 5);

    let tight = truncate_end("漢字漢字", 4);
    assert_eq!(tight, "漢…", "the second 漢 does not fit in three columns");
    assert_eq!(display_width(&tight), 3, "one column is left blank, not split");
    assert!(tight.is_char_boundary(tight.len()));
}

/// A ZWJ emoji sequence survives truncation whole or not at all.
///
/// Cutting inside `👨‍👩‍👧` would leave a dangling ZWJ and render as two or
/// three separate people, which is a different picture, not a shorter one.
#[test]
fn truncation_never_splits_a_zwj_emoji_sequence() {
    let family = "👨\u{200d}👩\u{200d}👧abc";
    let cut = truncate_end(family, 4);
    assert_eq!(cut, "👨\u{200d}👩\u{200d}👧a…");
    assert_eq!(display_width(&cut), 4);
    assert!(cut.starts_with("👨\u{200d}👩\u{200d}👧"), "the family is intact");

    let tighter = truncate_end(family, 2);
    assert_eq!(tighter, "…", "the family cannot fit beside an ellipsis");
    assert_eq!(display_width(&tighter), 1);
}

/// A combining mark is never separated from the character it modifies.
///
/// Truncating between `e` and U+0301 would leave a naked accent at the start of
/// the elided remainder, or a letter that silently lost its accent.
#[test]
fn truncation_keeps_combining_marks_with_their_base() {
    let cut = truncate_end("e\u{301}xyz", 3);
    assert_eq!(cut, "e\u{301}x…");
    assert_eq!(display_width(&cut), 3);
    assert!(cut.contains('\u{301}'), "the accent came along with its base");
}

/// The middle truncator splits the budget and keeps both ends.
///
/// The point of the middle form is that both ends carry meaning. Losing either
/// end would make it a plain head or tail truncation with extra steps.
#[test]
fn truncate_middle_keeps_both_ends() {
    assert_eq!(truncate_middle("abcdefghij", 7), "abc…hij");
    assert_eq!(display_width(&truncate_middle("abcdefghij", 7)), 7);
}

/// An odd budget gives the extra column to the head.
///
/// Deterministic tie-breaking. If it drifted, the same string would render
/// differently in two cells that happen to differ by one column, which reads
/// as a rendering bug even though both outputs fit.
#[test]
fn truncate_middle_gives_the_odd_column_to_the_head() {
    assert_eq!(truncate_middle("abcdefghij", 6), "abc…ij");
    assert_eq!(display_width(&truncate_middle("abcdefghij", 6)), 6);
}

/// Budget the tail cannot spend is handed back to the head.
///
/// With a naive half-and-half split, `漢字漢字漢字` in seven columns yields
/// `漢…字` and wastes two of the seven, because each half-budget of three can
/// only hold one two-column character. Reclaiming the slack fills the cell.
#[test]
fn truncate_middle_reclaims_budget_the_tail_cannot_use() {
    let cut = truncate_middle("漢字漢字漢字", 7);
    assert_eq!(cut, "漢字…字");
    assert_eq!(display_width(&cut), 7, "the whole budget is used");
}

/// Zero-width clusters at the end cannot be emitted on both sides of the
/// ellipsis.
///
/// The tail scan accepts any number of zero-width clusters because they cost
/// nothing. Without the head scan stopping at the tail's start offset, the head
/// would walk over the same clusters and the output would contain them twice,
/// producing a string longer than the input it was supposed to shorten.
#[test]
fn truncate_middle_does_not_duplicate_zero_width_clusters() {
    let cut = truncate_middle("abcdef\u{200b}", 2);
    assert_eq!(cut, "a…\u{200b}");
    assert_eq!(display_width(&cut), 2);
    assert_eq!(cut.matches('\u{200b}').count(), 1);
}

/// Leading zero-width clusters ride along with the head.
///
/// A stray combining mark at the start of a title is width zero, so it must not
/// consume budget, and it must not be dropped either.
#[test]
fn truncate_middle_carries_leading_zero_width_clusters() {
    let cut = truncate_middle("\u{300}\u{300}abcdef", 4);
    assert_eq!(cut, "\u{300}\u{300}ab…f");
    assert_eq!(display_width(&cut), 4);
}

/// No budget from zero to well past the input length ever produces output wider
/// than the budget, for text made entirely of wide characters.
///
/// The single invariant everything else depends on. A sweep catches the
/// off-by-one that only appears at one particular parity of budget against
/// character width, which a handful of point tests will miss.
#[test]
fn no_budget_is_ever_exceeded() {
    let samples = [
        "漢字漢字漢字漢字",
        "セッション-01",
        "👨\u{200d}👩\u{200d}👧👍🇯🇵",
        "abcdefghijklmnop",
        "a漢b字c漢d字e",
        "e\u{301}e\u{301}e\u{301}e\u{301}",
    ];
    for text in samples {
        for budget in 0..=24 {
            let end = truncate_end(text, budget);
            assert!(
                display_width(&end) <= budget,
                "truncate_end({text:?}, {budget}) = {end:?} is too wide"
            );
            let middle = truncate_middle(text, budget);
            assert!(
                display_width(&middle) <= budget,
                "truncate_middle({text:?}, {budget}) = {middle:?} is too wide"
            );
        }
    }
}

/// Once the budget reaches the input width, both truncators stop touching it.
///
/// Guards the boundary between "unchanged" and "elided" from drifting by one.
#[test]
fn truncation_stops_exactly_at_the_input_width() {
    let text = "漢字漢字";
    assert_eq!(display_width(text), 8);
    assert_eq!(truncate_end(text, 8), text);
    assert_eq!(truncate_middle(text, 8), text);
    assert_ne!(truncate_end(text, 7), text);
    assert_ne!(truncate_middle(text, 7), text);
}

/// Padding fills to exactly the budget, in columns.
///
/// A fixed-width cell padded by byte or character count would come out short
/// for CJK and the next column would start in the wrong place.
#[test]
fn pad_end_fills_to_exactly_the_budget_in_columns() {
    assert_eq!(pad_end("ok", 5), "ok   ");
    assert_eq!(display_width(&pad_end("漢", 5)), 5);
    assert_eq!(pad_end("漢", 5), "漢   ");
    assert_eq!(pad_end("", 3), "   ");
    assert_eq!(pad_end("exactly", 7), "exactly");
}

/// Padding a string that is too long truncates it to exactly the budget.
///
/// The whole point of a fixed cell is that its width does not depend on its
/// contents.
#[test]
fn pad_end_truncates_before_padding() {
    assert_eq!(pad_end("session-name", 5), "sess…");
    assert_eq!(display_width(&pad_end("session-name", 5)), 5);
    assert_eq!(pad_end("漢字漢字", 5), "漢字…", "exact fit needs no padding");
    assert_eq!(
        pad_end("漢字漢字", 4),
        "漢… ",
        "a wide character dropped for safety is replaced by padding"
    );
    assert_eq!(display_width(&pad_end("漢字漢字", 4)), 4);
}

/// The elision marker is one real ellipsis character, not three periods.
///
/// `...` costs three columns instead of one, so a budget that was calculated
/// against a one-column marker overflows by two on every truncated row. It also
/// reads as a pause in prose rather than as a cut.
#[test]
fn the_elision_marker_is_one_real_ellipsis_character() {
    assert_eq!(ELLIPSIS, '\u{2026}');
    assert_eq!(display_width("\u{2026}"), 1);
    assert_eq!(ELLIPSIS.len_utf8(), 3, "one column, three bytes");

    for cut in [truncate_end("abcdefgh", 4), truncate_middle("abcdefgh", 4)] {
        assert!(cut.contains(ELLIPSIS), "{cut:?} carries the marker");
        assert!(!cut.contains("..."), "{cut:?} must not use periods");
        assert_eq!(cut.matches(ELLIPSIS).count(), 1, "exactly one marker");
    }
}
#[test]
fn buffer_pool_and_into_formatters_test() {
    use crate::text::{BufferPool, pad_end_into, sanitize_line_into, title_into, truncate_end_into, truncate_middle_into};

    BufferPool::with_buf(|buf| {
        truncate_end_into("hello world", 8, buf);
        assert_eq!(buf, "hello w\u{2026}");
    });
    BufferPool::with_buf(|buf| {
        truncate_middle_into("crates/vitrum-fmt", 10, buf);
        assert_eq!(buf, "crate\u{2026}-fmt");
    });
    BufferPool::with_buf(|buf| {
        sanitize_line_into("hello\x1b[31mworld", buf);
        assert_eq!(buf, "helloworld");
    });

    BufferPool::with_buf(|buf| {
        title_into("  my  title\t", 10, buf);
        assert_eq!(buf, "my title");
    });

    BufferPool::with_buf(|buf| {
        pad_end_into("hi", 5, buf);
        assert_eq!(buf, "hi   ");
    });
}
