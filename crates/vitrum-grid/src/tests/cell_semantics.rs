//! Invariants of the storage primitives every other layer depends on.

use crate::cell::{Attrs, Cell, CellSlot, CharWidth, Rgba, Style, char_width};
use crate::font::FontStyle;

/// A `Cell` must stay 16 bytes with 4-byte alignment.
///
/// The whole memory argument for this crate rests on it: a 200x50 grid is
/// 160 KiB, and twenty of them are 3.2 MiB. If someone adds a `String` for
/// combining marks or widens the slot to a `usize`, the grid silently doubles
/// or triples and the client's flat-memory claim stops being true. This test
/// makes that a build failure instead of a heap-profiling surprise months
/// later.
#[test]
fn cell_stays_sixteen_bytes_with_four_byte_alignment() {
    assert_eq!(
        core::mem::size_of::<Cell>(),
        16,
        "Cell grew; a 200x50 grid is no longer 160 KiB"
    );
    assert_eq!(core::mem::align_of::<Cell>(), 4);
    assert_eq!(core::mem::size_of::<Rgba>(), 4);
    assert_eq!(core::mem::size_of::<Attrs>(), 1);
    assert_eq!(core::mem::size_of::<CellSlot>(), 1);
}

/// `Rgba` must lay out as `r, g, b, a` in ascending address order.
///
/// The renderer feeds these four bytes to the GPU as a `Unorm8x4` vertex
/// attribute with no conversion step. If the field order or the `repr(C)` ever
/// changes, every colour in the terminal silently swizzles (red text turns
/// blue) with no compile error anywhere.
#[test]
fn rgba_byte_order_is_r_g_b_a_for_direct_gpu_upload() {
    let c = Rgba::new(0x12, 0x34, 0x56, 0x78);
    assert_eq!(c.to_bytes(), [0x12, 0x34, 0x56, 0x78]);
    assert_eq!(Rgba::from_bytes([0x12, 0x34, 0x56, 0x78]), c);

    let raw: [u8; 4] = unsafe { core::mem::transmute(c) };
    assert_eq!(
        raw,
        [0x12, 0x34, 0x56, 0x78],
        "in-memory layout must match to_bytes, or the GPU sees swizzled colours"
    );
}

/// `Rgba::rgb` must produce opaque colours and the named constants must be the
/// exact values they claim.
///
/// A default background that is accidentally transparent turns the whole
/// terminal into a see-through pane on a compositor, which looks like a
/// blending bug and is really a constant typo.
#[test]
fn named_colors_have_exact_values_and_full_alpha() {
    assert_eq!(Rgba::BLACK.to_bytes(), [0, 0, 0, 255]);
    assert_eq!(Rgba::WHITE.to_bytes(), [255, 255, 255, 255]);
    assert_eq!(Rgba::TRANSPARENT.to_bytes(), [0, 0, 0, 0]);
    assert_eq!(Rgba::rgb(1, 2, 3).a, 255);
}

/// Every attribute bit must round trip through `bits` and
/// `from_bits_truncate`, and undefined bits must be dropped.
///
/// The renderer packs attributes into a GPU flag word. A parser that learns a
/// new SGR code and stuffs bit 7 in must not be able to reach the shader with
/// it, or the span field (bits 0-1 of the instance flags) can be corrupted by
/// an unrelated attribute.
#[test]
fn attribute_bits_round_trip_and_undefined_bits_are_dropped() {
    for bits in 0u8..=0b1111 {
        let a = Attrs::from_bits_truncate(bits);
        assert_eq!(a.bits(), bits, "defined bit pattern {bits:#06b} must survive");
    }
    assert_eq!(
        Attrs::from_bits_truncate(0b1111_1111).bits(),
        0b0000_1111,
        "bits 4..8 are undefined and must be discarded"
    );
    assert_eq!(Attrs::ALL.bits(), 0b1111);
}

/// The four attribute constants must occupy four distinct bits.
///
/// If two of them collided, setting bold would also set italic, and the bug
/// would show up as "the terminal renders everything slanted" long after the
/// change that caused it.
#[test]
fn attribute_constants_occupy_distinct_bits() {
    let all = [Attrs::BOLD, Attrs::ITALIC, Attrs::UNDERLINE, Attrs::REVERSE];
    let mut seen = 0u8;
    for a in all {
        assert_eq!(a.bits().count_ones(), 1, "{a:?} must be a single bit");
        assert_eq!(seen & a.bits(), 0, "{a:?} collides with an earlier attribute");
        seen |= a.bits();
    }
    assert_eq!(seen, Attrs::ALL.bits());
}

