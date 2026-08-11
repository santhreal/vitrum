//! The brand mark, asserted pixel by pixel, and the three containers it is
//! shipped in.
//!
//! The mark is the only picture this product publishes, and it is generated
//! rather than checked in, so nothing but this file stands between a wrong
//! coordinate and an icon that ships on three platforms. Every case here is
//! driven from [`MARK_SIZES`], so adding a size adds coverage instead of adding
//! an untested raster.

use crate::icon::{IconImage, Rgba};
use crate::iconfile::{icns, ico, icon_set, png, write_icon_set};
use crate::mark::{
    MARK_COLOUR, MARK_CULET_Y, MARK_GIRDLE_STOP, MARK_GIRDLE_Y, MARK_GRID, MARK_SIZES, mark_set,
    render_mark,
};

/// The pixel row whose centre is nearest `grid_y` from above.
///
/// Pixel centres sit at `(r + 0.5) * 96 / n` on the grid, so the row covering a
/// grid coordinate is not `grid_y * n / 96`: at 16 pixels that lands half a row
/// low and reads the empty space under the culet.
fn row_at(grid_y: f64, n: u32) -> u32 {
    ((grid_y / MARK_GRID * f64::from(n)) - 0.5).floor().max(0.0) as u32
}

/// The pixel column whose centre is nearest `grid_x` from the left, with
/// `grid_x` measured from the left edge of the grid rather than from the axis.
fn col_at(grid_x: f64, n: u32) -> u32 {
    row_at(grid_x, n)
}

fn alpha(img: &IconImage, x: u32, y: u32) -> u8 {
    img.pixel(x, y).expect("pixel inside the image").a
}

/// The mark must stay inside its box at every shipped size.
///
/// The outermost ink is a round cap at grid x = 36 + half the stroke, and the
/// stroke is floored in device pixels at small sizes. If that floor is ever
/// raised far enough, or the geometry is scaled to the box rather than to the
/// grid, the mark touches the frame and the four corners stop being empty,
/// which is what an icon clipped by its own container looks like.
#[test]
fn the_mark_stays_inside_the_box_at_every_shipped_size() {
    for &n in MARK_SIZES {
        let img = render_mark(n, MARK_COLOUR);
        assert_eq!((img.width, img.height), (n, n), "{n}: wrong raster size");
        for (x, y) in [(0, 0), (n - 1, 0), (0, n - 1), (n - 1, n - 1)] {
            assert_eq!(
                alpha(&img, x, y),
                0,
                "{n}: corner ({x},{y}) has ink, so the mark is clipped by its own box"
            );
        }
    }
}

/// The culet must carry ink at every shipped size.
///
/// The culet is the point the whole pavilion converges on, and it is the first
/// thing a rasteriser loses: it is the narrowest part of the drawing and it
/// sits at the bottom edge of the ink, where a half-pixel error puts the sample
/// in the empty space below it. A mark whose point has evaporated reads as a
/// blunt wedge.
#[test]
fn the_culet_carries_ink_at_every_shipped_size() {
    for &n in MARK_SIZES {
        let img = render_mark(n, MARK_COLOUR);
        let row = row_at(MARK_CULET_Y, n);
        for x in [n / 2 - 1, n / 2] {
            assert!(
                alpha(&img, x, row) > 0,
                "{n}: the culet row {row} has no ink at column {x}"
            );
        }
    }
}

