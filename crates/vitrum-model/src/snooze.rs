//! Snoozing a session until a wake instant, and the labels that describe it.
//!
//! Ported from T3 Code's `snoozeWakeDescription` (menu and toast text) and
//! `snoozeWakeLabel` / `resolveSnoozePresets` (the compact row badge and the
//! preset menu). A snoozed session is settled until it wakes: it drops out of
//! the active list without being closed, which is the only way a twenty-session
//! sidebar stays readable.
//!
//! Two labels, because they answer different questions:
//!
//! - [`wake_description`] is absolute: *"tomorrow 9:00"*. It goes in menus and
//!   confirmations, where you are choosing or reviewing a wake time.
//! - [`wake_countdown_label`] is relative: *"18h"*. It goes on the row, where
//!   you want the delay at a glance and the calendar date is noise.
//!
//! All calendar arithmetic goes through [`crate::civil`], never through adding
//! `86_400_000`. See that module for what the caller-supplied UTC offset can
//! and cannot express.

use serde::{Deserialize, Serialize};

use crate::civil::{Civil, MS_PER_DAY, MS_PER_HOUR, MS_PER_MINUTE};
use crate::view::Clock;

/// Hour of day the evening preset wakes at.
pub const EVENING_HOUR: u32 = 18;
/// Hour of day the morning presets wake at.
pub const MORNING_HOUR: u32 = 9;

/// A session parked until a wake instant.
///
/// `snoozed_at_ms` is kept because it is the explicit settle stamp: it is what
/// the settled list sorts by, mirroring T3 Code's `settledAt` taking precedence
/// over inferred activity times. Without it, snoozing a week-old session would
/// leave it buried at the bottom of the settled pile instead of at the top
/// where you just put it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snooze {
    /// When the operator snoozed it.
    pub snoozed_at_ms: u64,
    /// When it should return to the active list.
    pub wake_at_ms: u64,
}

impl Snooze {
    /// True while the wake instant is still in the future.
    ///
    /// Strictly `now < wake`, so a snooze wakes at its own instant rather than
    /// one tick later.
    pub fn is_asleep(&self, now_ms: u64) -> bool {
        now_ms < self.wake_at_ms
    }

    /// Milliseconds until wake, saturating at zero once woken.
    pub fn remaining_ms(&self, now_ms: u64) -> u64 {
        self.wake_at_ms.saturating_sub(now_ms)
    }
}

/// Absolute wake time in words: `"17:30"`, `"tomorrow 9:00"`, `"Mon 9:00"`,
/// `"Mar 3, 9:00"`.
///
/// The bands are calendar-day deltas from the start of the operator's today,
/// not elapsed hours. Snoozing at 23:50 until 00:10 reads "tomorrow 0:10", not
/// "0:10", because those are different days on the wall clock even though only
/// twenty minutes separate them.
///
/// Bands:
///
/// - same day: time only
/// - one day ahead: `tomorrow HH:MM`
/// - two to six days ahead: `Wed HH:MM`, since a weekday name is unambiguous
///   inside one week and shorter than a date
/// - seven or more days ahead: `Mon D, HH:MM`
/// - already in the past: the full date form
///
/// The past case diverges from T3 Code, which falls into its weekday branch for
/// negative deltas and prints a weekday name that reads as a future day. A wake
/// time in the past means the snooze has lapsed and the operator is looking at
/// history, so the unambiguous date is the right answer.
pub fn wake_description(wake_at_ms: u64, clock: Clock) -> String {
    let wake = Civil::from_unix_ms(wake_at_ms as i64, clock.utc_offset_seconds);
    let today = Civil::from_unix_ms(clock.now_ms as i64, clock.utc_offset_seconds);
    let time = wake.time_label();

    match wake.day_number - today.day_number {
        0 => time,
        1 => format!("tomorrow {time}"),
        2..=6 => format!("{} {time}", wake.weekday().short_name()),
        _ => format!("{}, {time}", wake.date_label()),
    }
}

