//! Regression test suite for vitrum-core session concurrency, PTY stream chunking,
//! scrollback ring buffer slice boundaries, and child process status reaping.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tokio::time::{timeout, timeout_at, Instant};

use vitrum_core::{OutputChunk, Scrollback, SessionManager, SessionSpec, ViewerId};
use vitrum_proto::{ProjectId, SessionId, SessionStatus};

const DEADLINE: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(150);

fn shell(script: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), script.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        ("sh".to_string(), vec!["-c".to_string(), script.to_string()])
    }
}

fn shell_spec(script: &str) -> SessionSpec {
    let (command, args) = shell(script);
    SessionSpec {
        project_id: ProjectId(42),
        cwd: std::env::temp_dir(),
        command,
        args,
        env: Vec::new(),
        cols: 80,
        rows: 24,
        title: None,
    }
}

async fn wait_exit(mgr: &SessionManager, id: SessionId) -> Option<i32> {
    let rx = mgr
        .subscribe_status(id)
        .expect("session must exist to be awaited");
    wait_exit_on(rx).await
}

async fn wait_exit_on(mut rx: watch::Receiver<SessionStatus>) -> Option<i32> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let current = rx.borrow_and_update().clone();
        if let SessionStatus::Exited { code } = current {
            return code;
        }
        timeout_at(deadline, rx.changed())
            .await
            .expect("session did not reach a terminal state before deadline")
            .expect("status channel closed while session was still live");
    }
}

struct Collector {
    rx: broadcast::Receiver<OutputChunk>,
    next_seq: Option<u64>,
    pub first_seq: Option<u64>,
    pub bytes: Vec<u8>,
    pub chunks: usize,
}

impl Collector {
    fn new(rx: broadcast::Receiver<OutputChunk>) -> Self {
        Self {
            rx,
            next_seq: None,
            first_seq: None,
            bytes: Vec::new(),
            chunks: 0,
        }
    }

