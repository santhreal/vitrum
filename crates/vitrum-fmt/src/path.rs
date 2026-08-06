//! Path display: `~/src/foo`, and `~/src/…/crates/vitrum-fmt` when the column
//! budget runs out.
//!
//! # What is preserved and why
//!
//! A project path is read from both ends. The first component says which root
//! it lives under (`~`, `/opt`, `D:`) and the last says which thing it is.
//! Everything between is the part a reader skims. So [`shorten`] elides whole
//! middle components and keeps the first component plus as many trailing
//! components as the budget allows, rather than cutting characters off one end.
//!
//! When even `first/…/last` does not fit, it falls back to a character-level
//! middle truncation, which still keeps both ends visible. It never returns
//! something wider than the budget, and never splits a double-width glyph.
//!
//! # Separators
//!
//! Both `/` and `\` are recognised as separators on every target, because a
//! Windows daemon reports `C:\Users\...` to a client that may be laying it out
//! anywhere. The original separators are preserved verbatim in the output:
//! elision splices the original string rather than re-joining components, so a
//! Windows path stays a Windows path.
//!
//! Home matching is case-insensitive for Windows-shaped paths (a drive letter
//! or a backslash) and case-sensitive otherwise, matching how the two families
//! of filesystem actually behave.

use crate::text::{self, ELLIPSIS};

/// Replace a leading home directory with `~`.
///
/// `("/home/mk/src/foo", "/home/mk")` becomes `~/src/foo`, the home directory
/// itself becomes `~`, and anything outside home is returned unchanged. An
/// empty `home`, or a path that already starts with `~`, is a no-op.
///
/// A trailing separator on `home` is ignored, so a `HOME` of `/home/mk/` works.
#[must_use]
pub fn home_relative(path: &str, home: &str) -> String {
    match home_relative_str(path, home) {
        HomeRelative::Unchanged => path.to_owned(),
        HomeRelative::Home => "~".to_owned(),
        HomeRelative::Under(suffix) => {
            let mut out = String::with_capacity(1 + suffix.len());
            out.push('~');
            out.push_str(suffix);
            out
        }
    }
}

enum HomeRelative<'a> {
    Unchanged,
    Home,
    Under(&'a str),
}

fn home_relative_str<'a>(path: &'a str, home: &str) -> HomeRelative<'a> {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() || path.starts_with('~') {
        return HomeRelative::Unchanged;
    }
    if path.len() < home.len() {
        return HomeRelative::Unchanged;
    }

    let fold_case = windows_shaped(path) || windows_shaped(home);
    let path_head = &path.as_bytes()[..home.len()];
    let matches = path_head
        .iter()
        .zip(home.as_bytes())
        .all(|(&a, &b)| normalize_byte(a, fold_case) == normalize_byte(b, fold_case));
    if !matches {
        return HomeRelative::Unchanged;
    }

    match path.as_bytes().get(home.len()) {
        None => HomeRelative::Home,
        Some(b'/' | b'\\') => HomeRelative::Under(&path[home.len()..]),
        Some(_) => HomeRelative::Unchanged,
    }
}

fn normalize_byte(byte: u8, fold_case: bool) -> u8 {
    let byte = if byte == b'\\' { b'/' } else { byte };
    if fold_case {
        byte.to_ascii_lowercase()
    } else {
        byte
    }
}

fn windows_shaped(path: &str) -> bool {
    if path.contains('\\') {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Shorten to `budget` columns by eliding middle components.
///
/// Returns the path unchanged when it already fits. Otherwise keeps the first
/// component and the longest run of trailing components that fits, joined by
/// `…`. Falls back to [`text::truncate_middle`] when no component split fits,
/// which covers a single very long component and every degenerate budget.
#[must_use]
pub fn shorten(path: &str, budget: usize) -> String {
    if text::fits(path, budget) {
        return path.to_owned();
    }
    let parts = components(path);
    if parts.len() < 3 {
        return text::truncate_middle(path, budget);
    }

    // Everything up to and including the separator after the first component.
    let head = &path[..parts[1].0];
    let head_width = text::display_width(head) + 1;
    if head_width > budget {
        return text::truncate_middle(path, budget);
    }

    // `k` is the first component kept after the elision, so `k == 2` keeps the
    // most and each step drops one more from the front of the tail.
    for k in 2..parts.len() {
        let tail = &path[parts[k - 1].1..];
        if head_width + text::display_width(tail) <= budget {
            let mut out = String::with_capacity(head.len() + ELLIPSIS.len_utf8() + tail.len());
            out.push_str(head);
            out.push(ELLIPSIS);
            out.push_str(tail);
            return out;
        }
    }

    text::truncate_middle(path, budget)
}

/// [`home_relative`] then [`shorten`]: the one call a sidebar row needs.
#[must_use]
pub fn shorten_home_relative(path: &str, home: &str, budget: usize) -> String {
    match home_relative_str(path, home) {
        HomeRelative::Unchanged => shorten(path, budget),
        HomeRelative::Home => text::truncate_end("~", budget),
        HomeRelative::Under(suffix) => {
            let mut display = String::with_capacity(1 + suffix.len());
            display.push('~');
            display.push_str(suffix);
            shorten(&display, budget)
        }
    }
}

/// The last path component, ignoring trailing separators.
///
/// `/home/mk/src/foo/` and `/home/mk/src/foo` both yield `foo`. A path made
/// only of separators yields the original string, because a project label of
/// `""` is worse than `/`.
#[must_use]
pub fn base_name(path: &str) -> &str {
    match components(path).last() {
        Some(&(start, end)) => &path[start..end],
        None => path,
    }
}

/// Byte ranges of the non-separator runs in `path`.
///
/// Both `/` and `\` split. Scanning bytes is safe for UTF-8: a continuation
/// byte is always `>= 0x80` and can never be mistaken for either separator.
fn components(path: &str) -> Vec<(usize, usize)> {
    let bytes = path.as_bytes();
    let mut parts = Vec::with_capacity(8);
    let mut start: Option<usize> = None;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'/' || byte == b'\\' {
            if let Some(begin) = start.take() {
                parts.push((begin, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(begin) = start {
        parts.push((begin, bytes.len()));
    }
    parts
}
