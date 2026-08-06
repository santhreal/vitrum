//! Zero-Copy Chunked Binary Replay Format (.vbr) with Keyframe Index Table.
//!
//! `.vbr` is a compact, zero-copy binary format designed for high-performance session
//! recording, storage, streaming, and fast seek operations.
//!
//! # Format Specification
//!
//! - **Magic Header**: `b"VBR1"` (4 bytes)
//! - **Version**: `u16` (currently 1)
//! - **Header Fields**:
//!   - `header_len`: `u32` (size of header)
//!   - `cols`: `u16`
//!   - `rows`: `u16`
//!   - `base_seq`: `u64`
//!   - `head_seq`: `u64`
//!   - `chunk_count`: `u32`
//!   - `keyframe_count`: `u32`
//!   - `index_offset`: `u64` (byte offset to keyframe index table)
//! - **Chunk Payload Section**:
//!   Sequentially packed chunks:
//!   - `seq`: `u64`
//!   - `len`: `u32`
//!   - `micros`: `u64`
//!   - `data`: `[u8; len]` (raw chunk bytes)
//! - **Keyframe Index Section**:
//!   Array of fixed-size keyframe entries:
//!   - `seq`: `u64`
//!   - `stream_offset`: `u64`
//!   - `micros`: `u64`

use crate::error::{Error, Result};
use crate::keyframe::KeyframeIndex;
use crate::timeline::{ChunkStamp, Timeline};

/// Magic header bytes identifying a .vbr file.
pub const VBR_MAGIC: &[u8; 4] = b"VBR1";
/// Format version.
pub const VBR_VERSION: u16 = 1;
/// Fixed header size in bytes.
pub const HEADER_SIZE: usize = 4 + 2 + 4 + 2 + 2 + 8 + 8 + 4 + 4 + 8; // 46 bytes

/// Binary VBR Header metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VbrHeader {
    /// Format version.
    pub version: u16,
    /// Length of the header.
    pub header_len: u32,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Initial base sequence.
    pub base_seq: u64,
    /// Target head sequence.
    pub head_seq: u64,
    /// Total chunks in payload.
    pub chunk_count: u32,
    /// Total entries in keyframe index table.
    pub keyframe_count: u32,
    /// Byte offset where the index table begins.
    pub index_offset: u64,
}

impl VbrHeader {
    /// Encode header to byte array.
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(VBR_MAGIC);
        buf[4..6].copy_from_slice(&self.version.to_le_bytes());
        buf[6..10].copy_from_slice(&self.header_len.to_le_bytes());
        buf[10..12].copy_from_slice(&self.cols.to_le_bytes());
        buf[12..14].copy_from_slice(&self.rows.to_le_bytes());
        buf[14..22].copy_from_slice(&self.base_seq.to_le_bytes());
        buf[22..30].copy_from_slice(&self.head_seq.to_le_bytes());
        buf[30..34].copy_from_slice(&self.chunk_count.to_le_bytes());
        buf[34..38].copy_from_slice(&self.keyframe_count.to_le_bytes());
        buf[38..46].copy_from_slice(&self.index_offset.to_le_bytes());
        buf
    }

    /// Decode header from byte slice.
    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < HEADER_SIZE {
            return Err(Error::BinaryFormat(
                "VBR file header too short".to_string(),
            ));
        }

        if &data[0..4] != VBR_MAGIC {
            return Err(Error::BinaryFormat(
                "Invalid VBR magic bytes".to_string(),
            ));
        }

        let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
        if version != VBR_VERSION {
            return Err(Error::BinaryFormat(format!(
                "Unsupported VBR version: {version}"
            )));
        }

        let header_len = u32::from_le_bytes(data[6..10].try_into().unwrap());
        let cols = u16::from_le_bytes(data[10..12].try_into().unwrap());
        let rows = u16::from_le_bytes(data[12..14].try_into().unwrap());
        let base_seq = u64::from_le_bytes(data[14..22].try_into().unwrap());
        let head_seq = u64::from_le_bytes(data[22..30].try_into().unwrap());
        let chunk_count = u32::from_le_bytes(data[30..34].try_into().unwrap());
        let keyframe_count = u32::from_le_bytes(data[34..38].try_into().unwrap());
        let index_offset = u64::from_le_bytes(data[38..46].try_into().unwrap());

        Ok(Self {
            version,
            header_len,
            cols,
            rows,
            base_seq,
            head_seq,
            chunk_count,
            keyframe_count,
            index_offset,
        })
    }
}

