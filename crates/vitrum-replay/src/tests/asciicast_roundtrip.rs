//! Bytes to file to bytes.

use vitrum_proto::HintState;

use crate::asciicast::{self, Header, Utf8Policy};
use crate::hints;
use crate::replay::Replay;
use crate::stream::Stream;
use crate::tests::support::{CAPTURED, config, grown, linear};
use crate::timeline::{ChunkStamp, Marker, Timeline};

fn export(bytes: &[u8], timeline: &Timeline) -> String {
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    asciicast::to_string(
        &stream,
        timeline,
        &Header::new(80, 24),
        Utf8Policy::SurrogateEscape,
    )
    .expect("writes")
}

/// The real capture round-trips byte for byte.
///
/// This is the acceptance criterion for the whole export path, and the capture is what
/// makes it meaningful: colour escapes, `CR` redraws, Japanese text, a Latin-1 byte pair
/// that is not valid UTF-8, DEC graphics, an alternate-screen excursion, and three OSC
/// hints, all produced by a real PTY rather than by an author who knew what the codec
/// handled.
#[test]
fn the_real_capture_round_trips_byte_for_byte() {
    let text = export(CAPTURED, &Timeline::positional());
    let back = asciicast::read(&text).expect("reads");
    assert_eq!(back.bytes(), CAPTURED);
    assert_eq!(back.header.width, 80);
    assert_eq!(back.header.height, 24);
    assert_eq!(back.header.version, 2);
}

/// The round-tripped bytes replay to the identical screen.
///
/// Byte equality is the mechanism; screen equality is what a user sees. Asserting both
/// means a future change that fixed one by breaking the other cannot pass.
#[test]
fn the_round_tripped_bytes_replay_to_the_identical_screen() {
    let text = export(CAPTURED, &Timeline::positional());
    let back = asciicast::read(&text).expect("reads");
    assert_eq!(
        linear(80, 24, back.bytes()),
        linear(80, 24, CAPTURED),
        "the reloaded recording shows a different screen"
    );
}

/// Every byte value in one stream round-trips through a file.
#[test]
fn a_stream_of_every_byte_value_round_trips() {
    let all: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let text = export(&all, &Timeline::positional());
    let back = asciicast::read(&text).expect("reads");
    assert_eq!(back.bytes(), all.as_slice());
}

/// Chunk boundaries and times survive the round trip exactly.
///
/// The bug: rebuilding the event cut differently on the way back, so a re-export produces
/// a different file and a scrubber's playhead lands at times that shifted. One event per
/// stamp, one stamp per event.
#[test]
fn chunk_boundaries_and_times_survive_exactly() {
    let stamps = vec![
        ChunkStamp { end_seq: 10, micros: 0 },
        ChunkStamp { end_seq: 300, micros: 1_250_000 },
        ChunkStamp {
            end_seq: CAPTURED.len() as u64,
            micros: 9_876_543,
        },
    ];
    let timeline = Timeline::recorded(stamps.clone());

    let text = export(CAPTURED, &timeline);
    let back = asciicast::read(&text).expect("reads");

    assert_eq!(back.bytes(), CAPTURED);
    assert_eq!(back.stamps(), stamps.as_slice());
    assert_eq!(back.timeline().duration_micros(), 9_876_543);
    assert!(back.timeline().has_real_time());
}

/// A re-export of an imported recording is byte-identical to the first export.
///
/// The strongest form of stability: the format is a fixed point, so a recording can be
/// loaded, scrubbed, and saved without drifting.
#[test]
fn a_re_export_is_identical_to_the_first_export() {
    let timeline = Timeline::recorded(vec![
        ChunkStamp { end_seq: 500, micros: 100 },
        ChunkStamp {
            end_seq: CAPTURED.len() as u64,
            micros: 2_000_000,
        },
    ]);
    let first = export(CAPTURED, &timeline);
    let loaded = asciicast::read(&first).expect("reads");

    let bytes = loaded.bytes();
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let second = asciicast::to_string(
        &stream,
        &loaded.timeline(),
        &loaded.header,
        Utf8Policy::SurrogateEscape,
    )
    .expect("writes");

    assert_eq!(first, second);
}

