//! Dedicated integration test suite for SWAR ANSI escape state machine edge cases,
//! zero-alloc Cow borrowing, fast-path ASCII display width truncation, and 256-color LUT formatting.

use std::borrow::Cow;
use std::sync::Arc;
use std::thread;
use vitrum_fmt::color::{
    COLOR_256_LUT, color_256_to_rgb, format_256_bg, format_256_fg, rgb_to_256_color,
};
use vitrum_fmt::text::{
    display_width, fits, is_clean_swar, is_printable_ascii_swar, sanitize_cow, sanitize_line,
    truncate_ascii_cow, truncate_end, ELLIPSIS,
};

/// WHY: Validates that SWAR ANSI escape parsing correctly handles edge cases in CSI escape sequences
/// (such as `\x1b[31m`, `\x1b[1;32;40m`, private modes `\x1b[?25h`, cursor moves `\x1b[2K`, C1 CSI `\u{9b}31m`)
/// without corrupting payload text or missing multi-parameter SGR commands.
#[test]
fn swar_csi_and_c1_control_sequences_edge_cases() {
    assert_eq!(sanitize_line("\x1b[31mred text\x1b[0m"), "red text");
    assert_eq!(sanitize_line("\x1b[1;32;40mbold green on black\x1b[m"), "bold green on black");
    assert_eq!(sanitize_line("prefix\x1b[2K\x1b[1;1Hsuffix"), "prefixsuffix");
    assert_eq!(sanitize_line("cursor\x1b[?25hvisible"), "cursorvisible");
    assert_eq!(sanitize_line("c1_csi\u{9b}38;5;196mcolored"), "c1_csicolored");
    assert_eq!(sanitize_line("\x1b[mempty_sgr"), "empty_sgr");
    assert_eq!(sanitize_line("trailing_csi\x1b[10;20"), "trailing_csi");
}

/// WHY: Defends against OSC title injection attacks where malicious process output sets terminal titles
/// using OSC 0/2 sequences terminated by BEL (`\x07`), ST (`\x1b\`), or C1 ST (`\u{9c}`), as well as unterminated
/// strings that must safely consume remaining input without leaking payload into UI labels.
#[test]
fn swar_osc_string_terminator_variants() {
    assert_eq!(sanitize_line("clean\x1b]0;window title\x07header"), "cleanheader");
    assert_eq!(sanitize_line("clean\x1b]2;window title\x1b\\header"), "cleanheader");
    assert_eq!(sanitize_line("clean\u{9d}0;c1 osc title\u{9c}header"), "cleanheader");
    assert_eq!(sanitize_line("keep\x1b]0;unterminated title payload"), "keep");
    assert_eq!(sanitize_line("keep\u{9d}0;unterminated c1 osc"), "keep");
}

/// WHY: Ensures non-CSI escape sequences like DCS (`\x1bP`), SOS (`\x1bX`), PM (`\x1b^`), APC (`\x1b_`),
/// and character set designators (`\x1b(B`, `\x1b#8`) are completely stripped together with their
/// parameters/payloads so raw terminal graphics/control codes never leak into single-line UI strings.
#[test]
fn swar_dcs_sos_pm_apc_and_charset_designators() {
    assert_eq!(sanitize_line("start\x1bPq#0;2\x1b\\end"), "startend");
    assert_eq!(sanitize_line("start\x1bXsos payload\x1b\\end"), "startend");
    assert_eq!(sanitize_line("start\x1b^pm payload\x07end"), "startend");
    assert_eq!(sanitize_line("start\x1b_G a=T\x1b\\end"), "startend");
    assert_eq!(sanitize_line("plain\x1b(Btext"), "plaintext");
    assert_eq!(sanitize_line("grid\x1b#8test"), "gridtest");
}

/// WHY: Tests parser resilience under adversarial and malformed inputs such as double ESC (`\x1b\x1b[31m`),
/// truncated ESC at string boundaries (`\x1b`, `\x1b[`), nested escape attempts, embedded C0 control bytes
/// inside SGR parameter sequences, and extreme control byte sequences.
#[test]
fn swar_malformed_and_adversarial_escapes() {
    assert_eq!(sanitize_line("double\x1b\x1b[31mred"), "double[31mred");
    assert_eq!(sanitize_line("truncated\x1b"), "truncated");
    assert_eq!(sanitize_line("truncated_csi\x1b["), "truncated_csi");
    assert_eq!(sanitize_line("truncated_osc\x1b]"), "truncated_osc");
    assert_eq!(sanitize_line("embedded_nul\x00here"), "embedded_nulhere");
    assert_eq!(sanitize_line("embedded_bel\x07here"), "embedded_belhere");
    assert_eq!(sanitize_line("embedded_del\x7fhere"), "embedded_delhere");
    assert_eq!(sanitize_line("newlines\nand\rreturns"), "newlines and returns");
}

