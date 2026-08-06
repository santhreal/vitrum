//! Glyph atlas placement, caching, reset, and exhaustion.

use crate::atlas::{AtlasEntry, AtlasError, GlyphAtlas, GlyphKey};
use crate::font::{FontStyle, RasterGlyph};
use crate::tests::support::{fonts, gpu};

fn solid(width: u32, height: u32) -> RasterGlyph {
    RasterGlyph {
        width,
        height,
        left: 0,
        top: 0,
        coverage: vec![255; (width * height) as usize],
    }
}

fn key(ch: char) -> GlyphKey {
    GlyphKey {
        ch,
        style: FontStyle::Regular,
    }
}

/// The atlas must clamp its dimension into the device's supported range.
///
/// A caller asking for a 32768-pixel atlas would otherwise get a texture
/// creation panic deep inside wgpu on machines whose limit is 8192, and the
/// message would name neither the atlas nor the caller's number.
#[test]
fn atlas_dimension_is_clamped_to_the_device_limit() {
    let device = gpu().device();
    let limit = device.limits().max_texture_dimension_2d;

    let huge = GlyphAtlas::new(device, u32::MAX);
    assert_eq!(huge.dim(), limit, "an oversized request must clamp down");

    let tiny = GlyphAtlas::new(device, 1);
    assert_eq!(tiny.dim(), 256, "an undersized request must clamp up");

    let exact = GlyphAtlas::new(device, 512);
    assert_eq!(exact.dim(), 512);
}

/// A blank glyph must occupy no atlas space and return the blank entry.
///
/// Most cells on a terminal screen are spaces. Allocating a rectangle for each
/// one would fill even a 2048px atlas within a couple of screens and force a
/// reset, which costs a full-grid re-upload.
#[test]
fn blank_glyphs_consume_no_atlas_space() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 512);
    let entry = atlas
        .insert_glyph(gpu().queue(), key(' '), &RasterGlyph::blank())
        .expect("a blank glyph always fits");

    assert_eq!(entry, AtlasEntry::BLANK);
    assert!(entry.is_blank());
    assert_eq!(atlas.get(key(' ')), Some(AtlasEntry::BLANK));
    assert_eq!(atlas.resident(), 1, "the lookup is still cached");

    // Placing a real glyph afterwards must start at the origin, proving the
    // blank reserved nothing.
    let real = atlas
        .insert_glyph(gpu().queue(), key('A'), &solid(4, 4))
        .unwrap();
    assert_eq!((real.x, real.y), (1, 1), "one pixel of padding at the origin");
}

/// Shelf placement must be left to right, then top to bottom, with one pixel of
/// padding around every glyph.
///
/// The shader addresses the atlas with `textureLoad`, so a coordinate that is
/// off by one samples a neighbouring glyph's ink and text renders with fringes
/// of the wrong character. Pinning the exact coordinates makes any packer
/// change visible immediately.
#[test]
fn shelf_packing_places_glyphs_at_exact_coordinates() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 256);
    let queue = gpu().queue();

    // 10x20 needs a 12x22 box. Shelf 0 is 22 tall.
    let a = atlas.insert_glyph(queue, key('a'), &solid(10, 20)).unwrap();
    assert_eq!((a.x, a.y, a.w, a.h), (1, 1, 10, 20));

    let b = atlas.insert_glyph(queue, key('b'), &solid(10, 20)).unwrap();
    assert_eq!((b.x, b.y), (13, 1), "next glyph starts past the first box");

    // A shorter glyph on the same shelf keeps the shelf height.
    let c = atlas.insert_glyph(queue, key('c'), &solid(6, 8)).unwrap();
    assert_eq!((c.x, c.y, c.w, c.h), (25, 1, 6, 8));

    // The cursor now sits at x=32. Boxes are 12 wide and the atlas is 256, so
    // 18 more fit before the shelf wraps (32 + 18*12 = 248, and 248 + 12 > 256).
    for (i, ch) in ('d'..='u').enumerate() {
        let e = atlas.insert_glyph(queue, key(ch), &solid(10, 20)).unwrap();
        assert_eq!(e.y, 1, "{ch:?} must stay on the first shelf");
        assert_eq!(
            e.x,
            33 + 12 * i as u16,
            "{ch:?} must sit one box past its predecessor"
        );
    }
    let wrapped = atlas.insert_glyph(queue, key('v'), &solid(10, 20)).unwrap();
    assert_eq!(
        wrapped.y, 23,
        "a new shelf must open directly under a 22px-tall one"
    );
    assert_eq!(wrapped.x, 1, "a new shelf restarts at the left edge");
}

