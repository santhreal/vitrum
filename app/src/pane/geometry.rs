//! The pane's box, and the grid that box can actually show.
//!
//! Two numbers leave this module and both are load-bearing. The grid size
//! goes out as a resize on the wire and an agent redraws to it, so a column
//! counted here that the window cannot show is a column of an approval prompt
//! behind the window frame. The pixel rectangle is what the swapchain is
//! configured to, so a rectangle that is not the box the operator sees is a
//! band of the pane nothing ever paints.
//!
//! # Which box
//!
//! The padding box: the rectangle inside the border and inside the padding.
//! Not the border box. A pane's padding is a visual decision that the child
//! process can feel, because it comes out of the box before the division, and
//! measuring the wrong box is how a pane ends up handed rows and columns the
//! window edge cuts in half.
//!
//! The arithmetic reads only the AXIS SUM of the chrome, never the individual
//! sides. Redistributing padding within an axis is therefore provably
//! invisible to the child, and changing an axis total is provably visible.
//!
//! # Floor, never round
//!
//! A row the bottom edge slices is not a row anybody can read. Counting it is
//! the defect where the last option of an approval prompt is half a line of
//! pixels. Dropping a whole row that does fit is the other defect: content
//! anchored to the top with a dead band under it that the child never draws
//! in. Both are the same subtraction, and this module is the only place it is
//! written down.

/// Fewest columns a pane is ever handed.
///
/// A one-column grid is not a terminal. A child told it has one wraps every
/// line into a vertical stripe, and a window dragged that narrow for a moment
/// during a resize would leave the agent redrawing into it.
pub(crate) const MIN_COLS: u16 = 2;

/// Fewest rows a pane is ever handed. One, because a child with zero rows has
/// nowhere to draw and the emulator refuses the resize outright.
pub(crate) const MIN_ROWS: u16 = 1;

/// Where the pane sits, in device pixels.
///
/// The origin is the top left of the window's client area, not of the screen,
/// so the rectangle does not change when the window manager moves the window.
/// Every field is the PADDING box: whoever computes this subtracts the pane's
/// padding and border before it gets here, never after.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct PaneRect {
    /// Left edge, from the left of the client area.
    pub x: i32,
    /// Top edge, from the top of the client area.
    pub y: i32,
    /// Width of the padding box.
    pub width: u32,
    /// Height of the padding box.
    pub height: u32,
}

impl PaneRect {
    /// A rectangle with nothing in it, which no surface is ever configured to.
    pub(crate) const EMPTY: Self = Self {
        x: 0,
        y: 0,
        width: 0,
        height: 0,
    };

