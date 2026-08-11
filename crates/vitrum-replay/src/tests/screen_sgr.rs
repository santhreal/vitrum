//! Rendition: the named colours, both extended-colour spellings, and the attribute
//! bits the renderer draws.
//!
//! A cell in this product stores absolute channels, so every SGR colour has to be
//! resolved to concrete bytes before it reaches the grid. The engine does that
//! resolution and the daemon paints a live pane through the same engine, so what is
//! asserted here is the colour an operator saw.

use vitrum_grid::{Attrs, Rgba};

use crate::palette::Palette;
use crate::tests::support::{GHOSTTY_ANSI, cell_at, linear};

/// The colour of the first cell after `sgr` is applied.
fn fg_of(sgr: &str) -> Rgba {
    let bytes = format!("\x1b[{sgr}mX");
    cell_at(&linear(8, 2, bytes.as_bytes()), 0, 0).fg
}

/// The background of the first cell after `sgr` is applied.
fn bg_of(sgr: &str) -> Rgba {
    let bytes = format!("\x1b[{sgr}mX");
    cell_at(&linear(8, 2, bytes.as_bytes()), 0, 0).bg
}

/// The attributes of the first cell after `sgr` is applied.
fn attrs_of(sgr: &str) -> Attrs {
    let bytes = format!("\x1b[{sgr}mX");
    cell_at(&linear(8, 2, bytes.as_bytes()), 0, 0).attrs
}

/// `SGR 30..37` and `SGR 90..97` select the sixteen named colours in order.
///
/// Driven from the whole table rather than one representative, because the recurring
/// defect in a colour table is one row out and a single spot check cannot see it.
#[test]
fn the_named_colours_select_the_sixteen_slots_in_order() {
    for (index, want) in GHOSTTY_ANSI.iter().enumerate() {
        let code = if index < 8 { 30 + index } else { 82 + index };
        assert_eq!(fg_of(&code.to_string()), *want, "SGR {code} foreground");

        let bg_code = code + 10;
        assert_eq!(bg_of(&bg_code.to_string()), *want, "SGR {bg_code} background");
    }
}

/// `SGR 39` and `SGR 49` go back to the palette the caller supplied.
///
/// The bug: resolving the default to a hardcoded white on black. The default is a
/// display decision the caller made, so a hardcoded one silently overrides the theme of
/// whatever is showing the replay.
#[test]
fn thirty_nine_and_forty_nine_return_to_the_supplied_palette() {
    assert_eq!(fg_of("31;39"), Palette::DEFAULT.fg);
    assert_eq!(bg_of("44;49"), Palette::DEFAULT.bg);
}

/// `SGR 0`, and the bare `CSI m` with no parameter at all, clear everything.
///
/// The bare form is not a curiosity: `git log --color` emits it, and the capture in
/// this suite contains one. A parser that requires a parameter leaves the last colour
/// latched for the rest of the output, which is how one coloured commit hash turns the
/// remainder of a log red.
#[test]
fn a_reset_clears_colour_and_attributes_in_both_spellings() {
    for reset in ["\x1b[31;44;1;3;4;7m\x1b[0m", "\x1b[31;44;1;3;4;7m\x1b[m"] {
        let cell = cell_at(&linear(8, 2, format!("{reset}X").as_bytes()), 0, 0);
        assert_eq!(cell.fg, Palette::DEFAULT.fg, "reset spelled {reset:?}");
        assert_eq!(cell.bg, Palette::DEFAULT.bg, "reset spelled {reset:?}");
        assert_eq!(cell.attrs, Attrs::NONE, "reset spelled {reset:?}");
    }
}

/// The first sixteen entries of the 256-colour space are the named colours.
#[test]
fn the_low_sixteen_indexed_colours_are_the_named_ones() {
    for (index, want) in GHOSTTY_ANSI.iter().enumerate() {
        assert_eq!(fg_of(&format!("38;5;{index}")), *want, "index {index}");
    }
}

/// The 6x6x6 cube resolves on the standard level ladder.
///
/// Levels are 0, 95, 135, 175, 215, 255, not an even division of 0..255. The even
/// division is the classic bug: it puts every colour in the cube a few points off, which
/// nobody notices until a diff's green stops matching the one in another terminal.
#[test]
fn the_colour_cube_resolves_on_the_standard_level_ladder() {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

    for r in 0..6usize {
        for g in 0..6usize {
            for b in 0..6usize {
                let index = 16 + 36 * r + 6 * g + b;
                assert_eq!(
                    fg_of(&format!("38;5;{index}")),
                    Rgba::rgb(LEVELS[r], LEVELS[g], LEVELS[b]),
                    "cube index {index}"
                );
            }
        }
    }
}

/// The grey ramp runs from 8 to 238 in steps of ten.
#[test]
fn the_grey_ramp_runs_in_steps_of_ten() {
    for step in 0..24u8 {
        let index = 232 + u32::from(step);
        let level = 8 + step * 10;
        assert_eq!(
            fg_of(&format!("38;5;{index}")),
            Rgba::rgb(level, level, level),
            "grey index {index}"
        );
    }
}

/// Truecolour arrives as the exact channels it names.
///
/// The bug: quantising 24-bit colour down to the 256-colour cube. It is invisible on a
/// terminal palette and glaring on a syntax-highlighted diff, and it is why the product
/// advertises truecolour to the child rather than leaving it to be sniffed.
#[test]
fn truecolour_arrives_unquantised() {
    for (r, g, b) in [(0, 0, 0), (1, 2, 3), (0x33, 0x66, 0xcc), (255, 255, 255)] {
        assert_eq!(
            fg_of(&format!("38;2;{r};{g};{b}")),
            Rgba::rgb(r, g, b),
            "foreground {r},{g},{b}"
        );
        assert_eq!(
            bg_of(&format!("48;2;{r};{g};{b}")),
            Rgba::rgb(r, g, b),
            "background {r},{g},{b}"
        );
    }
}

