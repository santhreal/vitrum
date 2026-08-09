//! Frame-time and throughput accounting shared by both paths.
//!
//! A distribution, not a mean. The question this lab answers is whether the
//! pane stalls, and a stall lives in the tail: an average hides a 40 ms frame
//! behind two hundred 0.2 ms ones.

use std::time::Instant;

/// Samples for one run.
#[derive(Debug)]
pub struct Run {
    label: String,
    /// Microseconds per frame, in arrival order.
    frames: Vec<u64>,
    bytes: u64,
    started: Instant,
    first_byte: Option<Instant>,
    last_byte: Option<Instant>,
}

impl Run {
    /// Begin a run named `label`.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            frames: Vec::with_capacity(4096),
            bytes: 0,
            started: Instant::now(),
            first_byte: None,
            last_byte: None,
        }
    }

    /// Record one frame: `bytes` arrived and took `micros` to become pixels.
    pub fn frame(&mut self, bytes: usize, micros: u64) {
        let now = Instant::now();
        if self.first_byte.is_none() {
            self.first_byte = Some(now);
        }
        self.last_byte = Some(now);
        self.bytes += bytes as u64;
        self.frames.push(micros);
    }

    /// The run as JSON, one object, ready to be pasted into a table.
    #[must_use]
    pub fn report(&self) -> serde_json::Value {
        let mut sorted = self.frames.clone();
        sorted.sort_unstable();
        let pick = |q: f64| -> f64 {
            if sorted.is_empty() {
                return 0.0;
            }
            let idx = ((sorted.len() - 1) as f64 * q).round() as usize;
            sorted[idx] as f64 / 1000.0
        };
        // Wall time from the first byte to the last, which is what a
        // throughput figure must divide by. Process startup and font
        // discovery are not part of the terminal's rate.
        let span = match (self.first_byte, self.last_byte) {
            (Some(a), Some(b)) => (b - a).as_secs_f64(),
            _ => 0.0,
        };
        let sum: u64 = sorted.iter().sum();
        serde_json::json!({
            "label": self.label,
            "frames": sorted.len(),
            "bytes": self.bytes,
            "stream_seconds": span,
            "bytes_per_second": if span > 0.0 { self.bytes as f64 / span } else { 0.0 },
            "frame_ms_p50": pick(0.50),
            "frame_ms_p95": pick(0.95),
            "frame_ms_p99": pick(0.99),
            "frame_ms_max": pick(1.0),
            "frame_ms_mean": if sorted.is_empty() { 0.0 } else { sum as f64 / sorted.len() as f64 / 1000.0 },
            "run_seconds": self.started.elapsed().as_secs_f64(),
        })
    }
}
