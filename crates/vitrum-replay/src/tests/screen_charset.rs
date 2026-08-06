//! DEC Special Graphics and the shifts.

use crate::screen::Charset;
use crate::tests::support::{linear, rows_of};

/// `ESC ( 0` maps the ASCII letters onto line-drawing glyphs, and `ESC ( B` puts them
/// back.
///
/// The bug: ignoring charset designation. `ncurses` draws every box this way, and the
/// fixture contains a real one. Without the mapping a replay shows `lqqqk` where the
/// session showed a box, which looks like corrupted output rather than a missing
/// feature.
#[test]
fn dec_special_graphics_maps_letters_to_line_drawing() {
    let screen = linear(20, 2, b"\x1b(0lqqqk\x1b(B done");
    assert_eq!(rows_of(&screen)[0], "\u{250c}\u{2500}\u{2500}\u{2500}\u{2510} done");
}

/// Every code point in the DEC Special Graphics range maps to its documented glyph.
///
/// A table entered by hand is exactly the kind of thing that is one row out. This
/// pins the two ends and the corners a box uses.
#[test]
fn the_special_graphics_table_is_correct_at_its_edges_and_corners() {
    let set = Charset::DecSpecialGraphics;
    assert_eq!(set.map('_'), ' ', "0x5f, the first entry, is a blank");
    assert_eq!(set.map('`'), '\u{25c6}', "diamond");
    assert_eq!(set.map('a'), '\u{2592}', "checkerboard");
    assert_eq!(set.map('j'), '\u{2518}', "bottom right corner");
    assert_eq!(set.map('k'), '\u{2510}', "top right corner");
    assert_eq!(set.map('l'), '\u{250c}', "top left corner");
    assert_eq!(set.map('m'), '\u{2514}', "bottom left corner");
    assert_eq!(set.map('n'), '\u{253c}', "cross");
    assert_eq!(set.map('q'), '\u{2500}', "horizontal");
    assert_eq!(set.map('x'), '\u{2502}', "vertical");
    assert_eq!(set.map('t'), '\u{251c}', "left tee");
    assert_eq!(set.map('u'), '\u{2524}', "right tee");
    assert_eq!(set.map('v'), '\u{2534}', "bottom tee");
    assert_eq!(set.map('w'), '\u{252c}', "top tee");
    assert_eq!(set.map('~'), '\u{00b7}', "0x7e, the last entry, is a middle dot");
}

/// Characters outside `0x5f..=0x7e` are untouched by the mapping.
///
/// The bug: mapping the whole printable range. Digits and capital letters would turn
/// into garbage, and a program that designates the graphics set and then forgets to
/// restore it would render every subsequent word as line noise instead of merely the
/// lowercase letters.
#[test]
fn characters_outside_the_range_pass_through_unmapped() {
    let set = Charset::DecSpecialGraphics;
    assert_eq!(set.map('A'), 'A');
    assert_eq!(set.map('5'), '5');
    assert_eq!(set.map(' '), ' ');
    assert_eq!(set.map('\u{65e5}'), '\u{65e5}');
}

/// `SO` and `SI` shift between G1 and G0 without redesignating either.
///
/// The bug: treating `SO` as "switch to graphics". `SO` selects whatever G1 holds, and
/// if G1 was never designated that is ASCII. A program that shifts without designating
/// would otherwise have its plain text mangled.
#[test]
fn so_and_si_shift_between_the_designated_sets() {
    // G1 designated as graphics, then shifted in and out.
    let screen = linear(20, 2, b"\x1b)0q\x0eq\x0fq");
    assert_eq!(rows_of(&screen)[0], "q\u{2500}q");

    // SO with G1 left as ASCII changes nothing.
    let untouched = linear(20, 2, b"\x0eqqq");
    assert_eq!(rows_of(&untouched)[0], "qqq");
}

/// The charset survives `DECSC` and `DECRC`.
///
/// The bug: saving the position and rendition but not the charset. A program that saves
/// the cursor, draws a box, and restores would come back with the graphics set still
/// shifted in and print its next word as line noise.
#[test]
fn decsc_and_decrc_carry_the_charset() {
    let screen = linear(20, 3, b"\x1b(0\x1b7\x1b(B\x1b[2;1Habc\x1b8qqq");
    assert_eq!(rows_of(&screen)[1], "abc", "ASCII while designated so");
    assert_eq!(
        rows_of(&screen)[0],
        "\u{2500}\u{2500}\u{2500}",
        "the graphics set came back with the cursor"
    );
}

/// `RIS` puts both slots back to ASCII with G0 shifted in.
#[test]
fn ris_restores_both_charsets() {
    let screen = linear(20, 2, b"\x1b(0\x1b)0\x0e\x1bcqqq");
    assert_eq!(rows_of(&screen)[0], "qqq");
    assert_eq!(screen.charsets().g0, Charset::Ascii);
    assert_eq!(screen.charsets().g1, Charset::Ascii);
    assert!(!screen.charsets().shifted);
}

/// A national-variant designator is read as ASCII rather than guessed at.
///
/// The bug: mapping an unknown designator onto the graphics set. `ESC ( A` is the UK
/// set, which differs from ASCII in one glyph; treating it as line drawing would
/// destroy the whole line, which is far worse than the one wrong pound sign.
#[test]
fn an_unknown_designator_is_read_as_ascii() {
    let screen = linear(20, 2, b"\x1b(Aqqq");
    assert_eq!(rows_of(&screen)[0], "qqq");
}