/// Placement must record the glyph's offsets inside its cell verbatim.
///
/// `left` and `top` are what the fragment shader subtracts to find the glyph
/// pixel. Losing the sign on a negative `left` would shift every overhanging
/// glyph a few pixels right and clip its left edge.
#[test]
fn atlas_entries_preserve_glyph_offsets_including_negatives() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 256);
    let glyph = RasterGlyph {
        width: 5,
        height: 7,
        left: -3,
        top: 11,
        coverage: vec![128; 35],
    };
    let entry = atlas
        .insert_glyph(gpu().queue(), key('j'), &glyph)
        .unwrap();
    assert_eq!(entry.left, -3);
    assert_eq!(entry.top, 11);
    assert_eq!(entry.w, 5);
    assert_eq!(entry.h, 7);
    assert!(!entry.is_blank());
}

/// A repeated lookup must return the cached entry and rasterise nothing new.
///
/// The atlas is the reason a steady-state frame does no CPU work. If it missed
/// on every lookup, every frame would re-rasterise every visible glyph, which
/// is the per-frame reparse cost this renderer exists to avoid.
#[test]
fn a_repeated_lookup_reuses_the_cached_entry() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 512);
    let mut stack = fonts();
    let queue = gpu().queue();

    let first = atlas.get_or_insert(queue, &mut stack, key('Z')).unwrap();
    assert_eq!(atlas.resident(), 1);

    for _ in 0..50 {
        let again = atlas.get_or_insert(queue, &mut stack, key('Z')).unwrap();
        assert_eq!(again, first, "the cached entry must not move");
    }
    assert_eq!(atlas.resident(), 1, "no second slot may be allocated");
    assert_eq!(atlas.generation(), 0, "no reset may have happened");
}
#[test]
fn direct_ascii_atlas_array_fast_path_works() {
    let gpu_ctx = gpu();
    let mut atlas = GlyphAtlas::new(gpu_ctx.device(), 512);

    let k_ascii = GlyphKey { ch: 'A', style: FontStyle::Regular };
    assert_eq!(atlas.get(k_ascii), None);

    let entry = atlas.insert_glyph(gpu_ctx.queue(), k_ascii, &solid(10, 10)).unwrap();
    assert_eq!(atlas.get(k_ascii), Some(entry));

    let k_non_ascii = GlyphKey { ch: '€', style: FontStyle::Regular };
    assert_eq!(atlas.get(k_non_ascii), None);
    let entry_non = atlas.insert_glyph(gpu_ctx.queue(), k_non_ascii, &solid(10, 10)).unwrap();
    assert_eq!(atlas.get(k_non_ascii), Some(entry_non));
}

/// The same character in different styles must occupy different slots.
///
/// The key is the pair, not the character. Keying on the character alone would
/// make the first-seen style win, so a document whose first 'a' was bold would
/// render every 'a' bold thereafter.
#[test]
fn the_same_character_in_different_styles_gets_different_slots() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 512);
    let mut stack = fonts();
    let queue = gpu().queue();

    let mut seen = Vec::new();
    for style in FontStyle::ALL {
        let entry = atlas
            .get_or_insert(queue, &mut stack, GlyphKey { ch: 'a', style })
            .unwrap();
        seen.push((style, entry));
    }
    assert_eq!(atlas.resident(), 4);
    for (i, (sa, ea)) in seen.iter().enumerate() {
        for (sb, eb) in seen.iter().skip(i + 1) {
            assert_ne!(
                (ea.x, ea.y),
                (eb.x, eb.y),
                "{sa:?} and {sb:?} share atlas coordinates"
            );
        }
    }
}

