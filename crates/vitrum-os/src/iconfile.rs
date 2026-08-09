//! Container formats for the rasters [`crate::mark`] produces: PNG, Windows
//! `.ico` and macOS `.icns`.
//!
//! Three platforms want the same picture in three wrappers, and every tool
//! that normally writes them — ImageMagick, `rsvg-convert`, `iconutil`,
//! Pillow — is a build dependency that has to exist on the machine cutting the
//! release and that has nothing to do with this product. The wrappers are a
//! few hundred bytes of header each, so they are written here and the icon set
//! becomes a pure function of the geometry instead of a directory of checked-in
//! binaries nobody can regenerate.
//!
//! # The compressor
//!
//! [`png`] emits a deflate stream of fixed-Huffman blocks with greedy LZ77
//! matching, which is a hundred lines and no dependency. Stored blocks were
//! tried first and rejected on the number: the icon set came to 3.1 MB, of
//! which 2.7 MB was the `.icns`, and an installer that writes three megabytes
//! of mostly-transparent pixels onto a user's disk is not a defensible thing
//! to ship. Compressed it is a few tens of kilobytes.
//!
//! Fixed Huffman rather than dynamic: the code lengths are in the
//! specification, so there is no table to build and no table to get wrong, and
//! a run of transparent pixels reaches the 258-byte match limit either way.
//! `every_png_round_trips_to_the_pixels_it_was_given` decodes the result with
//! a separate implementation rather than with this one.

use std::path::{Path, PathBuf};

use crate::branding::ICON_NAME;
use crate::icon::IconImage;
use crate::mark::{MARK_COLOUR, MARK_SIZES, mark_set};

/// CRC-32, as PNG defines it: the IEEE polynomial, reflected, initialised and
/// finalised with all ones.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Adler-32, the checksum trailing a zlib stream.
fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in bytes {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// One PNG chunk: length, type, payload, CRC over type and payload.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Fixed-Huffman literal code for a symbol: `(code, bit length)`.
///
/// Straight out of RFC 1951 section 3.2.6. The four ranges are the whole
/// table; there is nothing to compute and nothing to transmit.
fn fixed_literal(sym: u16) -> (u32, u32) {
    match sym {
        0..=143 => (0b0011_0000 + u32::from(sym), 8),
        144..=255 => (0b1_1001_0000 + u32::from(sym - 144), 9),
        256..=279 => (u32::from(sym - 256), 7),
        _ => (0b1100_0000 + u32::from(sym - 280), 8),
    }
}

/// Length codes: `(symbol, extra bits, smallest length)` for lengths 3..=258.
const LENGTHS: [(u16, u32, u16); 29] = [
    (257, 0, 3),
    (258, 0, 4),
    (259, 0, 5),
    (260, 0, 6),
    (261, 0, 7),
    (262, 0, 8),
    (263, 0, 9),
    (264, 0, 10),
    (265, 1, 11),
    (266, 1, 13),
    (267, 1, 15),
    (268, 1, 17),
    (269, 2, 19),
    (270, 2, 23),
    (271, 2, 27),
    (272, 2, 31),
    (273, 3, 35),
    (274, 3, 43),
    (275, 3, 51),
    (276, 3, 59),
    (277, 4, 67),
    (278, 4, 83),
    (279, 4, 99),
    (280, 4, 115),
    (281, 5, 131),
    (282, 5, 163),
    (283, 5, 195),
    (284, 5, 227),
    (285, 0, 258),
];

/// Distance codes: `(symbol, extra bits, smallest distance)`.
const DISTANCES: [(u32, u32, u32); 30] = [
    (0, 0, 1),
    (1, 0, 2),
    (2, 0, 3),
    (3, 0, 4),
    (4, 1, 5),
    (5, 1, 7),
    (6, 2, 9),
    (7, 2, 13),
    (8, 3, 17),
    (9, 3, 25),
    (10, 4, 33),
    (11, 4, 49),
    (12, 5, 65),
    (13, 5, 97),
    (14, 6, 129),
    (15, 6, 193),
    (16, 7, 257),
    (17, 7, 385),
    (18, 8, 513),
    (19, 8, 769),
    (20, 9, 1025),
    (21, 9, 1537),
    (22, 10, 2049),
    (23, 10, 3073),
    (24, 11, 4097),
    (25, 11, 6145),
    (26, 12, 8193),
    (27, 12, 12289),
    (28, 13, 16385),
    (29, 13, 24577),
];

/// Longest match deflate can encode, and the shortest worth encoding.
const MAX_MATCH: usize = 258;
const MIN_MATCH: usize = 3;
/// The sliding window, which is also the largest distance a match may reach.
const WINDOW: usize = 32768;

/// A deflate bit sink.
///
/// Bits go into a byte from the least significant end, which is the stream's
/// order; a Huffman code goes in most significant bit first, which is the
/// code's order. Keeping those two straight is the whole of the format's
/// bit-level trickiness, so they are two methods rather than one with a flag.
struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    bits: u32,
}

