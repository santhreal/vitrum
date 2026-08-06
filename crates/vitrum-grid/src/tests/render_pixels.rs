//! Headless renders with exact pixel assertions.
//!
//! Every test here draws into an offscreen texture, reads it back, and compares
//! against a value computed on the CPU. Nothing asserts "not empty": either the
//! expected colour is known exactly (a blank cell, an underline rule, a
//! background span) or it is derived from the same glyph bitmap the renderer
//! used, pixel by pixel.

use crate::cell::{Attrs, Cell, CellSlot, Rgba, Style};
use crate::font::{FontStyle, RasterGlyph};
use crate::gpu::AdapterClass;
use crate::grid::CellGrid;
use crate::renderer::RenderError;
use crate::tests::support::{
    TEST_PX, assert_matches_reference, assert_rect_exact, blend, draw, fonts, gpu, grid, renderer,
    target_for,
};
use crate::{FontConfig, GpuContext, GridRenderer, HeadlessTarget, RendererConfig};

const FG: Rgba = Rgba::rgb(0x33, 0x66, 0xcc);
const BG: Rgba = Rgba::rgb(0x11, 0x22, 0x33);

/// Build a reference closure for a single glyph drawn at cell (0, 0).
fn glyph_reference(
    glyph: &RasterGlyph,
    fg: Rgba,
    bg: Rgba,
) -> impl Fn(u32, u32) -> (Rgba, u8) + '_ {
    move |x, y| {
        let gx = x as i32 - glyph.left;
        let gy = y as i32 - glyph.top;
        let coverage = if gx >= 0 && gy >= 0 {
            glyph.coverage_at(gx as u32, gy as u32)
        } else {
            0
        };
        (blend(bg, fg, coverage), coverage)
    }
}

/// The suite must run on a real adapter and say which one.
///
/// A measurement or a pixel assertion means nothing without knowing what
/// produced it, and "skipped: no GPU" on a machine with an NVIDIA card is a
/// configuration bug rather than a valid outcome. This test fails loudly if no
/// adapter of any kind can be created.
#[test]
fn an_adapter_is_available_and_identified() {
    let gpu = gpu();
    let description = gpu.describe();
    println!("vitrum-grid render tests are using: {description}");
    assert!(
        !description.is_empty(),
        "the adapter must identify itself for any measurement to be interpretable"
    );
    assert!(
        matches!(gpu.class(), AdapterClass::Hardware | AdapterClass::Software),
        "the adapter must be classified"
    );
    let info = gpu.adapter().get_info();
    assert!(!info.name.is_empty(), "the adapter must have a name");
    assert!(
        gpu.device().limits().max_texture_dimension_2d >= 2048,
        "a 2048px glyph atlas must be creatable, got a limit of {}",
        gpu.device().limits().max_texture_dimension_2d
    );
}

/// A blank cell must paint every pixel exactly its background colour.
///
/// This is the floor the whole renderer stands on: the cell quad must cover its
/// rectangle completely, the colour must survive the unorm round trip
/// unchanged, and no glyph may be sampled. If the quad were half a pixel off,
/// or the target were sRGB, the bytes would not match and every other pixel
/// assertion in this file would be built on sand.
#[test]
fn a_blank_cell_paints_exactly_its_background() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let mut g = grid(1, 1, Style::new(FG, BG));

    let image = draw(&mut renderer, &mut g, &target);
    assert_rect_exact(
        &image,
        "blank cell",
        0,
        0,
        image.width(),
        image.height(),
        BG,
    );
    assert_eq!(
        image.count(BG) as u32,
        image.width() * image.height(),
        "every pixel must be the background"
    );
}

/// Reverse video on a blank cell must paint the foreground colour everywhere.
///
/// Reverse is resolved on the CPU before upload, so a bug there is invisible in
/// the shader and shows up only as the wrong colour on screen. A selection
/// highlight or a status bar that renders the wrong way round is exactly this
/// bug.
#[test]
fn reverse_on_a_blank_cell_paints_exactly_the_foreground() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let mut g = grid(1, 1, Style::new(FG, BG).with_attrs(Attrs::REVERSE));

    let image = draw(&mut renderer, &mut g, &target);
    assert_rect_exact(
        &image,
        "reversed blank",
        0,
        0,
        image.width(),
        image.height(),
        FG,
    );
    assert_eq!(image.count(BG), 0, "no pixel may keep the original background");
}

