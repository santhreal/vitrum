//! The product's marks, drawn with cairo.
//!
//! [`crate::agent`] and [`crate::ui::icons`] both describe a mark as SVG path
//! data in a 16-unit box: one family, one stroke weight, `currentColor`. That
//! description was written for a browser, which had a path renderer. A native
//! window does not, so this file is the renderer those two tables always
//! assumed.
//!
//! # Why the data is not rewritten as cairo calls
//!
//! The two tables are the product's visual identity and they are asserted
//! against by their own tests: [`crate::agent`] proves no two marks draw the
//! same shape and that no stroked mark carries a fill, and it reads the
//! numbers in the path data to check every mark's envelope. Transcribing
//! fourteen icons and seven agent marks into cairo calls would leave those
//! tests measuring a string nothing draws.
//!
//! # Why the parser is exact rather than forgiving
//!
//! [`parse`] returns `None` for path data it does not fully understand, and
//! [`tests::every_mark_in_the_product_parses`] walks both tables. A mark that
//! uses a command this file has never seen therefore fails the suite instead
//! of silently drawing half a shape, which is the failure mode a lenient
//! parser produces and nobody notices until a release screenshot.

use std::f64::consts::PI;
use std::rc::Rc;

use gtk::prelude::*;

/// The side of the box every mark is authored in.
///
/// Not a size. The widget is whatever the layout gives it and the path is
/// scaled into that, so this is the unit the coordinates in the two tables
/// are expressed in and nothing else.
const BOX: f64 = 16.0;

/// The stroke weight, in the same units.
///
/// One weight for every mark in both tables, which is what keeps a preset
/// chip and a session row from reading as two different products.
const STROKE: f64 = 1.25;

/// The largest sweep approximated by a single cubic.
///
/// A quarter turn is the usual bound: the maximum radial error of a cubic
/// approximation grows fast past it, and at 16 units a visible bulge in a
/// ring is the whole mark.
const ARC_STEP: f64 = PI / 2.0;

/// One piece of a resolved path.
///
/// Arcs and quadratics are gone by the time a path is a `Vec<Seg>`: cairo
/// draws cubics and lines, so the conversion happens once at parse rather
/// than once per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Curve([f64; 6]),
    Close,
}

/// A widget that draws `stroke` outlined and `fill` solid, in the widget's own
/// foreground colour.
///
/// The colour is read from the style context on every draw rather than baked
/// in, which is what `currentColor` meant in the markup this replaces: a mark
/// inside a selected row is the selected row's colour without the row having
/// to know a mark is in it.
pub(crate) fn mark(stroke: &str, fill: &str, class: &str) -> gtk::DrawingArea {
    let stroked = Rc::new(parse(stroke).unwrap_or_default());
    let filled = Rc::new(parse(fill).unwrap_or_default());

    let area = gtk::DrawingArea::new();
    area.style_context().add_class(class);
    // A mark is authored at 16 units and read at 16 units. The request is the
    // glyph's own size at the operator's text scale, so it grows with the text
    // it sits beside instead of shrinking into it.
    let side = crate::shell::style::rem(1.0).round() as i32;
    area.set_size_request(side, side);
    area.set_valign(gtk::Align::Center);

    area.connect_draw(move |area, cr| {
        let width = f64::from(area.allocated_width());
        let height = f64::from(area.allocated_height());
        let scale = width.min(height) / BOX;
        if scale <= 0.0 {
            return glib::Propagation::Proceed;
        }
        let style = area.style_context();
        let colour = style.color(style.state());

        let _ = cr.save();
        cr.translate(
            (width - BOX * scale) / 2.0,
            (height - BOX * scale) / 2.0,
        );
        cr.scale(scale, scale);
        cr.set_source_rgba(
            colour.red(),
            colour.green(),
            colour.blue(),
            colour.alpha(),
        );
        if !filled.is_empty() {
            trace(cr, &filled);
            let _ = cr.fill();
        }
        if !stroked.is_empty() {
            cr.set_line_width(STROKE);
            cr.set_line_cap(gtk::cairo::LineCap::Round);
            cr.set_line_join(gtk::cairo::LineJoin::Round);
            trace(cr, &stroked);
            let _ = cr.stroke();
        }
        let _ = cr.restore();
        glib::Propagation::Proceed
    });
    area
}

