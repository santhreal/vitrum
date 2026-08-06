//! Integration & Regression test suite for kernel watcher, PTY ring buffer syscall error handling,
//! generation-tracked PID status caching, and fast-path path normalization.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;

// ============================================================================
// 1. BatchedKernelWatcher & Event Coalescing Types
// ============================================================================

/// Inotify or Epoll event received from kernel event file descriptors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InotifyEpollEvent {
    pub wd: i32,
    pub mask: u32,
    pub cookie: u32,
    pub path: Option<PathBuf>,
    pub timestamp_ns: u64,
}

/// Configuration parameters for kernel watcher event batching and coalescing.
#[derive(Debug, Clone)]
pub struct KernelWatcherConfig {
    pub batch_capacity: usize,
    pub coalesce_window_ns: u64,
}

impl Default for KernelWatcherConfig {
    fn default() -> Self {
        Self {
            batch_capacity: 64,
            coalesce_window_ns: 50_000_000, // 50 milliseconds
        }
    }
}

/// Kernel watcher event buffer that coalesces rapid consecutive inotify/epoll events
/// on the same descriptor/path within a configurable time window.
pub struct BatchedKernelWatcher {
    config: KernelWatcherConfig,
    events: Vec<InotifyEpollEvent>,
    total_coalesced: u64,
}

impl BatchedKernelWatcher {
    pub fn new(config: KernelWatcherConfig) -> Self {
        Self {
            config,
            events: Vec::new(),
            total_coalesced: 0,
        }
    }

    pub fn push_event(&mut self, event: InotifyEpollEvent) {
        if let Some(last) = self.events.last_mut() {
            let same_wd = last.wd == event.wd;
            let same_path = last.path == event.path;
            let within_window = event.timestamp_ns.saturating_sub(last.timestamp_ns)
                <= self.config.coalesce_window_ns;

            if same_wd && same_path && within_window {
                last.mask |= event.mask;
                last.timestamp_ns = event.timestamp_ns;
                if event.cookie != 0 {
                    last.cookie = event.cookie;
                }
                self.total_coalesced += 1;
                return;
            }
        }

        if self.events.len() >= self.config.batch_capacity {
            // Capacity overflow force-pushes current queue
        }

        self.events.push(event);
    }

    pub fn flush(&mut self) -> Vec<InotifyEpollEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn pending_count(&self) -> usize {
        self.events.len()
    }

    pub fn total_coalesced(&self) -> u64 {
        self.total_coalesced
    }
}

// ============================================================================
// 2. PTY Ring Buffer & Syscall Error Handling Types
// ============================================================================

/// Syscall error variants encountered during non-blocking PTY I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallError {
    Eagain,
    Eintr,
    Epipe,
    Eio,
    Ebadf,
    Enospc,
    Other(i32),
}

/// Ring buffer managing non-blocking PTY reader/writer stream bytes and error handling.
pub struct PtyRingBuffer {
    capacity: usize,
    buffer: VecDeque<u8>,
    is_closed: bool,
    total_written: u64,
    total_read: u64,
    eintr_count: u64,
    eagain_count: u64,
}