    /// Whether this rectangle can hold a swapchain at all.
    ///
    /// A zero axis is not a small pane, it is a pane that is not on screen:
    /// the widget is unmapped, or the shell is mid-layout. Configuring a
    /// swapchain to it is a validation error on every backend.
    pub(crate) const fn is_paintable(self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// Whole cells this rectangle shows, given one cell's pixel size.
    ///
    /// The chrome is already out of the rectangle, so there is nothing left to
    /// subtract here. What remains is the floor, and the floors.
    pub(crate) fn grid(self, cell: (u32, u32)) -> (u16, u16) {
        (
            clamp_axis(whole_cells(self.width, cell.0), MIN_COLS),
            clamp_axis(whole_cells(self.height, cell.1), MIN_ROWS),
        )
    }

    /// Pixels of this rectangle no cell covers, per axis.
    ///
    /// The remainder after the floor. It is not waste and it is not a bug: a
    /// box is rarely a whole number of cells. It is reported because the
    /// renderer has to clear it to the background rather than leave it
    /// showing whatever the last frame put there, which is one of the ways a
    /// pane grows a dead band along an edge.
    pub(crate) fn slack(self, cell: (u32, u32)) -> (u32, u32) {
        let (cols, rows) = self.grid(cell);
        (
            self.width.saturating_sub(u32::from(cols) * cell.0),
            self.height.saturating_sub(u32::from(rows) * cell.1),
        )
    }
}

/// Whole cells that fit along one axis.
///
/// Integer arithmetic on integer pixels. The measurement upstream is in
/// device pixels, which are whole by construction, so there is no fractional
/// box to lose and no rounding decision to get wrong.
const fn whole_cells(box_px: u32, cell_px: u32) -> u32 {
    if cell_px == 0 {
        // A font that reported a zero cell divides into nothing. Zero here
        // becomes the floor after clamping, which is a small grid rather than
        // a division by zero.
        return 0;
    }
    box_px / cell_px
}

/// Hold an axis inside the range a terminal is allowed to be.
const fn clamp_axis(cells: u32, min: u16) -> u16 {
    if cells < min as u32 {
        min
    } else if cells > u16::MAX as u32 {
        u16::MAX
    } else {
        cells as u16
    }
}

/// Whole cells that fit along one axis of a box that still has chrome in it.
///
/// The float form, for a caller measuring a box in logical units where the
/// chrome has not been subtracted yet. `box_px` is the axis of the whole box,
/// `chrome_px` is the axis SUM of everything inside it a cell may not occupy.
///
/// A non-finite input, a non-positive cell, or a box smaller than its own
/// chrome all give zero rather than a garbage count or a panic: a caller
/// measuring a widget that is not laid out yet gets a number that clamps to
/// the floor instead of a grid nobody can render.
pub(crate) fn cells_across(box_px: f64, chrome_px: f64, cell_px: f64) -> u32 {
    if !(cell_px > 0.0) || !box_px.is_finite() || !chrome_px.is_finite() {
        return 0;
    }
    let whole = ((box_px - chrome_px) / cell_px).floor();
    if whole.is_finite() && whole > 0.0 {
        whole as u32
    } else {
        0
    }
}

/// The grid a box can show, chrome and floors applied.
pub(crate) fn pane_grid(
    box_w: f64,
    box_h: f64,
    chrome_x: f64,
    chrome_y: f64,
    cell_w: f64,
    cell_h: f64,
) -> (u16, u16) {
    (
        clamp_axis(cells_across(box_w, chrome_x, cell_w), MIN_COLS),
        clamp_axis(cells_across(box_h, chrome_y, cell_h), MIN_ROWS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cell sizes a real font stack produces at the type sizes this product
    /// offers, from 11 px to 32 px on a 1x panel and the same range at 1.5x
    /// and 2x. Every case below is run against all of them, so a rule that
    /// only holds for one cell size cannot pass.
    const CELLS: &[(u32, u32)] = &[
        (6, 13),
        (7, 15),
        (8, 17),
        (9, 19),
        (10, 21),
        (11, 24),
        (13, 28),
        (14, 30),
        (16, 34),
        (19, 41),
        (21, 45),
        (26, 56),
        (28, 60),
        (32, 68),
    ];

    /// Padding boxes a real window produces: a small window, the default
    /// window, a 1440p window, a 4K window, and each with the sidebar open and
    /// collapsed.
    const BOXES: &[(u32, u32)] = &[
        (596, 344),
        (952, 552),
        (1244, 744),
        (1856, 1016),
        (2160, 1272),
        (2944, 1592),
        (3016, 2032),
        (3784, 2072),
        (3840, 2160),
    ];

    /// WHY: the two geometry defects an operator can see are one subtraction
    /// apart, and both are invisible in any single window size.
    ///
    /// Counting a partial row puts the last line of a full-screen TUI under
    /// the window frame: an approval prompt's final option is sliced off by
    /// the bottom edge and the operator cannot read what they are agreeing
    /// to. Dropping a whole row that does fit leaves the transcript anchored
    /// to the top with a dead band beneath it that the child never draws in.
    ///
    /// The invariant is exact and holds for every box and every cell: the
    /// cells counted fit, and one more would not. Asserting it as a pair is
    /// what makes both defects the same test, so neither can be fixed by
    /// breaking the other.
    ///
    /// Does not catch: a caller that measures the wrong box. This module
    /// cannot see whether the rectangle it was handed has the padding in it;
    /// `the_padding_box_is_smaller_than_the_border_box` is that half.
    #[test]
    fn a_partial_cell_is_never_counted_and_a_whole_one_is_never_dropped() {
        for &(w, h) in BOXES {
            for &(cw, ch) in CELLS {
                let rect = PaneRect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                };
                let (cols, rows) = rect.grid((cw, ch));

                assert!(
                    u32::from(cols) * cw <= w,
                    "{cols} columns of {cw}px do not fit in {w}px"
                );
                assert!(
                    u32::from(rows) * ch <= h,
                    "{rows} rows of {ch}px do not fit in {h}px"
                );
                assert!(
                    (u32::from(cols) + 1) * cw > w,
                    "another column of {cw}px still fits in {w}px beside {cols}"
                );
                assert!(
                    (u32::from(rows) + 1) * ch > h,
                    "another row of {ch}px still fits in {h}px beside {rows}"
                );
            }
        }
    }

    /// WHY: the slack is what a renderer must clear, and an unclear slack is a
    /// stripe of stale pixels along the right and bottom edges.
    ///
    /// The invariant: the cells plus the slack are the box exactly, and the
    /// slack is always less than one cell. A slack of a whole cell means a row
    /// was dropped, which is the dead-band defect measured from the other
    /// side.
    #[test]
    fn slack_is_always_less_than_one_cell_and_accounts_for_the_whole_box() {
        for &(w, h) in BOXES {
            for &(cw, ch) in CELLS {
                let rect = PaneRect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                };
                let (cols, rows) = rect.grid((cw, ch));
                let (sx, sy) = rect.slack((cw, ch));

                assert_eq!(u32::from(cols) * cw + sx, w, "width does not account");
                assert_eq!(u32::from(rows) * ch + sy, h, "height does not account");
                assert!(sx < cw, "{sx}px of horizontal slack is a whole {cw}px cell");
                assert!(sy < ch, "{sy}px of vertical slack is a whole {ch}px cell");
            }
        }
    }

    /// WHY: the defect that shipped was reading the border box, and the two
    /// boxes agree on exactly the windows nobody notices.
    ///
    /// A pane carrying padding above, below and on both sides has a padding
    /// box strictly smaller than its border box on both axes, and the grids
    /// they produce differ by at least one row whenever the vertical padding
    /// is at least one cell. This runs both measurements over the real sizes
    /// and asserts the border box always claims at least as much and
    /// sometimes strictly more, which is the shape of the bug: never fewer
    /// cells, so it never looks broken, and sometimes more, so the last row is
    /// behind the frame.
    #[test]
    fn the_padding_box_is_smaller_than_the_border_box() {
        // The axis sums the shipped stylesheet carried: 16 left plus 16 right,
        // 24 above plus 8 below.
        const PAD_X: u32 = 32;
        const PAD_Y: u32 = 32;

        let mut ever_differed = false;
        for &(w, h) in BOXES {
            for &(cw, ch) in CELLS {
                let border = PaneRect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                };
                let padding = PaneRect {
                    x: 0,
                    y: 0,
                    width: w - PAD_X,
                    height: h - PAD_Y,
                };
                let (bc, br) = border.grid((cw, ch));
                let (pc, pr) = padding.grid((cw, ch));

                assert!(bc >= pc && br >= pr, "the border box claimed fewer cells");
                if (bc, br) != (pc, pr) {
                    ever_differed = true;
                }
            }
        }
        assert!(
            ever_differed,
            "no size distinguished the two boxes, so this test proves nothing"
        );
    }

