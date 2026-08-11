//! When a frame is drawn, and what it cost.
//!
//! # The clock is the compositor's
//!
//! The pane does not run a timer. Output arriving from a session marks the
//! grid and returns; the frame is drawn from the toolkit's frame clock, which
//! fires once per compositor frame and is the only clock in the system that
//! knows when a frame can actually be shown. A fixed flush window is the
//! alternative and it is wrong in both directions at once: a 6 ms window spent
//! on a single keystroke is 6 ms of latency bought for nothing, and the same
//! window under a full-screen redraw batches two compositor frames into one
//! and drops the other.
//!
//! So there is no flush window here, no flush byte count, and no idle timer.
//! There is a mark, and a tick.
//!
//! # Nothing waits for the GPU
//!
//! The toolkit's main loop is the only thread that can service a keystroke, so
//! a frame must never make it wait. Two rules follow and both are enforced
//! here rather than by convention. A tick that arrives while an earlier frame
//! is still in flight is skipped rather than queued, because queueing is what
//! turns a slow frame into a growing backlog the operator experiences as lag
//! that never recovers. And the pane never polls the device to completion:
//! submitting is the end of the frame as far as this thread is concerned.
//!
//! # What is measured
//!
//! Every drawn frame's wall time, from the decision to draw to the end of the
//! submit. Not the GPU's execution time, which the pane cannot see without
//! waiting for it and which is not what the operator feels: what they feel is
//! how long the thread that reads their keystrokes was busy.

use std::time::Duration;

/// Frames kept for the percentiles.
///
/// Two thousand is about thirty seconds at 60 Hz and a quarter of that on a
/// 240 Hz panel. Long enough that a percentile means something, short enough
/// that a stall five minutes ago is not still in the p99 an operator is
/// reading off a diagnostics row. It is also the reporting cadence, so one
/// line covers exactly the window the numbers in it were taken from.
pub(crate) const WINDOW: usize = 2048;

/// What the pane should do with this tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tick {
    /// Draw a frame.
    Draw,
    /// Nothing changed. No encoder, no submit, no present.
    Idle,
    /// Something changed but an earlier frame has not been acknowledged.
    /// Skipped rather than queued.
    Backpressure,
}

/// Decides which ticks become frames.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Pacer {
    /// Something changed since the last frame was drawn.
    marked: bool,
    /// A frame has been submitted and not yet acknowledged.
    in_flight: bool,
    /// Ticks that found work but could not do it.
    skipped: u64,
    /// Ticks that found nothing to do.
    idle: u64,
    /// Frames drawn.
    drawn: u64,
}

impl Pacer {
    /// Record that something on screen would change.
    ///
    /// Called from the byte path, from a resize, from a theme change and from
    /// a cursor blink. Cheap on purpose: it is called once per socket read and
    /// must not be the thing that costs anything.
    pub(crate) const fn mark(&mut self) {
        self.marked = true;
    }

    /// Whether a frame is owed.
    pub(crate) const fn is_marked(&self) -> bool {
        self.marked
    }

    /// What to do with a tick from the frame clock.
    pub(crate) const fn tick(&mut self) -> Tick {
        if !self.marked {
            self.idle += 1;
            return Tick::Idle;
        }
        if self.in_flight {
            self.skipped += 1;
            return Tick::Backpressure;
        }
        self.marked = false;
        self.in_flight = true;
        self.drawn += 1;
        Tick::Draw
    }

    /// Record that the submitted frame reached the swapchain.
    pub(crate) const fn presented(&mut self) {
        self.in_flight = false;
    }

    /// Record that the frame could not be drawn after all, which is what a
    /// lost swapchain is.
    ///
    /// The mark is restored, not dropped: the change that asked for the frame
    /// has still not been shown, and dropping it leaves the pane holding a
    /// stale screen until something else happens to change.
    pub(crate) const fn failed(&mut self) {
        self.in_flight = false;
        self.marked = true;
    }

    /// Frames drawn, ticks skipped under backpressure, and ticks with nothing
    /// to do.
    pub(crate) const fn counts(&self) -> (u64, u64, u64) {
        (self.drawn, self.skipped, self.idle)
    }
}

/// A rolling window of frame times.
#[derive(Clone, Debug)]
pub(crate) struct FrameLog {
    samples: Vec<Duration>,
    next: usize,
    worst: Duration,
    total: u64,
}