/// The girdle must be open across the middle at every shipped size.
///
/// The girdle is two segments, not one line, so the V reads through the gap
/// between them and the T stem stands in the opening. Draw it as a single
/// `M12 42 H84` and the mark becomes a generic diamond outline with a bar
/// across it.
///
/// The assertion is relative rather than absolute, and that is the honest
/// shape of the claim: the opening is 7.6 grid units wide, which is 1.3 device
/// pixels at 16, so at the smallest size the round caps bleed into it and no
/// pixel there is empty. What must always hold is that the gap is dimmer than
/// both the girdle beside it and the stem inside it. At 32 and up the gap is
/// empty outright.
#[test]
fn the_girdle_is_open_at_every_shipped_size() {
    for &n in MARK_SIZES {
        let img = render_mark(n, MARK_COLOUR);
        let row = row_at(MARK_GIRDLE_Y, n);

        // Midway along the left girdle segment, which runs from grid x = 12.
        let edge = col_at(MARK_GRID / 2.0 - 26.0, n);
        // The middle of the opening. The girdle's round cap and the stem's are
        // the same radius, so the midpoint between where the girdle's ink stops
        // and where the stem's ink starts is simply half way from the axis to
        // the girdle's stop.
        let gap = col_at(MARK_GRID / 2.0 - MARK_GIRDLE_STOP / 2.0, n);
        // The T stem, on the axis.
        let stem = col_at(MARK_GRID / 2.0, n);

        let (a_edge, a_gap, a_stem) =
            (alpha(&img, edge, row), alpha(&img, gap, row), alpha(&img, stem, row));

        assert!(a_edge > 0, "{n}: the girdle has no ink at column {edge}");
        assert!(a_stem > 0, "{n}: the T stem has no ink at column {stem}");
        assert!(
            a_gap < a_edge && a_gap < a_stem,
            "{n}: the girdle is not open: gap alpha {a_gap} at column {gap} is not \
             dimmer than the girdle ({a_edge}) and the stem ({a_stem})"
        );

        // The same three columns mirrored, so a right half drawn from a
        // different table would be caught here and not only by the symmetry
        // case.
        let a_edge_r = alpha(&img, n - 1 - edge, row);
        let a_gap_r = alpha(&img, n - 1 - gap, row);
        assert!(a_edge_r > 0, "{n}: the right girdle has no ink");
        assert!(a_gap_r < a_edge_r, "{n}: the right side of the girdle is not open");
    }
}

/// The raster must be symmetric about its vertical axis, byte for byte.
///
/// Not approximately: one alpha step of asymmetry is visible at 16 pixels as a
/// mark that leans. Point sampling with a scale computed per column, or a
/// mirrored copy of the geometry table with its own rounding, both produce a
/// raster that is symmetric to the eye and not to the byte, and both are one
/// refactor away.
#[test]
fn the_mark_is_symmetric_about_its_vertical_axis() {
    for &n in MARK_SIZES {
        let img = render_mark(n, MARK_COLOUR);
        for y in 0..n {
            for x in 0..n / 2 {
                assert_eq!(
                    img.pixel(x, y),
                    img.pixel(n - 1 - x, y),
                    "{n}: ({x},{y}) and its mirror differ"
                );
            }
        }
    }
}

/// The diagonals must be anti-aliased.
///
/// Four of the mark's five shapes are diagonals. A hard inside/outside test
/// gives every pixel alpha 0 or 255, which at 16 and 24 pixels is a staircase
/// and at 512 is a rope. The proof is that partial coverage exists at all: a
/// row crossing the pavilion must contain alphas that are neither empty nor
/// full.
#[test]
fn the_diagonals_are_anti_aliased() {
    for &n in MARK_SIZES {
        let img = render_mark(n, MARK_COLOUR);
        let row = row_at((MARK_GIRDLE_Y + MARK_CULET_Y) / 2.0, n);
        let partial = (0..n).filter(|&x| (1..255).contains(&alpha(&img, x, row))).count();
        assert!(
            partial > 0,
            "{n}: row {row} crosses both pavilion facets with no partial coverage \
             anywhere, so the diagonals are aliased"
        );
    }
}

/// Rasterisation must be deterministic.
///
/// The reason the mark is generated rather than checked in is that the
/// generator is reproducible. Two calls that differ mean the release archive
/// and the installed copy can disagree.
#[test]
fn rasterisation_is_deterministic() {
    for &n in MARK_SIZES {
        assert_eq!(render_mark(n, MARK_COLOUR), render_mark(n, MARK_COLOUR), "{n}");
    }
}

