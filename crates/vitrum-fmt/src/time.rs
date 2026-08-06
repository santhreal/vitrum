//! Relative timestamps: `just now`, `12s`, `4m`, `2h`, `3d`, `Mar 3`.
//!
//! # Why the clock is a parameter
//!
//! Nothing here reads the system clock. `now` is passed in, so every function
//! is pure: the same inputs always produce the same string, boundaries can be
//! asserted to the millisecond, and a UI that ticks once per second renders one
//! consistent snapshot instead of a set of labels each sampled a few
//! microseconds apart.
//!
//! The UTC offset is a parameter for the same reason. Resolving the local zone
//! is an OS query, it can change under a running process, and `time`-style
//! crates refuse to do it soundly from a multi-threaded program. The caller
//! resolves it once and hands it down.
//!
//! # Threshold table
//!
//! `elapsed = now - then`, floored to whole seconds, clamped at zero.
//!
//! | elapsed                       | output          |
//! |-------------------------------|-----------------|
//! | `then` is in the future        | `just now`      |
//! | `0s ..= 4s`                    | `just now`      |
//! | `5s ..= 59s`                   | `5s` .. `59s`   |
//! | `60s ..= 3599s`                | `1m` .. `59m`   |
//! | `3600s ..= 86_399s`            | `1h` .. `23h`   |
//! | `86_400s ..= 604_799s`         | `1d` .. `6d`    |
//! | `>= 604_800s` (7 days)         | `Mar 3`         |
//!
//! Every unit **floors**: at 119 seconds the answer is `1m`, never `2m`. A
//! floored label never claims more time has passed than actually has, and it
//! only ever moves forward, so a row cannot appear to get younger between two
//! renders.
//!
//! Beyond a week the absolute local date is shown, and the year is appended
//! (`Mar 3, 2024`) whenever it differs from the local year of `now`, so a
//! stale session from a previous year can never be mistaken for a recent one.

use crate::text;

const MILLIS_PER_SEC: i64 = 1_000;
const SECS_PER_MIN: i64 = 60;
const SECS_PER_HOUR: i64 = 60 * SECS_PER_MIN;
const SECS_PER_DAY: i64 = 24 * SECS_PER_HOUR;

/// Elapsed seconds below which the label is `just now` rather than `Ns`.
pub const JUST_NOW_SECS: i64 = 5;
/// Elapsed seconds at or above which the absolute date replaces `Nd`.
pub const ABSOLUTE_AFTER_SECS: i64 = 7 * SECS_PER_DAY;

/// A wall-clock instant, milliseconds since the Unix epoch, signed.
///
/// Signed because a bad or unset clock really does report pre-1970 times, and a
/// panic in a sidebar label is a worse outcome than a strange date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp {
    millis: i64,
}

impl Timestamp {
    /// 1970-01-01T00:00:00Z.
    pub const EPOCH: Self = Self { millis: 0 };

    /// Build from milliseconds since the Unix epoch.
    #[must_use]
    pub const fn from_millis(millis: i64) -> Self {
        Self { millis }
    }

    /// Build from whole seconds since the Unix epoch.
    ///
    /// Saturates instead of overflowing; a garbage value yields a clamped date,
    /// not a panic.
    #[must_use]
    pub const fn from_secs(secs: i64) -> Self {
        Self {
            millis: secs.saturating_mul(MILLIS_PER_SEC),
        }
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.millis
    }

    /// Whole seconds since the Unix epoch, floored toward negative infinity.
    #[must_use]
    pub const fn as_secs(self) -> i64 {
        floor_div(self.millis, MILLIS_PER_SEC)
    }

    /// Convert from a [`std::time::SystemTime`].
    ///
    /// Handles both directions from the epoch and saturates rather than
    /// panicking, because `SystemTime` arithmetic on a machine whose clock was
    /// just stepped backwards is a real occurrence.
    #[must_use]
    pub fn from_system_time(time: std::time::SystemTime) -> Self {
        match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(delta) => Self::from_millis(i64::try_from(delta.as_millis()).unwrap_or(i64::MAX)),
            Err(err) => {
                let before = err.duration().as_millis();
                Self::from_millis(i64::try_from(before).map_or(i64::MIN, i64::wrapping_neg))
            }
        }
    }

    /// Time elapsed from `earlier` to `self`, clamped at zero.
    ///
    /// A clock that moved backwards, or an event stamped by a daemon whose
    /// clock runs a little ahead of the client's, yields zero rather than a
    /// negative duration that would render as `-3m`.
    #[must_use]
    pub const fn saturating_since(self, earlier: Self) -> std::time::Duration {
        let delta = self.millis.saturating_sub(earlier.millis);
        if delta <= 0 {
            std::time::Duration::ZERO
        } else {
            std::time::Duration::from_millis(delta as u64)
        }
    }
}

/// A frozen clock plus a UTC offset: everything needed to render a timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeFormat {
    now: Timestamp,
    utc_offset_secs: i32,
}