/// `contains` must require every bit of the query, not merely one of them.
///
/// A sloppy `!= 0` implementation makes a bold-only cell claim to be
/// bold-and-italic, which picks the wrong font face for every bold run.
#[test]
fn contains_requires_all_queried_bits_not_just_one() {
    let bold = Attrs::BOLD;
    assert!(bold.contains(Attrs::BOLD));
    assert!(!bold.contains(Attrs::ITALIC));
    assert!(!bold.contains(Attrs::BOLD.with(Attrs::ITALIC)));

    let both = Attrs::BOLD | Attrs::ITALIC;
    assert!(both.contains(Attrs::BOLD));
    assert!(both.contains(Attrs::ITALIC));
    assert!(both.contains(Attrs::BOLD.with(Attrs::ITALIC)));
    assert!(!both.contains(Attrs::UNDERLINE));
}

/// `with` and `without` must not mutate the receiver and must be exact.
///
/// These are the operations a VT parser uses for SGR 1 and SGR 22. An
/// off-by-one mask there leaks an attribute into every following cell for the
/// rest of the session.
#[test]
fn attribute_set_and_clear_are_exact_and_non_mutating() {
    let base = Attrs::BOLD | Attrs::UNDERLINE;
    assert_eq!(base.with(Attrs::ITALIC).bits(), 0b0111);
    assert_eq!(base.without(Attrs::BOLD).bits(), 0b0100);
    assert_eq!(base.without(Attrs::REVERSE).bits(), 0b0101);
    assert_eq!(base.bits(), 0b0101, "with/without must leave the input alone");
    assert!(Attrs::NONE.is_empty());
    assert!(!base.is_empty());
}

/// `Attrs` must render its set bits by name.
///
/// Every failure message in this crate that mentions attributes goes through
/// this. `Attrs(5)` in a panic tells nobody anything; `Attrs(BOLD|UNDERLINE)`
/// identifies the case immediately.
#[test]
fn attribute_debug_names_the_set_bits() {
    assert_eq!(format!("{:?}", Attrs::NONE), "Attrs(NONE)");
    assert_eq!(format!("{:?}", Attrs::BOLD), "Attrs(BOLD)");
    assert_eq!(
        format!("{:?}", Attrs::BOLD | Attrs::UNDERLINE),
        "Attrs(BOLD|UNDERLINE)"
    );
    assert_eq!(
        format!("{:?}", Attrs::ALL),
        "Attrs(BOLD|ITALIC|UNDERLINE|REVERSE)"
    );
}

/// The reverse attribute must swap the pair the renderer uploads, and leave the
/// stored colours untouched.
///
/// Reverse is resolved on the CPU so the shader has no branch for it. If the
/// swap happened in place instead, toggling reverse twice would be
/// destructive and a selection highlight would permanently recolour the text
/// under it.
#[test]
fn reverse_swaps_uploaded_colors_without_touching_stored_ones() {
    let fg = Rgba::rgb(0xff, 0x00, 0x00);
    let bg = Rgba::rgb(0x00, 0x00, 0xff);

    let plain = Cell::new('x', Style::new(fg, bg));
    assert_eq!(plain.resolved_colors(), (fg, bg));

    let reversed = Cell::new('x', Style::new(fg, bg).with_attrs(Attrs::REVERSE));
    assert_eq!(reversed.resolved_colors(), (bg, fg));
    assert_eq!(reversed.fg, fg, "stored foreground must not change");
    assert_eq!(reversed.bg, bg, "stored background must not change");
}

/// A cell must report itself glyphless exactly when there is nothing to
/// rasterise.
///
/// The renderer skips the atlas for these. Getting it wrong in one direction
/// wastes an atlas slot per space (a full screen of spaces would thrash the
/// atlas); getting it wrong in the other direction drops real glyphs.
#[test]
fn glyphless_covers_blanks_and_wide_tails_only() {
    let style = Style::DEFAULT;
    assert!(Cell::blank(style).is_glyphless());
    assert!(Cell::new(' ', style).is_glyphless());
    assert!(Cell::new('\0', style).is_glyphless());
    assert!(!Cell::new('a', style).is_glyphless());
    assert!(!Cell::new('漢', style).is_glyphless());

    let tail = Cell {
        ch: '\0',
        slot: CellSlot::WideTail,
        ..Cell::new('x', style)
    };
    assert!(tail.is_glyphless());

    let head = Cell {
        slot: CellSlot::WideHead,
        ..Cell::new('漢', style)
    };
    assert!(!head.is_glyphless(), "the head carries the glyph");
}

/// Slots must claim exactly 1, 2, and 0 drawn columns.
///
/// The instance flag word carries this number and the vertex shader multiplies
/// the quad width by it. A tail that claimed 1 would paint its own background
/// over the right half of the wide glyph the head just drew; a head that
/// claimed 1 would clip a CJK character in half.
#[test]
fn slot_drawn_column_counts_are_one_two_and_zero() {
    assert_eq!(CellSlot::Single.drawn_columns(), 1);
    assert_eq!(CellSlot::WideHead.drawn_columns(), 2);
    assert_eq!(CellSlot::WideTail.drawn_columns(), 0);
}