/// Zero-copy representation of a chunk in a .vbr file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VbrChunk<'a> {
    /// Starting sequence number of chunk.
    pub seq: u64,
    /// Timestamp relative to recording start (in microseconds).
    pub micros: u64,
    /// Borrowed raw chunk payload.
    pub data: &'a [u8],
}

/// Keyframe Index Table entry encoded in .vbr file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VbrIndexEntry {
    /// Sequence number of keyframe boundary.
    pub seq: u64,
    /// Stream offset in raw bytes.
    pub stream_offset: u64,
    /// Microsecond timestamp.
    pub micros: u64,
}

impl VbrIndexEntry {
    /// Size of an index entry in bytes.
    pub const SIZE: usize = 24;

    /// Encode entry to byte slice.
    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..8].copy_from_slice(&self.seq.to_le_bytes());
        buf[8..16].copy_from_slice(&self.stream_offset.to_le_bytes());
        buf[16..24].copy_from_slice(&self.micros.to_le_bytes());
        buf
    }

    /// Decode entry from byte slice.
    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < Self::SIZE {
            return Err(Error::BinaryFormat(
                "Keyframe index entry truncated".to_string(),
            ));
        }

        let seq = u64::from_le_bytes(data[0..8].try_into().unwrap());
        let stream_offset = u64::from_le_bytes(data[8..16].try_into().unwrap());
        let micros = u64::from_le_bytes(data[16..24].try_into().unwrap());

        Ok(Self {
            seq,
            stream_offset,
            micros,
        })
    }
}

/// Zero-copy binary replay viewer over a borrowed `.vbr` byte slice.
#[derive(Debug, Clone, Copy)]
pub struct VbrView<'a> {
    data: &'a [u8],
    header: VbrHeader,
}

impl<'a> VbrView<'a> {
    /// Parse and validate a borrowed binary `.vbr` slice without allocation.
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let header = VbrHeader::decode(data)?;
        if (header.index_offset as usize) > data.len() {
            return Err(Error::BinaryFormat(
                "Invalid VBR index_offset out of bounds".to_string(),
            ));
        }
        Ok(Self { data, header })
    }

    /// Return the VBR header.
    pub fn header(&self) -> &VbrHeader {
        &self.header
    }

    /// Return an iterator over zero-copy chunks.
    pub fn chunks(&self) -> VbrChunkIterator<'a> {
        let payload = &self.data[HEADER_SIZE..self.header.index_offset as usize];
        VbrChunkIterator {
            remaining: payload,
            expected: self.header.chunk_count,
            yielded: 0,
        }
    }

    /// Read zero-copy keyframe index table entries.
    pub fn keyframe_index(&self) -> Vec<VbrIndexEntry> {
        let start = self.header.index_offset as usize;
        let mut entries = Vec::with_capacity(self.header.keyframe_count as usize);
        let mut offset = start;

        for _ in 0..self.header.keyframe_count {
            if offset + VbrIndexEntry::SIZE > self.data.len() {
                break;
            }
            if let Ok(entry) = VbrIndexEntry::decode(&self.data[offset..]) {
                entries.push(entry);
                offset += VbrIndexEntry::SIZE;
            }
        }
        entries
    }

    /// Fast zero-copy lookup for the latest keyframe entry at or before target_seq.
    pub fn binary_search_keyframe(&self, target_seq: u64) -> Option<VbrIndexEntry> {
        let count = self.header.keyframe_count as usize;
        if count == 0 {
            return None;
        }
        let start = self.header.index_offset as usize;
        let end = start + count * VbrIndexEntry::SIZE;
        if end > self.data.len() {
            return None;
        }
        let keyframe_slice = &self.data[start..end];

        let mut low = 0;
        let mut high = count;
        let mut best = None;

        while low < high {
            let mid = low + (high - low) / 2;
            let offset = mid * VbrIndexEntry::SIZE;
            if let Ok(entry) = VbrIndexEntry::decode(&keyframe_slice[offset..]) {
                if entry.seq <= target_seq {
                    best = Some(entry);
                    low = mid + 1;
                } else {
                    high = mid;
                }
            } else {
                break;
            }
        }
        best
    }

    /// Reconstruct raw buffer payload from chunks for Replay stream construction.
    pub fn reconstruct_bytes(&self) -> Vec<u8> {
        let mut raw = Vec::new();
        for chunk in self.chunks() {
            raw.extend_from_slice(chunk.data);
        }
        raw
    }

    /// Reconstruct Timeline from VBR chunks metadata.
    pub fn reconstruct_timeline(&self) -> Timeline {
        let mut stamps = Vec::new();
        for chunk in self.chunks() {
            stamps.push(ChunkStamp {
                end_seq: chunk.seq + chunk.data.len() as u64,
                micros: chunk.micros,
            });
        }
        Timeline::recorded(stamps)
    }
}

