//! Every way a recording can be rejected.
//!
//! A reader that guessed at a malformed file would produce a replay that looks right and
//! shows something the session never showed. Every error here names the 1-based line, so
//! a user handed a broken file can find the line in an editor.

use crate::asciicast::read;
use crate::error::CastError;

const HEAD: &str = "{\"version\":2,\"width\":80,\"height\":24}";

fn with_events(events: &[&str]) -> String {
    let mut text = String::from(HEAD);
    for event in events {
        text.push('\n');
        text.push_str(event);
    }
    text
}

/// An empty file is rejected rather than read as a zero-length recording.
#[test]
fn an_empty_file_is_rejected() {
    assert_eq!(read(""), Err(CastError::Empty));
}

/// A header that is not JSON is rejected with the parser's own message.
#[test]
fn a_header_that_is_not_json_is_rejected_with_its_reason() {
    let error = read("not json\n").expect_err("must fail");
    match error {
        CastError::HeaderSyntax { message } => {
            assert!(!message.is_empty(), "the reason has to say something");
        }
        other => panic!("expected a header syntax error, got {other:?}"),
    }
}

/// A version other than 2 is rejected rather than read hopefully.
///
/// v1 is a completely different shape: a single JSON document with a `stdout` array of
/// *delta* times. Reading it as v2 would produce zero events and an empty recording.
#[test]
fn a_version_other_than_two_is_rejected() {
    assert_eq!(
        read("{\"version\":1,\"width\":80,\"height\":24}\n"),
        Err(CastError::Version { found: 1 })
    );
    assert_eq!(
        read("{\"version\":3,\"width\":80,\"height\":24}\n"),
        Err(CastError::Version { found: 3 })
    );
    assert_eq!(
        read("{\"width\":80,\"height\":24}\n"),
        Err(CastError::Version { found: 0 }),
        "an absent version is not an implicit 2"
    );
}

/// A missing or zero geometry is rejected.
///
/// The same bytes are a different screen at a different width, so a recording without one
/// cannot be replayed at all. Defaulting to 80x24 would silently rewrap every line of a
/// recording made at 200 columns.
#[test]
fn a_missing_or_zero_geometry_is_rejected() {
    for header in [
        "{\"version\":2,\"height\":24}",
        "{\"version\":2,\"width\":80}",
        "{\"version\":2,\"width\":0,\"height\":24}",
        "{\"version\":2,\"width\":80,\"height\":0}",
    ] {
        assert_eq!(
            read(&format!("{header}\n")),
            Err(CastError::MissingGeometry),
            "{header}"
        );
    }
}

/// An event that is not a three-element array is rejected, naming its line.
#[test]
fn a_malformed_event_line_is_rejected_with_its_line_number() {
    for (events, line) in [
        (vec!["[0.0, \"o\", \"a\"]", "not an array"], 3),
        (vec!["0.0, \"o\", \"a\"]"], 2),
        (vec!["[0.0, \"o\", \"a\""], 2),
        (vec!["[0.0, \"o\"]"], 2),
        (vec!["[0.0 \"o\" \"a\"]"], 2),
        (vec!["[0.0, \"o\", \"a\"] trailing"], 2),
        (vec!["[0.0, \"o\", unquoted]"], 2),
    ] {
        assert_eq!(
            read(&with_events(&events)),
            Err(CastError::EventShape { line }),
            "{events:?}"
        );
    }
}

/// A time that is not a finite non-negative decimal is rejected.
///
/// A negative time would make the timeline non-monotonic; `NaN` would make every
/// comparison false and a binary search return anything at all.
#[test]
fn a_bad_time_is_rejected() {
    for time in ["-1.0", "abc", "NaN", "Infinity", "", ".5", "+1.0", "1.2.3"] {
        let events = [format!("[{time}, \"o\", \"a\"]")];
        let refs: Vec<&str> = events.iter().map(String::as_str).collect();
        assert_eq!(
            read(&with_events(&refs)),
            Err(CastError::EventTime { line: 2 }),
            "time {time:?} should have been refused"
        );
    }
}

