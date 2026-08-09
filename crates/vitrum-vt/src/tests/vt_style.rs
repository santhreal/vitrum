//! How colour and rendition reach a cell.
//!
//! The engine reports a cell's colour as "mine" or "the terminal default", and
//! the grid stores absolute colour. Every test here is about that substitution
//! being right, because a wrong default is invisible in the engine and obvious
//! on screen.

use vitrum_grid::cell::{Attrs, Rgba};

use super::support::Fixture;

#[test]
fn truecolor_reaches_the_cell_exactly() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[38;2;255;128;0mx");
    assert_eq!(fx.cell(0, 0).fg, Rgba::rgb(255, 128, 0));
}

/// The promise this crate publishes is one it keeps.
///
/// [`crate::COLORTERM`] is what a host puts in every child's environment, and
/// an agent reads it to decide whether to emit 24-bit colour at all. The value
/// and the behaviour are asserted in one place so they cannot drift: quantise
/// in the engine, or weaken the claim, and this fails.
///
/// The colours are chosen to be nowhere near the 256-colour cube's grid, so a
/// quantising engine cannot pass by luck. What this does NOT catch is a host
/// that fails to set the variable at all; `vitrum-core` owns that half.
#[test]
fn the_engine_keeps_the_promise_this_crate_makes() {
    assert_eq!(crate::COLORTERM, "truecolor");

    let mut fx = Fixture::new(10, 1);
    // Each `x` advances the cursor, so colour N lands in column N. Asserting
    // every column afterwards also proves the colours did not bleed into each
    // other, which a shared-style bug would do while a single write passed.
    let want = [(1u8, 2u8, 3u8), (17, 34, 51), (254, 253, 252), (7, 200, 91)];
    for (r, g, b) in want {
        fx.write(format!("\x1b[38;2;{r};{g};{b}mx").as_bytes());
    }
    for (col, (r, g, b)) in want.into_iter().enumerate() {
        assert_eq!(
            fx.cell(col as u16, 0).fg,
            Rgba::rgb(r, g, b),
            "advertising truecolor means reproducing it exactly, column {col}"
        );
    }
}

#[test]
fn a_background_colour_reaches_the_cell() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[48;2;10;20;30mx");
    assert_eq!(fx.cell(0, 0).bg, Rgba::rgb(10, 20, 30));
}

#[test]
fn a_palette_index_is_resolved_to_a_colour() {
    // The grid has no palette, so an unresolved index would arrive as the
    // default colour and the text would silently lose its colour.
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[31mx");
    let red = fx.cell(0, 0).fg;
    assert_ne!(red, fx.cell(1, 0).fg, "palette red must differ from default");
    assert!(red.r > red.g && red.r > red.b, "palette red is reddish: {red:?}");
}

#[test]
fn an_uncoloured_cell_takes_the_terminal_default() {
    let mut fx = Fixture::new(10, 1);
    fx.vt
        .set_theme(Rgba::rgb(1, 2, 3), Rgba::rgb(4, 5, 6), None)
        .expect("theme applies");
    fx.write(b"x");

    let cell = fx.cell(0, 0);
    assert_eq!(cell.fg, Rgba::rgb(1, 2, 3));
    assert_eq!(cell.bg, Rgba::rgb(4, 5, 6));
}

#[test]
fn a_blank_cell_still_carries_the_default_background() {
    // Blanks are what most of the screen is. If they miss the background the
    // window shows the renderer's clear colour in the gaps.
    let mut fx = Fixture::new(4, 1);
    fx.vt
        .set_theme(Rgba::rgb(1, 2, 3), Rgba::rgb(9, 9, 9), None)
        .expect("theme applies");
    fx.write(b"x");

    assert_eq!(fx.cell(3, 0).bg, Rgba::rgb(9, 9, 9));
}

#[test]
fn bold_italic_and_underline_become_attribute_bits() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[1;3;4mx");

    let attrs = fx.cell(0, 0).attrs;
    assert!(attrs.contains(Attrs::BOLD));
    assert!(attrs.contains(Attrs::ITALIC));
    assert!(attrs.contains(Attrs::UNDERLINE));
    assert!(!attrs.contains(Attrs::REVERSE));
}

#[test]
fn every_underline_style_still_draws_an_underline() {
    // The shader draws one rule. Curly and dotted must not collapse to nothing,
    // because "no underline" is a different meaning from "a plain underline".
    for sgr in [b"\x1b[4m".as_slice(), b"\x1b[4:3m", b"\x1b[4:4m", b"\x1b[21m"] {
        let mut fx = Fixture::new(4, 1);
        fx.write(sgr);
        fx.write(b"x");
        assert!(
            fx.cell(0, 0).attrs.contains(Attrs::UNDERLINE),
            "underline missing for {:?}",
            String::from_utf8_lossy(sgr)
        );
    }
}

#[test]
fn reverse_video_is_a_bit_and_not_a_colour_swap() {
    // Swapping the colours here would be wrong twice: the renderer already
    // applies reverse, and a cell that swapped colours could not be un-reversed
    // when the program turns the attribute off.
    let mut fx = Fixture::new(10, 1);
    fx.vt
        .set_theme(Rgba::rgb(200, 200, 200), Rgba::rgb(0, 0, 0), None)
        .expect("theme applies");
    fx.write(b"\x1b[7mx");

    let cell = fx.cell(0, 0);
    assert!(cell.attrs.contains(Attrs::REVERSE));
    assert_eq!(cell.fg, Rgba::rgb(200, 200, 200));
    assert_eq!(cell.bg, Rgba::rgb(0, 0, 0));
    assert_eq!(cell.resolved_colors(), (Rgba::rgb(0, 0, 0), Rgba::rgb(200, 200, 200)));
}

#[test]
fn a_reset_clears_the_attributes_that_follow_it() {
    let mut fx = Fixture::new(10, 1);
    fx.write(b"\x1b[1;4ma\x1b[0mb");

    assert!(fx.cell(0, 0).attrs.contains(Attrs::BOLD));
    assert!(fx.cell(1, 0).attrs.is_empty());
}

#[test]
fn a_program_can_override_the_default_background() {
    // OSC 11 is how a program (or a theme-aware shell) repaints the window.
    let mut fx = Fixture::new(4, 1);
    fx.vt
        .set_theme(Rgba::rgb(255, 255, 255), Rgba::rgb(0, 0, 0), None)
        .expect("theme applies");
    fx.write(b"x");
    assert_eq!(fx.cell(0, 0).bg, Rgba::rgb(0, 0, 0));

    fx.write(b"\x1b]11;rgb:20/30/40\x1b\\");
    fx.write(b"\x1b[2J");
    assert_eq!(fx.cell(0, 0).bg, Rgba::rgb(0x20, 0x30, 0x40));
}
