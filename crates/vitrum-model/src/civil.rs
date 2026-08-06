//! Proleptic Gregorian civil-time arithmetic.
//!
//! The sidebar needs calendar reasoning for exactly one thing: snooze labels.
//! "tomorrow 9:00" is a statement about local calendar days, not about elapsed
//! milliseconds, and the two disagree twice a year. This module supplies the
//! minimum needed to answer "which local day is this instant on" and "what is
//! the instant of 09:00 on the day after this one" without pulling in a date
//! library.
//!
//! # Why day numbers rather than millisecond offsets
//!
//! Adding `86_400_000` to an instant is wrong across a daylight-saving
//! transition: a spring-forward day is 23 hours long, so 23:30 plus 24 hours
//! lands on the day after tomorrow. Every day-level operation here moves a
//! *day number* and then rebuilds the time of day, which is correct regardless
//! of how long the intervening day happened to be.
//!
//! # The offset the caller supplies
//!
//! There is no timezone database here. Callers pass `utc_offset_seconds`, the
//! offset in effect for the user right now. Converting a civil time back to an
//! instant uses that same offset, so a wake time on the far side of a
//! daylight-saving transition is off by the transition delta (normally one
//! hour) in absolute terms. That is stated rather than hidden: the alternative
//! is a bundled tz database, which this crate does not carry.
//!
//! The algorithms are Howard Hinnant's `days_from_civil` / `civil_from_days`,
//! which are exact for the full proleptic Gregorian range and rely only on
//! truncating integer division, matching Rust's `/` on signed integers.

/// Milliseconds in one nominal day. Used only for durations, never for
/// advancing a calendar day.
pub const MS_PER_DAY: i64 = 86_400_000;
/// Milliseconds in one hour.
pub const MS_PER_HOUR: i64 = 3_600_000;
/// Milliseconds in one minute.
pub const MS_PER_MINUTE: i64 = 60_000;

/// Day of the week, Sunday first to match the `%w` convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

impl Weekday {
    /// Weekday of a day number, where day 0 is 1970-01-01 (a Thursday).
    pub fn from_day_number(day_number: i64) -> Self {
        match (day_number + 4).rem_euclid(7) {
            0 => Weekday::Sunday,
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            _ => Weekday::Saturday,
        }
    }

    /// Three-letter English abbreviation, as used in snooze labels.
    pub fn short_name(self) -> &'static str {
        match self {
            Weekday::Sunday => "Sun",
            Weekday::Monday => "Mon",
            Weekday::Tuesday => "Tue",
            Weekday::Wednesday => "Wed",
            Weekday::Thursday => "Thu",
            Weekday::Friday => "Fri",
            Weekday::Saturday => "Sat",
        }
    }

    /// Index with Sunday as 0, matching `Date.getDay()` in the ported source.
    pub fn index(self) -> i64 {
        match self {
            Weekday::Sunday => 0,
            Weekday::Monday => 1,
            Weekday::Tuesday => 2,
            Weekday::Wednesday => 3,
            Weekday::Thursday => 4,
            Weekday::Friday => 5,
            Weekday::Saturday => 6,
        }
    }
}

/// Three-letter English month abbreviation for `month` in `1..=12`.
///
/// Out-of-range input cannot occur from [`Civil`] construction; it returns
/// `"?"` rather than panicking so a corrupt persisted value degrades to a
/// visible oddity instead of taking down the sidebar.
pub fn month_short_name(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "?",
    }
}

/// Days from 1970-01-01 to the civil date `year-month-day`.
pub fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date for a day number, inverse of [`days_from_civil`].
pub fn civil_from_days(day_number: i64) -> (i32, u32, u32) {
    let z = day_number + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((y + i64::from(m <= 2)) as i32, m as u32, d as u32)
}

/// A local wall-clock date and time, carrying the day number it was built from
/// so day arithmetic never has to round-trip through a timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    /// Days since 1970-01-01 in local time.
    pub day_number: i64,
    pub year: i32,
    /// 1-based month.
    pub month: u32,
    /// 1-based day of month.
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

impl Civil {
    /// Local civil time of `unix_ms` under `utc_offset_seconds`.
    pub fn from_unix_ms(unix_ms: i64, utc_offset_seconds: i32) -> Self {
        let local_ms = unix_ms + i64::from(utc_offset_seconds) * 1000;
        let day_number = local_ms.div_euclid(MS_PER_DAY);
        let ms_of_day = local_ms.rem_euclid(MS_PER_DAY);
        let (year, month, day) = civil_from_days(day_number);
        Civil {
            day_number,
            year,
            month,
            day,
            hour: (ms_of_day / MS_PER_HOUR) as u32,
            minute: (ms_of_day % MS_PER_HOUR / MS_PER_MINUTE) as u32,
            second: (ms_of_day % MS_PER_MINUTE / 1000) as u32,
            millis: (ms_of_day % 1000) as u32,
        }
    }

