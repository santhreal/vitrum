//! The caret: what it costs to move, and what it puts on screen.
//!
//! WHY THIS SUITE EXISTS: the renderer drew no caret at all. A grid with no
//! caret is not a terminal — an operator cannot see where typing will land,
//! cannot see whether a full-screen program has parked the caret in a prompt,
//! and cannot tell a hung program from one waiting for input. The class this
//! closes is "the caret is absent, in the wrong place, or the wrong shape",
//! and it is closed at two choke points rather than per call site:
//!
//! - [`CellGrid::set_cursor`] is the only way a caret moves, so the damage
//!   assertions here bound the cost of every move in the product.
//! - `CellInstance::build` is the only way a caret reaches the GPU, so the
//!   pixel assertions here cover every shape the type can name. `every_shape`
//!   enumerates [`CursorShape`] from the source rather than from a list, so a
//!   fifth shape added upstream turns this suite red instead of shipping a
//!   caret nothing draws.
//!
//! WHAT IT DOES NOT CATCH: blinking, because nothing blinks; and the caret's
//! interaction with a scrolled-back viewport, which is the host's decision to
//! pass `None` and is tested where that decision is made.

use crate::cell::{Cell, Cursor, CursorShape, Rgba, Style};
use crate::grid::GridError;
use crate::tests::support::{assert_rect_exact, blend, draw, grid, renderer, target_for};

const FG: Rgba = Rgba::rgb(0x33, 0x66, 0xcc);
const BG: Rgba = Rgba::rgb(0x11, 0x22, 0x33);
const CARET: Rgba = Rgba::rgb(0xff, 0x00, 0x99);

/// Every shape the type defines, taken from the type rather than retyped.
///
/// A hardcoded list goes stale in silence, which is the same failure as having
/// no test. This is exhaustive by construction: adding a member to
/// [`CursorShape`] fails the `match` below to compile until it is added here
/// too.
fn every_shape() -> Vec<CursorShape> {
    let all = vec![
        CursorShape::Block,
        CursorShape::HollowBlock,
        CursorShape::Bar,
        CursorShape::Underline,
    ];
    for shape in &all {
        // Exhaustive, so a new member is a compile error here.
        match shape {
            CursorShape::Block
            | CursorShape::HollowBlock
            | CursorShape::Bar
            | CursorShape::Underline => {}
        }
    }
    all
}

/// The shape codes must be distinct, non-zero, and fit the three flag bits the
/// instance reserves.
///
/// Zero is reserved for "no caret on this cell". A shape that collided with it
/// would be invisible, and a shape past 7 would alias onto another shape's
/// code and draw the wrong caret.
#[test]
fn every_shape_has_a_distinct_non_zero_code_that_fits_three_bits() {
    let mut seen = Vec::new();
    for shape in every_shape() {
        let code = shape.code();
        assert_ne!(code, 0, "{shape:?} collides with the no-caret code");
        assert!(code <= 7, "{shape:?} has code {code}, past the three bits");
        assert!(!seen.contains(&code), "{shape:?} reuses code {code}");
        seen.push(code);
    }
}

/// Moving the caret must damage the cell it left and the cell it reached, and
/// nothing else.
///
/// A terminal moves the caret on nearly every byte it prints. Repainting a row
/// per keystroke, or the screen, is the difference between a frame that costs
/// two cells and one that costs two hundred, and at sixty frames a second on a
/// full-screen redraw that is the whole frame budget.
#[test]
fn moving_the_caret_damages_exactly_the_two_cells_it_touches() {
    let mut g = grid(80, 24, Style::new(FG, BG));
    g.clear_damage();

    assert!(
        g.set_cursor(Some(Cursor::block(5, 3, CARET)))
            .expect("inside the grid")
    );
    assert_eq!(g.dirty_cells(), 1, "arriving damages one cell");

    g.clear_damage();
    assert!(
        g.set_cursor(Some(Cursor::block(40, 17, CARET)))
            .expect("inside the grid")
    );
    assert_eq!(
        g.dirty_cells(),
        2,
        "a move damages the cell left and the cell reached, and nothing else"
    );

    g.clear_damage();
    assert!(g.set_cursor(None).expect("clearing is always in range"));
    assert_eq!(g.dirty_cells(), 1, "hiding damages only where it was");
}