/// Every microsecond value formats and parses back exactly.
///
/// The bug: going through a float formatter. Six fixed decimal places and integer
/// arithmetic on both sides make this exact; `f64` shortest-form printing does not.
#[test]
fn every_time_value_round_trips_through_the_decimal_form() {
    let bytes: &[u8] = b"x";
    for micros in [
        0u64,
        1,
        9,
        999_999,
        1_000_000,
        1_000_001,
        59_999_999,
        3_600_000_000,
        86_400_000_000,
        999_999_999_999,
    ] {
        let timeline = Timeline::recorded(vec![ChunkStamp { end_seq: 1, micros }]);
        let text = export(bytes, &timeline);
        let back = asciicast::read(&text).expect("reads");
        assert_eq!(
            back.stamps(),
            &[ChunkStamp { end_seq: 1, micros }],
            "{micros}us did not survive"
        );
    }
}

/// A stream with no recorded times becomes one event at time zero.
///
/// The honest answer for bytes whose delivery times were never recorded, and it must
/// still carry every byte.
#[test]
fn a_stream_with_no_times_becomes_one_event_at_zero() {
    let text = export(CAPTURED, &Timeline::positional());
    let events: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(events.len(), 1, "one output event and nothing else");
    assert!(events[0].starts_with("[0.000000, \"o\", "));

    let back = asciicast::read(&text).expect("reads");
    assert_eq!(back.bytes(), CAPTURED);
}

/// A synthetic timeline exports a plausible pace and still carries every byte.
#[test]
fn a_synthetic_timeline_exports_several_events_covering_everything() {
    let timeline = Timeline::synthetic(0, CAPTURED.len() as u64, 30_000_000, 10);
    let text = export(CAPTURED, &timeline);
    let events: Vec<&str> = text.lines().skip(1).collect();
    assert_eq!(events.len(), 10);

    let back = asciicast::read(&text).expect("reads");
    assert_eq!(back.bytes(), CAPTURED);
    assert_eq!(back.timeline().duration_micros(), 30_000_000);
}

/// Chapter markers export as `"m"` events and come back as markers.
///
/// asciicast has a marker event type, so vitrum's OSC 7373 chapters survive a trip
/// through the standard format and a player that supports markers shows them.
#[test]
fn chapter_markers_export_and_import() {
    let stream = Stream::new(0, core::slice::from_ref(&CAPTURED));
    let markers = hints::scan(&stream);
    assert_eq!(markers.len(), 3);

    let timeline = Timeline::recorded(vec![
        ChunkStamp { end_seq: 400, micros: 1_000_000 },
        ChunkStamp {
            end_seq: CAPTURED.len() as u64,
            micros: 8_000_000,
        },
    ])
    .with_markers(markers.clone());

    let text = export(CAPTURED, &timeline);
    let marker_lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("\"m\""))
        .collect();
    assert_eq!(marker_lines.len(), 3);

    let back = asciicast::read(&text).expect("reads");
    let labels: Vec<&str> = back
        .markers()
        .iter()
        .map(|marker| marker.label.as_str())
        .collect();
    assert_eq!(
        labels,
        vec!["building vitrum-replay", "force push to main?", "done"]
    );
    assert_eq!(
        back.markers()[0].hint, None,
        "an imported marker carries a label and no state, and says so"
    );
}

/// Every chapter comes back where it happened, even with no recorded times.
///
/// The bug: a stream with no stamps exports as one output event, so writing the markers
/// after it collapsed all of them onto the last byte. A reimported recording then showed
/// every chapter of a session stacked at the end, which is the state the export path was
/// actually in until the writer learnt to cut an event at a marker.
#[test]
fn chapters_keep_their_positions_through_a_timeless_export() {
    let stream = Stream::new(0, core::slice::from_ref(&CAPTURED));
    let found = hints::scan(&stream);
    let expected: Vec<u64> = found.iter().map(|marker| marker.seq).collect();
    assert_eq!(expected.len(), 3);

    let text = export(CAPTURED, &Timeline::positional().with_markers(found));
    let back = asciicast::read(&text).expect("reads");

    assert_eq!(back.bytes(), CAPTURED, "cutting the events changed the bytes");
    let positions: Vec<u64> = back.markers().iter().map(|marker| marker.seq).collect();
    assert_eq!(positions, expected);
}