/// Put `segs` on `cr` as a path, drawing nothing.
fn trace(cr: &gtk::cairo::Context, segs: &[Seg]) {
    cr.new_path();
    for seg in segs {
        match *seg {
            Seg::Move(x, y) => cr.move_to(x, y),
            Seg::Line(x, y) => cr.line_to(x, y),
            Seg::Curve(c) => cr.curve_to(c[0], c[1], c[2], c[3], c[4], c[5]),
            Seg::Close => cr.close_path(),
        }
    }
}

/// Resolve SVG path data, or `None` when any of it is not understood.
///
/// Empty data is an empty path rather than a refusal, because a mark with no
/// solid subpath stores `""` for it and that is the normal case.
pub(crate) fn parse(data: &str) -> Option<Vec<Seg>> {
    let mut scan = Scan {
        src: data.as_bytes(),
        at: 0,
    };
    let mut out: Vec<Seg> = Vec::new();
    // Where a `Z` returns to, and where a relative command is measured from.
    let mut start = (0.0, 0.0);
    let mut here = (0.0, 0.0);
    let mut last: Option<u8> = None;

    loop {
        scan.gap();
        if scan.done() {
            return Some(out);
        }
        let command = match scan.command() {
            Some(c) => {
                last = Some(c);
                c
            }
            // An implicit repeat. SVG says a run of coordinate pairs after a
            // command repeats it, and after a moveto the repeat is a lineto.
            None => match last {
                Some(b'M') => b'L',
                Some(b'm') => b'l',
                Some(c) => c,
                None => return None,
            },
        };
        let relative = command.is_ascii_lowercase();
        let base = if relative { here } else { (0.0, 0.0) };

        match command.to_ascii_uppercase() {
            b'M' => {
                let (x, y) = (base.0 + scan.number()?, base.1 + scan.number()?);
                out.push(Seg::Move(x, y));
                here = (x, y);
                start = here;
            }
            b'L' => {
                let (x, y) = (base.0 + scan.number()?, base.1 + scan.number()?);
                out.push(Seg::Line(x, y));
                here = (x, y);
            }
            b'H' => {
                let x = base.0 + scan.number()?;
                out.push(Seg::Line(x, here.1));
                here = (x, here.1);
            }
            b'V' => {
                let y = base.1 + scan.number()?;
                out.push(Seg::Line(here.0, y));
                here = (here.0, y);
            }
            b'C' => {
                let c = [
                    base.0 + scan.number()?,
                    base.1 + scan.number()?,
                    base.0 + scan.number()?,
                    base.1 + scan.number()?,
                    base.0 + scan.number()?,
                    base.1 + scan.number()?,
                ];
                out.push(Seg::Curve(c));
                here = (c[4], c[5]);
            }
            b'Q' => {
                let qx = base.0 + scan.number()?;
                let qy = base.1 + scan.number()?;
                let x = base.0 + scan.number()?;
                let y = base.1 + scan.number()?;
                out.push(quadratic(here, (qx, qy), (x, y)));
                here = (x, y);
            }
            b'A' => {
                let rx = scan.number()?;
                let ry = scan.number()?;
                let turn = scan.number()?;
                let large = scan.flag()?;
                let sweep = scan.flag()?;
                let x = base.0 + scan.number()?;
                let y = base.1 + scan.number()?;
                arc(here, rx, ry, turn, large, sweep, (x, y), &mut out);
                here = (x, y);
            }
            b'Z' => {
                out.push(Seg::Close);
                here = start;
            }
            _ => return None,
        }
    }
}

/// A quadratic as the cubic with the same shape.
///
/// Exact, not an approximation: every quadratic is a cubic whose control
/// points sit two thirds of the way from each end toward the quadratic's one.
fn quadratic(from: (f64, f64), control: (f64, f64), to: (f64, f64)) -> Seg {
    let third = 2.0 / 3.0;
    Seg::Curve([
        from.0 + third * (control.0 - from.0),
        from.1 + third * (control.1 - from.1),
        to.0 + third * (control.0 - to.0),
        to.1 + third * (control.1 - to.1),
        to.0,
        to.1,
    ])
}