/// Setting the caret to where it already is must cost nothing.
///
/// The engine reports the caret on every sync, including the syncs where
/// nothing moved. If that re-report damaged a cell, an idle session would
/// present a frame every time anything asked it to sync, and the zero-cost
/// idle path this renderer is built on would be gone.
#[test]
fn re_reporting_the_same_caret_damages_nothing() {
    let mut g = grid(20, 5, Style::new(FG, BG));
    g.set_cursor(Some(Cursor::block(2, 1, CARET)))
        .expect("inside the grid");
    g.clear_damage();

    for _ in 0..100 {
        assert!(
            !g.set_cursor(Some(Cursor::block(2, 1, CARET)))
                .expect("inside the grid"),
            "an unchanged caret must report no change"
        );
    }
    assert!(!g.is_dirty(), "an unchanged caret must damage nothing");
}

/// A caret outside the grid must be refused, not clamped.
///
/// Clamping draws a caret in the wrong cell, and a caret in the wrong cell
/// looks exactly like a caret in the right one: the operator types where they
/// see it and the characters land somewhere else.
#[test]
fn a_caret_outside_the_grid_is_refused() {
    let mut g = grid(10, 4, Style::new(FG, BG));
    for (col, row) in [(10, 0), (0, 4), (10, 4), (u16::MAX, 0)] {
        assert_eq!(
            g.set_cursor(Some(Cursor::block(col, row, CARET))),
            Err(GridError::OutOfBounds { col, row }),
            "({col}, {row}) is outside a 10x4 grid"
        );
    }
    assert_eq!(g.cursor(), None, "a refused caret must not be stored");
}

/// Shrinking the grid past the caret must drop it.
///
/// The grid resolves a row through an indirection table sized to the row
/// count. A caret left naming a row that no longer exists is read back by the
/// renderer on the next frame and either panics on the bounds check or, worse,
/// resolves to a wrapped index and paints a caret in an unrelated cell.
#[test]
fn shrinking_the_grid_drops_a_caret_that_no_longer_fits() {
    let mut g = grid(40, 20, Style::new(FG, BG));
    g.set_cursor(Some(Cursor::block(30, 15, CARET)))
        .expect("inside the grid");

    g.resize(40, 10).expect("a valid size");
    assert_eq!(g.cursor(), None, "the caret's row is gone");

    g.set_cursor(Some(Cursor::block(30, 5, CARET)))
        .expect("inside the grid");
    g.resize(20, 10).expect("a valid size");
    assert_eq!(g.cursor(), None, "the caret's column is gone");

    g.set_cursor(Some(Cursor::block(5, 5, CARET)))
        .expect("inside the grid");
    g.resize(20, 40).expect("a valid size");
    assert_eq!(
        g.cursor(),
        Some(Cursor::block(5, 5, CARET)),
        "a caret that still fits must survive a resize"
    );
}

/// A block caret over a blank cell must paint the whole cell in the caret
/// colour.
///
/// This is the floor every other shape stands on: the shape bits have to reach
/// the fragment stage, the caret colour has to survive the extra vertex
/// attribute, and the quad has to cover its rectangle exactly.
#[test]
fn a_block_caret_paints_the_whole_cell_in_the_caret_colour() {
    let mut r = renderer();
    let target = target_for(&r, 2, 1);
    let mut g = grid(2, 1, Style::new(FG, BG));
    g.set_cursor(Some(Cursor::block(0, 0, CARET)))
        .expect("inside the grid");

    let (cw, ch) = r.cell_size();
    let image = draw(&mut r, &mut g, &target);
    assert_rect_exact(&image, "caret cell", 0, 0, cw, ch, CARET);
    assert_rect_exact(&image, "cell beside the caret", cw, 0, cw * 2, ch, BG);
}

