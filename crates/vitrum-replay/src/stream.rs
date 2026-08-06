//! The byte stream and its seq coordinate space.
//!
//! A [`Stream`] is a session's retained output plus the seq its first byte
//! carries. That is the entire input to replay: no session id, no socket, no
//! recording.
//!
//! # Why it is a list of chunks
//!
//! Reading a ring gives two contiguous runs, the older tail and the newer head,
//! and the join lands wherever the write cursor happens to be. Stitching them
//! costs a copy of the whole ring, so nothing is stitched. This is the same
//! shape [`vitrum_search::Haystack`] takes for the same reason, and
//! [`Stream::from_haystack`] converts without copying so a caller that already
//! built a haystack for search does not build a second thing for replay.
//!
//! # Coordinates
//!
//! `seq` is a byte offset into the session's whole output stream. `offset` is a
//! byte offset into this stream's chunks. They differ by [`Stream::base_seq`],
//! which is how many bytes the ring has already evicted. Every public API on
//! this crate speaks seq, because seq is what the daemon's data plane numbers by
//! and what survives eviction; offset never leaves this module.

use core::ops::Range;

use vitrum_search::Haystack;

/// A session's retained output, in stream order, with the seq of its first byte.
///
/// `Copy`, because it borrows the chunks rather than owning them.
///
/// ```
/// use vitrum_replay::Stream;
///
/// let tail: &[u8] = b"hello ";
/// let head: &[u8] = b"world";
/// let chunks = [tail, head];
/// let stream = Stream::new(1_000, &chunks);
///
/// assert_eq!(stream.base_seq(), 1_000);
/// assert_eq!(stream.head_seq(), 1_011);
/// // A range that crosses the ring's join reads as one run of bytes.
/// assert_eq!(stream.to_vec(1_004..1_008), b"o wo");
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Stream<'a> {
    base_seq: u64,
    chunks: &'a [&'a [u8]],
}

impl<'a> Stream<'a> {
    /// A stream whose first byte is at `base_seq`.
    ///
    /// `chunks` are in stream order, oldest first. Empty chunks are legal: a
    /// ring whose head half has not wrapped yet hands over a zero-length slice.
    #[must_use]
    pub const fn new(base_seq: u64, chunks: &'a [&'a [u8]]) -> Self {
        Self { base_seq, chunks }
    }

    /// The same bytes a [`vitrum_search::Haystack`] holds, without copying.
    #[must_use]
    pub const fn from_haystack(haystack: &Haystack<'a>) -> Self {
        Self {
            base_seq: haystack.base_seq,
            chunks: haystack.chunks,
        }
    }

    /// Seq of the first retained byte.
    #[must_use]
    pub const fn base_seq(&self) -> u64 {
        self.base_seq
    }

    /// Total retained bytes.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.chunks.iter().map(|chunk| chunk.len() as u64).sum()
    }

    /// Is there anything to replay?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.iter().all(|chunk| chunk.is_empty())
    }

    /// One past the seq of the last retained byte.
    ///
    /// This is a legal seek target and means "the end of everything written so
    /// far", which is where a live client's view sits.
    #[must_use]
    pub fn head_seq(&self) -> u64 {
        self.base_seq + self.len()
    }

    /// Can `seq` be sought to?
    ///
    /// True for `base_seq..=head_seq` inclusive of both ends.
    #[must_use]
    pub fn holds(&self, seq: u64) -> bool {
        seq >= self.base_seq && seq <= self.head_seq()
    }

    /// The chunks, in stream order.
    #[must_use]
    pub const fn chunks(&self) -> &'a [&'a [u8]] {
        self.chunks
    }

    /// Borrowed runs covering `seqs`, in order, skipping empty ones.
    ///
    /// The range is clamped to what the stream holds, so an out-of-range request
    /// yields nothing rather than panicking. Nothing is copied: a range that
    /// crosses the ring's join yields two slices.
    #[must_use]
    pub fn slices(&self, seqs: Range<u64>) -> Slices<'a> {
        let head = self.head_seq();
        let start = seqs.start.clamp(self.base_seq, head) - self.base_seq;
        let end = seqs.end.clamp(self.base_seq, head) - self.base_seq;
        Slices {
            chunks: self.chunks,
            chunk: 0,
            // Offsets consumed so far while walking to `start`.
            walked: 0,
            remaining: end.saturating_sub(start),
            skip: start,
        }
    }

    /// A copy of `seqs` as one contiguous buffer.
    ///
    /// Allocates. This is for export, tests, and anything that genuinely needs
    /// one slice; the replay path uses [`Stream::slices`] and copies nothing.
    #[must_use]
    pub fn to_vec(&self, seqs: Range<u64>) -> Vec<u8> {
        let mut out = Vec::new();
        for slice in self.slices(seqs) {
            out.extend_from_slice(slice);
        }
        out
    }

    /// The byte at `seq`, or `None` past the end.
    #[must_use]
    pub fn byte_at(&self, seq: u64) -> Option<u8> {
        self.slices(seq..seq.saturating_add(1))
            .next()
            .and_then(|slice| slice.first().copied())
    }
}

/// Borrowed runs of a seq range, yielded in stream order.
///
/// See [`Stream::slices`].
#[derive(Clone, Debug)]
pub struct Slices<'a> {
    chunks: &'a [&'a [u8]],
    chunk: usize,
    walked: u64,
    remaining: u64,
    skip: u64,
}

impl<'a> Iterator for Slices<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        while self.remaining > 0 && self.chunk < self.chunks.len() {
            let bytes = self.chunks[self.chunk];
            let len = bytes.len() as u64;
            let chunk_end = self.walked + len;
            if self.skip >= chunk_end {
                self.walked = chunk_end;
                self.chunk += 1;
                continue;
            }
            let from = (self.skip - self.walked) as usize;
            let take = ((chunk_end - self.skip).min(self.remaining)) as usize;
            self.skip += take as u64;
            self.remaining -= take as u64;
            if self.skip == chunk_end {
                self.walked = chunk_end;
                self.chunk += 1;
            }
            if take > 0 {
                return Some(&bytes[from..from + take]);
            }
        }
        None
    }
}