/// The underline rule must fill exactly the rows the font metrics name, in the
/// foreground colour, and leave every other row untouched.
///
/// This assertion is completely independent of which font was discovered: the
/// shader is told the row range and must honour it exactly. An off-by-one puts
/// the rule through the descenders of `g` and `y`, and a thickness bug makes
/// underlines invisible at small sizes.
#[test]
fn underline_fills_exactly_the_metric_rows_in_the_foreground() {
    let mut renderer = renderer();
    let m = renderer.metrics();
    let target = target_for(&renderer, 1, 1);
    let mut g = grid(1, 1, Style::new(FG, BG));
    g.write_char(0, 0, ' ', Style::new(FG, BG).with_attrs(Attrs::UNDERLINE))
        .unwrap();

    let image = draw(&mut renderer, &mut g, &target);
    let top = m.underline_y;
    let bottom = m.underline_y + m.underline_thickness;
    assert!(bottom <= image.height());

    assert_rect_exact(&image, "above the rule", 0, 0, image.width(), top, BG);
    assert_rect_exact(&image, "the rule", 0, top, image.width(), bottom, FG);
    assert_rect_exact(
        &image,
        "below the rule",
        0,
        bottom,
        image.width(),
        image.height(),
        BG,
    );
    assert_eq!(
        image.count(FG),
        (image.width() * m.underline_thickness) as usize,
        "the rule must be exactly {} rows of {} pixels",
        m.underline_thickness,
        image.width()
    );
}

/// A glyph must match, pixel for pixel, the bitmap the rasteriser produced.
///
/// This proves the whole chain at once: the atlas got the right bytes, the
/// entry's offsets are right, the shader's cell-to-glyph coordinate transform
/// is right, and the coverage blend matches. Any single-pixel shift, a
/// transposed atlas write, or a swapped foreground and background shows up here
/// as a concrete coordinate in the failure message.
#[test]
fn a_single_glyph_matches_the_rasterized_bitmap_pixel_for_pixel() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let style = Style::new(FG, BG);
    let mut g = grid(1, 1, style);
    g.write_char(0, 0, 'A', style).unwrap();

    let image = draw(&mut renderer, &mut g, &target);
    let glyph = fonts().rasterize('A', FontStyle::Regular);
    assert!(!glyph.is_blank(), "'A' must have a bitmap to compare against");

    assert_matches_reference(&image, "glyph 'A'", glyph_reference(&glyph, FG, BG));

    // The counts below are exact integers derived from the same bitmap, so a
    // glyph drawn one pixel off fails even when every blend is individually
    // right.
    let mut partial = 0usize;
    let mut untouched = 0usize;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let gx = x as i32 - glyph.left;
            let gy = y as i32 - glyph.top;
            let coverage = if gx >= 0 && gy >= 0 {
                glyph.coverage_at(gx as u32, gy as u32)
            } else {
                0
            };
            match coverage {
                0 => untouched += 1,
                255 => {}
                _ => partial += 1,
            }
        }
    }
    assert!(
        partial > 0,
        "'A' must have antialiased edge pixels for the blend path to be exercised"
    );
    assert!(
        untouched < (image.width() * image.height()) as usize,
        "'A' must cover some of the cell"
    );
    assert_eq!(
        image.count(BG),
        untouched,
        "exactly the uncovered pixels must be pure background"
    );
}

/// A glyph with solid interior pixels must paint them as pure foreground.
///
/// The antialiased test above allows one bit of rounding slack on partially
/// covered pixels, so on its own it could hide a systematic off-by-one in the
/// blend. A fully covered pixel has no slack: it must come back as the exact
/// foreground bytes. U+2588 FULL BLOCK gives a cell full of them, and if the
/// font lacks it the fallback box supplies solid edges instead.
#[test]
fn a_solid_glyph_paints_pure_foreground_on_covered_pixels() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let style = Style::new(FG, BG);
    let mut g = grid(1, 1, style);
    g.write_char(0, 0, '\u{2588}', style).unwrap();

    let image = draw(&mut renderer, &mut g, &target);
    let glyph = fonts().rasterize('\u{2588}', FontStyle::Regular);
    assert_matches_reference(&image, "full block", glyph_reference(&glyph, FG, BG));

    let mut solid = 0usize;
    let mut untouched = 0usize;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let gx = x as i32 - glyph.left;
            let gy = y as i32 - glyph.top;
            let coverage = if gx >= 0 && gy >= 0 {
                glyph.coverage_at(gx as u32, gy as u32)
            } else {
                0
            };
            match coverage {
                0 => untouched += 1,
                255 => solid += 1,
                _ => {}
            }
        }
    }
    assert!(
        solid > 0,
        "U+2588 must produce fully covered pixels, directly or as a fallback box"
    );
    assert_eq!(
        image.count(FG),
        solid,
        "exactly the fully covered pixels must be the pure foreground bytes"
    );
    assert_eq!(
        image.count(BG),
        untouched,
        "exactly the uncovered pixels must be the pure background bytes"
    );
}

