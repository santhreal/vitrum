//! Shared fixtures for the test suites.
//!
//! One device and one font database are built for the whole test binary. Both
//! are expensive (adapter enumeration, a full scan of the system font
//! directories) and both are read-only once built, so sharing them keeps the
//! suite fast without making any test depend on another.

use std::sync::LazyLock;

use crate::{
    CellGrid, FontConfig, FontStack, GpuContext, GridRenderer, HeadlessTarget, Image, Rgba, Style,
};

/// Font size every test renders at. Small enough that a 200x50 grid fits a
/// default atlas, large enough that glyph bitmaps have interior pixels worth
/// asserting on.
pub const TEST_PX: f32 = 16.0;

/// Largest per-channel difference tolerated between a GPU pixel and the CPU
/// reference for a *partially* covered pixel.
///
/// The shader and the reference perform the same `bg + (fg - bg) * coverage` in
/// f32. The only legal divergence is the final rounding to 8 bits, worth at
/// most one least-significant bit. Fully covered and fully uncovered pixels are
/// held to exact equality, because those paths involve no blend at all.
pub const BLEND_TOLERANCE: i32 = 1;

static GPU: LazyLock<GpuContext> = LazyLock::new(|| {
    GpuContext::headless().unwrap_or_else(|err| {
        panic!(
            "no wgpu adapter of any kind could be created, so no render test can run: {err}\n\
             This machine is expected to expose an NVIDIA GPU through Vulkan. If that is broken, \
             install a CPU rasteriser (Mesa's Lavapipe provides one) and the suite will use it."
        )
    })
});

static FONT_DB: LazyLock<fontdb::Database> = LazyLock::new(|| {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    assert!(
        !db.is_empty(),
        "the system font database is empty, so no monospace face can be discovered; \
         install at least one monospace font (DejaVu Sans Mono or Liberation Mono)"
    );
    db
});

/// The shared device, adapter, and queue.
pub fn gpu() -> &'static GpuContext {
    &GPU
}

/// A clone of the shared system font database.
pub fn system_db() -> fontdb::Database {
    FONT_DB.clone()
}

/// The configuration every fixture uses: the shipping defaults with only the
/// size overridden, so the tests exercise the same discovery path an
/// application takes.
pub fn config_at(px: f32) -> FontConfig {
    FontConfig {
        size_px: px,
        ..FontConfig::default()
    }
}

/// A font stack at `px`, built from the shared database so no test pays for a
/// second scan of the font directories.
pub fn fonts_at(px: f32) -> FontStack {
    FontStack::from_database(FONT_DB.clone(), &config_at(px))
        .expect("the shared font database must yield a usable monospace face")
}

/// A font stack at [`TEST_PX`].
pub fn fonts() -> FontStack {
    fonts_at(TEST_PX)
}

/// The raw file bytes of the face discovery picked.
///
/// Used to exercise the caller-supplied-face path against a font that is known
/// to parse, without vendoring a font file into the repository.
pub fn primary_face_bytes() -> Vec<u8> {
    let family = fonts().family().to_owned();
    let id = FONT_DB
        .query(&fontdb::Query {
            families: &[fontdb::Family::Name(&family)],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        })
        .expect("the discovered family must resolve back to a face id");
    FONT_DB
        .with_face_data(id, |data, _| data.to_vec())
        .expect("the discovered face must still be readable")
}

/// A renderer targeting [`HeadlessTarget::FORMAT`] with a default-size atlas.
pub fn renderer() -> GridRenderer {
    renderer_with(TEST_PX, crate::DEFAULT_ATLAS_DIM)
}

/// A renderer with an explicit font size and atlas dimension.
pub fn renderer_with(px: f32, atlas_dim: u32) -> GridRenderer {
    GridRenderer::with_fonts(
        gpu().device(),
        &crate::RendererConfig {
            format: HeadlessTarget::FORMAT,
            atlas_dim,
            font: config_at(px),
        },
        fonts_at(px),
    )
}