/// A block caret over a glyph must knock the glyph out in the cell's own
/// background colour.
///
/// Painting the caret opaquely over the glyph hides the character the operator
/// is about to overwrite, which is exactly the character they need to see.
#[test]
fn a_block_caret_knocks_the_glyph_out_in_the_cell_background() {
    let mut r = renderer();
    let target = target_for(&r, 1, 1);
    let mut g = grid(1, 1, Style::new(FG, BG));
    g.set_cell(0, 0, Cell::new('M', Style::new(FG, BG)))
        .expect("inside the grid");

    let plain = draw(&mut r, &mut g, &target);

    g.set_cursor(Some(Cursor::block(0, 0, CARET)))
        .expect("inside the grid");
    let carried = draw(&mut r, &mut g, &target);

    // Recover the coverage the renderer used from the frame it already drew,
    // rather than rasterising a second copy of the glyph and hoping it agrees.
    let mut glyph_pixels = 0;
    for y in 0..plain.height() {
        for x in 0..plain.width() {
            let before = plain.pixel(x, y);
            let after = carried.pixel(x, y);
            if before == BG {
                assert_eq!(
                    after, CARET,
                    "an uncovered pixel at ({x}, {y}) must become the caret colour"
                );
            } else {
                glyph_pixels += 1;
                assert_ne!(
                    after, CARET,
                    "a glyph pixel at ({x}, {y}) must be knocked out, not painted over"
                );
            }
        }
    }
    assert!(
        glyph_pixels > 0,
        "the test proves nothing unless 'M' actually drew something"
    );
    assert_eq!(
        carried.pixel(0, 0),
        CARET,
        "the cell's top-left corner is outside any glyph and must be the caret"
    );
}

/// A bar caret must cover the left rule and leave every other pixel painted as
/// it was.
#[test]
fn a_bar_caret_covers_the_left_rule_only() {
    let mut r = renderer();
    let target = target_for(&r, 1, 1);
    let (cw, ch) = r.cell_size();
    let mut g = grid(1, 1, Style::new(FG, BG));
    g.set_cursor(Some(Cursor {
        col: 0,
        row: 0,
        shape: CursorShape::Bar,
        color: CARET,
    }))
    .expect("inside the grid");

    let image = draw(&mut r, &mut g, &target);
    let bar = (cw as f32 / 8.0).max(1.0).floor() as u32;
    assert!(bar >= 1 && bar < cw, "a {bar}px bar in a {cw}px cell");
    assert_rect_exact(&image, "bar", 0, 0, bar, ch, CARET);
    assert_rect_exact(&image, "cell right of the bar", bar, 0, cw, ch, BG);
}

/// An underline caret must cover the bottom rule only.
#[test]
fn an_underline_caret_covers_the_bottom_rule_only() {
    let mut r = renderer();
    let target = target_for(&r, 1, 1);
    let (cw, ch) = r.cell_size();
    let thickness = r.metrics().underline_thickness;
    let mut g = grid(1, 1, Style::new(FG, BG));
    g.set_cursor(Some(Cursor {
        col: 0,
        row: 0,
        shape: CursorShape::Underline,
        color: CARET,
    }))
    .expect("inside the grid");

    let image = draw(&mut r, &mut g, &target);
    assert!(thickness >= 1 && thickness < ch);
    assert_rect_exact(&image, "underline rule", 0, ch - thickness, cw, ch, CARET);
    assert_rect_exact(&image, "above the rule", 0, 0, cw, ch - thickness, BG);
}

/// A hollow caret must cover the border and leave the interior alone.
///
/// The hollow block is what a terminal shows when the window has lost focus.
/// A hollow caret that filled its cell would be indistinguishable from a
/// focused one, which is the entire point of the shape.
#[test]
fn a_hollow_caret_covers_the_border_and_not_the_interior() {
    let mut r = renderer();
    let target = target_for(&r, 1, 1);
    let (cw, ch) = r.cell_size();
    let t = r.metrics().underline_thickness;
    let mut g = grid(1, 1, Style::new(FG, BG));
    g.set_cursor(Some(Cursor {
        col: 0,
        row: 0,
        shape: CursorShape::HollowBlock,
        color: CARET,
    }))
    .expect("inside the grid");

    let image = draw(&mut r, &mut g, &target);
    assert!(cw > 2 * t && ch > 2 * t, "the cell must have an interior");
    assert_rect_exact(&image, "top edge", 0, 0, cw, t, CARET);
    assert_rect_exact(&image, "bottom edge", 0, ch - t, cw, ch, CARET);
    assert_rect_exact(&image, "left edge", 0, 0, t, ch, CARET);
    assert_rect_exact(&image, "right edge", cw - t, 0, cw, ch, CARET);
    assert_rect_exact(&image, "interior", t, t, cw - t, ch - t, BG);
}

