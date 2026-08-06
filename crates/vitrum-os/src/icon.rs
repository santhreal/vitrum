//! A tiny deterministic icon rasteriser, shared by the tray and the Windows
//! taskbar overlay.
//!
//! Three platforms need the same picture ("N sessions want you") in three pixel
//! formats, at sizes between 16 and 32 pixels. Pulling in a font stack and an
//! image decoder to draw two digits inside a circle would be absurd, and worse,
//! it would make the result depend on which fonts the machine has. A 5x7 bitmap
//! font and a rasteriser that is a pure function of `(size, count)` renders
//! identically everywhere and, unlike anything backed by GDI or Core Graphics,
//! can be asserted pixel by pixel from a Linux test run.

/// Glyph cell width in the bitmap font.
pub(crate) const GLYPH_WIDTH: usize = 5;
/// Glyph cell height in the bitmap font.
pub(crate) const GLYPH_HEIGHT: usize = 7;
/// Blank columns between two glyphs.
pub(crate) const GLYPH_GAP: usize = 1;

/// Rows of a glyph, most significant of the low five bits leftmost.
type Glyph = [u8; GLYPH_HEIGHT];

const GLYPH_0: Glyph = [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110];
const GLYPH_1: Glyph = [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110];
const GLYPH_2: Glyph = [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111];
const GLYPH_3: Glyph = [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110];
const GLYPH_4: Glyph = [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010];
const GLYPH_5: Glyph = [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110];
const GLYPH_6: Glyph = [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110];
const GLYPH_7: Glyph = [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000];
const GLYPH_8: Glyph = [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110];
const GLYPH_9: Glyph = [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100];
const GLYPH_PLUS: Glyph = [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000];
const GLYPH_GT: Glyph = [0b10000, 0b01000, 0b00100, 0b00010, 0b00100, 0b01000, 0b10000];
const GLYPH_UNDERSCORE: Glyph = [0, 0, 0, 0, 0, 0, 0b11111];

/// The bitmap for one supported character, or `None`.
///
/// Returning `None` rather than a blank keeps a typo in a caller from silently
/// rendering an icon with a hole in it.
pub fn glyph(c: char) -> Option<Glyph> {
    Some(match c {
        '0' => GLYPH_0,
        '1' => GLYPH_1,
        '2' => GLYPH_2,
        '3' => GLYPH_3,
        '4' => GLYPH_4,
        '5' => GLYPH_5,
        '6' => GLYPH_6,
        '7' => GLYPH_7,
        '8' => GLYPH_8,
        '9' => GLYPH_9,
        '+' => GLYPH_PLUS,
        '>' => GLYPH_GT,
        '_' => GLYPH_UNDERSCORE,
        _ => return None,
    })
}

/// An 8-bit-per-channel colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    /// Red at full scale, never premultiplied by `a`.
    pub r: u8,
    /// Green at full scale, never premultiplied by `a`.
    pub g: u8,
    /// Blue at full scale, never premultiplied by `a`.
    pub b: u8,
    /// Opacity, `0` fully transparent and `0xFF` opaque. Straight alpha,
    /// because the tray and taskbar APIs on all three platforms take the
    /// colour channels unscaled and would darken a blended edge twice if
    /// this were premultiplied.
    pub a: u8,
}

impl Rgba {
    /// Fully transparent, the background every glyph is composited onto.
    pub const TRANSPARENT: Self = Self { r: 0, g: 0, b: 0, a: 0 };
    /// Attention red. Reads as urgent at 16 pixels against both a light and a
    /// dark taskbar.
    pub const ATTENTION: Self = Self { r: 0xD1, g: 0x3F, b: 0x3F, a: 0xFF };
    /// Idle grey.
    pub const IDLE: Self = Self { r: 0x6E, g: 0x76, b: 0x81, a: 0xFF };
    /// Glyph strokes, drawn over the coloured plate.
    pub const WHITE: Self = Self { r: 0xFF, g: 0xFF, b: 0xFF, a: 0xFF };
}

/// A rasterised icon in straight (non-premultiplied) RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    /// Columns. With `height` this fixes the required length of `rgba`.
    pub width: u32,
    /// Rows. With `width` this fixes the required length of `rgba`.
    pub height: u32,
    /// `width * height * 4` bytes, row-major from the top-left.
    pub rgba: Vec<u8>,
}

