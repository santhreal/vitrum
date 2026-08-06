//! The calendar conversion behind `Mar 3`, and what a UTC offset does to it.
//!
//! Every expected date in this module was produced by `date -u -d @<seconds>`,
//! not by the code under test.

use crate::time::{TimeFormat, Timestamp};

/// 2023-11-14T22:13:20Z.
const NOW_SECS: i64 = 1_700_000_000;

fn utc() -> TimeFormat {
    TimeFormat::utc(Timestamp::from_secs(NOW_SECS))
}

fn date_at(secs: i64) -> String {
    utc().absolute_date(Timestamp::from_secs(secs))
}

/// A date in the current year omits the year.
///
/// Repeating `2023` on every row of a sidebar costs six columns and tells the
/// reader nothing they do not already know.
#[test]
fn a_date_in_the_current_year_omits_the_year() {
    assert_eq!(date_at(1_677_801_600), "Mar 3", "2023-03-03T00:00:00Z");
    assert_eq!(date_at(1_672_531_200), "Jan 1", "2023-01-01T00:00:00Z");
}

/// A date in another year carries the year.
///
/// Without it, a session last touched in March 2022 is indistinguishable from
/// one touched last March, and a user would restore the wrong one.
#[test]
fn a_date_in_another_year_carries_the_year() {
    assert_eq!(date_at(1_709_424_000), "Mar 3, 2024", "2024-03-03T00:00:00Z");
    assert_eq!(date_at(0), "Jan 1, 1970", "the epoch itself");
}

/// A pre-epoch instant lands on the previous day, not on 1 January 1970.
///
/// Rust's integer division truncates toward zero, so `-1 / 86400 == 0` and a
/// naive conversion puts one millisecond before the epoch on the epoch's own
/// day. The conversion must floor toward the past.
#[test]
fn pre_epoch_instants_floor_to_the_previous_day() {
    assert_eq!(date_at(-1), "Dec 31, 1969");
    assert_eq!(date_at(-86_400), "Dec 31, 1969");
    assert_eq!(date_at(-86_401), "Dec 30, 1969");
    assert_eq!(
        utc().absolute_date(Timestamp::from_millis(-1)),
        "Dec 31, 1969",
        "one millisecond before the epoch"
    );
}

/// 2024 is a leap year and 29 February exists.
#[test]
fn leap_day_resolves_correctly() {
    assert_eq!(date_at(1_709_208_000), "Feb 29, 2024", "2024-02-29T12:00:00Z");
}

/// 1900 was not a leap year and 2000 was.
///
/// The century rule is where every hand-rolled calendar breaks: divisible by
/// 100 is not a leap year unless also divisible by 400. Getting it wrong shifts
/// every date on one side of the boundary by a day.
#[test]
fn the_century_leap_rule_holds() {
    assert_eq!(date_at(-2_203_934_400), "Feb 28, 1900", "1900-02-28T12:00:00Z");
    assert_eq!(
        date_at(-2_203_848_000),
        "Mar 1, 1900",
        "the next day is March, so 1900 had no 29 February"
    );
    assert_eq!(date_at(951_825_600), "Feb 29, 2000", "2000 was a leap year");
    assert_eq!(date_at(4_107_585_600), "Mar 1, 2100", "2100-03-01T12:00:00Z");
}

/// Every month name renders correctly.
///
/// The month table is indexed by a value the conversion computes with a shifted
/// March-first year, so an off-by-one in the un-shift would silently rename ten
/// of the twelve months.
#[test]
fn every_month_name_is_correct() {
    let firsts = [
        (1_672_531_200i64, "Jan 1"),
        (1_675_209_600, "Feb 1"),
        (1_677_628_800, "Mar 1"),
        (1_680_307_200, "Apr 1"),
        (1_682_899_200, "May 1"),
        (1_685_577_600, "Jun 1"),
        (1_688_169_600, "Jul 1"),
        (1_690_848_000, "Aug 1"),
        (1_693_526_400, "Sep 1"),
        (1_696_118_400, "Oct 1"),
        (1_698_796_800, "Nov 1"),
        (1_701_388_800, "Dec 1"),
    ];
    for (secs, expected) in firsts {
        assert_eq!(date_at(secs), expected, "at {secs}");
    }
}