/// WHY: Proves that `sanitize_cow` on clean printable ASCII strings operates as a zero-allocation fast-path
/// returning `Cow::Borrowed` with identical pointer addresses (`std::ptr::eq`), avoiding heap allocation overhead
/// on the critical path for millions of clean UI title evaluations.
#[test]
fn zero_alloc_cow_clean_string_returns_borrowed() {
    let clean_str = "cargo build --release --workspace";
    assert!(is_clean_swar(clean_str));

    let result = sanitize_cow(clean_str);
    match result {
        Cow::Borrowed(borrowed) => {
            assert!(std::ptr::eq(clean_str, borrowed));
            assert_eq!(borrowed, clean_str);
        }
        Cow::Owned(_) => panic!("Expected Cow::Borrowed for clean ASCII string"),
    }
}

/// WHY: Verifies that `sanitize_cow` correctly returns `Cow::Owned` when encountering ANSI escapes,
/// C0 control characters, or C1 controls, returning a sanitized string with all controls removed or converted
/// to single spaces where appropriate.
#[test]
fn zero_alloc_cow_dirty_string_returns_owned() {
    let dirty_str = "compiling \x1b[32mvitrum-fmt\x1b[0m v0.1.0\nline2";
    assert!(!is_clean_swar(dirty_str));

    let result = sanitize_cow(dirty_str);
    match result {
        Cow::Borrowed(_) => panic!("Expected Cow::Owned for dirty string with ANSI escapes and newlines"),
        Cow::Owned(owned) => {
            assert_eq!(owned, "compiling vitrum-fmt v0.1.0 line2");
        }
    }
}

/// WHY: Ensures that `sanitize_cow` handles internationalized text (CJK characters, ZWJ emoji sequences,
/// combining marks) without mis-identifying clean UTF-8 as dirty, preserving `Cow::Borrowed` zero-allocation guarantees
/// for clean non-English titles.
#[test]
fn zero_alloc_cow_clean_unicode_and_emoji_returns_borrowed() {
    let unicode_str = "セッション一覧 - 🚀 vitrum-fmt";
    let result = sanitize_cow(unicode_str);

    match result {
        Cow::Borrowed(borrowed) => {
            assert!(std::ptr::eq(unicode_str, borrowed));
            assert_eq!(borrowed, unicode_str);
        }
        Cow::Owned(_) => panic!("Expected Cow::Borrowed for clean Unicode/emoji string"),
    }
}

/// WHY: Defends the invariant that `is_printable_ascii_swar` fast-path display width measurement produces
/// results identical to full unicode grapheme cluster width calculations for printable ASCII inputs while executing
/// in O(1) allocation and minimal CPU cycles.
#[test]
fn fast_path_ascii_display_width_accuracy() {
    let ascii = "git checkout -b feat/expand-regression-test-suite";
    assert!(is_printable_ascii_swar(ascii));

    assert_eq!(display_width(ascii), ascii.len());
    assert_eq!(display_width(ascii), 49);
    assert!(fits(ascii, 49));
    assert!(fits(ascii, 60));
    assert!(!fits(ascii, 48));
}

/// WHY: Confirms that `truncate_ascii_cow` returns `Cow::Borrowed` when an ASCII string fits within the
/// allocated column budget, avoiding memory allocations during sidebar and table cell layout passes.
#[test]
fn fast_path_ascii_truncation_borrowed_when_fits() {
    let short_ascii = "vitrum-fmt";
    assert!(is_printable_ascii_swar(short_ascii));

    let truncated = truncate_ascii_cow(short_ascii, 15);
    match truncated {
        Cow::Borrowed(borrowed) => {
            assert!(std::ptr::eq(short_ascii, borrowed));
            assert_eq!(borrowed, "vitrum-fmt");
        }
        Cow::Owned(_) => panic!("Expected Cow::Borrowed when string fits within budget"),
    }
}

/// WHY: Tests fast-path ASCII truncation edge cases including exact budget boundaries, zero budget,
/// 1-column budget, whitespace trimming before ellipsis, and long strings truncated at arbitrary bounds,
/// guaranteeing the resulting string never exceeds budget and is formatted with `ELLIPSIS`.
#[test]
fn fast_path_ascii_truncation_boundary_and_ellipsis() {
    let text = "cargo test --all"; // 16 bytes
    assert!(is_printable_ascii_swar(text));

    // Zero budget
    assert_eq!(truncate_ascii_cow(text, 0), "");

    // Exact budget
    assert_eq!(truncate_ascii_cow(text, 16), "cargo test --all");

    // Truncate with budget 10 -> keep 9 cols, "cargo tes" + "…"
    let res10 = truncate_ascii_cow(text, 10);
    assert_eq!(res10, format!("cargo tes{ELLIPSIS}"));
    assert_eq!(display_width(&res10), 10);

    // Truncate landing on space boundary -> "cargo" + "…" (trailing space trimmed before ellipsis)
    let res6 = truncate_ascii_cow(text, 6);
    assert_eq!(res6, format!("cargo{ELLIPSIS}"));
    assert_eq!(display_width(&res6), 6);

    // Full truncate_end delegation
    assert_eq!(truncate_end(text, 6), format!("cargo{ELLIPSIS}"));
}

