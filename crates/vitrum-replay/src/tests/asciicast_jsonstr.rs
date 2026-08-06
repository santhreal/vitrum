//! The byte-exact JSON string codec.

use crate::asciicast::jsonstr::{Utf8Policy, decode, encode};
use crate::error::CastError;

fn round_trip(bytes: &[u8]) -> Vec<u8> {
    let mut text = String::new();
    encode(bytes, Utf8Policy::SurrogateEscape, &mut text);
    let mut back = Vec::new();
    decode(&text, 2, &mut back).expect("decodes");
    back
}

fn encoded(bytes: &[u8]) -> String {
    let mut text = String::new();
    encode(bytes, Utf8Policy::SurrogateEscape, &mut text);
    text
}

/// The characters JSON forbids raw are escaped, and `ESC` most of all.
///
/// The bug: emitting a raw `ESC` inside a JSON string. It is legal-looking and every
/// strict parser rejects the file, which means a recording that plays in one player and
/// fails in the next.
#[test]
fn control_characters_are_escaped_as_json_requires() {
    assert_eq!(encoded(b"\x1b[31m"), "\\u001b[31m");
    assert_eq!(encoded(b"\n\r\t"), "\\n\\r\\t");
    assert_eq!(encoded(b"\x08\x0c"), "\\b\\f");
    assert_eq!(encoded(b"\x00\x01\x1f"), "\\u0000\\u0001\\u001f");
    assert_eq!(encoded(b"\x7f"), "\u{7f}", "DEL is legal raw and stays raw");
}

