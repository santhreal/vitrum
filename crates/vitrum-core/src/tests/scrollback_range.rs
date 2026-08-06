//! `Scrollback::range` semantics: what is served, what is refused, and the
//! difference between "your history is gone" and "you are caught up".

use crate::Scrollback;
use crate::tests::helpers::pattern;

/// An evicted offset must be refused, not silently answered with newer bytes.
/// Splicing here is the worst possible failure: the client would paint bytes at
/// the wrong offset and every subsequent frame would be misaligned.
#[test]
fn range_returns_none_for_an_evicted_offset() {
    let mut sb = Scrollback::with_capacity(4);
    sb.push(b"01234567");
    assert_eq!(sb.oldest_seq(), 4);
    assert!(sb.range(0, 4).is_none());
    assert!(sb.range(3, 1).is_none(), "one byte before oldest is gone");
}

/// The oldest retained byte must be servable. An off-by-one that refuses it
/// makes a client unable to page back to the start of its own history.
#[test]
fn range_returns_some_for_the_oldest_retained_byte() {
    let mut sb = Scrollback::with_capacity(4);
    sb.push(b"01234567");
    assert_eq!(sb.range(4, 1).unwrap(), b"4");
    assert_eq!(sb.range(4, 4).unwrap(), b"4567");
}

/// Asking at the head must return an empty vector, not `None`. The caller uses
/// `None` to mean "resync, your history is gone"; conflating the two would make
/// a caught-up client throw away its viewport.
#[test]
fn range_at_head_seq_is_empty_not_none() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"abc");
    assert_eq!(sb.range(3, 16).unwrap(), b"");
}

/// An offset past the head is not a real position and must be refused, so a
/// client bug or a corrupted frame cannot make the server serve garbage.
#[test]
fn range_past_head_seq_is_none() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"abc");
    assert!(sb.range(4, 1).is_none());
    assert!(sb.range(u64::MAX, 1).is_none());
}

/// `max` is a ceiling, not a promise. Asking for more than is retained must
/// return what exists rather than over-reading the buffer.
#[test]
fn range_clamps_max_to_the_retained_tail() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"abcde");
    assert_eq!(sb.range(2, 1000).unwrap(), b"cde");
    assert_eq!(sb.range(0, 2).unwrap(), b"ab");
}

/// A zero-length request must be answerable, because a client that has nothing
/// to backfill still needs a positive answer to proceed.
#[test]
fn range_with_max_zero_is_empty_but_valid() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"abcde");
    assert_eq!(sb.range(0, 0).unwrap(), b"");
    assert_eq!(sb.range(5, 0).unwrap(), b"");
    assert!(sb.range(6, 0).is_none(), "still out of range at max 0");
}

/// A range spanning the wrap point must come back contiguous and in order. If
/// the two halves are copied in the wrong order the client sees the newest bytes
/// first, which looks like scrambled output rather than an error.
#[test]
fn range_across_the_seam_is_contiguous() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"01234567");
    sb.push(b"abcd");
    assert_eq!(sb.oldest_seq(), 4);
    assert_eq!(sb.range(4, 8).unwrap(), b"4567abcd");
    assert_eq!(sb.range(6, 4).unwrap(), b"67ab", "starts before the seam");
    assert_eq!(sb.range(8, 4).unwrap(), b"abcd", "starts at the seam");
}

/// Every sub-range of a wrapped ring must agree with the flat expected bytes.
/// This catches modulo mistakes that only show up at particular offsets.
#[test]
fn every_subrange_of_a_wrapped_ring_matches() {
    let cap = 16;
    let mut sb = Scrollback::with_capacity(cap);
    let src = pattern(40);
    sb.push(&src[..10]);
    sb.push(&src[10..29]);
    sb.push(&src[29..]);
    let oldest = sb.oldest_seq();
    assert_eq!(oldest, 24);
    for start in 0..cap {
        for len in 0..=(cap - start) {
            let from = oldest + start as u64;
            let got = sb
                .range(from, len)
                .unwrap_or_else(|| panic!("range({from}, {len}) must be servable"));
            assert_eq!(
                got,
                &src[24 + start..24 + start + len],
                "range({from}, {len}) mismatch"
            );
        }
    }
}

