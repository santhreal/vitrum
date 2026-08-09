//! What the output path costs, measured in process against a real PTY.
//!
//! Every other workload in this crate is a WebSocket client: it measures the
//! product as an operator meets it, which is the right question for delivery
//! and the wrong one for "where does a megabyte's time go". A socket in the
//! middle hides the pipeline behind its own scheduling, and a report that
//! cannot separate the two cannot say whether a change helped.
//!
//! So this one links [`vitrum_core`] directly and drives a real
//! [`SessionManager`], a real pseudoterminal and a real child process. Nothing
//! here is a fake: the bytes come out of a pty the kernel opened, and they are
//! observed where a client would observe them, on the broadcast channel.
//!
//! # What it measures
//!
//! - **Throughput.** A child writing a fixed number of megabytes as fast as the
//!   pty will take them, with a viewer attached, at one session and at several
//!   at once. Concurrency is the honest case: a daemon hosts a fleet.
//! - **Interactive latency.** A single byte written into the pty, timed until
//!   the run containing it is observable on the broadcast channel. Reported as
//!   a distribution, because the tail is the thing an operator feels and a mean
//!   is exactly where a stall goes to hide.
//! - **Structural cost per megabyte.** Read syscalls, publishes, task wakeups,
//!   bytes copied when a run cannot be rejoined in place, bytes the daemon
//!   scans, reader arena allocations, and process-wide allocations. These come
//!   from [`vitrum_core::PumpCounts`] and from this binary's own allocator, so
//!   they are counts of what happened rather than inferences from a duration.
//!   Reads per megabyte is the one that matters most now: with the engine off
//!   the read path and the staging copy gone, a megabyte is mostly syscalls.
//! - **The parse the daemon does not make.** A full terminal engine used to be
//!   fed every byte on the read thread so the daemon could learn two strings.
//!   It is timed here on its own, over the same bytes, because that is the
//!   cost taking it off the path removed — and the cost the client's own
//!   emulator still pays once, which is where it belongs.
//!
//! # The child
//!
//! This binary re-executes itself as `vitrum-bench emit`, which writes
//! synthetic agent-shaped output: styled status lines, cursor control, a
//! progress line that rewrites itself, and periodic title announcements. A
//! generator inside the harness rather than a system tool is what makes the
//! numbers reproducible on a machine whose `yes` or `dd` differs, and it is the
//! only way to hold escape density fixed across runs.
//!
//! The child waits for one byte on stdin before it writes anything, so the
//! clock starts after the fork, the exec and the attach rather than measuring
//! them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use vitrum_core::{OutputChunk, PumpCounts, SessionManager, SessionSpec};
use vitrum_proto::ProjectId;

use crate::report::Report;
use crate::stats::{Latencies, Throughput};

/// Size of one write the emitter makes, matched to the daemon's read size so a
/// slow child cannot be mistaken for a fast pipeline.
const EMIT_BLOCK: usize = 32 * 1024;

/// Upper bound on any single wait here. Only reached when something is wedged.
const DEADLINE: Duration = Duration::from_secs(300);

/// Gap between interactive samples.
///
/// Comfortably longer than the coalescing window, so each sample is its own
/// run of output rather than one that merged with the sample before it. A
/// shorter gap would measure batching and call it latency.
const SAMPLE_GAP: Duration = Duration::from_millis(25);

// ---------------------------------------------------------------------------
// Allocation accounting
// ---------------------------------------------------------------------------

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

/// The system allocator, counted.
///
/// Process-wide and therefore honest about its own scope: it sees the tokio
/// runtime and this harness as well as the pipeline. That is why it is only
/// ever read as a delta across a phase whose work is overwhelmingly the
/// pipeline's, and why the report says "per megabyte" rather than "per chunk".
/// An allocation this cannot attribute is still an allocation the daemon's
/// process made while moving those bytes.
pub struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Allocations made, and bytes requested, since the process started.
fn allocated() -> (u64, u64) {
    (
        ALLOCS.load(Ordering::Relaxed),
        ALLOC_BYTES.load(Ordering::Relaxed),
    )
}

// ---------------------------------------------------------------------------
// The child
// ---------------------------------------------------------------------------