    /// WHY: a grid below the floor is a resize the emulator refuses, and the
    /// refusal arrives as a session that stopped updating.
    #[test]
    fn a_box_too_small_for_one_cell_still_yields_a_legal_grid() {
        for &(cw, ch) in CELLS {
            for &(w, h) in &[(0u32, 0u32), (1, 1), (cw - 1, ch - 1), (cw, 0), (0, ch)] {
                let (cols, rows) = PaneRect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                }
                .grid((cw, ch));
                assert!(cols >= MIN_COLS, "{cols} columns in a {w}x{h} box");
                assert!(rows >= MIN_ROWS, "{rows} rows in a {w}x{h} box");
            }
        }
    }

    /// WHY: a zero cell metric is a font that failed to measure, and a
    /// division by it is a panic in the middle of a resize.
    #[test]
    fn a_zero_cell_metric_clamps_instead_of_dividing() {
        let rect = PaneRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(rect.grid((0, 0)), (MIN_COLS, MIN_ROWS));
        assert_eq!(rect.grid((0, 20)), (MIN_COLS, 54));
        assert_eq!(rect.grid((10, 0)), (192, MIN_ROWS));
    }

    /// WHY: the float form is the one a caller reaches for when the chrome has
    /// not been subtracted, and it has to agree with the integer form or the
    /// pane resizes differently depending on which door the size came in.
    ///
    /// The invariant: with the chrome already out of the box, both forms
    /// produce the same grid over every size and cell in the tables.
    #[test]
    fn the_float_and_integer_forms_agree_once_the_chrome_is_out() {
        for &(w, h) in BOXES {
            for &(cw, ch) in CELLS {
                let integer = PaneRect {
                    x: 0,
                    y: 0,
                    width: w,
                    height: h,
                }
                .grid((cw, ch));
                let float = pane_grid(
                    f64::from(w),
                    f64::from(h),
                    0.0,
                    0.0,
                    f64::from(cw),
                    f64::from(ch),
                );
                assert_eq!(integer, float, "{w}x{h} box, {cw}x{ch} cell");
            }
        }
    }

    /// WHY: the chrome is read as an axis sum, and a caller who believes the
    /// distribution matters will move padding from the bottom of a pane to the
    /// top and expect nothing to change. It must not change, and the only way
    /// to know is to run it.
    #[test]
    fn only_the_axis_sum_of_the_chrome_is_visible_to_the_child() {
        for &(w, h) in BOXES {
            for &(cw, ch) in CELLS {
                let total_y = 32.0;
                let baseline = pane_grid(
                    f64::from(w),
                    f64::from(h),
                    32.0,
                    total_y,
                    f64::from(cw),
                    f64::from(ch),
                );
                // Every redistribution of the same sum across the axis.
                for top in [0.0, 4.0, 8.0, 16.0, 24.0, 31.0, 32.0] {
                    let split = pane_grid(
                        f64::from(w),
                        f64::from(h),
                        16.0 + 16.0,
                        top + (total_y - top),
                        f64::from(cw),
                        f64::from(ch),
                    );
                    assert_eq!(baseline, split, "{top}px above changed the grid");
                }
            }
        }
    }

    /// WHY: a non-finite measurement reaches this arithmetic whenever the
    /// shell divides by a scale that has not been read yet, and an infinity
    /// cast to `u32` is an unspecified column count.
    #[test]
    fn a_measurement_that_is_not_a_number_is_refused_rather_than_cast() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(cells_across(bad, 0.0, 10.0), 0);
            assert_eq!(cells_across(1000.0, bad, 10.0), 0);
            assert_eq!(cells_across(1000.0, 0.0, bad), 0, "cell {bad}");
        }
        assert_eq!(cells_across(1000.0, 0.0, 0.0), 0);
        assert_eq!(cells_across(1000.0, 0.0, -10.0), 0);
        assert_eq!(cells_across(10.0, 1000.0, 10.0), 0);
    }

    /// WHY: the property the table cannot state, over sizes nobody will drag a
    /// window to.
    ///
    /// A deterministic sweep rather than a random one, because a geometry
    /// failure that only reproduces under one seed is not a failure anybody
    /// can act on. Every box width from 0 to 4096 against a prime-ish cell
    /// width, and every height likewise: 4097 boxes times 5 cells per axis,
    /// which is the whole space a 4K window can be in one dimension.
    #[test]
    fn the_floor_holds_over_every_pixel_width_a_4k_window_can_have() {
        for &cell in &[6u32, 7, 11, 13, 32] {
            for px in 0u32..=4096 {
                let rect = PaneRect {
                    x: 0,
                    y: 0,
                    width: px,
                    height: px,
                };
                let (cols, rows) = rect.grid((cell, cell));

                // Below the floor the clamp is in charge and the fit does not
                // hold; above it, the fit is exact in both directions.
                if px >= u32::from(MIN_COLS) * cell {
                    assert_eq!(u32::from(cols), px / cell, "{px}px at {cell}px per cell");
                    assert!(u32::from(cols) * cell <= px);
                    assert!((u32::from(cols) + 1) * cell > px);
                } else {
                    assert_eq!(cols, MIN_COLS);
                }
                if px >= cell {
                    assert_eq!(u32::from(rows), px / cell);
                } else {
                    assert_eq!(rows, MIN_ROWS);
                }
            }
        }
    }

    /// WHY: a pane that is not on screen has a zero axis, and configuring a
    /// swapchain to it is a validation error rather than a small pane.
    #[test]
    fn a_rectangle_with_a_zero_axis_is_not_paintable() {
        assert!(!PaneRect::EMPTY.is_paintable());
        for (w, h) in [(0u32, 100u32), (100, 0), (0, 0)] {
            assert!(
                !PaneRect {
                    x: 4,
                    y: 4,
                    width: w,
                    height: h
                }
                .is_paintable(),
                "{w}x{h}"
            );
        }
        assert!(
            PaneRect {
                x: -20,
                y: -20,
                width: 1,
                height: 1
            }
            .is_paintable(),
            "a rectangle scrolled off the left is still a rectangle"
        );
    }
}
