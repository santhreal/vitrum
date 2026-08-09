//! Graphic rendition, including both extended colour spellings.

use vitrum_grid::{Attrs, Rgba};

use crate::palette::Palette;
use crate::tests::support::{GHOSTTY_ANSI, linear};

fn style_at(bytes: &[u8], col: u16) -> (Rgba, Rgba, Attrs) {
    let screen = linear(20, 2, bytes);
    let cell = screen.grid().cell(col, 0).expect("cell");
    (cell.fg, cell.bg, cell.attrs)
}

/// A bare `CSI m`, with no parameter at all, is a full reset.
///
/// This is not a corner case: it is what `git` writes. Every `git log --color` line in
/// the fixture ends with `ESC [ m`, and a parser that required an explicit `0` would
/// leave every commit hash yellow for the rest of the session.
#[test]
fn a_bare_csi_m_resets_the_rendition() {
    let (fg, bg, attrs) = style_at(b"\x1b[1;33ma\x1b[mb", 1);
    assert_eq!(fg, Palette::XTERM.fg);
    assert_eq!(bg, Palette::XTERM.bg);
    assert_eq!(attrs, Attrs::NONE);
}

/// The eight normal and eight bright colours resolve through the engine's palette,
/// and `SGR 3x`, `SGR 9x`, `SGR 4x` and `SGR 38;5;x` all reach the same entry.
///
/// The old parser resolved these itself out of [`Palette`], and the assertion was
/// against xterm's compiled-in table. Ghostty resolves them now, out of its own
/// sixteen defaults, which are not xterm's — `SGR 31` is `cc6666` and not `cd0000`.
/// Ghostty is the truth because Ghostty is what paints the live pane too; a replay
/// that kept xterm's table would be the only surface in the product disagreeing
/// about what red is.
///
/// The relationships are what this asserts rather than sixteen literals in sixteen
/// places: the same entry has to come back through every spelling that names it.
/// [`GHOSTTY_ANSI`] is the one place a Ghostty theme change turns the suite red.
#[test]
fn the_sixteen_ansi_colours_resolve_through_the_engines_palette() {
    for index in 0u8..8 {
        let normal = format!("\x1b[{}mX", 30 + u16::from(index));
        let (fg, _, _) = style_at(normal.as_bytes(), 0);
        assert_eq!(fg, GHOSTTY_ANSI[index as usize], "SGR 3{index}");

        let bright = format!("\x1b[{}mX", 90 + u16::from(index));
        let (fg, _, _) = style_at(bright.as_bytes(), 0);
        assert_eq!(fg, GHOSTTY_ANSI[index as usize + 8], "SGR 9{index}");
    }

    for index in 0u8..16 {
        let indexed = format!("\x1b[38;5;{index}mX");
        let (fg, _, _) = style_at(indexed.as_bytes(), 0);
        assert_eq!(
            fg, GHOSTTY_ANSI[index as usize],
            "SGR 38;5;{index} names the same entry as the short form"
        );
    }

    let (_, bg, _) = style_at(b"\x1b[44mX", 0);
    assert_eq!(bg, GHOSTTY_ANSI[4]);
    let (_, bg, _) = style_at(b"\x1b[104mX", 0);
    assert_eq!(bg, GHOSTTY_ANSI[12]);
}

/// `39` and `49` return to the palette's defaults, not to black on black.
///
/// The bug: resolving them to colour 0 and colour 7. A program that sets a background
/// and then resets only the foreground would end up with the default foreground drawn
/// as grey rather than the theme's own.
#[test]
fn thirty_nine_and_forty_nine_return_to_the_palette_defaults() {
    let (fg, bg, _) = style_at(b"\x1b[31;44m\x1b[39;49mX", 0);
    assert_eq!(fg, Palette::XTERM.fg);
    assert_eq!(bg, Palette::XTERM.bg);
}

