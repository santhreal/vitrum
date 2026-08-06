//! Local UTC offset and home directory resolution.

use std::path::PathBuf;

use crate::paths::{PathEnv, Platform, home_dir_from};
use crate::time::{utc_offset_secs, windows_offset_secs};

/// The Windows bias must be negated, because Windows stores it backwards.
///
/// Windows defines `UTC = local + Bias`, so a `Bias` of 480 is UTC-8. Copying
/// the value straight through puts every timestamp sixteen hours out for a US
/// Pacific user, and exactly zero hours out in London, which is where it never
/// gets noticed.
#[test]
fn the_windows_bias_is_negated() {
    assert_eq!(windows_offset_secs(480, 0), -28_800, "US Pacific standard time is UTC-8");
    assert_eq!(windows_offset_secs(0, 0), 0, "UTC");
    assert_eq!(windows_offset_secs(-330, 0), 19_800, "India is UTC+5:30");
}

/// Daylight saving must be folded in through the extra bias.
///
/// `DaylightBias` is normally -60. Ignoring it leaves the app an hour out for
/// half the year in most of the northern hemisphere.
#[test]
fn the_daylight_bias_is_folded_in() {
    assert_eq!(windows_offset_secs(480, -60), -25_200, "US Pacific daylight time is UTC-7");
    assert_eq!(windows_offset_secs(0, -60), 3_600, "British summer time is UTC+1");
}

/// The live offset must be a real timezone offset, not a random integer.
///
/// The valid range is UTC-12 to UTC+14, and every real zone is a multiple of
/// fifteen minutes. A garbage read from `tm_gmtoff` fails both.
#[test]
fn the_live_offset_is_a_plausible_timezone() {
    let offset = utc_offset_secs();
    assert!(
        (-12 * 3600..=14 * 3600).contains(&offset),
        "offset {offset} is outside the range of real timezones"
    );
    assert_eq!(offset % 900, 0, "offset {offset} is not a multiple of fifteen minutes");
}

/// The offset must be stable across calls within a test run.
///
/// It is read fresh each time so a long-running process follows a daylight
/// transition. Two reads a microsecond apart must still agree, which catches an
/// uninitialised `struct tm`.
#[test]
fn the_live_offset_is_stable_across_calls() {
    assert_eq!(utc_offset_secs(), utc_offset_secs());
}

/// Unix reads `$HOME`.
#[test]
fn unix_home_comes_from_home() {
    let env = PathEnv::from_pairs([("HOME", "/home/ada")]);
    assert_eq!(home_dir_from(Platform::Linux, &env), Some(PathBuf::from("/home/ada")));
    assert_eq!(home_dir_from(Platform::MacOs, &env), Some(PathBuf::from("/home/ada")));
    assert_eq!(home_dir_from(Platform::Linux, &PathEnv::default()), None);
}

/// Windows prefers `%USERPROFILE%`.
///
/// There is no `HOME` on Windows. Reading it would make path shortening a
/// silent no-op for every Windows user.
#[test]
fn windows_home_prefers_userprofile() {
    let env = PathEnv::from_pairs([
        ("USERPROFILE", "C:/Users/ada"),
        ("HOMEDRIVE", "C:"),
        ("HOMEPATH", "/Users/other"),
    ]);
    assert_eq!(home_dir_from(Platform::Windows, &env), Some(PathBuf::from("C:/Users/ada")));
}

/// Windows falls back to `%HOMEDRIVE%` plus `%HOMEPATH%`.
///
/// `%USERPROFILE%` is absent in some service and scheduled-task contexts where
/// the split pair still resolves.
#[test]
fn windows_home_falls_back_to_the_split_pair() {
    let env = PathEnv::from_pairs([("HOMEDRIVE", "C:"), ("HOMEPATH", "/Users/ada")]);
    assert_eq!(home_dir_from(Platform::Windows, &env), Some(PathBuf::from("C:/Users/ada")));
}

/// A half-present split pair must yield nothing rather than a broken path.
///
/// `HOMEDRIVE` alone is `C:`, which as a home directory would silently shorten
/// every path to nonsense.
#[test]
fn a_half_present_windows_pair_yields_nothing() {
    assert_eq!(
        home_dir_from(Platform::Windows, &PathEnv::from_pairs([("HOMEDRIVE", "C:")])),
        None
    );
    assert_eq!(
        home_dir_from(Platform::Windows, &PathEnv::from_pairs([("HOMEPATH", "/Users/ada")])),
        None
    );
    assert_eq!(
        home_dir_from(Platform::Windows, &PathEnv::from_pairs([("HOME", "/home/ada")])),
        None,
        "HOME is not a Windows variable and must not be used there"
    );
}
