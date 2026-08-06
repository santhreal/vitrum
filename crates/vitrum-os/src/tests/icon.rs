//! Icon rasterisation, asserted pixel by pixel.
//!
//! The tray icon and the Windows taskbar overlay are the same picture in two
//! byte orders. Rasterising it here rather than through GDI or Core Graphics is
//! what makes "the badge shows a 3" a testable claim on a Linux machine.

use crate::icon::{
    GLYPH_HEIGHT, GLYPH_WIDTH, Rgba, count_text, glyph, render_count_icon, render_idle_icon,
    render_tray_icon,
};

/// Zero must produce no icon at all.
///
/// Every platform's "no badge" state is the absence of an image. Returning a
/// blank one leaves an empty pill on the dock and an invisible-but-present
/// overlay on the taskbar.
#[test]
fn a_zero_count_produces_no_badge() {
    assert!(render_count_icon(16, 0).is_none());
    assert_eq!(count_text(0), "");
}

/// One through nine must render as themselves.
#[test]
fn single_digits_render_as_themselves() {
    for n in 1..=9 {
        assert_eq!(count_text(n), n.to_string());
    }
}

/// Ten and above must become `9+`.
///
/// Three 5-pixel glyphs plus gaps is 17 pixels, wider than a 16-pixel overlay.
/// Rendering `12` would clip the `2` and show `1` plus a smear, which reads as
/// "1 session" and is worse than an honest `9+`.
#[test]
fn double_digits_become_nine_plus() {
    assert_eq!(count_text(10), "9+");
    assert_eq!(count_text(99), "9+");
    assert_eq!(count_text(u32::MAX), "9+");
}

/// The badge must be a disc: opaque in the middle, transparent at the corners.
///
/// A square badge with no alpha looks like a rendering bug on a rounded
/// taskbar button, and the corner test is what proves the alpha channel is
/// actually being written rather than left at the default.
#[test]
fn the_badge_is_a_disc_with_transparent_corners() {
    let img = render_count_icon(16, 3).expect("a nonzero count renders");
    assert_eq!(img.width, 16);
    assert_eq!(img.height, 16);
    assert_eq!(img.rgba.len(), 16 * 16 * 4);

    for (x, y) in [(0, 0), (15, 0), (0, 15), (15, 15)] {
        assert_eq!(img.pixel(x, y), Some(Rgba::TRANSPARENT), "corner ({x},{y}) must be clear");
    }
    // Mid-edge is inside the disc.
    assert_eq!(img.pixel(0, 8).map(|p| p.a), Some(255), "the disc must reach the left edge");
    assert_eq!(img.pixel(8, 0).map(|p| p.a), Some(255), "the disc must reach the top edge");
}

/// The digit must be drawn in white on the attention colour, at exact pixels.
///
/// A `3` is centred at x offset 5, y offset 4 in a 16-pixel icon, and its top
/// row is solid. If the glyph table, the centring or the scale ever drifts,
/// this is the assertion that notices.
#[test]
fn the_digit_lands_on_exact_pixels() {
    let img = render_count_icon(16, 3).expect("a nonzero count renders");
    // Top row of `3` is 0b11111, at y = (16 - 7) / 2 = 4, x from (16 - 5) / 2 = 5.
    for x in 5..10 {
        assert_eq!(img.pixel(x, 4), Some(Rgba::WHITE), "glyph pixel ({x},4)");
    }
    // The pixel just left of the glyph is disc, not glyph.
    assert_eq!(img.pixel(4, 4), Some(Rgba::ATTENTION));
    // Second row of `3` is 0b00010: only the fourth column is set.
    assert_eq!(img.pixel(8, 5), Some(Rgba::WHITE));
    assert_eq!(img.pixel(5, 5), Some(Rgba::ATTENTION));
}

/// Two glyphs must be centred as a unit with a one-pixel gap.
///
/// `9+` is 11 pixels wide, so it starts at x = 2. Centring each glyph
/// independently, or forgetting the gap, produces overlapping strokes.
#[test]
fn two_glyphs_are_centred_as_a_unit() {
    let img = render_count_icon(16, 42).expect("a nonzero count renders");
    // Top row of `9` is 0b01110: columns 1..4 of the glyph, so x = 3, 4, 5.
    assert_eq!(img.pixel(3, 4), Some(Rgba::WHITE));
    assert_eq!(img.pixel(4, 4), Some(Rgba::WHITE));
    assert_eq!(img.pixel(5, 4), Some(Rgba::WHITE));
    // Column 0 of `9` is clear on the top row.
    assert_eq!(img.pixel(2, 4), Some(Rgba::ATTENTION));
    // The gap column between the two glyphs is never drawn on.
    // `9` occupies x 2..7, the gap is x 7, `+` starts at x 8.
    // Row 3 of `+` is 0b11111, at y = 4 + 3 = 7, so x 8..13.
    for x in 8..13 {
        assert_eq!(img.pixel(x, 7), Some(Rgba::WHITE), "plus stroke at ({x},7)");
    }
}

/// The idle icon must be grey and must not be the attention colour.
///
/// A permanently red tray icon trains the user to ignore red.
#[test]
fn the_idle_icon_is_grey() {
    let img = render_idle_icon(22);
    assert_eq!(img.width, 22);
    assert_eq!(img.pixel(11, 11), Some(Rgba::IDLE));
    assert_eq!(img.pixel(0, 0), Some(Rgba::TRANSPARENT));
    assert_ne!(img.pixel(11, 11), Some(Rgba::ATTENTION));
}

