//! The one clock reading per render tick.
//!
//! `vitrum-fmt` never reads the clock itself: `now` and the UTC offset are
//! parameters, so a caller cannot accidentally take two readings that straddle
//! a threshold and render "59s ago" on one row and "1m ago" on the next in the
//! same paint. This module is where the single reading happens.
//!
//! [`now`] and [`home`] are hoisted to the top of the root component and
//! passed down as props. Neither is free: `now` costs a `localtime_r` through
//! `vitrum-os`, which deliberately does not cache so a window open across a
//! DST transition stays correct, and `home` reads the environment. Calling
//! either inside a row would pay that per row, twenty times over, on every
//! paint. [`tests::the_clock_has_exactly_one_literal_call_site`] defends it.
//!
//! Clock-free by design, not by convenience. A self-updating "2m ago" needs a
//! repeating timer, and a repeating timer in a GUI process is a wakeup per tick
//! forever. Rows re-render when the server pushes `SessionUpdated`, and between
//! pushes the label is allowed to be stale.

use vitrum_fmt::{TimeFormat, Timestamp};

/// The clock for one render tick. Read once, passed to every row.
///
/// The UTC offset comes from the platform through `vitrum-os`, not from a
/// constant. It only affects the past-7-days branch, which renders a date, but
/// a hardcoded zero puts a user seven hours west of UTC on the wrong day for
/// the last seven hours of every day.
pub fn now() -> TimeFormat {
    let ms = Timestamp::from_system_time(std::time::SystemTime::now()).as_millis();
    render_clock(ms, vitrum_os::time::utc_offset_secs())
}

/// Build the clock a paint hands to every row, floored to a whole second.
///
/// # Why the reading is deliberately blunted
///
/// Every label derived from this clock is measured in seconds at its finest:
/// [`age`] renders "12s ago", and the model's disposition, parked and working
/// labels are coarser still. The milliseconds are therefore never visible on
/// screen — but they were visible to `PartialEq`.
///
/// [`TimeFormat`] is a prop of every session row, so a reading that differs by
/// one millisecond makes every row's props compare unequal, and Dioxus rebuilds
/// and re-diffs the entire list. At the stated load of twenty sessions the
/// daemon pushes twenty `SessionUpdated` a second and exactly one row changes
/// on each, so the client was rebuilding the whole sidebar twenty times a
/// second to redraw one row's timestamp.
///
/// Flooring makes the reading change exactly as often as the coarsest thing
/// drawn from it, which is once a second. Inside a second every untouched row
/// compares equal and is skipped; on the boundary they all update together,
/// which is also the only way twenty rows can agree on "now".
///
/// Floor rather than round: rounding would put the boundary half a second away
/// from the second it names, so a row created at t would read "1s ago" for the
/// first half of its life.
pub fn render_clock(now_ms: i64, utc_offset_secs: i32) -> TimeFormat {
    TimeFormat::new(
        Timestamp::from_millis(now_ms.div_euclid(1_000) * 1_000),
        utc_offset_secs,
    )
}