/// A glyph larger than the whole atlas must be reported with exact numbers.
///
/// Retrying forever or resetting in a loop would hang the render thread. The
/// error names the padded size and the atlas dimension so the fix (smaller
/// font or bigger atlas) is obvious from the message alone.
#[test]
fn a_glyph_larger_than_the_atlas_is_reported_not_retried() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 256);
    let err = atlas
        .insert_glyph(gpu().queue(), key('X'), &solid(300, 10))
        .expect_err("a 300px glyph cannot fit a 256px atlas");
    assert_eq!(
        err,
        AtlasError::GlyphTooLarge {
            width: 302,
            height: 12,
            dim: 256
        }
    );
    assert_eq!(atlas.generation(), 0, "an impossible glyph must not reset");
    assert_eq!(atlas.resident(), 0);

    let tall = atlas
        .insert_glyph(gpu().queue(), key('Y'), &solid(10, 300))
        .expect_err("a 300px-tall glyph cannot fit either");
    assert_eq!(
        tall,
        AtlasError::GlyphTooLarge {
            width: 12,
            height: 302,
            dim: 256
        }
    );
}

/// Filling the atlas must reset it and bump the generation exactly once.
///
/// The generation is the renderer's signal that every cached coordinate is now
/// meaningless. A reset that did not bump it would leave the whole screen
/// sampling the wrong rectangles until something else forced a rebuild.
#[test]
fn overflowing_the_atlas_resets_it_and_bumps_the_generation() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 256);
    let queue = gpu().queue();
    atlas.begin_frame();

    // 32x32 boxes: 8 per shelf (256/34 = 7), 7 shelves of 34 = 238, so the
    // atlas holds 7*7 = 49 boxes before it runs out of vertical room.
    let mut generation_bumps = 0;
    let mut placed = 0;
    for i in 0..200u32 {
        // A fresh frame each iteration, so one reset per frame is allowed.
        atlas.begin_frame();
        let before = atlas.generation();
        let ch = char::from_u32(0x4000 + i).unwrap();
        atlas
            .insert_glyph(queue, key(ch), &solid(32, 32))
            .expect("one reset per frame keeps every insert satisfiable");
        if atlas.generation() != before {
            generation_bumps += 1;
            assert_eq!(
                atlas.generation(),
                before + 1,
                "a reset must bump the generation by exactly one"
            );
            assert_eq!(
                atlas.resident(),
                1,
                "after a reset only the glyph that triggered it is resident"
            );
        }
        placed += 1;
    }
    assert_eq!(placed, 200);
    assert!(
        generation_bumps >= 3,
        "200 distinct 32x32 glyphs must overflow a 256px atlas repeatedly, saw {generation_bumps} resets"
    );
    assert_eq!(atlas.generation(), generation_bumps);
}

/// A second reset inside one frame must be reported as exhaustion.
///
/// Without this the renderer would loop: reset, refill, reset again, forever,
/// with the UI thread pinned. The error names the atlas size and how many
/// glyphs were resident so the operator can size the atlas correctly.
#[test]
fn a_second_reset_in_one_frame_reports_exhaustion() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 256);
    let queue = gpu().queue();
    atlas.begin_frame();

    let mut err = None;
    for i in 0..4000u32 {
        let ch = char::from_u32(0x4000 + i).unwrap();
        if let Err(e) = atlas.insert_glyph(queue, key(ch), &solid(32, 32)) {
            err = Some(e);
            break;
        }
    }
    match err.expect("one frame cannot hold 4000 distinct 32x32 glyphs in a 256px atlas") {
        AtlasError::Exhausted { dim, resident } => {
            assert_eq!(dim, 256);
            assert!(
                resident > 0,
                "exhaustion must report how many glyphs were resident"
            );
        }
        other => panic!("expected Exhausted, got {other:?}"),
    }
    assert_eq!(
        atlas.generation(),
        1,
        "exactly one reset is allowed per frame"
    );
}