/// Times going backwards are rejected rather than silently reordered.
///
/// v2 times are absolute and monotonic. A file whose times go backwards makes a scrubber
/// seek to the wrong place with no visible error, which is worse than refusing the file.
#[test]
fn times_going_backwards_are_rejected() {
    assert_eq!(
        read(&with_events(&["[1.000000, \"o\", \"a\"]", "[0.500000, \"o\", \"b\"]"])),
        Err(CastError::EventTimeOrder {
            line: 3,
            micros: 500_000,
            previous: 1_000_000,
        })
    );
}

/// Equal times are accepted, because two reads in the same microsecond happen.
#[test]
fn equal_times_are_accepted() {
    let recording = read(&with_events(&[
        "[0.000000, \"o\", \"a\"]",
        "[0.000000, \"o\", \"b\"]",
    ]))
    .expect("reads");
    assert_eq!(recording.bytes(), b"ab");
}

/// A type code that is not exactly one character is rejected.
#[test]
fn a_bad_type_code_is_rejected() {
    for code in ["", "oo", "\\u0000\\u0000"] {
        let events = [format!("[0.0, \"{code}\", \"a\"]")];
        let refs: Vec<&str> = events.iter().map(String::as_str).collect();
        assert_eq!(
            read(&with_events(&refs)),
            Err(CastError::EventCode { line: 2 }),
            "code {code:?} should have been refused"
        );
    }
}

/// A malformed data string is rejected, naming its line and what was wrong.
#[test]
fn a_malformed_data_string_is_rejected_with_a_reason() {
    assert_eq!(
        read(&with_events(&["[0.0, \"o\", \"a\\udc00\"]"])),
        Err(CastError::EventData {
            line: 2,
            reason: "a lone low surrogate outside DC80..DCFF has no meaning",
        })
    );
    assert_eq!(
        read(&with_events(&["[0.0, \"o\", \"a\\q\"]"])),
        Err(CastError::EventData {
            line: 2,
            reason: "unknown escape after a backslash",
        })
    );
}

/// A resize event with data that is not `COLSxROWS` is rejected.
#[test]
fn a_malformed_resize_event_is_rejected() {
    assert_eq!(
        read(&with_events(&["[0.0, \"r\", \"eighty by twenty\"]"])),
        Err(CastError::EventData {
            line: 2,
            reason: "a resize event's data is not \"COLSxROWS\"",
        })
    );
}

/// A well-formed resize event is recorded with its position and its new size.
#[test]
fn a_resize_event_is_recorded_with_its_position() {
    let recording = read(&with_events(&[
        "[0.000000, \"o\", \"abc\"]",
        "[1.000000, \"r\", \"120x40\"]",
        "[2.000000, \"o\", \"def\"]",
    ]))
    .expect("reads");

    assert_eq!(recording.bytes(), b"abcdef");
    assert_eq!(recording.resizes().len(), 1);
    let resize = recording.resizes()[0];
    assert_eq!((resize.cols, resize.rows), (120, 40));
    assert_eq!(resize.seq, 3, "after the three bytes already delivered");
    assert_eq!(resize.micros, 1_000_000);
}

/// Input events are counted and kept out of the byte stream.
///
/// The bug: merging them in. The terminal never received the user's keystrokes as output;
/// the echo already in the stream is what it showed. Merging would print every keystroke
/// twice.
#[test]
fn input_events_are_counted_and_not_merged_into_the_output() {
    let recording = read(&with_events(&[
        "[0.000000, \"o\", \"prompt$ \"]",
        "[0.100000, \"i\", \"ls\"]",
        "[0.200000, \"o\", \"ls\"]",
    ]))
    .expect("reads");

    assert_eq!(recording.bytes(), b"prompt$ ls");
    assert_eq!(recording.input_events(), 1);
}

