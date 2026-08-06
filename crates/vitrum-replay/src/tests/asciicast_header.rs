//! Header requirements, and what survives a round trip.
//!
//! The header is the only place a recording says how wide it was. Getting it wrong is
//! not a cosmetic failure: the same bytes are a different screen at a different width,
//! so a header that is guessed at, defaulted, or dropped produces a replay that is
//! confidently wrong. The other half of this file is the unknown-key contract, because
//! asciinema keeps adding header keys and a reader that discarded the ones it did not
//! model would damage every recording it touched.

use crate::asciicast::{self, Header, Utf8Policy};
use crate::error::CastError;
use crate::stream::Stream;
use crate::timeline::{ChunkStamp, Timeline};

/// Read `text` and unwrap, for the cases where reading is expected to work.
fn read_ok(text: &str) -> asciicast::Recording {
    asciicast::read(text).expect("reads")
}

/// Write one output event under `header` and return the header line only.
fn header_line(header: &Header) -> String {
    let bytes: &[u8] = b"hi";
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let text = asciicast::to_string(
        &stream,
        &Timeline::positional(),
        header,
        Utf8Policy::SurrogateEscape,
    )
    .expect("writes");
    text.lines().next().expect("a header line").to_owned()
}

/// The three required keys are read back exactly as written.
///
/// The bug this stops: reading `width` and `height` off the first event, or off a
/// default, instead of off the header. Both produce a plausible screen at the wrong
/// geometry.
#[test]
fn the_required_keys_are_read_back_exactly() {
    for (cols, rows) in [(1u16, 1u16), (80, 24), (200, 50), (u16::MAX, u16::MAX)] {
        let text = format!("{{\"version\":2,\"width\":{cols},\"height\":{rows}}}\n");
        let recording = read_ok(&text);

        assert_eq!(recording.header.version, 2);
        assert_eq!(recording.header.width, cols, "width at {cols}x{rows}");
        assert_eq!(recording.header.height, rows, "height at {cols}x{rows}");
    }
}

/// A header missing `width`, missing `height`, or naming zero for either is refused.
///
/// `serde` defaults an absent integer to zero here, so absent and zero arrive at the
/// reader as the same value and must be refused by the same check. Accepting zero would
/// hand `CellGrid` a geometry it cannot build, one layer further away from the file that
/// caused it.
#[test]
fn a_header_without_a_usable_geometry_is_refused() {
    let cases = [
        ("{\"version\":2,\"height\":24}", "width absent"),
        ("{\"version\":2,\"width\":80}", "height absent"),
        ("{\"version\":2}", "both absent"),
        ("{\"version\":2,\"width\":0,\"height\":24}", "width zero"),
        ("{\"version\":2,\"width\":80,\"height\":0}", "height zero"),
    ];

    for (head, what) in cases {
        assert_eq!(
            asciicast::read(&format!("{head}\n")),
            Err(CastError::MissingGeometry),
            "{what} was accepted"
        );
    }
}

/// A geometry too large for the grid fails when the config is asked for, not silently.
///
/// The header is honest about what the file says; `config()` is where the geometry meets
/// the grid. Keeping the two apart means a caller can still read the markers and the
/// bytes out of a recording it cannot replay.
#[test]
fn an_unbuildable_geometry_surfaces_from_config_not_from_read() {
    let recording = read_ok("{\"version\":2,\"width\":65535,\"height\":65535}\n");

    assert_eq!(recording.header.width, u16::MAX);
    assert!(
        recording.config().is_err(),
        "a 65535x65535 grid was accepted"
    );
}

/// A version that is not 2 names the version it found.
///
/// The bug this stops: reading v1 as v2. v1 is a single JSON document with delta times
/// and no event lines, so it would parse as a valid, empty, silent recording.
#[test]
fn a_wrong_version_is_refused_and_names_itself() {
    for found in [0u64, 1, 3, 99] {
        let text = format!("{{\"version\":{found},\"width\":80,\"height\":24}}\n");
        assert_eq!(
            asciicast::read(&text),
            Err(CastError::Version { found }),
            "version {found} was accepted"
        );
    }
}

/// The version check runs before the geometry check, so a v1 file says "version".
///
/// A v1 header has no `width`, so both checks fire. Reporting the geometry would send
/// the user to add a key to a file that is the wrong format entirely.
#[test]
fn a_v1_file_is_reported_as_a_version_problem_not_a_geometry_one() {
    let v1 = "{\"version\":1,\"stdout\":[[0.1,\"hi\"]]}\n";

    assert_eq!(
        asciicast::read(v1),
        Err(CastError::Version { found: 1 })
    );
}