/// A target sized to exactly `cols` x `rows` cells of `renderer`.
pub fn target_for(renderer: &GridRenderer, cols: u16, rows: u16) -> HeadlessTarget {
    let (w, h) = renderer.pixel_size_for(cols, rows);
    HeadlessTarget::new(gpu().device(), w, h)
}

/// Render `grid` into `target` and read the result back.
pub fn draw(renderer: &mut GridRenderer, grid: &mut CellGrid, target: &HeadlessTarget) -> Image {
    renderer
        .render(
            gpu().device(),
            gpu().queue(),
            grid,
            target.view(),
            (target.width(), target.height()),
        )
        .expect("render must succeed");
    target.read(gpu().device(), gpu().queue())
}

/// A grid of blanks in `style`.
pub fn grid(cols: u16, rows: u16, style: Style) -> CellGrid {
    CellGrid::new(cols, rows, style).expect("test grid dimensions must be valid")
}

/// The colour the shader produces for `coverage` of `fg` over `bg`.
///
/// Mirrors the fragment stage exactly: unorm decode, linear interpolation in
/// f32, unorm encode.
pub fn blend(bg: Rgba, fg: Rgba, coverage: u8) -> Rgba {
    let t = f32::from(coverage) / 255.0;
    let ch = |b: u8, f: u8| {
        let b = f32::from(b) / 255.0;
        let f = f32::from(f) / 255.0;
        ((b + (f - b) * t) * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Rgba {
        r: ch(bg.r, fg.r),
        g: ch(bg.g, fg.g),
        b: ch(bg.b, fg.b),
        a: ch(bg.a, fg.a),
    }
}

/// Assert every pixel of `image` matches the reference.
///
/// `reference` returns the expected colour and the coverage that produced it.
/// Coverage 0 and 255 are held to exact equality; anything between is allowed
/// [`BLEND_TOLERANCE`] of rounding slack. The panic message names the first
/// offending pixel with both colours, so a failure identifies the bug rather
/// than just reporting that one exists.
pub fn assert_matches_reference(
    image: &Image,
    label: &str,
    reference: impl Fn(u32, u32) -> (Rgba, u8),
) {
    let mut mismatches = 0usize;
    let mut first: Option<(u32, u32, Rgba, Rgba, u8)> = None;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let (want, coverage) = reference(x, y);
            let got = image.pixel(x, y);
            let slack = if coverage == 0 || coverage == 255 {
                0
            } else {
                BLEND_TOLERANCE
            };
            let off = [
                (i32::from(got.r) - i32::from(want.r)).abs(),
                (i32::from(got.g) - i32::from(want.g)).abs(),
                (i32::from(got.b) - i32::from(want.b)).abs(),
                (i32::from(got.a) - i32::from(want.a)).abs(),
            ]
            .into_iter()
            .max()
            .unwrap_or(0);
            if off > slack {
                mismatches += 1;
                if first.is_none() {
                    first = Some((x, y, want, got, coverage));
                }
            }
        }
    }
    if let Some((x, y, want, got, coverage)) = first {
        panic!(
            "{label}: {mismatches} of {} pixels differ from the CPU reference.\n\
             first at ({x}, {y}): want {want:?}, got {got:?}, glyph coverage {coverage}\n\
             image palette: {:?}",
            image.width() * image.height(),
            image.palette()
        );
    }
}

/// Assert every pixel in the rectangle equals `want` exactly.
pub fn assert_rect_exact(
    image: &Image,
    label: &str,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    want: Rgba,
) {
    for y in y0..y1 {
        for x in x0..x1 {
            let got = image.pixel(x, y);
            assert_eq!(
                got, want,
                "{label}: pixel ({x}, {y}) in rect ({x0},{y0})..({x1},{y1}) is {got:?}, want {want:?}"
            );
        }
    }
}
