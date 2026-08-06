//! Seq to time and back, and the honesty flag.

use vitrum_proto::HintState;

use crate::timeline::{ChunkStamp, Marker, Timeline};

fn stamps() -> Vec<ChunkStamp> {
    vec![
        ChunkStamp { end_seq: 100, micros: 0 },
        ChunkStamp { end_seq: 250, micros: 500_000 },
        ChunkStamp { end_seq: 400, micros: 5_000_000 },
    ]
}

/// A timeline with no recorded stamps says so, and one with stamps says so.
///
/// This flag is the difference between a scrubber that reads "3.2s" and one that reads
/// "40%", and only one of those is true on a daemon that does not record chunk times.
/// A UI cannot tell the difference without being told, so being told is the contract.
#[test]
fn a_timeline_states_whether_its_times_are_real() {
    assert!(!Timeline::positional().has_real_time());
    assert!(Timeline::recorded(stamps()).has_real_time());
    assert!(
        !Timeline::synthetic(0, 1000, 10_000_000, 10).has_real_time(),
        "a spread-evenly clock is invented, and must not claim otherwise"
    );
}

/// Every byte of a chunk carries that chunk's delivery time, with no interpolation.
///
/// The bug: interpolating within a chunk. Every byte of one PTY read arrived together as
/// far as anything outside the kernel can tell, and inventing sub-chunk timing would put
/// a scrubber's playhead at times that never happened.
#[test]
fn every_byte_of_a_chunk_carries_that_chunks_time() {
    let timeline = Timeline::recorded(stamps());

    assert_eq!(timeline.micros_at(0), Some(0));
    assert_eq!(timeline.micros_at(99), Some(0));
    assert_eq!(
        timeline.micros_at(100),
        Some(500_000),
        "seq 100 is the first byte of the second chunk"
    );
    assert_eq!(timeline.micros_at(249), Some(500_000));
    assert_eq!(
        timeline.micros_at(150),
        Some(500_000),
        "the middle of a chunk has the chunk's time, not a blend"
    );
    assert_eq!(timeline.micros_at(399), Some(5_000_000));
}

/// Past the last stamp there is no time, which is distinguishable from time zero.
///
/// The bug: returning zero for "no answer". A scrubber would then jump its playhead to
/// the start whenever the byte ring ran ahead of the stamp ring.
#[test]
fn past_the_last_stamp_there_is_no_time_rather_than_time_zero() {
    let timeline = Timeline::recorded(stamps());
    assert_eq!(timeline.micros_at(400), None);
    assert_eq!(timeline.micros_at(u64::MAX), None);
    assert_eq!(timeline.micros_at(0), Some(0), "and zero is still a real answer");
}

/// A time maps back to the end of the last chunk delivered by then.
///
/// The bug: mapping to the *start* of the chunk that contains the time, which shows a
/// screen missing the output that had already arrived.
#[test]
fn a_time_maps_to_the_end_of_the_last_chunk_delivered_by_then() {
    let timeline = Timeline::recorded(stamps());

    assert_eq!(timeline.seq_at(0, 0), 100, "the first chunk was already delivered");
    assert_eq!(timeline.seq_at(499_999, 0), 100);
    assert_eq!(timeline.seq_at(500_000, 0), 250);
    assert_eq!(timeline.seq_at(4_999_999, 0), 250);
    assert_eq!(timeline.seq_at(5_000_000, 0), 400);
    assert_eq!(timeline.seq_at(u64::MAX, 0), 400, "clamped to the end");
}

/// Before the first stamp, a time maps to the start of the retained stream.
///
/// The base seq is a parameter for exactly this: a ring that has evicted 9 MiB starts at
/// 9 MiB, and answering zero would name a byte the ring no longer holds.
#[test]
fn before_the_first_stamp_a_time_maps_to_the_streams_own_base() {
    let timeline = Timeline::recorded(vec![ChunkStamp {
        end_seq: 10_000_500,
        micros: 1_000,
    }]);
    assert_eq!(timeline.seq_at(0, 10_000_000), 10_000_000);
    assert_eq!(timeline.seq_at(999, 10_000_000), 10_000_000);
    assert_eq!(timeline.seq_at(1_000, 10_000_000), 10_000_500);
}

/// Out-of-order or duplicate stamps are dropped, keeping both binary searches sound.
///
/// The bug: trusting the input. A stamp ring that lost an entry to eviction, or a
/// caller pushing a retry, would otherwise make `partition_point` return nonsense and a
/// scrubber jump backwards mid-drag.
#[test]
fn out_of_order_stamps_are_dropped_rather_than_corrupting_the_search() {
    let timeline = Timeline::recorded(vec![
        ChunkStamp { end_seq: 100, micros: 1_000 },
        ChunkStamp { end_seq: 90, micros: 2_000 },
        ChunkStamp { end_seq: 100, micros: 3_000 },
        ChunkStamp { end_seq: 200, micros: 500 },
        ChunkStamp { end_seq: 300, micros: 4_000 },
    ]);

    assert_eq!(
        timeline.stamps(),
        &[
            ChunkStamp { end_seq: 100, micros: 1_000 },
            ChunkStamp { end_seq: 300, micros: 4_000 },
        ]
    );
    assert_eq!(timeline.duration_micros(), 4_000);
}