/// A positive UTC offset can push an instant onto the next calendar day.
///
/// 2023-03-03T23:30Z is 4 March in India. Rendering it as 3 March to a user in
/// Bengaluru means the sidebar disagrees with their wall clock.
#[test]
fn a_positive_offset_can_advance_the_date() {
    let instant = Timestamp::from_secs(1_677_886_200); // 2023-03-03T23:30:00Z
    let india = TimeFormat::new(Timestamp::from_secs(NOW_SECS), 19_800);
    assert_eq!(india.absolute_date(instant), "Mar 4");
    assert_eq!(utc().absolute_date(instant), "Mar 3");
}

/// A negative UTC offset can push an instant onto the previous calendar day.
#[test]
fn a_negative_offset_can_retreat_the_date() {
    let instant = Timestamp::from_secs(1_677_801_600); // 2023-03-03T00:00:00Z
    let pacific = TimeFormat::new(Timestamp::from_secs(NOW_SECS), -28_800);
    assert_eq!(pacific.absolute_date(instant), "Mar 2");
    assert_eq!(utc().absolute_date(instant), "Mar 3");
}

/// An offset that crosses a year boundary changes whether the year is shown.
///
/// 2023-12-31T23:00Z is already 2024 in Helsinki, so the label must gain a year
/// even though the same instant in UTC would not.
#[test]
fn an_offset_can_cross_the_year_boundary() {
    let instant = Timestamp::from_secs(1_704_063_600); // 2023-12-31T23:00:00Z
    assert_eq!(utc().absolute_date(instant), "Dec 31");
    let helsinki = TimeFormat::new(Timestamp::from_secs(NOW_SECS), 7_200);
    assert_eq!(helsinki.absolute_date(instant), "Jan 1, 2024");
}

/// The year comparison uses the local year of `now`, not the UTC year.
///
/// If `now` were compared in UTC while the timestamp were converted locally,
/// the year would appear and disappear for instants near midnight on 31
/// December depending on which side of the offset each one fell.
#[test]
fn the_current_year_is_also_taken_in_local_time() {
    // now = 2023-12-31T23:00:00Z, which is already 2024 in Helsinki.
    let now = Timestamp::from_secs(1_704_063_600);
    let helsinki = TimeFormat::new(now, 7_200);
    assert_eq!(helsinki.absolute_date(now), "Jan 1", "same local year as now");
    assert_eq!(
        helsinki.absolute_date(Timestamp::from_secs(1_677_886_200)),
        "Mar 4, 2023",
        "March 2023 is a different local year from January 2024"
    );
}

/// The datetime form shows a zero-padded 24-hour clock.
///
/// A tooltip has to be unambiguous, so no AM/PM and no single-digit hours that
/// could be misread against a padded neighbour.
#[test]
fn the_datetime_form_is_zero_padded_and_twenty_four_hour() {
    assert_eq!(
        utc().absolute_datetime(Timestamp::from_secs(1_677_852_300)),
        "Mar 3 14:05",
        "2023-03-03T14:05:00Z"
    );
    assert_eq!(
        utc().absolute_datetime(Timestamp::from_secs(1_677_801_600)),
        "Mar 3 00:00",
        "midnight is 00:00, not 24:00 of the previous day"
    );
}

/// The datetime form's time of day is also computed in local time.
///
/// A negative offset that pushes the instant into the previous day must move
/// the clock and the date together; computing one locally and the other in UTC
/// produces a label that is internally inconsistent.
#[test]
fn the_datetime_form_moves_date_and_clock_together() {
    let instant = Timestamp::from_secs(1_677_801_600); // 2023-03-03T00:00:00Z
    let one_hour_west = TimeFormat::new(Timestamp::from_secs(NOW_SECS), -3_600);
    assert_eq!(one_hour_west.absolute_datetime(instant), "Mar 2 23:00");
}

/// The formatter reports back the clock and offset it was built with.
///
/// Callers cache a `TimeFormat` for a render pass and need to compare it
/// against the next tick to decide whether anything must be re-rendered.
#[test]
fn the_formatter_exposes_its_own_clock() {
    let format = TimeFormat::new(Timestamp::from_secs(NOW_SECS), 19_800);
    assert_eq!(format.now(), Timestamp::from_secs(NOW_SECS));
    assert_eq!(format.utc_offset_secs(), 19_800);
    assert_eq!(TimeFormat::utc(Timestamp::EPOCH).utc_offset_secs(), 0);
}
