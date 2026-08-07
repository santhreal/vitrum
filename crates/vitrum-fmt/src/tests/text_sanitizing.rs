//! Untrusted single-line titles. These strings come from OSC 0/2 sequences
//! written by whatever program the user ran, so they are attacker-controlled
//! in the same sense any terminal output is.

use crate::text::{display_width, is_all_printable_ascii_8, sanitize_line, title, truncate_end};

/// A newline or carriage return must not survive into a single-line label, and
/// must leave a space behind.
///
/// A `\n` in a sidebar row breaks the row into two and shifts every row below
/// it. A `\r` is worse: it returns the cursor to column zero and the second
/// half of the title overwrites the first, so the label a user sees is not the
/// label that was stored. Dropping them outright would run the words together
/// and change the meaning, so they become spaces.
#[test]
fn line_breaks_become_spaces() {
    assert_eq!(sanitize_line("build\nrelease"), "build release");
    assert_eq!(sanitize_line("build\r\nrelease"), "build  release");
    assert_eq!(sanitize_line("\nbuild\n"), " build ");
    assert_eq!(sanitize_line("a\u{b}b\u{c}c"), "a b c", "vertical tab, form feed");
}

/// A non-whitespace control character is dropped, not replaced.
///
/// It separates nothing, so unlike a tab or a newline it leaves nothing behind.
#[test]
fn non_whitespace_controls_are_dropped() {
    assert_eq!(sanitize_line("bell\u{7}"), "bell");
    assert_eq!(sanitize_line("del\u{7f}"), "del");
    assert_eq!(sanitize_line("nul\u{0}here"), "nulhere");
    assert_eq!(sanitize_line("shift\u{e}out"), "shiftout");
}

/// An escape is removed together with the sequence it introduces.
///
/// Dropping only the `\x1b` from `\x1b[31m` leaves the literal text `[31m` in
/// the label: the colour is gone and four columns of noise remain, which is the
/// worst of both outcomes and exactly what a naive control-character filter
/// produces. A CSI sequence runs from `ESC [` to its final byte in the range
/// `0x40..=0x7E`.
#[test]
fn a_csi_sequence_is_consumed_whole() {
    assert_eq!(sanitize_line("red\u{1b}[31mtext"), "redtext");
    assert_eq!(sanitize_line("\u{1b}[1;32mgreen\u{1b}[0m"), "green");
    assert_eq!(sanitize_line("home\u{1b}[Hafter"), "homeafter");
    assert_eq!(sanitize_line("c1\u{9b}31mhere"), "c1here", "C1 CSI needs no ESC");
}

/// An OSC string is consumed up to its terminator, in either form.
///
/// A title that itself contains an OSC 0 would otherwise leak the payload of a
/// nested title-set sequence into the visible label.
#[test]
fn an_osc_string_is_consumed_to_its_terminator() {
    assert_eq!(sanitize_line("a\u{1b}]0;nested title\u{7}b"), "ab", "BEL terminated");
    assert_eq!(
        sanitize_line("a\u{1b}]0;nested title\u{1b}\\b"),
        "ab",
        "ESC-backslash terminated"
    );
    assert_eq!(sanitize_line("a\u{1b}]0;nested\u{9c}b"), "ab", "C1 ST terminated");
}

/// An unterminated string sequence swallows the rest of the input.
///
/// That is what a real terminal does with it, and the alternative, guessing
/// where the author meant it to end, would put arbitrary payload bytes into a
/// label that is supposed to be sanitised.
#[test]
fn an_unterminated_string_sequence_consumes_the_remainder() {
    assert_eq!(sanitize_line("keep\u{1b}]0;never ends"), "keep");
    assert_eq!(sanitize_line("keep\u{1b}Pdcs payload"), "keep");
}

