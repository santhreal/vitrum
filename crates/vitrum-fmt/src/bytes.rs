//! Byte sizes for scrollback and buffer displays: `1.2 KiB`, `9.5 MiB`.
//!
//! # Binary units, spelled out
//!
//! Scrollback buffers are allocated in powers of two and their limits are
//! configured in powers of two, so reporting them in powers of ten would make
//! a 4 MiB cap read as `4.2 MB` and invite a bug report. Units are the IEC
//! names (`KiB`, `MiB`), not the ambiguous `KB`, because `KB` means 1000 in
//! half the industry and 1024 in the other half.
//!
//! # Formatting
//!
//! Under 1024 bytes the exact integer is shown with a `B` suffix: `0 B`,
//! `1023 B`. From 1 KiB up, exactly one decimal place is always shown, even
//! when it is zero (`1.0 KiB`), so the column width is stable and a reader
//! never has to work out whether `1 MiB` was rounded.
//!
//! Rounding is half-up on integers, never floating point, so the output is
//! identical on every target and cannot drift by one in the last place. When
//! rounding pushes a value to `1024.0` of its unit, it is promoted to the next
//! unit instead: 1 048 570 bytes renders `1.0 MiB`, never `1024.0 KiB`.

use std::fmt::Write as _;

const INFALLIBLE: &str = "writing to a String cannot fail";

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// Human-readable binary size: `0 B`, `1023 B`, `1.0 KiB`, `1.2 KiB`, `9.5 MiB`.
#[must_use]
pub fn binary(bytes: u64) -> String {
    let mut out = String::with_capacity(12);
    write_binary(&mut out, bytes);
    out
}

/// Append the rounded size to `out`.
fn write_binary(out: &mut String, bytes: u64) {
    if bytes < 1024 {
        write!(out, "{bytes} B").expect(INFALLIBLE);
        return;
    }

    let mut unit = 0usize;
    let mut divisor: u128 = 1;
    while unit + 1 < UNITS.len() && u128::from(bytes) >= divisor * 1024 {
        divisor *= 1024;
        unit += 1;
    }

    let mut tenths = round_tenths(u128::from(bytes), divisor);
    if tenths >= 10_240 && unit + 1 < UNITS.len() {
        divisor *= 1024;
        unit += 1;
        tenths = round_tenths(u128::from(bytes), divisor);
    }

    write!(out, "{}.{} {}", tenths / 10, tenths % 10, UNITS[unit]).expect(INFALLIBLE);
}

/// `bytes / divisor` in tenths, rounded half-up.
fn round_tenths(bytes: u128, divisor: u128) -> u128 {
    (bytes * 10 + divisor / 2) / divisor
}

/// Size plus the exact byte count, for tooltips: `9.5 MiB (9961472 bytes)`.
///
/// The rounded form is what a person reads; the exact form is what they paste
/// into a bug report.
#[must_use]
pub fn binary_exact(bytes: u64) -> String {
    if bytes < 1024 {
        return binary(bytes);
    }
    // One buffer for both halves: building the rounded form on its own and
    // then formatting it into a second string allocated twice per tooltip.
    let mut out = String::with_capacity(36);
    write_binary(&mut out, bytes);
    write!(out, " ({bytes} bytes)").expect(INFALLIBLE);
    out
}
