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
    /// The snapshot.
    #[must_use]
    pub const fn screen(&self) -> &Screen {
        &self.screen
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
