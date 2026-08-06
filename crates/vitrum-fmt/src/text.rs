//! Column-accurate width measurement and truncation.
//!
//! Everything in this module counts **terminal columns**, never bytes and never
//! `char`s. A sidebar is a grid of cells; a label that measures 15 bytes can
//! occupy 5 chars and 10 columns (`"漢字テスト"`). Truncating on bytes corrupts
//! UTF-8, truncating on chars overflows the layout, and both split double-width
//! glyphs in half.
//!
//! Text is walked as grapheme clusters so that a base character never loses its
//! combining marks and a ZWJ emoji sequence is never cut apart.

use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// The single character used for every elision in this crate.
///
/// U+2026 HORIZONTAL ELLIPSIS. One column wide, one glyph, never three periods:
/// three periods cost three columns and read as a pause rather than a cut.
pub const ELLIPSIS: char = '\u{2026}';

/// Columns occupied by one grapheme cluster.
///
/// Control characters measure `0`. `unicode-width` charges them one column,
/// which is right for a `printf` and wrong for a terminal: an `ESC` opens a
/// sequence, a `BEL` rings, a `DEL` does nothing, and none of them advance the
/// cursor. Charging them a column makes a title that contains a stray escape
/// measure wider than it draws, so the layout under-fills its own cell.
///
/// Grapheme break rules put control characters in clusters of their own, so a
/// cluster is either entirely control or contains none, and the whole-cluster
/// test cannot mis-zero a printable glyph. `CRLF` is one cluster of two
/// controls and correctly measures `0`.
#[must_use]
pub fn cluster_width(cluster: &str) -> usize {
    match cluster.as_bytes() {
        // The common case by a wide margin: one printable ASCII byte.
        [byte] if byte.is_ascii_graphic() || *byte == b' ' => 1,
        _ if cluster.chars().all(char::is_control) => 0,
        _ => UnicodeWidthStr::width(cluster),
    }
}

/// Columns occupied by a string when printed to a terminal.
#[must_use]
pub fn display_width(text: &str) -> usize {
    text.graphemes(true).map(cluster_width).sum()
}

/// Whether `text` fits in `budget` columns.
///
/// Stops as soon as the budget is blown, so testing a 4 MiB pasted title
/// against a 24 column cell segments 25 clusters rather than the whole thing.
#[must_use]
pub fn fits(text: &str, budget: usize) -> bool {
    let mut used = 0usize;
    for cluster in text.graphemes(true) {
        used += cluster_width(cluster);
        if used > budget {
            return false;
        }
    }
    true
}

/// Truncate to `budget` columns, cutting the tail and marking it with [`ELLIPSIS`].
///
/// Used for titles, where the front of the string carries the meaning. The
/// result is never wider than `budget`; it can be one column narrower when the
/// last cluster that would fit is double-width, because half a glyph is worse
/// than a blank column.
///
/// Whitespace immediately before the cut is dropped, so a cut that lands after
/// a word boundary reads `cargo test…` rather than `cargo test …`.
///
/// `budget == 0` yields an empty string rather than a bare ellipsis, so a
/// zero-width column renders nothing at all.
#[must_use]
pub fn truncate_end(text: &str, budget: usize) -> String {
    if fits(text, budget) {
        return text.to_owned();
    }
    if budget == 0 {
        return String::new();
    }

    let keep = budget - 1;
    let mut used = 0usize;
    let mut end = 0usize;
    for (offset, cluster) in text.grapheme_indices(true) {
        let width = cluster_width(cluster);
        if used + width > keep {
            break;
        }
        used += width;
        end = offset + cluster.len();
    }

    let head = text[..end].trim_end();
    let mut out = String::with_capacity(head.len() + ELLIPSIS.len_utf8());
    out.push_str(head);
    out.push(ELLIPSIS);
    out
}