/// A two-or-more character escape is consumed whole.
///
/// `ESC ( B` selects a character set and is three bytes. Consuming only two
/// would leave a stray `B` in the label.
#[test]
fn a_short_escape_is_consumed_whole() {
    assert_eq!(sanitize_line("a\u{1b}(Bb"), "ab");
    assert_eq!(sanitize_line("a\u{1b}=b"), "ab", "two-character escape");
    assert_eq!(sanitize_line("trailing\u{1b}"), "trailing", "a bare trailing ESC");
}

/// A tab becomes a single space rather than vanishing.
///
/// Dropping it outright would run two words together (`make\tall` reading as
/// `makeall`), which changes the meaning of the label rather than its spacing.
#[test]
fn tabs_become_spaces() {
    assert_eq!(sanitize_line("make\tall"), "make all");
    assert_eq!(display_width(&sanitize_line("make\tall")), 8);
}

/// U+0085 is both a C1 control and Unicode whitespace, so it becomes a space.
///
/// It is the one codepoint where the "drop controls" and "keep word breaks"
/// rules collide, and the word break has to win.
#[test]
fn next_line_control_becomes_a_space() {
    assert_eq!(sanitize_line("nel\u{85}here"), "nel here");
}

/// Sanitising leaves ordinary text, including CJK and emoji, untouched.
///
/// An over-eager filter that stripped anything non-ASCII would mangle every
/// non-English project name in the sidebar.
#[test]
fn printable_text_is_untouched() {
    assert_eq!(sanitize_line("セッション 01"), "セッション 01");
    assert_eq!(sanitize_line("build 👍"), "build 👍");
    assert_eq!(sanitize_line("~/src/vitrum"), "~/src/vitrum");
    assert_eq!(sanitize_line("nbsp\u{a0}here"), "nbsp\u{a0}here", "NBSP is not a control");
}

/// `title` collapses runs of whitespace and trims the ends.
///
/// A shell prompt that sets its title from a command line often emits runs of
/// spaces and a trailing one. Rendering them literally wastes columns that the
/// truncator then charges against the visible text.
#[test]
fn title_collapses_whitespace_and_trims() {
    assert_eq!(title("  cargo   test  -p  vitrum  ", 40), "cargo test -p vitrum");
    assert_eq!(title("a\n\nb", 40), "a b", "two newlines collapse to one space");
    assert_eq!(title("   ", 40), "");
    assert_eq!(title("", 40), "");
}

/// `title` truncates after collapsing, not before.
///
/// Truncating first would spend budget on whitespace that was about to be
/// removed, so a title padded with spaces would come back shorter than a clean
/// one of the same visible length.
#[test]
fn title_truncates_after_collapsing() {
    assert_eq!(title("cargo    test    suite", 12), "cargo test…");
    assert!(display_width(&title("cargo    test    suite", 12)) <= 12);
    assert_eq!(
        title("cargo test suite", 12),
        title("cargo    test    suite", 12),
        "padding must not change the result"
    );
}

/// The ellipsis never follows a space.
///
/// When the cut lands just after a word boundary, keeping the space produces
/// `cargo test …`, which reads as a missing word rather than a truncation and
/// wastes a column.
#[test]
fn truncation_does_not_leave_a_space_before_the_ellipsis() {
    assert_eq!(truncate_end("cargo test suite", 12), "cargo test…");
    assert_eq!(truncate_end("one two three", 8), "one two…");
    assert!(!truncate_end("cargo test suite", 12).contains(" \u{2026}"));
}

/// A hostile title cannot exceed its column budget however many control
/// characters it contains.
///
/// Control characters are free in column terms, so a title padded with a
/// thousand of them still has to render inside its cell.
#[test]
fn control_characters_cannot_inflate_a_title_past_its_budget() {
    let hostile = format!("{}danger", "\u{1b}".repeat(1000));
    let rendered = title(&hostile, 8);
    assert_eq!(rendered, "danger");
    assert_eq!(display_width(&rendered), 6);
    assert!(!rendered.contains('\u{1b}'));
}

