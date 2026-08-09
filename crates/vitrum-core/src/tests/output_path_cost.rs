//! What the output path is allowed to spend per byte, counted rather than timed.
//!
//! These assertions are on `PumpCounts`, not on a clock. A wall-clock threshold
//! on a shared runner measures the runner, so the properties here are the ones
//! the shape of the code guarantees: how many times a byte is copied, how many
//! times a byte is parsed, and how many task wakeups a burst costs. Each of
//! them was a real cost in this loop and each of them is cheap to reintroduce
//! by accident, because reintroducing it looks like ordinary code.

#[cfg(not(windows))]
use crate::SessionManager;
#[cfg(not(windows))]
use crate::tests::helpers::{collect, shell_spec, wait_exit};

/// Bytes published by a session, and what the pump spent getting them there.
#[cfg(not(windows))]
async fn burst(script: &str, want: usize) -> (usize, crate::PumpCounts) {
    let mgr = SessionManager::new(1 << 24);
    let id = mgr.spawn(shell_spec(script)).expect("spawn");
    let mut live = collect(&mgr, id);
    live.until(move |b| b.len() >= want).await;
    assert_eq!(wait_exit(&mgr, id).await, Some(0), "child failed");
    // The reader can still be draining the tail when the child is reaped, so
    // wait for the byte count the script is known to produce rather than for a
    // duration.
    live.until(move |b| b.len() >= want).await;
    let counts = mgr.pump_counts(id).expect("counts for a live session");
    (live.bytes.len(), counts)
}

/// A script that writes `kib` KiB as fast as the pty will take it, and the
/// exact byte count it produces.
///
/// Fast matters: the byte ceiling that caps a run only comes into play when a
/// child can outrun a 6 ms window, and a shell `printf` loop cannot. The
/// payload carries no newline, so the pty's CRLF translation cannot make the
/// published length differ from what the child wrote.
#[cfg(not(windows))]
fn flood(kib: usize) -> (String, usize) {
    (
        format!("dd if=/dev/zero bs=1024 count={kib} 2>/dev/null | tr '\\000' 'x'"),
        kib * 1024,
    )
}

/// A merged run must not be copied into a second buffer.
///
/// WHY: the coalescer used to build every run by appending each read into a
/// staging `BytesMut`, which moved the whole firehose an extra time on the way
/// to a broadcast that then handed out the result by reference anyway.
/// Consecutive pty reads are consecutive slices of one reader arena, so a run
/// is rejoined in place and `staged_bytes` counts only the bytes that could
/// not be: a run that straddles two arenas.
///
/// The bound is a fraction of the volume rather than zero, because straddling
/// is legitimate and its frequency is set by the arena size. Reintroducing an
/// unconditional copy makes `staged_bytes` equal the byte count, which is more
/// than an order of magnitude over this bound.
///
/// This does NOT catch a copy made somewhere other than the merge, and it does
/// not claim the fallback is rare for every arena size: it claims that the
/// common path is not the fallback.
#[cfg(not(windows))]
#[tokio::test]
async fn a_merged_run_is_rejoined_in_place_rather_than_copied() {
    let (script, want) = flood(4096);
    let (published, counts) = burst(&script, want).await;
    assert!(
        counts.staged_bytes * 4 < published as u64,
        "{} of {published} published bytes were copied into a staging buffer; \
         merged runs are supposed to be rejoined in place",
        counts.staged_bytes,
    );
}

/// A single-read run must be published without touching its bytes at all.
///
/// WHY: this is the interactive case, one keystroke echoed back, and it is the
/// one the staging buffer served worst: a copy, and an allocation, to move
/// eight bytes. A run that never merged anything is the reader's own
/// allocation and is handed on as it stands.
///
/// Exactly zero is the right bound here, not a fraction: there is no second
/// read to be non-contiguous with.
///
/// This does NOT catch a copy in the broadcast or the scrollback ring, which
/// are downstream of `publish` and counted nowhere.
#[cfg(not(windows))]
#[tokio::test]
async fn a_single_read_run_is_published_without_a_copy() {
    let (published, counts) = burst("printf 'hello\\n'", 7).await;
    assert_eq!(published, 7, "the child wrote something else");
    assert_eq!(counts.publishes, 1, "one write should be one run");
    assert_eq!(
        counts.staged_bytes, 0,
        "a run of one read was copied on its way out",
    );
}