/// An unknown event code is skipped and counted rather than rejecting the file.
///
/// asciinema has added codes over time. Refusing an unfamiliar one would reject valid
/// files from newer recorders; hiding the fact would misreport what the file contains.
#[test]
fn an_unknown_event_code_is_skipped_and_counted() {
    let recording = read(&with_events(&[
        "[0.000000, \"o\", \"a\"]",
        "[1.000000, \"x\", \"0\"]",
        "[2.000000, \"o\", \"b\"]",
    ]))
    .expect("reads");

    assert_eq!(recording.bytes(), b"ab");
    assert_eq!(recording.skipped_events(), 1);
}

/// Blank lines are ignored, because a file that grew by appending often has one.
#[test]
fn blank_lines_are_ignored() {
    let recording = read("{\"version\":2,\"width\":80,\"height\":24}\n\n[0.0, \"o\", \"a\"]\n\n")
        .expect("reads");
    assert_eq!(recording.bytes(), b"a");
}

/// Whitespace inside an event line is tolerated.
///
/// A recording written by a different encoder may space its arrays differently, and the
/// format does not forbid it.
#[test]
fn whitespace_inside_an_event_line_is_tolerated() {
    let recording = read(&with_events(&["[  0.500000 ,   \"o\" ,  \"hi\"  ]"])).expect("reads");
    assert_eq!(recording.bytes(), b"hi");
    assert_eq!(recording.stamps()[0].micros, 500_000);
}

/// A time in exponent notation is read, with a documented trip through `f64`.
///
/// asciinema does not write this form; a hand-written or machine-generated file might, and
/// refusing it would reject a file that is valid JSON and unambiguous.
#[test]
fn a_time_in_exponent_notation_is_read() {
    let recording = read(&with_events(&["[1.5e-3, \"o\", \"a\"]"])).expect("reads");
    assert_eq!(recording.stamps()[0].micros, 1_500);
}

/// More than six fraction digits are truncated, never rounded up past the next event.
///
/// Rounding up could make one event overtake the next and turn a valid file into a
/// non-monotonic timeline.
#[test]
fn extra_fraction_digits_are_truncated() {
    let recording = read(&with_events(&[
        "[0.0000009, \"o\", \"a\"]",
        "[0.000001, \"o\", \"b\"]",
    ]))
    .expect("reads");
    assert_eq!(recording.stamps()[0].micros, 0);
    assert_eq!(recording.stamps()[1].micros, 1);
}

/// An integer time with no fraction is read.
#[test]
fn an_integer_time_is_read() {
    let recording = read(&with_events(&["[3, \"o\", \"a\"]"])).expect("reads");
    assert_eq!(recording.stamps()[0].micros, 3_000_000);
}

/// A header line with no events reads as an empty recording rather than an error.
///
/// asciinema writes the header the moment recording starts, so a session killed
/// immediately produces exactly this file.
#[test]
fn a_header_with_no_events_reads_as_an_empty_recording() {
    let recording = read(&format!("{HEAD}\n")).expect("reads");
    assert!(recording.bytes().is_empty());
    assert!(recording.stamps().is_empty());
    assert_eq!(recording.header.width, 80);
}

/// Every error's message names the value and the line, so it can be acted on.
#[test]
fn every_error_message_says_what_to_look_at() {
    let cases = [
        (CastError::Version { found: 1 }, vec!["version 1", "version 2"]),
        (
            CastError::EventTimeOrder {
                line: 9,
                micros: 5,
                previous: 10,
            },
            vec!["line 9", "5us", "10us"],
        ),
        (CastError::EventShape { line: 4 }, vec!["line 4"]),
        (
            CastError::EventData {
                line: 6,
                reason: "unknown escape after a backslash",
            },
            vec!["line 6", "unknown escape"],
        ),
    ];
    for (error, fragments) in cases {
        let text = error.to_string();
        for fragment in fragments {
            assert!(
                text.contains(fragment),
                "{text:?} should mention {fragment:?}"
            );
        }
    }
}
