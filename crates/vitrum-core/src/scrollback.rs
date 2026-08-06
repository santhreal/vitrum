//! Bounded byte ring holding one session's most recent output.

use std::fmt;

/// Smallest allocation step while the ring is still growing.
const GROWTH_FLOOR: usize = 4 * 1024;

/// Bounded ring of recent output bytes for one session.
///
/// Sequence numbers are cumulative byte offsets into the session's entire
/// output stream. They never restart and never renumber when bytes are evicted,
/// which is what lets a reconnecting client name the exact range it missed
/// instead of guessing. They are `u64` throughout because a long-lived agent
/// writes far more than `u32::MAX` bytes.
///
/// Storage grows geometrically up to the configured capacity and then never
/// reallocates. A session that emits 200 bytes does not pay for a 10 MB ring,
/// and a session that emits gigabytes never grows past one.
pub struct Scrollback {
    /// Ring storage. `buf.len()` is how much has ever been needed, capped at
    /// `cap`; once it reaches `cap` the buffer is overwritten in place.
    buf: Vec<u8>,
    cap: usize,
    /// Index of the oldest retained byte. Stays 0 until the ring fills, because
    /// eviction is the only thing that advances it and eviction cannot happen
    /// before `buf.len() == cap`.
    start: usize,
    /// Retained byte count. Equals `buf.len()` while growing, then `cap`.
    len: usize,
    /// Total bytes ever pushed, retained or evicted. This is the seq the next
    /// written byte will get.
    head_seq: u64,
}

impl Scrollback {
    /// Create a ring retaining at most `bytes` of the most recent output.
    ///
    /// A capacity of 0 retains nothing but still counts sequence numbers, so
    /// `oldest_seq() == head_seq()` always holds for it.
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap: bytes,
            start: 0,
            len: 0,
            head_seq: 0,
        }
    }

    /// Append `data`, evicting the oldest bytes once capacity is exceeded.
    pub fn push(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        // Sequence numbers count every byte the session ever produced, so this
        // advances by the full length even when most of `data` is dropped.
        self.head_seq += data.len() as u64;
        if self.cap == 0 {
            return;
        }
        // A single push larger than the whole ring can only leave its tail.
        let data = &data[data.len().saturating_sub(self.cap)..];

        if self.buf.len() < self.cap {
            debug_assert_eq!(self.start, 0, "growth phase must stay contiguous");
            debug_assert_eq!(self.len, self.buf.len());
            let room = self.cap - self.len;
            let head = data.len().min(room);
            self.reserve(head);
            self.buf.extend_from_slice(&data[..head]);
            self.len += head;
            if head == data.len() {
                return;
            }
            // The buffer just reached capacity; the remainder wraps.
            self.write_ring(&data[head..]);
        } else {
            self.write_ring(data);
        }
    }

    /// Seq of the oldest byte still retained.
    pub fn oldest_seq(&self) -> u64 {
        self.head_seq - self.len as u64
    }

    /// Seq the next written byte will get, i.e. one past the newest byte.
    pub fn head_seq(&self) -> u64 {
        self.head_seq
    }

    /// Retained byte count, never more than the configured capacity.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when nothing is retained.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The retained bytes as at most two contiguous runs, oldest first.
    ///
    /// Deliberately not stitched. Cross-session search sweeps every ring at
    /// once, and joining the halves would copy up to 200 MB per query to spare
    /// the caller one seam; the caller walks lines across the seam instead and
    /// copies only the single line that straddles it.
    ///
    /// Either run may be empty: a ring that has never wrapped is entirely in
    /// the first, and a ring with no capacity is empty in both. `start` is
    /// provably 0 until the buffer reaches capacity, which is what lets one
    /// expression cover the growing, full and zero-capacity states.
    pub fn halves(&self) -> (&[u8], &[u8]) {
        if self.len == 0 {
            return (&[], &[]);
        }
        let first = (self.buf.len() - self.start).min(self.len);
        (
            &self.buf[self.start..self.start + first],
            &self.buf[..self.len - first],
        )
    }

    /// Bytes in `[from_seq, from_seq + max)` clamped to what is retained.
    ///
    /// Returns `None` when `from_seq` has already been evicted, or when it
    /// names a byte the session has not produced yet, so a caller can tell
    /// "your history is gone, resync" apart from "you are caught up" (which is
    /// `from_seq == head_seq` and yields an empty vector).
    pub fn range(&self, from_seq: u64, max: usize) -> Option<Vec<u8>> {
        if from_seq < self.oldest_seq() || from_seq > self.head_seq {
            return None;
        }
        let off = (from_seq - self.oldest_seq()) as usize;
        let take = (self.len - off).min(max);
        let mut out = Vec::with_capacity(take);
        if take > 0 {
            let begin = (self.start + off) % self.buf.len();
            let first = (self.buf.len() - begin).min(take);
            out.extend_from_slice(&self.buf[begin..begin + first]);
            if first < take {
                out.extend_from_slice(&self.buf[..take - first]);
            }
        }
        Some(out)
    }

    /// Grow storage toward `cap` without ever allocating past it.
    ///
    /// `Vec::reserve` would overshoot: reserving 10 MB from an 8 MB allocation
    /// doubles to 16 MB, so a 10 MB ring would cost 16 MB of resident memory.
    fn reserve(&mut self, extra: usize) {
        let needed = self.buf.len() + extra;
        debug_assert!(needed <= self.cap);
        if self.buf.capacity() >= needed {
            return;
        }
        let target = self
            .buf
            .capacity()
            .max(GROWTH_FLOOR)
            .saturating_mul(2)
            .max(needed)
            .min(self.cap);
        self.buf.reserve_exact(target - self.buf.len());
    }

    /// Overwrite the oldest bytes with `data`, wrapping at the end.
    ///
    /// Only valid once the buffer is full, which is also the only state in
    /// which `len == cap`, so the write always starts exactly at `start`.
    fn write_ring(&mut self, data: &[u8]) {
        debug_assert_eq!(self.buf.len(), self.cap);
        debug_assert_eq!(self.len, self.cap);
        debug_assert!(data.len() <= self.cap);
        if data.is_empty() {
            return;
        }
        let at = self.start;
        let first = (self.cap - at).min(data.len());
        self.buf[at..at + first].copy_from_slice(&data[..first]);
        if first < data.len() {
            self.buf[..data.len() - first].copy_from_slice(&data[first..]);
        }
        self.start = (at + data.len()) % self.cap;
    }
}

impl fmt::Debug for Scrollback {
    /// Never dump the retained bytes: they are megabytes of terminal escape
    /// sequences and dumping them in a log turns a diagnostic into an incident.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scrollback")
            .field("cap", &self.cap)
            .field("len", &self.len)
            .field("oldest_seq", &self.oldest_seq())
            .field("head_seq", &self.head_seq)
            .finish()
    }
}
