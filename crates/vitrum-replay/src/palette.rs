//! Indexed colour to RGB.
//!
//! [`vitrum_grid::Cell`] stores straight RGBA and has no concept of "colour 4" or
//! "default foreground", so SGR has to be resolved to concrete channels before it
//! reaches a cell. That resolution is a theme decision, which means it belongs to
//! whoever is displaying the replay, not to the replay.
//!
//! So [`Palette`] is data the caller supplies. [`Palette::XTERM`] is the default
//! and matches what a terminal with no configuration does, which is the right
//! default for an exported recording that will be played somewhere else. A UI
//! showing a replay inside vitrum passes vitrum's own ramp instead, and the
//! replayed screen then matches the live one exactly.
//!
//! # Default foreground and background
//!
//! SGR 39 and 49 mean "back to the default", and there is no RGB value that means
//! that. [`Palette::fg`] and [`Palette::bg`] are what they resolve to, and they
//! are also what blank cells are painted in, so a screen erased with `ED 2` and a
//! screen that was never written look identical, which is what a terminal does.

use vitrum_grid::{Rgba, Style};

/// The 16 ANSI colours plus the default foreground and background.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    /// Colours 0 through 15: the eight normal then the eight bright.
    pub ansi: [Rgba; 16],
    /// What SGR 39 resolves to, and the colour of a blank cell's text.
    pub fg: Rgba,
    /// What SGR 49 resolves to, and the colour of a blank cell.
    pub bg: Rgba,
}

impl Palette {
    /// xterm's compiled-in defaults.
    ///
    /// These are the values every terminal falls back to with no theme
    /// configured, so a recording exported with this palette looks the same
    /// everywhere.
    pub const XTERM: Self = Self {
        ansi: [
            Rgba::rgb(0x00, 0x00, 0x00),
            Rgba::rgb(0xcd, 0x00, 0x00),
            Rgba::rgb(0x00, 0xcd, 0x00),
            Rgba::rgb(0xcd, 0xcd, 0x00),
            Rgba::rgb(0x00, 0x00, 0xee),
            Rgba::rgb(0xcd, 0x00, 0xcd),
            Rgba::rgb(0x00, 0xcd, 0xcd),
            Rgba::rgb(0xe5, 0xe5, 0xe5),
            Rgba::rgb(0x7f, 0x7f, 0x7f),
            Rgba::rgb(0xff, 0x00, 0x00),
            Rgba::rgb(0x00, 0xff, 0x00),
            Rgba::rgb(0xff, 0xff, 0x00),
            Rgba::rgb(0x5c, 0x5c, 0xff),
            Rgba::rgb(0xff, 0x00, 0xff),
            Rgba::rgb(0x00, 0xff, 0xff),
            Rgba::rgb(0xff, 0xff, 0xff),
        ],
        fg: Rgba::rgb(0xe5, 0xe5, 0xe5),
        bg: Rgba::rgb(0x00, 0x00, 0x00),
    };

    /// Levels the 6x6x6 colour cube steps through.
    ///
    /// Not evenly spaced: the first step is 95 and the rest are 40 apart, which
    /// is xterm's table and therefore the one every other terminal copied.
    const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

    /// Resolve one of the 256 indexed colours.
    ///
    /// - 0 to 15: [`Palette::ansi`].
    /// - 16 to 231: the 6x6x6 cube.
    /// - 232 to 255: the 24-step grey ramp.
    #[must_use]
    pub const fn indexed(&self, index: u8) -> Rgba {
        if index < 16 {
            return self.ansi[index as usize];
        }
        if index < 232 {
            let n = index - 16;
            return Rgba::rgb(
                Self::CUBE_LEVELS[(n / 36) as usize],
                Self::CUBE_LEVELS[((n / 6) % 6) as usize],
                Self::CUBE_LEVELS[(n % 6) as usize],
            );
        }
        let level = 8 + (index - 232) * 10;
        Rgba::rgb(level, level, level)
    }

    /// The style a blank cell and an untouched screen are painted in.
    #[must_use]
    pub const fn default_style(&self) -> Style {
        Style::new(self.fg, self.bg)
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::XTERM
    }
}
