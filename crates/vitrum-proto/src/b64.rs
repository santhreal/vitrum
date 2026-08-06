//! Base64 for the one control-plane field that carries arbitrary bytes.
//!
//! The crate doc explains why live PTY output is a binary frame rather than
//! base64: a 33% tax on the hottest path in the product is not worth paying.
//! That argument is about the firehose, and it does not reach
//! [`ServerMsg::ScrollbackChunk`](crate::ServerMsg::ScrollbackChunk), which
//! answers a deliberate gesture a few times a minute.
//!
//! Serde's default for `Vec<u8>` in JSON is an array of decimal integers, which
//! measures 3.6 bytes of JSON per payload byte on real terminal output. That is
//! a 260% tax, so the encoding chosen to avoid a 33% tax cost eight times more
//! than the thing it avoided. Worse than the size is the shape: `JSON.parse` of
//! a 2 MiB history builds a transient array of two million JavaScript numbers
//! before anything can copy it into the grid, and twenty windows share one
//! `WebKitWebProcess`.
//!
//! No dependency, because this crate is published and forty lines of table
//! lookup is not worth a supply-chain edge on a wire contract.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Standard alphabet (RFC 4648 §4), with padding. Not the URL-safe variant:
/// this rides inside a JSON string, where `+` and `/` need no escaping.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Reverse lookup, `-1` for every byte that is not a base64 symbol. Built at
/// compile time so decoding is a table read rather than a search.
const DECODE: [i8; 256] = {
    let mut table = [-1i8; 256];
    let mut i = 0;
    while i < 64 {
        table[ALPHABET[i] as usize] = i as i8;
        i += 1;
    }
    table
};

/// Why a base64 payload was refused.
///
/// Named cases rather than a bool: these bytes arrive from a socket, and
/// "malformed" with no detail is not something an operator can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Length is not a multiple of four, so the payload is truncated.
    Length(usize),
    /// A byte outside the alphabet.
    Symbol(u8),
    /// Padding somewhere other than the last one or two symbols of the input.
    Padding,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Length(n) => {
                write!(f, "base64 length {n} is not a multiple of four")
            }
            DecodeError::Symbol(b) => {
                write!(f, "byte {b:#04x} is not a base64 symbol")
            }
            DecodeError::Padding => f.write_str("padding is not at the end of the payload"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Encode `data`. Output length is exactly `data.len().div_ceil(3) * 4`.
#[must_use]
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        // Missing bytes read as zero, and the symbols they would have produced
        // are replaced by padding below, so the zero never reaches the output.
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decode `text`. Total: every rejected shape has a named error and there is
/// no panicking path, because this runs on bytes that arrived from a socket.
pub fn decode(text: &str) -> Result<Vec<u8>, DecodeError> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(DecodeError::Length(bytes.len()));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, quad) in bytes.chunks_exact(4).enumerate() {
        let is_last = (index + 1) * 4 == bytes.len();
        let mut symbols = [0u32; 4];
        let mut padding = 0usize;
        for (position, &byte) in quad.iter().enumerate() {
            if byte == b'=' {
                // Only the third and fourth symbols of the final quad may pad.
                // Anywhere else and the payload is spliced or truncated, which
                // would otherwise decode to plausible-looking wrong bytes.
                if !is_last || position < 2 {
                    return Err(DecodeError::Padding);
                }
                padding += 1;
            } else {
                if padding > 0 {
                    return Err(DecodeError::Padding);
                }
                let value = DECODE[byte as usize];
                if value < 0 {
                    return Err(DecodeError::Symbol(byte));
                }
                symbols[position] = value as u32;
            }
        }
        let n = (symbols[0] << 18) | (symbols[1] << 12) | (symbols[2] << 6) | symbols[3];
        out.push((n >> 16) as u8);
        if padding < 2 {
            out.push((n >> 8) as u8);
        }
        if padding < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

/// `#[serde(with = "crate::b64::bytes")]` for a `Vec<u8>` field that must ride
/// in a JSON string instead of an integer array.
pub mod bytes {
    use super::{Deserialize, Deserializer, Serialize, Serializer, decode, encode};

    /// # Errors
    /// Propagates whatever the serializer reports.
    pub fn serialize<S: Serializer>(data: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
        encode(data).serialize(serializer)
    }

    /// # Errors
    /// Fails when the field is not a string, or not valid base64.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        decode(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WHY: this rides on a wire contract, so agreeing with RFC 4648 matters
    /// more than agreeing with itself. Every padding case is here because the
    /// tail is the only place the encoder branches.
    #[test]
    fn the_rfc_test_vectors_encode_exactly() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(plain.as_bytes()), encoded, "encoding {plain:?}");
            assert_eq!(
                decode(encoded).as_deref(),
                Ok(plain.as_bytes()),
                "decoding {encoded:?}"
            );
        }
    }

    /// WHY: the whole reason scrollback cannot be a JSON string is that PTY
    /// bytes are not text. If the encoder only survives UTF-8 it has not
    /// replaced the integer array, it has broken it.
    #[test]
    fn every_byte_value_survives_a_round_trip_at_every_alignment() {
        let all: Vec<u8> = (0..=255u8).collect();
        for skip in 0..4 {
            let payload = &all[skip..];
            let round = decode(&encode(payload)).expect("own output decodes");
            assert_eq!(round, payload, "alignment {skip}");
        }
        // A lone invalid UTF-8 byte and a split multi-byte sequence, which are
        // the two shapes a terminal read boundary actually produces.
        for payload in [&[0x80u8][..], &[0xe2, 0x9c][..], &[0x1b, 0x00, 0xff][..]] {
            assert_eq!(decode(&encode(payload)).as_deref(), Ok(payload));
        }
    }

    /// WHY: a truncated or spliced payload must be refused, not silently
    /// decoded into plausible wrong bytes that would corrupt the grid mid
    /// escape sequence.
    #[test]
    fn malformed_payloads_are_refused_by_named_case() {
        assert_eq!(decode("Zm9"), Err(DecodeError::Length(3)));
        assert_eq!(decode("Zm9vYg="), Err(DecodeError::Length(7)));
        assert_eq!(decode("Zm9*"), Err(DecodeError::Symbol(b'*')));
        assert_eq!(decode("Zm-v"), Err(DecodeError::Symbol(b'-')));
        // Padding in the middle, and padding that starts before the third
        // symbol, are both splices rather than a short tail.
        assert_eq!(decode("Zg==Zg=="), Err(DecodeError::Padding));
        assert_eq!(decode("===="), Err(DecodeError::Padding));
        assert_eq!(decode("Z==g"), Err(DecodeError::Padding));
        assert_eq!(
            DecodeError::Length(3).to_string(),
            "base64 length 3 is not a multiple of four"
        );
        assert_eq!(
            DecodeError::Symbol(b'*').to_string(),
            "byte 0x2a is not a base64 symbol"
        );
    }

    /// WHY: the point of the change is the size, so the size is a contract.
    /// The integer array this replaced measured 3.6 bytes of JSON per payload
    /// byte; base64 must stay at 4/3 or the change did not earn itself.
    #[test]
    fn the_encoding_costs_four_thirds_and_the_array_it_replaced_cost_far_more() {
        let payload: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        let base64_len = encode(&payload).len() + 2; // the two JSON quotes
        let array_len = serde_json::to_string(&payload).expect("a byte array serializes").len();

        assert_eq!(base64_len, 4002, "4/3 of 3000, plus quotes");
        assert!(
            array_len > base64_len * 2,
            "the integer array was {array_len} bytes against {base64_len}"
        );
    }
}
