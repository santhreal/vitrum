//! The veil: dimming the pane from inside its own renderer.
//!
//! A pane draws into a native child window. A toolkit scrim laid over it is a
//! separate window of its own, and separate windows do not composite: a
//! translucent fill paints opaque and the terminal goes black instead of dim.
//! So the dimming lives here, and these tests hold the two halves of it
//! together:
//!
//! - every colour the fragment stage can return is veiled, including each
//!   caret shape, so no path escapes and stays bright under a sheet;
//! - the clear colour is veiled to exactly the same value the shader produces,
//!   because the clear is the one pixel source the fragment stage never sees
//!   and a mismatch there is a bright seam around dimmed cells.

use crate::cell::{Attrs, Cursor, CursorShape, Rgba, Style};
use crate::renderer::veil_over;
use crate::tests::support::{
    TEST_PX, assert_rect_exact, draw, gpu, grid, renderer, target_for,
};
use crate::HeadlessTarget;

const FG: Rgba = Rgba::rgb(0x33, 0x66, 0xcc);
/// Chosen so a half-strength veil towards black lands on whole bytes: every
/// channel is even, so `c / 2` needs no rounding and the assertion can be
/// exact rather than tolerant.
const BG: Rgba = Rgba::rgb(0x10, 0x20, 0x40);
const VEIL: Rgba = Rgba::rgb(0x00, 0x00, 0x00);
const CARET: Rgba = Rgba::rgb(0xff, 0xcc, 0x00);

/// Every caret shape the type can name, checked exhaustively by the compiler.
///
/// The fragment stage returns early for three of the four, and an early return
/// that skipped the veil would leave a bright caret burning through a dimmed
/// pane. A fifth shape added upstream fails the `match` to compile until it is
/// listed here.
fn every_shape() -> Vec<CursorShape> {
    let all = vec![
        CursorShape::Block,
        CursorShape::HollowBlock,
        CursorShape::Bar,
        CursorShape::Underline,
    ];
    for shape in &all {
        match shape {
            CursorShape::Block
            | CursorShape::HollowBlock
            | CursorShape::Bar
            | CursorShape::Underline => {}
        }
    }
    all
}

/// A veil at full strength must leave the veil colour and nothing else, for
/// every caret shape and over a glyph.
///
/// Full strength is the one setting whose result is knowable exactly without
/// mirroring the blend, so it is the assertion that proves coverage of every
/// return path in the fragment stage: if any branch returned unveiled, its
/// pixels would keep the colour they had.
#[test]
fn a_veil_at_full_strength_leaves_only_its_own_colour() {
    for shape in every_shape() {
        let mut renderer = renderer();
        let target = target_for(&renderer, 2, 1);
        let mut g = grid(2, 1, Style::new(FG, BG));
        g.write_char(0, 0, 'M', Style::new(FG, BG).with_attrs(Attrs::UNDERLINE))
            .expect("inside the grid");
        g.write_char(1, 0, 'W', Style::new(FG, BG))
            .expect("inside the grid");
        g.set_cursor(Some(Cursor {
            col: 1,
            row: 0,
            shape,
            color: CARET,
        }))
        .expect("inside the grid");

        renderer.set_veil(VEIL, 1.0);
        let image = draw(&mut renderer, &mut g, &target);
        assert_rect_exact(
            &image,
            &format!("{shape:?} under a full veil"),
            0,
            0,
            image.width(),
            image.height(),
            VEIL,
        );
    }
}

/// A half-strength veil must move every pixel halfway to the veil colour, and
/// no pixel may be left at the colour it had.
#[test]
fn a_half_veil_moves_every_pixel_halfway() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let mut g = grid(1, 1, Style::new(FG, BG));

    renderer.set_veil(VEIL, 0.5);
    let image = draw(&mut renderer, &mut g, &target);
    let want = Rgba::rgb(BG.r / 2, BG.g / 2, BG.b / 2);
    assert_rect_exact(
        &image,
        "blank cell under a half veil",
        0,
        0,
        image.width(),
        image.height(),
        want,
    );
    assert_eq!(
        image.count(BG),
        0,
        "no pixel may keep the undimmed background"
    );
}

