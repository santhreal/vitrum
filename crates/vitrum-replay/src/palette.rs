//! The default foreground and background.
//!
//! [`vitrum_grid::Cell`] stores straight RGBA and has no concept of "default
//! foreground", so SGR 39 and 49 have to be resolved to concrete channels before
//! they reach a cell. That resolution is a theme decision, which means it belongs
//! to whoever is displaying the replay, not to the replay.
//!
//! So [`Palette`] is data the caller supplies, and it is handed to the engine as
//! the terminal's default colours. It is also what blank cells are painted in, so
//! a screen erased with `ED 2` and a screen that was never written look identical,
//! which is what a terminal does.
//!
//! # Why the sixteen ANSI colours are not here
//!
//! They used to be, alongside a 256-entry lookup this crate resolved SGR through.
//! Ghostty resolves indexed colour now, out of its own palette, and libghostty's C
//! API takes a palette but `vitrum-vt` does not expose a setter for it. Publishing
//! sixteen fields that no longer paint anything would be worse than not publishing
//! them.
//!
//! The consequence is the good one. The daemon renders a live pane through the
//! same engine, so a replayed screen and the pane the user watched resolve
//! `SGR 31` through one table rather than two that agree until somebody edits one.
//! The 6x6x6 cube and the 24-step grey ramp are xterm's, unchanged, because
//! Ghostty uses xterm's table for those; only the sixteen named colours are
//! Ghostty's own theme rather than xterm's compiled-in defaults.

use vitrum_grid::{Rgba, Style};

/// What SGR 39 and 49 resolve to, and what a blank cell is painted in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
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
        fg: Rgba::rgb(0xe5, 0xe5, 0xe5),
        bg: Rgba::rgb(0x00, 0x00, 0x00),
    };

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
