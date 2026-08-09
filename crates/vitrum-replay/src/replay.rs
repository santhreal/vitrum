//! The seek API.
//!
//! [`Replay`] owns a [`Stream`], a [`Timeline`], and one live [`Emulator`].
//! [`Replay::seek`] moves that emulator to any seq the stream holds and hands
//! back the [`Screen`].
//!
//! # The two ways a seek can start
//!
//! 1. **The current position**, when the target is at or ahead of it. Nothing is
//!    rebuilt: the emulator is already correct at `at`, so the seek feeds
//!    `at..target` and stops. This is the case a user dragging the scrubber
//!    rightwards hits every frame, and it makes a full drag cost one linear pass
//!    over the region dragged across rather than one per frame.
//! 2. **The base of the stream**, when the target is behind the current position.
//!    A fresh emulator replays `base..target`.
//!
//! # Why a rewind is not cheaper than that
//!
//! It used to be. A [`Screen`] was the whole of terminal state and it was
//! `Clone`, so an index of snapshots every 256 KiB bounded a rewind to one
//! stride. Ghostty owns terminal state now, and libghostty's C API offers no
//! clone, no serialisation, and no readback for the scroll region, tab stops,
//! charset designations, saved cursor or inactive buffer.
//!
//! An index of live engines does not recover it, and the reason is worth stating
//! so nobody re-proposes it: using a parked engine means advancing it to the
//! target, which consumes it. Refilling its slot needs an engine at that seq,
//! whose only source is the slot below advanced one span, cascading down to a
//! fresh engine at the base. The refill therefore costs exactly what rebuilding
//! from the base would have cost for the same target. The engines are a wash, and
//! they are a wash while charging one terminal of memory each and half a pass per
//! checkpoint at build.
//!
//! Snapshotting the [`vitrum_grid::CellGrid`] instead does not recover it either.
//! A grid is a value at one seq, not a state machine: advancing one from the
//! snapshot to the target means applying VT sequences to a grid, which is the
//! hand-written translation this crate deleted in favour of Ghostty. A snapshot
//! answers the one seq it was taken at, and every seek between two snapshots is a
//! question it cannot answer.
//!
//! Rebuilding from the base is therefore optimal for a non-clonable engine, and
//! it is what this does.

use crate::config::ReplayConfig;
use crate::emulator::Emulator;
use crate::error::{Error, Result};
use crate::hints;
use crate::screen::Screen;
use crate::stream::Stream;
use crate::timeline::Timeline;

/// A seekable session.
#[derive(Debug)]
pub struct Replay<'a> {
    stream: Stream<'a>,
    config: ReplayConfig,
    timeline: Timeline,
    emulator: Emulator,
    at: u64,
    last_seek_bytes: u64,
}

impl<'a> Replay<'a> {
    /// Prepare `stream` and park at the start of it.
    ///
    /// One cheap pass collects OSC 7373 chapter markers (see [`crate::hints`]).
    /// No terminal bytes are parsed here: the emulator starts blank at the base of
    /// the stream, and the first [`Replay::seek`] is what feeds it. The timeline
    /// starts as [`Timeline::positional`], which is the honest state for a live
    /// session: seq scrubbing works, and there is no clock until someone supplies
    /// one through [`Replay::set_timeline`].
    ///
    /// # Errors
    ///
    /// Whatever [`ReplayConfig::validate`] rejects, and [`Error::Engine`] when
    /// Ghostty refuses to allocate a terminal.
    pub fn build(stream: Stream<'a>, config: &ReplayConfig) -> Result<Self> {
        config.validate()?;
        let timeline = Timeline::positional().with_markers(hints::scan(&stream));
        let emulator = Emulator::new(config.cols, config.rows, config.palette)?;
        Ok(Self {
            stream,
            config: *config,
            timeline,
            emulator,
            at: stream.base_seq(),
            last_seek_bytes: 0,
        })
    }