/// One block of synthetic agent output.
///
/// Escape density is the point. Plain text is the cheapest thing a terminal
/// parser can be handed, and a benchmark built out of it would report a parse
/// cost no real session ever sees. This is shaped like what an agent TUI
/// actually emits: colour, a redrawn progress line, cursor moves, and a title
/// announcement every block.
fn block() -> Vec<u8> {
    const LINES: [&str; 6] = [
        "\x1b[38;5;244m  read\x1b[0m src/session.rs \x1b[32m✓\x1b[0m 412 lines\r\n",
        "\x1b[1m●\x1b[0m \x1b[36mplanning\x1b[0m the next edit against the coalescer\r\n",
        "    the reader hands each filled prefix over without copying it\r\n",
        "\x1b[2K\r  \x1b[33mworking\x1b[0m ▕████████░░░░░░░▏ 53%",
        "\x1b[38;5;244m  edit\x1b[0m src/scrollback.rs \x1b[32m+18 −4\x1b[0m\r\n",
        "\x1b[K\x1b[1;31m!\x1b[0m one test still red, re-reading the assertion\r\n",
    ];
    let mut out = Vec::with_capacity(EMIT_BLOCK);
    out.extend_from_slice(b"\x1b]0;agent: reviewing the output path\x07");
    let mut next = 0;
    while out.len() + 128 < EMIT_BLOCK {
        out.extend_from_slice(LINES[next % LINES.len()].as_bytes());
        next += 1;
    }
    // Padded to exactly one block so the emitted total is the requested total
    // and never lands mid-escape.
    out.resize(EMIT_BLOCK - 2, b'.');
    out.extend_from_slice(b"\r\n");
    out
}

/// Write `total` bytes of agent-shaped output, after one byte of stdin says go.
///
/// The handshake is what keeps process startup out of the measurement: the
/// harness attaches its viewer, then writes the go byte, then starts its clock.
pub fn emit(total: usize) -> anyhow::Result<()> {
    let mut go = [0u8; 1];
    std::io::stdin()
        .read_exact(&mut go)
        .context("waiting for the go byte")?;
    let block = block();
    let mut out = std::io::stdout().lock();
    let mut written = 0usize;
    while written < total {
        let n = block.len().min(total - written);
        out.write_all(&block[..n])?;
        written += n;
    }
    out.flush()?;
    Ok(())
}

/// Hold the pty open, writing nothing, for the interactive phase.
///
/// The latency samples are the terminal line discipline echoing what the
/// harness typed, so the child must not add output of its own; anything it
/// wrote would be timed as if it were the echo.
pub fn idle(secs: u64) {
    std::thread::sleep(Duration::from_secs(secs));
}

// ---------------------------------------------------------------------------
// Parameters and results
// ---------------------------------------------------------------------------

/// What to run.
#[derive(Debug, Clone)]
pub struct PipelineSpec {
    /// Megabytes one session streams in the single-session phase.
    pub megabytes: usize,
    /// Megabytes each session streams in the concurrent phases.
    pub fanout_megabytes: usize,
    /// Session counts to measure, in order.
    pub sessions: Vec<usize>,
    /// Interactive samples to take.
    pub samples: usize,
    /// Retained scrollback per session; the daemon's own default by default.
    pub scrollback_bytes: usize,
}

/// One throughput phase's result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phase {
    pub sessions: usize,
    pub megabytes_each: usize,
    pub bytes_total: u64,
    pub seconds: f64,
    pub mb_per_sec: f64,
    /// Wall-clock megabytes per second one session sustained, which is the
    /// number that stops improving when the per-session path is the limit.
    pub mb_per_sec_per_session: f64,
    /// Runs the client actually received. One wakeup per attached client each.
    pub chunks_delivered: u64,
    pub chunks_dropped: u64,
    pub reads_per_mb: f64,
    pub publishes_per_mb: f64,
    pub wakeups_per_mb: f64,
    pub staged_bytes_per_mb: f64,
    pub parsed_bytes_per_mb: f64,
    /// Reader arena allocations, the byte path's whole allocation cost.
    pub arenas_per_mb: f64,
    pub allocs_per_mb: f64,
    pub alloc_bytes_per_mb: f64,
}

