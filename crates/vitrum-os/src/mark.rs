//! The brand mark, rasterised from its geometry rather than from a picture.
//!
//! `assets/logo/vitrum.svg` is the only place the mark's shape is written down:
//! a cut stone on a 96 unit grid, ten stroked segments, round joins and caps.
//! Every raster the platforms need — the window icon, the Windows `.ico`, the
//! macOS `.icns`, the freedesktop hicolor PNGs — comes out of this file, so
//! there is no checked-in binary that can drift from the drawing and no
//! external converter in the build.
//!
//! The rasteriser is a pure function of `(size, colour)`, in the same shape as
//! [`crate::icon`], which is what makes "the girdle is open at 32 pixels" a
//! claim a Linux test run can check byte by byte.
//!
//! # Anti-aliasing
//!
//! The mark is four diagonals and a horizontal. Point-sampling a stroked
//! diagonal at 16 pixels produces a staircase, so coverage is computed
//! analytically: the distance from the pixel centre to the nearest segment is
//! turned into an alpha through a one-pixel-wide ramp centred on the stroke
//! outline. Round caps and joins come free, because the distance to a union of
//! capsules already rounds every end and every corner.
//!
//! # Symmetry
//!
//! The mark is symmetric about its vertical axis and the raster has to be
//! symmetric to the byte, because a mark that is one alpha step heavier on the
//! left is visibly off-centre at 16 pixels. That is structural here rather than
//! hoped for: geometry is stored with x measured from the axis, only the left
//! half is stored, and the right half is reached by negating the sample's x.
//! Negation is exact in binary floating point and IEEE rounding is symmetric
//! under it, so the mirrored pixel takes the identical code path to the
//! identical bits.

use crate::icon::{IconImage, Rgba};

/// The grid the mark's geometry is drawn on.
///
/// `assets/logo/vitrum.svg` uses a `0 0 96 96` viewBox and every coordinate in
/// this module is in those units.
pub const MARK_GRID: f64 = 96.0;

/// Stroke width of the mark on the grid, matching the SVG's `stroke-width`.
pub const MARK_STROKE: f64 = 5.0;

/// Grid y of the girdle, the horizontal the crown and the pavilion meet on.
pub const MARK_GIRDLE_Y: f64 = 42.0;

/// Grid y of the culet, the point at the bottom of the pavilion.
pub const MARK_CULET_Y: f64 = 84.0;

/// Grid x, measured from the vertical axis, where the girdle segments stop.
///
/// The girdle is drawn as two segments so the middle stays open and the V
/// reads through it. The stop is where the line from a table corner to the
/// culet crosses the girdle, which is what makes the opening land on the
/// facets rather than at an arbitrary distance from them.
pub const MARK_GIRDLE_STOP: f64 = 12.6;

/// Smallest stroke half-width the rasteriser will draw, in device pixels.
///
/// Scaling the stroke honestly gives `5 * 16/96`, a 0.83 pixel line, and a
/// sub-pixel line rendered by coverage alone is a grey smudge rather than a
/// mark. Sixteen pixels is the only shipped size this floor lifts; at 24 the
/// true stroke is already 1.25 pixels and the floor does nothing. Raising it
/// further closes the girdle's opening at 16, which is the one feature of the
/// geometry that has to survive the smallest size.
const MIN_STROKE_HALF_PX: f64 = 0.55;

/// Width of the coverage ramp across a stroke's outline, in device pixels.
///
/// One pixel is the physically exact answer: it is what a box filter over the
/// pixel's own area computes, and it is what a large icon wants. It is also
/// what turns the 16 pixel raster into a single soft lozenge, because at that
/// size every gap in the drawing is under a pixel wide and a one-pixel ramp
/// spreads each stroke across the gap beside it until the negative space is
/// gone.
///
/// Half a pixel keeps two to three intermediate levels on a diagonal, which is
/// all a diagonal needs to stop being a staircase, and leaves the openings in
/// the mark visible at 16. It is one constant at every size rather than a
/// small-size special case, so nothing about the drawing changes with the
/// raster it is drawn into.
const EDGE_RAMP_PX: f64 = 0.5;

/// Every size the platform icon set ships.
///
/// One list, read by the emitter, by the window icon and by the pixel tests,
/// so adding a size adds coverage rather than adding an untested raster.
/// 16 through 64 are what a taskbar, a titlebar and a launcher row ask for;
/// 128 through 512 are what a HiDPI dock, a macOS icon and the freedesktop
/// hicolor tree ask for.
pub const MARK_SIZES: &[u32] = &[16, 24, 32, 48, 64, 128, 256, 512];

/// The colour the mark is drawn in when nobody names one.
///
/// The product accent. The mark is a line drawing with no plate behind it, so
/// it is composited straight onto whatever the taskbar, dock or title bar is
/// painted with, and the accent is the one colour that is legible on both a
/// dark and a light one. Black or white would each disappear on one of them.
pub const MARK_COLOUR: Rgba = Rgba { r: 0x4C, g: 0x6E, b: 0xF5, a: 0xFF };

