//! Off-by-default attribution of a frame's cost to the phases inside it.
//!
//! A frame time says a frame got slower. It does not say which part of the
//! frame got slower, and the parts have nothing in common: the escape-sequence
//! state machine, the projection onto the grid, the walk over damaged spans,
//! the buffer writes, and the command submission fail in different ways and
//! are fixed in different files. This module is how a measurement crosses that
//! boundary without a process boundary: the real render path records into it,
//! in process, and a harness reads the totals.
//!
//! # Cost
//!
//! Nothing here is compiled into a default build. The call sites in
//! [`crate::renderer`] and in `vitrum-vt` are behind the `probe` cargo
//! feature, which is off by default, so a shipped renderer contains no probe
//! instruction at all.
//!
//! With the feature on, an off probe is one relaxed load of [`enabled`] and a
//! not-taken branch per span. `vitrum-bench frame` measures that arm against a
//! build with the feature off rather than claiming it is free.
//!
//! # Scope
//!
//! Totals are per thread, because a session belongs to the thread that drives
//! it. A window with several panes on one thread accumulates all of them, so a
//! caller that wants one pane's frame reads [`take`] around that pane's frame
//! and nothing else.
//!
//! ```
//! use vitrum_grid::probe::{self, Phase};
//!
//! probe::set_enabled(true);
//! {
//!     let _span = probe::span(Phase::Parse);
//!     // the work being attributed
//! }
//! let frame = probe::take();
//! assert_eq!(frame.calls(Phase::Parse), 1);
//! probe::set_enabled(false);
//! ```

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// One attributable region of a frame.
///
/// The variants are the report's rows. [`Phase::ALL`] is what a reader
/// iterates, so a phase added here without a place to print it turns the
/// harness suite red instead of vanishing from the report.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Phase {
    /// The escape-sequence state machine consuming bytes.
    Parse,
    /// Projecting the engine's screen onto the grid, including the
    /// compare-and-record that turns a write into damage.
    Store,
    /// Walking the damaged spans and building the instance data for them,
    /// glyph lookups included.
    Damage,
    /// Writing that instance data into the GPU buffer.
    Upload,
    /// Recording the render pass and handing the command buffer to the queue.
    Submit,
}

impl Phase {
    /// Every phase, in the order a frame passes through them.
    pub const ALL: [Phase; 5] = [
        Phase::Parse,
        Phase::Store,
        Phase::Damage,
        Phase::Upload,
        Phase::Submit,
    ];

    /// The phase's name as a report prints it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Phase::Parse => "parse",
            Phase::Store => "store",
            Phase::Damage => "damage",
            Phase::Upload => "upload",
            Phase::Submit => "submit",
        }
    }

    /// Index into a per-phase array.
    #[must_use]
    const fn index(self) -> usize {
        match self {
            Phase::Parse => 0,
            Phase::Store => 1,
            Phase::Damage => 2,
            Phase::Upload => 3,
            Phase::Submit => 4,
        }
    }
}

/// What one thread accumulated between two calls to [`take`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Frame {
    nanos: [u64; Phase::ALL.len()],
    calls: [u32; Phase::ALL.len()],
}

impl Frame {
    /// Nanoseconds spent in `phase`.
    #[must_use]
    pub const fn nanos(&self, phase: Phase) -> u64 {
        self.nanos[phase.index()]
    }

    /// How many spans of `phase` closed.
    #[must_use]
    pub const fn calls(&self, phase: Phase) -> u32 {
        self.calls[phase.index()]
    }

    /// Nanoseconds across every phase.
    #[must_use]
    pub fn total_nanos(&self) -> u64 {
        self.nanos.iter().sum()
    }

    /// Whether anything was recorded at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.iter().all(|c| *c == 0)
    }
}

static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static TOTALS: RefCell<Frame> = const { RefCell::new(Frame {
        nanos: [0; Phase::ALL.len()],
        calls: [0; Phase::ALL.len()],
    }) };
}

/// Turn recording on or off for every thread.
///
/// The switch is process wide because it answers a process-wide question, and
/// a per-thread switch would leave a pane silent because it happened to be
/// driven from a thread nobody armed.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether recording is on.
#[inline(always)]
#[must_use]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Open a span for `phase`, or nothing at all when the probe is off.
///
/// The returned value records on drop, so a span is one `let` at the top of
/// the region being attributed. `None` costs a drop of nothing.
#[inline(always)]
#[must_use]
pub fn span(phase: Phase) -> Option<Span> {
    if enabled() {
        Some(Span {
            phase,
            start: Instant::now(),
        })
    } else {
        None
    }
}

/// An open region, closed when it drops.
#[derive(Debug)]
pub struct Span {
    phase: Phase,
    start: Instant,
}

impl Drop for Span {
    fn drop(&mut self) {
        let nanos = self.start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
        let index = self.phase.index();
        // A thread tearing down has already dropped its thread-local, and a
        // span that outlives it is a lost sample rather than a panic.
        let _ = TOTALS.try_with(|t| {
            if let Ok(mut t) = t.try_borrow_mut() {
                t.nanos[index] += nanos;
                t.calls[index] += 1;
            }
        });
    }
}

/// This thread's totals, reset to zero.
#[must_use]
pub fn take() -> Frame {
    TOTALS
        .try_with(|t| {
            let mut t = t.borrow_mut();
            core::mem::take(&mut *t)
        })
        .unwrap_or_default()
}

/// Discard this thread's totals without reading them.
pub fn reset() {
    let _ = take();
}
