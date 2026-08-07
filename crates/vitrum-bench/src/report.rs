//! Run reports: one directory per run, JSON for machines, Markdown for people.
//!
//! Both are written from the same value, so the prose and the numbers cannot
//! drift. The JSON is the record; the Markdown is a rendering of it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::stats::{LatencySummary, Throughput};

/// What a workload measured, whatever the workload was.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// `load`, `race`, `fuzz` or `profile`.
    pub workload: String,
    /// Identifies the run and names its directory.
    pub run_id: String,
    /// The daemon this ran against.
    pub server: String,
    /// Everything that shaped the run, so a report explains its own numbers.
    pub params: serde_json::Value,
    pub duration_secs: f64,
    /// Per-operation latency, keyed by operation name.
    pub latencies: Vec<(String, LatencySummary)>,
    pub throughput: Option<Throughput>,
    /// Invariants that were checked and held. A report with an empty list
    /// asserted nothing, and says so rather than looking clean.
    pub checks_passed: Vec<String>,
    /// Anything the run found. Non-empty means the run failed.
    pub failures: Vec<String>,
    /// Free-form measurements a particular workload adds.
    pub extra: serde_json::Value,
    /// Binary inputs that triggered a failure. Written under `repro/` when the
    /// report is persisted; skipped from JSON so a report stays readable.
    #[serde(skip)]
    pub artifacts: Vec<(String, Vec<u8>)>,
}

impl Report {
    pub fn new(workload: &str, server: &str, params: serde_json::Value) -> Self {
        Self {
            workload: workload.to_string(),
            run_id: run_id(workload),
            server: server.to_string(),
            params,
            duration_secs: 0.0,
            latencies: Vec::new(),
            throughput: None,
            checks_passed: Vec::new(),
            failures: Vec::new(),
            extra: serde_json::Value::Null,
            artifacts: Vec::new(),
        }
    }

    pub fn failed(&self) -> bool {
        !self.failures.is_empty()
    }

    /// Write `report.json` and `report.md` under `dir/<run_id>/`.
    ///
    /// When [`Self::artifacts`] is non-empty, also writes each input under
    /// `repro/<name>` so a panic or bound failure carries the exact bytes that
    /// triggered it — not just a description that has to be reverse-engineered
    /// from a seed and an index.
    pub fn write(&self, dir: &Path) -> anyhow::Result<PathBuf> {
        let out = dir.join(&self.run_id);
        std::fs::create_dir_all(&out)?;
        std::fs::write(out.join("report.json"), serde_json::to_vec_pretty(self)?)?;
        std::fs::write(out.join("report.md"), self.markdown())?;
        if !self.artifacts.is_empty() {
            let repro = out.join("repro");
            std::fs::create_dir_all(&repro)?;
            for (name, bytes) in &self.artifacts {
                std::fs::write(repro.join(name), bytes)?;
            }
        }
        Ok(out)
    }

    pub fn markdown(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "# {} run {}\n", self.workload, self.run_id);
        let _ = writeln!(s, "- server: `{}`", self.server);
        let _ = writeln!(s, "- duration: {:.2}s", self.duration_secs);
        let _ = writeln!(
            s,
            "- verdict: {}\n",
            if self.failed() { "FAILED" } else { "passed" }
        );

        let _ = writeln!(s, "## Parameters\n");
        let _ = writeln!(s, "```json\n{}\n```\n", pretty(&self.params));

        if let Some(t) = &self.throughput {
            let _ = writeln!(s, "## Throughput\n");
            let _ = writeln!(s, "| | |\n|---|---|");
            let _ = writeln!(s, "| bytes | {} |", t.bytes);
            let _ = writeln!(s, "| messages | {} |", t.messages);
            let _ = writeln!(s, "| bytes/s | {:.0} |", t.bytes_per_sec);
            let _ = writeln!(s, "| messages/s | {:.1} |\n", t.messages_per_sec);
        }

        if !self.latencies.is_empty() {
            let _ = writeln!(s, "## Latency, microseconds\n");
            let _ = writeln!(
                s,
                "| operation | n | min | p50 | p95 | p99 | max | mean |\n\
                 |---|--:|--:|--:|--:|--:|--:|--:|"
            );
            for (name, l) in &self.latencies {
                let _ = writeln!(
                    s,
                    "| {} | {} | {} | {} | {} | {} | {} | {} |",
                    name, l.count, l.min_us, l.p50_us, l.p95_us, l.p99_us, l.max_us, l.mean_us
                );
            }
            let _ = writeln!(s);
        }

        if !self.checks_passed.is_empty() {
            let _ = writeln!(s, "## Invariants held\n");
            for c in &self.checks_passed {
                let _ = writeln!(s, "- {c}");
            }
            let _ = writeln!(s);
        }

        if self.failed() {
            let _ = writeln!(s, "## Failures\n");
            for f in &self.failures {
                let _ = writeln!(s, "- {f}");
            }
            let _ = writeln!(s);
        }

        if !self.artifacts.is_empty() {
            let _ = writeln!(s, "## Repro artifacts\n");
            let _ = writeln!(
                s,
                "Exact inputs that triggered a failure, under `repro/` next to this report:\n"
            );
            for (name, bytes) in &self.artifacts {
                let _ = writeln!(s, "- `{name}` ({} bytes)", bytes.len());
            }
            let _ = writeln!(s);
        }

        if !self.extra.is_null() {
            let _ = writeln!(s, "## Detail\n");
            let _ = writeln!(s, "```json\n{}\n```", pretty(&self.extra));
        }
        s
    }
}

fn pretty(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// `<workload>-<seconds since epoch>`, which sorts chronologically and never
/// collides across the sequential runs one host does.
fn run_id(workload: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{workload}-{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_captured_failure_writes_a_repro_file() {
        let mut report = Report::new("fuzz", "ws://test", json!({}));
        report.failures.push("synthetic [repro: fuzz-0001.bin]".into());
        report.artifacts.push(("fuzz-0001.bin".into(), b"hostile-frame".to_vec()));
        let dir = std::env::temp_dir().join(format!("vitrum-fuzz-repro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let out = report.write(&dir).expect("write");
        let bytes = std::fs::read(out.join("repro/fuzz-0001.bin")).expect("repro");
        assert_eq!(bytes, b"hostile-frame");
        let md = std::fs::read_to_string(out.join("report.md")).expect("md");
        assert!(md.contains("fuzz-0001.bin"), "markdown must name the repro");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