/// Compact time-until-wake for the row badge: `"1m"`, `"30m"`, `"2h"`, `"3d"`.
///
/// Rounds up so a live snooze never reads `"0m"` while the row is still hidden,
/// and reports `"now"` once the wake instant has passed. Ported unit-for-unit
/// from T3 Code so a wake time never reads differently between surfaces.
pub fn wake_countdown_label(wake_at_ms: u64, now_ms: u64) -> String {
    let remaining = wake_at_ms.saturating_sub(now_ms);
    if remaining == 0 {
        return "now".to_string();
    }
    let remaining = remaining as i64;
    if remaining < MS_PER_HOUR {
        return format!("{}m", div_ceil(remaining, MS_PER_MINUTE).max(1));
    }
    if remaining < MS_PER_DAY {
        return format!("{}h", div_ceil(remaining, MS_PER_HOUR));
    }
    format!("{}d", div_ceil(remaining, MS_PER_DAY))
}

fn div_ceil(value: i64, divisor: i64) -> i64 {
    (value + divisor - 1) / divisor
}

/// Which preset a menu row offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SnoozePresetId {
    Hour,
    Evening,
    Tomorrow,
    NextWeek,
}

/// One row of the snooze menu.
///
/// `label` names the choice and `when_label` states the resulting time. They
/// complement rather than repeat: "Tomorrow" pairs with "9:00", never
/// "tomorrow 9:00".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnoozePreset {
    pub id: SnoozePresetId,
    pub label: &'static str,
    pub when_label: String,
    pub wake_at_ms: u64,
}