/// Character classification must match the East Asian Width property with
/// ambiguous treated as narrow.
///
/// Every column calculation in the grid depends on this. A CJK character
/// misclassified as narrow shifts the rest of the line by one column and the
/// misalignment compounds down the screen.
#[test]
fn char_width_classifies_controls_marks_narrow_and_wide() {
    assert_eq!(char_width('\u{0}'), CharWidth::Control);
    assert_eq!(char_width('\u{7}'), CharWidth::Control, "BEL");
    assert_eq!(char_width('\u{1b}'), CharWidth::Control, "ESC");
    assert_eq!(char_width('\u{7f}'), CharWidth::Control, "DEL");

    assert_eq!(char_width('\u{301}'), CharWidth::ZeroWidth, "combining acute");
    assert_eq!(char_width('\u{200b}'), CharWidth::ZeroWidth, "zero width space");

    assert_eq!(char_width('a'), CharWidth::Narrow);
    assert_eq!(char_width(' '), CharWidth::Narrow);
    assert_eq!(char_width('\u{e9}'), CharWidth::Narrow, "precomposed e-acute");
    assert_eq!(char_width('\u{2502}'), CharWidth::Narrow, "box drawing");

    assert_eq!(char_width('漢'), CharWidth::Wide);
    assert_eq!(char_width('あ'), CharWidth::Wide);
    assert_eq!(char_width('한'), CharWidth::Wide);
    assert_eq!(char_width('！'), CharWidth::Wide, "fullwidth exclamation");
    assert_eq!(char_width('\u{1f600}'), CharWidth::Wide, "grinning face");
}

/// `CharWidth::columns` must expose 1 and 2 and refuse the unstorable classes.
///
/// Callers use this to advance a cursor. Returning `Some(0)` for a combining
/// mark would wedge a VT parser in an infinite loop on a single byte.
#[test]
fn char_width_columns_refuses_controls_and_zero_width() {
    assert_eq!(CharWidth::Narrow.columns(), Some(1));
    assert_eq!(CharWidth::Wide.columns(), Some(2));
    assert_eq!(CharWidth::Control.columns(), None);
    assert_eq!(CharWidth::ZeroWidth.columns(), None);
}

/// Attribute combinations must map onto the four font slots one to one.
///
/// This is the lookup that turns SGR 1 and SGR 3 into a face. A collision here
/// renders bold-italic text in the plain face and nobody notices until someone
/// reads a diff.
#[test]
fn font_style_maps_bold_and_italic_bits_onto_four_distinct_slots() {
    assert_eq!(FontStyle::from_attrs(Attrs::NONE), FontStyle::Regular);
    assert_eq!(FontStyle::from_attrs(Attrs::BOLD), FontStyle::Bold);
    assert_eq!(FontStyle::from_attrs(Attrs::ITALIC), FontStyle::Italic);
    assert_eq!(
        FontStyle::from_attrs(Attrs::BOLD | Attrs::ITALIC),
        FontStyle::BoldItalic
    );

    // Attributes the face selection must ignore.
    assert_eq!(
        FontStyle::from_attrs(Attrs::UNDERLINE | Attrs::REVERSE),
        FontStyle::Regular
    );
    assert_eq!(
        FontStyle::from_attrs(Attrs::ALL),
        FontStyle::BoldItalic,
        "underline and reverse must not disturb face selection"
    );

    let indices: Vec<usize> = FontStyle::ALL.iter().map(|s| s.index()).collect();
    assert_eq!(indices, vec![0, 1, 2, 3]);
    assert_eq!(
        FontStyle::ALL.map(FontStyle::is_bold),
        [false, true, false, true]
    );
    assert_eq!(
        FontStyle::ALL.map(FontStyle::is_italic),
        [false, false, true, true]
    );
}

/// A blank cell must carry a space, not NUL, and must adopt the whole style.
///
/// `row_text` and clipboard extraction read `ch` straight out. A NUL-filled
/// grid would copy as a run of NUL bytes instead of spaces.
#[test]
fn blank_cell_holds_a_space_and_the_full_style() {
    let style = Style {
        fg: Rgba::rgb(1, 2, 3),
        bg: Rgba::rgb(4, 5, 6),
        attrs: Attrs::UNDERLINE,
    };
    let blank = Cell::blank(style);
    assert_eq!(blank.ch, ' ');
    assert_eq!(blank.slot, CellSlot::Single);
    assert_eq!(blank.style(), style);
    assert_eq!(Cell::default(), Cell::blank(Style::DEFAULT));
    assert_eq!(Style::default(), Style::DEFAULT);
    assert_eq!(Style::DEFAULT.fg, Rgba::WHITE);
    assert_eq!(Style::DEFAULT.bg, Rgba::BLACK);
    assert!(Style::DEFAULT.attrs.is_empty());
}
