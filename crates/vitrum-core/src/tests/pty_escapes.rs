//! Byte fidelity for real escape sequences, including sequences deliberately
//! split across chunk boundaries.
//!
//! Plain ASCII throughput does not exercise the path that matters. A terminal is
//! destroyed by losing or duplicating a single byte inside a control sequence: an
//! ESC that arrives without its final byte leaves the emulator parsing the next
//! line of output as parameters, so one dropped byte at a coalescing boundary
//! corrupts the whole screen rather than one character.

#[cfg(not(windows))]
use crate::SessionManager;
#[cfg(not(windows))]
use crate::tests::helpers::{collect, shell_spec, wait_exit};

/// Real ESC bytes must arrive verbatim, not as a printable transcription.
///
/// 0x1B must stay one byte. Any layer that rendered it as `\033`, or that routed
/// output through a string type, would turn four screen-controlling bytes into
/// eleven visible characters, which is exactly what a text-mangling transport
/// looks like from the client side.
#[cfg(not(windows))]
#[tokio::test]
async fn escape_sequences_arrive_as_real_bytes() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec("printf '\\033[32mgreen\\033[0m'"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| b.ends_with(b"m")).await;
    assert_eq!(c.bytes, b"\x1b[32mgreen\x1b[0m");
    assert_eq!(
        c.bytes.iter().filter(|b| **b == 0x1b).count(),
        2,
        "two real ESC bytes, not their printable spelling"
    );
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// An escape sequence split across two published chunks must reassemble exactly.
///
/// The child emits a lone ESC, waits for input so the coalescing window provably
/// closes, then emits the rest. That guarantees the sequence straddles a chunk
/// boundary instead of hoping the kernel splits it there. A boundary that drops,
/// duplicates, or reorders a byte is invisible in plain ASCII and catastrophic
/// here.
#[cfg(not(windows))]
#[tokio::test]
async fn an_escape_split_across_chunks_reassembles() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec(
            "printf '\\033'; read -r x; printf '[31mred\\033[0m'",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);

    // Wait for the lone ESC to be published on its own.
    c.until(|b| !b.is_empty()).await;
    assert_eq!(c.bytes, b"\x1b", "the first chunk is the bare ESC");
    let chunks_after_esc = c.chunks;

    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.ends_with(b"[0m")).await;
    assert!(
        c.chunks > chunks_after_esc,
        "the remainder must arrive in a later chunk, or the split is not tested"
    );
    // The echoed newline sits between the two halves of the stream, exactly as a
    // real terminal would see it; the escape bytes themselves are intact.
    assert_eq!(c.bytes, b"\x1b\r\n[31mred\x1b[0m");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// Many escape sequences across many chunks must concatenate byte for byte.
///
/// 300 separate writes of a full SGR sequence guarantees several coalescing
/// windows elapse mid-stream. Comparing the whole concatenation against the
/// expected bytes is what catches a boundary that loses or repeats a byte, which
/// a length check alone would miss.
#[cfg(not(windows))]
#[tokio::test]
async fn many_escapes_across_many_chunks_are_byte_exact() {
    let mgr = SessionManager::new(256 * 1024);
    let id = mgr
        .spawn(shell_spec(
            "i=0; while [ $i -lt 300 ]; do printf '\\033[32mstep %s\\033[0m\\n' $i; i=$((i+1)); done",
        ))
        .expect("spawn");
    let mut c = collect(&mgr, id);

    let expected: Vec<u8> = (0..300)
        .flat_map(|i| format!("\x1b[32mstep {i}\x1b[0m\r\n").into_bytes())
        .collect();
    c.until(|b| b.len() >= expected.len()).await;
    assert_eq!(c.bytes, expected);
    assert_eq!(
        c.bytes.iter().filter(|b| **b == 0x1b).count(),
        600,
        "every ESC byte survived the coalescing boundaries"
    );
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}

/// Escape bytes must survive the scrollback ring, including eviction.
///
/// Replay is what a client paints after a reconnect, so a ring that mangled a
/// control byte would corrupt the restored screen even though the live stream was
/// fine. The capacity here forces eviction mid-sequence.
#[cfg(not(windows))]
#[tokio::test]
async fn escape_bytes_survive_scrollback_eviction() {
    let capacity = 64;
    let mgr = SessionManager::new(capacity);
    let id = mgr
        .spawn(shell_spec(
            "i=0; while [ $i -lt 20 ]; do printf '\\033[3%sm%s' $i $i; i=$((i+1)); done",
        ))
        .expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));

    let expected: Vec<u8> = (0..20)
        .flat_map(|i| format!("\x1b[3{i}m{i}").into_bytes())
        .collect();
    let (from, bytes, more) = mgr.scrollback(id, u64::MAX, 4096).expect("session exists");
    assert_eq!(bytes.len(), capacity);
    assert_eq!(from as usize, expected.len() - capacity);
    assert!(!more);
    assert_eq!(
        bytes,
        expected[expected.len() - capacity..],
        "the retained tail must be byte-exact through the ring, ESC bytes included"
    );
}

/// A UTF-8 sequence split across chunks must not be repaired or replaced.
///
/// Partial UTF-8 legitimately straddles a read boundary, and a transport that
/// validated or lossily decoded would substitute replacement characters and
/// permanently corrupt the pane. The bytes must pass through untouched.
#[cfg(not(windows))]
#[tokio::test]
async fn a_split_utf8_sequence_passes_through_untouched() {
    let mgr = SessionManager::new(4096);
    // The three bytes of U+4E16, split after the first.
    let id = mgr
        .spawn(shell_spec("printf '\\344'; read -r x; printf '\\270\\226'"))
        .expect("spawn");
    let mut c = collect(&mgr, id);
    c.until(|b| !b.is_empty()).await;
    assert_eq!(c.bytes, b"\xe4", "a lone continuation-less lead byte");

    mgr.write(id, b"\n").expect("write");
    c.until(|b| b.len() >= 5).await;
    assert_eq!(c.bytes, b"\xe4\r\n\xb8\x96");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
}