    /// The instant this civil time denotes under `utc_offset_seconds`.
    pub fn to_unix_ms(&self, utc_offset_seconds: i32) -> i64 {
        let ms_of_day = i64::from(self.hour) * MS_PER_HOUR
            + i64::from(self.minute) * MS_PER_MINUTE
            + i64::from(self.second) * 1000
            + i64::from(self.millis);
        self.day_number * MS_PER_DAY + ms_of_day - i64::from(utc_offset_seconds) * 1000
    }

    /// Same calendar day, time replaced. Seconds and milliseconds are zeroed
    /// because every caller here wants a clean "at 9:00" instant.
    pub fn with_time(&self, hour: u32, minute: u32) -> Self {
        Civil {
            hour,
            minute,
            second: 0,
            millis: 0,
            ..*self
        }
    }

    /// Midnight at the start of this local day.
    pub fn start_of_day(&self) -> Self {
        self.with_time(0, 0)
    }

    /// Advance by whole calendar days. This is the DST-safe day step: it moves
    /// the day number and keeps the wall-clock time, so it never lands on the
    /// wrong local date the way a fixed millisecond offset does.
    pub fn add_days(&self, days: i64) -> Self {
        let day_number = self.day_number + days;
        let (year, month, day) = civil_from_days(day_number);
        Civil {
            day_number,
            year,
            month,
            day,
            ..*self
        }
    }

    /// Weekday of this local date.
    pub fn weekday(&self) -> Weekday {
        Weekday::from_day_number(self.day_number)
    }

    /// 24-hour time of day, `"9:00"` / `"17:30"`.
    ///
    /// Deliberately not locale-aware: this crate has no locale data, and a
    /// half-applied locale (English weekday names beside 12-hour times) reads
    /// worse than one consistent format.
    pub fn time_label(&self) -> String {
        format!("{}:{:02}", self.hour, self.minute)
    }

    /// Abbreviated month and day, `"Mar 3"`.
    pub fn date_label(&self) -> String {
        format!("{} {}", month_short_name(self.month), self.day)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks the epoch anchor. If `days_from_civil` or `civil_from_days` drift
    /// by one, every snooze label silently shifts by a day and the "tomorrow"
    /// boundary lands in the wrong place.
    #[test]
    fn epoch_day_is_1970_01_01_thursday() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(Weekday::from_day_number(0), Weekday::Thursday);
    }

    /// Round-trips every day across a leap year and a century boundary. A
    /// mistake in the era arithmetic shows up only on specific dates, so a
    /// single spot check would not catch it.
    #[test]
    fn civil_day_round_trip_is_exact_over_two_centuries() {
        let start = days_from_civil(1899, 1, 1);
        let end = days_from_civil(2101, 1, 1);
        let mut checked = 0u32;
        for day_number in start..end {
            let (y, m, d) = civil_from_days(day_number);
            assert_eq!(
                days_from_civil(y, m, d),
                day_number,
                "round trip failed at {y}-{m}-{d}"
            );
            checked += 1;
        }
        assert_eq!(checked, 73_779);
    }

