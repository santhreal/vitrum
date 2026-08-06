//! Seq to time and back, and where the times come from.
//!
//! # Where the times come from
//!
//! They do not come from the ring. This is the one honest caveat in the crate and
//! it is worth being blunt about it: vitrum-core's `Scrollback` stores bytes and a
//! byte count. It stores no clock. Nothing in the retained bytes says when they
//! arrived, and nothing can be inferred from them, because a program can print
//! 4 KiB in a microsecond or over an hour and the bytes are identical.
//!
//! So there are exactly three sources, and a [`Timeline`] says which one it has:
//!
//! **Recorded stamps.** [`Timeline::recorded`] takes one [`ChunkStamp`] per PTY
//! read: the seq just past that chunk, and when the daemon read it.
//! [`Timeline::has_real_time`] is then true. **The daemon does not record these
//! today.** What it would take is spelled out in [`ChunkStamp`], and it is a
//! parallel ring of 16-byte entries, not a redesign.
//!
//! **An imported recording.** [`crate::asciicast::read`] produces recorded stamps,
//! because an asciicast file's whole point is that it carries the times. An
//! imported recording therefore scrubs by wall clock with no daemon change at all.
//!
//! **Nothing.** [`Timeline::positional`] is the honest answer for a live session on
//! today's daemon: it maps every seq to time zero, [`Timeline::has_real_time`] is
//! false, and a UI that checks that flag shows a byte-position scrubber instead of
//! inventing a clock. This matters. A scrubber that says "3.2s" when it is really
//! showing "40% of the way through the bytes" is worse than one that says "40%",
//! because the user believes the first one.
//!
//! # Scrubbing by seq needs none of this
//!
//! Seq scrubbing works today, on the daemon as it stands, with nothing added. A
//! session's bytes are already numbered, so "show me the screen 200 KiB ago" is
//! already answerable. Time is a *label* on that axis, and it is the label that
//! needs the daemon's help, not the axis.
//!
//! # No interpolation inside a chunk
//!
//! [`Timeline::micros_at`] returns the stamp of the chunk containing the seq, not a
//! value interpolated across it. Every byte in one PTY read arrived at the same
//! moment as far as anyone outside the kernel can tell, and interpolating would
//! invent sub-chunk timing that never existed. A 4 KiB chunk therefore has one
//! time, which is the truth.


/// When one chunk of output arrived.
///
/// # What the daemon would have to add
///
/// The daemon reads a chunk from the PTY, gives it a seq, and pushes it into
/// vitrum-core's `Scrollback`. To make time scrubbing real it would additionally
/// push `ChunkStamp { end_seq, micros }` into a bounded ring alongside, evicting
/// stamps whose `end_seq` has fallen below the byte ring's oldest seq.
///
/// The cost is small and bounded. At a typical 4 KiB read, 16 bytes of stamp per
/// 4 KiB of output is 0.4% overhead, so a 10 MiB ring carries about 40 KiB of
/// stamps. `micros` is measured from session start rather than from the epoch,
/// which keeps it monotonic across a clock change and fits a 584 000 year session
/// in a `u64`.
///
/// Nothing else changes: no protocol break, no new message, no per-byte cost. The
/// stamps ride out with the existing scrollback range reply as a second array.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChunkStamp {
    /// Seq one past the last byte of this chunk.
    ///
    /// The end rather than the start, so a lookup for seq `n` is "the first stamp
    /// whose `end_seq` is greater than `n`" and needs no length field.
    pub end_seq: u64,
    /// Microseconds from the start of the session.
    pub micros: u64,
}

/// A point of interest on the timeline.
///
/// Markers are what turn a scrubber into a table of contents. vitrum's own OSC 7373
/// hint channel already tells the daemon when an agent started working, asked for
/// approval, or finished, so [`crate::hints::scan`] can put a marker at the exact
/// byte where each of those happened and a user can jump to "the moment it asked to
/// force push" instead of dragging.
///
/// asciicast v2 has a marker event type (`"m"`), so these survive an export and come
/// back on import.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Marker {
    /// Stream position the marker sits at.
    pub seq: u64,
    /// Operator-facing text.
    pub label: String,
    /// The agent state this marker came from, when it came from OSC 7373.
    ///
    /// `None` for a marker imported from an asciicast file, which carries a label
    /// and no structure.
    pub hint: Option<vitrum_proto::HintState>,
}

/// Seq to time, time to seq, and the markers in between.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Timeline {
    /// Sorted by `end_seq`, strictly increasing.
    stamps: Vec<ChunkStamp>,
    real: bool,
    markers: Vec<Marker>,
}

impl Timeline {
    /// A timeline over real per-chunk delivery times.
    ///
    /// `stamps` must be sorted by `end_seq`. Duplicate or out-of-order entries are
    /// dropped rather than rejected, because a stamp ring that lost an entry to
    /// eviction is a normal condition and refusing to build a timeline over it
    /// would take away scrubbing for the rest of the session.
    #[must_use]
    pub fn recorded(stamps: Vec<ChunkStamp>) -> Self {
        let mut kept: Vec<ChunkStamp> = Vec::with_capacity(stamps.len());
        for stamp in stamps {
            match kept.last() {
                Some(last) if stamp.end_seq <= last.end_seq => continue,
                Some(last) if stamp.micros < last.micros => continue,
                _ => kept.push(stamp),
            }
        }
        Self {
            stamps: kept,
            real: true,
            markers: Vec::new(),
        }
    }