/// One stroked segment: `[ax, ay, bx, by]` in grid units, x measured from the
/// vertical axis.
type Segment = [f64; 4];

/// The left half of the mark, and the halves of the shapes that cross the axis.
///
/// Mirroring these about x = 0 and taking the union gives the whole drawing.
/// The table and the pavilion cross the axis and are cut there; the cut adds a
/// round cap at the axis, which lands inside the union and changes nothing.
const HALF: &[Segment] = &[
    // Crown: the shoulder up from the girdle to the table corner.
    [-36.0, 42.0, -18.0, 24.0],
    // Crown: half the table.
    [-18.0, 24.0, 0.0, 24.0],
    // Pavilion: the shoulder down from the girdle to the culet.
    [-36.0, 42.0, 0.0, 84.0],
    // Girdle, stopping where the facet crosses it.
    [-36.0, 42.0, -MARK_GIRDLE_STOP, 42.0],
    // The V: the table corner down to the culet.
    [-18.0, 24.0, 0.0, 84.0],
];

/// The parts of the mark that lie on the axis and are their own mirror.
const AXIS: &[Segment] = &[
    // The T: a stem from the middle of the table through the girdle.
    [0.0, 24.0, 0.0, 50.0],
];

/// Distance from a point to a segment, in grid units.
fn segment_distance(px: f64, py: f64, s: &Segment) -> f64 {
    let (ax, ay, bx, by) = (s[0], s[1], s[2], s[3]);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    // Clamped projection, so a segment is a segment and not a line: the mark
    // is made of capsules and the clamp is what puts the round cap on the end.
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    let (ex, ey) = (px - cx, py - cy);
    (ex * ex + ey * ey).sqrt()
}

/// Distance from a point to the whole mark, in grid units.
///
/// `x` is measured from the vertical axis. The right half is reached by
/// negating `x` rather than by storing mirrored segments, which is what makes
/// the two halves bit-identical rather than merely close.
fn mark_distance(x: f64, y: f64) -> f64 {
    let mut best = f64::INFINITY;
    for s in HALF {
        let left = segment_distance(x, y, s);
        let right = segment_distance(-x, y, s);
        best = best.min(left).min(right);
    }
    for s in AXIS {
        best = best.min(segment_distance(x, y, s));
    }
    best
}

/// The mark, anti-aliased, in straight RGBA.
///
/// `size` is both width and height; the mark is square and every platform that
/// takes an icon takes a square one. `colour`'s alpha scales the whole mark, so
/// a translucent watermark is `MARK_COLOUR` with a lower `a` rather than a
/// second code path.
///
/// Nothing is clipped: the outermost ink is the girdle's round cap at grid
/// x = 36 + half the stroke, which stays inside the box at every size in
/// [`MARK_SIZES`], so the four corners are always transparent.
#[must_use]
pub fn render_mark(size: u32, colour: Rgba) -> IconImage {
    let n = size.max(1);
    let px_per_grid = f64::from(n) / MARK_GRID;
    let half_stroke = (MARK_STROKE / 2.0).max(MIN_STROKE_HALF_PX / px_per_grid);
    // Half the raster, exactly: `n/2` is representable for both parities, and
    // subtracting it before the scale is what makes column `x` and column
    // `n - 1 - x` sample exactly negated grid coordinates.
    let centre = f64::from(n) / 2.0;

    let mut rgba = vec![0u8; (n as usize) * (n as usize) * 4];
    for y in 0..n {
        let gy = (f64::from(y) + 0.5) * MARK_GRID / f64::from(n);
        for x in 0..n {
            let gx = (f64::from(x) + 0.5 - centre) * MARK_GRID / f64::from(n);
            let d = mark_distance(gx, gy);
            // An `EDGE_RAMP_PX` wide ramp centred on the outline: fully inside
            // half a ramp in, fully outside half a ramp out. `d` is in grid
            // units, so it is scaled back to pixels before the ramp.
            let coverage =
                (0.5 + (half_stroke - d) * px_per_grid / EDGE_RAMP_PX).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let i = ((y as usize) * (n as usize) + x as usize) * 4;
            rgba[i] = colour.r;
            rgba[i + 1] = colour.g;
            rgba[i + 2] = colour.b;
            rgba[i + 3] = (coverage * f64::from(colour.a)).round() as u8;
        }
    }
    IconImage { width: n, height: n, rgba }
}

/// The mark at every shipped size, in [`MARK_SIZES`] order.
#[must_use]
pub fn mark_set(colour: Rgba) -> Vec<IconImage> {
    MARK_SIZES.iter().map(|&s| render_mark(s, colour)).collect()
}