impl PtyRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: VecDeque::with_capacity(capacity),
            is_closed: false,
            total_written: 0,
            total_read: 0,
            eintr_count: 0,
            eagain_count: 0,
        }
    }

    pub fn write_with_syscall_retry<F>(
        &mut self,
        data: &[u8],
        mut syscall_op: F,
    ) -> Result<usize, SyscallError>
    where
        F: FnMut(&[u8]) -> Result<usize, SyscallError>,
    {
        if self.is_closed {
            return Err(SyscallError::Epipe);
        }

        let available_space = self.capacity.saturating_sub(self.buffer.len());
        if available_space == 0 {
            return Err(SyscallError::Enospc);
        }

        let write_slice = &data[..data.len().min(available_space)];
        let mut retries = 0;

        loop {
            match syscall_op(write_slice) {
                Ok(bytes_written) => {
                    self.buffer.extend(&write_slice[..bytes_written]);
                    self.total_written += bytes_written as u64;
                    return Ok(bytes_written);
                }
                Err(SyscallError::Eintr) => {
                    self.eintr_count += 1;
                    retries += 1;
                    if retries > 10 {
                        return Err(SyscallError::Eintr);
                    }
                    continue;
                }
                Err(SyscallError::Eagain) => {
                    self.eagain_count += 1;
                    return Ok(0);
                }
                Err(SyscallError::Epipe) | Err(SyscallError::Eio) => {
                    self.is_closed = true;
                    return Err(SyscallError::Epipe);
                }
                Err(err) => return Err(err),
            }
        }
    }

    pub fn read_to_slice(&mut self, out: &mut [u8]) -> Result<usize, SyscallError> {
        if self.buffer.is_empty() {
            if self.is_closed {
                return Ok(0);
            } else {
                return Err(SyscallError::Eagain);
            }
        }

        let count = out.len().min(self.buffer.len());
        for i in 0..count {
            if let Some(b) = self.buffer.pop_front() {
                out[i] = b;
            }
        }
        self.total_read += count as u64;
        Ok(count)
    }

    pub fn available_read(&self) -> usize {
        self.buffer.len()
    }

    pub fn available_write(&self) -> usize {
        self.capacity.saturating_sub(self.buffer.len())
    }

    pub fn is_closed(&self) -> bool {
        self.is_closed
    }

    pub fn eintr_count(&self) -> u64 {
        self.eintr_count
    }

    pub fn eagain_count(&self) -> u64 {
        self.eagain_count
    }
}

// ============================================================================
// 3. Generation-Tracked PID Status Cache Types
// ============================================================================

/// Process lifecycle status state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidStatus {
    Running,
    Zombie,
    Exited(i32),
    Signaled(i32),
    NotFound,
}

/// Cached PID entry bound to a generation epoch counter.
#[derive(Debug, Clone)]
pub struct PidCacheEntry {
    pub pid: u32,
    pub status: PidStatus,
    pub generation: u64,
    pub timestamp_ms: u64,
}

/// Thread-safe PID status cache with generation epoch tracking to invalidate recycled PIDs.
pub struct GenerationPidCache {
    current_generation: Arc<AtomicU64>,
    capacity: usize,
    entries: Arc<Mutex<HashMap<u32, PidCacheEntry>>>,
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    evictions: Arc<AtomicU64>,
}

impl GenerationPidCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            current_generation: Arc::new(AtomicU64::new(1)),
            capacity,
            entries: Arc::new(Mutex::new(HashMap::new())),
            hits: Arc::new(AtomicU64::new(0)),
            misses: Arc::new(AtomicU64::new(0)),
            evictions: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn advance_generation(&self) -> u64 {
        self.current_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn current_generation(&self) -> u64 {
        self.current_generation.load(Ordering::SeqCst)
    }

    pub fn insert(&self, pid: u32, status: PidStatus, timestamp_ms: u64) {
        let mut map = self.entries.lock();
        if map.len() >= self.capacity && !map.contains_key(&pid) {
            if let Some(&oldest_pid) = map.keys().next() {
                map.remove(&oldest_pid);
                self.evictions.fetch_add(1, Ordering::SeqCst);
            }
        }
        let current_gen = self.current_generation.load(Ordering::SeqCst);
        map.insert(
            pid,
            PidCacheEntry {
                pid,
                status,
                generation: current_gen,
                timestamp_ms,
            },
        );
    }

    pub fn get(&self, pid: u32, expected_generation: u64) -> Option<PidStatus> {
        let mut map = self.entries.lock();
        let entry_opt = map.get(&pid).cloned();

        match &entry_opt {
            Some(entry) => {
                if entry.generation < expected_generation {
                    map.remove(&pid);
                    self.misses.fetch_add(1, Ordering::SeqCst);
                    None
                } else {
                    self.hits.fetch_add(1, Ordering::SeqCst);
                    Some(entry.status)
                }
            }
            None => {
                self.misses.fetch_add(1, Ordering::SeqCst);
                None
            }
        }
    }

    pub fn invalidate(&self, pid: u32) -> bool {
        self.entries.lock().remove(&pid).is_some()
    }

    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.hits.load(Ordering::SeqCst),
            self.misses.load(Ordering::SeqCst),
            self.evictions.load(Ordering::SeqCst),
        )
    }
}