/// `begin_frame` must re-arm the reset budget.
///
/// Without it the atlas would permit one reset for the lifetime of the process
/// and every frame after a busy one would fail, turning a transient glyph spike
/// into a permanently broken terminal.
#[test]
fn begin_frame_rearms_the_reset_budget() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 256);
    let queue = gpu().queue();

    let fill_until_reset = |atlas: &mut GlyphAtlas, base: u32| {
        let start = atlas.generation();
        for i in 0..4000u32 {
            let ch = char::from_u32(base + i).unwrap();
            atlas
                .insert_glyph(queue, key(ch), &solid(32, 32))
                .expect("the first reset of a frame must succeed");
            if atlas.generation() != start {
                return;
            }
        }
        panic!("the atlas never overflowed");
    };

    atlas.begin_frame();
    fill_until_reset(&mut atlas, 0x4000);
    assert_eq!(atlas.generation(), 1);

    atlas.begin_frame();
    fill_until_reset(&mut atlas, 0x8000);
    assert_eq!(
        atlas.generation(),
        2,
        "a new frame must be allowed its own reset"
    );
}

/// A reset must forget every entry so stale coordinates cannot be handed out.
///
/// The texture keeps its old pixels on purpose, so a stale entry would return a
/// perfectly valid-looking rectangle pointing at a different character's ink.
#[test]
fn a_reset_forgets_every_previous_entry() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 256);
    let queue = gpu().queue();
    atlas.begin_frame();

    atlas.insert_glyph(queue, key('a'), &solid(32, 32)).unwrap();
    assert!(atlas.get(key('a')).is_some());

    for i in 0..4000u32 {
        let ch = char::from_u32(0x4000 + i).unwrap();
        if atlas.insert_glyph(queue, key(ch), &solid(32, 32)).is_err() {
            break;
        }
        if atlas.generation() == 1 {
            break;
        }
    }
    assert_eq!(atlas.generation(), 1);
    assert_eq!(
        atlas.get(key('a')),
        None,
        "the pre-reset entry must be gone, not stale"
    );
}

/// A glyph exactly the size of the atlas minus its padding must fit.
///
/// This is the boundary between "fits" and `GlyphTooLarge`. An off-by-one in
/// the padding arithmetic would reject the largest legal glyph, which at big
/// font sizes means the terminal refuses to draw at all.
#[test]
fn a_glyph_exactly_filling_the_atlas_fits() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 256);
    let entry = atlas
        .insert_glyph(gpu().queue(), key('#'), &solid(254, 254))
        .expect("254 + 2 padding is exactly 256");
    assert_eq!((entry.x, entry.y, entry.w, entry.h), (1, 1, 254, 254));

    let mut other = GlyphAtlas::new(gpu().device(), 256);
    let err = other
        .insert_glyph(gpu().queue(), key('#'), &solid(255, 254))
        .expect_err("255 + 2 padding is one past the edge");
    assert_eq!(
        err,
        AtlasError::GlyphTooLarge {
            width: 257,
            height: 256,
            dim: 256
        }
    );
}

/// Real glyphs from the font stack must land inside the atlas bounds.
///
/// A coordinate past the texture edge is undefined behaviour on the GPU: some
/// drivers clamp, some wrap, some return garbage. The bug would be invisible on
/// the development machine and obvious on someone else's.
#[test]
fn every_rasterized_glyph_lands_inside_the_atlas() {
    let mut atlas = GlyphAtlas::new(gpu().device(), 1024);
    let mut stack = fonts();
    let queue = gpu().queue();
    let dim = atlas.dim();

    for ch in ('!'..='~').chain(['漢', 'あ', '\u{f8ff}', '\u{2588}']) {
        for style in FontStyle::ALL {
            let entry = atlas
                .get_or_insert(queue, &mut stack, GlyphKey { ch, style })
                .expect("a 1024px atlas holds one page of glyphs");
            assert!(
                u32::from(entry.x) + u32::from(entry.w) <= dim,
                "{ch:?}/{style:?} spans x {}..{} past a {dim}px atlas",
                entry.x,
                u32::from(entry.x) + u32::from(entry.w)
            );
            assert!(
                u32::from(entry.y) + u32::from(entry.h) <= dim,
                "{ch:?}/{style:?} spans y {}..{} past a {dim}px atlas",
                entry.y,
                u32::from(entry.y) + u32::from(entry.h)
            );
        }
    }
    assert_eq!(atlas.generation(), 0, "one page of glyphs must not overflow");
}