/// Quotes and backslashes are escaped, so a JSON line cannot be broken by output.
///
/// The bug: not escaping a backslash. A program printing a Windows path would terminate
/// the string early and the rest of the line would be parsed as JSON, which is how a
/// recording gets truncated at the first path.
#[test]
fn quotes_and_backslashes_are_escaped() {
    assert_eq!(encoded(br#"say "hi""#), r#"say \"hi\""#);
    assert_eq!(encoded(br"C:\Users\x"), r"C:\\Users\\x");
    assert_eq!(round_trip(br#"a\"b"#), br#"a\"b"#);
}

/// Valid UTF-8 is written as itself, so a normal player shows it normally.
///
/// Escaping everything would be safe and would also make the file unreadable and three
/// times larger.
#[test]
fn valid_utf8_is_written_verbatim() {
    assert_eq!(encoded("日本語 café ✓".as_bytes()), "日本語 café ✓");
    assert_eq!(round_trip("日本語 café ✓".as_bytes()), "日本語 café ✓".as_bytes());
}

/// A byte that is not valid UTF-8 becomes a `\udcXX` escape and comes back exactly.
///
/// This is the whole reason this module exists. Without it, exporting a session with a
/// Latin-1 commit message in its `git log` and importing it back gives different bytes,
/// and every screen after that point in the replay is wrong.
#[test]
fn an_invalid_byte_becomes_a_surrogate_escape_and_returns_exactly() {
    assert_eq!(encoded(b"\xff"), "\\udcff");
    assert_eq!(encoded(b"\x80"), "\\udc80");
    assert_eq!(encoded(b"a\xffb\xfec"), "a\\udcffb\\udcfec");
    assert_eq!(round_trip(b"\xff\xfe\x80\x9f"), b"\xff\xfe\x80\x9f");
}

/// Every single byte value round-trips, alone and in a run.
///
/// The exhaustive form. A codec with one hole in it fails on exactly the byte nobody
/// thought to try.
#[test]
fn every_byte_value_round_trips_alone_and_in_a_run() {
    for byte in 0u8..=255 {
        assert_eq!(round_trip(&[byte]), vec![byte], "byte {byte:#04x} alone");
    }
    let all: Vec<u8> = (0u8..=255).collect();
    assert_eq!(round_trip(&all), all, "all 256 values in one run");
    let reversed: Vec<u8> = (0u8..=255).rev().collect();
    assert_eq!(round_trip(&reversed), reversed);
}

/// A truncated UTF-8 character at the end of the input is escaped byte by byte.
///
/// A PTY read boundary lands mid-character constantly, and an exporter that dropped the
/// partial bytes would lose the character on reassembly.
#[test]
fn a_truncated_character_at_the_end_is_escaped_byte_by_byte() {
    let bytes = "日".as_bytes();
    let cut = &bytes[..2];
    assert_eq!(encoded(cut), "\\udce6\\udc97");
    assert_eq!(round_trip(cut), cut);

    // And the two halves, exported separately, reassemble into the character.
    let mut whole = round_trip(&bytes[..1]);
    whole.extend_from_slice(&round_trip(&bytes[1..]));
    assert_eq!(whole, bytes);
}

/// A truncated character followed by more bytes is escaped without eating them.
///
/// The bug: consuming to the end of the input on an incomplete sequence. Everything after
/// the bad byte would vanish from the recording.
#[test]
fn a_truncated_character_does_not_consume_what_follows() {
    assert_eq!(encoded(b"\xe6\x97ok"), "\\udce6\\udc97ok");
    assert_eq!(round_trip(b"\xe6\x97ok"), b"\xe6\x97ok");
}

/// A real surrogate pair is read as one non-BMP character, not as two raw bytes.
///
/// The bug: treating every low surrogate as a raw byte. `\ud83d\ude00` is a legal
/// spelling of an emoji that plenty of encoders emit, and reading it as two bytes would
/// corrupt the character and everything after it.
#[test]
fn a_surrogate_pair_decodes_as_one_character() {
    let mut out = Vec::new();
    decode("\\ud83d\\ude00", 2, &mut out).expect("decodes");
    assert_eq!(out, "\u{1f600}".as_bytes());

    // Escaped in the same range this module uses for raw bytes, but as a proper pair.
    let mut pair = Vec::new();
    decode("\\ud801\\udc80", 2, &mut pair).expect("decodes");
    assert_eq!(pair, "\u{10480}".as_bytes());
}

/// Ordinary `\u` escapes decode to their character.
#[test]
fn ordinary_unicode_escapes_decode_to_their_character() {
    let mut out = Vec::new();
    decode("\\u0041\\u00e9\\u65e5", 2, &mut out).expect("decodes");
    assert_eq!(out, "Aé日".as_bytes());
}

/// A lone low surrogate outside the byte range is refused rather than guessed at.
///
/// `\udc00` is not a byte under this convention (byte values start at `0x80`) and is not
/// a character. Inventing a meaning for it would let a malformed file decode into bytes
/// the recorder never wrote.
#[test]
fn a_lone_low_surrogate_outside_the_byte_range_is_refused() {
    let mut out = Vec::new();
    assert_eq!(
        decode("\\udc00", 7, &mut out),
        Err(CastError::EventData {
            line: 7,
            reason: "a lone low surrogate outside DC80..DCFF has no meaning",
        })
    );
    assert_eq!(
        decode("\\udd00", 7, &mut out),
        Err(CastError::EventData {
            line: 7,
            reason: "a lone low surrogate outside DC80..DCFF has no meaning",
        })
    );
}

/// A high surrogate with no low half is refused.
#[test]
fn an_unpaired_high_surrogate_is_refused() {
    let mut out = Vec::new();
    assert_eq!(
        decode("\\ud83d", 3, &mut out),
        Err(CastError::EventData {
            line: 3,
            reason: "a high surrogate escape is not followed by \\u",
        })
    );
    assert_eq!(
        decode("\\ud83dX", 3, &mut out),
        Err(CastError::EventData {
            line: 3,
            reason: "a high surrogate escape is not followed by \\u",
        })
    );
    assert_eq!(
        decode("\\ud83d\\u0041", 3, &mut out),
        Err(CastError::EventData {
            line: 3,
            reason: "a high surrogate escape is followed by a non-surrogate",
        })
    );
}

/// A malformed or truncated escape is refused with the line that carried it.
#[test]
fn a_malformed_escape_is_refused_and_names_its_line() {
    let mut out = Vec::new();
    assert_eq!(
        decode("abc\\", 11, &mut out),
        Err(CastError::EventData {
            line: 11,
            reason: "the string ends with a lone backslash",
        })
    );
    assert_eq!(
        decode("\\u00", 11, &mut out),
        Err(CastError::EventData {
            line: 11,
            reason: "a \\u escape needs four hex digits",
        })
    );
    assert_eq!(
        decode("\\uZZZZ", 11, &mut out),
        Err(CastError::EventData {
            line: 11,
            reason: "a \\u escape needs four hex digits",
        })
    );
    assert_eq!(
        decode("\\q", 11, &mut out),
        Err(CastError::EventData {
            line: 11,
            reason: "unknown escape after a backslash",
        })
    );
}

/// The replacement policy is lossy, and the test says so out loud.
///
/// This is documented behaviour, not a defect, and pinning it keeps anyone from
/// "fixing" the default by making the exact policy lossy too.
#[test]
fn the_replacement_policy_is_deliberately_lossy() {
    let mut text = String::new();
    encode(b"a\xffb", Utf8Policy::Replacement, &mut text);
    assert_eq!(text, "a\u{fffd}b");

    let mut back = Vec::new();
    decode(&text, 2, &mut back).expect("decodes");
    assert_ne!(back, b"a\xffb", "the byte is gone, which is the point");
    assert_eq!(back, "a\u{fffd}b".as_bytes());
}

/// The escape output is pure ASCII, so a file written this way is safe to transport
/// anywhere.
#[test]
fn surrogate_escapes_keep_the_output_ascii() {
    let all: Vec<u8> = (0x80u8..=0xff).collect();
    let text = encoded(&all);
    assert!(text.is_ascii(), "escapes must not themselves need encoding");
}
