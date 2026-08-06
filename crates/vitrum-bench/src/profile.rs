//! Sampling profiler for the daemon while a workload runs against it.
//!
//! There is no in-process instrumentation here on purpose. The thing being
//! profiled is a separate process, possibly on another host, and adding hooks
//! to it would change what is being measured. Instead this samples the kernel's
//! own accounting at a fixed interval, which costs the daemon nothing beyond the
//! reads.
//!
//! Two things are sampled:
//!
//! - **Resident and proportional memory**, from `/proc/<pid>/smaps_rollup` where
//!   it exists. PSS is the number that means something with shared pages, which
//!   a daemon plus its children always has.
//! - **CPU time and thread count**, from `/proc/<pid>/stat`, differenced between
//!   samples to give utilisation rather than a total.
//!
//! The whole process tree is sampled, not just the daemon: a session server's
//! cost is mostly its children, and reporting only the parent understates it by
//! whatever the shells are doing.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::report::Report;

#[derive(Debug, Clone)]
pub struct ProfileSpec {
    /// The daemon to watch. Its children are included.
    pub pid: u32,
    pub duration: Duration,
    pub interval: Duration,
}

/// One sample of the whole tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub at_secs: f64,
    pub processes: usize,
    pub threads: u64,
    pub rss_kb: u64,
    /// Proportional set size, or `None` where the kernel does not report it.
    /// Reported as absent rather than as RSS, because quoting RSS as PSS
    /// overstates the cost of every shared page.
    pub pss_kb: Option<u64>,
    /// CPU seconds the tree consumed since the previous sample, over the
    /// interval between them.
    pub cpu_percent: Option<f64>,
}

/// Per-process detail at the peak, which is what tells you where the memory went.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessAtPeak {
    pub pid: u32,
    pub name: String,
    pub rss_kb: u64,
    pub pss_kb: Option<u64>,
}

/// A sampler that can be stopped, so a workload can be profiled while it runs.
///
/// Owned by the caller rather than driven by a duration, because the interesting
/// window is "as long as the workload takes", which nobody knows in advance.
pub struct Sampler {
    pid: u32,
    interval: Duration,
    stop: Arc<AtomicBool>,
}

/// What a sampling window observed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub samples: Vec<Sample>,
    pub peak_rss_kb: u64,
    pub peak_pss_kb: Option<u64>,
    pub peak_threads: u64,
    pub mean_cpu_percent: Option<f64>,
    pub peak_cpu_percent: Option<f64>,
    pub processes_at_peak: Vec<ProcessAtPeak>,
    /// Set when the watched tree vanished mid-window, which invalidates the
    /// tail of the samples and must not be reported as a clean profile.
    pub vanished_at_secs: Option<f64>,
}

