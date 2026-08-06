//! Seq to byte mapping across a ring's seam.
//!
//! Every other suite in this crate trusts [`Stream`] to hand back exactly the bytes
//! between two seqs. If it is off by one, or silently drops the second half of a
//! ring, every seek is wrong in a way that looks like an emulator bug.

use crate::stream::Stream;
use crate::tests::support::CAPTURED;

/// A stream that has evicted bytes still numbers what it kept by absolute seq.
///
/// The bug: treating a range as an offset into the retained bytes. A ring that has
/// evicted 10 MiB would then hand back bytes the session wrote at the very start of
/// its life for a seek to the newest byte, and every replayed screen would be from
/// the wrong era of the session.
#[test]
fn a_range_is_absolute_seq_not_an_offset_into_what_was_kept() {
    let bytes: &[u8] = b"0123456789";
    let stream = Stream::new(10_000_000, core::slice::from_ref(&bytes));

    assert_eq!(stream.base_seq(), 10_000_000);
    assert_eq!(stream.head_seq(), 10_000_010);
    assert_eq!(stream.to_vec(10_000_003..10_000_007), b"3456");
    assert_eq!(stream.byte_at(10_000_009), Some(b'9'));
    assert_eq!(stream.byte_at(10_000_010), None);
}

/// A range spanning the join between the ring's two halves yields both halves, in
/// order, with nothing lost or repeated at the boundary.
///
/// The bug this locks out: walking only the chunk the range starts in. The join lands
/// mid-line and mid-sequence, so losing the tail silently truncates a colour escape
/// and every colour after it in the replay is wrong.
#[test]
fn a_range_across_the_ring_join_yields_both_halves_in_order() {
    let older: &[u8] = b"\x1b[31mred";
    let newer: &[u8] = b" text\x1b[0m";
    let chunks = [older, newer];
    let stream = Stream::new(0, &chunks);

    assert_eq!(stream.len(), 17);
    let slices: Vec<&[u8]> = stream.slices(0..17).collect();
    assert_eq!(slices, vec![older, newer]);
    assert_eq!(stream.to_vec(0..17), b"\x1b[31mred text\x1b[0m");

    // A range wholly inside the second chunk still starts where it should.
    assert_eq!(stream.to_vec(11..17), b"xt\x1b[0m");
    // A range that straddles the join by one byte on each side.
    assert_eq!(stream.to_vec(7..10), b"d t");
}

/// An empty chunk in the middle does not end iteration.
///
/// A ring whose head half has not wrapped yet hands over a zero-length slice, and a
/// walker that stopped on it would report the whole session as empty.
#[test]
fn an_empty_chunk_does_not_end_the_walk() {
    let first: &[u8] = b"abc";
    let empty: &[u8] = b"";
    let last: &[u8] = b"def";
    let chunks = [first, empty, last];
    let stream = Stream::new(5, &chunks);

    assert!(!stream.is_empty());
    assert_eq!(stream.to_vec(5..11), b"abcdef");
    let slices: Vec<&[u8]> = stream.slices(5..11).collect();
    assert_eq!(slices, vec![first, last], "the empty chunk is skipped, not yielded");
}

/// A range outside the stream clamps to nothing instead of panicking.
///
/// A scrubber asks for ranges derived from a timeline that may name seqs the ring has
/// since evicted. Clamping keeps that a visible empty read rather than a crash in a
/// UI thread.
#[test]
fn an_out_of_range_request_clamps_instead_of_panicking() {
    let bytes: &[u8] = b"abcdef";
    let stream = Stream::new(100, core::slice::from_ref(&bytes));

    assert_eq!(stream.to_vec(0..50), b"", "entirely before the retained window");
    assert_eq!(stream.to_vec(200..300), b"", "entirely after it");
    assert_eq!(stream.to_vec(0..103), b"abc", "clipped at the start");
    assert_eq!(stream.to_vec(103..500), b"def", "clipped at the end");
    // An inverted range is empty, not a wrap-around read. The ends are bound
    // first because a literal backwards range is a lint, and here it is the
    // point of the test: a scrubber that swaps its ends must read nothing.
    let (start, end) = (104, 102);
    assert_eq!(stream.to_vec(start..end), b"");
}

/// Both ends of the retained window are seekable, and nothing outside is.
#[test]
fn holds_accepts_both_ends_of_the_window_and_nothing_past_it() {
    let bytes: &[u8] = b"abcdef";
    let stream = Stream::new(100, core::slice::from_ref(&bytes));

    assert!(stream.holds(100), "the oldest retained byte");
    assert!(stream.holds(106), "one past the newest, meaning `everything so far`");
    assert!(!stream.holds(99));
    assert!(!stream.holds(107));
}

/// Splitting the capture at every byte and reading it back yields the capture.
///
/// This is the property the ring actually exercises: the join can fall anywhere,
/// including inside a UTF-8 character and inside an escape sequence, and the walk must
/// not care.
#[test]
fn every_possible_ring_join_reads_back_the_whole_capture() {
    for split in 0..CAPTURED.len() {
        let (older, newer) = CAPTURED.split_at(split);
        let chunks = [older, newer];
        let stream = Stream::new(0, &chunks);
        assert_eq!(
            stream.to_vec(0..CAPTURED.len() as u64),
            CAPTURED,
            "join at byte {split} lost or duplicated bytes"
        );
    }
}