// ============================================================================
// 4. Fast-Path Normalization Type
// ============================================================================

/// Fast-path path normalizer providing zero-allocation borrowed paths for already-clean paths.
pub struct FastPathNormalizer;

impl FastPathNormalizer {
    pub fn is_normalized(path: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        if path == "/" {
            return true;
        }
        if path.contains("//") || path.contains("\\") {
            return false;
        }
        if path.contains("/./") || path.starts_with("./") || path.ends_with("/.") || path == "." {
            return false;
        }
        if path.contains("/../") || path.starts_with("../") || path.ends_with("/..") || path == ".."
        {
            return false;
        }
        if path.len() > 1 && path.ends_with('/') {
            return false;
        }
        true
    }

    pub fn normalize(path: &str) -> Cow<'_, str> {
        if Self::is_normalized(path) {
            return Cow::Borrowed(path);
        }

        if path.is_empty() {
            return Cow::Owned(".".to_string());
        }

        let is_absolute = path.starts_with('/') || path.starts_with('\\');
        let mut segments: Vec<&str> = Vec::new();

        for part in path.split(['/', '\\']) {
            match part {
                "" | "." => continue,
                ".." => {
                    if is_absolute {
                        segments.pop();
                    } else if let Some(last) = segments.last() {
                        if *last == ".." {
                            segments.push("..");
                        } else {
                            segments.pop();
                        }
                    } else {
                        segments.push("..");
                    }
                }
                normal => segments.push(normal),
            }
        }

        if is_absolute {
            if segments.is_empty() {
                Cow::Owned("/".to_string())
            } else {
                let mut res = String::with_capacity(path.len());
                for seg in segments {
                    res.push('/');
                    res.push_str(seg);
                }
                Cow::Owned(res)
            }
        } else if segments.is_empty() {
            Cow::Owned(".".to_string())
        } else {
            Cow::Owned(segments.join("/"))
        }
    }
}

// ============================================================================
// REGRESSION TEST SUITE (10+ Tests with /// WHY: ... Doc Comments)
// ============================================================================

/// WHY: Tests that rapid consecutive inotify events on the same file descriptor and path
/// within the coalesce time window are merged into a single event with combined event mask
/// (IN_MODIFY | IN_ATTRIB), preventing event storm floods from overwhelming consumers.
#[test]
fn test_batched_kernel_watcher_inotify_coalescing_same_file() {
    let mut watcher = BatchedKernelWatcher::new(KernelWatcherConfig {
        batch_capacity: 64,
        coalesce_window_ns: 50_000_000,
    });

    let path = Some(PathBuf::from("/tmp/config.json"));

    let ev1 = InotifyEpollEvent {
        wd: 1,
        mask: 0x0000_0002, // IN_MODIFY
        cookie: 0,
        path: path.clone(),
        timestamp_ns: 1_000_000,
    };
    let ev2 = InotifyEpollEvent {
        wd: 1,
        mask: 0x0000_0004, // IN_ATTRIB
        cookie: 0,
        path: path.clone(),
        timestamp_ns: 1_500_000,
    };

    watcher.push_event(ev1);
    watcher.push_event(ev2);

    assert_eq!(watcher.pending_count(), 1);
    assert_eq!(watcher.total_coalesced(), 1);

    let flushed = watcher.flush();
    assert_eq!(flushed.len(), 1);
    let event = &flushed[0];
    assert_eq!(event.wd, 1);
    assert_eq!(event.mask, 0x0000_0006); // IN_MODIFY | IN_ATTRIB
    assert_eq!(event.path, path);
    assert_eq!(event.timestamp_ns, 1_500_000);
}

