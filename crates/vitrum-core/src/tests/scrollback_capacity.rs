//! Eviction behaviour of the bounded ring: what is kept, what is dropped, and
//! where the boundary sits.

use crate::Scrollback;
use crate::tests::helpers::pattern;

/// Locks out an off-by-one that evicts while there is still room. A push that
/// exactly fills the ring must keep every byte; if this regresses, a client
/// with a 10 MB ring silently loses the first line of every session.
#[test]
fn push_exactly_at_capacity_evicts_nothing() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"01234567");
    assert_eq!(sb.len(), 8);
    assert_eq!(sb.oldest_seq(), 0);
    assert_eq!(sb.head_seq(), 8);
    assert_eq!(sb.range(0, 8).unwrap(), b"01234567");
}

/// Locks out the opposite off-by-one: one byte past capacity must evict exactly
/// one byte, not a whole chunk and not zero. If this regresses, scrollback
/// either loses far more history than asked or grows without bound.
#[test]
fn push_one_over_capacity_evicts_exactly_one_byte() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"01234567");
    sb.push(b"8");
    assert_eq!(sb.len(), 8);
    assert_eq!(sb.oldest_seq(), 1);
    assert_eq!(sb.head_seq(), 9);
    assert_eq!(sb.range(1, 8).unwrap(), b"12345678");
    assert!(
        sb.range(0, 1).is_none(),
        "byte 0 was evicted and must not be served"
    );
}

/// A single push bigger than the whole ring must keep its tail, not its head and
/// not panic. A 1 MB burst into a 64 KB ring is ordinary agent behaviour, and
/// keeping the head would show the client stale bytes forever.
#[test]
fn single_push_larger_than_capacity_keeps_only_the_tail() {
    let mut sb = Scrollback::with_capacity(4);
    sb.push(b"0123456789");
    assert_eq!(sb.len(), 4);
    assert_eq!(sb.head_seq(), 10, "seq counts every byte, evicted or not");
    assert_eq!(sb.oldest_seq(), 6);
    assert_eq!(sb.range(6, 4).unwrap(), b"6789");
    assert!(sb.range(5, 1).is_none());
}

/// A single push of exactly capacity into an empty ring must fill it without
/// wrapping. This is the seam between the growth path and the ring path; getting
/// it wrong corrupts the first full buffer.
#[test]
fn single_push_of_exactly_capacity_fills_without_wrapping() {
    let mut sb = Scrollback::with_capacity(6);
    sb.push(b"abcdef");
    assert_eq!(sb.oldest_seq(), 0);
    assert_eq!(sb.range(0, 6).unwrap(), b"abcdef");
}

/// A push of exactly capacity onto a full ring must replace all of it. If the
/// ring index does not advance by the full length, old and new bytes interleave.
#[test]
fn push_of_capacity_onto_a_full_ring_replaces_everything() {
    let mut sb = Scrollback::with_capacity(4);
    sb.push(b"0123");
    sb.push(b"4567");
    assert_eq!(sb.oldest_seq(), 4);
    assert_eq!(sb.head_seq(), 8);
    assert_eq!(sb.range(4, 4).unwrap(), b"4567");
}

/// A push that straddles the end of the buffer must wrap into two copies and
/// still read back in order. This is the single most likely place for a ring to
/// corrupt output, and corruption here looks like garbled escape sequences.
#[test]
fn push_across_the_ring_seam_reads_back_in_order() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"01234567");
    sb.push(b"abc");
    assert_eq!(sb.oldest_seq(), 3);
    assert_eq!(sb.head_seq(), 11);
    assert_eq!(sb.range(3, 8).unwrap(), b"34567abc");
}

/// Repeated wrapping must never drift. Ten wraps of an 8-byte ring keeps the
/// last 8 bytes; drift by even one byte would splice output at a random offset.
#[test]
fn repeated_wrapping_keeps_exactly_the_last_capacity_bytes() {
    let mut sb = Scrollback::with_capacity(8);
    for i in 0..30u8 {
        sb.push(&[b'a' + i % 26]);
    }
    assert_eq!(sb.len(), 8);
    assert_eq!(sb.head_seq(), 30);
    assert_eq!(sb.oldest_seq(), 22);
    let expected: Vec<u8> = (22..30u8).map(|i| b'a' + i % 26).collect();
    assert_eq!(sb.range(22, 8).unwrap(), expected);
}

