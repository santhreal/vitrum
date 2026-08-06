//! Parser plus screen, and the ground-state probe keyframes depend on.
//!
//! An [`Emulator`] is a [`vte::Parser`] and a [`Screen`]. Feeding it bytes moves
//! the screen forward. That is the whole type.
//!
//! # Why `feed_byte` exists
//!
//! A keyframe is a clone of the screen, and a screen alone is not enough to
//! resume from: if the snapshot was taken with the parser halfway through
//! `ESC [ 3 1 m`, then restoring the screen and feeding `m` next would produce a
//! parser that has never seen the `ESC [ 3 1`, and the colour would be lost. A
//! keyframe is therefore only sound at a byte boundary where the parser is back
//! in its ground state with no partial UTF-8 buffered, because a fresh parser and
//! the live one are then indistinguishable for all future input.
//!
//! [`vte::Parser`] does not expose its state and is not `Clone`, so the boundary
//! is detected rather than read. [`Emulator::feed_byte`] feeds exactly one byte
//! and reports whether that byte triggered [`vte::Perform::print`],
//! [`vte::Perform::csi_dispatch`] or [`vte::Perform::esc_dispatch`], the three
//! callbacks vte only ever fires on a transition into ground. See
//! [`crate::perform`] for why the other five callbacks are not usable for this.
//!
//! One byte at a time is slower than bulk feeding, which is why only the search
//! for the boundary runs that way. Ordinary terminal output prints a character
//! every byte or two, so the search almost always ends on the first byte, and
//! [`KeyframeIndex::build`](crate::KeyframeIndex::build) bulk-feeds everything
//! between boundaries.

use vte::Parser;

use crate::error::Result;
use crate::palette::Palette;
use crate::screen::Screen;

/// A VT parser driving a [`Screen`].
pub struct Emulator {
    parser: Parser,
    screen: Screen,
}

impl core::fmt::Debug for Emulator {
    /// [`vte::Parser`] is not `Debug`, and its internals are not something a caller
    /// can act on anyway. The screen is what a reader of a log wants.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Emulator")
            .field("cols", &self.screen.cols())
            .field("rows", &self.screen.rows())
            .field("cursor", &self.screen.cursor())
            .field("on_alt_screen", &self.screen.on_alt_screen())
            .finish_non_exhaustive()
    }
}

impl Emulator {
    /// A fresh emulator over a blank `cols` x `rows` screen.
    ///
    /// # Errors
    ///
    /// [`crate::Error::Geometry`] when the size is not one
    /// [`vitrum_grid::CellGrid`] accepts.
    pub fn new(cols: u16, rows: u16, palette: Palette) -> Result<Self> {
        Ok(Self::resume(Screen::new(cols, rows, palette)?))
    }

    /// An emulator with a fresh parser over an existing screen.
    ///
    /// This is how a seek restores a keyframe. The parser is new, which is exactly
    /// right: the keyframe was only taken at a boundary where a fresh parser and
    /// the recording one behave identically.
    #[must_use]
    pub fn resume(screen: Screen) -> Self {
        Self {
            parser: Parser::new(),
            screen,
        }
    }

    /// The current screen.
    #[must_use]
    pub const fn screen(&self) -> &Screen {
        &self.screen
    }

    /// The current screen, mutable, for a renderer clearing damage.
    pub const fn screen_mut(&mut self) -> &mut Screen {
        &mut self.screen
    }

    /// Take the screen, consuming the emulator.
    #[must_use]
    pub fn into_screen(self) -> Screen {
        self.screen
    }

    /// Feed a run of bytes.
    ///
    /// The parser keeps whatever state the run ends in, so a caller may split the
    /// stream anywhere: a UTF-8 character or an escape sequence cut in half by the
    /// split is completed by the next call.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.screen, bytes);
    }

    /// Feed exactly one byte and report whether the parser is now in its ground
    /// state.
    ///
    /// `true` means a keyframe taken *after* this byte can be resumed from. `false`
    /// means the byte landed inside a UTF-8 character, an escape sequence, an OSC
    /// or DCS string, or one byte short of a `ESC \` terminator.
    ///
    /// A `false` answer is never wrong in the unsafe direction: it can only cause
    /// a keyframe to be skipped or slid forward, never a keyframe to be taken
    /// somewhere it cannot be resumed from.
    pub fn feed_byte(&mut self, byte: u8) -> bool {
        self.screen.ground = false;
        self.parser.advance(&mut self.screen, &[byte]);
        self.screen.ground
    }
}
