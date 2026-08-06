//! Byte-exact JSON strings.
//!
//! # The problem the standard has
//!
//! An asciicast event carries its bytes in a JSON string, and a JSON string holds
//! Unicode text. Terminal output is not text: it is bytes. A `git log` of a
//! repository with a Latin-1 commit message, a `cat` of a binary file, a program
//! whose UTF-8 got cut in half by a PTY read boundary, all produce byte sequences
//! that are not valid UTF-8 and therefore have no JSON string that spells them.
//!
//! asciinema resolves this by replacing invalid bytes with U+FFFD. That is fine for
//! watching a recording and fatal for round-tripping one: export then import no
//! longer gives you the session back, and a scrubber built on the imported bytes
//! shows a screen the session never showed.
//!
//! # The resolution
//!
//! [`Utf8Policy::SurrogateEscape`] maps each byte that cannot be decoded to the
//! code point `U+DC00 + byte`, written as a `\udcXX` escape. This is Python's
//! `surrogateescape` convention, and it works here for a specific reason: the
//! surrogate range is not valid Unicode, so no valid UTF-8 and no conforming JSON
//! string can contain those code points by any other route. The mapping is
//! therefore a bijection, and the file stays pure ASCII.
//!
//! A high surrogate followed by a low surrogate is still read as an ordinary
//! surrogate pair, because that is a legal spelling of a non-BMP character and
//! plenty of encoders emit it. Only a *lone* low surrogate in `DC80..DCFF` means a
//! raw byte, which is exactly the range that raw bytes `0x80..0xFF` map into.
//!
//! # Interoperability
//!
//! A player that does not know this convention shows U+FFFD where the escapes are,
//! which is the same thing it would have shown had the bytes been replaced at export
//! time. Nothing is worse for other tools and the recording is exactly reversible
//! for this one. [`Utf8Policy::Replacement`] is available for a caller who wants the
//! lossy form written into the file itself.

use crate::error::CastError;

/// What to do with a byte that is not valid UTF-8.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Utf8Policy {
    /// Map the byte to `U+DC00 + byte` and write it as `\udcXX`.
    ///
    /// Exactly reversible. The default, because a recording you cannot reload is
    /// not a recording.
    #[default]
    SurrogateEscape,
    /// Replace the byte with U+FFFD, as asciinema does.
    ///
    /// Lossy, and chosen only when the file must contain no surrogate escapes.
    Replacement,
}

/// Append `bytes` to `out` as the contents of a JSON string, without the quotes.
pub fn encode(bytes: &[u8], policy: Utf8Policy, out: &mut String) {
    let mut rest = bytes;
    loop {
        match core::str::from_utf8(rest) {
            Ok(text) => {
                encode_str(text, out);
                return;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    // Valid by construction: `valid_up_to` is where decoding stopped.
                    match core::str::from_utf8(&rest[..valid]) {
                        Ok(text) => encode_str(text, out),
                        Err(_) => return,
                    }
                }
                // `None` means the input ended mid-character. Those trailing bytes
                // are individually undecodable, so each one is escaped.
                let bad = error.error_len().unwrap_or(rest.len() - valid);
                for &byte in &rest[valid..valid + bad] {
                    encode_invalid(byte, policy, out);
                }
                rest = &rest[valid + bad..];
            }
        }
    }
}

/// Append valid text with JSON's mandatory escapes applied.
fn encode_str(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            // JSON forbids raw control characters. Terminal output is full of them,
            // ESC most of all, so this branch is the common one.
            c if (c as u32) < 0x20 => push_u_escape(c as u32, out),
            c => out.push(c),
        }
    }
}

/// Append one undecodable byte.
fn encode_invalid(byte: u8, policy: Utf8Policy, out: &mut String) {
    match policy {
        Utf8Policy::SurrogateEscape => push_u_escape(0xdc00 + u32::from(byte), out),
        Utf8Policy::Replacement => out.push('\u{fffd}'),
    }
}

fn push_u_escape(code: u32, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push_str("\\u");
    for shift in [12u32, 8, 4, 0] {
        out.push(HEX[((code >> shift) & 0xf) as usize] as char);
    }
}

/// Decode the contents of a JSON string back to bytes.
///
/// `body` is the text between the quotes, already extracted by the caller.
///
/// # Errors
///
/// [`CastError::EventData`] for a truncated escape, an unknown escape, a malformed
/// `\u`, an unpaired high surrogate, or a lone low surrogate outside the
/// `DC80..DCFF` range this module gives meaning to.
pub fn decode(body: &str, line: usize, out: &mut Vec<u8>) -> Result<(), CastError> {
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buffer = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        let Some(kind) = chars.next() else {
            return Err(CastError::EventData {
                line,
                reason: "the string ends with a lone backslash",
            });
        };
        match kind {
            '"' => out.push(b'"'),
            '\\' => out.push(b'\\'),
            '/' => out.push(b'/'),
            'b' => out.push(0x08),
            'f' => out.push(0x0c),
            'n' => out.push(b'\n'),
            'r' => out.push(b'\r'),
            't' => out.push(b'\t'),
            'u' => {
                let code = hex4(&mut chars, line)?;
                match code {
                    // A high surrogate must be followed by its low half.
                    0xd800..=0xdbff => {
                        if chars.next() != Some('\\') || chars.next() != Some('u') {
                            return Err(CastError::EventData {
                                line,
                                reason: "a high surrogate escape is not followed by \\u",
                            });
                        }
                        let low = hex4(&mut chars, line)?;
                        if !(0xdc00..=0xdfff).contains(&low) {
                            return Err(CastError::EventData {
                                line,
                                reason: "a high surrogate escape is followed by a non-surrogate",
                            });
                        }
                        let scalar =
                            0x1_0000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                        match char::from_u32(scalar) {
                            Some(ch) => {
                                let mut buffer = [0u8; 4];
                                out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
                            }
                            None => {
                                return Err(CastError::EventData {
                                    line,
                                    reason: "a surrogate pair does not name a character",
                                });
                            }
                        }
                    }
                    // The surrogate-escape range: one raw byte each.
                    0xdc80..=0xdcff => out.push((code - 0xdc00) as u8),
                    0xdc00..=0xdfff => {
                        return Err(CastError::EventData {
                            line,
                            reason: "a lone low surrogate outside DC80..DCFF has no meaning",
                        });
                    }
                    _ => match char::from_u32(code) {
                        Some(ch) => {
                            let mut buffer = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
                        }
                        None => {
                            return Err(CastError::EventData {
                                line,
                                reason: "a \\u escape does not name a character",
                            });
                        }
                    },
                }
            }
            _ => {
                return Err(CastError::EventData {
                    line,
                    reason: "unknown escape after a backslash",
                });
            }
        }
    }
    Ok(())
}

/// Read exactly four hex digits.
fn hex4<I>(chars: &mut core::iter::Peekable<I>, line: usize) -> Result<u32, CastError>
where
    I: Iterator<Item = char>,
{
    let mut code = 0u32;
    for _ in 0..4 {
        let Some(digit) = chars.next().and_then(|c| c.to_digit(16)) else {
            return Err(CastError::EventData {
                line,
                reason: "a \\u escape needs four hex digits",
            });
        };
        code = code * 16 + digit;
    }
    Ok(code)
}