impl Default for FrameLog {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameLog {
    /// An empty log.
    pub(crate) fn new() -> Self {
        Self {
            samples: Vec::with_capacity(WINDOW),
            next: 0,
            worst: Duration::ZERO,
            total: 0,
        }
    }

    /// Record one frame.
    pub(crate) fn record(&mut self, frame: Duration) {
        self.total += 1;
        if frame > self.worst {
            self.worst = frame;
        }
        if self.samples.len() < WINDOW {
            self.samples.push(frame);
        } else {
            self.samples[self.next] = frame;
            self.next = (self.next + 1) % WINDOW;
        }
    }

    /// Frames recorded since the log was created, including those the window
    /// has rolled past.
    pub(crate) const fn count(&self) -> u64 {
        self.total
    }

    /// The worst frame ever recorded, which is the number that decides whether
    /// a redraw was felt.
    ///
    /// Kept for the whole run rather than for the window, because a stall is
    /// interesting long after it has rolled out of a percentile.
    pub(crate) const fn worst(&self) -> Duration {
        self.worst
    }

    /// The `q`th percentile of the window, with `q` from 0.0 to 1.0.
    ///
    /// Nearest-rank: the smallest sample at or above `q` of the way through
    /// the sorted window, which is the definition that makes p99 of a hundred
    /// frames the 99th slowest rather than an interpolation between two frames
    /// neither of which happened. A frame time is an observation, not a
    /// distribution to fit.
    ///
    /// Computed on a copy. Sorting the live buffer would reorder the ring and
    /// make the next eviction throw away a sample that is not the oldest.
    pub(crate) fn percentile(&self, q: f64) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let n = sorted.len();
        let rank = (q.clamp(0.0, 1.0) * n as f64).ceil() as usize;
        sorted[rank.saturating_sub(1).min(n - 1)]
    }

    /// p50, p95, p99 and the worst frame, in that order.
    pub(crate) fn summary(&self) -> (Duration, Duration, Duration, Duration) {
        (
            self.percentile(0.50),
            self.percentile(0.95),
            self.percentile(0.99),
            self.worst(),
        )
    }
}

/// What the frame clock has been doing, read from the pacer and the log at
/// one instant.
///
/// Both halves in one value because neither answers the question on its own:
/// a p99 says how long a frame took and the counts say how many frames were
/// wanted and never drawn, and a pane that skips half its ticks has a good
/// p99 and a bad picture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FrameStats {
    /// Frames drawn.
    pub drawn: u64,
    /// Ticks that found work and could not do it.
    pub skipped: u64,
    /// Ticks with nothing to do.
    pub idle: u64,
    /// Frames whose time was recorded.
    pub recorded: u64,
    /// Median frame time.
    pub p50: Duration,
    /// 95th percentile of the window.
    pub p95: Duration,
    /// 99th percentile of the window.
    pub p99: Duration,
    /// The worst frame of the whole run.
    pub worst: Duration,
    /// Whether a frame was already owed again when this was read. A window
    /// sampled while the pane is behind is a window whose percentiles were
    /// measured under a backlog.
    pub behind: bool,
}

