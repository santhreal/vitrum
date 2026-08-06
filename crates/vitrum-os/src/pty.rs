//! High-performance Zero-Copy PTY Reader with Preallocated Ring Buffer.
//!
//! Provides zero-allocation, high-throughput reading from PTY file descriptors
//! using preallocated circular ring buffers and direct syscall slice targets.

use std::io;

/// Operational metrics for PTY ring buffer syscall operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PtyReadStats {
    /// Total bytes read into the ring buffer.
    pub bytes_read: u64,
    /// Total read syscalls executed.
    pub syscall_count: u64,
    /// Number of zero-copy direct slice fills.
    pub zero_copy_reads: u64,
    /// Number of ring buffer wrap-around events handled.
    pub buffer_wraps: u64,
}

/// Preallocated circular ring buffer for PTY byte stream reading.
#[derive(Debug)]
pub struct PtyRingBuffer {
    buffer: Vec<u8>,
    head: usize, // Write offset
    tail: usize, // Read offset
    len: usize,  // Total readable bytes present
}

impl PtyRingBuffer {
    /// Allocate a ring buffer with specified capacity (minimum 64 bytes).
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.max(64);
        Self {
            buffer: vec![0u8; cap],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    /// Default 64 KiB preallocated ring buffer.
    pub fn new() -> Self {
        Self::with_capacity(65_536)
    }

    /// Total allocated capacity of the ring buffer.
    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Number of readable bytes currently stored.
    pub fn readable_len(&self) -> usize {
        self.len
    }

    /// Number of writable bytes available without overwriting unread data.
    pub fn writable_len(&self) -> usize {
        self.buffer.len() - self.len
    }

    /// Returns true if there are no readable bytes.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns true if the ring buffer is completely full.
    pub fn is_full(&self) -> bool {
        self.len == self.buffer.len()
    }

    /// Get up to two contiguous mutable slices where incoming PTY data can be written directly via syscalls.
    pub fn prepare_write_slices(&mut self) -> (&mut [u8], &mut [u8]) {
        let cap = self.buffer.len();
        let avail = cap - self.len;
        if avail == 0 {
            return (&mut [], &mut []);
        }

        let head = self.head;
        if head + avail <= cap {
            (&mut self.buffer[head..head + avail], &mut [])
        } else {
            let first_len = cap - head;
            let second_len = avail - first_len;
            let (left, right) = self.buffer.split_at_mut(head);
            (&mut right[..first_len], &mut left[..second_len])
        }
    }

    /// Advance write head after reading bytes into the prepared mutable slices.
    pub fn advance_write(&mut self, bytes_written: usize) {
        let avail = self.writable_len();
        let actual = bytes_written.min(avail);
        if actual == 0 {
            return;
        }

        let cap = self.buffer.len();
        self.head = (self.head + actual) % cap;
        self.len += actual;
    }

    /// Get up to two contiguous slices of unread data.
    pub fn prepare_read_slices(&self) -> (&[u8], &[u8]) {
        if self.len == 0 {
            return (&[], &[]);
        }

        let cap = self.buffer.len();
        let tail = self.tail;
        if tail + self.len <= cap {
            (&self.buffer[tail..tail + self.len], &[])
        } else {
            let first_len = cap - tail;
            let second_len = self.len - first_len;
            (&self.buffer[tail..cap], &self.buffer[..second_len])
        }
    }

    /// Advance read tail after consuming bytes from the ring buffer.
    pub fn advance_read(&mut self, bytes_read: usize) {
        let actual = bytes_read.min(self.len);
        if actual == 0 {
            return;
        }

        let cap = self.buffer.len();
        self.tail = (self.tail + actual) % cap;
        self.len -= actual;
    }

    /// Clear all data in the ring buffer.
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }
}

impl Default for PtyRingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// Zero-Copy Direct Syscall PTY Reader engine.
#[derive(Debug)]
pub struct PtyReader {
    ring: PtyRingBuffer,
    stats: PtyReadStats,
}

impl PtyReader {
    /// Create a new reader with default 64 KiB buffer.
    pub fn new() -> Self {
        Self {
            ring: PtyRingBuffer::new(),
            stats: PtyReadStats::default(),
        }
    }

    /// Create a reader with explicit ring buffer capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            ring: PtyRingBuffer::with_capacity(capacity),
            stats: PtyReadStats::default(),
        }
    }

    /// Get reference to internal ring buffer.
    pub fn ring(&self) -> &PtyRingBuffer {
        &self.ring
    }

    /// Get mutable reference to internal ring buffer.
    pub fn ring_mut(&mut self) -> &mut PtyRingBuffer {
        &mut self.ring
    }

    /// Get current read statistics.
    pub fn stats(&self) -> PtyReadStats {
        self.stats
    }

    /// Perform a zero-copy read from a closure or syscall provider that writes into target buffer slices.
    pub fn read_direct<F>(&mut self, mut read_sys: F) -> io::Result<usize>
    where
        F: FnMut(&mut [u8]) -> io::Result<usize>,
    {
        let (first, _) = self.ring.prepare_write_slices();
        if first.is_empty() {
            return Ok(0);
        }

        self.stats.syscall_count += 1;
        match read_sys(first) {
            Ok(n) => {
                if n > 0 {
                    self.ring.advance_write(n);
                    self.stats.bytes_read += n as u64;
                    self.stats.zero_copy_reads += 1;
                }
                Ok(n)
            }
            Err(e) => Err(e),
        }
    }

    /// Fill ring buffer from a raw byte slice (e.g. for testing or memory-mapped PTY streams).
    pub fn fill_from_slice(&mut self, src: &[u8]) -> usize {
        let (first, second) = self.ring.prepare_write_slices();
        let mut total = 0;

        if !first.is_empty() && total < src.len() {
            let to_copy = first.len().min(src.len());
            first[..to_copy].copy_from_slice(&src[..to_copy]);
            total += to_copy;
        }

        if !second.is_empty() && total < src.len() {
            let rem = &src[total..];
            let to_copy = second.len().min(rem.len());
            second[..to_copy].copy_from_slice(&rem[..to_copy]);
            total += to_copy;
            self.stats.buffer_wraps += 1;
        }

        self.ring.advance_write(total);
        self.stats.bytes_read += total as u64;
        total
    }

    /// Inspect contiguous unread bytes available in the ring buffer.
    pub fn peek_slices(&self) -> (&[u8], &[u8]) {
        self.ring.prepare_read_slices()
    }

    /// Consume up to `count` bytes from the reader.
    pub fn consume(&mut self, count: usize) {
        self.ring.advance_read(count);
    }
}

impl Default for PtyReader {
    fn default() -> Self {
        Self::new()
    }
}