/// A title of wide characters is truncated by columns, not by characters.
///
/// The acceptance case for titles: `セッション一覧 - vitrum` is 17 characters
/// and 23 columns. Anything that budgets by character count lets it overrun a
/// 20-column tab by three columns, and the tab strip stops lining up.
#[test]
fn a_wide_character_title_is_truncated_by_columns() {
    let raw = "  セッション一覧 - vitrum\t";
    assert_eq!(title(raw, 40), "セッション一覧 - vitrum");
    assert_eq!(display_width(&title(raw, 40)), 23);

    let cut = title(raw, 20);
    assert_eq!(cut, "セッション一覧 - vi\u{2026}");
    assert_eq!(display_width(&cut), 20);

    let tighter = title(raw, 9);
    assert_eq!(tighter, "セッショ\u{2026}");
    assert_eq!(display_width(&tighter), 9);

    let odd = title(raw, 8);
    assert_eq!(odd, "セッシ\u{2026}", "a wide character is dropped, not split");
    assert_eq!(display_width(&odd), 7, "one column left blank rather than split");
}
#[test]
fn simd_swar_sanitizer_edge_cases() {
    let ascii_long = "abcdefghijklmnopqrstuvwxyz0123456789_ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    assert_eq!(sanitize_line(ascii_long), ascii_long);

    let swar_with_ansi = "abcdefgh\x1b[31m12345678ijklmnop\x1b[0mQRSTUVWX";
    assert_eq!(sanitize_line(swar_with_ansi), "abcdefgh12345678ijklmnopQRSTUVWX");

    let mixed_control = "hello\x07world\x1b[1mbold\x1b[m!";
    assert_eq!(sanitize_line(mixed_control), "helloworldbold!");
}

/// The eight-byte probe agrees with the rule it stands in for, exhaustively.
///
/// The probe decides with one subtract, one add and one mask over a whole word,
/// so a borrow out of one byte lands in the next. That is safe in one direction
/// only: a dirty byte must never be reported clean, because the fast path then
/// copies it through untouched, while a clean byte reported dirty costs only
/// the slow path. Every pair of adjacent byte values in every position is
/// checked, which is where a borrow would show if it could.
#[test]
fn the_eight_byte_probe_never_calls_a_dirty_byte_clean() {
    let printable = |b: u8| (0x20..=0x7e).contains(&b);
    for position in 0..7 {
        for first in 0..=u8::MAX {
            for second in 0..=u8::MAX {
                let mut chunk = [b'A'; 8];
                chunk[position] = first;
                chunk[position + 1] = second;
                let truth = chunk.iter().all(|b| printable(*b));
                if is_all_printable_ascii_8(&chunk) {
                    assert!(
                        truth,
                        "the probe called {chunk:?} clean when byte {first:#04x} \
                         or {second:#04x} is outside 0x20..=0x7e, so the fast \
                         path would copy it into a label untouched"
                    );
                }
            }
        }
    }
}

/// Sanitizing does not depend on whether the fast path ran.
///
/// `ESC [ m` is consumed whole and contributes nothing, so prefixing it leaves
/// the answer alone while forcing the first byte to be dirty, which is what
/// sends the same input down the copying path instead of the borrowing one.
/// The two must agree for every input, or the fast path is not an optimisation
/// but a second behaviour.
#[test]
fn the_fast_path_and_the_slow_path_agree() {
    let mut seed = 0x243f_6a88_85a3_08d3u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    // Bytes chosen to land on the boundaries the probe cares about, mixed with
    // the escape introducers the state machine has to consume.
    let alphabet = [
        b'a', b'Z', b'0', b' ', 0x1f, 0x20, 0x7e, 0x7f, 0x80, 0xff, 0x1b, b'[', b'm', b']', 0x07,
        b'\n', b'\r', b'\t',
    ];
    for _ in 0..4000 {
        let len = (next() % 40) as usize;
        let raw: Vec<u8> = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let forced = format!("\u{1b}[m{text}");
        assert_eq!(
            sanitize_line(&text),
            sanitize_line(&forced),
            "the borrowing path and the copying path disagreed on {text:?}"
        );
    }
}
