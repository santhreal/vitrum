//! Double-width characters and grapheme clusters.
//!
//! A wide character occupies two grid columns, and the pair has to stay a pair:
//! a head without its tail draws over its neighbour, and a tail without its
//! head draws a blank where a character should be.

use vitrum_grid::cell::CellSlot;

use super::support::Fixture;

#[test]
fn a_wide_character_owns_two_columns() {
    let mut fx = Fixture::new(10, 1);
    fx.write("中".as_bytes());

    assert_eq!(fx.cell(0, 0).ch, '中');
    assert_eq!(fx.cell(0, 0).slot, CellSlot::WideHead);
    assert_eq!(fx.cell(1, 0).slot, CellSlot::WideTail);
}

#[test]
fn a_wide_tail_draws_nothing() {
    // The head's quad already covers both columns. A tail carrying a space
    // would paint the background twice and a tail carrying the character would
    // paint the glyph twice.
    let mut fx = Fixture::new(10, 1);
    fx.write("中".as_bytes());

    let tail = fx.cell(1, 0);
    assert_eq!(tail.ch, '\0');
    assert!(tail.is_glyphless());
}

#[test]
fn an_emoji_is_wide() {
    let mut fx = Fixture::new(10, 1);
    fx.write("🦀".as_bytes());

    assert_eq!(fx.cell(0, 0).ch, '🦀');
    assert_eq!(fx.cell(0, 0).slot, CellSlot::WideHead);
    assert_eq!(fx.cell(1, 0).slot, CellSlot::WideTail);
}

#[test]
fn narrow_text_after_a_wide_character_starts_at_the_third_column() {
    let mut fx = Fixture::new(10, 1);
    fx.write("中x".as_bytes());

    assert_eq!(fx.cell(2, 0).ch, 'x');
    assert_eq!(fx.cell(2, 0).slot, CellSlot::Single);
}

#[test]
fn a_wide_character_that_does_not_fit_moves_to_the_next_row() {
    // The last column of a three-column row cannot hold a wide character, so it
    // becomes a spacer and the character wraps. The spacer is a real blank, not
    // half of a pair.
    let mut fx = Fixture::new(3, 2);
    fx.write("ab中".as_bytes());

    assert_eq!(fx.cell(2, 0).slot, CellSlot::Single);
    assert_eq!(fx.cell(0, 1).ch, '中');
    assert_eq!(fx.cell(0, 1).slot, CellSlot::WideHead);
}

#[test]
fn overwriting_half_of_a_wide_pair_leaves_no_orphan() {
    // Writing over the head must not leave the tail behind claiming to be the
    // second half of a character that is gone.
    let mut fx = Fixture::new(10, 1);
    fx.write("中x".as_bytes());
    fx.write(b"\ra");

    assert_eq!(fx.cell(0, 0).ch, 'a');
    assert_eq!(fx.cell(0, 0).slot, CellSlot::Single);
    assert_ne!(fx.cell(1, 0).slot, CellSlot::WideTail);
}

#[test]
fn a_combining_mark_is_flattened_and_counted() {
    // A grid cell is one `char`, so "e" plus a combining acute cannot both be
    // stored. The base survives and the sync reports the loss, because a screen
    // showing an approximation must say so rather than look correct.
    let mut fx = Fixture::new(10, 1);
    let stats = fx.write("e\u{0301}".as_bytes());

    assert_eq!(fx.cell(0, 0).ch, 'e');
    assert_eq!(stats.graphemes_flattened, 1);
}

#[test]
fn plain_text_flattens_nothing() {
    let mut fx = Fixture::new(10, 1);
    let stats = fx.write(b"plain");
    assert_eq!(stats.graphemes_flattened, 0);
}