/// What the terminal engine that used to sit on the read path costs on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseCost {
    pub bytes: u64,
    pub seconds: f64,
    pub mb_per_sec: f64,
    /// The engine's share of a single session's wall clock, as a fraction.
    ///
    /// Both rates are megabytes per second over the same bytes, so the parse
    /// time per byte divided by the pipeline time per byte is just the ratio of
    /// the rates. A value of 0.5 means feeding this engine again on the read
    /// thread would add half of what a session now spends moving a megabyte —
    /// which is what it did, and why it is gone.
    pub share_of_pipeline: f64,
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

/// Measure the output path and return a report.
pub async fn run(spec: &PipelineSpec) -> anyhow::Result<Report> {
    let started = Instant::now();
    let exe = std::env::current_exe().context("finding this binary to re-execute as the child")?;

    let mut report = Report::new(
        "pipeline",
        "in-process",
        serde_json::json!({
            "megabytes": spec.megabytes,
            "fanout_megabytes": spec.fanout_megabytes,
            "sessions": spec.sessions,
            "samples": spec.samples,
            "scrollback_bytes": spec.scrollback_bytes,
            "emit_block": EMIT_BLOCK,
        }),
    );

    let mut phases = Vec::new();
    for (index, &count) in spec.sessions.iter().enumerate() {
        let each = if count == 1 {
            spec.megabytes
        } else {
            spec.fanout_megabytes
        };
        let phase = throughput(&exe, count, each, spec.scrollback_bytes)
            .await
            .with_context(|| format!("throughput phase {index} with {count} session(s)"))?;
        if phase.chunks_dropped > 0 {
            report.failures.push(format!(
                "{count} session(s): the reading client fell behind and lost {} chunk(s); \
                 the throughput number below is a lower bound on what was produced",
                phase.chunks_dropped
            ));
        }
        phases.push(phase);
    }

    let single = phases
        .iter()
        .find(|p| p.sessions == 1)
        .map(|p| p.mb_per_sec)
        .unwrap_or(0.0);
    let parse = parse_cost(spec.megabytes.max(1) * 1024 * 1024, single)?;

    let (interactive, pump) = latency(&exe, spec.samples, spec.scrollback_bytes, 0).await?;

    if let Some(summary) = interactive.summary() {
        report.latencies.push(("pty-to-broadcast".into(), summary));
        report
            .checks_passed
            .push("every interactive sample was delivered".into());
    } else {
        report
            .failures
            .push("no interactive sample completed".into());
    }

    // The same keystroke with eight sessions streaming beside it. A window
    // tuned only against an idle daemon is tuned against the case nobody
    // complains about.
    let (loaded, loaded_pump) = latency(&exe, spec.samples, spec.scrollback_bytes, 8).await?;
    if let Some(summary) = loaded.summary() {
        report
            .latencies
            .push(("pty-to-broadcast under 8 flooding sessions".into(), summary));
    } else {
        report
            .failures
            .push("no interactive sample completed under load".into());
    }

    if let Some(one) = phases.iter().find(|p| p.sessions == 1) {
        report.throughput = Some(Throughput::new(
            one.bytes_total,
            one.chunks_delivered,
            Duration::from_secs_f64(one.seconds),
        ));
    }

    report.extra = serde_json::json!({
        "throughput": phases,
        "daemon_parse": parse,
        "interactive_pump_counts": counts_json(&pump),
        "interactive_pump_counts_under_load": counts_json(&loaded_pump),
    });
    report.duration_secs = started.elapsed().as_secs_f64();
    Ok(report)
}

fn counts_json(c: &PumpCounts) -> serde_json::Value {
    serde_json::json!({
        "reads": c.reads,
        "publishes": c.publishes,
        "wakeups": c.wakeups,
        "staged_bytes": c.staged_bytes,
        "idle_flushes": c.idle_flushes,
        "capped_flushes": c.capped_flushes,
        "parsed_bytes": c.parsed_bytes,
        "arenas": c.arenas,
    })
}