    /// 2000 is a leap year and 1900 is not. Getting the 100/400 rules backwards
    /// is the classic Gregorian bug and would move every date after it.
    #[test]
    fn century_leap_rules_are_gregorian() {
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 28) + 1), (2000, 2, 29));
        assert_eq!(civil_from_days(days_from_civil(1900, 2, 28) + 1), (1900, 3, 1));
        assert_eq!(days_from_civil(2001, 1, 1) - days_from_civil(2000, 1, 1), 366);
        assert_eq!(days_from_civil(1901, 1, 1) - days_from_civil(1900, 1, 1), 365);
    }

    /// Negative day numbers (pre-1970) must not fall out of the truncating
    /// division. A wrong branch here yields dates off by a full 400-year era.
    #[test]
    fn dates_before_the_epoch_decode_correctly() {
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(days_from_civil(1, 1, 1), -719_162);
        assert_eq!(civil_from_days(-719_162), (1, 1, 1));
        // The 719_468 constant is days to 0000-03-01, the March-based era
        // origin the algorithm counts from, NOT to year 1.
        assert_eq!(civil_from_days(-719_468), (0, 3, 1));
    }

    /// The offset must shift which local day an instant belongs to. Without
    /// this, a user at UTC+13 sees "tomorrow" for something happening today.
    #[test]
    fn utc_offset_moves_the_local_day() {
        // 2026-03-03T23:30:00Z
        let instant = 1_772_580_600_000;
        let utc = Civil::from_unix_ms(instant, 0);
        assert_eq!((utc.year, utc.month, utc.day, utc.hour, utc.minute), (2026, 3, 3, 23, 30));

        let ahead = Civil::from_unix_ms(instant, 3600);
        assert_eq!((ahead.year, ahead.month, ahead.day, ahead.hour, ahead.minute), (2026, 3, 4, 0, 30));

        let behind = Civil::from_unix_ms(instant, -8 * 3600);
        assert_eq!((behind.year, behind.month, behind.day, behind.hour, behind.minute), (2026, 3, 3, 15, 30));
    }

    /// `to_unix_ms` must invert `from_unix_ms` exactly, including sub-second
    /// parts, or snooze wake instants drift every time they round-trip through
    /// a persisted civil value.
    #[test]
    fn unix_round_trip_preserves_milliseconds_at_several_offsets() {
        let instant = 1_772_580_600_123;
        for offset in [0, 3600, -3600, 19_800, -8 * 3600, 13 * 3600] {
            let civil = Civil::from_unix_ms(instant, offset);
            assert_eq!(civil.to_unix_ms(offset), instant, "offset {offset}");
        }
    }

    /// The whole reason day arithmetic exists: adding a day must change the
    /// calendar date by one and keep the wall-clock time, even when a real
    /// timezone would have made that day 23 or 25 hours long.
    #[test]
    fn add_days_keeps_wall_clock_time_across_a_short_day() {
        // 2026-03-08 is the US spring-forward date; 23:30 local.
        let base = Civil::from_unix_ms(days_from_civil(2026, 3, 8) * MS_PER_DAY + 23 * MS_PER_HOUR + 30 * MS_PER_MINUTE, 0);
        let next = base.add_days(1);
        assert_eq!((next.year, next.month, next.day), (2026, 3, 9));
        assert_eq!((next.hour, next.minute), (23, 30));

        // A naive millisecond add from the same point under a 23-hour day
        // would land on the 10th; the day-number step cannot.
        let naive = Civil::from_unix_ms(base.to_unix_ms(0) + MS_PER_DAY - MS_PER_HOUR, 0);
        assert_eq!((naive.year, naive.month, naive.day, naive.hour), (2026, 3, 9, 22));
    }

    /// Day stepping must carry across month and year boundaries, which is where
    /// "in a week" labels land most often.
    #[test]
    fn add_days_crosses_month_and_year_boundaries() {
        let feb28 = Civil::from_unix_ms(days_from_civil(2026, 2, 28) * MS_PER_DAY, 0);
        assert_eq!(
            {
                let n = feb28.add_days(1);
                (n.year, n.month, n.day)
            },
            (2026, 3, 1)
        );

        let dec31 = Civil::from_unix_ms(days_from_civil(2026, 12, 31) * MS_PER_DAY, 0);
        assert_eq!(
            {
                let n = dec31.add_days(1);
                (n.year, n.month, n.day)
            },
            (2027, 1, 1)
        );

        let leap = Civil::from_unix_ms(days_from_civil(2028, 2, 28) * MS_PER_DAY, 0);
        assert_eq!(
            {
                let n = leap.add_days(1);
                (n.year, n.month, n.day)
            },
            (2028, 2, 29)
        );
    }

    /// Weekday names feed the "Mon 9:00" snooze label directly, so an off-by-one
    /// would print the wrong day for every within-week snooze.
    #[test]
    fn weekday_names_match_known_dates() {
        let cases = [
            (days_from_civil(2026, 3, 1), Weekday::Sunday, "Sun"),
            (days_from_civil(2026, 3, 2), Weekday::Monday, "Mon"),
            (days_from_civil(2026, 3, 7), Weekday::Saturday, "Sat"),
            (days_from_civil(1969, 12, 31), Weekday::Wednesday, "Wed"),
        ];
        for (day_number, weekday, name) in cases {
            assert_eq!(Weekday::from_day_number(day_number), weekday);
            assert_eq!(Weekday::from_day_number(day_number).short_name(), name);
        }
        assert_eq!(Weekday::Sunday.index(), 0);
        assert_eq!(Weekday::Saturday.index(), 6);
    }

    /// Labels are user-visible strings; pinning them stops a formatting tweak
    /// from silently changing what the snooze menu reads.
    #[test]
    fn labels_render_without_zero_padding_the_hour() {
        let morning = Civil::from_unix_ms(days_from_civil(2026, 3, 3) * MS_PER_DAY + 9 * MS_PER_HOUR, 0);
        assert_eq!(morning.time_label(), "9:00");
        assert_eq!(morning.date_label(), "Mar 3");

        let evening = morning.with_time(17, 5);
        assert_eq!(evening.time_label(), "17:05");

        let midnight = morning.start_of_day();
        assert_eq!(midnight.time_label(), "0:00");
        assert_eq!((midnight.hour, midnight.minute, midnight.second, midnight.millis), (0, 0, 0, 0));
    }

    /// `with_time` must clear seconds and milliseconds, otherwise a preset
    /// wake time inherits a stray 37.482s from "now" and two presets computed a
    /// second apart produce different instants for the same nominal hour.
    #[test]
    fn with_time_zeroes_sub_minute_components() {
        let messy = Civil::from_unix_ms(1_772_580_637_482, 0);
        assert_eq!((messy.second, messy.millis), (37, 482));
        let clean = messy.with_time(9, 0);
        assert_eq!((clean.hour, clean.minute, clean.second, clean.millis), (9, 0, 0, 0));
        assert_eq!(clean.day_number, messy.day_number);
    }

    /// A month index outside 1..=12 can only arrive from corrupt persisted
    /// state; it must render, not panic.
    #[test]
    fn month_name_of_out_of_range_index_is_inert() {
        assert_eq!(month_short_name(0), "?");
        assert_eq!(month_short_name(13), "?");
        assert_eq!(month_short_name(12), "Dec");
    }
}