/// Each cell must paint its own rectangle and no other.
///
/// The vertex shader turns `(col, row)` into a pixel origin. A swapped pair
/// transposes the screen, a missing multiply stacks every cell at the origin,
/// and a wrong sign flips the grid vertically. Four differently coloured
/// quadrants catch all three, and the quadrant colours are asserted exactly.
#[test]
fn every_cell_paints_its_own_rectangle() {
    let mut renderer = renderer();
    let (cw, ch) = renderer.cell_size();
    let target = target_for(&renderer, 3, 2);
    let mut g = grid(3, 2, Style::DEFAULT);

    let colors = [
        [Rgba::rgb(10, 0, 0), Rgba::rgb(20, 0, 0), Rgba::rgb(30, 0, 0)],
        [Rgba::rgb(0, 10, 0), Rgba::rgb(0, 20, 0), Rgba::rgb(0, 30, 0)],
    ];
    for (row, row_colors) in colors.iter().enumerate() {
        for (col, color) in row_colors.iter().enumerate() {
            g.set_cell(
                col as u16,
                row as u16,
                Cell::blank(Style::new(Rgba::WHITE, *color)),
            )
            .unwrap();
        }
    }

    let image = draw(&mut renderer, &mut g, &target);
    assert_eq!(image.width(), cw * 3);
    assert_eq!(image.height(), ch * 2);
    for (row, row_colors) in colors.iter().enumerate() {
        for (col, color) in row_colors.iter().enumerate() {
            let x0 = cw * col as u32;
            let y0 = ch * row as u32;
            assert_rect_exact(
                &image,
                &format!("cell ({col}, {row})"),
                x0,
                y0,
                x0 + cw,
                y0 + ch,
                *color,
            );
        }
    }
}

/// A wide character's head must paint both of its columns and the tail must
/// paint nothing.
///
/// Instances are drawn in cell order, so a tail that claimed one column would
/// paint its own background over the right half of the glyph the head just
/// drew. That is the exact bug this test locks out, and it does so without
/// depending on any glyph: the head is a blank in one colour, the tail is a
/// blank in another, and the tail's colour must not appear anywhere.
#[test]
fn a_wide_head_paints_both_columns_and_the_tail_paints_nothing() {
    let head_bg = Rgba::rgb(0xc0, 0x20, 0x20);
    let tail_bg = Rgba::rgb(0x20, 0xc0, 0x20);
    let after_bg = Rgba::rgb(0x20, 0x20, 0xc0);

    let mut renderer = renderer();
    let (cw, ch) = renderer.cell_size();
    let target = target_for(&renderer, 3, 1);
    let mut g = grid(3, 1, Style::DEFAULT);

    g.set_cell(0, 0, Cell {
        ch: ' ',
        fg: Rgba::WHITE,
        bg: head_bg,
        attrs: Attrs::NONE,
        slot: CellSlot::WideHead,
    })
    .unwrap();
    g.set_cell(1, 0, Cell {
        ch: '\0',
        fg: Rgba::WHITE,
        bg: tail_bg,
        attrs: Attrs::NONE,
        slot: CellSlot::WideTail,
    })
    .unwrap();
    g.set_cell(2, 0, Cell::blank(Style::new(Rgba::WHITE, after_bg)))
        .unwrap();

    let image = draw(&mut renderer, &mut g, &target);
    assert_rect_exact(&image, "wide pair", 0, 0, cw * 2, ch, head_bg);
    assert_rect_exact(&image, "cell after the pair", cw * 2, 0, cw * 3, ch, after_bg);
    assert_eq!(
        image.count(tail_bg),
        0,
        "the tail must contribute no pixels at all"
    );
    assert_eq!(image.count(head_bg), (cw * 2 * ch) as usize);
    assert_eq!(image.count(after_bg), (cw * ch) as usize);
}