impl BitWriter {
    fn new(capacity: usize) -> Self {
        Self { out: Vec::with_capacity(capacity), acc: 0, bits: 0 }
    }

    /// `n` bits of `value`, least significant first. Extra bits are written
    /// this way.
    fn bits(&mut self, value: u32, n: u32) {
        self.acc |= value << self.bits;
        self.bits += n;
        while self.bits >= 8 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.bits -= 8;
        }
    }

    /// A Huffman code of `n` bits, most significant first.
    fn code(&mut self, code: u32, n: u32) {
        for i in (0..n).rev() {
            self.bits((code >> i) & 1, 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bits > 0 {
            self.out.push((self.acc & 0xFF) as u8);
        }
        self.out
    }
}

/// Hash of the three bytes at `at`, used to find a match candidate.
fn hash3(raw: &[u8], at: usize) -> usize {
    let h = (u32::from(raw[at]) << 16) | (u32::from(raw[at + 1]) << 8) | u32::from(raw[at + 2]);
    (h.wrapping_mul(0x9E37_79B1) >> 17) as usize & (WINDOW - 1)
}

/// A zlib stream of one fixed-Huffman deflate block over `raw`.
fn zlib(raw: &[u8]) -> Vec<u8> {
    // 0x78 0x01: deflate, 32 KiB window. The pair must be a multiple of 31
    // read big-endian, and 0x7801 is.
    let mut w = BitWriter::new(raw.len() / 4 + 64);
    w.out.extend_from_slice(&[0x78, 0x01]);
    // Final block, fixed Huffman: BFINAL = 1, BTYPE = 01.
    w.bits(1, 1);
    w.bits(1, 2);

    // One slot per hash, holding the most recent position with that hash. A
    // single candidate rather than a chain: the runs this compresses are runs
    // of identical bytes, where the most recent position is also the best one,
    // and a chain would buy ratio on data this never sees.
    let mut head = vec![usize::MAX; WINDOW];
    let mut i = 0;
    while i < raw.len() {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        if i + MIN_MATCH <= raw.len() {
            let h = hash3(raw, i);
            let candidate = head[h];
            head[h] = i;
            if candidate != usize::MAX && i - candidate <= WINDOW {
                let limit = MAX_MATCH.min(raw.len() - i);
                let mut len = 0;
                // Overlapping matches are legal and are what turns a run of
                // 258 zeroes into one length code at distance 1.
                while len < limit && raw[candidate + len] == raw[i + len] {
                    len += 1;
                }
                if len >= MIN_MATCH {
                    best_len = len;
                    best_dist = i - candidate;
                }
            }
        }

        if best_len >= MIN_MATCH {
            let &(sym, extra, base) = LENGTHS
                .iter()
                .rev()
                .find(|&&(_, _, base)| usize::from(base) <= best_len)
                .expect("every length from 3 to 258 has a code");
            let (code, n) = fixed_literal(sym);
            w.code(code, n);
            w.bits((best_len - usize::from(base)) as u32, extra);

            let &(dsym, dextra, dbase) = DISTANCES
                .iter()
                .rev()
                .find(|&&(_, _, base)| base as usize <= best_dist)
                .expect("every distance from 1 to 32768 has a code");
            // Distance codes are five fixed bits, not the literal table's.
            w.code(dsym, 5);
            w.bits(best_dist as u32 - dbase, dextra);

            // Every position inside the match still has to be indexed, or the
            // next run starts with no candidate to match against.
            for j in i + 1..i + best_len {
                if j + MIN_MATCH <= raw.len() {
                    head[hash3(raw, j)] = j;
                }
            }
            i += best_len;
        } else {
            let (code, n) = fixed_literal(u16::from(raw[i]));
            w.code(code, n);
            i += 1;
        }
    }

    // End of block.
    let (code, n) = fixed_literal(256);
    w.code(code, n);

    let mut out = w.finish();
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// An 8-bit RGBA PNG of `img`.
#[must_use]
pub fn png(img: &IconImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.rgba.len() + 128);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    // 8 bits per channel, colour type 6 (truecolour with alpha), deflate,
    // adaptive filtering, no interlace.
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);

    // Filter type 0 on every scanline. A paeth filter would compress better,
    // and there is no compressor here to benefit from it.
    let stride = img.width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * img.height as usize);
    for row in img.rgba.chunks_exact(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    chunk(&mut out, b"IDAT", &zlib(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Largest icon a `.ico` directory entry can describe.
///
/// The entry stores width and height in one byte each, with zero meaning 256.
/// Anything larger has no encoding at all, so a 512-pixel raster handed to
/// [`ico`] is dropped rather than written as a 0x0 entry that Explorer reads
/// as 256 and then fails to decode.
const ICO_MAX: u32 = 256;

/// Smallest frame written as a PNG rather than as a BMP.
///
/// A PNG frame needs Windows Vista, which is not a constraint: this program
/// needs a WebView2 runtime, so the oldest Windows it can run on is a decade
/// past that. What it saves is the whole reason for the threshold. The 128
/// pixel frame is 66 KiB as an uncompressed BMP and 2.6 KiB as a PNG, in a
/// file that is otherwise 40 KiB, and Explorer reads that file on every icon
/// draw of every shortcut pointing at the binary.
const ICO_PNG_FROM: u32 = 128;

/// A Windows `.ico` holding every image in `images` that fits.
///
/// Frames under [`ICO_PNG_FROM`] are 32-bit BMP, which is the encoding every
/// Windows version reads and the only sensible one at sizes where a PNG's own
/// header is a measurable share of the frame.
#[must_use]
pub fn ico(images: &[IconImage]) -> Vec<u8> {
    let kept: Vec<&IconImage> = images.iter().filter(|i| i.width <= ICO_MAX).collect();
    let mut out = Vec::new();
    // Reserved, type 1 (icon), count.
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(kept.len() as u16).to_le_bytes());

    let payloads: Vec<Vec<u8>> = kept
        .iter()
        .map(|img| if img.width >= ICO_PNG_FROM { png(img) } else { bmp_frame(img) })
        .collect();

    let mut offset = 6 + 16 * kept.len() as u32;
    for (img, data) in kept.iter().zip(&payloads) {
        // Zero means 256 in a directory entry.
        let dim = u8::try_from(img.width).unwrap_or(0);
        out.push(dim);
        out.push(dim);
        // No palette, reserved, one plane, 32 bits per pixel.
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for data in &payloads {
        out.extend_from_slice(data);
    }
    out
}

/// One 32-bit BMP frame for a `.ico`: a header, bottom-up BGRA, and an AND mask.
///
/// The height in the header is doubled because the format says so: the frame
/// nominally holds a colour bitmap and a mask bitmap stacked. The mask is all
/// zeros — "no pixel is masked out" — because the alpha channel already carries
/// the transparency, and every reader since Windows XP uses it. The mask still
/// has to be there, at the right size, or the frame is rejected.
fn bmp_frame(img: &IconImage) -> Vec<u8> {
    let (w, h) = (img.width, img.height);
    let stride = w as usize * 4;
    // 1 bit per pixel, rows padded to four bytes.
    let mask_stride = (w as usize).div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + stride * h as usize + mask_stride * h as usize);

    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&(h * 2).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    // BI_RGB, image size, unused resolution and palette fields.
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((stride * h as usize) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);

    // Bottom-up, and BGRA rather than RGBA: a DIB is little-endian ARGB.
    for y in (0..h).rev() {
        let row = &img.rgba[y as usize * stride..(y as usize + 1) * stride];
        for px in row.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out.extend_from_slice(&vec![0u8; mask_stride * h as usize]);
    out
}

/// The `.icns` chunk type for a raster of `size`, or `None` if macOS has no
/// slot for it.
///
/// Two families, both PNG-carrying: `icp4`/`icp5` are the plain 16 and 32
/// pixel icons, and `ic07` upward are the sizes the Finder and the dock ask
/// for. `ic11` through `ic14` are the retina variants, which are the same
/// pixels under a different name; a bundle without them is rendered by
/// upscaling the 1x raster, which is exactly the blur this whole module exists
/// to avoid.
fn icns_types(size: u32) -> &'static [&'static [u8; 4]] {
    match size {
        16 => &[b"icp4"],
        32 => &[b"icp5", b"ic11"],
        64 => &[b"ic12"],
        128 => &[b"ic07"],
        256 => &[b"ic08", b"ic13"],
        512 => &[b"ic09", b"ic14"],
        _ => &[],
    }
}

/// A macOS `.icns` holding every image in `images` macOS has a slot for.
#[must_use]
pub fn icns(images: &[IconImage]) -> Vec<u8> {
    let mut body = Vec::new();
    for img in images {
        let types = icns_types(img.width);
        if types.is_empty() {
            continue;
        }
        let data = png(img);
        for kind in types {
            body.extend_from_slice(*kind);
            // The length covers the eight header bytes as well as the payload.
            body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
            body.extend_from_slice(&data);
        }
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Every file the platform icon set is made of, as a relative path and its
/// bytes.
///
/// The layout is a data directory's, not a private one: `icons/hicolor/...`
/// under `$XDG_DATA_HOME` is where a freedesktop desktop entry's `Icon=vitrum`
/// is resolved from, so passing `~/.local/share` to [`write_icon_set`] puts
/// every PNG exactly where the launcher already looks. The `.ico` and the
/// `.icns` sit beside the theme tree because Windows and macOS have no theme
/// tree to sit in.
#[must_use]
pub fn icon_set() -> Vec<(PathBuf, Vec<u8>)> {
    let images = mark_set(MARK_COLOUR);
    let mut files = Vec::with_capacity(MARK_SIZES.len() + 2);
    for img in &images {
        let n = img.width;
        files.push((
            PathBuf::from(format!("icons/hicolor/{n}x{n}/apps/{ICON_NAME}.png")),
            png(img),
        ));
    }
    files.push((PathBuf::from(format!("icons/{ICON_NAME}.ico")), ico(&images)));
    files.push((PathBuf::from(format!("icons/{ICON_NAME}.icns")), icns(&images)));
    files
}

/// Write the icon set under `dir`, leaving nothing behind if any write fails.
///
/// Every raster and every container is built in memory first, so the only
/// failures left are filesystem ones, and each of those unwinds the files
/// already written. A half-written theme tree is worse than no theme tree: the
/// launcher picks up whichever sizes landed and caches them, so the next
/// install has to fight a stale cache rather than an empty directory.
///
/// Returns the paths written, in the order they were written.
pub fn write_icon_set(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let files = icon_set();
    let mut written: Vec<PathBuf> = Vec::with_capacity(files.len());
    for (rel, bytes) in files {
        let path = dir.join(rel);
        let attempt = match path.parent() {
            Some(parent) => std::fs::create_dir_all(parent).and_then(|()| {
                std::fs::write(&path, &bytes)
            }),
            None => std::fs::write(&path, &bytes),
        };
        match attempt {
            Ok(()) => written.push(path),
            Err(e) => {
                for done in &written {
                    let _ = std::fs::remove_file(done);
                }
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("cannot write {}: {e}", path.display()),
                ));
            }
        }
    }
    Ok(written)
}