/// A marker is never written before the bytes that produced it.
///
/// A player walks events in order. A marker ahead of its output would light up a chapter
/// for text that has not been drawn yet.
#[test]
fn a_marker_is_never_written_before_its_own_bytes() {
    let bytes: &[u8] = b"aaaa\x1b]7373;ready;done\x07bbbb";
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let markers = hints::scan(&stream);
    let timeline = Timeline::recorded(vec![
        ChunkStamp { end_seq: 4, micros: 0 },
        ChunkStamp {
            end_seq: bytes.len() as u64,
            micros: 1_000_000,
        },
    ])
    .with_markers(markers);

    let text = export(bytes, &timeline);
    let lines: Vec<&str> = text.lines().skip(1).collect();
    // The hint ends 22 bytes in, so the second chunk is cut there: output up to the
    // marker, the marker, then the rest.
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("\"o\""));
    assert!(lines[1].contains("\"o\""), "the marker waits for its chunk");
    assert!(lines[2].contains("\"m\""));
    assert!(lines[3].contains("\"o\""));

    let back = asciicast::read(&text).expect("reads");
    assert_eq!(
        back.markers()[0].seq,
        22,
        "the marker came back at the end of the recording instead of where it happened"
    );
}

/// A marker past the last stamp is still written.
///
/// The bug: dropping trailing markers because the stamp loop ended first. The last hint of
/// a session, which is the one saying it needs approval, is exactly the one a user wants.
#[test]
fn a_marker_past_the_last_stamp_is_still_written() {
    let bytes: &[u8] = b"aaaabbbb";
    let timeline = Timeline::recorded(vec![ChunkStamp { end_seq: 4, micros: 0 }]).with_markers(
        vec![Marker {
            seq: 8,
            label: "at the very end".into(),
            hint: Some(HintState::Ready),
        }],
    );

    let text = export(bytes, &timeline);
    let back = asciicast::read(&text).expect("reads");
    assert_eq!(back.bytes(), bytes);
    assert_eq!(back.markers().len(), 1);
    assert_eq!(back.markers()[0].label, "at the very end");
}

/// A marker label with non-ASCII text and quotes survives.
#[test]
fn a_marker_label_with_awkward_text_survives() {
    let bytes: &[u8] = b"x";
    let timeline = Timeline::positional().with_markers(vec![Marker {
        seq: 0,
        label: "renommer «café» ? say \"yes\"\\no".into(),
        hint: None,
    }]);

    let text = export(bytes, &timeline);
    let back = asciicast::read(&text).expect("reads");
    assert_eq!(back.markers()[0].label, "renommer «café» ? say \"yes\"\\no");
}

/// A recording loads straight into a replay and scrubs by wall clock.
///
/// The import path exists so a recording can be scrubbed by the same code as a live
/// session, and this is that claim executed end to end.
#[test]
fn an_imported_recording_scrubs_by_wall_clock() {
    let timeline = Timeline::recorded(vec![
        ChunkStamp { end_seq: 200, micros: 0 },
        ChunkStamp { end_seq: 600, micros: 2_000_000 },
        ChunkStamp {
            end_seq: CAPTURED.len() as u64,
            micros: 6_000_000,
        },
    ]);
    let text = export(CAPTURED, &timeline);
    let recording = asciicast::read(&text).expect("reads");

    let bytes = recording.bytes();
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let mut replay = Replay::build(stream, &recording.config().expect("geometry")).expect("build");
    replay.set_timeline(recording.timeline());

    assert!(replay.timeline().has_real_time());

    replay.seek_micros(0).expect("in range");
    assert_eq!(replay.position(), 200);
    replay.seek_micros(1_999_999).expect("in range");
    assert_eq!(replay.position(), 200);
    replay.seek_micros(2_000_000).expect("in range");
    assert_eq!(replay.position(), 600);
    replay.seek_micros(u64::MAX).expect("in range");
    assert_eq!(replay.position(), CAPTURED.len() as u64);

    assert_eq!(replay.screen(), &linear(80, 24, CAPTURED));
}