/// Every byte is parsed exactly once in the daemon.
///
/// WHY: the reader thread used to feed a full terminal engine so the daemon
/// could know a session's title and working directory, and then the coalescer
/// scanned the same bytes again for hints. Two passes over the firehose, and
/// the expensive one existed to recover two strings. The engine is gone and
/// the scan reports title and pwd itself, so parsed volume must equal
/// published volume: not twice it, and not less than it either, which would
/// mean output was reaching clients unscanned.
///
/// This does NOT catch a third pass added outside the pump, and it says
/// nothing about how much work one pass does.
#[cfg(not(windows))]
#[tokio::test]
async fn published_bytes_are_parsed_exactly_once() {
    let (script, want) = flood(2048);
    let (published, counts) = burst(&script, want).await;
    assert_eq!(
        counts.parsed_bytes, published as u64,
        "the daemon parsed {} bytes to publish {published}",
        counts.parsed_bytes,
    );
}

/// A burst must cost far fewer task wakeups than it costs reads.
///
/// WHY: the coalescer awaited each read under its own `timeout_at`, which
/// armed a timer and rescheduled the task once per read. A pty hands back a
/// few hundred bytes at a time under load, so a megabyte of output was
/// thousands of timer registrations to publish sixteen runs. One timer per
/// window plus `recv_many` collapses that.
///
/// The assertion is a ratio against this run's own read count rather than an
/// absolute number, because how much a pty returns per read depends on the
/// machine and on how fast the reader is draining it. Per-read waiting pins
/// wakeups to reads exactly, so any real batching clears this bound and its
/// absence cannot.
///
/// This does NOT catch wakeups outside the coalescer, and it does not measure
/// scheduling latency: a batch that is too large would show up as latency, not
/// here.
#[cfg(not(windows))]
#[tokio::test]
async fn a_burst_is_drained_in_fewer_wakeups_than_reads() {
    let (script, want) = flood(4096);
    let (_, counts) = burst(&script, want).await;
    assert!(
        counts.reads > 32,
        "only {} reads: too small a burst to say anything about batching",
        counts.reads,
    );
    assert!(
        counts.wakeups * 2 < counts.reads,
        "{} wakeups for {} reads; reads are supposed to arrive in batches",
        counts.wakeups,
        counts.reads,
    );
}

/// Publishing is per window, not per read.
///
/// WHY: the whole point of the coalescer. Without it every few hundred bytes a
/// child writes becomes its own broadcast message, a scrollback entry and a
/// frame for every attached client, which is how a firehose turns into
/// thousands of tiny writes on the wire. The window and the byte ceiling
/// together bound this from both sides.
///
/// The floor matters as much as the ceiling: one publish for the whole burst
/// would mean the byte ceiling had stopped working and a client would wait for
/// the entire run before seeing anything.
///
/// This does NOT catch a stall inside a window, which is what the latency
/// figures in the pipeline benchmark are for.
#[cfg(not(windows))]
#[tokio::test]
async fn a_burst_is_published_in_runs_not_in_reads() {
    let (script, want) = flood(4096);
    let (published, counts) = burst(&script, want).await;
    assert!(
        counts.publishes * 8 < counts.reads,
        "{} publishes for {} reads; output is not being coalesced",
        counts.publishes,
        counts.reads,
    );
    // A run stops taking reads once 64 KiB is pending, and the batch that
    // crossed that line is already in hand, so the largest run the code can
    // produce is the ceiling plus one batch. Both terms are derived from this
    // run rather than assumed, because how much a pty returns per read is a
    // property of the machine. Dropping the byte ceiling leaves runs bounded
    // only by the 6 ms window, which at any realistic rate is an order of
    // magnitude over this.
    let per_read = published as u64 / counts.reads.max(1);
    let largest = 64 * 1024 + 64 * per_read;
    assert!(
        counts.publishes * largest >= published as u64,
        "{} publishes for {published} bytes averages more than {largest} bytes \
         a run; a run is supposed to be capped",
        counts.publishes,
    );
}
