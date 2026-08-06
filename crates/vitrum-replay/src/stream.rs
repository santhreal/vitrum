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
/// Compression algorithms supported for log stream archiving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompressionAlgorithm {
    /// Fast run-length byte packing tailored for terminal VT escape sequences and repeated space fills.
    #[default]
    RleDeflate,
    /// Chunked frame compression simulating Zstd frame headers and checksum verification.
    ZstdChunked,
}

/// A individual compressed block in a stream archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedBlock {
    /// Starting sequence number of this block.
    pub seq_start: u64,
    /// Uncompressed length of data in bytes.
    pub uncompressed_len: u32,
    /// CRC32 / FNV checksum for data integrity verification.
    pub checksum: u32,
    /// Compressed payload bytes.
    pub payload: Vec<u8>,
}

/// High-throughput compressed stream archive for session log storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedStreamArchive {
    /// Initial base sequence.
    pub base_seq: u64,
    /// Final head sequence.
    pub head_seq: u64,
    /// Total uncompressed length in bytes.
    pub uncompressed_len: u64,
    /// Total compressed length in bytes.
    pub compressed_len: u64,
    /// Algorithm used to compress the blocks.
    pub algorithm: CompressionAlgorithm,
    /// Sequence-indexed compressed blocks.
    pub blocks: Vec<CompressedBlock>,
}

impl CompressedStreamArchive {
    /// Compression ratio achieved (uncompressed / compressed).
    #[must_use]
    pub fn compression_ratio(&self) -> f64 {
        if self.compressed_len == 0 {
            1.0
        } else {
            self.uncompressed_len as f64 / self.compressed_len as f64
        }
    }

    /// Verify integrity of all blocks via checksum validation.
    #[must_use]
    pub fn verify_checksums(&self) -> bool {
        for block in &self.blocks {
            let actual = compute_checksum(&block.payload);
            if actual != block.checksum {
                return false;
            }
        }
        true
    }

    /// Decompress the whole archive back into raw bytes and base_seq.
    pub fn decompress(&self) -> crate::error::Result<(u64, Vec<u8>)> {
        let mut result = Vec::with_capacity(self.uncompressed_len as usize);
        for block in &self.blocks {
            let decompressed = decompress_block(block, self.algorithm)?;
            result.extend_from_slice(&decompressed);
        }
        Ok((self.base_seq, result))
    }

    /// Random-access targeted decompression of a specific sequence range.
    pub fn decompress_range(&self, range: Range<u64>) -> crate::error::Result<Vec<u8>> {
        let mut result = Vec::new();
        for block in &self.blocks {
            let block_end = block.seq_start + block.uncompressed_len as u64;
            if block.seq_start < range.end && block_end > range.start {
                let decompressed = decompress_block(block, self.algorithm)?;
                let slice_start = range.start.saturating_sub(block.seq_start) as usize;
                let slice_end = (range.end.saturating_sub(block.seq_start) as usize).min(decompressed.len());
                if slice_start < decompressed.len() && slice_start < slice_end {
                    result.extend_from_slice(&decompressed[slice_start..slice_end]);
                }
            }
        }
        Ok(result)
    }
}

impl<'a> Stream<'a> {
    /// Compress the stream into a [`CompressedStreamArchive`] for high-throughput log archiving.
    pub fn compress_archive(
        &self,
        algorithm: CompressionAlgorithm,
        block_size: usize,
    ) -> CompressedStreamArchive {
        let block_size = block_size.max(256);
        let mut blocks = Vec::new();
        let total_uncompressed = self.len();
        let base_seq = self.base_seq();
        let head_seq = self.head_seq();

        let full_data = self.to_vec(base_seq..head_seq);
        let mut offset = 0usize;
        let mut total_compressed = 0u64;

        while offset < full_data.len() {
            let end = (offset + block_size).min(full_data.len());
            let chunk = &full_data[offset..end];
            let seq_start = base_seq + offset as u64;

            let payload = compress_block(chunk, algorithm);
            let checksum = compute_checksum(&payload);
            total_compressed += payload.len() as u64;

            blocks.push(CompressedBlock {
                seq_start,
                uncompressed_len: chunk.len() as u32,
                checksum,
                payload,
            });

            offset = end;
        }

        CompressedStreamArchive {
            base_seq,
            head_seq,
            uncompressed_len: total_uncompressed,
            compressed_len: total_compressed,
            algorithm,
            blocks,
        }
    }
}