/// A real double-width glyph must render across both of its columns.
///
/// The pair-colour test above proves the geometry with blanks; this proves the
/// glyph itself is not clipped at the one-column boundary. The reference comes
/// from the same rasteriser, so a CJK face and the fallback box are both
/// covered.
#[test]
fn a_wide_glyph_renders_across_both_columns() {
    let mut renderer = renderer();
    let (cw, _) = renderer.cell_size();
    let target = target_for(&renderer, 2, 1);
    let style = Style::new(FG, BG);
    let mut g = grid(2, 1, style);
    g.write_char(0, 0, '漢', style).unwrap();

    let image = draw(&mut renderer, &mut g, &target);
    let glyph = fonts().rasterize('漢', FontStyle::Regular);
    assert!(glyph.width > cw, "the test needs a bitmap wider than one cell");

    assert_matches_reference(&image, "wide glyph", glyph_reference(&glyph, FG, BG));

    let ink_past_the_first_column = (cw..image.width())
        .flat_map(|x| (0..image.height()).map(move |y| (x, y)))
        .filter(|(x, y)| image.pixel(*x, *y) != BG)
        .count();
    assert!(
        ink_past_the_first_column > 0,
        "a wide glyph must put ink in its second column; found none past x={cw}"
    );
}

/// Bold text must reach the screen as different pixels from regular text.
///
/// The face selection, the atlas key, and the upload all have to agree. A break
/// anywhere in that chain renders bold as regular, and because the layout is
/// identical nobody notices until they compare two screenshots.
#[test]
fn bold_and_regular_reach_the_screen_as_different_pixels() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let style = Style::new(FG, BG);

    let mut plain = grid(1, 1, style);
    plain.write_char(0, 0, 'E', style).unwrap();
    let plain_image = draw(&mut renderer, &mut plain, &target);

    let mut bold = grid(1, 1, style);
    bold.write_char(0, 0, 'E', style.with_attrs(Attrs::BOLD))
        .unwrap();
    let bold_image = draw(&mut renderer, &mut bold, &target);

    assert_ne!(
        plain_image.as_bytes(),
        bold_image.as_bytes(),
        "bold 'E' rendered identically to regular 'E'"
    );

    let mut stack = fonts();
    assert_matches_reference(
        &plain_image,
        "regular 'E'",
        glyph_reference(&stack.rasterize('E', FontStyle::Regular), FG, BG),
    );
    assert_matches_reference(
        &bold_image,
        "bold 'E'",
        glyph_reference(&stack.rasterize('E', FontStyle::Bold), FG, BG),
    );

    assert!(
        bold_image.count(BG) < plain_image.count(BG),
        "bold must cover more of the cell: {} background pixels vs {}",
        bold_image.count(BG),
        plain_image.count(BG)
    );
}

/// All four attribute combinations must reach the screen as distinct pixels.
///
/// Bold-italic is the combination that gets dropped by a copy-paste error in
/// the slot table. Rendering it as plain bold makes emphasised text in a diff
/// unreadable, and no single-attribute test would catch it.
#[test]
fn all_four_face_attributes_round_trip_to_distinct_pixels() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let style = Style::new(FG, BG);
    let mut stack = fonts();

    let mut images = Vec::new();
    for (attrs, font_style) in [
        (Attrs::NONE, FontStyle::Regular),
        (Attrs::BOLD, FontStyle::Bold),
        (Attrs::ITALIC, FontStyle::Italic),
        (Attrs::BOLD | Attrs::ITALIC, FontStyle::BoldItalic),
    ] {
        let mut g = grid(1, 1, style);
        g.write_char(0, 0, 'R', style.with_attrs(attrs)).unwrap();
        let image = draw(&mut renderer, &mut g, &target);
        assert_matches_reference(
            &image,
            &format!("'R' with {attrs:?}"),
            glyph_reference(&stack.rasterize('R', font_style), FG, BG),
        );
        images.push((attrs, image));
    }

    for (i, (a, ia)) in images.iter().enumerate() {
        for (b, ib) in images.iter().skip(i + 1) {
            assert_ne!(
                ia.as_bytes(),
                ib.as_bytes(),
                "{a:?} and {b:?} rendered identically"
            );
        }
    }
}