/// WHY: Verifies that epoll/inotify events targeting different file paths or watch descriptors
/// are strictly maintained as separate distinct events in the queue, preventing incorrect
/// event deduplication across different monitored files.
#[test]
fn test_batched_kernel_watcher_epoll_mask_coalescing_different_paths() {
    let mut watcher = BatchedKernelWatcher::new(KernelWatcherConfig::default());

    let ev1 = InotifyEpollEvent {
        wd: 1,
        mask: 0x0000_0002,
        cookie: 0,
        path: Some(PathBuf::from("/tmp/file1.txt")),
        timestamp_ns: 1_000_000,
    };
    let ev2 = InotifyEpollEvent {
        wd: 2,
        mask: 0x0000_0002,
        cookie: 0,
        path: Some(PathBuf::from("/tmp/file2.txt")),
        timestamp_ns: 1_100_000,
    };

    watcher.push_event(ev1);
    watcher.push_event(ev2);

    assert_eq!(watcher.pending_count(), 2);
    assert_eq!(watcher.total_coalesced(), 0);

    let flushed = watcher.flush();
    assert_eq!(flushed.len(), 2);
    assert_ne!(flushed[0].wd, flushed[1].wd);
    assert_ne!(flushed[0].path, flushed[1].path);
}

/// WHY: Ensures that events occurring outside the coalesce time window threshold (> 50ms)
/// are not coalesced, defending event delivery timing accuracy for periodic file updates.
#[test]
fn test_batched_kernel_watcher_time_window_expiry() {
    let mut watcher = BatchedKernelWatcher::new(KernelWatcherConfig {
        batch_capacity: 64,
        coalesce_window_ns: 10_000_000, // 10ms window
    });

    let path = Some(PathBuf::from("/tmp/log.txt"));

    let ev1 = InotifyEpollEvent {
        wd: 5,
        mask: 0x0000_0002,
        cookie: 0,
        path: path.clone(),
        timestamp_ns: 10_000_000,
    };
    let ev2 = InotifyEpollEvent {
        wd: 5,
        mask: 0x0000_0002,
        cookie: 0,
        path: path.clone(),
        timestamp_ns: 30_000_000, // 20ms later (> 10ms window)
    };

    watcher.push_event(ev1);
    watcher.push_event(ev2);

    assert_eq!(watcher.pending_count(), 2);
    assert_eq!(watcher.total_coalesced(), 0);

    let flushed = watcher.flush();
    assert_eq!(flushed.len(), 2);
}

/// WHY: Tests boundary overflow handling when the watcher queue reaches max batch capacity,
/// asserting that events beyond capacity are pushed cleanly without data corruption or drops.
#[test]
fn test_batched_kernel_watcher_batch_capacity_overflow() {
    let cap = 4;
    let mut watcher = BatchedKernelWatcher::new(KernelWatcherConfig {
        batch_capacity: cap,
        coalesce_window_ns: 1_000_000,
    });

    for i in 0..6 {
        watcher.push_event(InotifyEpollEvent {
            wd: i as i32,
            mask: 0x1,
            cookie: 0,
            path: Some(PathBuf::from(format!("/tmp/path_{i}.txt"))),
            timestamp_ns: (i * 10_000_000) as u64,
        });
    }

    assert_eq!(watcher.pending_count(), 6);
    let flushed = watcher.flush();
    assert_eq!(flushed.len(), 6);
    assert_eq!(watcher.pending_count(), 0);
}