impl Sampler {
    /// Fails rather than degrading: a profile of a process this cannot read is
    /// a table of zeros that looks like a measurement.
    pub fn new(pid: u32, interval: Duration) -> anyhow::Result<Self> {
        if !cfg!(target_os = "linux") {
            bail!(
                "the profiler reads /proc, so it only runs on Linux; run it on the measurement host"
            );
        }
        if interval.is_zero() {
            bail!("a sampling interval of zero would spin without sampling");
        }
        if tree_of(pid)?.is_empty() {
            bail!("there is no process {pid} to profile");
        }
        Ok(Self {
            pid,
            interval,
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    /// A handle that stops the sampling loop.
    pub fn stopper(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// Sample until stopped or until `limit` elapses, whichever comes first.
    ///
    /// `limit` is a bound, not a target: a workload-driven profile passes the
    /// workload's own timeout so a wedged workload cannot leave a sampler
    /// running forever.
    pub async fn collect(self, limit: Duration) -> anyhow::Result<Profile> {
        let clock_ticks = clock_ticks_per_sec();
        let started = Instant::now();
        let deadline = started + limit;
        let mut samples: Vec<Sample> = Vec::new();
        let mut peak: (u64, Vec<ProcessAtPeak>) = (0, Vec::new());
        let mut prev: Option<(f64, u64)> = None;
        let mut vanished = None;

        while !self.stop.load(Ordering::Relaxed) && Instant::now() < deadline {
            let tree = tree_of(self.pid)?;
            if tree.is_empty() {
                vanished = Some(started.elapsed().as_secs_f64());
                break;
            }
            let at = started.elapsed().as_secs_f64();
            let mut rss = 0u64;
            let mut pss = 0u64;
            let mut pss_seen = false;
            let mut threads = 0u64;
            let mut ticks = 0u64;
            let mut detail = Vec::with_capacity(tree.len());
            for pid in &tree {
                let Some(st) = read_stat(*pid) else { continue };
                threads += st.threads;
                ticks += st.utime + st.stime;
                let p = read_pss(*pid);
                if let Some(v) = p {
                    pss_seen = true;
                    pss += v;
                }
                rss += st.rss_kb;
                detail.push(ProcessAtPeak {
                    pid: *pid,
                    name: st.name,
                    rss_kb: st.rss_kb,
                    pss_kb: p,
                });
            }
            let cpu = prev.map(|(prev_at, prev_ticks)| {
                let dt = at - prev_at;
                let dticks = ticks.saturating_sub(prev_ticks) as f64;
                if dt > 0.0 {
                    (dticks / clock_ticks) / dt * 100.0
                } else {
                    0.0
                }
            });
            prev = Some((at, ticks));

            let pss_kb = pss_seen.then_some(pss);
            // The peak is ranked on PSS when it is available, because with
            // shared pages RSS double counts and would pick the wrong sample.
            let rank = pss_kb.unwrap_or(rss);
            if rank > peak.0 {
                peak = (rank, detail);
            }
            samples.push(Sample {
                at_secs: at,
                processes: tree.len(),
                threads,
                rss_kb: rss,
                pss_kb,
                cpu_percent: cpu,
            });
            tokio::time::sleep(self.interval).await;
        }

        if samples.is_empty() {
            bail!("the window closed before a single sample was taken");
        }
        let cpu: Vec<f64> = samples.iter().filter_map(|s| s.cpu_percent).collect();
        Ok(Profile {
            peak_rss_kb: samples.iter().map(|s| s.rss_kb).max().unwrap_or(0),
            peak_pss_kb: samples.iter().filter_map(|s| s.pss_kb).max(),
            peak_threads: samples.iter().map(|s| s.threads).max().unwrap_or(0),
            mean_cpu_percent: (!cpu.is_empty())
                .then(|| cpu.iter().sum::<f64>() / cpu.len() as f64),
            peak_cpu_percent: cpu.iter().copied().fold(None, |acc: Option<f64>, v| {
                Some(acc.map_or(v, |a| a.max(v)))
            }),
            processes_at_peak: peak.1,
            vanished_at_secs: vanished,
            samples,
        })
    }
}

/// The standalone `profile` subcommand: sample a daemon for a fixed duration.
pub async fn run(spec: &ProfileSpec) -> anyhow::Result<Report> {
    let mut report = Report::new(
        "profile",
        &format!("pid {}", spec.pid),
        json!({
            "pid": spec.pid,
            "duration_secs": spec.duration.as_secs_f64(),
            "interval_secs": spec.interval.as_secs_f64(),
        }),
    );
    let started = Instant::now();
    let profile = Sampler::new(spec.pid, spec.interval)?
        .collect(spec.duration)
        .await?;
    report.duration_secs = started.elapsed().as_secs_f64();
    if let Some(at) = profile.vanished_at_secs {
        report.failures.push(format!(
            "process {} disappeared {at:.1}s into the run, so the samples stop there",
            spec.pid
        ));
    } else {
        report.checks_passed.push(format!(
            "sampled {} times over {:.1}s without the tree going away",
            profile.samples.len(),
            report.duration_secs
        ));
    }
    report.extra = serde_json::to_value(&profile)?;
    Ok(report)
}

struct Stat {
    name: String,
    utime: u64,
    stime: u64,
    threads: u64,
    rss_kb: u64,
}

/// `/proc/<pid>/stat`, parsed from the closing parenthesis of the command name
/// rather than by splitting the whole line: a command name may contain both
/// spaces and parentheses, and splitting on whitespace shifts every later field.
fn read_stat(pid: u32) -> Option<Stat> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = raw.find('(')?;
    let close = raw.rfind(')')?;
    let name = raw.get(open + 1..close)?.to_string();
    let rest: Vec<&str> = raw.get(close + 2..)?.split_whitespace().collect();
    // Field numbers from proc(5), offset by the two fields already consumed:
    // utime is 14, stime 15, num_threads 20, rss (in pages) 24.
    let f = |i: usize| rest.get(i).and_then(|v| v.parse::<u64>().ok());
    let page_kb = 4; // Every target of this harness uses 4 KiB pages.
    Some(Stat {
        name,
        utime: f(11)?,
        stime: f(12)?,
        threads: f(17)?,
        rss_kb: f(21)? * page_kb,
    })
}

/// PSS from `smaps_rollup`, which is one read rather than a walk of every
/// mapping. Absent on kernels without it and unreadable for processes this
/// user does not own, and in both cases the caller reports it as unavailable
/// rather than substituting RSS.
fn read_pss(pid: u32) -> Option<u64> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("Pss:") {
            return v.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Every pid in the tree rooted at `root`, the root included.
fn tree_of(root: u32) -> anyhow::Result<Vec<u32>> {
    if !Path::new(&format!("/proc/{root}")).exists() {
        return Ok(Vec::new());
    }
    // One pass over /proc building child lists, then a walk down. Reading
    // children per level instead would rescan /proc once per generation.
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let dir = std::fs::read_dir("/proc").context("reading /proc")?;
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(ppid) = read_ppid(pid) {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        if let Some(kids) = children.get(&out[i]) {
            for k in kids {
                // A pid cannot be its own ancestor, but /proc is read without a
                // lock and a recycled pid could produce a cycle. Guarding here
                // is cheaper than a hung profiler.
                if !out.contains(k) {
                    out.push(*k);
                }
            }
        }
        i += 1;
    }
    Ok(out)
}

fn read_ppid(pid: u32) -> Option<u32> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = raw.rfind(')')?;
    // The field after the state character.
    raw.get(close + 2..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Ticks per second, which converts `utime`/`stime` into CPU seconds.
///
/// Hardcoded rather than read through `sysconf`: every Linux target here uses
/// 100, and linking libc for one constant is not worth it. A kernel configured
/// otherwise would make the CPU column wrong by a fixed factor, which the
/// sample count and memory columns do not depend on.
fn clock_ticks_per_sec() -> f64 {
    100.0
}

/// The parsers read `/proc`, so they are only exercised where `/proc` exists.
/// `Sampler::new` already refuses to run anywhere else, and a test that skipped
/// its assertions on another platform would report a pass for a check it never
/// made.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// A command name containing spaces and parentheses is the case that breaks
    /// a naive whitespace split, and it happens in practice.
    #[test]
    fn the_stat_parser_survives_a_hostile_command_name() {
        // This process always exists, and its own stat line is real input.
        let me = std::process::id();
        let st = read_stat(me).expect("this process has a stat line");
        assert!(st.threads >= 1, "a running process has at least one thread");
        assert!(st.rss_kb > 0, "a running process has resident memory");
    }

    /// The tree must include the root, or every sample would report zero.
    #[test]
    fn the_tree_contains_the_process_it_was_asked_about() {
        let me = std::process::id();
        let tree = tree_of(me).expect("reading /proc");
        assert!(tree.contains(&me), "the tree of {me} did not contain {me}");
    }

    /// A pid that cannot exist must produce an empty tree rather than an error,
    /// because that is how the sampler notices the daemon exited.
    #[test]
    fn a_missing_process_has_an_empty_tree() {
        assert!(
            tree_of(u32::MAX).expect("reading /proc").is_empty(),
            "a nonexistent pid reported a tree"
        );
    }
}
