//! Periodic snapshots, and why seeking is cheap.
//!
//! # The problem
//!
//! Reconstructing the screen at seq N means feeding every byte from the start of
//! the stream up to N, because a terminal is a state machine and byte 4 000 000
//! depends on byte 3. That is O(n) per seek. On a 10 MiB ring it is tens of
//! milliseconds of parsing per seek, and a user dragging a scrubber asks for a
//! seek every frame.
//!
//! # The index
//!
//! One linear pass over the stream, snapshotting the whole screen every `stride`
//! bytes. A seek then restores the newest snapshot at or before the target and
//! feeds at most `stride` bytes. Seek cost stops depending on the size of the ring
//! and starts depending on the stride, which the caller picks.
//!
//! # Why the seqs are not multiples of the stride
//!
//! A snapshot is only sound where the VT parser is in its ground state, so each
//! keyframe slides forward from its stride boundary to the next byte boundary that
//! satisfies that. See [`crate::emulator`] for how the boundary is found and why
//! nothing else would do. In ordinary output the slide is one or two bytes; in a
//! stream with no safe boundary within [`ReplayConfig::ground_scan`] bytes the
//! keyframe is skipped, which costs seek time in that region and nothing else.
//!
//! # What it costs
//!
//! [`KeyframeIndex::heap_bytes`] is exact rather than estimated, because "how much
//! does the scrubber cost" is a question with a real answer and vitrum's whole
//! idle-memory budget is built on knowing numbers like this one. A keyframe is one
//! [`Screen`], so the number is driven by the screen size, not the ring size:
//! 80x24 is about 31 KiB per keyframe, 200x50 about 160 KiB.

use crate::config::ReplayConfig;
use crate::emulator::Emulator;
use crate::error::Result;
use crate::screen::Screen;
use crate::stream::Stream;

/// One snapshot of the screen, and the seq it is the state *before*.
///
/// "Before" is the whole contract: the screen in a keyframe at seq `s` is what the
/// session showed after it had written exactly `s` bytes, so resuming means feeding
/// the stream from `s` onwards.
#[derive(Clone, Debug)]
pub struct Keyframe {
    /// Stream position this snapshot is the state before.
    pub seq: u64,
    screen: Screen,
}

impl Keyframe {
    /// Create a new keyframe with seq position and screen snapshot.
    #[must_use]
    pub const fn new(seq: u64, screen: Screen) -> Self {
        Self { seq, screen }
    }

    /// The snapshot.
    #[must_use]
    pub const fn screen(&self) -> &Screen {
        &self.screen
    }
}
/// Compact diff of VT terminal state between a baseline keyframe and a target keyframe.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyframeDelta {
    /// Target keyframe sequence number.
    pub seq: u64,
    /// Base keyframe sequence number.
    pub base_seq: u64,
    /// Target cursor state.
    pub cursor: crate::screen::Cursor,
    /// Vector of (column, row, cell) diffs compared to baseline.
    pub cell_diffs: Vec<(u16, u16, vitrum_grid::Cell)>,
}

/// Storage strategy for keyframes in index: anchor keyframe or compact delta.
#[derive(Clone, Debug)]
pub enum KeyframeStorage {
    /// Full anchor keyframe holding complete [`Screen`] state.
    Anchor(Keyframe),
    /// Compact delta encoding relative to an anchor keyframe.
    Delta(KeyframeDelta),
}

impl KeyframeStorage {
    /// Sequence number for this keyframe entry.
    #[must_use]
    pub fn seq(&self) -> u64 {
        match self {
            Self::Anchor(k) => k.seq,
            Self::Delta(d) => d.seq,
        }
    }
}

impl Keyframe {
    /// Compute delta snapshot relative to a base keyframe.
    #[must_use]
    pub fn compute_delta(&self, base: &Keyframe) -> KeyframeDelta {
        let cols = self.screen.cols();
        let rows = self.screen.rows();
        let mut cell_diffs = Vec::new();

        for r in 0..rows {
            for c in 0..cols {
                let cell_self = self.screen.grid().cell(c, r);
                let cell_base = base.screen.grid().cell(c, r);
                if cell_self != cell_base {
                    if let Some(cell) = cell_self {
                        cell_diffs.push((c, r, cell.clone()));
                    }
                }
            }
        }

        KeyframeDelta {
            seq: self.seq,
            base_seq: base.seq,
            cursor: self.screen.cursor(),
            cell_diffs,
        }
    }

    /// Reconstruct full Screen from base Screen and delta.
    #[must_use]
    pub fn apply_delta(base_screen: &Screen, delta: &KeyframeDelta) -> Screen {
        let mut screen = base_screen.clone();
        *screen.cursor_mut() = delta.cursor;
        for &(c, r, ref cell) in &delta.cell_diffs {
            let _ = screen.grid_mut().set_cell(c, r, cell.clone());
        }
        screen
    }
}

/// Seekable snapshots over one stream.
#[derive(Clone, Debug)]
pub struct KeyframeIndex {
    frames: Vec<Keyframe>,
    stride: usize,
    /// Bytes that had no reachable ground boundary, so no keyframe was taken.
    skipped: usize,
}