    /// Replace the timeline.
    ///
    /// This replaces it whole, markers included. To keep the markers the build
    /// found, pass them along:
    ///
    /// ```
    /// # use vitrum_replay::{ChunkStamp, Replay, ReplayConfig, Stream, Timeline};
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let bytes: &[u8] = b"hi";
    /// # let stream = Stream::new(0, std::slice::from_ref(&bytes));
    /// # let mut replay = Replay::build(stream, &ReplayConfig::new(10, 3)?)?;
    /// let markers = replay.timeline().markers().to_vec();
    /// let stamps = vec![ChunkStamp { end_seq: 2, micros: 1_000 }];
    /// replay.set_timeline(Timeline::recorded(stamps).with_markers(markers));
    /// assert!(replay.timeline().has_real_time());
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_timeline(&mut self, timeline: Timeline) {
        self.timeline = timeline;
    }

    /// Move to `seq` and return the screen as it stood there.
    ///
    /// `seq` counts bytes the session had written, so [`Stream::head_seq`] means
    /// "everything so far" and [`Stream::base_seq`] means "as far back as the ring
    /// still remembers".
    ///
    /// A `seq` that lands inside a multi-byte character or inside an escape
    /// sequence is legal and does the only correct thing: the incomplete sequence
    /// has not taken effect yet, so it is not shown. That is what the session's own
    /// terminal was showing at that instant too.
    ///
    /// # Errors
    ///
    /// [`Error::SeqOutOfRange`] when the stream does not hold `seq`, reporting the
    /// range it does hold, and [`Error::Engine`] when the engine cannot be read
    /// back.
    pub fn seek(&mut self, seq: u64) -> Result<&Screen> {
        if !self.stream.holds(seq) {
            return Err(Error::SeqOutOfRange {
                seq,
                oldest: self.stream.base_seq(),
                head: self.stream.head_seq(),
            });
        }

        if seq < self.at {
            self.emulator =
                Emulator::new(self.config.cols, self.config.rows, self.config.palette)?;
            self.at = self.stream.base_seq();
        }

        let from = self.at;
        for slice in self.stream.slices(from..seq) {
            self.emulator.feed_raw(slice);
        }
        self.emulator.project()?;
        self.last_seek_bytes = seq.saturating_sub(from);
        self.at = seq;
        Ok(self.emulator.screen())
    }

    /// Move to the position the timeline had reached at `micros`.
    ///
    /// Check [`Timeline::has_real_time`] first. On a timeline with no recorded
    /// stamps this always lands at the start of the stream, which is correct and
    /// useless: there is no clock to seek along. See [`crate::timeline`].
    ///
    /// # Errors
    ///
    /// [`Error::SeqOutOfRange`] only if the timeline names a seq outside the stream,
    /// which means the stamps and the bytes came from different sessions.
    pub fn seek_micros(&mut self, micros: u64) -> Result<&Screen> {
        let seq = self.timeline.seq_at(micros, self.stream.base_seq());
        self.seek(seq)
    }

    /// The screen at the current position.
    #[must_use]
    pub const fn screen(&self) -> &Screen {
        self.emulator.screen()
    }

    /// The current position.
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.at
    }

    /// Bytes the last [`Replay::seek`] had to feed.
    ///
    /// This is the seek's whole cost. Surfaced because it is the only way to tell
    /// a cheap forward drag from a rewind that had to restart from the base of the
    /// stream, and a UI that wants to stay responsive is choosing between exactly
    /// those two.
    #[must_use]
    pub const fn last_seek_bytes(&self) -> u64 {
        self.last_seek_bytes
    }

    /// The stream being replayed.
    #[must_use]
    pub const fn stream(&self) -> &Stream<'a> {
        &self.stream
    }

    /// The timeline.
    #[must_use]
    pub const fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// The configuration this replay was built with.
    #[must_use]
    pub const fn config(&self) -> &ReplayConfig {
        &self.config
    }

    /// Bytes this replay holds on the heap, not counting the borrowed stream.
    ///
    /// The stream is excluded because the daemon already owns those bytes; this is
    /// the cost of *adding* scrubbing to a session that already exists. The
    /// engine's own arena is not counted either, because libghostty neither
    /// reports it nor offers a way to measure it; it is one terminal, fixed, and
    /// it does not grow with the stream.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.timeline.heap_bytes() + self.screen().heap_bytes()
    }
}
