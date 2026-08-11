//! The terminal engine: escape sequences in, a [`CellGrid`] out.
//!
//! This crate is the only escape-sequence parser in the product. A session's
//! bytes are handed to libghostty's VT, which is the state machine shipped in
//! the Ghostty terminal, and the screen it maintains is projected onto a
//! [`vitrum_grid::CellGrid`] that a wgpu surface paints. There is no second
//! parser anywhere: the daemon, the replay tool, the benchmark harness and the
//! live pane all read the same implementation, so a replayed screen and a live
//! one cannot disagree.
//!
//! # What a renderer needs, and where it is
//!
//! | Question | Answer |
//! |----------|--------|
//! | give it bytes | [`Vt::feed`] |
//! | what changed | [`Vt::sync`], returning [`SyncStats`] |
//! | where is the cursor | [`Vt::cursor`], returning [`CursorState`] |
//! | what modes are set | [`Vt::mode`], [`Vt::mouse_tracking`], [`Vt::cols`], [`Vt::rows`], [`Vt::scrollback_rows`] |
//! | what does the program want back | [`Vt::drain_pty_write`] |
//! | what did it announce | [`Vt::events`], returning [`Events`] |
//! | move the viewport | [`Vt::scroll`] |
//! | the window changed size | [`Vt::resize`] |
//!
//! [`Vt::sync`] is the damage contract. It reads only the rows the engine
//! reports as dirty and writes only the cells whose value differs, so an idle
//! terminal produces [`SyncStats::is_noop`] and costs the renderer no upload
//! and no frame. That is the property a live renderer is driven from: a frame
//! is presented because something changed, never because a clock ticked.
//!
//! # Threading
//!
//! A [`Vt`] is not [`Send`]. libghostty invokes its callbacks on the thread
//! that calls [`Vt::feed`], so a session belongs to the thread that created it.
//! One session per thread is the intended shape; sharing one across threads is
//! not offered rather than being offered and unsound.

mod bridge;
mod engine;
pub mod events;
pub mod linkage;
pub mod pwd;

pub use bridge::{CursorShape, CursorState, SyncStats};
pub use engine::{Vt, VtError, VtOptions};
pub use events::Events;
pub use linkage::linkage;
pub use pwd::pwd_path;

/// Where the viewport sits in the scrollback.
///
/// Re-exported from the engine rather than mirrored, because a second enum
/// with the same variants is a second thing to keep in step and buys nothing.
pub use libghostty_vt::terminal::ScrollViewport;

/// One terminal mode, and whether it is a DEC or an ANSI mode.
///
/// Re-exported for the same reason as [`ScrollViewport`]: the numbers are the
/// terminal's, not this crate's. A host reads [`Vt::mode`] with one of the
/// constants on [`Mode`] to find out how to encode input, because bracketed
/// paste, DECCKM and the six mouse protocols each turn the same key, paste or
/// click into different bytes.
pub use libghostty_vt::terminal::{Mode, ModeKind};

/// The colour depth this engine renders, as a child process reads it.
///
/// A host puts this in every session's environment as `COLORTERM`, and agents
/// read it to decide whether to emit 24-bit colour at all: Gemini CLI prints
/// "True color (24-bit) support not detected" and quantises itself to 256
/// colours when it is absent.
///
/// The claim belongs here and not to whoever sets the variable, because this
/// is the crate that either reproduces a colour or does not.
/// `the_engine_keeps_the_promise_this_crate_makes` feeds every channel value
/// through the engine and asserts the cell comes back exact, so weakening the
/// renderer and weakening this string fail together.
pub const COLORTERM: &str = "truecolor";

#[cfg(test)]
mod tests;
