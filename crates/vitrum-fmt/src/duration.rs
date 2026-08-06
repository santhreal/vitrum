//! Elapsed-time labels for "working for X".
//!
//! Two shapes, from the same decomposition:
//!
//! - [`compact`] shows the two most significant adjacent units: `4m 12s`,
//!   `2h 5m`, `3d 4h`. This is the running label on an active session, where
//!   the second unit is what makes a slow agent visibly slow.
//! - [`terse`] shows one unit: `4m`, `2h`, `3d`. This is the settled label and
//!   the narrow-column fallback, where a stable width matters more than
//!   precision.
//!
//! # Unit dropping
//!
//! `compact` emits the largest non-zero unit and then the next smaller unit
//! *only if it is non-zero*. So `1h 0m 30s` renders `1h`, not `1h 0m` and not
//! `1h 30s`: a gap in the middle of a duration reads as a typo, and 30 seconds
//! is noise next to an hour. Units below the second-most-significant are always
//! dropped, because a label that ticks in the last place of `3d 4h 12m 7s` is
//! unreadable and forces a re-render every second for no information.
//!
//! Sub-second durations floor to `0s`. A zero duration is a real state (a
//! session that has just started) and renders `0s`, never an empty string.

use std::time::Duration;

const SECS_PER_MIN: u64 = 60;
const SECS_PER_HOUR: u64 = 60 * SECS_PER_MIN;
const SECS_PER_DAY: u64 = 24 * SECS_PER_HOUR;

/// A duration split into whole days, hours, minutes, and seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts {
    /// Whole days.
    pub days: u64,
    /// Hours after days are removed, `0..=23`.
    pub hours: u64,
    /// Minutes after hours are removed, `0..=59`.
    pub minutes: u64,
    /// Seconds after minutes are removed, `0..=59`.
    pub seconds: u64,
}

impl Parts {
    /// Decompose a duration, flooring to whole seconds.
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        let total = duration.as_secs();
        Self {
            days: total / SECS_PER_DAY,
            hours: (total % SECS_PER_DAY) / SECS_PER_HOUR,
            minutes: (total % SECS_PER_HOUR) / SECS_PER_MIN,
            seconds: total % SECS_PER_MIN,
        }
    }

    /// The four units, largest first, paired with their suffix.
    const fn ordered(self) -> [(u64, char); 4] {
        [
            (self.days, 'd'),
            (self.hours, 'h'),
            (self.minutes, 'm'),
            (self.seconds, 's'),
        ]
    }
}

/// Two-unit label: `0s`, `12s`, `4m 12s`, `2h 5m`, `3d 4h`.
#[must_use]
pub fn compact(duration: Duration) -> String {
    let units = Parts::new(duration).ordered();
    let Some(lead) = units.iter().position(|&(value, _)| value > 0) else {
        return "0s".to_owned();
    };

    let (value, suffix) = units[lead];
    match units.get(lead + 1) {
        Some(&(next_value, next_suffix)) if next_value > 0 => {
            format!("{value}{suffix} {next_value}{next_suffix}")
        }
        _ => format!("{value}{suffix}"),
    }
}

/// One-unit label: `0s`, `12s`, `4m`, `2h`, `3d`.
#[must_use]
pub fn terse(duration: Duration) -> String {
    let units = Parts::new(duration).ordered();
    match units.iter().find(|&&(value, _)| value > 0) {
        Some(&(value, suffix)) => format!("{value}{suffix}"),
        None => "0s".to_owned(),
    }
}

/// Fixed-width clock form for a session timer: `04:12`, `1:02:05`, `3d 04:12`.
///
/// Digits do not change width as the value grows, so a right-aligned timer
/// column does not shuffle every minute. Minutes and seconds are always two
/// digits; hours are only shown once there is an hour, and days are prefixed
/// separately because `27:00:00` reads worse than `1d 03:00`.
#[must_use]
pub fn clock(duration: Duration) -> String {
    let parts = Parts::new(duration);
    let Parts {
        days,
        hours,
        minutes,
        seconds,
    } = parts;
    if days > 0 {
        format!("{days}d {hours:02}:{minutes:02}")
    } else if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}
