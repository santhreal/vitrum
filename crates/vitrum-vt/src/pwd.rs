//! Turning an OSC 7 report into a path.
//!
//! A shell announces where it is with `OSC 7 ; file://host/path ST`, and the
//! engine hands that payload over exactly as it arrived: still a URL, still
//! percent-encoded, still carrying whatever hostname the program chose to put
//! in it. Every consumer wants the same thing out of it, and a second decoder
//! written later would be a second set of bugs, so the rule lives here once.

use std::path::PathBuf;

/// Decode an OSC 7 payload into a local path.
///
/// Returns `None` for anything that is not a `file://` URL, because that is the
/// only scheme that names a directory this machine could actually be in.
///
/// The hostname is deliberately ignored rather than checked. A program can put
/// any string there, so matching it proves nothing, and asking the operating
/// system for our own hostname to compare against costs a dependency for a
/// weaker guarantee than the caller can get by simply asking whether the
/// directory is there.
#[must_use]
pub fn pwd_path(raw: &str) -> Option<PathBuf> {
    let rest = raw.strip_prefix("file://")?;
    // Everything up to the first slash is the authority. An empty authority
    // (`file:///path`) is the common case and leaves `rest` starting at the
    // slash, which is already the path.
    let path = match rest.find('/') {
        Some(i) => &rest[i..],
        // `file://host` with no path names no directory.
        None => return None,
    };
    let decoded = percent_decode(path)?;
    if decoded.is_empty() {
        return None;
    }
    // A Windows path arrives as `/C:/Users/me`, which is not a path any API
    // here will open. The drive letter makes it unambiguous: a leading slash
    // before `X:` is URL syntax rather than a root.
    let decoded = match decoded.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &decoded[1..],
        _ => decoded.as_str(),
    };
    Some(PathBuf::from(decoded))
}

/// Percent-decode a URL path.
///
/// Returns `None` when an escape is truncated or is not hexadecimal, rather
/// than passing the raw bytes through. A malformed escape means the sender and
/// this decoder disagree about the string, and guessing which bytes were meant
/// is how a path silently becomes a different path.
///
/// The result is required to be UTF-8 for the same reason: a path built from
/// bytes nobody can display is not a path worth showing an operator.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    // Nothing to decode is the overwhelmingly common case, and it is worth not
    // allocating for: a shell sends one of these on every prompt.
    if !bytes.contains(&b'%') {
        return Some(s.to_string());
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = *bytes.get(i + 1)?;
            let lo = *bytes.get(i + 2)?;
            let byte = (hex(hi)? << 4) | hex(lo)?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// One hex digit's value, or `None` if it is not one.
fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
