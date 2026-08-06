//! Sequence numbering: continuity, the `head == oldest + len` invariant, and
//! correctness past `u32::MAX`.

use crate::Scrollback;
use crate::tests::helpers::pattern;

/// Every push must start exactly where the previous one ended. A gap or overlap
/// here breaks the one thing the wire protocol relies on: that `seq` is the byte
/// offset of the payload's first byte.
#[test]
fn seq_is_contiguous_across_many_pushes() {
    let mut sb = Scrollback::with_capacity(64);
    let src = pattern(500);
    let mut at = 0usize;
    let mut expected = 0u64;
    // Deterministic, uneven push sizes: a fixed stride would hide alignment bugs.
    for step in [1usize, 7, 3, 13, 2, 29, 5, 11].iter().cycle().take(60) {
        let end = (at + step).min(src.len());
        if at == end {
            break;
        }
        assert_eq!(sb.head_seq(), expected, "head_seq before push");
        sb.push(&src[at..end]);
        expected += (end - at) as u64;
        assert_eq!(sb.head_seq(), expected, "head_seq after push");
        at = end;
    }
    assert_eq!(sb.head_seq(), at as u64);
    assert_eq!(sb.oldest_seq(), at as u64 - 64);
    assert_eq!(sb.range(sb.oldest_seq(), 64).unwrap(), &src[at - 64..at]);
}

/// The `oldest_seq == head_seq - len` invariant must hold at every step, since
/// both the scrollback paging path and the gap detection derive offsets from it.
#[test]
fn oldest_plus_len_always_equals_head() {
    let mut sb = Scrollback::with_capacity(37);
    let src = pattern(300);
    let mut at = 0usize;
    let mut n = 1usize;
    while at < src.len() {
        let end = (at + n).min(src.len());
        sb.push(&src[at..end]);
        assert_eq!(
            sb.oldest_seq() + sb.len() as u64,
            sb.head_seq(),
            "invariant broken after pushing {} bytes",
            end - at
        );
        assert!(sb.len() <= 37, "retained more than capacity");
        at = end;
        n = n % 9 + 1;
    }
    assert_eq!(sb.head_seq(), 300);
}

/// Sequence numbers must keep counting past `u32::MAX`. A long-lived agent
/// exceeds 4 GiB of output, and a `u32` or `usize` truncation anywhere on this
/// path would wrap the offset back to zero and make every later frame land at
/// the wrong place in the client's viewport.
///
/// The pushes are large and the ring is tiny, so this crosses 4 GiB of stream
/// while only ever copying 64 bytes per push.
#[test]
fn seq_counts_past_u32_max() {
    let mut sb = Scrollback::with_capacity(64);
    let block = vec![b'.'; 1024 * 1024];
    let pushes = 4100u64;
    for _ in 0..pushes {
        sb.push(&block);
    }
    let total = pushes * block.len() as u64;
    assert_eq!(total, 4_299_161_600);
    assert!(total > u32::MAX as u64);
    assert_eq!(sb.head_seq(), total);
    assert_eq!(sb.oldest_seq(), total - 64);
    assert_eq!(sb.len(), 64);
}

/// Reading back at an offset above `u32::MAX` must return the right bytes. This
/// is the truncation bug's other half: counting correctly but indexing with a
/// wrapped offset would serve the wrong window or refuse a valid one.
#[test]
fn range_works_at_offsets_above_u32_max() {
    let mut sb = Scrollback::with_capacity(64);
    let block = vec![b'.'; 1024 * 1024];
    for _ in 0..4100u64 {
        sb.push(&block);
    }
    let tail = b"marker-past-four-gib";
    sb.push(tail);
    let head = sb.head_seq();
    assert!(head > u32::MAX as u64);
    let from = head - tail.len() as u64;
    assert!(from > u32::MAX as u64);
    assert_eq!(sb.range(from, tail.len()).unwrap(), tail);
    assert!(
        sb.range(sb.oldest_seq() - 1, 1).is_none(),
        "eviction must still be detected above u32::MAX"
    );
}

/// `head_seq` must reflect everything ever written even when nothing survives,
/// so a client can tell how far ahead the stream is rather than assuming the
/// retained bytes are the whole story.
#[test]
fn head_seq_counts_evicted_bytes() {
    let mut sb = Scrollback::with_capacity(0);
    sb.push(&pattern(1000));
    sb.push(&pattern(24));
    assert_eq!(sb.head_seq(), 1024);
    assert_eq!(sb.oldest_seq(), 1024);
    assert_eq!(sb.len(), 0);
}
