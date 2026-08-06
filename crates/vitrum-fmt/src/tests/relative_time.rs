//! Every threshold in the relative-timestamp table, asserted on both sides of
//! the boundary and to the millisecond where the boundary is a millisecond.

use crate::time::{ABSOLUTE_AFTER_SECS, JUST_NOW_SECS, TimeFormat, Timestamp};

/// 2023-11-14T22:13:20Z. A fixed, unremarkable instant: nothing here depends on
/// a leap second, a year boundary, or a daylight-saving transition.
const NOW_SECS: i64 = 1_700_000_000;

fn clock() -> TimeFormat {
    TimeFormat::utc(Timestamp::from_secs(NOW_SECS))
}

/// Relative label for an instant `secs` seconds before `NOW_SECS`.
fn ago(secs: i64) -> String {
    clock().relative(Timestamp::from_secs(NOW_SECS - secs))
}

/// Relative label for an instant `millis` milliseconds before `NOW_SECS`.
fn ago_millis(millis: i64) -> String {
    clock().relative(Timestamp::from_millis(NOW_SECS * 1_000 - millis))
}

/// Zero elapsed time reads `just now`.
///
/// A session created this instant must not render `0s`, which looks like a
/// stalled counter rather than a fresh row.
#[test]
fn zero_elapsed_is_just_now() {
    assert_eq!(ago(0), "just now");
    assert_eq!(ago_millis(1), "just now");
}

/// The `just now` to seconds boundary is exactly five seconds.
///
/// Four seconds is `just now`, five seconds is `5s`. If this drifted, the label
/// would either flicker between two forms within the first tick or sit on
/// `just now` long enough that a user watching a fast agent thinks the clock is
/// frozen.
#[test]
fn just_now_becomes_seconds_at_exactly_five_seconds() {
    assert_eq!(JUST_NOW_SECS, 5);
    assert_eq!(ago(4), "just now");
    assert_eq!(ago(5), "5s");
    assert_eq!(ago_millis(4_999), "just now");
    assert_eq!(ago_millis(5_000), "5s");
}

/// Seconds render as whole seconds all the way to 59.
#[test]
fn seconds_render_up_to_fifty_nine() {
    assert_eq!(ago(5), "5s");
    assert_eq!(ago(12), "12s");
    assert_eq!(ago(30), "30s");
    assert_eq!(ago(59), "59s");
}

/// The seconds to minutes boundary is exactly sixty seconds.
///
/// `60s` must never be printed; it is a minute and a reader counts it as one.
#[test]
fn seconds_become_minutes_at_exactly_sixty_seconds() {
    assert_eq!(ago(59), "59s");
    assert_eq!(ago(60), "1m");
    assert_eq!(ago_millis(59_999), "59s");
    assert_eq!(ago_millis(60_000), "1m");
}

/// Units floor, they do not round.
///
/// At 119 seconds the honest answer is `1m`: rounding to `2m` would claim more
/// time has passed than actually has, and a label that overshoots then corrects
/// itself on the next tick looks like a bug.
#[test]
fn units_floor_rather_than_round() {
    assert_eq!(ago(61), "1m");
    assert_eq!(ago(119), "1m");
    assert_eq!(ago(120), "2m");
    assert_eq!(ago(3_540), "59m");
    assert_eq!(ago(7_199), "1h");
    assert_eq!(ago(172_799), "1d");
}

/// The minutes to hours boundary is exactly 3600 seconds: `59m` then `1h`.
#[test]
fn fifty_nine_minutes_becomes_one_hour_at_exactly_one_hour() {
    assert_eq!(ago(3_599), "59m");
    assert_eq!(ago(3_600), "1h");
    assert_eq!(ago_millis(3_599_999), "59m");
    assert_eq!(ago_millis(3_600_000), "1h");
}

/// The hours to days boundary is exactly 86 400 seconds: `23h` then `1d`.
#[test]
fn twenty_three_hours_becomes_one_day_at_exactly_one_day() {
    assert_eq!(ago(86_399), "23h");
    assert_eq!(ago(86_400), "1d");
    assert_eq!(ago_millis(86_399_999), "23h");
    assert_eq!(ago_millis(86_400_000), "1d");
}

