//! What the output path is allowed to spend per byte, counted rather than timed.
//!
//! These assertions are on `PumpCounts`, not on a clock. A wall-clock threshold
//! on a shared runner measures the runner, so the properties here are the ones
//! the shape of the code guarantees: how many times a byte is copied, how many
//! times it is parsed, how many read syscalls and heap allocations a megabyte
//! costs, and how many task wakeups a burst costs — none at all when there is
//! nothing to read. Each of them was a real cost in this loop and each of them
//! is cheap to reintroduce by accident, because reintroducing it looks like
//! ordinary code.
//!
//! Every ceiling below is either derived from a constant in `session.rs` or
//! taken from the pipeline benchmark, and each doc comment says which. A bound
//! whose provenance nobody can state is a bound nobody can move honestly.

#[cfg(not(windows))]
use crate::SessionManager;
#[cfg(not(windows))]
use crate::tests::helpers::{QUIET, collect, probe_now, shell_spec};

/// Bytes published by a session, and what the pump spent getting them there.
///
/// The child is left running and the session is closed rather than waited on.
/// A run cut short by end of stream is neither an idle flush nor a capped one,
/// so measuring a child that has already exited would put the last run of
/// every burst into a third category and make the flush-reason counters read
/// as if a run had gone missing.
#[cfg(not(windows))]
async fn burst(script: &str, want: usize) -> (usize, crate::PumpCounts) {
    let mgr = SessionManager::new(1 << 24);
    // Kept alive well past the measurement: `until` returns as soon as the
    // bytes are published, so this is a bound on the child outliving the
    // burst, not a wait.
    let id = mgr
        .spawn(shell_spec(&format!("{script}; sleep 60")))
        .expect("spawn");
    let mut live = collect(&mgr, id);
    live.until(move |b| b.len() >= want).await;
    let counts = mgr.pump_counts(id).expect("counts for a live session");
    let published = live.bytes.len();
    let _ = mgr.close(id);
    (published, counts)
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

/// A child that writes once and stops must not be held for the whole cap.
///
/// WHY: the coalescer used to publish on a fixed 6 ms window, which charged an
/// echoed keystroke the price of batching a firehose: one read arrived, and
/// the run then waited out a window for a second read that was never coming.
/// The run now ends on silence and only reaches the cap while output is still
/// arriving, so a lone write is an idle flush.
///
/// This is the counter form of the latency claim on purpose. Asserting a
/// duration here would be asserting how fast this machine schedules a timer,
/// which is what makes a wall-clock test flake in CI; the property that
/// actually changed is which deadline ended the run.
///
/// This does NOT bound how long the idle gap is, and it does not prove the
/// firehose still batches: the test below does that, and the two together are
/// what stop the gap being tuned to either extreme.
#[cfg(not(windows))]
#[tokio::test]
async fn a_lone_write_is_published_on_silence_not_on_the_cap() {
    let (_, counts) = burst("printf 'hello\\n'", 7).await;
    assert_eq!(counts.publishes, 1, "one write should be one run");
    assert_eq!(
        (counts.idle_flushes, counts.capped_flushes),
        (1, 0),
        "a single write waited for the cap instead of being published on silence",
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

/// A backlog of reads costs one timer and a handful of wakeups, not one of
/// each per read.
///
/// WHY: the coalescer awaited each read under its own `timeout_at`, which
/// armed a timer and rescheduled the task once per read. A pty hands back a
/// few hundred bytes at a time under load, so a megabyte of output was
/// thousands of timer registrations to publish sixteen runs. One timer per
/// window plus `recv_many` collapses that.
///
/// The reads are queued before anything polls the channel, which is what makes
/// the bound exact rather than a ratio. Against a live session the number of
/// reads waiting when `recv_many` runs is a scheduling outcome — how far the
/// reader thread got ahead — so the ratio a burst produces is a property of
/// the machine. This assertion was written that way and was flaky for it: it
/// failed on the CI runner at 475 wakeups for 756 reads, reproduced locally in
/// four runs out of six, and its bound of half was never something the loop
/// promised.
///
/// With `N` reads already in the channel the fixed loop takes the first from
/// `next_read` and the rest `BATCH_READS` at a time, so it costs one timer,
/// one publish, and at most `1 + (N-1)/BATCH_READS` wakeups. Per-read waiting
/// costs `N` of each and cannot come near it.
///
/// This does NOT catch wakeups outside the coalescer, and it says nothing
/// about how a live reader thread and this loop interleave: that is the part
/// no count can pin down.
#[cfg(not(windows))]
#[tokio::test]
async fn a_backlog_costs_one_timer_and_a_batch_of_wakeups() {
    const READS: usize = 512;
    let coalescer = crate::session::Coalescer::new().expect("a pty for the harness");
    for _ in 0..READS {
        coalescer.queue(b"xxxxxxxx");
    }
    let counts = coalescer.drain().await;

    assert_eq!(
        counts.reads, 0,
        "the harness does no pty reads; {} were counted",
        counts.reads,
    );
    assert_eq!(
        (counts.publishes, counts.timers),
        (1, 1),
        "{READS} queued reads are one run: {counts:?}",
    );
    let ceiling = 1 + (READS - 1).div_ceil(crate::session::BATCH_READS) as u64;
    assert!(
        counts.wakeups <= ceiling,
        "{} wakeups for {READS} queued reads, ceiling {ceiling}; reads that \
         are already in the channel are supposed to arrive {} at a time",
        counts.wakeups,
        crate::session::BATCH_READS,
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

/// A megabyte of output may be copied at most an eighth of a time.
///
/// WHY: this is the budget form of the in-place rejoin above. That test says
/// the common path is not the copying one; this one says what the uncommon
/// path is allowed to cost, so a change that makes straddling ordinary rather
/// than rare is caught even though every individual copy is still legitimate.
///
/// Two ceilings, both derived from the code rather than measured. The first is
/// stated per megabyte and is the loose one: a run copies only when the
/// reader crosses into a fresh arena mid-run, which can happen at most once
/// per `READ_ARENA`, so a megabyte of output buys at most one straddle, and a
/// straddle copies only the remainder of that run, which `FLUSH_BYTES` puts
/// near 64 KiB. A sixteenth of the volume, in other words, and the ceiling is
/// set at an eighth so the batch that crossed the boundary and an unlucky
/// alignment of runs against arenas are both inside it.
///
/// The second is the exact invariant and does not depend on the volume at all:
/// copies are bounded by the number of arenas taken, times one run. That is
/// the sentence the code actually guarantees, so it holds for any burst size
/// and for any `READ_ARENA`, and it is the one that fails first if a run
/// starts copying for a reason other than a straddle.
///
/// The pipeline benchmark reports the same quantity as `staged_bytes_per_mb`
/// if a number rather than a bound is wanted.
///
/// This does NOT catch a copy made outside the merge, and neither bound
/// notices a smaller `READ_ARENA` on its own: shrinking the arena raises
/// straddling in proportion, which is a tuning decision the volume bound only
/// reports once it has been taken eight times over.
#[cfg(not(windows))]
#[tokio::test]
async fn a_megabyte_of_output_is_copied_at_most_an_eighth_of_a_time() {
    let (script, want) = flood(4096);
    let (published, counts) = burst(&script, want).await;
    assert!(
        counts.staged_bytes * 8 <= published as u64,
        "{} bytes copied for {published} published, over the eighth-of-volume \
         ceiling; runs are straddling arenas far more often than one per arena",
        counts.staged_bytes,
    );
    // One capped run per arena boundary, doubled for the batch that crossed it.
    assert!(
        counts.staged_bytes <= counts.arenas * 128 * 1024,
        "{} bytes copied across {} arenas; a straddle can only copy the rest \
         of one run, so something is copying that is not a straddle",
        counts.staged_bytes,
        counts.arenas,
    );
}

/// A megabyte of output may cost at most two allocations on the byte path.
///
/// WHY: the reader used to allocate and zero a fresh buffer for every read,
/// which at a few hundred bytes a read wrote about seventy times more memory
/// than the session produced. It now carves one `READ_ARENA` into consecutive
/// reads, and `arenas` is the whole allocation cost of the path, because
/// nothing downstream of the reader allocates per read or per run: the batch
/// vector is built once, the merge moves indices, and `freeze` is a handoff.
///
/// The ceiling is derived. A 1 MiB arena is replaced once its remainder falls
/// under `READ_FLOOR`, so a steady session takes 1 MiB / (1 MiB - 32 KiB) =
/// about 1.03 arenas per megabyte, plus the one allocated before the first
/// read. Two per megabyte is that doubled, which is loose enough for a short
/// burst where the initial arena is a large share of the total and tight
/// enough that returning to per-read allocation, at three orders of magnitude
/// more, cannot hide in it.
///
/// This does NOT count allocations made off this path — the broadcast ring,
/// the scrollback, the runtime — which is what the pipeline benchmark's
/// process-wide allocator is for.
#[cfg(not(windows))]
#[tokio::test]
async fn a_megabyte_of_output_costs_at_most_two_reader_allocations() {
    let (script, want) = flood(4096);
    let (published, counts) = burst(&script, want).await;
    assert!(
        counts.arenas * 1024 * 1024 <= 2 * published as u64,
        "{} reader arenas for {published} bytes, over two per megabyte; the \
         reader is allocating per read again rather than carving one block",
        counts.arenas,
    );
}

/// A megabyte of output must average at least 128 bytes to the read syscall.
///
/// WHY: with the engine off the read path and the staging copy gone, what a
/// megabyte costs is mostly trips to the kernel. The pipeline benchmark
/// measures about 2100 reads per megabyte, an average read near 500 bytes, and
/// that is the number any further work on this path has to move.
///
/// The ceiling is that measurement with room around it: 8192 reads per
/// megabyte, four times the observed cost, which is an average read of 128
/// bytes. The slack is deliberate, because how much a pty returns per read is
/// a property of the machine and of how fast the child writes, and a bound
/// drawn tightly around one host's number would fail on a loaded runner
/// instead of on a regression. What it does catch is a `READ_CHUNK` dropped
/// below the line discipline's 4 KiB buffer, which multiplies reads by the
/// factor it was shrunk by, and any change that reads a line or a record at a
/// time.
///
/// For scale in the other direction: the best a Linux pty can offer is
/// `N_TTY_BUF_SIZE` per read, so 256 reads per megabyte is the floor no amount
/// of tuning here beats.
///
/// This does NOT say the reads are cheap, only how many there are, and it says
/// nothing about the write side.
#[cfg(not(windows))]
#[tokio::test]
async fn a_megabyte_of_output_costs_fewer_than_eight_thousand_reads() {
    let (script, want) = flood(4096);
    let (published, counts) = burst(&script, want).await;
    assert!(
        counts.reads * 128 <= published as u64,
        "{} reads for {published} bytes, under 128 bytes a read; the reader is \
         making far more trips to the kernel than the pty requires",
        counts.reads,
    );
}

/// A session that writes nothing must cost nothing.
///
/// WHY: every timer on this path is armed by activity rather than by a clock,
/// and that is the whole claim behind a daemon that hosts twenty agents at 0%
/// while they think. It is also the easiest property to lose: a poll added
/// anywhere in the coalescer, or a periodic re-probe, would be invisible in
/// every other test here because they all measure a session that is producing
/// output.
///
/// Zero is the only defensible ceiling. A wakeup is either caused by a byte or
/// it is a defect, and there are no bytes.
///
/// The probe is forced rather than waited out, so the assertion is made after
/// the one timer this path does arm has fired and disarmed itself instead of
/// racing it; the quiet window afterwards is a bound on absence, not a wait
/// for a result.
///
/// This does NOT observe CPU, and it does not cover a session that has gone
/// quiet after producing output, which arms a settle timer once more and then
/// parks the same way.
#[cfg(not(windows))]
#[tokio::test]
async fn an_idle_session_costs_the_output_path_no_wakeups_at_all() {
    let mgr = SessionManager::new(1 << 20);
    let id = mgr.spawn(shell_spec("sleep 60")).expect("spawn");
    probe_now(&mgr, id).await;
    tokio::time::sleep(QUIET).await;
    let counts = mgr.pump_counts(id).expect("counts for a live session");
    let _ = mgr.close(id);
    assert_eq!(
        (counts.reads, counts.wakeups, counts.publishes),
        (0, 0, 0),
        "a session that wrote nothing still did work: {counts:?}",
    );
}
/// A closed session must reach a terminal state even if its child is never
/// reaped.
///
/// WHY: the coalescer leaves its read loop when the raw channel closes, which
/// happens once every descriptor on the terminal is gone. That is not the same
/// event as the child exiting. A child that redirected its own output
/// elsewhere and carried on closes the terminal while still running, and the
/// loop then waits for an exit code that nothing is going to produce.
///
/// Waiting only on that exit makes the terminal state depend on the child
/// eventually dying. It parks forever otherwise, holding the session, its
/// reader thread and its writer thread, and the row never leaves the sidebar.
/// Closing is the operator saying it is over and has to be sufficient by
/// itself.
///
/// The child here is genuinely never reaped: the harness holds the exit sender
/// for the whole run, so the close is the only thing that can end the loop.
/// Against the previous code this does not fail on an assertion, it hangs,
/// which is why the harness bounds the wait and returns whether the loop came
/// back rather than asserting on what it produced.
///
/// Covers only the wait after the read loop. The read loop has its own wait
/// with the same shape, pinned by
/// [`closing_ends_a_coalescer_whose_reader_is_still_parked`].
#[cfg(not(windows))]
#[tokio::test]
async fn closing_ends_a_coalescer_whose_child_was_never_reaped() {
    let coalescer = crate::session::Coalescer::new().expect("a pty for the harness");
    coalescer.queue(b"some output before the terminal closed");

    let returned = coalescer
        .close_while_unreaped(std::time::Duration::from_secs(5))
        .await;

    assert!(
        returned,
        "the coalescer never returned: it is still waiting for a child that \
         nothing will reap, holding the session and its threads"
    );
}

/// Closing a session ends its coalescer even while the terminal is still open.
///
/// The sibling of the test above, and the other half of the class: a wait that
/// cannot observe a close. Here the loop never reaches the wait for an exit at
/// all, because there is no end of stream to send it there. The reader thread
/// is still parked on a master a child is holding open, which is what every
/// live session looks like, and the read loop's own wait is unbounded once the
/// settle window has expired.
///
/// A child that ignores the hangup makes this reachable in production: closing
/// the session does not end the reader, so nothing closes the channel, and
/// nothing reaps the child either. Against a read loop with no close arm this
/// hangs rather than failing an assertion, so the harness bounds the wait.
///
/// Pins the unbounded park only, which is the one wait where a missed close
/// is a leak rather than a delay. The read loop's other two answers to a
/// close are deliberately not pinned here and no test distinguishes them: the
/// arm on the settle-window wait has a timer under it, so missing a close
/// there costs one settle window and then falls into the park this test does
/// pin, and the flag check at the top of the loop closes the window where a
/// close lands between two waits, which a hang test cannot observe because
/// the park behind it catches the same close.
#[cfg(not(windows))]
#[tokio::test]
async fn closing_ends_a_coalescer_whose_reader_is_still_parked() {
    let coalescer = crate::session::Coalescer::new().expect("a pty for the harness");
    coalescer.queue(b"some output while the terminal was open");

    let returned = coalescer
        .close_while_reading(std::time::Duration::from_secs(5))
        .await;

    assert!(
        returned,
        "the coalescer never returned: it is still waiting to read a terminal \
         that was closed, holding the session and its threads"
    );
}