impl FrameStats {
    /// Read the pacer and the log together.
    pub(crate) fn of(pacer: &Pacer, log: &FrameLog) -> Self {
        let (drawn, skipped, idle) = pacer.counts();
        let (p50, p95, p99, worst) = log.summary();
        Self {
            drawn,
            skipped,
            idle,
            recorded: log.count(),
            p50,
            p95,
            p99,
            worst,
            behind: pacer.is_marked(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WHY: an idle pane must cost nothing. Twenty sessions in a window, none
    /// of them printing, is the state this product spends most of its time in,
    /// and a tick that draws anyway is twenty pointless frames per vblank.
    #[test]
    fn a_tick_with_nothing_to_show_draws_nothing() {
        let mut p = Pacer::default();
        for _ in 0..1_000 {
            assert_eq!(p.tick(), Tick::Idle);
        }
        assert_eq!(p.counts(), (0, 0, 1_000));
    }

    /// WHY: a change must reach the screen on the next tick and not the one
    /// after. A mark that survives its own frame draws twice; a mark that is
    /// cleared before the frame draws never shows the change at all.
    #[test]
    fn one_change_produces_exactly_one_frame() {
        let mut p = Pacer::default();
        p.mark();
        assert_eq!(p.tick(), Tick::Draw);
        p.presented();
        assert_eq!(p.tick(), Tick::Idle, "the mark survived its own frame");

        // Many marks between two ticks are still one frame: that is the
        // coalescing a flush window used to buy, without the latency.
        for _ in 0..10_000 {
            p.mark();
        }
        assert_eq!(p.tick(), Tick::Draw);
        p.presented();
        assert_eq!(p.tick(), Tick::Idle);
        assert_eq!(p.counts().0, 2, "ten thousand marks drew more than twice");
    }

    /// WHY: queueing a frame behind one that has not finished is how a slow
    /// frame becomes a backlog. The operator experiences that as lag that
    /// never recovers, because every subsequent frame inherits the queue.
    ///
    /// The invariant is a bound: however many ticks arrive while a frame is in
    /// flight, exactly one frame is drawn when it completes, and no work is
    /// lost.
    #[test]
    fn ticks_during_a_frame_are_skipped_rather_than_queued() {
        let mut p = Pacer::default();
        p.mark();
        assert_eq!(p.tick(), Tick::Draw);

        for _ in 0..500 {
            p.mark();
            assert_eq!(p.tick(), Tick::Backpressure);
        }
        assert_eq!(p.counts(), (1, 500, 0));

        p.presented();
        assert_eq!(p.tick(), Tick::Draw, "the change was lost");
        p.presented();
        assert_eq!(p.tick(), Tick::Idle);
        assert_eq!(p.counts().0, 2, "the backlog drew more than one catch-up frame");
    }

    /// WHY: a lost swapchain is normal, not exceptional. It happens on every
    /// resize and on every monitor change. Dropping the mark leaves the pane
    /// showing a stale screen until the child happens to print something,
    /// which for an agent waiting on approval is forever.
    #[test]
    fn a_frame_that_failed_is_owed_again() {
        let mut p = Pacer::default();
        p.mark();
        assert_eq!(p.tick(), Tick::Draw);
        p.failed();

        assert!(p.is_marked(), "the change was forgotten when the frame failed");
        assert_eq!(p.tick(), Tick::Draw);
        p.presented();
        assert_eq!(p.tick(), Tick::Idle);
    }

    /// WHY: percentiles read off a live ring must not reorder it, or the next
    /// eviction throws away a sample that is not the oldest and the window
    /// stops being a window.
    #[test]
    fn reading_the_percentiles_does_not_disturb_the_window() {
        let mut log = FrameLog::new();
        for ms in 1..=100u64 {
            log.record(Duration::from_millis(ms));
        }
        let first = log.summary();
        for _ in 0..10 {
            assert_eq!(log.summary(), first);
        }

        assert_eq!(log.percentile(0.50), Duration::from_millis(50));
        assert_eq!(log.percentile(0.99), Duration::from_millis(99));
        assert_eq!(log.percentile(1.0), Duration::from_millis(100));
        assert_eq!(log.percentile(0.0), Duration::from_millis(1));
        assert_eq!(log.worst(), Duration::from_millis(100));
    }

    /// WHY: the worst frame is the number that says whether a redraw was felt,
    /// and it is interesting long after it has rolled out of the window. A
    /// worst that decays with the ring reports a smooth run that was not.
    #[test]
    fn the_worst_frame_survives_the_window_rolling_past_it() {
        let mut log = FrameLog::new();
        log.record(Duration::from_millis(500));
        for _ in 0..WINDOW * 2 {
            log.record(Duration::from_micros(300));
        }

        assert_eq!(log.worst(), Duration::from_millis(500));
        assert_eq!(log.percentile(0.99), Duration::from_micros(300));
        assert_eq!(log.count(), (WINDOW * 2 + 1) as u64);
    }

    /// WHY: an empty log is read by the diagnostics row before a single frame
    /// has been drawn, and a percentile over nothing must be a zero rather
    /// than a panic on an empty slice.
    #[test]
    fn an_empty_log_reports_zero_rather_than_panicking() {
        let log = FrameLog::new();
        assert_eq!(log.count(), 0);
        assert_eq!(log.worst(), Duration::ZERO);
        for q in [0.0, 0.5, 0.95, 0.99, 1.0, -1.0, 2.0] {
            assert_eq!(log.percentile(q), Duration::ZERO, "q={q}");
        }
        assert_eq!(
            log.summary(),
            (
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO
            )
        );
    }
}
