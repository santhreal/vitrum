//! The colours a replay resolves "default" to.
//!
//! [`vitrum_grid::Cell`] stores straight RGBA and has no notion of a default
//! foreground, so `SGR 39` and `SGR 49` have to become concrete channels before
//! they reach a cell. Which channels is a display decision, so it belongs to
//! whoever is showing the replay, not to the replay.
//!
//! [`Palette`] is therefore data the caller supplies. It is handed to the engine
//! as the terminal's default colours and it is what blank cells are painted in,
//! so a screen erased with `ED 2` and a screen that was never written are the
//! same colour, which is what a terminal does.
//!
//! # Two colours, not eighteen
//!
//! This type carries a foreground and a background and nothing else.
//!
//! The sixteen named ANSI slots are not here because this crate no longer
//! resolves them. The engine owns indexed colour: `SGR 31` and `SGR 38;5;n` are
//! resolved inside it, out of its own table, and the cell arrives at this crate
//! already carrying concrete channels. Publishing sixteen fields that resolve
//! nothing would be a table a caller could set and then watch have no effect.
//!
//! That is also the property worth having. The daemon paints a live pane through
//! the same engine, so a replayed screen and the pane a session was watched in
//! resolve `SGR 31` through one table rather than through two that agree until
//! somebody edits one.
//!
//! A program can still override both of these at run time with `OSC 10` and
//! `OSC 11`, exactly as it can against a live terminal.

use vitrum_grid::{Rgba, Style};

/// What `SGR 39` and `SGR 49` resolve to, and what a blank cell is painted in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    /// What `SGR 39` resolves to, and the colour of a blank cell's text.
    pub fg: Rgba,
    /// What `SGR 49` resolves to, and the colour of a blank cell.
    pub bg: Rgba,
}

impl Palette {
    /// The colours a replay uses when the caller names none.
    ///
    /// Light grey on black. Grey rather than pure white because a full-white
    /// default puts unstyled text above every one of the sixteen named colours
    /// in luminance, and the engine's bright white then has nowhere to go.
    ///
    /// A recording exported with these colours reads the same wherever it is
    /// replayed, because it does not depend on a theme the reader has to have.
    pub const DEFAULT: Self = Self {
        fg: Rgba::rgb(0xe5, 0xe5, 0xe5),
        bg: Rgba::rgb(0x00, 0x00, 0x00),
    };

    /// A palette from an explicit pair.
    #[must_use]
    pub const fn new(fg: Rgba, bg: Rgba) -> Self {
        Self { fg, bg }
    }

    /// The style a blank cell and an untouched screen are painted in.
    #[must_use]
    pub const fn default_style(&self) -> Style {
        Style::new(self.fg, self.bg)
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::DEFAULT
    }
}