/// Truncate to `budget` columns by removing the middle.
///
/// Used where both ends carry meaning: paths (root and file name), branch names
/// (prefix and ticket), titles that share a long common prefix. The budget is
/// split down the middle, then any column the tail could not spend is handed
/// back to the head, so a run of double-width characters does not silently
/// waste half the cell: `漢字漢字漢字` in 7 columns is `漢字…字`, not `漢…字`.
/// The head absorbs the slack because a leading fragment disambiguates more
/// often than a trailing one.
///
/// The result is never wider than `budget` and never splits a cluster.
#[must_use]
pub fn truncate_middle(text: &str, budget: usize) -> String {
    if fits(text, budget) {
        return text.to_owned();
    }
    if budget == 0 {
        return String::new();
    }
    if budget == 1 {
        return ELLIPSIS.to_string();
    }

    let available = budget - 1;
    let tail_budget = available / 2;

    // Walk from the right first so the head loop knows where to stop. Without
    // that bound, zero-width clusters (combining marks) would let the head walk
    // past the tail and emit them on both sides of the ellipsis.
    let mut tail_start = text.len();
    let mut tail_used = 0usize;
    for (offset, cluster) in text.grapheme_indices(true).rev() {
        let width = cluster_width(cluster);
        if tail_used + width > tail_budget {
            break;
        }
        tail_used += width;
        tail_start = offset;
    }
    let head_budget = available - tail_used;

    let mut head_end = 0usize;
    let mut used = 0usize;
    for (offset, cluster) in text.grapheme_indices(true) {
        if offset >= tail_start {
            break;
        }
        let width = cluster_width(cluster);
        if used + width > head_budget {
            break;
        }
        used += width;
        head_end = offset + cluster.len();
    }

    let head = text[..head_end].trim_end();
    let tail = text[tail_start..].trim_start();
    let mut out = String::with_capacity(head.len() + ELLIPSIS.len_utf8() + tail.len());
    out.push_str(head);
    out.push(ELLIPSIS);
    out.push_str(tail);
    out
}

/// Strip escape sequences and control characters so a single-line label stays a
/// single line of text.
///
/// Titles reach us from OSC 0/2 sequences written by whatever program the user
/// ran, so they are untrusted. A `\n` would break the sidebar row into two, a
/// `\r` would return the cursor so the second half overwrites the first, and a
/// raw `\x1b` would be re-interpreted as a control sequence by any terminal the
/// label is echoed into.
///
/// An escape is consumed together with the sequence it introduces, not on its
/// own. Dropping only the `\x1b` from `\x1b[31m` leaves the literal text
/// `[31m` in the label, which is the worst of both outcomes: the colour is lost
/// and four columns of noise are kept. CSI sequences run to their final byte,
/// and OSC, DCS, SOS, PM, and APC strings run to `BEL` or `ST`. An unterminated
/// string consumes the rest of the input, which is what a terminal does with it
/// too.
///
/// A control character that is whitespace (`\t`, `\n`, `\r`, vertical tab, form
/// feed, `U+0085`) becomes one space, because it was separating two words and
/// dropping it would run them together. Every other C0 and C1 control is
/// dropped outright. Runs of spaces are left alone here; [`title`] collapses
/// them.
#[inline(always)]
fn is_all_printable_ascii_8(chunk: &[u8; 8]) -> bool {
    let w = u64::from_ne_bytes(*chunk);
    let sub = w.wrapping_sub(0x2020_2020_2020_2020);
    let chk = sub.wrapping_add(0x2121_2121_2121_2121);
    ((sub | chk) & 0x8080_8080_8080_8080) == 0
}

