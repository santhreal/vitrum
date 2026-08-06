//! Graphic rendition, including both extended colour spellings.

use vitrum_grid::{Attrs, Rgba};

use crate::palette::Palette;
use crate::tests::support::linear;

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

/// The eight normal and eight bright colours resolve through the palette.
#[test]
fn the_sixteen_ansi_colours_resolve_through_the_palette() {
    for index in 0u8..8 {
        let bytes = format!("\x1b[{}mX", 30 + u16::from(index));
        let (fg, _, _) = style_at(bytes.as_bytes(), 0);
        assert_eq!(fg, Palette::XTERM.indexed(index), "SGR 3{index}");

        let bright = format!("\x1b[{}mX", 90 + u16::from(index));
        let (fg, _, _) = style_at(bright.as_bytes(), 0);
        assert_eq!(fg, Palette::XTERM.indexed(index + 8), "SGR 9{index}");
    }
    let (_, bg, _) = style_at(b"\x1b[44mX", 0);
    assert_eq!(bg, Palette::XTERM.indexed(4));
    let (_, bg, _) = style_at(b"\x1b[104mX", 0);
    assert_eq!(bg, Palette::XTERM.indexed(12));
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
    assert_eq!(fg, Palette::XTERM.indexed(120));

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
    assert_eq!(fg, Palette::XTERM.indexed(120));
}

/// A channel above 255 saturates instead of wrapping.
///
/// `38;2;300;0;0` is malformed. Truncating to eight bits turns "brighter than red"
/// into dark red, which looks like a colour the program chose.
#[test]
fn an_out_of_range_channel_saturates() {
    let (fg, _, _) = style_at(b"\x1b[38;2;300;999;70000mX", 0);
    assert_eq!(fg, Rgba::rgb(255, 255, 255));
}

/// A truncated extended colour consumes what is there and leaves the pen alone.
#[test]
fn a_truncated_extended_colour_leaves_the_pen_alone() {
    let (fg, _, _) = style_at(b"\x1b[31m\x1b[38;2;10mX", 0);
    assert_eq!(fg, Palette::XTERM.indexed(1), "the red from before survived");
}

/// Codes the grid cannot store are ignored and do not disturb the ones it can.
///
/// Dim, blink, conceal and strikethrough have no bit in `vitrum-grid`'s `Attrs`.
/// Mapping any of them onto a bit that means something else would make the replay
/// lie about the session; dropping them is the honest answer, and the codes around
/// them must still land.
#[test]
fn unmodelled_codes_are_ignored_without_disturbing_the_rest() {
    let (fg, _, attrs) = style_at(b"\x1b[2;5;8;9;1;31mX", 0);
    assert_eq!(attrs, Attrs::BOLD, "only bold, and nothing invented");
    assert_eq!(fg, Palette::XTERM.indexed(1));

    let (_, _, unknown) = style_at(b"\x1b[1;73;99mX", 0);
    assert_eq!(unknown, Attrs::BOLD, "an unassigned code changes nothing");
}

/// Rendition persists across a line feed and applies to every following cell.
#[test]
fn rendition_persists_until_it_is_changed() {
    let screen = linear(8, 3, b"\x1b[31ma\r\nb");
    let first = screen.grid().cell(0, 0).expect("cell");
    let second = screen.grid().cell(0, 1).expect("cell");
    assert_eq!(first.fg, Palette::XTERM.indexed(1));
    assert_eq!(second.fg, Palette::XTERM.indexed(1));
}

/// The 6x6x6 cube and the grey ramp use xterm's uneven level table.
///
/// The bug: evenly spaced levels. The first step is 95, not 51, and getting it wrong
/// shifts every one of the 216 cube colours, which is most of what a modern TUI uses.
#[test]
fn the_colour_cube_and_grey_ramp_use_xterms_level_table() {
    let palette = Palette::XTERM;
    assert_eq!(palette.indexed(16), Rgba::rgb(0, 0, 0), "cube origin");
    assert_eq!(palette.indexed(17), Rgba::rgb(0, 0, 95), "the first step is 95");
    assert_eq!(palette.indexed(231), Rgba::rgb(255, 255, 255), "cube corner");
    assert_eq!(palette.indexed(232), Rgba::rgb(8, 8, 8), "grey ramp starts at 8");
    assert_eq!(palette.indexed(255), Rgba::rgb(238, 238, 238));
    assert_eq!(palette.indexed(120), Rgba::rgb(135, 255, 135));
}

/// A caller's own palette is used, so a replay inside vitrum matches the live pane.
#[test]
fn a_custom_palette_replaces_the_defaults() {
    use crate::emulator::Emulator;

    let mut palette = Palette::XTERM;
    palette.ansi[1] = Rgba::rgb(1, 2, 3);
    palette.fg = Rgba::rgb(9, 9, 9);

    let mut emulator = Emulator::new(8, 2, palette).expect("geometry");
    emulator.feed(b"\x1b[31ma\x1b[39mb");
    let screen = emulator.into_screen();
    assert_eq!(screen.grid().cell(0, 0).expect("cell").fg, Rgba::rgb(1, 2, 3));
    assert_eq!(screen.grid().cell(1, 0).expect("cell").fg, Rgba::rgb(9, 9, 9));
}
