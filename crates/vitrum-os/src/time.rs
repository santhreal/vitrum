//! The machine's current offset from UTC.
//!
//! Formatting an absolute local date needs this and `std` does not provide it:
//! `SystemTime` is UTC and there is no local-time conversion in the standard
//! library. Pulling in a date-time crate to answer one question the operating
//! system already knows would be the wrong trade, so this asks the OS.
//!
//! The offset is read at each call rather than cached. It changes twice a year
//! for most of the world, and a process that runs for weeks and caches the
//! offset shows every timestamp an hour out for half the year.

/// Seconds to add to UTC to get local time.
///
/// Positive east of Greenwich: `+19800` for India, `-28800` for US Pacific
/// standard time. Returns `0` if the platform refuses to answer, which is the
/// same as treating local time as UTC and is the only defensible fallback: the
/// alternative is refusing to render a timestamp at all.
pub fn utc_offset_secs() -> i32 {
    platform_offset_secs()
}

/// Convert the two halves of a Windows `TIME_ZONE_INFORMATION` bias into an
/// offset.
///
/// Windows stores the bias the other way round from everyone else:
/// `UTC = local + Bias`, in minutes, so the sign flips. `extra_bias` is
/// `StandardBias` or `DaylightBias` depending on which the zone is currently
/// in, and it is almost always zero and minus sixty respectively.
pub const fn windows_offset_secs(bias_minutes: i32, extra_bias_minutes: i32) -> i32 {
    -(bias_minutes + extra_bias_minutes) * 60
}

#[cfg(unix)]
fn platform_offset_secs() -> i32 {
    // SAFETY: `time(NULL)` takes no output pointer and cannot fail in a way
    // that matters here; a -1 return feeds localtime_r, which then fails.
    let now = unsafe { libc::time(core::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { core::mem::zeroed() };
    // SAFETY: `now` is a valid time_t and `tm` is a live, zeroed struct tm that
    // outlives the call. localtime_r is the reentrant form precisely so this is
    // safe to call from any thread.
    let result = unsafe { libc::localtime_r(&now, &mut tm) };
    if result.is_null() {
        return 0;
    }
    // `tm_gmtoff` is seconds east of UTC on glibc, musl and Darwin alike, and
    // already accounts for daylight saving.
    tm.tm_gmtoff as i32
}

#[cfg(windows)]
fn platform_offset_secs() -> i32 {
    use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

    // The windows crate exports only TIME_ZONE_ID_INVALID, so the other three
    // return codes are spelled out here as the SDK defines them.
    const TIME_ZONE_ID_STANDARD: u32 = 1;
    const TIME_ZONE_ID_DAYLIGHT: u32 = 2;

    let mut info = TIME_ZONE_INFORMATION::default();
    // SAFETY: `info` is a live, default-initialised struct that outlives the
    // call.
    let kind = unsafe { GetTimeZoneInformation(&raw mut info) };
    let extra = match kind {
        TIME_ZONE_ID_STANDARD => info.StandardBias,
        TIME_ZONE_ID_DAYLIGHT => info.DaylightBias,
        // TIME_ZONE_ID_UNKNOWN means the zone has no daylight rules, so the
        // base bias is the whole answer. TIME_ZONE_ID_INVALID also lands here
        // and yields the base bias, which beats returning nothing.
        _ => 0,
    };
    windows_offset_secs(info.Bias, extra)
}

#[cfg(not(any(unix, windows)))]
fn platform_offset_secs() -> i32 {
    0
}