/// The colon spelling of extended colour means the same thing as the semicolon one.
///
/// Both are in the wild. The colon form is what ITU-T T.416 actually specifies and what
/// several toolkits emit; the semicolon form is what everything else emits. A parser
/// that knows only one of them renders half the world's output in the wrong colour, and
/// the semicolon form is additionally ambiguous with plain parameter separation, so the
/// two have to be handled by different code and therefore tested separately.
#[test]
fn the_colon_and_semicolon_spellings_agree() {
    assert_eq!(fg_of("38:5:203"), fg_of("38;5;203"));
    assert_eq!(fg_of("38:2::17:34:51"), Rgba::rgb(17, 34, 51));
    assert_eq!(bg_of("48:2::17:34:51"), Rgba::rgb(17, 34, 51));
}

/// A colon-spelled colour inside a longer parameter list does not swallow what follows.
///
/// This is the reason the colon form exists. `CSI 1;38:5:203;4 m` is bold, a colour, and
/// an underline; a parser that treats colons as separators reads the 5 and the 203 as
/// further attributes and loses the underline.
#[test]
fn a_colon_spelled_colour_does_not_swallow_the_parameters_after_it() {
    let cell = cell_at(&linear(8, 2, b"\x1b[1;38:5:203;4mX"), 0, 0);

    assert_eq!(cell.fg, fg_of("38;5;203"));
    assert!(cell.attrs.contains(Attrs::BOLD));
    assert!(cell.attrs.contains(Attrs::UNDERLINE));
}

/// An index past the end of the 256-colour space still resolves inside the table.
///
/// 300 is not a colour. What must not happen is a read past the end of a 256-entry
/// table, so the assertion is that the result is one of the entries that exist. The
/// engine reaches it by truncating the parameter to eight bits, which is a choice this
/// crate observes rather than specifies: the parameter is out of range either way.
#[test]
fn an_out_of_range_indexed_colour_stays_inside_the_table() {
    let got = fg_of("31;38;5;300");
    let table: Vec<Rgba> = (0..256).map(|i| fg_of(&format!("38;5;{i}"))).collect();

    assert!(
        table.contains(&got),
        "an index of 300 produced {got:?}, which is not in the 256-colour table"
    );
}

/// The four attributes the renderer draws are set and cleared by their own codes.
#[test]
fn each_drawn_attribute_has_its_own_set_and_clear() {
    for (set, clear, bit) in [
        (1u8, 22u8, Attrs::BOLD),
        (3, 23, Attrs::ITALIC),
        (4, 24, Attrs::UNDERLINE),
        (7, 27, Attrs::REVERSE),
    ] {
        assert!(
            attrs_of(&set.to_string()).contains(bit),
            "SGR {set} did not set {bit:?}"
        );
        assert!(
            !attrs_of(&format!("{set};{clear}")).contains(bit),
            "SGR {clear} did not clear {bit:?}"
        );
    }
}

/// Attributes accumulate across separate sequences rather than replacing each other.
#[test]
fn attributes_accumulate_across_sequences() {
    let cell = cell_at(&linear(8, 2, b"\x1b[1m\x1b[4m\x1b[31mX"), 0, 0);

    assert!(cell.attrs.contains(Attrs::BOLD));
    assert!(cell.attrs.contains(Attrs::UNDERLINE));
    assert_eq!(cell.fg, GHOSTTY_ANSI[1]);
}

/// Reverse video is stored as a bit, not applied by swapping the stored colours.
///
/// The renderer resolves it at upload time. Swapping here instead would make the cell
/// report a foreground the session never set, and a second `SGR 7` would swap it back
/// to something different again.
#[test]
fn reverse_is_a_bit_on_the_cell_and_not_a_swap_of_its_colours() {
    let cell = cell_at(&linear(8, 2, b"\x1b[31;44;7mX"), 0, 0);

    assert_eq!(cell.fg, GHOSTTY_ANSI[1]);
    assert_eq!(cell.bg, GHOSTTY_ANSI[4]);
    assert!(cell.attrs.contains(Attrs::REVERSE));
    assert_eq!(
        cell.resolved_colors(),
        (GHOSTTY_ANSI[4], GHOSTTY_ANSI[1]),
        "the swap happens where the renderer reads it"
    );
}

/// Underline style variants are all visible as an underline.
///
/// The shader draws one underline rule, so curly and dotted collapse onto it. Dropping
/// them instead would make a compiler diagnostic's squiggle disappear entirely, which is
/// worse than drawing it straight.
#[test]
fn every_underline_style_is_drawn_as_an_underline() {
    for spelling in ["4", "4:1", "4:2", "4:3", "4:4", "4:5", "21"] {
        assert!(
            attrs_of(spelling).contains(Attrs::UNDERLINE),
            "SGR {spelling} was not drawn as an underline"
        );
    }
    assert!(!attrs_of("4:3;24").contains(Attrs::UNDERLINE));
}

/// The rendition applies from the sequence onwards and not retroactively.
#[test]
fn a_rendition_change_applies_from_that_point_onwards() {
    let screen = linear(8, 2, b"a\x1b[31mb");

    assert_eq!(cell_at(&screen, 0, 0).fg, Palette::DEFAULT.fg);
    assert_eq!(cell_at(&screen, 1, 0).fg, GHOSTTY_ANSI[1]);
}