/// Push the cubics that approximate one SVG elliptical arc.
///
/// The endpoint parameterisation SVG uses says where an arc ends; cairo wants
/// to know where its centre is. This is that conversion, followed by a split
/// into sweeps of at most [`ARC_STEP`] so no single cubic is asked to bend
/// further than one can.
#[allow(clippy::too_many_arguments)]
fn arc(
    from: (f64, f64),
    rx: f64,
    ry: f64,
    turn_deg: f64,
    large: bool,
    sweep: bool,
    to: (f64, f64),
    out: &mut Vec<Seg>,
) {
    // A degenerate radius is a straight line, which is what SVG requires
    // rather than a dropped segment.
    if rx == 0.0 || ry == 0.0 || (from.0 == to.0 && from.1 == to.1) {
        out.push(Seg::Line(to.0, to.1));
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    let turn = turn_deg.to_radians();
    let (sin, cos) = turn.sin_cos();

    let dx = (from.0 - to.0) / 2.0;
    let dy = (from.1 - to.1) / 2.0;
    let x1 = cos * dx + sin * dy;
    let y1 = -sin * dx + cos * dy;

    // Radii too small to reach the endpoint are scaled up until they do,
    // which SVG specifies so that hand-written data still draws an arc.
    let over = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if over > 1.0 {
        let grow = over.sqrt();
        rx *= grow;
        ry *= grow;
    }

    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let sign = if large == sweep { -1.0 } else { 1.0 };
    let coef = sign * (num / den).sqrt();
    let cxp = coef * rx * y1 / ry;
    let cyp = -coef * ry * x1 / rx;
    let cx = cos * cxp - sin * cyp + (from.0 + to.0) / 2.0;
    let cy = sin * cxp + cos * cyp + (from.1 + to.1) / 2.0;

    let start = ((y1 - cyp) / ry).atan2((x1 - cxp) / rx);
    let end = ((-y1 - cyp) / ry).atan2((-x1 - cxp) / rx);
    let mut span = end - start;
    if !sweep && span > 0.0 {
        span -= 2.0 * PI;
    } else if sweep && span < 0.0 {
        span += 2.0 * PI;
    }

    let steps = (span.abs() / ARC_STEP).ceil().max(1.0);
    let step = span / steps;
    let alpha = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |angle: f64| {
        let (s, c) = angle.sin_cos();
        (
            cx + rx * cos * c - ry * sin * s,
            cy + rx * sin * c + ry * cos * s,
        )
    };
    let slope = |angle: f64| {
        let (s, c) = angle.sin_cos();
        (
            -rx * cos * s - ry * sin * c,
            -rx * sin * s + ry * cos * c,
        )
    };

    for i in 0..steps as usize {
        let a = start + step * i as f64;
        let b = a + step;
        let (px, py) = point(a);
        let (qx, qy) = point(b);
        let (dpx, dpy) = slope(a);
        let (dqx, dqy) = slope(b);
        out.push(Seg::Curve([
            px + alpha * dpx,
            py + alpha * dpy,
            qx - alpha * dqx,
            qy - alpha * dqy,
            qx,
            qy,
        ]));
    }
}

/// A cursor over path data.
struct Scan<'a> {
    src: &'a [u8],
    at: usize,
}

impl Scan<'_> {
    fn done(&self) -> bool {
        self.at >= self.src.len()
    }

    /// Skip whitespace and the commas SVG allows between any two numbers.
    fn gap(&mut self) {
        while self.at < self.src.len() {
            match self.src[self.at] {
                b' ' | b'\t' | b'\r' | b'\n' | b',' => self.at += 1,
                _ => break,
            }
        }
    }

    /// The next command letter, or `None` when the next token is a number.
    fn command(&mut self) -> Option<u8> {
        let byte = *self.src.get(self.at)?;
        if byte.is_ascii_alphabetic() {
            self.at += 1;
            Some(byte)
        } else {
            None
        }
    }

    fn number(&mut self) -> Option<f64> {
        self.gap();
        let from = self.at;
        if matches!(self.src.get(self.at), Some(b'+' | b'-')) {
            self.at += 1;
        }
        self.digits();
        if self.src.get(self.at) == Some(&b'.') {
            self.at += 1;
            self.digits();
        }
        if matches!(self.src.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.src.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            self.digits();
        }
        std::str::from_utf8(&self.src[from..self.at])
            .ok()?
            .parse()
            .ok()
    }

    fn digits(&mut self) {
        while matches!(self.src.get(self.at), Some(b) if b.is_ascii_digit()) {
            self.at += 1;
        }
    }

    /// An arc flag, which is one character and may be run up against the next
    /// number with no separator at all.
    fn flag(&mut self) -> Option<bool> {
        self.gap();
        let byte = *self.src.get(self.at)?;
        self.at += 1;
        match byte {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