/// Underline must combine with a glyph rather than replace it.
///
/// The shader forces coverage to 1 on the rule rows. If it wrote the rule
/// before sampling the glyph, or returned early, underlined text would lose its
/// letters and show only rules.
#[test]
fn underline_combines_with_the_glyph_underneath() {
    let mut renderer = renderer();
    let m = renderer.metrics();
    let target = target_for(&renderer, 1, 1);
    let style = Style::new(FG, BG);

    let mut g = grid(1, 1, style);
    g.write_char(0, 0, 'H', style.with_attrs(Attrs::UNDERLINE))
        .unwrap();
    let image = draw(&mut renderer, &mut g, &target);

    let glyph = fonts().rasterize('H', FontStyle::Regular);
    let base = glyph_reference(&glyph, FG, BG);
    assert_matches_reference(&image, "underlined 'H'", |x, y| {
        if y >= m.underline_y && y < m.underline_y + m.underline_thickness {
            (FG, 255)
        } else {
            base(x, y)
        }
    });

    // The letter must still be there above the rule.
    let letter_pixels = (0..m.underline_y)
        .flat_map(|y| (0..image.width()).map(move |x| (x, y)))
        .filter(|(x, y)| image.pixel(*x, *y) != BG)
        .count();
    assert!(
        letter_pixels > 0,
        "the glyph must survive underlining; found no ink above the rule"
    );
}

/// A translucent background must reach the target with its alpha intact.
///
/// A terminal over a compositor blur needs the alpha channel written through,
/// not premultiplied away or forced opaque. The pipeline runs with blending
/// disabled precisely so the cell's own alpha is what lands in the texture.
#[test]
fn cell_alpha_is_written_through_to_the_target() {
    let translucent = Rgba::rgba(0x40, 0x80, 0xc0, 0x55);
    let mut renderer = renderer();
    let target = target_for(&renderer, 1, 1);
    let mut g = grid(1, 1, Style::new(Rgba::WHITE, translucent));

    let image = draw(&mut renderer, &mut g, &target);
    assert_rect_exact(
        &image,
        "translucent background",
        0,
        0,
        image.width(),
        image.height(),
        translucent,
    );
    assert_eq!(image.pixel(0, 0).a, 0x55, "alpha must survive verbatim");
}

/// A zero-sized viewport must be refused, not drawn.
///
/// A window dragged to zero width delivers this every time. Passing it through
/// produces a wgpu validation error about a zero-extent render pass, which
/// names nothing useful; refusing it here names the viewport.
#[test]
fn a_zero_viewport_is_refused_with_its_dimensions() {
    let mut renderer = renderer();
    let target = target_for(&renderer, 2, 1);
    let mut g = grid(2, 1, Style::DEFAULT);

    for viewport in [(0, 10), (10, 0), (0, 0)] {
        let err = renderer
            .render(gpu().device(), gpu().queue(), &mut g, target.view(), viewport)
            .expect_err("a zero viewport must be refused");
        match err {
            RenderError::ZeroViewport { width, height } => {
                assert_eq!((width, height), viewport);
            }
            other => panic!("expected ZeroViewport for {viewport:?}, got {other}"),
        }
    }
    assert!(g.is_dirty(), "a refused render must not consume the damage");
}

/// Grid geometry helpers must agree with the pixels that actually get drawn.
///
/// A client sizes its grid from `grid_size_for` and its texture from
/// `pixel_size_for`. If those disagreed with the shader's `column * width`,
/// the last row or column would be clipped or a strip of clear colour would
/// show along an edge.
#[test]
fn grid_and_pixel_size_helpers_agree_with_the_rendered_output() {
    let renderer = renderer();
    let (cw, ch) = renderer.cell_size();
    assert!(cw >= 1 && ch >= 1);

    assert_eq!(renderer.pixel_size_for(10, 4), (cw * 10, ch * 4));
    assert_eq!(renderer.grid_size_for(cw * 10, ch * 4), (10, 4));
    assert_eq!(
        renderer.grid_size_for(cw * 10 + cw - 1, ch * 4 + ch - 1),
        (10, 4),
        "a partial cell must not be counted"
    );
    assert_eq!(
        renderer.grid_size_for(0, 0),
        (1, 1),
        "a degenerate viewport must still yield a constructible grid"
    );
    assert_eq!(renderer.grid_size_for(1, 1), (1, 1));
    assert_eq!(
        renderer.format(),
        HeadlessTarget::FORMAT,
        "the pipeline's colour target must match the texture it draws into, or wgpu \
         rejects the render pass"
    );
}

