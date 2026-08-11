//! What the path renderer is allowed to get wrong: nothing the two mark
//! tables can express.
//!
//! No widget is built here. `gtk_init` needs a display and this program is
//! tested without one, so what is asserted is the resolved geometry: the same
//! `Vec<Seg>` cairo is handed on every frame.

use super::*;
use crate::agent::AgentMarks;

/// How far a computed point may sit from where it belongs, in the 16-unit box
/// the marks are authored in.
///
/// Tight enough that a wrong arc centre, a flipped sweep or a dropped rotation
/// fails: any of those moves a point by whole units, not by a thousandth of
/// one.
const SLACK: f64 = 1e-9;

/// Every path string the product can draw, with the name of what draws it.
///
/// Enumerated from the two tables at run time rather than listed. A list would
/// go stale the first time an icon is added, and a renderer tested against a
/// stale list is a renderer tested against nothing.
fn every_path() -> Vec<(String, &'static str)> {
    let mut out = Vec::new();
    for icon in crate::ui::icons::ALL {
        out.push((format!("icon {}", icon.slug), icon.stroke));
        out.push((format!("icon {} fill", icon.slug), icon.fill));
    }
    for kind in vitrum_model::ALL_AGENT_KINDS {
        let mark = kind.mark();
        out.push((format!("{kind:?} mark"), mark.stroke));
        out.push((format!("{kind:?} mark fill"), mark.fill));
    }
    out
}

/// **Fails by default on a new command.** Every mark in the product resolves
/// completely, so an icon authored with a path command this file has never
/// seen fails here rather than drawing part of a shape in a release
/// screenshot.
#[test]
fn every_mark_in_the_product_parses() {
    for (what, data) in every_path() {
        let segs = parse(data)
            .unwrap_or_else(|| panic!("{what} uses a path command the renderer cannot draw"));
        assert_eq!(
            segs.is_empty(),
            data.trim().is_empty(),
            "{what} resolved to a path that does not match whether it has data"
        );
    }
}

/// Nothing is a path with no segments, not a refusal. Six of the seven agent
/// marks and twelve of the fourteen icons carry no solid subpath, and that is
/// the ordinary case rather than an error.
#[test]
fn an_empty_path_is_an_empty_shape() {
    assert_eq!(parse(""), Some(Vec::new()));
    assert_eq!(parse("   "), Some(Vec::new()));
}

/// A command this renderer does not implement is refused whole. Drawing the
/// prefix that did parse would put a truncated mark on screen and report
/// success.
#[test]
fn an_unknown_command_is_refused_rather_than_half_drawn() {
    assert_eq!(parse("M1 1 S2 2 3 3"), None);
    assert_eq!(parse("M1 1 T4 4"), None);
}

/// Truncated data is refused for the same reason: a lineto missing its second
/// coordinate is not a lineto.
#[test]
fn a_missing_coordinate_is_refused() {
    assert_eq!(parse("M1 1L2"), None);
    assert_eq!(parse("M1"), None);
}

/// A run of coordinate pairs after a moveto is a run of linetos, which is what
/// SVG says and what several of the marks rely on to stay readable.
#[test]
fn a_second_pair_after_a_moveto_is_a_line() {
    assert_eq!(
        parse("M1 2 3 4 5 6"),
        Some(vec![Seg::Move(1.0, 2.0), Seg::Line(3.0, 4.0), Seg::Line(5.0, 6.0)])
    );
}

/// The horizontal and vertical shorthands keep the other coordinate. Reading
/// zero instead would collapse every bar mark onto the top edge of its box.
#[test]
fn the_axis_shorthands_hold_the_other_coordinate() {
    assert_eq!(
        parse("M3.25 4.5H12.75V8"),
        Some(vec![
            Seg::Move(3.25, 4.5),
            Seg::Line(12.75, 4.5),
            Seg::Line(12.75, 8.0),
        ])
    );
}

/// A relative run draws the same shape as the absolute one it is a shorthand
/// for. Measuring a relative command from the origin rather than from the
/// current point is the classic defect, and it puts every subpath after the
/// first in the wrong place.
#[test]
fn relative_commands_are_measured_from_the_current_point() {
    let absolute = parse("M2 2L5 2L5 6Z").expect("absolute path parses");
    let relative = parse("m2 2l3 0l0 4z").expect("relative path parses");
    assert_eq!(absolute, relative);
}

