//! The storage primitives a terminal grid is made of: colour, attributes, and
//! the single [`Cell`].
//!
//! Everything here is `Copy` and fixed size. A [`Cell`] is 16 bytes, so a
//! 200x50 grid is 160 KiB in one allocation. No cell ever owns heap memory,
//! which is what keeps a 20-session client's memory flat.

use unicode_width::UnicodeWidthChar;

/// Straight 8-bit-per-channel colour, stored exactly as the terminal produced
/// it (no linearisation, no premultiplication).
///
/// Byte order is `r, g, b, a`, which is also the memory layout the renderer
/// hands to the GPU as a `Unorm8x4` vertex attribute, so no swizzle happens
/// anywhere on the hot path.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(C)]
pub struct Rgba {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel. 255 is opaque.
    pub a: u8,
}

impl Rgba {
    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);
    /// Opaque black.
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    /// Opaque white.
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    /// Opaque colour from three channels.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Colour from four channels.
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// The four channels in GPU upload order.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// Rebuild a colour from [`Rgba::to_bytes`] output.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self {
            r: bytes[0],
            g: bytes[1],
            b: bytes[2],
            a: bytes[3],
        }
    }
}

/// Rendition bits for one cell.
///
/// Only the four attributes the renderer actually draws are modelled. Bold and
/// italic select a font face (or a synthesised one), reverse swaps foreground
/// and background before the cell reaches the GPU, and underline is drawn by
/// the fragment shader.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Attrs(u8);

impl Attrs {
    /// No attributes set.
    pub const NONE: Self = Self(0);
    /// Draw with the bold face.
    pub const BOLD: Self = Self(1 << 0);
    /// Draw with the italic face.
    pub const ITALIC: Self = Self(1 << 1);
    /// Draw a rule at the font's underline position.
    pub const UNDERLINE: Self = Self(1 << 2);
    /// Swap foreground and background at draw time.
    pub const REVERSE: Self = Self(1 << 3);

    /// Every bit this type defines.
    pub const ALL: Self = Self(0b1111);

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Build from raw bits, dropping any bit this type does not define.
    ///
    /// Truncating rather than rejecting is deliberate: a VT parser that learns
    /// a new SGR code must not be able to smuggle an undefined bit into the
    /// renderer's flag word.
    #[must_use]
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & Self::ALL.0)
    }

    /// True when every bit of `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when no bit is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Set every bit of `other`.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Clear every bit of `other`.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl core::ops::BitOr for Attrs {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.with(rhs)
    }
}

impl core::ops::BitOrAssign for Attrs {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::fmt::Debug for Attrs {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_empty() {
            return f.write_str("Attrs(NONE)");
        }
        f.write_str("Attrs(")?;
        let mut first = true;
        for (bit, name) in [
            (Self::BOLD, "BOLD"),
            (Self::ITALIC, "ITALIC"),
            (Self::UNDERLINE, "UNDERLINE"),
            (Self::REVERSE, "REVERSE"),
        ] {
            if self.contains(bit) {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        f.write_str(")")
    }
}

/// Which part of a character a cell holds.
///
/// A double-width character occupies two columns: the left column is
/// [`CellSlot::WideHead`] and carries the character, the right column is
/// [`CellSlot::WideTail`] and carries nothing. Keeping the tail as a real cell
/// (rather than, say, a wider first cell) is what lets the grid stay a flat
/// `cols * rows` array with O(1) addressing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(u8)]
pub enum CellSlot {
    /// A single-column character.
    #[default]
    Single,
    /// Left column of a double-width character; holds the character.
    WideHead,
    /// Right column of a double-width character; holds no character.
    WideTail,
}

impl CellSlot {
    /// How many columns this slot draws. The tail draws nothing because the
    /// head's quad already covers both columns.
    #[must_use]
    pub const fn drawn_columns(self) -> u16 {
        match self {
            Self::Single => 1,
            Self::WideHead => 2,
            Self::WideTail => 0,
        }
    }
}

/// Colour and rendition applied to written text.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Style {
    /// Glyph colour.
    pub fg: Rgba,
    /// Cell background colour.
    pub bg: Rgba,
    /// Rendition bits.
    pub attrs: Attrs,
}

impl Style {
    /// White on black, no attributes: the conventional terminal default.
    pub const DEFAULT: Self = Self {
        fg: Rgba::WHITE,
        bg: Rgba::BLACK,
        attrs: Attrs::NONE,
    };