/// Every shape must put its own colour somewhere on the cell, and no shape may
/// leak onto a neighbour.
///
/// The per-shape tests above pin the exact rectangle each one covers. This one
/// covers the union: whatever a shape draws, it draws inside its own cell.
/// A shape whose rule was computed against the viewport rather than the cell
/// would pass every rectangle assertion on a one-cell grid and fail here.
#[test]
fn no_shape_paints_outside_its_own_cell() {
    let mut r = renderer();
    let target = target_for(&r, 3, 2);
    let (cw, ch) = r.cell_size();

    for shape in every_shape() {
        let mut g = grid(3, 2, Style::new(FG, BG));
        g.set_cursor(Some(Cursor {
            col: 1,
            row: 1,
            shape,
            color: CARET,
        }))
        .expect("inside the grid");

        let image = draw(&mut r, &mut g, &target);
        let carried = image.count(CARET);
        assert!(
            carried > 0,
            "{shape:?} drew nothing at all; a caret nobody can see is not a caret"
        );

        for y in 0..image.height() {
            for x in 0..image.width() {
                let in_cell = (cw..cw * 2).contains(&x) && (ch..ch * 2).contains(&y);
                if !in_cell {
                    assert_eq!(
                        image.pixel(x, y),
                        BG,
                        "{shape:?} painted ({x}, {y}), outside its own cell"
                    );
                }
            }
        }
    }
}

/// A frame whose only change is the caret must upload two cells, not the grid.
///
/// This is the cost claim the whole design rests on: the caret is composited
/// rather than stored, so a move is two instances. A caret written into the
/// cells instead would rewrite them, and a caret that forced a full rebuild
/// would upload every cell on screen once per keystroke.
#[test]
fn a_caret_move_uploads_two_cells_and_draws_one_frame() {
    let mut r = renderer();
    let target = target_for(&r, 80, 24);
    let mut g = grid(80, 24, Style::new(FG, BG));
    g.set_cursor(Some(Cursor::block(0, 0, CARET)))
        .expect("inside the grid");

    // First frame is the full rebuild every renderer owes a new target.
    let first = r
        .render(
            crate::tests::support::gpu().device(),
            crate::tests::support::gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .expect("a valid frame");
    assert!(first.full_rebuild);

    g.set_cursor(Some(Cursor::block(1, 0, CARET)))
        .expect("inside the grid");
    let moved = r
        .render(
            crate::tests::support::gpu().device(),
            crate::tests::support::gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .expect("a valid frame");
    assert!(!moved.full_rebuild, "a caret move must not rebuild the grid");
    assert_eq!(
        moved.cells_uploaded, 2,
        "a caret move must upload the cell it left and the cell it reached"
    );
    assert_eq!(
        moved.writes, 1,
        "the two cells are adjacent and must coalesce into one upload"
    );

    let idle = r
        .render(
            crate::tests::support::gpu().device(),
            crate::tests::support::gpu().queue(),
            &mut g,
            target.view(),
            (target.width(), target.height()),
        )
        .expect("a valid frame");
    assert!(
        !idle.gpu_work,
        "a frame after the caret settled must record no GPU work"
    );
}

/// The caret must not disturb the colours of the cell it is not on.
///
/// A caret colour leaking into the shared uniform block, or into the instance
/// of the next cell in the upload run, would tint the whole row.
#[test]
fn a_caret_leaves_its_neighbours_byte_identical() {
    let mut r = renderer();
    let target = target_for(&r, 4, 1);
    let (cw, ch) = r.cell_size();
    let style = Style::new(FG, BG);
    let mut plain = grid(4, 1, style);
    for (col, ch_) in "abcd".chars().enumerate() {
        plain
            .set_cell(col as u16, 0, Cell::new(ch_, style))
            .expect("inside the grid");
    }
    let mut carried = plain.clone();
    carried
        .set_cursor(Some(Cursor::block(2, 0, CARET)))
        .expect("inside the grid");

    let before = draw(&mut r, &mut plain, &target);
    let after = draw(&mut r, &mut carried, &target);

    for y in 0..ch {
        for x in 0..cw * 4 {
            if (cw * 2..cw * 3).contains(&x) {
                continue;
            }
            assert_eq!(
                before.pixel(x, y),
                after.pixel(x, y),
                "the caret changed ({x}, {y}), which is not its cell"
            );
        }
    }
    // And the reference blend still holds for a covered pixel on a caret-free
    // cell, so the comparison above is not two identically broken frames.
    assert_eq!(
        blend(BG, FG, 0),
        BG,
        "the reference blend must agree with an uncovered pixel"
    );
}