/// Optional metadata is read into its own field rather than into `extra`.
#[test]
fn the_modelled_optional_keys_are_read_into_their_fields() {
    let text = "{\"version\":2,\"width\":80,\"height\":24,\
                \"timestamp\":1700000000,\"duration\":12.5,\"idle_time_limit\":2.0,\
                \"command\":\"cargo test\",\"title\":\"a run\",\
                \"env\":{\"TERM\":\"xterm-256color\"}}\n";
    let header = read_ok(text).header;

    assert_eq!(header.timestamp, Some(1_700_000_000));
    assert_eq!(header.duration, Some(12.5));
    assert_eq!(header.idle_time_limit, Some(2.0));
    assert_eq!(header.command.as_deref(), Some("cargo test"));
    assert_eq!(header.title.as_deref(), Some("a run"));
    assert_eq!(header.env, Some(serde_json::json!({"TERM": "xterm-256color"})));
    assert!(
        header.extra.is_empty(),
        "a modelled key leaked into extra: {:?}",
        header.extra
    );
}

/// A key this crate does not model is kept, and written back with its value intact.
///
/// The bug this stops: `#[serde(deny_unknown_fields)]`, or a hand-built header struct
/// that drops what it does not recognise. Either one silently strips metadata from every
/// recording made by a newer asciinema than this reader, and the loss is invisible until
/// someone compares the file to the one they started with.
#[test]
fn an_unmodelled_key_survives_a_round_trip_with_its_value() {
    let text = "{\"version\":2,\"width\":80,\"height\":24,\
                \"vitrum_session\":\"abc-123\",\"future_flag\":true,\"nested\":{\"a\":[1,2]}}\n";
    let header = read_ok(text).header;

    assert_eq!(header.extra.len(), 3);
    assert_eq!(header.extra["vitrum_session"], serde_json::json!("abc-123"));
    assert_eq!(header.extra["future_flag"], serde_json::json!(true));
    assert_eq!(header.extra["nested"], serde_json::json!({"a": [1, 2]}));

    let written = read_ok(&format!("{}\n", header_line(&header))).header;
    assert_eq!(written.extra, header.extra, "extra keys did not survive");
}

/// Writing forces version 2 whatever the caller's header said.
///
/// A caller that built a `Header` by hand and left `version` at zero would otherwise
/// write a file this crate's own reader refuses.
#[test]
fn writing_forces_version_two() {
    let mut header = Header::new(80, 24);
    header.version = 0;

    assert_eq!(read_ok(&format!("{}\n", header_line(&header))).header.version, 2);
}

/// Writing fills in `duration` when the timeline has real times and the caller did not.
///
/// A player draws its progress bar from `duration` before it has parsed the events, so a
/// recording without one shows a bar that jumps as the file loads.
#[test]
fn a_recorded_timeline_supplies_the_duration() {
    let timeline = Timeline::recorded(vec![
        ChunkStamp { end_seq: 1, micros: 500_000 },
        ChunkStamp { end_seq: 2, micros: 2_250_000 },
    ]);
    let bytes: &[u8] = b"hi";
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let text = asciicast::to_string(
        &stream,
        &timeline,
        &Header::new(80, 24),
        Utf8Policy::SurrogateEscape,
    )
    .expect("writes");

    assert_eq!(read_ok(&text).header.duration, Some(2.25));
}

/// A caller's own `duration` is not overwritten.
#[test]
fn an_explicit_duration_is_left_alone() {
    let timeline = Timeline::recorded(vec![ChunkStamp { end_seq: 2, micros: 2_250_000 }]);
    let mut header = Header::new(80, 24);
    header.duration = Some(99.0);
    let bytes: &[u8] = b"hi";
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let text = asciicast::to_string(
        &stream,
        &timeline,
        &header,
        Utf8Policy::SurrogateEscape,
    )
    .expect("writes");

    assert_eq!(read_ok(&text).header.duration, Some(99.0));
}

/// A positional timeline writes no duration, because there is no clock to measure.
///
/// Emitting a zero here would tell a player the recording is instantaneous.
#[test]
fn a_positional_timeline_writes_no_duration() {
    assert_eq!(read_ok(&format!("{}\n", header_line(&Header::new(80, 24)))).header.duration, None);
}

/// Unset optional keys are omitted from the file rather than written as null.
///
/// `"title": null` is legal JSON and asciinema does not write it; a player that checks
/// for the key's presence would see a title that is not there.
#[test]
fn unset_optional_keys_are_omitted_entirely() {
    let line = header_line(&Header::new(80, 24));

    for key in ["timestamp", "duration", "idle_time_limit", "command", "title", "env", "theme"] {
        assert!(!line.contains(key), "{key} was written while unset: {line}");
    }
}

/// The builders set exactly the field they name.
#[test]
fn the_header_builders_set_one_field_each() {
    let header = Header::new(120, 40)
        .with_title("a run")
        .with_timestamp(1_700_000_000);

    assert_eq!(header.width, 120);
    assert_eq!(header.height, 40);
    assert_eq!(header.title.as_deref(), Some("a run"));
    assert_eq!(header.timestamp, Some(1_700_000_000));
    assert_eq!(header.command, None);
}