/// A close returns the pen to the subpath's start, so what follows it is
/// measured from there rather than from the last coordinate written.
#[test]
fn a_close_returns_the_pen_to_the_start_of_the_subpath() {
    let segs = parse("M4 4L8 4L8 8Zl2 0").expect("path parses");
    assert_eq!(segs.last(), Some(&Seg::Line(6.0, 4.0)));
}

/// The quadratic conversion is exact, not an approximation. Asserted at the
/// midpoint, which is where the two curves differ most when the conversion is
/// wrong.
#[test]
fn a_quadratic_becomes_the_cubic_with_the_same_shape() {
    let from = (8.0, 1.2);
    let control = (9.35, 6.65);
    let to = (14.8, 8.0);
    let Some(segs) = parse("M8 1.2Q9.35 6.65 14.8 8") else {
        panic!("quadratic path parses");
    };
    let Some(Seg::Curve(c)) = segs.get(1) else {
        panic!("the quadratic resolved to something other than a curve");
    };

    let quad = |t: f64, a: f64, b: f64, d: f64| {
        let u = 1.0 - t;
        u * u * a + 2.0 * u * t * b + t * t * d
    };
    let cubic = |t: f64, a: f64, b: f64, d: f64, e: f64| {
        let u = 1.0 - t;
        u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * d + t * t * t * e
    };
    for step in 0..=8 {
        let t = f64::from(step) / 8.0;
        let want = (
            quad(t, from.0, control.0, to.0),
            quad(t, from.1, control.1, to.1),
        );
        let got = (
            cubic(t, from.0, c[0], c[2], c[4]),
            cubic(t, from.1, c[1], c[3], c[5]),
        );
        assert!(
            (want.0 - got.0).abs() < SLACK && (want.1 - got.1).abs() < SLACK,
            "at t={t} the cubic is at {got:?} and the quadratic at {want:?}"
        );
    }
}

/// **The ring must be a ring.** Every point the arc conversion produces sits
/// on the circle the path names, which is the assertion that catches a wrong
/// centre, a flipped sweep flag and a dropped radius correction at once.
#[test]
fn an_arc_pair_resolves_to_the_circle_it_names() {
    // The `ring` icon and the Veyyon mark: two half-turns of r 5.5 about
    // (8, 8), written the way every icon set writes a circle.
    let segs = parse("M2.5 8a5.5 5.5 0 1 0 11 0a5.5 5.5 0 1 0-11 0").expect("ring parses");

    let mut points = Vec::new();
    for seg in &segs {
        match *seg {
            Seg::Move(x, y) | Seg::Line(x, y) => points.push((x, y)),
            Seg::Curve(c) => points.push((c[4], c[5])),
            Seg::Close => {}
        }
    }
    assert!(
        points.len() > 4,
        "a full circle split into quarter turns is more than {} points",
        points.len()
    );
    for (x, y) in &points {
        let r = ((x - 8.0).powi(2) + (y - 8.0).powi(2)).sqrt();
        assert!(
            (r - 5.5).abs() < 1e-6,
            "({x}, {y}) is {r} from the centre, not 5.5"
        );
    }
    let last = points.last().copied().expect("the ring has points");
    assert!(
        (last.0 - 2.5).abs() < 1e-6 && (last.1 - 8.0).abs() < 1e-6,
        "the ring ends at {last:?} rather than back where it started"
    );
}

/// The two sweep directions are different arcs between the same endpoints. A
/// renderer that ignores the flag draws one of them twice, which turns every
/// rounded corner in the `brackets` icon inside out.
#[test]
fn the_sweep_flag_chooses_between_the_two_arcs() {
    let one = parse("M3 6A1.5 1.5 0 0 1 4.5 4.5").expect("clockwise parses");
    let other = parse("M3 6A1.5 1.5 0 0 0 4.5 4.5").expect("anticlockwise parses");
    assert_ne!(one, other);
}

/// A zero radius is a straight line, which is what SVG requires. Dropping the
/// segment instead would leave a gap in the outline.
#[test]
fn a_degenerate_arc_is_a_line() {
    assert_eq!(
        parse("M1 1A0 0 0 0 1 5 5"),
        Some(vec![Seg::Move(1.0, 1.0), Seg::Line(5.0, 5.0)])
    );
}