impl KeyframeIndex {
    /// Build the index in one linear pass over `stream`.
    ///
    /// # Errors
    ///
    /// Whatever [`ReplayConfig::validate`] rejects.
    pub fn build(stream: &Stream<'_>, config: &ReplayConfig) -> Result<Self> {
        config.validate()?;
        let head = stream.head_seq();
        let mut emulator = Emulator::new(config.cols, config.rows, config.palette)?;
        let mut frames = Vec::new();
        let mut skipped = 0usize;
        let mut at = stream.base_seq();

        while at < head {
            let boundary = (at + config.keyframe_stride as u64).min(head);
            for slice in stream.slices(at..boundary) {
                emulator.feed(slice);
            }
            at = boundary;
            if at >= head {
                break;
            }
            match ground_boundary(&mut emulator, stream, at, config.ground_scan) {
                Some(consumed) => {
                    at += consumed;
                    frames.push(Keyframe {
                        seq: at,
                        screen: emulator.screen().clone(),
                    });
                }
                None => {
                    // The scan fed bytes without finding a boundary. Those bytes
                    // are consumed and correct; only the snapshot is missing.
                    at += scanned(stream, at, config.ground_scan);
                    skipped += 1;
                }
            }
        }

        Ok(Self {
            frames,
            stride: config.keyframe_stride,
            skipped,
        })
    }

    /// The newest keyframe at or before `seq`, or `None` when the only sound start
    /// is the beginning of the stream.
    #[must_use]
    pub fn latest_at_or_before(&self, seq: u64) -> Option<&Keyframe> {
        let index = self.frames.partition_point(|frame| frame.seq <= seq);
        if index == 0 {
            None
        } else {
            self.frames.get(index - 1)
        }
    }

    /// Every keyframe, in stream order.
    #[must_use]
    pub fn frames(&self) -> &[Keyframe] {
        &self.frames
    }

    /// How many keyframes there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Is the index empty? True for a stream shorter than one stride.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The stride this index was built with.
    #[must_use]
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Stride boundaries that had no resumable byte boundary within
    /// [`ReplayConfig::ground_scan`], so no keyframe was taken.
    ///
    /// Non-zero means seeks into those regions cost more, and is worth surfacing
    /// rather than hiding: it says the session emitted a very long stretch with no
    /// completed character or sequence, which is what catting a binary file looks
    /// like.
    #[must_use]
    pub const fn skipped_boundaries(&self) -> usize {
        self.skipped
    }

    /// Bytes this index holds on the heap.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        let frames: usize = self.frames.iter().map(|frame| frame.screen.heap_bytes()).sum();
        frames + self.frames.capacity() * core::mem::size_of::<Keyframe>()
    }
    /// Build delta-encoded keyframes storage table with periodic anchor intervals.
    pub fn build_delta_encoded_storage(&self, anchor_interval: usize) -> Vec<KeyframeStorage> {
        let anchor_interval = anchor_interval.max(1);
        let mut storage = Vec::with_capacity(self.frames.len());
        let mut last_anchor: Option<Keyframe> = None;

        for (i, frame) in self.frames.iter().enumerate() {
            if i % anchor_interval == 0 || last_anchor.is_none() {
                storage.push(KeyframeStorage::Anchor(frame.clone()));
                last_anchor = Some(frame.clone());
            } else {
                let anchor = last_anchor.as_ref().unwrap();
                let delta = frame.compute_delta(anchor);
                storage.push(KeyframeStorage::Delta(delta));
            }
        }
        storage
    }

    /// Calculate heap bytes required when keyframe index is delta-encoded with anchor_interval.
    #[must_use]
    pub fn delta_encoded_heap_bytes(&self, anchor_interval: usize) -> usize {
        let storage = self.build_delta_encoded_storage(anchor_interval);
        let mut bytes = storage.capacity() * core::mem::size_of::<KeyframeStorage>();
        for entry in storage {
            match entry {
                KeyframeStorage::Anchor(k) => bytes += k.screen.heap_bytes(),
                KeyframeStorage::Delta(d) => {
                    bytes += d.cell_diffs.capacity() * core::mem::size_of::<(u16, u16, vitrum_grid::Cell)>();
                }
            }
        }
        bytes
    }
}

/// Feed one byte at a time from `at` until the parser is resumable, returning how
/// many bytes that took.
///
/// `None` means `limit` bytes went by without a boundary; the emulator has still
/// consumed them, and the caller must account for that with [`scanned`].
fn ground_boundary(
    emulator: &mut Emulator,
    stream: &Stream<'_>,
    at: u64,
    limit: usize,
) -> Option<u64> {
    let mut consumed = 0u64;
    for slice in stream.slices(at..at + limit as u64) {
        for &byte in slice {
            consumed += 1;
            if emulator.feed_byte(byte) {
                return Some(consumed);
            }
        }
    }
    None
}

/// How many bytes [`ground_boundary`] fed before giving up.
fn scanned(stream: &Stream<'_>, at: u64, limit: usize) -> u64 {
    stream
        .slices(at..at + limit as u64)
        .map(|slice| slice.len() as u64)
        .sum()
}