/// A zero-capacity ring must retain nothing yet still count bytes, so a session
/// configured with no history still reports coherent sequence numbers instead of
/// dividing by zero or panicking on the modulo.
#[test]
fn zero_capacity_retains_nothing_but_still_counts_seq() {
    let mut sb = Scrollback::with_capacity(0);
    sb.push(b"hello");
    assert_eq!(sb.len(), 0);
    assert!(sb.is_empty());
    assert_eq!(sb.head_seq(), 5);
    assert_eq!(sb.oldest_seq(), 5);
    assert_eq!(sb.range(5, 10).unwrap(), b"");
    assert!(sb.range(4, 1).is_none());
}

/// An empty push must not move any sequence number. If it did, a zero-length
/// read from the PTY would desynchronise every subsequent frame offset.
#[test]
fn empty_push_is_a_no_op() {
    let mut sb = Scrollback::with_capacity(8);
    sb.push(b"abc");
    sb.push(b"");
    assert_eq!(sb.head_seq(), 3);
    assert_eq!(sb.len(), 3);
    assert_eq!(sb.range(0, 8).unwrap(), b"abc");
}

/// Many small pushes must cross from the growth path into the ring path without
/// losing or duplicating a byte. This is the real access pattern: coalesced PTY
/// reads land as a long series of appends.
#[test]
fn many_small_pushes_never_exceed_capacity() {
    let mut sb = Scrollback::with_capacity(100);
    let src = pattern(1000);
    for b in &src {
        sb.push(&[*b]);
    }
    assert_eq!(sb.len(), 100);
    assert_eq!(sb.head_seq(), 1000);
    assert_eq!(sb.oldest_seq(), 900);
    assert_eq!(sb.range(900, 100).unwrap(), &src[900..]);
}

/// The geometric growth path must hand over to the ring path mid-push. A ring
/// that grows past its cap would defeat the whole memory budget; one that wraps
/// early would throw away history it was asked to keep.
#[test]
fn growth_then_wrap_preserves_the_newest_bytes() {
    let cap = 10_000;
    let mut sb = Scrollback::with_capacity(cap);
    let src = pattern(12_000);
    sb.push(&src[..6_000]);
    assert_eq!(sb.len(), 6_000, "still growing, nothing evicted yet");
    assert_eq!(sb.oldest_seq(), 0);
    sb.push(&src[6_000..]);
    assert_eq!(sb.len(), cap);
    assert_eq!(sb.head_seq(), 12_000);
    assert_eq!(sb.oldest_seq(), 2_000);
    assert_eq!(sb.range(2_000, cap).unwrap(), &src[2_000..]);
}

/// A push landing exactly on the last free byte must switch modes cleanly, with
/// the following push wrapping from index zero. An off-by-one at this exact
/// transition is invisible until a session happens to align with it.
#[test]
fn push_filling_the_last_free_byte_then_wrapping() {
    let mut sb = Scrollback::with_capacity(5);
    sb.push(b"abc");
    sb.push(b"de");
    assert_eq!(sb.len(), 5);
    assert_eq!(sb.oldest_seq(), 0);
    assert_eq!(sb.range(0, 5).unwrap(), b"abcde");
    sb.push(b"f");
    assert_eq!(sb.oldest_seq(), 1);
    assert_eq!(sb.range(1, 5).unwrap(), b"bcdef");
}

/// A one-byte ring is the smallest non-degenerate case and exercises the modulo
/// on every push. It must always hold exactly the newest byte.
#[test]
fn capacity_one_holds_only_the_newest_byte() {
    let mut sb = Scrollback::with_capacity(1);
    for (i, b) in b"xyz".iter().enumerate() {
        sb.push(&[*b]);
        assert_eq!(sb.len(), 1);
        assert_eq!(sb.head_seq(), i as u64 + 1);
        assert_eq!(sb.oldest_seq(), i as u64);
        assert_eq!(sb.range(i as u64, 1).unwrap(), &[*b]);
    }
}