    /// A timeline with no times at all, for a stream whose delivery times were
    /// never recorded.
    ///
    /// [`Timeline::has_real_time`] is false. Seq scrubbing over such a stream is
    /// fully correct; only the clock is missing.
    #[must_use]
    pub const fn positional() -> Self {
        Self {
            stamps: Vec::new(),
            real: false,
            markers: Vec::new(),
        }
    }

    /// A timeline that spreads `total_micros` evenly across `base_seq..head_seq`.
    ///
    /// [`Timeline::has_real_time`] is false, because this is a made-up clock. It
    /// exists for one honest purpose: exporting a session with no recorded stamps
    /// to asciicast, where the format requires a time on every event. An importer
    /// then plays the recording back at a plausible pace instead of dumping the
    /// whole session in one frame.
    ///
    /// `steps` is how many events to spread across, and is clamped to at least one.
    #[must_use]
    pub fn synthetic(base_seq: u64, head_seq: u64, total_micros: u64, steps: usize) -> Self {
        let steps = steps.max(1) as u64;
        let span = head_seq.saturating_sub(base_seq);
        let mut stamps = Vec::with_capacity(steps as usize);
        for step in 1..=steps {
            stamps.push(ChunkStamp {
                end_seq: base_seq + span * step / steps,
                micros: total_micros * step / steps,
            });
        }
        Self {
            stamps,
            real: false,
            markers: Vec::new(),
        }
    }

    /// Attach markers, replacing any already present.
    #[must_use]
    pub fn with_markers(mut self, markers: Vec<Marker>) -> Self {
        self.markers = markers;
        self.markers.sort_by_key(|marker| marker.seq);
        self
    }

    /// Are these real delivery times, or a placeholder?
    ///
    /// A UI must check this before showing a clock. See the module header.
    #[must_use]
    pub const fn has_real_time(&self) -> bool {
        self.real
    }

    /// The per-chunk stamps, in stream order.
    #[must_use]
    pub fn stamps(&self) -> &[ChunkStamp] {
        &self.stamps
    }

    /// The markers, in stream order.
    #[must_use]
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// Total recorded span in microseconds, or zero when there are no stamps.
    #[must_use]
    pub fn duration_micros(&self) -> u64 {
        self.stamps.last().map_or(0, |stamp| stamp.micros)
    }

    /// When the byte at `seq` arrived.
    ///
    /// `None` past the last stamp, which is how a caller tells "the timeline does
    /// not reach here" from "it arrived at time zero".
    #[must_use]
    pub fn micros_at(&self, seq: u64) -> Option<u64> {
        let index = self.stamps.partition_point(|stamp| stamp.end_seq <= seq);
        self.stamps.get(index).map(|stamp| stamp.micros)
    }

    /// The seq the timeline had reached by `micros`.
    ///
    /// This is the end of the last chunk delivered at or before `micros`, so
    /// seeking there shows exactly what the screen showed at that moment and not a
    /// half-delivered chunk. Before the first stamp it is the start of the stream,
    /// which needs the stream's own base seq and is why that is a parameter.
    #[must_use]
    pub fn seq_at(&self, micros: u64, base_seq: u64) -> u64 {
        let index = self.stamps.partition_point(|stamp| stamp.micros <= micros);
        if index == 0 {
            base_seq
        } else {
            self.stamps[index - 1].end_seq
        }
    }

    /// The marker at or before `seq`, for a "which chapter am I in" readout.
    #[must_use]
    pub fn marker_at_or_before(&self, seq: u64) -> Option<&Marker> {
        let index = self.markers.partition_point(|marker| marker.seq <= seq);
        if index == 0 {
            None
        } else {
            self.markers.get(index - 1)
        }
    }

    /// The first marker strictly after `seq`, for a "jump to next event" control.
    #[must_use]
    pub fn marker_after(&self, seq: u64) -> Option<&Marker> {
        let index = self.markers.partition_point(|marker| marker.seq <= seq);
        self.markers.get(index)
    }

    /// Bytes this timeline holds on the heap.
    #[must_use]
    pub fn heap_bytes(&self) -> usize {
        self.stamps.capacity() * core::mem::size_of::<ChunkStamp>()
            + self.markers.capacity() * core::mem::size_of::<Marker>()
            + self
                .markers
                .iter()
                .map(|marker| marker.label.capacity())
                .sum::<usize>()
    }

    /// Push one stamp, and report whether it was kept.
    ///
    /// For a caller following a live session: each new chunk extends the timeline
    /// by one entry. `false` means the stamp went backwards in seq or in time and
    /// was dropped to keep the two binary searches sound, for the same reason
    /// [`Timeline::recorded`] drops such entries.
    pub fn push(&mut self, stamp: ChunkStamp) -> bool {
        match self.stamps.last() {
            Some(last) if stamp.end_seq <= last.end_seq || stamp.micros < last.micros => false,
            _ => {
                self.stamps.push(stamp);
                true
            }
        }
    }
}

impl Default for Timeline {
    fn default() -> Self {
        Self::positional()
    }
}
