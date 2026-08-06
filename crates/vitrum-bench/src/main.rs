//! `vitrum-bench` command line.
//!
//! Argument parsing is by hand for the same reason the rest of the workspace
//! does it: four subcommands with a dozen flags between them do not justify a
//! dependency, and the error messages here can say what to do next.

use std::future::Future;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, bail};
use vitrum_bench::report::Report;
use vitrum_bench::{fuzz, load, profile, race};

const USAGE: &str = "\
vitrum-bench: load, concurrency, fuzz and profiling harness for the vitrum daemon

Usage:
  vitrum-bench load    [--server URL] [--sessions N] [--lines N] [--drain SECS]
  vitrum-bench race    [--server URL] [--connections N] [--sessions N] [--renames N]
  vitrum-bench fuzz    [--server URL] [--cases N] [--seed N]
  vitrum-bench profile --pid PID [--duration SECS] [--interval SECS]

Common:
  --out DIR          where to write the report directory (default harness/out)
  --server URL       daemon to test (default ws://127.0.0.1:7777/ws)
  --profile-pid PID  sample that process tree while the workload runs, and fold
                     the profile into the same report
  --interval SECS    sampling interval (default 0.5)

Every run writes report.json and report.md into <out>/<workload>-<timestamp>/.
Exit status is 1 when the run found a failure, so it can gate CI.
";

/// The port `vitrum-server` listens on by default.
const DEFAULT_SERVER: &str = "ws://127.0.0.1:7777/ws";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("vitrum-bench: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `Ok(false)` means the run completed and found a failure, which is different
/// from the harness itself being unable to run.
fn run() -> anyhow::Result<bool> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else {
        print!("{USAGE}");
        bail!("no subcommand given");
    };
    if cmd == "--help" || cmd == "-h" || cmd == "help" {
        print!("{USAGE}");
        return Ok(true);
    }
    let flags = Flags::parse(args)?;
    let out = flags
        .path("--out")?
        .unwrap_or_else(|| PathBuf::from("harness/out"));
    let server = flags
        .string("--server")
        .unwrap_or_else(|| DEFAULT_SERVER.to_string());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the tokio runtime")?;

    // A workload may carry a profile of the daemon taken while it ran, which is
    // the combination that answers "what did that cost", rather than two runs
    // whose windows do not line up.
    let profile_pid: Option<u32> = match flags.u64_or("--profile-pid", 0)? {
        0 => None,
        pid => Some(pid.try_into().context("--profile-pid must be a process id")?),
    };
    let interval = flags.secs_or("--interval", 0.5)?;
    // The bound a workload-driven sampler stops at if the workload never
    // finishes, so a wedged run cannot leave a sampler spinning.
    let sampler_limit = Duration::from_secs(3600);

    let report: Report = match cmd.as_str() {
        "load" => {
            let spec = load::LoadSpec {
                server,
                sessions: flags.usize_or("--sessions", 20)?,
                lines: flags.usize_or("--lines", 2000)?,
                drain: flags.secs_or("--drain", 60.0)?,
                cols: flags.usize_or("--cols", 120)? as u16,
                rows: flags.usize_or("--rows", 40)? as u16,
            };
            runtime.block_on(profiled(profile_pid, interval, sampler_limit, load::run(&spec)))?
        }
        "race" => {
            let spec = race::RaceSpec {
                server,
                connections: flags.usize_or("--connections", 8)?,
                sessions_per_conn: flags.usize_or("--sessions", 4)?,
                renames: flags.usize_or("--renames", 5)?,
                settle: flags.secs_or("--settle", 2.0)?,
            };
            runtime.block_on(profiled(profile_pid, interval, sampler_limit, race::run(&spec)))?
        }
        "fuzz" => {
            let spec = fuzz::FuzzSpec {
                server,
                cases: flags.usize_or("--cases", 2000)?,
                seed: flags.u64_or("--seed", 1)?,
                oracle_timeout: flags.secs_or("--oracle-timeout", 10.0)?,
            };
            runtime.block_on(profiled(profile_pid, interval, sampler_limit, fuzz::run(&spec)))?
        }
        "profile" => {
            let pid = profile_pid
                .or_else(|| flags.u64_or("--pid", 0).ok().and_then(|v| v.try_into().ok()))
                .filter(|p| *p != 0);
            let Some(pid) = pid else {
                bail!("profile needs --pid, the pid of the vitrum-server to watch");
            };
            runtime.block_on(profile::run(&profile::ProfileSpec {
                pid,
                duration: flags.secs_or("--duration", 60.0)?,
                interval,
            }))?
        }
        other => {
            print!("{USAGE}");
            bail!("unknown subcommand `{other}`");
        }
    };

    let dir = report
        .write(&out)
        .with_context(|| format!("writing the report under {}", out.display()))?;
    // Printed rather than logged: the path is the point of the run, and a caller
    // that wants the numbers reads the file.
    println!("{}", report.markdown());
    println!("report written to {}", dir.display());
    Ok(!report.failed())
}

/// Run `workload`, sampling the daemon's process tree alongside it when a pid
/// was given, and fold the profile into the workload's own report.
///
/// One report rather than two, because a profile is only interpretable next to
/// what was happening while it was taken.
async fn profiled(
    pid: Option<u32>,
    interval: Duration,
    limit: Duration,
    workload: impl Future<Output = anyhow::Result<Report>>,
) -> anyhow::Result<Report> {
    let Some(pid) = pid else {
        return workload.await;
    };
    let sampler = profile::Sampler::new(pid, interval)?;
    let stop = sampler.stopper();
    let sampling = tokio::spawn(sampler.collect(limit));

    let outcome = workload.await;
    // Stopped before the workload's result is unwrapped, so a failing workload
    // still ends the sampler rather than leaving it to run out its bound.
    stop.store(true, Ordering::SeqCst);
    let sampled = sampling.await.context("the sampling task did not finish")?;

    let mut report = outcome?;
    let profile = sampled?;
    if let Some(at) = profile.vanished_at_secs {
        report.failures.push(format!(
            "the daemon's process tree vanished {at:.1}s into the run, so the profile stops there"
        ));
    }
    let value = serde_json::to_value(&profile)?;
    match &mut report.extra {
        serde_json::Value::Object(map) => {
            map.insert("daemon_profile".to_string(), value);
        }
        other => {
            *other = serde_json::json!({ "workload": other.clone(), "daemon_profile": value });
        }
    }
    Ok(report)
}

/// `--name value` pairs, collected once so lookups do not depend on order.
struct Flags(Vec<(String, String)>);

impl Flags {
    fn parse(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut out = Vec::new();
        let mut it = args.peekable();
        while let Some(k) = it.next() {
            if !k.starts_with("--") {
                bail!("expected a --flag, got `{k}`");
            }
            let Some(v) = it.next() else {
                bail!("`{k}` needs a value");
            };
            out.push((k, v));
        }
        Ok(Self(out))
    }

    fn string(&self, key: &str) -> Option<String> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn path(&self, key: &str) -> anyhow::Result<Option<PathBuf>> {
        Ok(self.string(key).map(PathBuf::from))
    }

    fn usize_or(&self, key: &str, default: usize) -> anyhow::Result<usize> {
        match self.string(key) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .with_context(|| format!("`{key}` expects a whole number, got `{v}`")),
        }
    }

    fn u64_or(&self, key: &str, default: u64) -> anyhow::Result<u64> {
        match self.string(key) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .with_context(|| format!("`{key}` expects a whole number, got `{v}`")),
        }
    }

    fn secs_or(&self, key: &str, default: f64) -> anyhow::Result<Duration> {
        let secs = match self.string(key) {
            None => default,
            Some(v) => v
                .parse()
                .with_context(|| format!("`{key}` expects seconds, got `{v}`"))?,
        };
        if !secs.is_finite() || secs < 0.0 {
            bail!("`{key}` must be a non-negative number of seconds");
        }
        Ok(Duration::from_secs_f64(secs))
    }
}