/// Days render up to six, then the label switches to an absolute date.
///
/// Seven days is the point where `7d` stops being easier to read than the date
/// itself, and beyond it the count grows without bound (`143d` tells nobody
/// anything). Asserted on both sides to the second.
#[test]
fn relative_becomes_absolute_at_exactly_seven_days() {
    assert_eq!(ABSOLUTE_AFTER_SECS, 604_800);
    assert_eq!(ago(86_400), "1d");
    assert_eq!(ago(259_200), "3d");
    assert_eq!(ago(604_799), "6d");
    assert_eq!(ago(604_800), "Nov 7", "seven days back from 2023-11-14");
    assert_eq!(ago_millis(604_799_999), "6d");
    assert_eq!(ago_millis(604_800_000), "Nov 7");
}

/// A timestamp in the future reads `just now`, never a negative duration.
///
/// Clock skew between the daemon and the client is real and unavoidable: the
/// daemon stamps an event, the client's clock is a few hundred milliseconds
/// behind, and the event is now in the future. `-1s ago` in a sidebar is a
/// visible defect; treating it as the present is correct and harmless.
#[test]
fn future_timestamps_read_as_just_now() {
    let format = clock();
    assert_eq!(format.relative(Timestamp::from_millis(NOW_SECS * 1_000 + 1)), "just now");
    assert_eq!(format.relative(Timestamp::from_secs(NOW_SECS + 1)), "just now");
    assert_eq!(format.relative(Timestamp::from_secs(NOW_SECS + 86_400)), "just now");
    assert_eq!(
        format.relative(Timestamp::from_secs(NOW_SECS + 315_360_000)),
        "just now",
        "a clock ten years fast still produces a sane label"
    );
}

/// The elapsed duration for a future timestamp is zero, not a wrapped huge
/// value.
///
/// Callers feed this into the duration formatter for "working for X". An
/// unsigned subtraction that underflowed would render `584942417355h`.
#[test]
fn future_elapsed_is_zero_not_wrapped() {
    let format = clock();
    assert_eq!(
        format.elapsed(Timestamp::from_secs(NOW_SECS + 1_000)),
        std::time::Duration::ZERO
    );
    assert_eq!(
        Timestamp::from_secs(0).saturating_since(Timestamp::from_secs(i64::MAX)),
        std::time::Duration::ZERO,
        "the extreme case must saturate, not overflow"
    );
}

/// Extreme timestamps cannot panic or wrap.
///
/// A corrupt or uninitialised timestamp arriving over the wire must degrade to
/// a strange string, never to a panic on the render thread.
#[test]
fn extreme_timestamps_do_not_panic() {
    let format = clock();
    assert_eq!(format.relative(Timestamp::from_millis(i64::MAX)), "just now");
    let ancient = format.relative(Timestamp::from_millis(i64::MIN));
    assert!(
        ancient.contains("292"),
        "a minimal timestamp lands in year -292277022399: got {ancient}"
    );
    assert_eq!(Timestamp::from_secs(i64::MAX).as_millis(), i64::MAX, "saturates");
    assert_eq!(Timestamp::from_secs(i64::MIN).as_millis(), i64::MIN, "saturates");
}

/// `relative_ago` adds a suffix only where one reads naturally.
///
/// `just now ago` and `Nov 7 ago` are both wrong; `4m ago` is right. A single
/// unconditional suffix would produce two of the three.
#[test]
fn ago_suffix_is_added_only_to_counted_units() {
    let format = clock();
    let ago_label = |secs: i64| format.relative_ago(Timestamp::from_secs(NOW_SECS - secs));
    assert_eq!(ago_label(0), "just now");
    assert_eq!(ago_label(4), "just now");
    assert_eq!(ago_label(12), "12s ago");
    assert_eq!(ago_label(240), "4m ago");
    assert_eq!(ago_label(7_200), "2h ago");
    assert_eq!(ago_label(604_799), "6d ago");
    assert_eq!(ago_label(604_800), "Nov 7", "an absolute date takes no suffix");
}