/// The tray icon must switch from idle to count at one.
#[test]
fn the_tray_icon_switches_at_one() {
    assert_eq!(render_tray_icon(16, 0), render_idle_icon(16));
    assert_eq!(render_tray_icon(16, 1), render_count_icon(16, 1).expect("count icon"));
}

/// A larger icon must scale the glyph rather than leaving it tiny.
///
/// A 32-pixel tray icon with a 5x7 glyph in the middle looks broken. Integer
/// scaling keeps the bitmap crisp; interpolation would not.
#[test]
fn a_larger_icon_scales_the_glyph() {
    let img = render_count_icon(32, 3).expect("count icon");
    assert_eq!(img.width, 32);
    // Scale 2: the glyph is 10x14, starting at x = 11, y = 9.
    for x in 11..21 {
        for y in 9..11 {
            assert_eq!(img.pixel(x, y), Some(Rgba::WHITE), "scaled glyph pixel ({x},{y})");
        }
    }
}

/// An icon smaller than one glyph must be refused rather than drawn clipped.
#[test]
fn an_icon_too_small_for_a_glyph_is_refused() {
    assert!(render_count_icon(4, 3).is_none());
    assert!(render_count_icon(GLYPH_HEIGHT as u32 - 1, 3).is_none());
    assert!(render_count_icon(GLYPH_HEIGHT as u32, 3).is_some());
}

/// BGRA conversion must swap red and blue and keep alpha last.
///
/// A Win32 DIB is BGRA. Getting the order wrong renders the badge blue, which
/// is the kind of bug that only shows up on a machine nobody in the team has.
#[test]
fn bgra_conversion_swaps_red_and_blue() {
    let img = render_count_icon(16, 1).expect("count icon");
    let bgra = img.to_bgra();
    assert_eq!(bgra.len(), img.rgba.len());
    // The centre pixel of a `1` glyph column is white, so pick a disc pixel.
    let i = ((4u32 * 16 + 1) * 4) as usize;
    let px = img.pixel(1, 4).expect("in bounds");
    assert_eq!(px, Rgba::ATTENTION);
    assert_eq!(&bgra[i..i + 4], &[px.b, px.g, px.r, px.a]);
    assert_eq!(&bgra[i..i + 4], &[0x3F, 0x3F, 0xD1, 0xFF]);
}

/// ARGB network order must put alpha first.
///
/// The StatusNotifierItem specification says ARGB32 in network byte order. A
/// backend that handed it RGBA renders the tray icon with the alpha channel
/// interpreted as red.
#[test]
fn argb_network_conversion_puts_alpha_first() {
    let img = render_count_icon(16, 1).expect("count icon");
    let argb = img.to_argb_network();
    assert_eq!(argb.len(), img.rgba.len());
    let i = ((4u32 * 16 + 1) * 4) as usize;
    assert_eq!(&argb[i..i + 4], &[0xFF, 0xD1, 0x3F, 0x3F]);
    // A transparent corner stays fully transparent in both orders.
    assert_eq!(&argb[0..4], &[0, 0, 0, 0]);
}

/// Rasterisation must be deterministic.
///
/// The whole reason for a hand-rolled 5x7 font is that the result does not
/// depend on which fonts the machine has. Two calls must be byte-identical.
#[test]
fn rasterisation_is_deterministic() {
    assert_eq!(render_count_icon(16, 7), render_count_icon(16, 7));
    assert_eq!(render_idle_icon(22), render_idle_icon(22));
}

/// Unsupported characters must be reported, not silently blanked.
///
/// A caller that asked for a glyph we do not have gets `None` so the mistake is
/// visible, rather than an icon with a hole where a character should be.
#[test]
fn unsupported_glyphs_are_reported() {
    assert!(glyph('0').is_some());
    assert!(glyph('9').is_some());
    assert!(glyph('+').is_some());
    assert!(glyph('>').is_some());
    assert!(glyph('_').is_some());
    assert!(glyph('a').is_none());
    assert!(glyph(' ').is_none());
    assert!(glyph('日').is_none());
}

/// Every glyph must fit in five columns.
///
/// A row with the sixth bit set would silently shift the whole glyph left by
/// one when drawn, because the renderer shifts by `GLYPH_WIDTH - 1 - col`.
#[test]
fn every_glyph_fits_in_five_columns() {
    for c in "0123456789+>_".chars() {
        let g = glyph(c).expect("listed glyph exists");
        assert_eq!(g.len(), GLYPH_HEIGHT);
        for (row, bits) in g.iter().enumerate() {
            assert_eq!(
                bits & !((1u8 << GLYPH_WIDTH) - 1),
                0,
                "glyph {c:?} row {row} has a bit outside five columns"
            );
        }
    }
}

/// Reading outside the image must return `None`, not panic.
#[test]
fn out_of_bounds_pixels_are_none() {
    let img = render_idle_icon(16);
    assert!(img.pixel(16, 0).is_none());
    assert!(img.pixel(0, 16).is_none());
    assert!(img.pixel(u32::MAX, u32::MAX).is_none());
}