/// Zero strength must be byte-identical to no veil at all.
///
/// The veil is applied on every frame of every session, almost always at zero.
/// A blend that shifted the picture by a bit when nothing is dimmed would make
/// the ordinary case wrong to pay for the rare one.
#[test]
fn a_veil_at_zero_strength_changes_no_pixel() {
    let mut plain = renderer();
    let target = target_for(&plain, 2, 1);
    let mut g = grid(2, 1, Style::new(FG, BG));
    g.write_char(0, 0, 'M', Style::new(FG, BG))
        .expect("inside the grid");
    let before = draw(&mut plain, &mut g, &target);

    plain.set_veil(VEIL, 0.0);
    plain.invalidate();
    let after = draw(&mut plain, &mut g, &target);

    for y in 0..before.height() {
        for x in 0..before.width() {
            assert_eq!(
                before.pixel(x, y),
                after.pixel(x, y),
                "pixel ({x}, {y}) changed under a zero-strength veil"
            );
        }
    }
}

/// The veiled clear must equal the veiled cells, byte for byte.
///
/// The clear fills the pixels no instance covers and is computed on the host,
/// while the cells are veiled in the shader. Two implementations of one blend
/// is exactly where a seam comes from: a border of brighter pixels around a
/// dimmed grid. The target here is wider than the grid, so the clear is on
/// screen and can be read rather than assumed.
#[test]
fn the_clear_is_veiled_to_the_same_colour_as_a_cell() {
    let mut renderer = renderer();
    let (w, h) = renderer.pixel_size_for(1, 1);
    // Half a cell of margin on the right: pixels no instance covers, so what
    // is there is the clear colour and nothing else.
    let target = HeadlessTarget::new(gpu().device(), w + w / 2, h);
    let mut g = grid(1, 1, Style::new(FG, BG));

    renderer.set_veil(VEIL, 0.5);
    let image = draw(&mut renderer, &mut g, &target);
    let cell = image.pixel(0, 0);
    let margin = image.pixel(w + w / 4, h / 2);

    assert_eq!(
        margin, cell,
        "the clear and the cells must land on the same veiled colour"
    );
    assert_eq!(
        cell,
        Rgba::rgb(BG.r / 2, BG.g / 2, BG.b / 2),
        "and that colour must be the background, half way to the veil"
    );

    let host = veil_over(BG, [0.0, 0.0, 0.0, 0.5]);
    assert_eq!(
        Rgba::new(
            (host.r * 255.0).round() as u8,
            (host.g * 255.0).round() as u8,
            (host.b * 255.0).round() as u8,
            (host.a * 255.0).round() as u8,
        ),
        cell,
        "the host-side blend and the shader must agree, byte for byte"
    );
}

/// Changing the veil must owe a frame; re-stating it must not.
///
/// Nothing in the grid changes when a sheet opens, so the damage the renderer
/// normally trusts says there is nothing to draw. The veil has to force the
/// redraw itself. It equally has to stop forcing one: the shell re-states the
/// current layer on every state change, and a veil that invalidated on each of
/// those would repaint the whole pane for every keystroke typed into a dialog.
#[test]
fn changing_the_veil_owes_a_frame_and_restating_it_does_not() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let mut g = grid(1, 1, Style::new(FG, BG));
    let _ = draw(&mut renderer, &mut g, &target);
    assert!(
        !renderer.needs_rebuild(),
        "a drawn frame leaves nothing owed"
    );

    renderer.set_veil(VEIL, 0.5);
    assert!(renderer.needs_rebuild(), "a new veil must force a redraw");

    let _ = draw(&mut renderer, &mut g, &target);
    renderer.set_veil(VEIL, 0.5);
    assert!(
        !renderer.needs_rebuild(),
        "the same veil, stated again, must cost nothing"
    );
    assert_eq!(renderer.veil_strength(), 0.5);
}

/// Strength outside 0..=1 must be clamped, not passed through.
///
/// The strength arrives from settings and from animation, and a value past one
/// inverts the mix in the shader: the picture would come back brighter than it
/// started, tinted the complement of the veil.
#[test]
fn strength_is_clamped_to_the_unit_range() {
    let mut renderer = renderer();
    renderer.set_veil(VEIL, 4.0);
    assert_eq!(renderer.veil_strength(), 1.0);
    renderer.set_veil(VEIL, -1.0);
    assert_eq!(renderer.veil_strength(), 0.0);
    renderer.set_veil(VEIL, f32::NAN);
    assert_eq!(
        renderer.veil_strength(),
        0.0,
        "a NaN strength must land on no veil, not on an undefined mix"
    );
}

/// The uniform block the host writes must be the size the shader's own layout
/// rules give it.
///
/// WGSL aligns a `vec4` to sixteen bytes, so the veil sits at offset 48 and
/// the block is 64 bytes. A packed Rust struct would be 56 and every pipeline
/// creation would fail validation, which is how this was found the first time.
#[test]
fn the_uniform_block_is_the_size_the_shader_expects() {
    assert_eq!(core::mem::size_of::<crate::renderer::Globals>(), 64);
    assert!(TEST_PX > 0.0);
}