/// WHY: Validates the 256-color lookup table (`COLOR_256_LUT`) static array bounds, ensuring `color_256_to_rgb`
/// maps standard ANSI colors (0..15), the 6x6x6 color cube (16..231), and grayscale ramps (232..255) to exact,
/// deterministic RGB triples.
#[test]
fn color_lut_256_rgb_mapping_roundtrip_and_bounds() {
    assert_eq!(COLOR_256_LUT.len(), 256);

    // Index 0: Black
    assert_eq!(color_256_to_rgb(0), (0x00, 0x00, 0x00));
    // Index 1: Red
    assert_eq!(color_256_to_rgb(1), (0x80, 0x00, 0x00));
    // Index 9: Bright Red
    assert_eq!(color_256_to_rgb(9), (0xff, 0x00, 0x00));
    // Index 15: Bright White
    assert_eq!(color_256_to_rgb(15), (0xff, 0xff, 0xff));

    // Color Cube Base 16: (0, 0, 0)
    assert_eq!(color_256_to_rgb(16), (0, 0, 0));
    // Color Cube End 231: (255, 255, 255)
    assert_eq!(color_256_to_rgb(231), (255, 255, 255));

    // Grayscale Start 232: (8, 8, 8)
    assert_eq!(color_256_to_rgb(232), (8, 8, 8));
    // Grayscale End 255: (238, 238, 238)
    assert_eq!(color_256_to_rgb(255), (238, 238, 238));
}

/// WHY: Verifies `rgb_to_256_color` nearest-neighbor matching algorithm using Euclidean RGB distance,
/// ensuring exact RGB matches return their exact palette index and arbitrary RGB values match their closest
/// visual equivalent in the 256-color palette.
#[test]
fn color_lut_rgb_to_256_nearest_matching() {
    // Exact standard color matches
    assert_eq!(rgb_to_256_color(0, 0, 0), 0);
    assert_eq!(rgb_to_256_color(255, 0, 0), 9);
    assert_eq!(rgb_to_256_color(0, 255, 0), 10);
    assert_eq!(rgb_to_256_color(0, 0, 255), 12);
    assert_eq!(rgb_to_256_color(255, 255, 255), 15);

    // Approximate colors mapping to closest index
    let near_red = rgb_to_256_color(250, 5, 5);
    assert_eq!(color_256_to_rgb(near_red), (255, 0, 0));

    let near_gray = rgb_to_256_color(100, 100, 100);
    let (r, g, b) = color_256_to_rgb(near_gray);
    assert!((r as i32 - 100).abs() <= 15);
    assert!((g as i32 - 100).abs() <= 15);
    assert!((b as i32 - 100).abs() <= 15);
}

/// WHY: Tests that `format_256_fg` and `format_256_bg` produce standard, specification-compliant ANSI 256-color
/// SGR escape sequences (`\x1b[38;5;Nm` and `\x1b[48;5;Nm`) terminated with reset sequence `\x1b[0m`.
#[test]
fn color_lut_sgr_formatting_fg_and_bg() {
    let fg_formatted = format_256_fg(196, "Error");
    assert_eq!(fg_formatted, "\x1b[38;5;196mError\x1b[0m");

    let bg_formatted = format_256_bg(21, "Highlight");
    assert_eq!(bg_formatted, "\x1b[48;5;21mHighlight\x1b[0m");
}

/// WHY: Demonstrates round-trip integrity: text formatted with 256-color SGR escape sequences via `format_256_fg`
/// or `format_256_bg` is cleanly stripped back to its original plain string by `sanitize_cow`.
#[test]
fn color_lut_roundtrip_formatting_and_sanitization() {
    let original = "status: active";
    let colored_fg = format_256_fg(46, original);
    let colored_bg = format_256_bg(235, original);
    let combined = format!("{colored_fg} / {colored_bg}");

    let sanitized = sanitize_cow(&combined);
    assert_eq!(sanitized, "status: active / status: active");
}

/// WHY: Guarantees thread safety and absence of data races when multiple threads concurrently execute SWAR
/// sanitization, fast-path ASCII truncation, and 256-color LUT lookup across shared read-only tables and state machines.
#[test]
fn concurrent_swar_sanitization_and_lut_formatting() {
    let input = Arc::new("\x1b[38;5;208mwarning:\x1b[0m high CPU utilization detected\n");
    let mut handles = Vec::new();

    for i in 0..8 {
        let input_clone = Arc::clone(&input);
        handles.push(thread::spawn(move || {
            for j in 0..1000 {
                let sanitized = sanitize_cow(&input_clone);
                assert_eq!(sanitized, "warning: high CPU utilization detected ");

                let code = ((i * 32 + j) % 256) as u8;
                let (r, g, b) = color_256_to_rgb(code);
                let matched_code = rgb_to_256_color(r, g, b);
                assert_eq!(color_256_to_rgb(matched_code), (r, g, b));

                let formatted = format_256_fg(code, "thread_test");
                assert_eq!(sanitize_line(&formatted), "thread_test");
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }
}