impl TimeFormat {
    /// `now` is the instant every label is measured against; `utc_offset_secs`
    /// is seconds to add to UTC to reach the display zone (`+19800` for IST,
    /// `-28800` for PST).
    #[must_use]
    pub const fn new(now: Timestamp, utc_offset_secs: i32) -> Self {
        Self {
            now,
            utc_offset_secs,
        }
    }

    /// A formatter that renders absolute dates in UTC.
    #[must_use]
    pub const fn utc(now: Timestamp) -> Self {
        Self::new(now, 0)
    }

    /// The instant this formatter measures against.
    #[must_use]
    pub const fn now(self) -> Timestamp {
        self.now
    }

    /// Seconds to add to UTC to reach the display zone.
    #[must_use]
    pub const fn utc_offset_secs(self) -> i32 {
        self.utc_offset_secs
    }

    /// Elapsed time from `then` to `now`, clamped at zero.
    #[must_use]
    pub const fn elapsed(self, then: Timestamp) -> std::time::Duration {
        self.now.saturating_since(then)
    }

    /// The relative label. See the [module table](self) for exact thresholds.
    #[must_use]
    pub fn relative(self, then: Timestamp) -> String {
        let secs = self.elapsed(then).as_secs() as i64;

        if secs < JUST_NOW_SECS {
            return "just now".to_owned();
        }
        if secs < SECS_PER_MIN {
            return format!("{secs}s");
        }
        if secs < SECS_PER_HOUR {
            return format!("{}m", secs / SECS_PER_MIN);
        }
        if secs < SECS_PER_DAY {
            return format!("{}h", secs / SECS_PER_HOUR);
        }
        if secs < ABSOLUTE_AFTER_SECS {
            return format!("{}d", secs / SECS_PER_DAY);
        }
        self.absolute_date(then)
    }

    /// The relative label with an `ago` suffix where one reads naturally.
    ///
    /// `just now` and absolute dates take no suffix; `4m` becomes `4m ago`.
    #[must_use]
    pub fn relative_ago(self, then: Timestamp) -> String {
        let secs = self.elapsed(then).as_secs() as i64;
        if secs < JUST_NOW_SECS {
            return "just now".to_owned();
        }
        if secs >= ABSOLUTE_AFTER_SECS {
            return self.absolute_date(then);
        }
        self.relative(then) + " ago"
    }

    /// The local calendar date: `Mar 3`, or `Mar 3, 2024` in another year.
    #[must_use]
    pub fn absolute_date(self, then: Timestamp) -> String {
        let (year, month, day) = self.civil(then);
        let (now_year, _, _) = self.civil(self.now);
        let name = MONTH_NAMES[(month - 1) as usize];
        if year == now_year {
            format!("{name} {day}")
        } else {
            format!("{name} {day}, {year}")
        }
    }

    /// The local calendar date and 24-hour wall clock: `Mar 3 14:05`.
    ///
    /// For tooltips, where the exact instant matters more than brevity.
    #[must_use]
    pub fn absolute_datetime(self, then: Timestamp) -> String {
        let local = then.as_secs().saturating_add(i64::from(self.utc_offset_secs));
        let secs_of_day = local - floor_div(local, SECS_PER_DAY) * SECS_PER_DAY;
        let hour = secs_of_day / SECS_PER_HOUR;
        let minute = (secs_of_day % SECS_PER_HOUR) / SECS_PER_MIN;
        format!("{} {hour:02}:{minute:02}", self.absolute_date(then))
    }

    fn civil(self, at: Timestamp) -> (i64, u32, u32) {
        let local = at.as_secs().saturating_add(i64::from(self.utc_offset_secs));
        civil_from_days(floor_div(local, SECS_PER_DAY))
    }
}

/// Relative label truncated to a column budget.
///
/// Relative labels are short by construction, but an absolute date in a very
/// narrow sidebar still needs a bound.
#[must_use]
pub fn relative_within(format: TimeFormat, then: Timestamp, budget: usize) -> String {
    text::truncate_end(&format.relative(then), budget)
}

const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Floor division: `-1 / 1000 == 0` in Rust but we need `-1`.
///
/// Truncating division would put the instant one millisecond before the epoch
/// on 1970-01-01 instead of 1969-12-31.
const fn floor_div(lhs: i64, rhs: i64) -> i64 {
    let quotient = lhs / rhs;
    if lhs % rhs != 0 && ((lhs < 0) != (rhs < 0)) {
        quotient - 1
    } else {
        quotient
    }
}

/// Days since 1970-01-01 to a proleptic Gregorian `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, which is exact for the whole range of
/// `i64` days and needs no lookup tables, no leap-year branches, and no
/// dependency. The era arithmetic is shifted so that a 400-year era starts on
/// 1 March, which puts the leap day at the end of the cycle and removes every
/// special case.
const fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = (if shifted >= 0 { shifted } else { shifted - 146_096 }) / 146_097;
    let day_of_era = shifted - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11], March = 0
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32; // [1, 31]
    let month = (if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    }) as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}