/// Helper function to compute lightweight FNV-1a 32-bit checksum.
fn compute_checksum(data: &[u8]) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for &byte in data {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

/// Compress a block payload using specified algorithm.
fn compress_block(data: &[u8], algorithm: CompressionAlgorithm) -> Vec<u8> {
    match algorithm {
        CompressionAlgorithm::RleDeflate => {
            let mut out = Vec::with_capacity(data.len());
            out.push(b'R'); // Header marker
            let mut i = 0;
            while i < data.len() {
                let byte = data[i];
                let mut count = 1u8;
                while i + (count as usize) < data.len() && data[i + count as usize] == byte && count < 255 {
                    count += 1;
                }
                if count > 3 || byte == 0x00 || byte == b' ' {
                    out.push(0x00); // RLE escape tag
                    out.push(count);
                    out.push(byte);
                    i += count as usize;
                } else {
                    out.push(byte);
                    i += 1;
                }
            }
            out
        }
        CompressionAlgorithm::ZstdChunked => {
            let mut out = Vec::with_capacity(data.len() + 12);
            out.extend_from_slice(b"ZSTD");
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            // Fast LZ-window style compression simulation for Zstd chunk frame
            let mut i = 0;
            while i < data.len() {
                let byte = data[i];
                let mut run = 1u8;
                while i + (run as usize) < data.len() && data[i + run as usize] == byte && run < 255 {
                    run += 1;
                }
                if run > 2 || byte == 0xFF {
                    out.push(0xFF);
                    out.push(run);
                    out.push(byte);
                    i += run as usize;
                } else {
                    out.push(byte);
                    i += 1;
                }
            }
            out
        }
    }
}

/// Decompress a block payload using specified algorithm.
fn decompress_block(block: &CompressedBlock, algorithm: CompressionAlgorithm) -> crate::error::Result<Vec<u8>> {
    let actual_checksum = compute_checksum(&block.payload);
    if actual_checksum != block.checksum {
        return Err(crate::error::Error::StreamCompression(
            "Compressed block checksum verification failed".to_string(),
        ));
    }

    let mut out = Vec::with_capacity(block.uncompressed_len as usize);

    match algorithm {
        CompressionAlgorithm::RleDeflate => {
            if block.payload.first() != Some(&b'R') {
                return Err(crate::error::Error::StreamCompression(
                    "Invalid RLE block header".to_string(),
                ));
            }
            let mut i = 1;
            while i < block.payload.len() {
                if block.payload[i] == 0x00 && i + 2 < block.payload.len() {
                    let count = block.payload[i + 1] as usize;
                    let byte = block.payload[i + 2];
                    out.resize(out.len() + count, byte);
                    i += 3;
                } else {
                    out.push(block.payload[i]);
                    i += 1;
                }
            }
        }
        CompressionAlgorithm::ZstdChunked => {
            if block.payload.len() < 8 || &block.payload[0..4] != b"ZSTD" {
                return Err(crate::error::Error::StreamCompression(
                    "Invalid ZSTD block header".to_string(),
                ));
            }
            let mut i = 8;
            while i < block.payload.len() {
                if block.payload[i] == 0xFF && i + 2 < block.payload.len() {
                    let count = block.payload[i + 1] as usize;
                    let byte = block.payload[i + 2];
                    out.resize(out.len() + count, byte);
                    i += 3;
                } else {
                    out.push(block.payload[i]);
                    i += 1;
                }
            }
        }
    }

    Ok(out)
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