/// A fresh ring must answer offset 0 with an empty vector and refuse anything
/// else, so a client attaching before any output behaves like a caught-up one.
#[test]
fn range_on_an_untouched_ring() {
    let sb = Scrollback::with_capacity(8);
    assert_eq!(sb.range(0, 8).unwrap(), b"");
    assert!(sb.range(1, 1).is_none());
}

/// The two halves must reconstruct exactly what `range` returns, in every ring
/// state.
///
/// The halves accessor exists so a cross-session sweep can scan 200 MB without
/// copying it. That only works if the seam is the ONLY thing the caller has to
/// handle: an off-by-one in the split silently drops or duplicates bytes at the
/// wrap, which is precisely where a search would then miss a line.
#[test]
fn the_halves_reconstruct_the_retained_bytes_in_every_ring_state() {
    let cap = 64;
    // Never written, growing, exactly full, wrapped once, wrapped many times.
    for total in [0usize, 1, 40, cap, cap + 1, cap + 7, cap * 5 + 3] {
        let mut sb = Scrollback::with_capacity(cap);
        let data = pattern(total);
        // Pushed in awkward slices so the wrap lands mid-push, which is what a
        // PTY read boundary does.
        for piece in data.chunks(7).filter(|c| !c.is_empty()) {
            sb.push(piece);
        }
        let (first, second) = sb.halves();
        let mut joined = Vec::with_capacity(first.len() + second.len());
        joined.extend_from_slice(first);
        joined.extend_from_slice(second);

        assert_eq!(
            joined.len(),
            sb.len(),
            "halves must cover exactly the retained bytes at total={total}"
        );
        let expected = sb
            .range(sb.oldest_seq(), sb.len())
            .expect("the whole retained range is retained");
        assert_eq!(
            joined, expected,
            "halves disagree with range at total={total}"
        );
        assert_eq!(
            joined,
            data[data.len() - joined.len()..],
            "the retained bytes must be the newest ones at total={total}"
        );
    }
}

/// A ring that has not wrapped must be entirely in the first half.
///
/// The caller pays for a seam only when there is one; a growing ring must not
/// hand back a spurious split that makes every line look like it straddles.
#[test]
fn an_unwrapped_ring_has_no_seam() {
    let mut sb = Scrollback::with_capacity(64);
    sb.push(b"still growing");
    let (first, second) = sb.halves();
    assert_eq!(first, b"still growing");
    assert!(second.is_empty());
}

/// A wrapped ring must split at the wrap, oldest run first.
#[test]
fn a_wrapped_ring_splits_at_the_wrap() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"abcdefgh");
    sb.push(b"ijk");
    let (first, second) = sb.halves();
    assert_eq!(first, b"defgh", "the older run runs to the end of storage");
    assert_eq!(second, b"ijk", "the newer run is the wrapped remainder");
}

/// A zero-capacity ring must report two empty runs rather than panicking.
#[test]
fn a_ring_with_no_capacity_has_two_empty_halves() {
    let mut sb = Scrollback::with_capacity(0);
    sb.push(b"discarded");
    assert_eq!(sb.halves(), (&[][..], &[][..]));
    assert_eq!(sb.head_seq(), 9, "seq still counts what was thrown away");
}

/// After eviction the halves must be positioned by the TRUE oldest seq, not by
/// zero.
///
/// A cross-session sweep reports every hit as an absolute offset computed from
/// the base seq of the first half. If the ring reported 0 after wrapping, every
/// offset in every hit would be wrong by the number of evicted bytes, and the
/// error would be invisible until someone clicked a result and landed in the
/// wrong place.
#[test]
fn the_halves_are_positioned_by_the_true_oldest_seq() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"abcdefgh");
    sb.push(b"ijk");
    assert_eq!(sb.oldest_seq(), 3, "three bytes were evicted");
    assert_eq!(sb.head_seq(), 11);

    let (first, second) = sb.halves();
    // Reading a hit at index 2 of the joined halves must resolve to seq 5.
    let mut joined = Vec::new();
    joined.extend_from_slice(first);
    joined.extend_from_slice(second);
    assert_eq!(joined, b"defghijk");
    assert_eq!(joined[2], b'f');
    assert_eq!(
        sb.range(sb.oldest_seq() + 2, 1).expect("retained"),
        b"f",
        "index 2 of the halves is seq oldest_seq()+2"
    );
}