/// A spec that re-executes this binary as the emitting child.
fn spec_for(exe: &Path, args: Vec<String>, name: &str) -> SessionSpec {
    SessionSpec {
        project_id: ProjectId(1),
        cwd: workdir(),
        command: exe.to_string_lossy().into_owned(),
        args,
        env: Vec::new(),
        cols: 120,
        rows: 40,
        // Named, so the engine's title announcements do not rename the row.
        // That is what an agent session looks like, and an unpinned name would
        // make the measurement include a rename per block.
        title: Some(name.to_string()),
    }
}

/// A directory the child can start in that certainly exists.
fn workdir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir())
}

/// Stream `megabytes` out of each of `sessions` children at once.
async fn throughput(
    exe: &Path,
    sessions: usize,
    megabytes: usize,
    scrollback_bytes: usize,
) -> anyhow::Result<Phase> {
    let each = megabytes * 1024 * 1024;
    let manager = Arc::new(SessionManager::new(scrollback_bytes));

    let mut ids = Vec::with_capacity(sessions);
    let mut drains = Vec::with_capacity(sessions);
    for n in 0..sessions {
        let spec = spec_for(
            exe,
            vec!["emit".into(), "--bytes".into(), each.to_string()],
            &format!("firehose {n}"),
        );
        let id = manager.spawn(spec).context("spawning an emitter")?;
        let info = manager.info(id).context("a freshly spawned session")?;
        let rx = manager
            .attach(id, manager.new_viewer(), info.cols, info.rows)
            .context("attaching a viewer")?;
        ids.push(id);
        drains.push(tokio::spawn(drain(rx, each as u64)));
    }

    let before = allocated();
    let started = Instant::now();
    for &id in &ids {
        manager.write(id, b"\n").context("releasing an emitter")?;
    }

    let mut delivered = 0u64;
    let mut dropped = 0u64;
    for drain in drains {
        let outcome = tokio::time::timeout(DEADLINE, drain)
            .await
            .context("an emitter never finished streaming")?
            .context("a draining task panicked")?;
        delivered += outcome.chunks;
        dropped += outcome.dropped;
    }
    let seconds = started.elapsed().as_secs_f64();
    let after = allocated();

    let mut pump = PumpCounts::default();
    for &id in &ids {
        let counts = manager
            .pump_counts(id)
            .context("a session vanished mid-run")?;
        pump.reads += counts.reads;
        pump.publishes += counts.publishes;
        pump.wakeups += counts.wakeups;
        pump.staged_bytes += counts.staged_bytes;
        pump.parsed_bytes += counts.parsed_bytes;
        pump.arenas += counts.arenas;
    }
    for &id in &ids {
        let _ = manager.close(id);
    }

    let bytes_total = (each * sessions) as u64;
    let mb = bytes_total as f64 / (1024.0 * 1024.0);
    Ok(Phase {
        sessions,
        megabytes_each: megabytes,
        bytes_total,
        seconds,
        mb_per_sec: mb / seconds,
        mb_per_sec_per_session: mb / seconds / sessions as f64,
        chunks_delivered: delivered,
        chunks_dropped: dropped,
        reads_per_mb: pump.reads as f64 / mb,
        publishes_per_mb: pump.publishes as f64 / mb,
        wakeups_per_mb: pump.wakeups as f64 / mb,
        staged_bytes_per_mb: pump.staged_bytes as f64 / mb,
        parsed_bytes_per_mb: pump.parsed_bytes as f64 / mb,
        arenas_per_mb: pump.arenas as f64 / mb,
        allocs_per_mb: (after.0 - before.0) as f64 / mb,
        alloc_bytes_per_mb: (after.1 - before.1) as f64 / mb,
    })
}

/// What one client saw.
struct Drained {
    chunks: u64,
    dropped: u64,
}