/// This user's home directory, for shortening paths on screen.
///
/// An absolute path is unreadable in a 14rem sidebar row and only slightly
/// better in a tooltip, and the leading `/home/someone` is the least
/// informative part of it. Empty when the platform cannot answer, which
/// `vitrum_fmt::path::shorten_home_relative` treats as "no home to strip"
/// rather than as an error.
pub fn home() -> String {
    vitrum_os::paths::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Render one session timestamp as an age.
pub fn age(clock: TimeFormat, then_ms: u64) -> String {
    clock.relative_ago(Timestamp::from_millis(then_ms as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one reading must return a plausible epoch value, not zero. A zero
    /// clock would make every row read as decades old.
    #[test]
    fn the_render_clock_is_after_2020() {
        assert!(
            now().now().as_millis() > 1_577_836_800_000,
            "clock before 2020-01-01"
        );
    }

    /// The millisecond form the shell stamps visits with must be derived from
    /// the SAME reading the labels render from, not from a second syscall.
    /// Two readings can straddle a threshold and disagree about whether a
    /// snooze has elapsed in the same paint that renders its countdown.
    #[test]
    fn the_millisecond_form_comes_from_the_same_reading() {
        let fmt = now();
        let ms = fmt.now().as_millis().max(0) as u64;
        assert_eq!(
            fmt.relative_ago(Timestamp::from_millis(ms as i64)),
            "just now"
        );
    }

    /// Ages must render from the caller's frozen clock, not from a second
    /// reading taken inside. Passing a fixed clock is what makes every row in
    /// one paint agree.
    #[test]
    fn ages_render_from_the_passed_clock_only() {
        let base = 1_700_000_000_000i64;
        let clock = TimeFormat::new(Timestamp::from_millis(base), 0);
        assert_eq!(age(clock, base as u64), "just now");
        assert_eq!(age(clock, (base - 12_000) as u64), "12s ago");
        assert_eq!(age(clock, (base - 4 * 60_000) as u64), "4m ago");
        assert_eq!(age(clock, (base - 2 * 3_600_000) as u64), "2h ago");
        assert_eq!(age(clock, (base - 3 * 86_400_000) as u64), "3d ago");
    }

    /// A timestamp from the future must not print a negative age. The daemon
    /// stamps times from its own clock; on a machine where the two disagree by
    /// a second, every fresh row would otherwise read "-1s ago".
    #[test]
    fn a_future_timestamp_clamps_to_just_now() {
        let base = 1_700_000_000_000i64;
        let clock = TimeFormat::new(Timestamp::from_millis(base), 0);
        assert_eq!(age(clock, (base + 60_000) as u64), "just now");
    }

    /// The render clock must carry this machine's real UTC offset, not zero.
    ///
    /// Only the past-7-days branch renders a date, so a hardcoded zero is
    /// invisible in every test that looks at "4m ago" and wrong for the last
    /// seven hours of every day for anyone west of Greenwich. Comparing the
    /// two `TimeFormat`s is what catches a regression back to a constant.
    #[test]
    fn the_render_clock_uses_the_platform_utc_offset() {
        let offset = vitrum_os::time::utc_offset_secs();
        assert!(
            (-12 * 3600..=14 * 3600).contains(&offset),
            "offset {offset} is outside the range of real timezones"
        );
        // A fixed instant, and a date thirty days before it, so both
        // formatters are in the absolute branch where the offset is the only
        // thing that can differ.
        let base = 1_700_000_000_000i64;
        let stamp = Timestamp::from_millis(base);
        let old = Timestamp::from_millis(base - 30 * 86_400_000);
        let utc = TimeFormat::new(stamp, 0);
        let local = TimeFormat::new(stamp, offset);
        if offset != 0 {
            assert_ne!(
                utc.absolute_datetime(old),
                local.absolute_datetime(old),
                "a non-zero offset must change an absolute datetime"
            );
        }
        // And the shipped clock must be the local one, not the UTC one.
        assert_eq!(
            now().absolute_datetime(old),
            TimeFormat::new(
                Timestamp::from_system_time(std::time::SystemTime::now()),
                offset
            )
            .absolute_datetime(old)
        );
    }

    /// The home directory must come from the platform, and must be usable as
    /// the prefix `vitrum_fmt` strips. An empty string is the honest answer on
    /// a machine with no home, and it must not panic or become "/".
    #[test]
    fn home_comes_from_the_platform_and_is_never_a_bare_slash() {
        let h = home();
        assert_ne!(h, "/", "a bare slash would strip the root off every path");
        if let Some(expected) = vitrum_os::paths::home_dir() {
            assert_eq!(h, expected.to_string_lossy());
            assert!(!h.is_empty());
        }
    }

    /// There is exactly one literal call site for the clock and for the home
    /// directory, in the root component, and none in any `ui/` module.
    ///
    /// Both cost a syscall: `now` goes through `localtime_r`, which
    /// `vitrum-os` deliberately does not cache so a window open across a DST
    /// transition stays correct, and `home` reads the environment. Called from
    /// a row they would be paid twenty times per paint at the stated load.
    /// Worse, two rows reading the clock separately can straddle a threshold
    /// and disagree about whether the same instant is "59s ago" or "1m ago".
    ///
    /// Checked against the source because there is no runtime hook for "this
    /// call happens outside a loop"; the invariant lives in where the call is
    /// written, so that is where it has to be enforced.
    ///
    /// What a green result does NOT prove: that exactly one syscall happens.
    /// A future helper or re-export that reaches `utc_offset_secs` indirectly
    /// would pass this unchanged. The thing actually guaranteeing one reading
    /// is that [`TimeFormat`] is `Copy` and is threaded down as a prop, so a
    /// row has the value and never a way to ask for it. This test defends that
    /// arrangement against the obvious way of breaking it, and nothing more;
    /// closing the indirect case would need machinery worth more than the bug.
    #[test]
    fn the_clock_has_exactly_one_literal_call_site() {
        let main = include_str!("main.rs");
        assert_eq!(
            main.matches("clock::now()").count(),
            1,
            "main.rs must read the render clock exactly once"
        );
        assert_eq!(
            main.matches("clock::home()").count(),
            1,
            "main.rs must read the home directory exactly once"
        );
        // The MARKUP half only. These names legitimately appear in a test's
        // own name and prose — `the_home_directory_is_written_...` contains
        // `home_dir` — and a guard that greps its own test module reports the
        // documentation as the violation.
        for (name, src) in [
            ("ui/sidebar.rs", include_str!("ui/sidebar.rs")),
            ("ui/titlebar.rs", include_str!("ui/titlebar.rs")),
            ("ui/dialog.rs", include_str!("ui/dialog.rs")),
            ("ui/terminal.rs", include_str!("ui/terminal.rs")),
            ("ui/menu.rs", include_str!("ui/menu.rs")),
        ] {
            let markup = src
                .split_once("\n#[cfg(test)]\n")
                .map_or(src, |(before, _)| before);
            assert!(
                markup.contains("rsx!"),
                "{name}: the markup/test split ate the markup, so this guard \
                 is scanning nothing"
            );
            // A CALL, not a bare token: `home_dir` is a substring of
            // `home_directory`, and matching the token alone fires on prose.
            for banned in [
                "clock::now()",
                "clock::home()",
                "utc_offset_secs(",
                "home_dir(",
            ] {
                assert!(
                    !markup.contains(banned),
                    "{name} calls {banned}; it must take the value as a prop instead"
                );
            }
        }
    }
}