/// The four rendition bits the grid models set and clear independently.
#[test]
fn the_four_rendition_bits_set_and_clear_independently() {
    let (_, _, all) = style_at(b"\x1b[1;3;4;7mX", 0);
    assert_eq!(all, Attrs::ALL);

    let (_, _, some) = style_at(b"\x1b[1;3;4;7m\x1b[22;24mX", 0);
    assert_eq!(some, Attrs::ITALIC.with(Attrs::REVERSE));

    let (_, _, none) = style_at(b"\x1b[1;3;4;7m\x1b[22;23;24;27mX", 0);
    assert_eq!(none, Attrs::NONE);
}

/// `4 : 0` turns underline off; every other underline subparameter turns it on.
///
/// Curly and dotted underlines are spelled `4:3` and `4:4`. A parser that read the
/// subparameter as a colour index, or that treated any `4:` form as "off", would lose
/// the underline that a compiler diagnostic uses to point at the error.
#[test]
fn underline_subparameters_choose_on_or_off() {
    let (_, _, off) = style_at(b"\x1b[4m\x1b[4:0mX", 0);
    assert_eq!(off, Attrs::NONE);

    let (_, _, curly) = style_at(b"\x1b[4:3mX", 0);
    assert_eq!(curly, Attrs::UNDERLINE);
}

/// 256-colour and 24-bit colour work in the semicolon spelling.
///
/// Both appear in the fixture, from real output.
#[test]
fn extended_colour_works_in_the_semicolon_spelling() {
    let (fg, _, _) = style_at(b"\x1b[38;5;120mX", 0);
    assert_eq!(fg, Rgba::rgb(0x87, 0xff, 0x87));

    let (fg, _, _) = style_at(b"\x1b[38;2;255;170;0mX", 0);
    assert_eq!(fg, Rgba::rgb(255, 170, 0));

    let (_, bg, _) = style_at(b"\x1b[48;2;16;32;48mX", 0);
    assert_eq!(bg, Rgba::rgb(16, 32, 48));
}

/// The same colours work in the colon spelling, with and without the colour-space id.
///
/// The bug: only handling semicolons. The colon form is what a terminal that supports
/// it prefers, because the whole colour is one parameter, and anything driving a
/// modern terminal may emit it. Reading `38:2::255:170:0` as "38, then 2" would set
/// the foreground to green.
#[test]
fn extended_colour_works_in_the_colon_spelling() {
    let (fg, _, _) = style_at(b"\x1b[38:2::255:170:0mX", 0);
    assert_eq!(fg, Rgba::rgb(255, 170, 0), "with the colour-space slot");

    let (fg, _, _) = style_at(b"\x1b[38:2:255:170:0mX", 0);
    assert_eq!(fg, Rgba::rgb(255, 170, 0), "without it");

    let (fg, _, _) = style_at(b"\x1b[38:5:120mX", 0);
    assert_eq!(fg, Rgba::rgb(0x87, 0xff, 0x87));
}

/// A channel above 255 is truncated to its low eight bits, after the parameter itself
/// saturates at 16 bits.
///
/// `38;2;300;0;0` is malformed and there is no right answer, only a consistent one.
/// The old parser clamped each channel to 255; Ghostty takes the low byte, so 300
/// becomes 44 and 256 becomes 0. Above 65535 the CSI parameter saturates first, so
/// every larger value lands on 255.
///
/// Ghostty's answer is the one asserted because Ghostty paints the live pane, and a
/// replay that disagreed with the pane about the colour of the same bytes would be
/// worse than either rule. The bug this stops is the projection quietly re-clamping
/// on top of the engine and drifting away from it again.
#[test]
fn an_out_of_range_channel_truncates_to_eight_bits() {
    let (fg, _, _) = style_at(b"\x1b[38;2;300;999;70000mX", 0);
    assert_eq!(fg, Rgba::rgb(44, 231, 255));

    let (fg, _, _) = style_at(b"\x1b[38;2;256;65535;65536mX", 0);
    assert_eq!(
        fg,
        Rgba::rgb(0, 255, 255),
        "256 wraps to 0; 65536 saturates at the parameter and then truncates to 255"
    );
}

/// A truncated extended colour consumes what is there and leaves the pen alone.
#[test]
fn a_truncated_extended_colour_leaves_the_pen_alone() {
    let (fg, _, _) = style_at(b"\x1b[31m\x1b[38;2;10mX", 0);
    assert_eq!(fg, GHOSTTY_ANSI[1], "the red from before survived");
}