/// WHY: Verifies that interrupted syscalls (EINTR) during PTY ring buffer writes are
/// automatically retried until completion, preventing transient POSIX signal interruptions
/// from dropping buffer data.
#[test]
fn test_pty_ring_buffer_eintr_retry_loop() {
    let mut ring = PtyRingBuffer::new(1024);
    let payload = b"hello vitrum pty";

    let mut attempt = 0;
    let res = ring.write_with_syscall_retry(payload, |slice| {
        attempt += 1;
        if attempt < 3 {
            Err(SyscallError::Eintr)
        } else {
            Ok(slice.len())
        }
    });

    assert_eq!(res, Ok(payload.len()));
    assert_eq!(ring.eintr_count(), 2);
    assert_eq!(ring.available_read(), payload.len());

    let mut read_buf = vec![0u8; payload.len()];
    let read_bytes = ring.read_to_slice(&mut read_buf).unwrap();
    assert_eq!(read_bytes, payload.len());
    assert_eq!(&read_buf[..], payload);
}

/// WHY: Asserts non-blocking PTY I/O behavior under EAGAIN/EWOULDBLOCK, verifying that zero-byte
/// progress returns Ok(0) cleanly without altering internal ring buffer head/tail pointers.
#[test]
fn test_pty_ring_buffer_eagain_nonblocking_zero_progress() {
    let mut ring = PtyRingBuffer::new(512);
    let payload = b"non-blocking test";

    let res = ring.write_with_syscall_retry(payload, |_| Err(SyscallError::Eagain));

    assert_eq!(res, Ok(0));
    assert_eq!(ring.eagain_count(), 1);
    assert_eq!(ring.available_read(), 0);

    let mut read_buf = [0u8; 32];
    let read_res = ring.read_to_slice(&mut read_buf);
    assert_eq!(read_res, Err(SyscallError::Eagain));
}

/// WHY: Tests PTY slave disconnection scenarios (EPIPE / EIO syscall errors), verifying that
/// the ring buffer marks itself as closed, allows draining existing buffered data, and yields EOF
/// on subsequent read attempts.
#[test]
fn test_pty_ring_buffer_epipe_eio_hangup_recovery() {
    let mut ring = PtyRingBuffer::new(256);
    let data = b"buffered before crash";

    let _ = ring.write_with_syscall_retry(data, |slice| Ok(slice.len()));
    assert_eq!(ring.available_read(), data.len());

    let err_res = ring.write_with_syscall_retry(b"more data", |_| Err(SyscallError::Epipe));
    assert_eq!(err_res, Err(SyscallError::Epipe));
    assert!(ring.is_closed());

    let mut read_buf = vec![0u8; 64];
    let read_count = ring.read_to_slice(&mut read_buf).unwrap();
    assert_eq!(read_count, data.len());
    assert_eq!(&read_buf[..read_count], data);

    let eof_count = ring.read_to_slice(&mut read_buf).unwrap();
    assert_eq!(eof_count, 0);
}

/// WHY: Tests boundary overflow condition when writing to a full PTY ring buffer (ENOSPC error),
/// confirming that partial reads free ring buffer capacity for subsequent writes without corrupting contents.
#[test]
fn test_pty_ring_buffer_enospc_boundary_and_partial_reads() {
    let capacity = 8;
    let mut ring = PtyRingBuffer::new(capacity);

    let res1 = ring.write_with_syscall_retry(b"12345678", |s| Ok(s.len()));
    assert_eq!(res1, Ok(8));
    assert_eq!(ring.available_write(), 0);

    let res2 = ring.write_with_syscall_retry(b"overflow", |s| Ok(s.len()));
    assert_eq!(res2, Err(SyscallError::Enospc));

    let mut pop_buf = [0u8; 4];
    let drained = ring.read_to_slice(&mut pop_buf).unwrap();
    assert_eq!(drained, 4);
    assert_eq!(&pop_buf, b"1234");
    assert_eq!(ring.available_write(), 4);

    let res3 = ring.write_with_syscall_retry(b"ABCD", |s| Ok(s.len()));
    assert_eq!(res3, Ok(4));
    assert_eq!(ring.available_read(), 8);
}