/// The software rasteriser must produce the same pixels as the GPU.
///
/// Two things ride on this. First, correctness: if the two disagree, one of
/// them is wrong and the shader has undefined behaviour in it (an out-of-bounds
/// `textureLoad`, an uninitialised varying). Second, the honesty rule: a
/// machine with no GPU must still run these tests on a CPU adapter rather than
/// skip them, and that path has to be proven to work.
///
/// macOS is the one platform where the comparison cannot be made at all.
/// `force_fallback_adapter` has nothing to select: Metal exposes no CPU
/// adapter, and Lavapipe is Mesa's, which macOS does not ship. That is a fact
/// about the platform rather than about the machine, so it is asserted there
/// instead of being demanded, and the real adapter still has to render.
#[test]
fn the_software_adapter_produces_identical_pixels() {
    let config = RendererConfig {
        format: HeadlessTarget::FORMAT,
        atlas_dim: 1024,
        font: FontConfig {
            families: Vec::new(),
            size_px: TEST_PX,
            max_fallback_faces: 24,
        },
    };

    let style = Style::new(FG, BG);
    let build = |ctx: &GpuContext| {
        let mut renderer = GridRenderer::with_fonts(ctx.device(), &config, fonts());
        let (w, h) = renderer.pixel_size_for(6, 2);
        let target = HeadlessTarget::new(ctx.device(), w, h);
        let mut g = CellGrid::new(6, 2, style).unwrap();
        g.write_str(0, 0, "Ag_|", style).unwrap();
        g.write_char(4, 0, ' ', style.with_attrs(Attrs::UNDERLINE))
            .unwrap();
        g.write_str(0, 1, "漢", style).unwrap();
        g.write_char(2, 1, 'B', style.with_attrs(Attrs::BOLD))
            .unwrap();
        g.write_char(3, 1, 'I', style.with_attrs(Attrs::ITALIC))
            .unwrap();
        renderer
            .render(
                ctx.device(),
                ctx.queue(),
                &mut g,
                target.view(),
                (target.width(), target.height()),
            )
            .expect("render must succeed on both adapters");
        target.read(ctx.device(), ctx.queue())
    };

    let hardware_image = build(gpu());

    let software = match GpuContext::headless_software() {
        Ok(ctx) => ctx,
        #[cfg(target_os = "macos")]
        Err(_) => {
            assert!(
                hardware_image.width() > 0 && hardware_image.height() > 0,
                "with no fallback adapter to compare against, the real one must still render"
            );
            return;
        }
        #[cfg(not(target_os = "macos"))]
        Err(err) => panic!(
            "no software adapter could be created: {err}\n\
             Install Mesa's Lavapipe (mesa-vulkan-drivers) so this crate can be verified on \
             machines without a GPU."
        ),
    };
    println!("software comparison adapter: {}", software.describe());
    assert_eq!(
        software.class(),
        AdapterClass::Software,
        "force_fallback_adapter must select a CPU rasteriser, got {}",
        software.describe()
    );

    let software_image = build(&software);

    assert_eq!(hardware_image.width(), software_image.width());
    assert_eq!(hardware_image.height(), software_image.height());

    let mut differing = 0usize;
    let mut worst = 0i32;
    let mut first = None;
    for y in 0..hardware_image.height() {
        for x in 0..hardware_image.width() {
            let a = hardware_image.pixel(x, y);
            let b = software_image.pixel(x, y);
            let delta = [
                (i32::from(a.r) - i32::from(b.r)).abs(),
                (i32::from(a.g) - i32::from(b.g)).abs(),
                (i32::from(a.b) - i32::from(b.b)).abs(),
                (i32::from(a.a) - i32::from(b.a)).abs(),
            ]
            .into_iter()
            .max()
            .unwrap_or(0);
            if delta > 0 {
                differing += 1;
                worst = worst.max(delta);
                if first.is_none() {
                    first = Some((x, y, a, b));
                }
            }
        }
    }
    assert!(
        worst <= 1,
        "hardware and software renders differ by up to {worst} in {differing} pixels; \
         first at {first:?}"
    );
}
