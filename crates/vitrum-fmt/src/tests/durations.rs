//! Elapsed-time labels: the two-unit compact form, the one-unit terse form, and
//! the fixed-shape clock form.

use crate::duration::{Parts, clock, compact, terse};
use std::time::Duration;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// A zero duration renders `0s`, not an empty string.
///
/// A session that has just started is a real state and its timer has to show
/// something. An empty label reads as "no data" and a missing unit reads as a
/// broken format string.
#[test]
fn zero_renders_as_zero_seconds() {
    assert_eq!(compact(Duration::ZERO), "0s");
    assert_eq!(terse(Duration::ZERO), "0s");
    assert_eq!(clock(Duration::ZERO), "00:00");
}

/// Sub-second durations floor to `0s` rather than showing milliseconds.
///
/// A label that ticked in milliseconds would force a re-render every frame,
/// which is exactly the idle-CPU cost this product is built to avoid.
#[test]
fn sub_second_durations_floor_to_zero() {
    assert_eq!(compact(Duration::from_millis(1)), "0s");
    assert_eq!(compact(Duration::from_millis(999)), "0s");
    assert_eq!(terse(Duration::from_millis(999)), "0s");
    assert_eq!(compact(Duration::from_millis(1_000)), "1s");
}

/// Seconds render alone below one minute.
#[test]
fn seconds_render_alone_below_one_minute() {
    assert_eq!(compact(secs(1)), "1s");
    assert_eq!(compact(secs(12)), "12s");
    assert_eq!(compact(secs(59)), "59s");
    assert_eq!(terse(secs(59)), "59s");
}

/// The compact form shows minutes and seconds together: `4m 12s`.
///
/// This is the running "working for" label. The seconds are what make a slow
/// agent visibly slow; without them a user cannot tell a stuck session from one
/// that is merely between tool calls.
#[test]
fn compact_shows_minutes_with_seconds() {
    assert_eq!(compact(secs(60)), "1m");
    assert_eq!(compact(secs(61)), "1m 1s");
    assert_eq!(compact(secs(252)), "4m 12s");
    assert_eq!(compact(secs(3_599)), "59m 59s");
}

/// The compact form shows hours with minutes, and drops the seconds.
///
/// `2h 5m 30s` is three units of precision nobody reads and a label that
/// changes every second for an hour. Two units is the cut-off.
#[test]
fn compact_shows_hours_with_minutes_and_drops_seconds() {
    assert_eq!(compact(secs(3_600)), "1h");
    assert_eq!(compact(secs(3_660)), "1h 1m");
    assert_eq!(compact(secs(7_530)), "2h 5m", "the trailing 30s is dropped");
    assert_eq!(compact(secs(86_399)), "23h 59m");
}

/// The compact form shows days with hours.
#[test]
fn compact_shows_days_with_hours() {
    assert_eq!(compact(secs(86_400)), "1d");
    assert_eq!(compact(secs(90_000)), "1d 1h");
    assert_eq!(compact(secs(90_061)), "1d 1h", "minutes and seconds are dropped");
    assert_eq!(compact(secs(273_600)), "3d 4h");
}

/// A zero middle unit is dropped rather than printed.
///
/// `1h 0m` reads as a formatting accident and `1h 30s` reads as a gap in the
/// middle of a number. When the second unit is zero the label is one unit.
#[test]
fn a_zero_second_unit_is_dropped_entirely() {
    assert_eq!(compact(secs(3_601)), "1h", "not \"1h 0m\" and not \"1h 1s\"");
    assert_eq!(compact(secs(3_630)), "1h", "not \"1h 30s\"");
    assert_eq!(compact(secs(86_460)), "1d", "not \"1d 0h\" and not \"1d 1m\"");
    assert_eq!(compact(secs(172_800)), "2d");
}

/// The terse form is always exactly one unit.
///
/// The settled-session label and the narrow-column fallback. A stable width
/// matters more than the second unit once a session has stopped moving.
#[test]
fn terse_is_always_one_unit() {
    assert_eq!(terse(secs(12)), "12s");
    assert_eq!(terse(secs(252)), "4m");
    assert_eq!(terse(secs(3_599)), "59m");
    assert_eq!(terse(secs(7_530)), "2h");
    assert_eq!(terse(secs(86_399)), "23h");
    assert_eq!(terse(secs(273_600)), "3d");
    assert_eq!(terse(secs(31_536_000)), "365d");
}

/// The terse form matches the compact form's leading unit exactly.
///
/// If the two disagreed, a row would appear to jump backwards when it switched
/// from the running label to the settled one.
#[test]
fn terse_matches_the_leading_unit_of_compact() {
    for total in [0u64, 1, 59, 60, 61, 3_599, 3_600, 86_399, 86_400, 273_600] {
        let compact_label = compact(secs(total));
        let leading = compact_label.split(' ').next().unwrap_or_default();
        assert_eq!(terse(secs(total)), leading, "at {total}s");
    }
}

/// The clock form keeps a stable shape as the value grows.
///
/// A right-aligned timer column that changed width every minute would shuffle
/// the whole column. Minutes and seconds are always two digits.
#[test]
fn clock_keeps_a_stable_shape() {
    assert_eq!(clock(secs(0)), "00:00");
    assert_eq!(clock(secs(9)), "00:09");
    assert_eq!(clock(secs(252)), "04:12");
    assert_eq!(clock(secs(3_599)), "59:59");
    assert_eq!(clock(secs(3_600)), "1:00:00");
    assert_eq!(clock(secs(3_725)), "1:02:05");
    assert_eq!(clock(secs(86_399)), "23:59:59");
}

/// Past a day the clock form prefixes the day count rather than growing hours.
///
/// `27:00:00` is arithmetically fine and unreadable; `1d 03:00` is not.
#[test]
fn clock_prefixes_days_rather_than_overflowing_hours() {
    assert_eq!(clock(secs(86_400)), "1d 00:00");
    assert_eq!(clock(secs(97_200)), "1d 03:00");
    assert_eq!(clock(secs(273_600)), "3d 04:00");
}

/// The decomposition puts each remainder in the right unit.
///
/// Everything else reads these four numbers. A modulus applied to the wrong
/// total (minutes taken from the daily remainder instead of the hourly one) is
/// invisible for small values and wrong for large ones.
#[test]
fn the_decomposition_is_exact() {
    assert_eq!(
        Parts::new(secs(90_061)),
        Parts {
            days: 1,
            hours: 1,
            minutes: 1,
            seconds: 1
        }
    );
    assert_eq!(
        Parts::new(secs(86_399)),
        Parts {
            days: 0,
            hours: 23,
            minutes: 59,
            seconds: 59
        }
    );
    assert_eq!(
        Parts::new(Duration::from_millis(1_999)),
        Parts {
            days: 0,
            hours: 0,
            minutes: 0,
            seconds: 1
        },
        "milliseconds are floored away"
    );
}

/// An enormous duration formats without panicking or wrapping.
///
/// `Duration::MAX` is about 584 billion years. It cannot arrive from a healthy
/// clock, but it can arrive from a corrupt one, and a panic in a render thread
/// is not an acceptable response to a bad number.
#[test]
fn an_enormous_duration_does_not_panic() {
    let label = compact(Duration::MAX);
    assert!(label.ends_with('h') || label.ends_with('d'), "got {label}");
    assert_eq!(terse(Duration::MAX), "213503982334601d");
    assert_eq!(compact(Duration::MAX), "213503982334601d 7h");
}