/// The snooze choices to offer at `clock`.
///
/// "This evening" only appears while evening is more than an hour away; after
/// that it would resolve to a time barely distinguishable from "in 1 hour", or
/// worse, to a time already past. Everything below the hour preset advances by
/// calendar days, so a snooze set the evening before a clock change still lands
/// at 9:00 on the intended date.
pub fn snooze_presets(clock: Clock) -> Vec<SnoozePreset> {
    let offset = clock.utc_offset_seconds;
    let now_ms = clock.now_ms as i64;
    let now = Civil::from_unix_ms(now_ms, offset);

    let mut presets = Vec::with_capacity(4);

    let in_an_hour_ms = now_ms + MS_PER_HOUR;
    presets.push(SnoozePreset {
        id: SnoozePresetId::Hour,
        label: "In 1 hour",
        when_label: Civil::from_unix_ms(in_an_hour_ms, offset).time_label(),
        wake_at_ms: in_an_hour_ms as u64,
    });

    let evening_ms = now.with_time(EVENING_HOUR, 0).to_unix_ms(offset);
    if evening_ms - now_ms > MS_PER_HOUR {
        presets.push(SnoozePreset {
            id: SnoozePresetId::Evening,
            label: "This evening",
            when_label: Civil::from_unix_ms(evening_ms, offset).time_label(),
            wake_at_ms: evening_ms as u64,
        });
    }

    let tomorrow = now.add_days(1).with_time(MORNING_HOUR, 0);
    presets.push(SnoozePreset {
        id: SnoozePresetId::Tomorrow,
        label: "Tomorrow",
        when_label: tomorrow.time_label(),
        wake_at_ms: tomorrow.to_unix_ms(offset) as u64,
    });

    // Monday of next week, never today even when today is Monday.
    let days_until_monday = match (1 - now.weekday().index()).rem_euclid(7) {
        0 => 7,
        days => days,
    };
    let next_week = now.add_days(days_until_monday).with_time(MORNING_HOUR, 0);
    presets.push(SnoozePreset {
        id: SnoozePresetId::NextWeek,
        label: "Next week",
        when_label: format!(
            "{} {}",
            next_week.weekday().short_name(),
            next_week.time_label()
        ),
        wake_at_ms: next_week.to_unix_ms(offset) as u64,
    });

    presets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civil::days_from_civil;

    /// Build a UTC instant for a local wall-clock time under `offset`.
    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32, offset: i32) -> u64 {
        let civil = Civil {
            day_number: days_from_civil(year, month, day),
            year,
            month,
            day,
            hour,
            minute,
            second: 0,
            millis: 0,
        };
        civil.to_unix_ms(offset) as u64
    }

    fn clock(now_ms: u64, offset: i32) -> Clock {
        Clock {
            now_ms,
            utc_offset_seconds: offset,
        }
    }

    /// A snooze must expire exactly at its wake instant. One tick either way
    /// leaves a row hidden past its wake or wakes it a tick early, and the
    /// settled/active split is computed from this predicate.
    #[test]
    fn sleep_ends_precisely_at_the_wake_instant() {
        let snooze = Snooze {
            snoozed_at_ms: 1_000,
            wake_at_ms: 5_000,
        };
        assert!(snooze.is_asleep(4_999));
        assert!(!snooze.is_asleep(5_000));
        assert!(!snooze.is_asleep(5_001));
        assert_eq!(snooze.remaining_ms(4_000), 1_000);
        assert_eq!(snooze.remaining_ms(5_000), 0);
        assert_eq!(snooze.remaining_ms(9_999), 0);
    }

    /// Same-day wakes show the time alone. Prefixing them with a weekday would
    /// make the common "snooze an hour" case read like a date.
    #[test]
    fn a_wake_later_today_shows_only_the_time() {
        let now = at(2026, 3, 3, 9, 15, 0);
        assert_eq!(wake_description(at(2026, 3, 3, 17, 30, 0), clock(now, 0)), "17:30");
        assert_eq!(wake_description(at(2026, 3, 3, 9, 16, 0), clock(now, 0)), "9:16");
        assert_eq!(wake_description(at(2026, 3, 3, 23, 59, 0), clock(now, 0)), "23:59");
    }

    /// The today/tomorrow boundary is a calendar boundary, not a 24-hour one.
    /// Twenty minutes across midnight is "tomorrow"; twenty-three hours inside
    /// the same day is not.
    #[test]
    fn the_tomorrow_boundary_follows_midnight_not_elapsed_hours() {
        let late = at(2026, 3, 3, 23, 50, 0);
        assert_eq!(wake_description(at(2026, 3, 4, 0, 10, 0), clock(late, 0)), "tomorrow 0:10");

        let early = at(2026, 3, 3, 0, 5, 0);
        assert_eq!(wake_description(at(2026, 3, 3, 23, 55, 0), clock(early, 0)), "23:55");
    }

    /// Days two through six read as a weekday name; day seven crosses into the
    /// dated form. Both edges are asserted because an inclusive/exclusive slip
    /// prints "Tue 9:00" for something a full week away.
    #[test]
    fn the_within_week_band_runs_from_two_days_to_six() {
        // 2026-03-03 is a Tuesday.
        let now = at(2026, 3, 3, 9, 0, 0);
        assert_eq!(wake_description(at(2026, 3, 4, 9, 0, 0), clock(now, 0)), "tomorrow 9:00");
        assert_eq!(wake_description(at(2026, 3, 5, 9, 0, 0), clock(now, 0)), "Thu 9:00");
        assert_eq!(wake_description(at(2026, 3, 8, 9, 0, 0), clock(now, 0)), "Sun 9:00");
        assert_eq!(
            wake_description(at(2026, 3, 9, 9, 0, 0), clock(now, 0)),
            "Mon 9:00",
            "six days out is the last day of the weekday band"
        );
        assert_eq!(
            wake_description(at(2026, 3, 10, 9, 0, 0), clock(now, 0)),
            "Mar 10, 9:00",
            "seven days out crosses into the dated form"
        );
    }

    /// A wake time in a later month must print the month, and the weekday band
    /// must still work when the week itself spans the month boundary. Getting
    /// this wrong is invisible for most of the year and wrong for three days of
    /// every month.
    #[test]
    fn labels_are_correct_across_a_month_boundary() {
        // 2026-02-27 is a Friday, so Mar 1 is two days out and inside the week.
        let now = at(2026, 2, 27, 10, 0, 0);
        assert_eq!(wake_description(at(2026, 2, 28, 9, 0, 0), clock(now, 0)), "tomorrow 9:00");
        assert_eq!(wake_description(at(2026, 3, 1, 9, 0, 0), clock(now, 0)), "Sun 9:00");
        assert_eq!(wake_description(at(2026, 3, 3, 9, 0, 0), clock(now, 0)), "Tue 9:00");
        assert_eq!(wake_description(at(2026, 3, 6, 9, 0, 0), clock(now, 0)), "Mar 6, 9:00");

        // And across a year boundary, where the month name is the only signal.
        let december = at(2026, 12, 30, 10, 0, 0);
        assert_eq!(
            wake_description(at(2026, 12, 31, 9, 0, 0), clock(december, 0)),
            "tomorrow 9:00"
        );
        assert_eq!(wake_description(at(2027, 1, 1, 9, 0, 0), clock(december, 0)), "Fri 9:00");
        assert_eq!(wake_description(at(2027, 1, 20, 9, 0, 0), clock(december, 0)), "Jan 20, 9:00");
    }

    /// A lapsed wake time must read as a date, not as a future weekday. T3 Code
    /// falls into its weekday branch here and prints "Mon 9:00" for last Monday.
    #[test]
    fn a_wake_time_in_the_past_reads_as_a_date() {
        let now = at(2026, 3, 10, 9, 0, 0);
        assert_eq!(wake_description(at(2026, 3, 9, 9, 0, 0), clock(now, 0)), "Mar 9, 9:00");
        assert_eq!(wake_description(at(2026, 2, 1, 14, 30, 0), clock(now, 0)), "Feb 1, 14:30");
    }

    /// The offset decides which day "today" is. Without it a user at UTC+13
    /// gets "tomorrow" for a wake time happening this afternoon.
    #[test]
    fn the_utc_offset_decides_which_calendar_day_a_wake_lands_on() {
        let offset = 13 * 3600;
        let now = at(2026, 3, 3, 20, 0, offset);
        assert_eq!(wake_description(at(2026, 3, 3, 22, 0, offset), clock(now, offset)), "22:00");
        assert_eq!(
            wake_description(at(2026, 3, 4, 9, 0, offset), clock(now, offset)),
            "tomorrow 9:00"
        );

        // The same absolute instant, read at UTC, lands on a different local
        // day and therefore in a different band: "tomorrow 9:00" at +13 is
        // "20:00" today at UTC.
        let instant = at(2026, 3, 4, 9, 0, offset);
        assert_eq!(wake_description(instant, clock(now, 0)), "20:00");
    }

    /// The countdown rounds up so a live snooze never reads zero, and reports
    /// "now" only once genuinely elapsed. A "0m" badge on a hidden row looks
    /// like a bug to the operator.
    #[test]
    fn the_countdown_rounds_up_and_never_reads_zero_while_asleep() {
        let now = 1_000_000;
        assert_eq!(wake_countdown_label(now + 1, now), "1m");
        assert_eq!(wake_countdown_label(now + 30_000, now), "1m");
        assert_eq!(wake_countdown_label(now + 60_000, now), "1m");
        assert_eq!(wake_countdown_label(now + 60_001, now), "2m");
        assert_eq!(wake_countdown_label(now, now), "now");
        assert_eq!(wake_countdown_label(now - 1, now), "now");
    }

    /// Unit boundaries: exactly one hour is "1h", not "60m"; exactly one day is
    /// "1d", not "24h". Sitting on the wrong side of a boundary makes the badge
    /// flicker between units as the clock ticks.
    #[test]
    fn the_countdown_switches_units_at_exact_boundaries() {
        let now = 0u64;
        assert_eq!(wake_countdown_label(59 * 60_000, now), "59m");
        assert_eq!(wake_countdown_label(60 * 60_000, now), "1h");
        assert_eq!(wake_countdown_label(90 * 60_000, now), "2h");
        assert_eq!(wake_countdown_label(23 * 3_600_000, now), "23h");
        assert_eq!(wake_countdown_label(24 * 3_600_000, now), "1d");
        assert_eq!(wake_countdown_label(26 * 3_600_000, now), "2d");
        assert_eq!(wake_countdown_label(7 * 86_400_000, now), "7d");
    }

    /// Mid-morning offers all four choices in a fixed order. The order is what
    /// the menu renders, so it is part of the contract.
    #[test]
    fn a_mid_morning_clock_offers_all_four_presets_in_order() {
        let now = at(2026, 3, 3, 10, 0, 0);
        let presets = snooze_presets(clock(now, 0));
        assert_eq!(
            presets.iter().map(|preset| preset.id).collect::<Vec<_>>(),
            vec![
                SnoozePresetId::Hour,
                SnoozePresetId::Evening,
                SnoozePresetId::Tomorrow,
                SnoozePresetId::NextWeek,
            ]
        );
        assert_eq!(
            presets.iter().map(|preset| preset.label).collect::<Vec<_>>(),
            vec!["In 1 hour", "This evening", "Tomorrow", "Next week"]
        );
        assert_eq!(
            presets.iter().map(|preset| preset.when_label.as_str()).collect::<Vec<_>>(),
            vec!["11:00", "18:00", "9:00", "Mon 9:00"]
        );
        assert_eq!(presets[0].wake_at_ms, at(2026, 3, 3, 11, 0, 0));
        assert_eq!(presets[1].wake_at_ms, at(2026, 3, 3, 18, 0, 0));
        assert_eq!(presets[2].wake_at_ms, at(2026, 3, 4, 9, 0, 0));
        assert_eq!(presets[3].wake_at_ms, at(2026, 3, 9, 9, 0, 0));
    }

    /// "This evening" disappears once it is within an hour, otherwise it would
    /// duplicate "In 1 hour" and eventually offer a time already past.
    #[test]
    fn the_evening_preset_drops_once_evening_is_within_an_hour() {
        let offset = 0;
        let comfortably_before = at(2026, 3, 3, 16, 59, offset);
        assert!(
            snooze_presets(clock(comfortably_before, offset))
                .iter()
                .any(|preset| preset.id == SnoozePresetId::Evening)
        );

        let exactly_an_hour = at(2026, 3, 3, 17, 0, offset);
        assert_eq!(
            snooze_presets(clock(exactly_an_hour, offset))
                .iter()
                .map(|preset| preset.id)
                .collect::<Vec<_>>(),
            vec![SnoozePresetId::Hour, SnoozePresetId::Tomorrow, SnoozePresetId::NextWeek]
        );

        let past_evening = at(2026, 3, 3, 21, 0, offset);
        assert!(
            !snooze_presets(clock(past_evening, offset))
                .iter()
                .any(|preset| preset.id == SnoozePresetId::Evening)
        );
    }

    /// "Next week" always lands on a Monday and never on today, including when
    /// today is Monday. A modulo that returns zero would offer "next week" as
    /// nine o'clock this morning.
    #[test]
    fn next_week_always_lands_on_the_following_monday() {
        // 2026-03-02 is a Monday, 2026-03-08 a Sunday.
        for (day, expected) in [
            (2, at(2026, 3, 9, 9, 0, 0)),
            (3, at(2026, 3, 9, 9, 0, 0)),
            (7, at(2026, 3, 9, 9, 0, 0)),
            (8, at(2026, 3, 9, 9, 0, 0)),
            (9, at(2026, 3, 16, 9, 0, 0)),
        ] {
            let now = at(2026, 3, day, 10, 0, 0);
            let presets = snooze_presets(clock(now, 0));
            let next_week = presets
                .iter()
                .find(|preset| preset.id == SnoozePresetId::NextWeek)
                .expect("next week preset is always offered");
            assert_eq!(next_week.wake_at_ms, expected, "day {day}");
            assert_eq!(
                Civil::from_unix_ms(next_week.wake_at_ms as i64, 0).weekday().short_name(),
                "Mon"
            );
        }
    }

    /// Presets that advance a day must move the calendar date, not add
    /// 86_400_000 ms. Snoozing at 23:30 to "tomorrow" must land on the very next
    /// date at 9:00, which a millisecond add would get wrong on a short day.
    #[test]
    fn day_advancing_presets_use_calendar_days() {
        let late = at(2026, 3, 7, 23, 30, 0);
        let presets = snooze_presets(clock(late, 0));
        let tomorrow = presets
            .iter()
            .find(|preset| preset.id == SnoozePresetId::Tomorrow)
            .expect("tomorrow preset is always offered");
        let landed = Civil::from_unix_ms(tomorrow.wake_at_ms as i64, 0);
        assert_eq!((landed.year, landed.month, landed.day, landed.hour), (2026, 3, 8, 9));
        assert_eq!(tomorrow.wake_at_ms - late, 9 * 3_600_000 + 30 * 60_000);
    }

    /// Snooze state is persisted between runs, so it has to survive a JSON
    /// round-trip byte for byte. A renamed field would silently drop every
    /// pending snooze on upgrade.
    #[test]
    fn snooze_state_round_trips_through_json() {
        let snooze = Snooze {
            snoozed_at_ms: 1_772_580_600_000,
            wake_at_ms: 1_772_620_800_000,
        };
        let json = serde_json::to_string(&snooze).expect("snooze serialises");
        assert_eq!(
            json,
            r#"{"snoozedAtMs":1772580600000,"wakeAtMs":1772620800000}"#
        );
        let back: Snooze = serde_json::from_str(&json).expect("snooze round-trips");
        assert_eq!(back, snooze);
    }
}