impl IconImage {
    /// Colour at a pixel. Out-of-bounds reads return `None` rather than
    /// panicking, because callers use this to assert edges.
    pub fn pixel(&self, x: u32, y: u32) -> Option<Rgba> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y * self.width + x) * 4) as usize;
        Some(Rgba { r: self.rgba[i], g: self.rgba[i + 1], b: self.rgba[i + 2], a: self.rgba[i + 3] })
    }

    /// BGRA byte order, which is what a Win32 32-bit DIB expects.
    pub fn to_bgra(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len());
        for px in self.rgba.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
        out
    }

    /// ARGB32 in network byte order, which is what the StatusNotifierItem
    /// specification requires of an icon pixmap.
    pub fn to_argb_network(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rgba.len());
        for px in self.rgba.chunks_exact(4) {
            out.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
        }
        out
    }

    fn blank(size: u32) -> Self {
        Self { width: size, height: size, rgba: vec![0; (size * size * 4) as usize] }
    }

    fn set(&mut self, x: u32, y: u32, c: Rgba) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = ((y * self.width + x) * 4) as usize;
        self.rgba[i] = c.r;
        self.rgba[i + 1] = c.g;
        self.rgba[i + 2] = c.b;
        self.rgba[i + 3] = c.a;
    }

    fn fill_disc(&mut self, c: Rgba) {
        let n = self.width as i64;
        // Doubled coordinates so the centre lands between pixels and the disc
        // is symmetric for even sizes. The radius is half the icon, so the disc
        // touches all four edge midpoints; an inscribed circle one pixel
        // smaller leaves a visible gap at 16 pixels.
        let limit = n * n;
        for y in 0..n {
            for x in 0..n {
                let dx = 2 * x - (n - 1);
                let dy = 2 * y - (n - 1);
                if dx * dx + dy * dy <= limit {
                    self.set(x as u32, y as u32, c);
                }
            }
        }
    }

    fn draw_text(&mut self, text: &str, scale: u32, c: Rgba) {
        let glyphs: Vec<Glyph> = text.chars().filter_map(glyph).collect();
        if glyphs.is_empty() {
            return;
        }
        let text_w =
            (glyphs.len() * GLYPH_WIDTH + (glyphs.len() - 1) * GLYPH_GAP) as u32 * scale;
        let text_h = GLYPH_HEIGHT as u32 * scale;
        // Integer-centred. For an odd remainder the extra pixel goes left and
        // up, which keeps a single digit visually centred in a disc.
        let ox = (self.width.saturating_sub(text_w)) / 2;
        let oy = (self.height.saturating_sub(text_h)) / 2;

        for (gi, g) in glyphs.iter().enumerate() {
            let gx = ox + (gi * (GLYPH_WIDTH + GLYPH_GAP)) as u32 * scale;
            for (row, bits) in g.iter().enumerate() {
                for col in 0..GLYPH_WIDTH {
                    if bits & (1 << (GLYPH_WIDTH - 1 - col)) == 0 {
                        continue;
                    }
                    for sy in 0..scale {
                        for sx in 0..scale {
                            self.set(
                                gx + col as u32 * scale + sx,
                                oy + row as u32 * scale + sy,
                                c,
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Text drawn inside a count badge.
///
/// Two glyphs is the honest maximum at 16 pixels: three 5-pixel glyphs plus
/// gaps is 17 pixels wide and would be clipped, so anything past nine becomes
/// `9+`. Rendering `12` as an unreadable smear is worse than rendering `9+`.
pub(crate) fn count_text(count: u32) -> String {
    match count {
        0 => String::new(),
        1..=9 => count.to_string(),
        _ => "9+".to_string(),
    }
}

/// The count badge: a filled disc with the count, or nothing at zero.
///
/// `None` at zero because every platform's "no badge" state is the absence of
/// an image, not a picture of nothing.
pub(crate) fn render_count_icon(size: u32, count: u32) -> Option<IconImage> {
    if count == 0 || size < GLYPH_HEIGHT as u32 {
        return None;
    }
    let mut img = IconImage::blank(size);
    img.fill_disc(Rgba::ATTENTION);
    img.draw_text(&count_text(count), (size / 16).max(1), Rgba::WHITE);
    Some(img)
}

/// The tray icon with nothing pending: a grey disc carrying a prompt glyph.
pub(crate) fn render_idle_icon(size: u32) -> IconImage {
    let mut img = IconImage::blank(size);
    img.fill_disc(Rgba::IDLE);
    img.draw_text(">_", (size / 16).max(1), Rgba::WHITE);
    img
}

/// The tray icon for a given attention count.
pub(crate) fn render_tray_icon(size: u32, count: u32) -> IconImage {
    render_count_icon(size, count).unwrap_or_else(|| render_idle_icon(size))
}