/// Zero-copy iterator over VBR chunks in payload slice.
#[derive(Debug, Clone)]
pub struct VbrChunkIterator<'a> {
    remaining: &'a [u8],
    expected: u32,
    yielded: u32,
}

impl<'a> Iterator for VbrChunkIterator<'a> {
    type Item = VbrChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.yielded >= self.expected || self.remaining.len() < 8 + 4 + 8 {
            return None;
        }

        let seq = u64::from_le_bytes(self.remaining[0..8].try_into().unwrap());
        let len = u32::from_le_bytes(self.remaining[8..12].try_into().unwrap()) as usize;
        let micros = u64::from_le_bytes(self.remaining[12..20].try_into().unwrap());

        if self.remaining.len() < 20 + len {
            return None;
        }

        let chunk_data = &self.remaining[20..20 + len];
        self.remaining = &self.remaining[20 + len..];
        self.yielded += 1;

        Some(VbrChunk {
            seq,
            micros,
            data: chunk_data,
        })
    }
}

/// Binary VBR format builder and serializer.
#[derive(Debug, Default)]
pub struct VbrWriter {
    cols: u16,
    rows: u16,
    base_seq: u64,
    head_seq: u64,
    chunk_count: u32,
    payload: Vec<u8>,
    keyframes: Vec<VbrIndexEntry>,
}

impl VbrWriter {
    /// Create a new VBR writer with dimensions.
    #[inline]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            base_seq: 0,
            head_seq: 0,
            chunk_count: 0,
            payload: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// Add a chunk payload to the VBR file without allocating per chunk.
    #[inline]
    pub fn add_chunk(&mut self, seq: u64, micros: u64, data: &[u8]) {
        if self.chunk_count == 0 {
            self.base_seq = seq;
        }
        self.head_seq = seq + data.len() as u64;
        self.payload.reserve(20 + data.len());
        self.payload.extend_from_slice(&seq.to_le_bytes());
        self.payload.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.payload.extend_from_slice(&micros.to_le_bytes());
        self.payload.extend_from_slice(data);
        self.chunk_count += 1;
    }

    /// Add a keyframe index entry.
    #[inline]
    pub fn add_keyframe(&mut self, seq: u64, stream_offset: u64, micros: u64) {
        self.keyframes.push(VbrIndexEntry {
            seq,
            stream_offset,
            micros,
        });
    }

    /// Serialize into binary `.vbr` format.
    pub fn serialize(&self) -> Vec<u8> {
        let index_offset = (HEADER_SIZE + self.payload.len()) as u64;

        let header = VbrHeader {
            version: VBR_VERSION,
            header_len: HEADER_SIZE as u32,
            cols: self.cols,
            rows: self.rows,
            base_seq: self.base_seq,
            head_seq: self.head_seq,
            chunk_count: self.chunk_count,
            keyframe_count: self.keyframes.len() as u32,
            index_offset,
        };

        let mut output = Vec::with_capacity(index_offset as usize + self.keyframes.len() * VbrIndexEntry::SIZE);
        output.extend_from_slice(&header.encode());
        output.extend_from_slice(&self.payload);

        for keyframe in &self.keyframes {
            output.extend_from_slice(&keyframe.encode());
        }

        output
    }
}