/// Pushing a stamp says whether it was kept.
///
/// A caller following a live session needs to know its stamp was rejected, rather than
/// discovering later that the timeline is missing entries.
#[test]
fn pushing_a_stamp_reports_whether_it_was_kept() {
    let mut timeline = Timeline::recorded(Vec::new());
    assert!(timeline.push(ChunkStamp { end_seq: 10, micros: 5 }));
    assert!(timeline.push(ChunkStamp { end_seq: 20, micros: 9 }));
    assert!(
        !timeline.push(ChunkStamp { end_seq: 15, micros: 20 }),
        "a seq that goes backwards is refused"
    );
    assert!(
        !timeline.push(ChunkStamp { end_seq: 30, micros: 1 }),
        "a time that goes backwards is refused"
    );
    assert_eq!(timeline.stamps().len(), 2);
}

/// A synthetic timeline spreads a duration evenly and reaches both ends exactly.
///
/// The bug: an off-by-one in the spread that leaves the last event short of the stream's
/// end, so exporting a session drops its final bytes.
#[test]
fn a_synthetic_timeline_reaches_both_ends_of_the_stream() {
    let timeline = Timeline::synthetic(1_000, 2_000, 4_000_000, 4);
    assert_eq!(
        timeline.stamps(),
        &[
            ChunkStamp { end_seq: 1_250, micros: 1_000_000 },
            ChunkStamp { end_seq: 1_500, micros: 2_000_000 },
            ChunkStamp { end_seq: 1_750, micros: 3_000_000 },
            ChunkStamp { end_seq: 2_000, micros: 4_000_000 },
        ]
    );
    assert_eq!(timeline.duration_micros(), 4_000_000);
}

/// A synthetic timeline with zero steps still produces one, rather than nothing.
///
/// Nothing would mean an export with no events at all, which is a file that loses the
/// whole session.
#[test]
fn a_synthetic_timeline_with_no_steps_still_produces_one() {
    let timeline = Timeline::synthetic(0, 500, 1_000, 0);
    assert_eq!(timeline.stamps().len(), 1);
    assert_eq!(timeline.stamps()[0].end_seq, 500);
}

/// Markers sort by seq however they were supplied, and navigate both ways.
///
/// A "jump to next event" control that walked an unsorted list would skip chapters.
#[test]
fn markers_sort_by_seq_and_navigate_both_ways() {
    let timeline = Timeline::positional().with_markers(vec![
        Marker { seq: 300, label: "ready".into(), hint: Some(HintState::Ready) },
        Marker { seq: 100, label: "working".into(), hint: Some(HintState::Working) },
        Marker { seq: 200, label: "approval".into(), hint: Some(HintState::Approval) },
    ]);

    let seqs: Vec<u64> = timeline.markers().iter().map(|marker| marker.seq).collect();
    assert_eq!(seqs, vec![100, 200, 300]);

    assert!(timeline.marker_at_or_before(99).is_none());
    assert_eq!(
        timeline.marker_at_or_before(100).map(|marker| marker.label.as_str()),
        Some("working")
    );
    assert_eq!(
        timeline.marker_at_or_before(299).map(|marker| marker.label.as_str()),
        Some("approval")
    );
    assert_eq!(
        timeline.marker_after(100).map(|marker| marker.seq),
        Some(200),
        "`after` is strict, so it does not return the marker you are standing on"
    );
    assert!(timeline.marker_after(300).is_none());
}

/// An empty timeline answers every question without panicking.
#[test]
fn an_empty_timeline_answers_everything_safely() {
    let timeline = Timeline::positional();
    assert_eq!(timeline.duration_micros(), 0);
    assert_eq!(timeline.micros_at(0), None);
    assert_eq!(timeline.seq_at(0, 77), 77);
    assert_eq!(timeline.seq_at(u64::MAX, 77), 77);
    assert!(timeline.markers().is_empty());
    assert!(timeline.marker_at_or_before(0).is_none());
    assert!(timeline.marker_after(0).is_none());
    assert_eq!(timeline.heap_bytes(), 0);
}

/// The reported memory cost accounts for the stamps and the marker labels.
#[test]
fn the_reported_memory_cost_accounts_for_stamps_and_labels() {
    let timeline = Timeline::recorded(stamps()).with_markers(vec![Marker {
        seq: 1,
        label: "a fairly long operator facing label".into(),
        hint: None,
    }]);
    let stamp_bytes = 3 * core::mem::size_of::<ChunkStamp>();
    assert!(timeline.heap_bytes() >= stamp_bytes + 35);
}
