//! Latency and throughput accounting.
//!
//! Percentiles come from every recorded sample rather than from a decaying
//! estimator. A load run is bounded and its samples fit in memory easily, and
//! the tail is the only interesting part of a latency distribution: an
//! estimator that smooths p99 hides exactly the stall worth finding.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Every latency sample for one kind of operation.
#[derive(Debug, Default)]
pub struct Latencies {
    micros: Vec<u64>,
}

impl Latencies {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, d: Duration) {
        // Microseconds, because a local round trip is tens to hundreds of them
        // and milliseconds would quantise most of the distribution to zero.
        self.micros.push(d.as_micros().min(u128::from(u64::MAX)) as u64);
    }

    pub fn len(&self) -> usize {
        self.micros.len()
    }

    pub fn is_empty(&self) -> bool {
        self.micros.is_empty()
    }

    /// Absorb another set of samples, for a per-task collector joining a total.
    pub fn merge(&mut self, other: &Self) {
        self.micros.extend_from_slice(&other.micros);
    }

    /// The distribution, or `None` when nothing was recorded.
    ///
    /// An empty run reports nothing rather than zeros: zeros read as "instant"
    /// and would be quoted as a result.
    pub fn summary(&self) -> Option<LatencySummary> {
        if self.micros.is_empty() {
            return None;
        }
        let mut s = self.micros.clone();
        s.sort_unstable();
        let pick = |q: f64| -> u64 {
            // Nearest-rank: the reported value is always one that was actually
            // measured, never an interpolation between two samples.
            let rank = (q * s.len() as f64).ceil().max(1.0) as usize;
            s[rank.min(s.len()) - 1]
        };
        let total: u128 = s.iter().map(|&v| u128::from(v)).sum();
        Some(LatencySummary {
            count: s.len(),
            min_us: s[0],
            p50_us: pick(0.50),
            p95_us: pick(0.95),
            p99_us: pick(0.99),
            max_us: s[s.len() - 1],
            mean_us: (total / s.len() as u128) as u64,
        })
    }
}

/// A reportable latency distribution, in microseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencySummary {
    pub count: usize,
    pub min_us: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
    pub mean_us: u64,
}

/// Bytes and messages moved over a wall-clock window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Throughput {
    pub bytes: u64,
    pub messages: u64,
    pub seconds: f64,
    pub bytes_per_sec: f64,
    pub messages_per_sec: f64,
}

impl Throughput {
    /// Rates over `elapsed`, which must be the measured window and not a target.
    pub fn new(bytes: u64, messages: u64, elapsed: Duration) -> Self {
        let seconds = elapsed.as_secs_f64();
        // A window of zero is a run that recorded nothing, and dividing by it
        // would report infinity as a throughput.
        let per = |n: u64| if seconds > 0.0 { n as f64 / seconds } else { 0.0 };
        Self {
            bytes,
            messages,
            seconds,
            bytes_per_sec: per(bytes),
            messages_per_sec: per(messages),
        }
    }
}

/// A measured distribution, in whatever unit the caller recorded.
///
/// Separate from [`LatencySummary`] because that type names its unit in every
/// field and a frame phase is measured in nanoseconds, not microseconds. One
/// percentile rule serves both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dist {
    pub count: usize,
    pub min: u64,
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
    pub mean: u64,
}

impl Dist {
    /// Nearest-rank percentiles, so every reported figure is a sample that was
    /// actually taken rather than an interpolation between two that were.
    ///
    /// # Errors
    ///
    /// An empty set has no distribution. Reporting zeros for one would publish
    /// "instant" for a measurement that never ran.
    pub fn of(mut samples: Vec<u64>) -> anyhow::Result<Self> {
        if samples.is_empty() {
            anyhow::bail!("no samples");
        }
        samples.sort_unstable();
        let pick = |q: f64| -> u64 {
            let rank = (q * samples.len() as f64).ceil().max(1.0) as usize;
            samples[rank.min(samples.len()) - 1]
        };
        let total: u128 = samples.iter().map(|&v| u128::from(v)).sum();
        Ok(Dist {
            count: samples.len(),
            min: samples[0],
            p50: pick(0.50),
            p95: pick(0.95),
            p99: pick(0.99),
            max: samples[samples.len() - 1],
            mean: (total / samples.len() as u128) as u64,
        })
    }
}