    /// Style with the given colours and no attributes.
    #[must_use]
    pub const fn new(fg: Rgba, bg: Rgba) -> Self {
        Self {
            fg,
            bg,
            attrs: Attrs::NONE,
        }
    }

    /// Copy of this style with `attrs` set.
    #[must_use]
    pub const fn with_attrs(self, attrs: Attrs) -> Self {
        Self { attrs, ..self }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One grid cell: exactly 16 bytes, no heap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    /// The character drawn here. `'\0'` for a [`CellSlot::WideTail`], which
    /// draws nothing.
    pub ch: char,
    /// Glyph colour before the reverse attribute is applied.
    pub fg: Rgba,
    /// Background colour before the reverse attribute is applied.
    pub bg: Rgba,
    /// Rendition bits.
    pub attrs: Attrs,
    /// Which column of a (possibly wide) character this is.
    pub slot: CellSlot,
}

impl Cell {
    /// A blank cell painted in `style`.
    #[must_use]
    pub const fn blank(style: Style) -> Self {
        Self {
            ch: ' ',
            fg: style.fg,
            bg: style.bg,
            attrs: style.attrs,
            slot: CellSlot::Single,
        }
    }

    /// A single-column cell holding `ch`.
    ///
    /// This does not validate the width of `ch`; use
    /// [`CellGrid::write_char`](crate::CellGrid::write_char) when the caller is
    /// a VT front end and the character may be wide.
    #[must_use]
    pub const fn new(ch: char, style: Style) -> Self {
        Self {
            ch,
            fg: style.fg,
            bg: style.bg,
            attrs: style.attrs,
            slot: CellSlot::Single,
        }
    }

    /// The colour and rendition of this cell as a [`Style`].
    #[must_use]
    pub const fn style(self) -> Style {
        Style {
            fg: self.fg,
            bg: self.bg,
            attrs: self.attrs,
        }
    }

    /// Foreground and background after applying [`Attrs::REVERSE`]. This is the
    /// pair the renderer uploads.
    #[must_use]
    pub const fn resolved_colors(self) -> (Rgba, Rgba) {
        if self.attrs.contains(Attrs::REVERSE) {
            (self.bg, self.fg)
        } else {
            (self.fg, self.bg)
        }
    }

    /// True when this cell has no glyph to rasterise: a blank, or the tail of a
    /// wide pair. The renderer skips the atlas entirely for these.
    #[must_use]
    pub const fn is_glyphless(self) -> bool {
        matches!(self.slot, CellSlot::WideTail) || self.ch == ' ' || self.ch == '\0'
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Style::DEFAULT)
    }
}

/// How many grid columns a character claims.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CharWidth {
    /// A C0/C1 control character. It has no printable form and the grid refuses
    /// to store it; the VT front end must act on it instead.
    Control,
    /// A combining mark or other zero-width character. The grid stores one
    /// `char` per cell and cannot compose, so it refuses these too.
    ZeroWidth,
    /// One column.
    Narrow,
    /// Two columns (CJK, most emoji, fullwidth forms).
    Wide,
}

impl CharWidth {
    /// Columns claimed, or `None` when the character cannot be stored.
    #[must_use]
    pub const fn columns(self) -> Option<u16> {
        match self {
            Self::Narrow => Some(1),
            Self::Wide => Some(2),
            Self::Control | Self::ZeroWidth => None,
        }
    }
}

/// Classify `ch` by the number of grid columns it claims.
///
/// Uses the East Asian Width property with ambiguous characters treated as
/// narrow, matching every terminal that does not run in an explicitly CJK
/// locale.
#[must_use]
pub fn char_width(ch: char) -> CharWidth {
    match UnicodeWidthChar::width(ch) {
        None => CharWidth::Control,
        Some(0) => CharWidth::ZeroWidth,
        Some(1) => CharWidth::Narrow,
        Some(_) => CharWidth::Wide,
    }
}