/// The recording's own geometry is used, not a guess.
///
/// The bug: replaying an 80-column recording at whatever the caller happens to be showing.
/// Every wrapped line would break in the wrong place.
#[test]
fn an_imported_recording_replays_at_its_own_geometry() {
    let bytes: &[u8] = b"0123456789ABCDE";
    let stream = Stream::new(0, core::slice::from_ref(&bytes));
    let text = asciicast::to_string(
        &stream,
        &Timeline::positional(),
        &Header::new(10, 3),
        Utf8Policy::SurrogateEscape,
    )
    .expect("writes");

    let recording = asciicast::read(&text).expect("reads");
    let config = recording.config().expect("geometry");
    assert_eq!((config.cols, config.rows), (10, 3));

    let loaded = recording.bytes();
    let stream = Stream::new(0, core::slice::from_ref(&loaded));
    let mut replay = Replay::build(stream, &config).expect("build");
    let screen = replay.seek(15).expect("in range");
    assert_eq!(screen.line(0).trim_end(), "0123456789");
    assert_eq!(screen.line(1).trim_end(), "ABCDE");
}

/// A large stream round-trips, so nothing depends on the recording being small.
#[test]
fn a_large_stream_round_trips() {
    let bytes = grown(512 * 1024);
    let timeline = Timeline::synthetic(0, bytes.len() as u64, 60_000_000, 128);
    let text = export(&bytes, &timeline);
    let back = asciicast::read(&text).expect("reads");
    assert_eq!(back.bytes().len(), bytes.len());
    assert_eq!(back.bytes(), bytes.as_slice());
    assert_eq!(back.stamps().len(), 128);
}

/// A stream that has evicted a prefix exports the bytes it holds, numbered from zero.
///
/// A recording is self-contained: its first byte is time zero and offset zero. Carrying
/// the daemon's absolute seq into the file would make the times and the offsets disagree
/// for anyone who opened it.
#[test]
fn an_evicted_prefix_exports_as_a_self_contained_recording() {
    let bytes = CAPTURED;
    let stream = Stream::new(9_000_000, core::slice::from_ref(&bytes));
    let timeline = Timeline::recorded(vec![ChunkStamp {
        end_seq: 9_000_000 + bytes.len() as u64,
        micros: 1_000_000,
    }]);
    let text = asciicast::to_string(
        &stream,
        &timeline,
        &Header::new(80, 24),
        Utf8Policy::SurrogateEscape,
    )
    .expect("writes");

    let back = asciicast::read(&text).expect("reads");
    assert_eq!(back.bytes(), CAPTURED);
    assert_eq!(
        back.stamps(),
        &[ChunkStamp {
            end_seq: CAPTURED.len() as u64,
            micros: 1_000_000
        }]
    );
}

/// Replaying a recording exported from a replay agrees with the original.
///
/// End to end: seek, export, import, seek again.
#[test]
fn export_then_import_then_seek_agrees_with_the_original_seek() {
    let bytes = grown(96 * 1024);
    let config = config(80, 24);

    let chunks = [bytes.as_slice()];
    let stream = Stream::new(0, &chunks);
    let mut original = Replay::build(stream, &config).expect("build");

    let timeline = Timeline::synthetic(0, bytes.len() as u64, 20_000_000, 40);
    let text = asciicast::to_string(&stream, &timeline, &Header::new(80, 24), Utf8Policy::SurrogateEscape)
        .expect("writes");
    let recording = asciicast::read(&text).expect("reads");

    let loaded = recording.bytes();
    let loaded_stream = Stream::new(0, core::slice::from_ref(&loaded));
    let mut reloaded = Replay::build(loaded_stream, &config).expect("build");

    for target in [0u64, 1, 5_000, 40_000, bytes.len() as u64 / 2, bytes.len() as u64] {
        let before = original.seek(target).expect("in range").clone();
        let after = reloaded.seek(target).expect("in range");
        assert_eq!(&before, after, "seek to {target} differs after a round trip");
    }
}