    async fn until(&mut self, mut stop: impl FnMut(&[u8]) -> bool) -> &[u8] {
        let deadline = Instant::now() + DEADLINE;
        while !stop(&self.bytes) {
            let chunk = timeout_at(deadline, self.rx.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "deadline passed with {} bytes collected: {:?}",
                        self.bytes.len(),
                        String::from_utf8_lossy(&self.bytes)
                    )
                });
            match chunk {
                Ok(c) => {
                    if let Some(expected) = self.next_seq {
                        assert_eq!(
                            c.seq, expected,
                            "output seq must be cumulative byte offset with no gaps"
                        );
                    }
                    self.first_seq = self.first_seq.or(Some(c.seq));
                    self.next_seq = Some(c.seq + c.data.len() as u64);
                    self.bytes.extend_from_slice(&c.data);
                    self.chunks += 1;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    panic!("test client lagged and lost {n} chunks")
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        &self.bytes
    }

    async fn expect_quiet(&mut self) {
        let r = timeout(QUIET, self.rx.recv()).await;
        assert!(
            r.is_err(),
            "expected no further output, got {:?}",
            r.map(|c| c.map(|c| c.seq))
        );
    }
}

// ---------------------------------------------------------------------------
// Area 1: SessionManager Concurrent Lookups under 50 Task Threads
// ---------------------------------------------------------------------------

/// WHY: Validates that `SessionManager` handles high-concurrency read lookups
/// (`info`, `list`, `child_pid`, `watchers`, `probe_count`, `resize_count`)
/// across 50 concurrent tokio tasks without contention deadlocks, race conditions,
/// or data corruption while sessions are active.
#[tokio::test]
async fn test_concurrent_session_lookups_50_threads() {
    let mgr = Arc::new(SessionManager::new(64 * 1024));

    // Spawn 5 long-running sessions
    let mut session_ids = Vec::new();
    for i in 0..5 {
        let mut spec = shell_spec("sleep 30");
        spec.title = Some(format!("Concurrent Session {i}"));
        let id = mgr.spawn(spec).expect("spawn must succeed");
        session_ids.push(id);
    }

    let mut handles = Vec::new();
    for task_idx in 0..50 {
        let mgr = Arc::clone(&mgr);
        let ids = session_ids.clone();
        handles.push(tokio::spawn(async move {
            for iter in 0..100 {
                let list = mgr.list();
                assert_eq!(list.len(), 5, "list length must remain constant");

                let target_id = ids[iter % ids.len()];
                let info = mgr.info(target_id).expect("session info must exist");
                assert_eq!(info.id, target_id);

                let pid = mgr.child_pid(target_id);
                assert!(pid.is_some(), "live session must have a child_pid");

                let watchers = mgr.watchers(target_id);
                assert_eq!(watchers, Some(0));

                let probes = mgr.probe_count(target_id);
                assert!(probes.is_some());

                let resizes = mgr.resize_count(target_id);
                assert!(resizes.is_some());

                if task_idx % 2 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        }));
    }

    for h in handles {
        h.await.expect("task thread panicked during concurrent lookups");
    }

    // Cleanup spawned sessions
    for id in session_ids {
        mgr.close(id).expect("close must succeed");
    }
}

/// WHY: Validates that concurrent spawns, lookups (`info`, `scrollback`), and closes
/// across 50 task threads operate safely under heavy lock contention without leaking
/// sessions or deadlocking the manager registry.
#[tokio::test]
async fn test_concurrent_session_spawn_lookup_close_50_threads() {
    let mgr = Arc::new(SessionManager::new(32 * 1024));
    let mut handles = Vec::new();

    for i in 0..50 {
        let mgr = Arc::clone(&mgr);
        handles.push(tokio::spawn(async move {
            let mut spec = shell_spec("echo 'hello concurrency'");
            spec.title = Some(format!("Task-{i}"));
            let id = mgr.spawn(spec).expect("spawn session");

            let info = mgr.info(id).expect("fetch session info");
            assert_eq!(info.id, id);

            let _ = mgr.write(id, b"echo test\n");
            let _ = mgr.scrollback(id, u64::MAX, 1024);

            let exit = wait_exit(&mgr, id).await;
            assert_eq!(exit, Some(0));

            mgr.close(id).expect("close session");
            assert!(mgr.info(id).is_none(), "info must be None after close");
        }));
    }

    for h in handles {
        h.await.expect("spawn-lookup-close task panicked");
    }

    assert_eq!(mgr.list().len(), 0, "registry must be empty after all sessions closed");
}

/// WHY: Defends against lock poisoning, out-of-bounds access, or crash panics
/// when 50 concurrent task threads query nonexistent, zero-valued, or u64::MAX
/// `SessionId`s during active session registry operations.
#[tokio::test]
async fn test_concurrent_lookup_nonexistent_and_invalid_ids() {
    let mgr = Arc::new(SessionManager::new(16 * 1024));
    let id_real = mgr.spawn(shell_spec("sleep 30")).expect("spawn real session");

    let mut handles = Vec::new();
    let bogus_ids = vec![
        SessionId(0),
        SessionId(999_999),
        SessionId(u64::MAX),
        SessionId(id_real.0 + 100),
    ];

    for _ in 0..50 {
        let mgr = Arc::clone(&mgr);
        let boguses = bogus_ids.clone();
        handles.push(tokio::spawn(async move {
            for &bogus in &boguses {
                assert!(mgr.info(bogus).is_none());
                assert!(mgr.child_pid(bogus).is_none());
                assert!(mgr.watchers(bogus).is_none());
                assert!(mgr.probe_count(bogus).is_none());
                assert!(mgr.resize_count(bogus).is_none());
                assert!(mgr.scrollback(bogus, u64::MAX, 100).is_none());
                assert!(mgr.write(bogus, b"test").is_err());
                assert!(mgr.rename(bogus, "new title").is_err());
                assert!(mgr.close(bogus).is_err());
            }
        }));
    }

    for h in handles {
        h.await.expect("bogus lookup task panicked");
    }

    mgr.close(id_real).expect("close real session");
}

/// WHY: Ensures atomic allocation of `ViewerId`s via `new_viewer()` and safe
/// registration/deregistration of viewer attachments and geometry resizes
/// under 50 concurrent task threads.
#[tokio::test]
async fn test_concurrent_viewer_attachment_and_resize_50_threads() {
    let mgr = Arc::new(SessionManager::new(64 * 1024));
    let id = mgr.spawn(shell_spec("sleep 30")).expect("spawn session");

    let mut handles = Vec::new();

    for task_id in 0..50 {
        let mgr = Arc::clone(&mgr);
        handles.push(tokio::spawn(async move {
            let viewer = mgr.new_viewer();
            assert!(viewer.0 > 0, "viewer id must be strictly positive");

            let cols = (80 + task_id % 40) as u16;
            let rows = (24 + task_id % 20) as u16;

            let _rx = mgr.attach(id, viewer, cols, rows).expect("attach viewer");
            mgr.resize(id, viewer, cols + 5, rows + 5).expect("resize viewer");

            mgr.detach(id, viewer);
        }));
    }

    for h in handles {
        h.await.expect("viewer attachment task panicked");
    }

    mgr.close(id).expect("close session");
}

// ---------------------------------------------------------------------------
// Area 2: Bytes PTY Stream Chunking
// ---------------------------------------------------------------------------

/// WHY: Defends the PTY byte stream chunking logic by ensuring large PTY outputs
/// (>64KB `FLUSH_BYTES`) are split into bounded `OutputChunk`s while preserving sequence
/// order (`seq`) and byte integrity.
#[tokio::test]
async fn test_pty_stream_chunking_large_burst_flush_boundaries() {
    let mgr = SessionManager::new(512 * 1024);

    // Generate ~150KB of repetitive output
    let chunk_count = 150;
    let pattern_block = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let total_bytes = pattern_block.len() * 30 * chunk_count; // 162,000 bytes

    let script = format!(
        "python3 -c \"import sys; data = b'{}' * 30; sys.stdout.buffer.write(data * {chunk_count}); sys.stdout.flush()\"",
        pattern_block
    );

    let id = mgr.spawn(shell_spec(&script)).expect("spawn python generator");
    let mut collector = Collector::new(mgr.attach(id, ViewerId(1), 80, 24).unwrap());

    collector.until(|b| b.len() >= total_bytes).await;

    assert!(
        collector.chunks >= 2,
        "large output burst (>64KB) must be chunked across multiple OutputChunk broadcasts"
    );
    assert_eq!(
        collector.bytes.len(),
        total_bytes,
        "all output bytes must be received without loss or corruption"
    );

    wait_exit(&mgr, id).await;
    mgr.close(id).unwrap();
}

/// WHY: Ensures rapid micro-bursts of PTY output are correctly coalesced within
/// `FLUSH_WINDOW` without generating excessive single-byte broadcast notifications
/// or dropping bytes.
#[tokio::test]
async fn test_pty_stream_chunking_rapid_micro_bursts_coalescing() {
    let mgr = SessionManager::new(64 * 1024);
    let id = mgr
        .spawn(shell_spec("for i in $(seq 1 20); do printf 'line %d\\n' $i; sleep 0.001; done"))
        .expect("spawn loop");

    let mut collector = Collector::new(mgr.attach(id, ViewerId(1), 80, 24).unwrap());

    let output = collector.until(|b| b.ends_with(b"line 20\r\n") || b.ends_with(b"line 20\n")).await;
    assert!(!output.is_empty());

    // 20 prints with 1ms sleep should coalesce into far fewer than 20 chunks
    assert!(
        collector.chunks < 20,
        "micro-bursts must be coalesced into fewer chunks (got {} chunks)",
        collector.chunks
    );

    wait_exit(&mgr, id).await;
    mgr.close(id).unwrap();
}

/// WHY: Validates that PTY stream chunking safely preserves raw binary payloads
/// (null bytes, invalid UTF-8 sequences, ANSI control codes) without UTF-8 parsing
/// panics, string truncation, or sequence gaps.
#[tokio::test]
async fn test_pty_stream_chunking_adversarial_binary_payloads() {
    let mgr = SessionManager::new(64 * 1024);
    let script = "python3 -c \"import sys; sys.stdout.buffer.write(bytes([0x00, 0xFF, 0xFE, 0x80, 0x1B, 0x5B, 0x33, 0x31, 0x6D, 0x07])); sys.stdout.flush()\"";

    let id = mgr.spawn(shell_spec(script)).expect("spawn binary script");
    let mut collector = Collector::new(mgr.attach(id, ViewerId(1), 80, 24).unwrap());

    let bytes = collector.until(|b| b.len() >= 10).await;
    let expected: &[u8] = &[0x00, 0xFF, 0xFE, 0x80, 0x1B, 0x5B, 0x33, 0x31, 0x6D, 0x07];

    assert_eq!(bytes, expected, "raw binary payload must be chunked verbatim without modification");

    wait_exit(&mgr, id).await;
    mgr.close(id).unwrap();
}

/// WHY: Tests that zero-length writes or empty read buffers do not advance
/// stream sequence numbers or cause invalid empty `OutputChunk` emissions.
#[test]
fn test_pty_stream_chunking_zero_byte_and_empty_reads() {
    let mut ring = Scrollback::with_capacity(1024);
    assert_eq!(ring.head_seq(), 0);
    assert_eq!(ring.oldest_seq(), 0);

    // Pushing empty slice
    ring.push(b"");
    assert_eq!(ring.head_seq(), 0, "empty push must not advance sequence");
    assert_eq!(ring.len(), 0);
    assert!(ring.is_empty());

    // Pushing real bytes
    ring.push(b"hello");
    assert_eq!(ring.head_seq(), 5);
    assert_eq!(ring.oldest_seq(), 0);
    assert_eq!(ring.len(), 5);

    // Pushing empty slice again
    ring.push(b"");
    assert_eq!(ring.head_seq(), 5);
    assert_eq!(ring.len(), 5);
}

// ---------------------------------------------------------------------------
// Area 3: Scrollback Ring Buffer Slice Boundaries
// ---------------------------------------------------------------------------

/// WHY: Defends ring buffer slice boundary calculation in `Scrollback::halves()` and
/// `Scrollback::range()` when data wraps around the capacity limit, verifying zero
/// missing bytes or index out-of-bounds panics.
#[test]
fn test_scrollback_ring_slice_boundaries_halves_wrap() {
    let mut sb = Scrollback::with_capacity(100);

    // Fill initial 60 bytes
    let data1: Vec<u8> = (0..60).map(|i| i as u8).collect();
    sb.push(&data1);
    assert_eq!(sb.len(), 60);
    let (h1, h2) = sb.halves();
    assert_eq!(h1.len(), 60);
    assert_eq!(h2.len(), 0);

    // Push 80 more bytes -> total 140 bytes, capacity 100
    let data2: Vec<u8> = (60..140).map(|i| i as u8).collect();
    sb.push(&data2);

    assert_eq!(sb.len(), 100);
    assert_eq!(sb.oldest_seq(), 40);
    assert_eq!(sb.head_seq(), 140);

    let (first, second) = sb.halves();
    assert_eq!(first.len() + second.len(), 100);

    let mut reconstructed = Vec::new();
    reconstructed.extend_from_slice(first);
    reconstructed.extend_from_slice(second);

    let expected: Vec<u8> = (40..140).map(|i| i as u8).collect();
    assert_eq!(reconstructed, expected, "halves must reconstruct the exact retained byte range");

    // Test range retrieval across boundary seam
    let range_bytes = sb.range(40, 100).expect("range inside retained");
    assert_eq!(range_bytes, expected);
}

/// WHY: Defends boundary behavior when `Scrollback` capacity is configured to 0 bytes,
/// ensuring sequence numbers (`head_seq`, `oldest_seq`) advance correctly while memory
/// usage remains zero and lookups return empty/None appropriately.
#[test]
fn test_scrollback_zero_capacity_boundary() {
    let mut sb = Scrollback::with_capacity(0);

    sb.push(b"hello world zero capacity");

    assert_eq!(sb.len(), 0);
    assert!(sb.is_empty());
    assert_eq!(sb.head_seq(), 25);
    assert_eq!(sb.oldest_seq(), 25);

    let (h1, h2) = sb.halves();
    assert_eq!(h1, b"");
    assert_eq!(h2, b"");

    // Lookup at head_seq returns empty vec
    assert_eq!(sb.range(25, 10), Some(vec![]));

    // Lookup before oldest_seq returns None
    assert_eq!(sb.range(0, 10), None);
    assert_eq!(sb.range(24, 10), None);

    // Lookup past head_seq returns None
    assert_eq!(sb.range(26, 10), None);
}

/// WHY: Tests exact boundary conditions when pushing data that fills the scrollback ring
/// to 100% capacity and beyond, verifying `oldest_seq` calculation and contiguous slice
/// retrieval via `range` and `halves`.
#[test]
fn test_scrollback_exact_capacity_fill_and_eviction_slice() {
    let cap = 256;
    let mut sb = Scrollback::with_capacity(cap);

    let payload: Vec<u8> = (0..256).map(|i| i as u8).collect();
    sb.push(&payload);

    assert_eq!(sb.len(), cap);
    assert_eq!(sb.oldest_seq(), 0);
    assert_eq!(sb.head_seq(), 256);

    let (h1, h2) = sb.halves();
    assert_eq!(h1.len(), 256);
    assert_eq!(h2.len(), 0);
    assert_eq!(h1, payload.as_slice());

    // Push exactly 1 byte to trigger wrap eviction
    sb.push(&[255]);
    assert_eq!(sb.len(), cap);
    assert_eq!(sb.oldest_seq(), 1);
    assert_eq!(sb.head_seq(), 257);

    let fetched = sb.range(1, 256).expect("range 1..257");
    let mut expected = payload[1..].to_vec();
    expected.push(255);
    assert_eq!(fetched, expected);
}

/// WHY: Defends `Scrollback::range()` against adversarial inputs (out-of-bound `before_seq`,
/// `from_seq < oldest_seq`, `from_seq > head_seq`, high sequence values) returning `None`
/// or safely clamped vectors without arithmetic underflow/overflow panics.
#[test]
fn test_scrollback_malformed_range_queries_and_seq_overflow() {
    let mut sb = Scrollback::with_capacity(50);
    sb.push(b"0123456789"); // head_seq = 10, oldest_seq = 0, len = 10

    // Valid range
    assert_eq!(sb.range(0, 5), Some(b"01234".to_vec()));

    // from_seq past head_seq
    assert_eq!(sb.range(11, 5), None);
    assert_eq!(sb.range(u64::MAX, 5), None);

    // max_bytes = 0
    assert_eq!(sb.range(5, 0), Some(vec![]));

    // max_bytes larger than retained
    assert_eq!(sb.range(0, 1000), Some(b"0123456789".to_vec()));

    // Push 100 bytes to evict initial 10
    let big: Vec<u8> = vec![b'A'; 100];
    sb.push(&big); // total head_seq = 110, len = 50, oldest_seq = 60

    // Requesting evicted range (0..10) must return None
    assert_eq!(sb.range(0, 10), None);
    assert_eq!(sb.range(59, 1), None);

    // Requesting exactly oldest_seq (60)
    assert_eq!(sb.range(60, 5).unwrap().len(), 5);
}

// ---------------------------------------------------------------------------
// Area 4: Child Process Status Reaping
// ---------------------------------------------------------------------------

/// WHY: Verifies that short-lived child processes exiting cleanly with status 0 are
/// reaped promptly, transitioning session status to `Exited { code: Some(0) }` and
/// clearing `child_pid()` to prevent PID reuse hazards.
#[tokio::test]
async fn test_child_process_status_reaping_clean_exit() {
    let mgr = SessionManager::new(16 * 1024);
    let id = mgr.spawn(shell_spec("exit 0")).expect("spawn exit 0");

    let pid_before = mgr.child_pid(id);
    assert!(pid_before.is_some(), "pid must be present while starting/running");

    let exit_code = wait_exit(&mgr, id).await;
    assert_eq!(exit_code, Some(0), "clean exit status must be Some(0)");

    let info = mgr.info(id).expect("session info preserved after exit");
    assert_eq!(info.status, SessionStatus::Exited { code: Some(0) });

    let pid_after = mgr.child_pid(id);
    assert!(
        pid_after.is_none(),
        "child_pid must return None after child process is reaped"
    );

    mgr.close(id).unwrap();
}

/// WHY: Verifies that child processes failing with non-zero exit codes (e.g. exit code 42)
/// report exact exit statuses to status subscribers upon process reaping.
#[tokio::test]
async fn test_child_process_status_reaping_non_zero_exit() {
    let mgr = SessionManager::new(16 * 1024);
    let id = mgr.spawn(shell_spec("exit 42")).expect("spawn exit 42");

    let exit_code = wait_exit(&mgr, id).await;
    assert_eq!(exit_code, Some(42), "exit code 42 must be reported accurately");

    let info = mgr.info(id).expect("info preserved");
    assert_eq!(info.status, SessionStatus::Exited { code: Some(42) });

    mgr.close(id).unwrap();
}

/// WHY: Verifies that calling `SessionManager::close()` on a running process safely
/// kills and reaps the child process without hanging or blocking manager operations.
#[tokio::test]
async fn test_child_process_status_reaping_forced_close_kill() {
    let mgr = SessionManager::new(16 * 1024);
    let id = mgr.spawn(shell_spec("sleep 60")).expect("spawn long sleep");

    let status_rx = mgr.subscribe_status(id).expect("status rx");
    let pid = mgr.child_pid(id).expect("live pid");
    assert!(pid > 0);

    // Close while child process is sleeping
    let start = Instant::now();
    mgr.close(id).expect("close must succeed on live process");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "close() on a live process must return promptly without blocking on sleep"
    );

    // Wait for the exit status notification
    let _exit_code = wait_exit_on(status_rx).await;

    // Registry lookup should now be None
    assert!(mgr.info(id).is_none());
}
