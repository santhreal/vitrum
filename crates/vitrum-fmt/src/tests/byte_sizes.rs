//! Binary byte sizes for scrollback display: units, rounding, and promotion.

use crate::bytes::{binary, binary_exact};

/// Below one kibibyte the exact count is shown with a `B` suffix.
///
/// `0.0 KiB` for an empty scrollback tells a user nothing; `0 B` says the
/// buffer is empty. Under a kibibyte the exact number is short enough to print,
/// so rounding it away would lose information for free.
#[test]
fn small_sizes_are_exact_bytes() {
    assert_eq!(binary(0), "0 B");
    assert_eq!(binary(1), "1 B");
    assert_eq!(binary(512), "512 B");
    assert_eq!(binary(1_023), "1023 B");
}

/// The `B` to `KiB` boundary is exactly 1024, and 1024 is `1.0 KiB`.
///
/// One column of unit and one of scale change at the same value; an off-by-one
/// here prints `1024 B`, which is the one number in the range that a reader
/// would immediately recognise as a formatting bug.
#[test]
fn the_kibibyte_boundary_is_exactly_1024() {
    assert_eq!(binary(1_023), "1023 B");
    assert_eq!(binary(1_024), "1.0 KiB");
    assert_eq!(binary(1_025), "1.0 KiB");
}

/// One decimal place is always shown above a kibibyte, even when it is zero.
///
/// A column that alternates between `1 MiB` and `1.2 MiB` changes width as the
/// value changes, so a right-aligned size column shuffles. Always printing the
/// tenth also tells the reader the number was rounded.
#[test]
fn one_decimal_place_is_always_shown() {
    assert_eq!(binary(1_024), "1.0 KiB");
    assert_eq!(binary(1_048_576), "1.0 MiB");
    assert_eq!(binary(1_073_741_824), "1.0 GiB");
    assert_eq!(binary(1_536), "1.5 KiB");
}

/// The documented examples render exactly as documented.
#[test]
fn the_documented_examples_hold() {
    assert_eq!(binary(1_229), "1.2 KiB");
    assert_eq!(binary(9_961_472), "9.5 MiB");
}

/// Rounding is half-up on integers, with the tie resolved upwards.
///
/// Computed in integer arithmetic rather than through `f64`, so the answer does
/// not depend on the target's floating-point rounding mode and cannot drift by
/// one in the last place between two machines showing the same session.
#[test]
fn rounding_is_half_up_and_exact() {
    // 1.05 KiB is exactly 1075.2 bytes, so 1075 rounds down and 1076 rounds up.
    assert_eq!(binary(1_074), "1.0 KiB");
    assert_eq!(binary(1_075), "1.0 KiB");
    assert_eq!(binary(1_076), "1.1 KiB");
    assert_eq!(binary(1_126), "1.1 KiB");
}

/// Rounding that reaches a full 1024 of a unit promotes to the next unit.
///
/// 1 048 570 bytes is 1023.99 KiB, which rounds to 1024.0. Printing
/// `1024.0 KiB` is arithmetically true and looks broken, because the whole
/// point of the unit is that it never reaches 1024.
#[test]
fn rounding_up_to_a_full_unit_promotes_instead() {
    assert_eq!(binary(1_048_570), "1.0 MiB");
    assert_eq!(binary(1_048_575), "1.0 MiB");
    assert_eq!(binary(1_048_576), "1.0 MiB");
    assert_eq!(binary(1_073_741_823), "1.0 GiB");
}

/// No output ever contains a mantissa of 1024 or more.
///
/// The invariant behind the promotion rule, swept across every unit boundary
/// and the values just below them, where a naive implementation fails.
#[test]
fn no_size_ever_renders_as_1024_of_its_unit() {
    let mut value: u64 = 1;
    for _ in 0..64 {
        for probe in [value.saturating_sub(1), value, value.saturating_add(1)] {
            let rendered = binary(probe);
            let mantissa = rendered
                .split(' ')
                .next()
                .and_then(|n| n.parse::<f64>().ok())
                .unwrap_or_else(|| panic!("unparseable size {rendered:?} for {probe}"));
            assert!(
                mantissa < 1024.0,
                "{probe} rendered as {rendered:?}, whose mantissa is not below 1024"
            );
        }
        let Some(next) = value.checked_mul(2) else { break };
        value = next;
    }
}

/// Every binary unit up to exbibytes is reachable and correctly named.
///
/// IEC names, not `KB`/`MB`, which mean 1000 to half the industry.
#[test]
fn every_unit_is_named_with_its_iec_suffix() {
    assert_eq!(binary(1_024), "1.0 KiB");
    assert_eq!(binary(1_024u64.pow(2)), "1.0 MiB");
    assert_eq!(binary(1_024u64.pow(3)), "1.0 GiB");
    assert_eq!(binary(1_024u64.pow(4)), "1.0 TiB");
    assert_eq!(binary(1_024u64.pow(5)), "1.0 PiB");
    assert_eq!(binary(1_024u64.pow(6)), "1.0 EiB");
}

/// The largest representable size formats without overflowing.
///
/// The rounding multiplies by ten before dividing, which overflows a `u64` for
/// anything above about 1.8 EiB. The computation is done in `u128` for exactly
/// this case.
#[test]
fn the_largest_size_does_not_overflow() {
    assert_eq!(binary(u64::MAX), "16.0 EiB");
    assert_eq!(binary(u64::MAX - 1), "16.0 EiB");
}

/// The exact form appends the raw byte count, and only above a kibibyte.
///
/// Below a kibibyte the rounded form is already exact, so repeating it would
/// read as `512 B (512 bytes)`.
#[test]
fn the_exact_form_appends_the_raw_count_when_it_adds_information() {
    assert_eq!(binary_exact(9_961_472), "9.5 MiB (9961472 bytes)");
    assert_eq!(binary_exact(1_024), "1.0 KiB (1024 bytes)");
    assert_eq!(binary_exact(512), "512 B", "already exact");
    assert_eq!(binary_exact(0), "0 B");
}