/// Colour must be carried straight, never premultiplied.
///
/// [`IconImage`] is straight RGBA all the way through this crate, and the tray
/// backends hand the colour channels to the platform unscaled. A rasteriser
/// that multiplied the edge coverage into the colour would render every
/// anti-aliased pixel of the mark as a dark fringe on Windows and as a halo on
/// the StatusNotifierItem path.
#[test]
fn colour_is_carried_straight_and_never_premultiplied() {
    let colour = Rgba { r: 0x11, g: 0x99, b: 0xEE, a: 0xFF };
    let img = render_mark(64, colour);
    let mut partial = 0;
    for y in 0..64 {
        for x in 0..64 {
            let px = img.pixel(x, y).expect("in bounds");
            if px.a == 0 {
                continue;
            }
            assert_eq!(
                (px.r, px.g, px.b),
                (colour.r, colour.g, colour.b),
                "({x},{y}) alpha {} carries a scaled colour",
                px.a
            );
            partial += usize::from(px.a < 255);
        }
    }
    assert!(partial > 0, "no partially covered pixel to check");
}

/// The colour's own alpha must scale the whole mark.
///
/// A watermark is the mark at a lower alpha, not a second rasteriser.
#[test]
fn the_requested_alpha_scales_the_whole_mark() {
    let solid = render_mark(64, Rgba { a: 0xFF, ..MARK_COLOUR });
    let half = render_mark(64, Rgba { a: 0x80, ..MARK_COLOUR });
    let mut checked = 0;
    for y in 0..64 {
        for x in 0..64 {
            let s = u32::from(alpha(&solid, x, y));
            let h = u32::from(alpha(&half, x, y));
            assert!(h <= s, "({x},{y}) got brighter at half alpha");
            if s == 255 {
                assert_eq!(h, 128, "({x},{y}) solid ink did not scale to the request");
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no fully covered pixel to check");
}

// --------------------------------------------------------------------------
// Containers
// --------------------------------------------------------------------------

/// Inflate a zlib stream of stored and fixed-Huffman deflate blocks.
///
/// A second implementation of RFC 1951's decode side, written against the
/// specification rather than against the encoder. That is the point: an
/// encoder checked with its own inverse agrees with itself and with nothing
/// else, and the failure that matters is a stream this crate writes and
/// GTK, Explorer or the Finder refuses.
fn inflate(z: &[u8]) -> Vec<u8> {
    assert_eq!(z[0] & 0x0F, 8, "not a deflate stream");
    assert_eq!(
        (u16::from(z[0]) << 8 | u16::from(z[1])) % 31,
        0,
        "zlib header is not a multiple of 31"
    );

    let mut r = Bits { z, byte: 2, bit: 0 };

    const LENGTH_BASE: [(u16, u32); 29] = [
        (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0), (9, 0), (10, 0),
        (11, 1), (13, 1), (15, 1), (17, 1),
        (19, 2), (23, 2), (27, 2), (31, 2),
        (35, 3), (43, 3), (51, 3), (59, 3),
        (67, 4), (83, 4), (99, 4), (115, 4),
        (131, 5), (163, 5), (195, 5), (227, 5),
        (258, 0),
    ];
    const DIST_BASE: [(u32, u32); 30] = [
        (1, 0), (2, 0), (3, 0), (4, 0), (5, 1), (7, 1), (9, 2), (13, 2),
        (17, 3), (25, 3), (33, 4), (49, 4), (65, 5), (97, 5), (129, 6),
        (193, 6), (257, 7), (385, 7), (513, 8), (769, 8), (1025, 9),
        (1537, 9), (2049, 10), (3073, 10), (4097, 11), (6145, 11),
        (8193, 12), (12289, 12), (16385, 13), (24577, 13),
    ];

    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = r.take(1) == 1;
        let kind = r.take(2);
        match kind {
            0 => {
                r.align();
                let at = r.byte;
                let len = u16::from_le_bytes([z[at], z[at + 1]]) as usize;
                let nlen = u16::from_le_bytes([z[at + 2], z[at + 3]]);
                assert_eq!(!(len as u16), nlen, "stored block length is not complemented");
                out.extend_from_slice(&z[at + 4..at + 4 + len]);
                r.byte = at + 4 + len;
            }
            1 => loop {
                // Fixed literal codes are 7, 8 or 9 bits, and the ranges say
                // which after the first seven.
                let first = r.code(7);
                let sym = if first <= 23 {
                    256 + first
                } else {
                    let eight = (first << 1) | r.take(1);
                    if (48..=191).contains(&eight) {
                        eight - 48
                    } else if (192..=199).contains(&eight) {
                        280 + eight - 192
                    } else {
                        144 + ((eight << 1) | r.take(1)) - 400
                    }
                };
                if sym == 256 {
                    break;
                }
                if sym < 256 {
                    out.push(sym as u8);
                    continue;
                }
                let (base, extra) = LENGTH_BASE[(sym - 257) as usize];
                let len = u32::from(base) + r.take(extra);
                let dsym = r.code(5) as usize;
                let (dbase, dextra) = DIST_BASE[dsym];
                let dist = (dbase + r.take(dextra)) as usize;
                assert!(dist <= out.len(), "match reaches before the start of the output");
                // Byte at a time: an overlapping match is how a run is coded.
                for _ in 0..len {
                    out.push(out[out.len() - dist]);
                }
            },
            other => panic!("block type {other} is not written by this encoder"),
        }
        if last {
            break;
        }
    }
    r.align();
    assert_eq!(r.byte, z.len() - 4, "trailing bytes before the adler32");
    out
}

/// A deflate bit reader. Bits leave a byte from the least significant end.
struct Bits<'a> {
    z: &'a [u8],
    byte: usize,
    bit: u32,
}

impl Bits<'_> {
    /// `n` bits, least significant first, which is how extra bits arrive.
    fn take(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for i in 0..n {
            v |= u32::from((self.z[self.byte] >> self.bit) & 1) << i;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.byte += 1;
            }
        }
        v
    }

    /// A Huffman code of `n` bits, most significant first.
    fn code(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.take(1);
        }
        v
    }

    /// Advance to the next byte boundary, as a stored block requires.
    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.byte += 1;
        }
    }
}

