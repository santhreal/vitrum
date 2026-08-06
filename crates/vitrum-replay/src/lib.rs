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
//! # Why seeking is cheap
//!
//! Feeding the stream from byte zero on every seek is O(n) per seek, and a
//! 10 MiB ring makes that a visible stall on a scrubber the user is dragging.
//! [`KeyframeIndex`] therefore snapshots the whole screen every `stride` bytes
//! during one linear build pass. A seek restores the newest keyframe at or
//! before the target and feeds at most `stride` bytes from there.
//!
//! A keyframe may only be taken where the VT parser is provably back in its
//! ground state, because a snapshot taken halfway through an escape sequence
//! could not be resumed from. [`Emulator::feed_byte`] reports exactly that, and
//! [`KeyframeIndex::build`] uses it to slide each keyframe forward to the next
//! safe boundary. See [`mod@keyframe`].
//!
//! Forward scrubbing does not rewind at all: a seek to a seq at or after the
//! current position just feeds the bytes in between, so dragging the scrubber
//! rightwards costs one linear pass over the region dragged across, not one per
//! frame.
//!
//! # What this does not do
//!
//! It does not emulate a terminal from scratch. The cell grid is
//! [`vitrum_grid::CellGrid`], already tested in its own crate, and the byte
//! level state machine is [`vte`], the parser Alacritty uses. What lives here is
//! the layer between them: cursor, scroll region, modes, tab stops, charsets,
//! and the mapping from VT commands onto grid operations. See [`mod@perform`]
//! for the exact command set and what is deliberately ignored.
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
//! - [`mod@screen`]: the reconstructed terminal state.
//! - [`mod@perform`]: the VT command set, mapped onto the grid.
//! - [`mod@emulator`]: parser plus screen, and the ground state probe.
//! - [`mod@keyframe`]: periodic snapshots, and why seeking is cheap.
//! - [`mod@timeline`]: seq to time, and where the times come from.
//! - [`mod@hints`]: OSC 7373 chapter markers over the same stream.
//! - [`mod@replay`]: the seek API.
//! - [`asciicast`]: v2 import and export, byte exact.
//! - [`mod@palette`]: indexed colour to RGB.
//! - [`mod@error`]: how this crate fails.

#![deny(missing_docs)]

pub mod asciicast;
pub mod binary;
pub mod config;
pub mod emulator;
pub mod error;
pub mod hints;
pub mod keyframe;
pub mod palette;
pub mod perform;
pub mod replay;
pub mod screen;
pub mod stream;
pub mod timeline;

#[cfg(test)]
mod tests;

pub use config::{DEFAULT_GROUND_SCAN, DEFAULT_KEYFRAME_STRIDE, ReplayConfig};
pub use binary::{VbrChunk, VbrHeader, VbrIndexEntry, VbrView, VbrWriter};
pub use emulator::Emulator;
pub use error::{CastError, Error, Result};
pub use keyframe::{Keyframe, KeyframeIndex};
pub use palette::Palette;
pub use replay::Replay;
pub use screen::{Charset, Charsets, Cursor, Modes, SavedCursor, Screen, ScrollRegion, TabStops};
pub use stream::{Slices, Stream};
pub use timeline::{ChunkStamp, Marker, Timeline};