/// The label never moves backwards as time passes.
///
/// Every threshold in one sweep. A label that got shorter as more time elapsed
/// (which a mis-ordered comparison chain produces) would look like the row had
/// just been touched.
#[test]
fn the_whole_threshold_table_in_order() {
    let expected = [
        (0i64, "just now"),
        (4, "just now"),
        (5, "5s"),
        (59, "59s"),
        (60, "1m"),
        (3_599, "59m"),
        (3_600, "1h"),
        (86_399, "23h"),
        (86_400, "1d"),
        (604_799, "6d"),
        (604_800, "Nov 7"),
        (2_592_000, "Oct 15"),
        (31_536_000, "Nov 14, 2022"),
    ];
    for (secs, label) in expected {
        assert_eq!(ago(secs), label, "at {secs}s elapsed");
    }
}

/// A relative label truncated to a narrow column stays inside it.
///
/// The relative forms are short, but an absolute date with a year is eleven
/// columns and a collapsed sidebar can be narrower than that.
#[test]
fn relative_within_respects_a_narrow_budget() {
    use crate::time::relative_within;
    let format = clock();
    let old = Timestamp::from_secs(NOW_SECS - 31_536_000);
    assert_eq!(format.relative(old), "Nov 14, 2022");
    assert_eq!(relative_within(format, old, 12), "Nov 14, 2022");
    assert_eq!(relative_within(format, old, 6), "Nov 1…");
    assert_eq!(relative_within(format, old, 0), "");
}

/// A `SystemTime` converts in both directions from the epoch without panicking.
///
/// `SystemTime::duration_since` returns an error for instants before the epoch,
/// and the obvious `unwrap` there panics on any machine whose clock is unset.
#[test]
fn system_time_converts_across_the_epoch() {
    use std::time::{Duration, UNIX_EPOCH};
    assert_eq!(Timestamp::from_system_time(UNIX_EPOCH), Timestamp::EPOCH);
    assert_eq!(
        Timestamp::from_system_time(UNIX_EPOCH + Duration::from_millis(1_700_000_000_123)),
        Timestamp::from_millis(1_700_000_000_123)
    );
    assert_eq!(
        Timestamp::from_system_time(UNIX_EPOCH - Duration::from_millis(1_500)),
        Timestamp::from_millis(-1_500),
        "a pre-epoch clock must round-trip, not panic"
    );
}

/// Millisecond and second accessors agree, and flooring is toward the past.
///
/// `as_secs` on a negative millisecond count must floor, not truncate toward
/// zero, or an instant one millisecond before the epoch lands on the wrong day.
#[test]
fn timestamp_accessors_floor_toward_the_past() {
    assert_eq!(Timestamp::from_millis(1_500).as_secs(), 1);
    assert_eq!(Timestamp::from_millis(1_000).as_secs(), 1);
    assert_eq!(Timestamp::from_millis(999).as_secs(), 0);
    assert_eq!(Timestamp::from_millis(-1).as_secs(), -1);
    assert_eq!(Timestamp::from_millis(-1_000).as_secs(), -1);
    assert_eq!(Timestamp::from_millis(-1_001).as_secs(), -2);
}

/// The UTC offset changes absolute dates and nothing else.
///
/// Elapsed time is a difference between two instants, so an offset applied to
/// both cancels. Applying it to only one, which is what happens when the
/// conversion helper is reused for the relative path, would shift every
/// relative label by the size of the offset: a session touched a minute ago in
/// Bengaluru would read `5h`.
#[test]
fn the_utc_offset_does_not_affect_relative_labels() {
    let then = Timestamp::from_secs(NOW_SECS - 240);
    for offset in [-43_200, -28_800, -3_600, 0, 3_600, 19_800, 50_400] {
        let format = TimeFormat::new(Timestamp::from_secs(NOW_SECS), offset);
        assert_eq!(format.relative(then), "4m", "at offset {offset}");
        assert_eq!(format.relative_ago(then), "4m ago", "at offset {offset}");
        assert_eq!(
            format.elapsed(then),
            std::time::Duration::from_secs(240),
            "at offset {offset}"
        );
    }
}

/// A `TimeFormat` built for one instant is not affected by later real time.
///
/// The whole crate is pure, so one render tick has to produce one snapshot.
/// If any function reached for the system clock, two labels rendered a
/// microsecond apart could straddle a threshold and disagree.
#[test]
fn the_same_inputs_always_produce_the_same_label() {
    let format = clock();
    let then = Timestamp::from_secs(NOW_SECS - 5);
    let first = format.relative(then);
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(format.relative(then), first);
    assert_eq!(first, "5s");
}