/// Codes the grid cannot store are ignored and do not disturb the ones it can.
///
/// Dim, blink, conceal and strikethrough have no bit in `vitrum-grid`'s `Attrs`.
/// Mapping any of them onto a bit that means something else would make the replay
/// lie about the session; dropping them is the honest answer, and the codes around
/// them must still land.
///
/// Ghostty parses all four and stores them; the drop now happens one layer out, in
/// `vitrum_vt::bridge::attrs_of`, for the same reason and with the same result. The
/// contract this file asserts is unchanged: the modelled bits land, the unmodelled
/// ones change nothing, and nothing is invented in their place.
#[test]
fn unmodelled_codes_are_ignored_without_disturbing_the_rest() {
    let (fg, _, attrs) = style_at(b"\x1b[2;5;8;9;1;31mX", 0);
    assert_eq!(attrs, Attrs::BOLD, "only bold, and nothing invented");
    assert_eq!(fg, GHOSTTY_ANSI[1]);

    let (_, _, unknown) = style_at(b"\x1b[1;73;99mX", 0);
    assert_eq!(unknown, Attrs::BOLD, "an unassigned code changes nothing");
}

/// Rendition persists across a line feed and applies to every following cell.
#[test]
fn rendition_persists_until_it_is_changed() {
    let screen = linear(8, 3, b"\x1b[31ma\r\nb");
    let first = screen.grid().cell(0, 0).expect("cell");
    let second = screen.grid().cell(0, 1).expect("cell");
    assert_eq!(first.fg, GHOSTTY_ANSI[1]);
    assert_eq!(second.fg, GHOSTTY_ANSI[1]);
}

/// The 6x6x6 cube and the grey ramp use xterm's uneven level table.
///
/// The bug: evenly spaced levels. The first step is 95, not 51, and getting it wrong
/// shifts every one of the 216 cube colours, which is most of what a modern TUI uses.
///
/// This used to read a lookup table in this crate. It now drives the engine, which
/// is the only table that paints anything, and it is asserted against xterm's
/// published values rather than against whatever the engine happens to return.
#[test]
fn the_colour_cube_and_grey_ramp_use_xterms_level_table() {
    let indexed = |index: u16| {
        let bytes = format!("\x1b[38;5;{index}mX");
        style_at(bytes.as_bytes(), 0).0
    };

    assert_eq!(indexed(16), Rgba::rgb(0, 0, 0), "cube origin");
    assert_eq!(indexed(17), Rgba::rgb(0, 0, 95), "the first step is 95");
    assert_eq!(indexed(231), Rgba::rgb(255, 255, 255), "cube corner");
    assert_eq!(indexed(232), Rgba::rgb(8, 8, 8), "grey ramp starts at 8");
    assert_eq!(indexed(255), Rgba::rgb(238, 238, 238));
    assert_eq!(indexed(120), Rgba::rgb(135, 255, 135));
}

/// A caller's own default colours are used, so a replay inside vitrum matches the
/// live pane.
///
/// Only the two defaults. The sixteen named colours are the engine's and this crate
/// has no way to set them, which is why [`Palette`] no longer carries them; see
/// [`crate::palette`]. Passing a theme's default foreground still works, and it is
/// the one that matters for a blank screen and for `SGR 39`.
#[test]
fn a_custom_palette_replaces_the_defaults() {
    use crate::emulator::Emulator;

    let palette = Palette {
        fg: Rgba::rgb(9, 9, 9),
        bg: Rgba::rgb(3, 2, 1),
    };

    let mut emulator = Emulator::new(8, 2, palette).expect("geometry");
    emulator.feed(b"\x1b[31ma\x1b[39mb").expect("engine readable");
    let screen = emulator.into_screen();

    assert_eq!(
        screen.grid().cell(1, 0).expect("cell").fg,
        Rgba::rgb(9, 9, 9),
        "SGR 39 went back to the caller's foreground"
    );
    assert_eq!(
        screen.grid().cell(4, 1).expect("cell").bg,
        Rgba::rgb(3, 2, 1),
        "an untouched cell is painted in the caller's background"
    );
}