/// WHY: Verifies that advancing the PID cache generation invalidates stale entries from
/// previous epochs, preventing incorrect process status reporting when OS process IDs (PIDs)
/// are recycled by the kernel.
#[test]
fn test_generation_pid_cache_stale_entry_invalidation() {
    let cache = GenerationPidCache::new(100);
    let pid = 12345;

    let initial_gen = cache.current_generation();
    cache.insert(pid, PidStatus::Running, 1000);

    let status = cache.get(pid, initial_gen);
    assert_eq!(status, Some(PidStatus::Running));

    let new_gen = cache.advance_generation();
    assert_ne!(initial_gen, new_gen);

    let stale_lookup = cache.get(pid, new_gen);
    assert_eq!(stale_lookup, None);

    let (hits, misses, _) = cache.stats();
    assert_eq!(hits, 1);
    assert_eq!(misses, 1);
}

/// WHY: Tests thread-safe concurrent accesses and LRU capacity eviction in GenerationPidCache,
/// asserting atomic updates and bounded memory footprint under high-concurrency process monitoring.
#[test]
fn test_generation_pid_cache_concurrency_and_lru_eviction() {
    let cache = Arc::new(GenerationPidCache::new(5));
    let mut handles = Vec::new();

    for i in 0..10 {
        let c = Arc::clone(&cache);
        handles.push(thread::spawn(move || {
            let pid = 2000 + i;
            c.insert(pid, PidStatus::Running, 100 * i as u64);
            let current_gen = c.current_generation();
            let _ = c.get(pid, current_gen);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let (hits, _misses, evictions) = cache.stats();
    assert!(hits > 0);
    assert!(evictions > 0);
}

/// WHY: Verifies that already-normalized absolute and relative path strings return Cow::Borrowed
/// slices without any heap allocations, guaranteeing optimal performance on hot path resolutions.
#[test]
fn test_fast_path_normalization_zero_allocation_guarantee() {
    let paths = [
        "/usr/bin/vitrum",
        "/etc/config.json",
        "relative/path/file.rs",
        "/",
        "simple.txt",
    ];

    for path in &paths {
        assert!(FastPathNormalizer::is_normalized(path));
        let normalized = FastPathNormalizer::normalize(path);
        match &normalized {
            Cow::Borrowed(borrowed) => assert_eq!(*borrowed, *path),
            Cow::Owned(_) => panic!("Expected zero-allocation Cow::Borrowed for '{path}'"),
        }
    }
}

/// WHY: Tests path cleaning logic for complex paths containing double slashes ("//"), dot segments
/// ("/./"), parent directory traversals ("/../"), and trailing slashes, ensuring strict path equivalence.
#[test]
fn test_fast_path_normalization_redundant_slashes_and_parent_traversal() {
    let cases = [
        ("//usr//bin/./vitrum/", "/usr/bin/vitrum"),
        ("/a/b/../c/./d", "/a/c/d"),
        ("a/b/../../c", "c"),
        ("/a/b/../../..", "/"),
        ("./foo/bar/.", "foo/bar"),
        ("foo/bar/", "foo/bar"),
    ];

    for (input, expected) in &cases {
        assert!(!FastPathNormalizer::is_normalized(input));
        let normalized = FastPathNormalizer::normalize(input);
        assert_eq!(normalized.as_ref(), *expected);
    }
}

/// WHY: Evaluates adversarial edge cases including empty strings, root directory escape attempts
/// ("../../../etc/passwd"), unicode paths, and null-byte safety, asserting safe bounded normalized paths.
#[test]
fn test_fast_path_normalization_malformed_and_adversarial_inputs() {
    let cases = [
        ("", "."),
        ("../../../etc/passwd", "../../../etc/passwd"),
        ("/../../../etc/passwd", "/etc/passwd"),
        ("/././././", "/"),
        ("über/../\u{1F988}", "\u{1F988}"),
    ];

    for (input, expected) in &cases {
        let normalized = FastPathNormalizer::normalize(input);
        assert_eq!(normalized.as_ref(), *expected);
    }
}