/// Consume the live stream until it has carried `target` bytes.
///
/// Progress is measured with the sequence number rather than by adding up what
/// arrived, so a client that falls behind still knows where the stream got to.
/// Counting only delivered bytes would make a lagging run wait forever for
/// bytes that were dropped on purpose.
async fn drain(mut rx: broadcast::Receiver<OutputChunk>, target: u64) -> Drained {
    let mut chunks = 0u64;
    let mut dropped = 0u64;
    loop {
        match rx.recv().await {
            Ok(chunk) => {
                chunks += 1;
                if chunk.seq + chunk.data.len() as u64 >= target {
                    return Drained { chunks, dropped };
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => dropped += n,
            Err(broadcast::error::RecvError::Closed) => return Drained { chunks, dropped },
        }
    }
}

/// Time a single byte from the pty to the broadcast channel, repeatedly.
///
/// The byte is echoed by the terminal line discipline, not by the child, so
/// what is timed is the daemon's own path: read, coalesce, scan, scrollback,
/// publish. A child echoing in userspace would add its own scheduling to every
/// sample and hide the thing being measured.
async fn latency(
    exe: &Path,
    samples: usize,
    scrollback_bytes: usize,
    floods: usize,
) -> anyhow::Result<(Latencies, PumpCounts)> {
    let manager = Arc::new(SessionManager::new(scrollback_bytes));
    let spec = spec_for(
        exe,
        vec!["emit".into(), "--idle".into(), "600".into()],
        "interactive",
    );
    let id = manager.spawn(spec).context("spawning the idle child")?;
    // Sibling sessions streaming flat out for the whole measurement. This is
    // the question an operator asks: not what a keystroke costs on an idle
    // daemon, but what it costs while an agent is dumping a build log beside
    // it. Each emitter is given far more to write than the sample loop can
    // consume so none of them finishes early and quietly ends the load.
    let mut noisy = Vec::with_capacity(floods);
    for n in 0..floods {
        let spec = spec_for(
            exe,
            vec!["emit".into(), "--bytes".into(), (1usize << 40).to_string()],
            &format!("flood {n}"),
        );
        let flood = manager.spawn(spec).context("spawning a flooding sibling")?;
        let flood_info = manager.info(flood).context("a freshly spawned session")?;
        let rx = manager
            .attach(flood, manager.new_viewer(), flood_info.cols, flood_info.rows)
            .context("attaching to a flooding sibling")?;
        // Drained on a task so the broadcast does not simply back up: a client
        // that never reads makes the load disappear.
        tokio::spawn(drain(rx, u64::MAX));
        manager.write(flood, b"\n").context("releasing a flood")?;
        noisy.push(flood);
    }
    let info = manager.info(id).context("a freshly spawned session")?;
    let mut rx = manager
        .attach(id, manager.new_viewer(), info.cols, info.rows)
        .context("attaching a viewer")?;

    let mut recorded = Latencies::new();
    for _ in 0..samples {
        tokio::time::sleep(SAMPLE_GAP).await;
        // Anything already pending belongs to the previous sample.
        while rx.try_recv().is_ok() {}
        let at = Instant::now();
        manager.write(id, b"x").context("typing into the pty")?;
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(_)) => recorded.record(at.elapsed()),
            // A dropped sample is not a slow sample. Recording it as one would
            // put a number in the tail that no byte ever waited.
            Ok(Err(_)) | Err(_) => break,
        }
    }

    let pump = manager
        .pump_counts(id)
        .context("the interactive session vanished")?;
    for flood in noisy {
        let _ = manager.close(flood);
    }
    let _ = manager.close(id);
    Ok((recorded, pump))
}

/// Time the engine that used to sit on the read path over the same bytes, alone.
fn parse_cost(bytes: usize, pipeline_mb_per_sec: f64) -> anyhow::Result<ParseCost> {
    let block = block();
    let mut vt = vitrum_vt::Vt::new(vitrum_vt::VtOptions {
        cols: 120,
        rows: 40,
        max_scrollback: 0,
    })
    .context("building a terminal engine to time")?;

    let started = Instant::now();
    let mut fed = 0usize;
    while fed < bytes {
        let n = block.len().min(bytes - fed);
        vt.feed(&block[..n]);
        // Drained for the same reason the daemon drains them: an engine holding
        // every title it ever saw would be measuring a leak.
        let _ = vt.events().take_title();
        let _ = vt.events().take_pwd();
        fed += n;
    }
    let seconds = started.elapsed().as_secs_f64();
    let mb = fed as f64 / (1024.0 * 1024.0);
    let mb_per_sec = mb / seconds;
    Ok(ParseCost {
        bytes: fed as u64,
        seconds,
        mb_per_sec,
        share_of_pipeline: if mb_per_sec > 0.0 {
            pipeline_mb_per_sec / mb_per_sec
        } else {
            0.0
        },
    })
}
