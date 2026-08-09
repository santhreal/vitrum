//! Ghostty, driving a [`Screen`].
//!
//! An [`Emulator`] is a [`vitrum_vt::Vt`] and the [`Screen`] it projects onto.
//! Feeding it bytes moves the screen forward. That is the whole type.
//!
//! # Why there is no ground-state probe any more
//!
//! There used to be one, and it existed because a checkpoint was a *clone of the
//! screen*: the parser was thrown away and a fresh one created beside the
//! restored cells, so the snapshot was only sound at a byte where a fresh parser
//! and the recording one were provably indistinguishable. `feed_byte` fed one
//! byte at a time looking for that point, and it decided the point had been
//! reached by watching which of the `vte` crate's eight callbacks fired.
//!
//! That was a belief about somebody else's parser, held one call away from the
//! parser itself, and it was the bug surface of the whole seek path: every
//! callback that crate added, reordered, or fired one byte earlier than the
//! comment assumed would move a checkpoint onto a position it could not be
//! resumed from, silently, with every screen still looking plausible.
//!
//! Nothing is checkpointed now (see [`crate::replay`]), and a live engine has no
//! such requirement. An engine that has been fed `base..s` *is* the state at `s`
//! for every `s`, including one in the middle of a UTF-8 character, in the middle
//! of `ESC [ 3 1 m`, or on the `ESC` of an `ESC \` OSC terminator. There is no
//! unsafe position left to detect, so there is no probe and no scan bound, and a
//! rewind resumes from the base of the stream, which is sound at every byte.

use vitrum_vt::{Vt, VtOptions};

use crate::error::{Error, Result};
use crate::palette::Palette;
use crate::screen::{Cursor, Screen};

/// Ghostty's terminal engine, projected onto a [`Screen`].
pub struct Emulator {
    vt: Vt,
    screen: Screen,
    /// Where device-query replies go to die. One buffer per emulator, reused,
    /// so a projection that has nothing to discard allocates nothing.
    discard: Vec<u8>,
}

impl core::fmt::Debug for Emulator {
    /// [`Vt`] prints as an opaque engine handle, and its internals are not
    /// something a caller can act on anyway. The screen is what a reader of a log
    /// wants.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Emulator")
            .field("cols", &self.screen.cols())
            .field("rows", &self.screen.rows())
            .field("cursor", &self.screen.cursor())
            .finish_non_exhaustive()
    }
}

impl Emulator {
    /// A fresh emulator over a blank `cols` x `rows` screen.
    ///
    /// The engine is given no scrollback. A [`Screen`] is `rows` tall and this
    /// crate never scrolls the viewport, so scrollback would be rows nobody can
    /// ask for, paid for once per emulator and again once per checkpoint.
    ///
    /// # Errors
    ///
    /// [`Error::Geometry`] when the size is not one [`vitrum_grid::CellGrid`]
    /// accepts, and [`Error::Engine`] when Ghostty refuses it.
    pub fn new(cols: u16, rows: u16, palette: Palette) -> Result<Self> {
        let screen = Screen::new(cols, rows, palette)?;
        let mut vt = Vt::new(VtOptions {
            cols,
            rows,
            max_scrollback: 0,
        })
        .map_err(|error| Error::Engine(error.to_string()))?;

        // SGR 39 and 49 mean "back to the default", and the default is a theme
        // decision the caller made. Handing it to the engine is what makes a
        // blank cell, an erased cell and a `CSI 39 m` cell agree.
        vt.set_theme(palette.fg, palette.bg, None)
            .map_err(|error| Error::Engine(error.to_string()))?;

        Ok(Self {
            vt,
            screen,
            discard: Vec::new(),
        })
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

    /// Feed a run of bytes and reproject the screen.
    ///
    /// The engine keeps whatever state the run ends in, so a caller may split the
    /// stream anywhere: a UTF-8 character or an escape sequence cut in half by the
    /// split is completed by the next call.
    ///
    /// # Errors
    ///
    /// [`Error::Engine`] when the engine handle cannot be read back.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<()> {
        self.vt.feed(bytes);
        self.project()
    }

    /// Feed a run of bytes without reprojecting.
    ///
    /// The engine advances; the [`Screen`] does not, and stays stale until the
    /// next [`Emulator::feed`] or [`Emulator::project`]. This is what a build
    /// pass uses when it is going to feed the next segment anyway, because
    /// projecting a screen nobody is going to look at is the largest avoidable
    /// cost in the pass.
    pub fn feed_raw(&mut self, bytes: &[u8]) {
        self.vt.feed(bytes);
    }

    /// Bring the [`Screen`] up to date with the engine.
    ///
    /// # Errors
    ///
    /// [`Error::Engine`] when the engine handle cannot be read back.
    pub fn project(&mut self) -> Result<()> {
        self.vt
            .sync(self.screen.grid_mut())
            .map_err(|error| Error::Engine(error.to_string()))?;

        let cursor = self
            .vt
            .cursor()
            .map_err(|error| Error::Engine(error.to_string()))?;
        self.screen.set_cursor(Cursor {
            col: cursor.col,
            row: cursor.row,
            visible: cursor.visible,
        });

        // Last-wins, and taken rather than read: the engine reports a title only
        // when it changed, so an untaken title would be re-applied on every
        // projection and a taken one that is `None` means "still the old one".
        if let Some(title) = self.vt.events().take_title() {
            self.screen.set_title(title);
        }

        // Replay has no PTY. A program that asked the recorded session a question
        // got its answer then; asking us now would invent traffic that never
        // happened, so the reply is dropped rather than queued for a writer that
        // does not exist.
        self.discard.clear();
        self.vt.drain_pty_write(&mut self.discard);

        Ok(())
    }
}