/// Strip escape sequences and control characters so a single-line label stays a
/// single line of text.
///
/// Titles reach us from OSC 0/2 sequences written by whatever program the user
/// ran, so they are untrusted. A `\n` would break the sidebar row into two, a
/// `\r` would return the cursor so the second half overwrites the first, and a
/// raw `\x1b` would be re-interpreted as a control sequence by any terminal the
/// label is echoed into.
///
/// An escape is consumed together with the sequence it introduces, not on its
/// own. Dropping only the `\x1b` from `\x1b[31m` leaves the literal text
/// `[31m` in the label, which is the worst of both outcomes: the colour is lost
/// and four columns of noise are kept. CSI sequences run to their final byte,
/// and OSC, DCS, SOS, PM, and APC strings run to `BEL` or `ST`. An unterminated
/// string consumes the rest of the input, which is what a terminal does with it
/// too.
///
/// A control character that is whitespace (`\t`, `\n`, `\r`, vertical tab, form
/// feed, `U+0085`) becomes one space, because it was separating two words and
/// dropping it would run them together. Every other C0 and C1 control is
/// dropped outright. Runs of spaces are left alone here; [`title`] collapses
/// them.
#[must_use]
pub fn sanitize_line<'a>(text: &'a str) -> Cow<'a, str> {
    let bytes = text.as_bytes();

    // Fast path: SWAR scan for pure printable ASCII input.
    let mut scan = 0;
    while scan + 8 <= bytes.len() {
        let chunk: &[u8; 8] = bytes[scan..scan + 8].try_into().unwrap();
        if !is_all_printable_ascii_8(chunk) {
            break;
        }
        scan += 8;
    }
    if scan == bytes.len() {
        return Cow::Borrowed(text);
    }
    let mut tail_printable = true;
    for &b in &bytes[scan..] {
        if !(0x20..=0x7E).contains(&b) {
            tail_printable = false;
            break;
        }
    }
    if tail_printable {
        return Cow::Borrowed(text);
    }

    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..scan]);
    let mut chars = text[scan..].chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.next() {
                Some('[') => skip_control_sequence(&mut chars),
                Some(']' | 'P' | 'X' | '^' | '_') => skip_string_sequence(&mut chars),
                Some(second) => skip_escape_tail(second, &mut chars),
                None => {}
            },
            '\u{9b}' => skip_control_sequence(&mut chars),
            '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => {
                skip_string_sequence(&mut chars);
            }
            c if !c.is_control() => out.push(c),
            c if c.is_whitespace() => out.push(' '),
            _ => {}
        }
    }
    Cow::Owned(out)
}

/// Consume a CSI sequence's parameters and intermediates up to its final byte.
fn skip_control_sequence(chars: &mut std::str::Chars<'_>) {
    for ch in chars {
        if matches!(ch, '\u{40}'..='\u{7e}') {
            break;
        }
    }
}

/// Consume an OSC, DCS, SOS, PM, or APC string up to `BEL` or `ST`.
fn skip_string_sequence(chars: &mut std::str::Chars<'_>) {
    while let Some(ch) = chars.next() {
        match ch {
            '\u{7}' | '\u{9c}' => break,
            '\u{1b}' => {
                // `ESC \` is the seven-bit form of ST. Any other escape ends the
                // string too, malformed, and its own sequence is left for the
                // caller's loop to handle.
                if chars.clone().next() == Some('\\') {
                    chars.next();
                }
                break;
            }
            _ => {}
        }
    }
}

/// Consume the tail of a two-or-more character escape such as `ESC ( B`.
///
/// `second` has already been taken. If it was an intermediate byte the sequence
/// continues until a final byte; otherwise `second` was the whole tail.
fn skip_escape_tail(second: char, chars: &mut std::str::Chars<'_>) {
    if !matches!(second, '\u{20}'..='\u{2f}') {
        return;
    }
    for ch in chars {
        if !matches!(ch, '\u{20}'..='\u{2f}') {
            break;
        }
    }
}

/// Sanitize, collapse runs of whitespace, trim, then truncate to `budget`.
///
/// The one call a view layer should make for an untrusted single-line title.
#[must_use]
pub fn title(text: &str, budget: usize) -> String {
    let cleaned = sanitize_line(text);
    let mut collapsed = String::with_capacity(cleaned.len());
    let mut pending_space = false;
    for ch in cleaned.chars() {
        if ch.is_whitespace() {
            pending_space = !collapsed.is_empty();
            continue;
        }
        if pending_space {
            collapsed.push(' ');
            pending_space = false;
        }
        collapsed.push(ch);
    }
    truncate_end(&collapsed, budget)
}

/// Pad on the right with spaces to exactly `budget` columns, truncating first.
///
/// For fixed-column layouts where a short label must still consume its cell.
#[must_use]
pub fn pad_end(text: &str, budget: usize) -> String {
    let mut out = truncate_end(text, budget);
    let width = display_width(&out);
    out.extend(std::iter::repeat_n(' ', budget - width));
    out
}
