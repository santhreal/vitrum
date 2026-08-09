//! Session replay and scrubbing over a scrollback ring.
//!
//! A vitrum session's scrollback is already a timeline. Every chunk the daemon
//! reads from the PTY is numbered by `seq`, the cumulative byte offset of that
//! chunk in the session's whole output stream, and those numbers never restart
//! and never renumber when the ring evicts. So "what did the screen look like
//! 40 KiB ago" is a question the daemon can already answer exactly, and nobody
//! had to record anything to make it answerable. This crate turns that property
//! into a scrubber.
//!
//! Nothing here talks to the daemon, a socket, or a UI. The whole input is a
//! byte stream plus the seq its first byte carries.
//!
//! ```
//! use vitrum_replay::{Replay, ReplayConfig, Stream};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bytes: &[u8] = b"one\r\ntwo\r\nthree";
//! let stream = Stream::new(0, std::slice::from_ref(&bytes));
//! let mut replay = Replay::build(stream, &ReplayConfig::new(10, 3)?)?;
//!
//! // The end of the stream: all three lines.
//! replay.seek(stream.head_seq())?;
//! assert_eq!(replay.screen().line(2).trim_end(), "three");
//!
//! // Back to just after "one\r\n". Row 1 has not been written yet.
//! replay.seek(5)?;
//! assert_eq!(replay.screen().line(0).trim_end(), "one");
//! assert_eq!(replay.screen().line(1).trim_end(), "");
//! # Ok(())
//! # }
//! ```
//!
//! # The four things this does
//!
//! **A timeline index.** [`Timeline`] maps seq to time and back. See
//! [`mod@timeline`] for where the times come from, which is the one honest
//! caveat in this crate: the ring stores bytes, not clocks.
//!
//! **State reconstruction at an arbitrary seq.** [`Replay::seek`] produces the
//! [`Screen`] as it stood when the session had written exactly `seq` bytes.
//!
//! **Export to asciicast v2.** [`asciicast::write`] emits the de facto standard
//! recording format, so a session can be shared, embedded, or played by
//! anything.
//!
//! **Import of the same format.** [`asciicast::read`] loads a recording into the
//! same [`Stream`] plus [`Timeline`] pair the live path uses, so an imported
//! recording scrubs through exactly the same code.
//!
//! # Why forward scrubbing is cheap and rewinding is not
//!
//! A seek at or ahead of the current position feeds only the bytes in between,
//! so dragging the scrubber rightwards costs one linear pass over the region
//! dragged across, not one per frame.
//!
//! A rewind replays from the base of the stream. It used to restore a snapshot
//! taken every 256 KiB, which bounded it; that index is gone because Ghostty's
//! terminal state cannot be cloned, serialised or read back, and neither an
//! index of live engines nor an index of cell grids recovers the bound. See
//! [`mod@replay`] for the argument in full, so nobody re-proposes either one.
//!
//! # What this does not do
//!
//! It does not emulate a terminal. Ghostty does, through
//! [`vitrum_vt`], which is the same engine the daemon runs against the live
//! session, so a replayed screen and the screen the user watched are produced by
//! one parser rather than by two that agree until they do not. The cell grid is
//! [`vitrum_grid::CellGrid`]. What lives here is the stream, the timeline, the
//! seek, and the asciicast codec.
//!
//! It does not keep scrollback of its own. A [`Screen`] is the screen: rows that
//! scrolled off the top before the seek target are gone, exactly as they are
//! gone in a real terminal that has no scrollback. The session's scrollback is
//! the byte stream this crate reads from, so nothing is actually lost, but
//! [`Screen`] is `rows` tall and no taller.
//!
//! # Module map
//!
//! - [`mod@stream`]: the byte stream and its seq coordinate space.
//! - [`mod@screen`]: the projected terminal state.
//! - [`mod@emulator`]: the engine driving a screen.
//! - [`mod@timeline`]: seq to time, and where the times come from.
//! - [`mod@hints`]: OSC 7373 chapter markers over the same stream.
//! - [`mod@replay`]: the seek API.
//! - [`asciicast`]: v2 import and export, byte exact.
//! - [`mod@palette`]: the default foreground and background.
//! - [`mod@error`]: how this crate fails.

#![deny(missing_docs)]

pub mod asciicast;
pub mod config;
pub mod emulator;
pub mod error;
pub mod hints;
pub mod palette;
pub mod replay;
pub mod screen;
pub mod stream;
pub mod timeline;

#[cfg(test)]
mod tests;

pub use config::ReplayConfig;
pub use emulator::Emulator;
pub use error::{CastError, Error, Result};
pub use palette::Palette;
pub use replay::Replay;
pub use screen::{Cursor, Screen};
pub use stream::{Slices, Stream};
pub use timeline::{ChunkStamp, Marker, Timeline};
