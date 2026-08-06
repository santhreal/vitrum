//! The seek API.
//!
//! [`Replay`] owns a [`Stream`], the [`KeyframeIndex`] over it, a [`Timeline`], and
//! one live [`Emulator`]. [`Replay::seek`] moves that emulator to any seq the stream
//! holds and hands back the [`Screen`].
//!
//! # The three ways a seek can start
//!
//! A seek picks the cheapest sound starting point, which is one of:
//!
//! 1. **The current position**, when the target is ahead of it. Nothing is
//!    restored: the emulator is already correct at `at`, so the seek feeds
//!    `at..target` and stops. This is the case a user dragging the scrubber
//!    rightwards hits every frame, and it makes a full drag cost one linear pass
//!    over the region dragged across rather than one per frame.
//! 2. **A keyframe**, when one sits between the current position and the target,
//!    or when the target is behind the current position. See [`crate::keyframe`].
//! 3. **The start of the stream**, when the target is behind the first keyframe.
//!
//! In every case the emulator then feeds a contiguous byte range and nothing else
//! happens, so a seek is a bounded amount of parsing and one screen clone.

use crate::config::ReplayConfig;
use crate::emulator::Emulator;
use crate::error::{Error, Result};
use crate::hints;
use crate::keyframe::KeyframeIndex;
use crate::binary::VbrWriter;
use crate::screen::Screen;
use crate::stream::Stream;
use crate::timeline::Timeline;

/// A seekable session.
#[derive(Debug)]
pub struct Replay<'a> {
    stream: Stream<'a>,
    config: ReplayConfig,
    index: KeyframeIndex,
    timeline: Timeline,
    emulator: Emulator,
    at: u64,
    last_seek_bytes: u64,
}

impl<'a> Replay<'a> {
    /// Index `stream` and park at the start of it.
    ///
    /// One linear pass builds the keyframes, and a second, much cheaper pass
    /// collects OSC 7373 chapter markers (see [`crate::hints`]). The timeline starts
    /// as [`Timeline::positional`], which is the honest state for a live session:
    /// seq scrubbing works, and there is no clock until someone supplies one through
    /// [`Replay::set_timeline`].
    ///
    /// # Errors
    ///
    /// Whatever [`ReplayConfig::validate`] rejects.
    pub fn build(stream: Stream<'a>, config: &ReplayConfig) -> Result<Self> {
        let index = KeyframeIndex::build(&stream, config)?;
        let timeline = Timeline::positional().with_markers(hints::scan(&stream));
        let emulator = Emulator::new(config.cols, config.rows, config.palette)?;
        Ok(Self {
            stream,
            config: *config,
            index,
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
    /// range it does hold.
    pub fn seek(&mut self, seq: u64) -> Result<&Screen> {
        if !self.stream.holds(seq) {
            return Err(Error::SeqOutOfRange {
                seq,
                oldest: self.stream.base_seq(),
                head: self.stream.head_seq(),
            });
        }

        let keyframe_seq = self.index.latest_at_or_before(seq).map(|frame| frame.seq);
        let rewinding = seq < self.at;
        let keyframe_is_closer = keyframe_seq.is_some_and(|at| at > self.at);

        if rewinding || keyframe_is_closer {
            match self.index.latest_at_or_before(seq) {
                Some(frame) => {
                    self.emulator = Emulator::resume(frame.screen().clone());
                    self.at = frame.seq;
                }
                None => {
                    self.emulator =
                        Emulator::new(self.config.cols, self.config.rows, self.config.palette)?;
                    self.at = self.stream.base_seq();
                }
            }
        }

        let from = self.at;
        for slice in self.stream.slices(from..seq) {
            self.emulator.feed(slice);
        }
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
    /// This is the seek's whole cost: everything else it does is one screen clone.
    /// Surfaced because a scrubber choosing a stride is choosing this number, and
    /// because it is the only way to tell a cheap forward drag from a rewind that
    /// had to restart from a keyframe.
    #[must_use]
    pub const fn last_seek_bytes(&self) -> u64 {
        self.last_seek_bytes
    }

    /// The stream being replayed.
    #[must_use]
    pub const fn stream(&self) -> &Stream<'a> {
        &self.stream
    }

    /// The keyframe index.
    #[must_use]
    pub const fn index(&self) -> &KeyframeIndex {
        &self.index
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
    /// the cost of *adding* scrubbing to a session that already exists.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.index.heap_bytes() + self.timeline.heap_bytes() + self.screen().heap_bytes()
    }

    /// Export the session stream and keyframe index to zero-copy binary `.vbr` format.
    #[must_use]
    pub fn export_vbr(&self) -> Vec<u8> {
        let mut writer = VbrWriter::new(self.config.cols, self.config.rows);        let mut current_seq = self.stream.base_seq();
        for chunk in self.stream.chunks() {
            if chunk.is_empty() {
                continue;
            }
            let end_seq = current_seq + chunk.len() as u64;
            let micros = self.timeline.micros_at(end_seq).unwrap_or(0);
            writer.add_chunk(current_seq, micros, chunk);
            current_seq = end_seq;
        }
        writer.import_keyframe_index(&self.index);
        writer.serialize()
    }
}