/// Walk a PNG and return `(width, height, rgba)`.
///
/// Also checks every chunk's CRC, which is the field a hand-rolled encoder gets
/// wrong: a decoder that ignores CRCs will happily read a file that `pngcheck`,
/// GTK and the Finder all reject.
fn decode_png(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    assert_eq!(&bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A], "bad signature");
    let mut i = 8;
    let (mut w, mut h) = (0u32, 0u32);
    let mut idat = Vec::new();
    let mut saw_end = false;
    while i < bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        let kind = &bytes[i + 4..i + 8];
        let payload = &bytes[i + 8..i + 8 + len];
        let crc = u32::from_be_bytes(bytes[i + 8 + len..i + 12 + len].try_into().unwrap());
        let mut check = 0xFFFF_FFFFu32;
        for &b in &bytes[i + 4..i + 8 + len] {
            check ^= u32::from(b);
            for _ in 0..8 {
                let mask = (check & 1).wrapping_neg();
                check = (check >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        assert_eq!(!check, crc, "bad CRC on chunk {:?}", std::str::from_utf8(kind));
        match kind {
            b"IHDR" => {
                w = u32::from_be_bytes(payload[..4].try_into().unwrap());
                h = u32::from_be_bytes(payload[4..8].try_into().unwrap());
                assert_eq!(&payload[8..], &[8, 6, 0, 0, 0], "not 8-bit RGBA, uninterlaced");
            }
            b"IDAT" => idat.extend_from_slice(payload),
            b"IEND" => saw_end = true,
            _ => {}
        }
        i += 12 + len;
    }
    assert!(saw_end, "no IEND");
    let raw = inflate(&idat);
    let stride = w as usize * 4;
    let mut rgba = Vec::with_capacity(stride * h as usize);
    for row in raw.chunks_exact(stride + 1) {
        assert_eq!(row[0], 0, "scanline filter is not None");
        rgba.extend_from_slice(&row[1..]);
    }
    (w, h, rgba)
}

/// Every PNG the emitter writes must decode back to the pixels it was given.
///
/// The encoder is hand written: a CRC table, an adler32 and a stored deflate
/// stream. Any one of those being subtly wrong produces a file that this
/// crate's own reader would accept and that the platform's would not, so the
/// round trip is checked against a decoder that verifies the checksums rather
/// than against the encoder's own arithmetic.
#[test]
fn every_png_round_trips_to_the_pixels_it_was_given() {
    for &n in MARK_SIZES {
        let img = render_mark(n, MARK_COLOUR);
        let (w, h, rgba) = decode_png(&png(&img));
        assert_eq!((w, h), (n, n), "{n}: wrong dimensions in the PNG header");
        assert_eq!(rgba, img.rgba, "{n}: PNG pixels differ from the raster");
    }
}

/// The compressor must actually compress.
///
/// The encoder started out writing stored blocks, which are valid deflate and
/// produce a correct PNG, so every other case in this file passed while the
/// icon set came to 3.1 MB and the installer wrote all of it onto a user's
/// disk. Correctness alone cannot see that.
///
/// Two bounds, because the ratio is a function of size. A 16 pixel icon is a
/// kilobyte of scanlines with the mark covering most of them, and threefold is
/// all there is to win. At 128 and up the mark is a thin line on a wide
/// transparent field, the runs reach the 258 byte match limit, and twentyfold
/// is the floor below which the matcher has stopped matching. The sizes that
/// dominate the shipped set are the large ones, and they are the ones the
/// second bound holds to account.
#[test]
fn the_compressor_actually_compresses() {
    for &n in MARK_SIZES {
        let img = render_mark(n, MARK_COLOUR);
        let raw = img.rgba.len() + n as usize;
        let encoded = png(&img).len();
        let floor = if n >= 128 { 20 } else { 3 };
        assert!(
            encoded * floor < raw,
            "{n}: {encoded} bytes from {raw} of scanlines is under {floor}x, \
             which is barely better than storing them; the matcher is not \
             finding the transparent runs"
        );
    }
}

/// The zlib stream's adler32 must cover the raw scanlines.
///
/// A wrong checksum is accepted by some decoders and rejected by others, which
/// is the worst failure mode available: the icon works on the machine that
/// built it and is blank on the user's.
#[test]
fn the_png_checksum_covers_the_scanlines() {
    let bytes = png(&render_mark(32, MARK_COLOUR));
    // Locate the IDAT payload the same way a reader does.
    let mut i = 8;
    let mut idat = Vec::new();
    while i < bytes.len() {
        let len = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        if &bytes[i + 4..i + 8] == b"IDAT" {
            idat.extend_from_slice(&bytes[i + 8..i + 8 + len]);
        }
        i += 12 + len;
    }
    let raw = inflate(&idat);
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    let want = (b << 16) | a;
    let got = u32::from_be_bytes(idat[idat.len() - 4..].try_into().unwrap());
    assert_eq!(got, want, "the adler32 does not match the scanlines");
}

/// The `.ico` directory must describe frames that are actually in the file.
///
/// An offset or a length that runs past the end is the classic hand-rolled
/// `.ico` bug, and Explorer answers it by drawing the generic placeholder with
/// no error anywhere.
#[test]
fn the_ico_directory_addresses_real_frames() {
    let images = mark_set(MARK_COLOUR);
    let bytes = ico(&images);
    assert_eq!(&bytes[..4], &[0, 0, 1, 0], "not an icon resource");
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let expected = MARK_SIZES.iter().filter(|&&n| n <= 256).count();
    assert_eq!(count, expected, "wrong frame count");

    for e in 0..count {
        let entry = &bytes[6 + e * 16..6 + (e + 1) * 16];
        let len = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let off = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        assert_eq!(entry[0], entry[1], "frame {e} is not square");
        assert_eq!(u16::from_le_bytes(entry[6..8].try_into().unwrap()), 32, "not 32bpp");
        assert!(off + len <= bytes.len(), "frame {e} runs past the end of the file");
        assert!(off >= 6 + count * 16, "frame {e} overlaps the directory");
    }
}

/// 512 has no `.ico` encoding and must be dropped, not written as 0x0.
///
/// A directory entry stores the dimension in one byte, with zero meaning 256.
/// A 512-pixel frame written with a zero byte claims to be 256 and decodes to
/// a quarter of the icon.
#[test]
fn the_ico_drops_sizes_windows_cannot_describe() {
    let bytes = ico(&mark_set(MARK_COLOUR));
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut dims: Vec<u32> = Vec::new();
    for e in 0..count {
        let entry = &bytes[6 + e * 16..6 + (e + 1) * 16];
        dims.push(if entry[0] == 0 { 256 } else { u32::from(entry[0]) });
    }
    assert!(!dims.contains(&512), "a 512 pixel frame reached the .ico");
    assert!(dims.contains(&256), "the 256 pixel frame is missing");
    assert!(dims.contains(&16), "the 16 pixel frame is missing");
    assert_eq!(dims.len(), dims.iter().collect::<std::collections::BTreeSet<_>>().len());
}

/// Large frames must be PNG and small frames must be BMP.
///
/// The threshold is a size decision, not a compatibility one: an uncompressed
/// 128-pixel frame is 66 KiB and a PNG one is 2.6 KiB, and Explorer reads the
/// whole file on every icon draw. Below it a PNG's own header is a measurable
/// share of the frame and a BMP is smaller. Getting either side wrong is
/// silent: the file still parses, it is just several times the size it should
/// be, or a frame nothing can decode.
#[test]
fn the_ico_encodes_each_frame_in_the_smaller_form() {
    let bytes = ico(&mark_set(MARK_COLOUR));
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut seen_png = 0;
    let mut seen_bmp = 0;
    for e in 0..count {
        let entry = &bytes[6 + e * 16..6 + (e + 1) * 16];
        let dim = if entry[0] == 0 { 256 } else { u32::from(entry[0]) };
        let off = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let is_png = bytes[off..off + 8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        if dim >= 128 {
            assert!(is_png, "the {dim} frame is an uncompressed BMP");
            seen_png += 1;
        } else {
            assert!(!is_png, "the {dim} frame is a PNG, which costs more than it saves");
            seen_bmp += 1;
            // BITMAPINFOHEADER: 40 bytes, width, doubled height.
            let hdr = &bytes[off..off + 12];
            assert_eq!(u32::from_le_bytes(hdr[..4].try_into().unwrap()), 40);
            assert_eq!(u32::from_le_bytes(hdr[4..8].try_into().unwrap()), dim);
            assert_eq!(
                u32::from_le_bytes(hdr[8..12].try_into().unwrap()),
                dim * 2,
                "the {dim} frame's header height must cover the AND mask"
            );
        }
    }
    assert!(seen_png >= 2 && seen_bmp >= 4, "the threshold moved off the shipped sizes");
}

/// The `.icns` chunk table must walk exactly to the end of the file.
///
/// `icns` is a length-prefixed chunk walk with the total length in the header.
/// One byte of disagreement and macOS reports the file as corrupt and falls
/// back to the generic document icon.
#[test]
fn the_icns_chunk_table_walks_to_the_end() {
    let bytes = icns(&mark_set(MARK_COLOUR));
    assert_eq!(&bytes[..4], b"icns", "not an icns file");
    let total = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    assert_eq!(total, bytes.len(), "the header length is not the file length");

    let mut i = 8;
    let mut kinds = Vec::new();
    while i < bytes.len() {
        let kind = std::str::from_utf8(&bytes[i..i + 4]).expect("chunk type is ASCII").to_string();
        let len = u32::from_be_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        assert!(len >= 8, "chunk {kind} has no room for its own header");
        assert!(i + len <= bytes.len(), "chunk {kind} runs past the end");
        assert_eq!(
            &bytes[i + 8..i + 16],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "chunk {kind} does not carry a PNG"
        );
        kinds.push(kind);
        i += len;
    }
    assert_eq!(i, bytes.len(), "the chunk walk overshot the file");
    // The retina slots are the point of shipping 512: without them the Finder
    // upscales the 1x raster on every display sold in the last decade.
    for want in ["icp4", "icp5", "ic07", "ic08", "ic09", "ic11", "ic12", "ic13", "ic14"] {
        assert!(kinds.iter().any(|k| k == want), "the {want} slot is missing");
    }
}

/// The set must land where a freedesktop launcher looks for it.
///
/// `Icon=vitrum` in a desktop entry is resolved against
/// `<data dir>/icons/hicolor/<n>x<n>/apps/vitrum.png` and nowhere else. A set
/// written one directory to the side installs cleanly and shows the generic
/// placeholder.
#[test]
fn the_set_lands_on_the_hicolor_paths() {
    let files = icon_set();
    for &n in MARK_SIZES {
        let want = format!("icons/hicolor/{n}x{n}/apps/vitrum.png");
        assert!(
            files.iter().any(|(p, _)| p.to_string_lossy() == want),
            "{want} is not in the set"
        );
    }
    assert!(files.iter().any(|(p, _)| p.to_string_lossy() == "icons/vitrum.ico"));
    assert!(files.iter().any(|(p, _)| p.to_string_lossy() == "icons/vitrum.icns"));
    assert_eq!(files.len(), MARK_SIZES.len() + 2, "the set has files nobody asked for");
}

/// A failed write must leave nothing behind.
///
/// A half-written theme tree is worse than an empty one: the launcher caches
/// whichever sizes landed, so the next install fights a stale cache. The
/// failure is provoked by making one of the destinations a directory, which is
/// the same `EISDIR` an install into a path the user already owns hits.
#[test]
fn a_failed_write_leaves_nothing_behind() {
    let root = std::env::temp_dir().join(format!(
        "vitrum-iconset-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    // The .ico is written after every PNG, so blocking it proves the unwind
    // reaches back through the whole hicolor tree.
    std::fs::create_dir_all(root.join("icons/vitrum.ico")).expect("stage the blocker");

    let err = write_icon_set(&root).expect_err("writing over a directory must fail");
    assert!(
        err.to_string().contains("vitrum.ico"),
        "the error must name the file it could not write: {err}"
    );
    for &n in MARK_SIZES {
        let leftover = root.join(format!("icons/hicolor/{n}x{n}/apps/vitrum.png"));
        assert!(!leftover.exists(), "{} survived a failed write", leftover.display());
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A destination that is a file is `NotADirectory` on every platform.
///
/// WHY: the caller decides between "fix the path and run again" (exit 3) and
/// "the write failed" (exit 1) from the error's kind, and the two platforms
/// do not agree on what the first `mkdir` under a file reports. Unix gives
/// `ENOTDIR`; Windows gives `ERROR_DIRECTORY`, which std leaves
/// uncategorised, so `vitrum icons <a file>` exited 3 on one and 1 on the
/// other, and the windows leg of the platform matrix was red on exactly that.
///
/// This does NOT cover a file part way up the path, where the kind still
/// comes from the platform; it covers the destination itself, which is the
/// argument an operator mistypes.
#[test]
fn a_destination_that_is_a_file_is_not_a_directory() {
    let root = std::env::temp_dir().join(format!(
        "vitrum-iconset-file-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&root);
    std::fs::write(&root, b"x").expect("stage the file");

    let err = write_icon_set(&root).expect_err("a file is not a directory");
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::NotADirectory,
        "a destination that is a file was reported as {:?}, so the caller \
         cannot tell a wrong argument from a broken write: {err}",
        err.kind()
    );
    assert_eq!(
        std::fs::read(&root).expect("the file is still there"),
        b"x",
        "the refusal wrote through the destination"
    );
    let _ = std::fs::remove_file(&root);
}

/// A clean write must produce every file, and each must be readable back.
#[test]
fn a_clean_write_produces_the_whole_set() {
    let root = std::env::temp_dir().join(format!("vitrum-iconset-ok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let written = write_icon_set(&root).expect("write the set");
    assert_eq!(written.len(), MARK_SIZES.len() + 2);
    for path in &written {
        let bytes = std::fs::read(path).expect("read back");
        assert!(!bytes.is_empty(), "{} is empty", path.display());
    }
    let png_16 = std::fs::read(root.join("icons/hicolor/16x16/apps/vitrum.png")).expect("16px");
    let (w, h, _) = decode_png(&png_16);
    assert_eq!((w, h), (16, 16));
    let _ = std::fs::remove_dir_all(&root);
}
