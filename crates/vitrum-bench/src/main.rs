//! `vitrum-bench` command line.
//!
//! Argument parsing is by hand for the same reason the rest of the workspace
//! does it: six subcommands with a dozen flags between them do not justify a
//! dependency, and the error messages here can say what to do next.

use std::future::Future;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::{Context, bail};
use vitrum_bench::report::Report;
#[cfg(feature = "daemon")]
use vitrum_bench::pipeline;
use vitrum_bench::{divergence, frame, fuzz, latency, load, probe, profile, race, world};

/// Allocations on the output path are part of what `pipeline` reports, and an
/// allocator cannot be swapped in per workload. Counting is two relaxed adds
/// per allocation, which is noise next to a syscall and is paid by a harness
/// binary rather than by the daemon.
///
/// Only with the daemon workloads compiled in. `latency` times a GPU fence and
/// would rather not pay two atomics on every allocation a driver makes.
#[cfg(feature = "daemon")]
#[global_allocator]
static ALLOCATOR: pipeline::Counting = pipeline::Counting;

const USAGE: &str = "\
vitrum-bench: load, concurrency, fuzz, pipeline, latency and profiling harness for vitrum

Usage:
  vitrum-bench load    [--server URL] [--sessions N] [--lines N] [--drain SECS]
  vitrum-bench race    [--server URL] [--connections N] [--sessions N] [--renames N]
  vitrum-bench fuzz    [--server URL] [--cases N] [--seed N]
  vitrum-bench pipeline [--megabytes N] [--fanout-megabytes N] [--sessions LIST]
                       [--samples N] [--scrollback-bytes N]
  vitrum-bench latency [--samples N] [--spawns N] [--panes N] [--sessions N]
                       [--gate] [--software]
  vitrum-bench probe   [--cases N] [--seed N] [--threads N]
  vitrum-bench frame   [--frames N] [--rounds N] [--cols N] [--rows N] [--seed N]
                       [--software]
  vitrum-bench divergence [--cases N] [--schedules N] [--threads N] [--seed N]
                       [--corpus DIR]
  vitrum-bench world   [--server URL] [--windows N] [--sessions N] [--widest N]
                       [--burst-lines N] [--ssh-host HOST] [--settle SECS]
                       [--keystrokes N] [--streams N] [--stream-lines N]
  vitrum-bench profile --pid PID [--duration SECS] [--interval SECS]
  vitrum-bench emit    --bytes N | --idle SECS

Common:
  --out DIR          where to write the report directory (default harness/out)
  --server URL       daemon to test (default ws://127.0.0.1:7777/ws)
  --profile-pid PID  sample that process tree while the workload runs, and fold
                     the profile into the same report
  --interval SECS    sampling interval (default 0.5)
  --sessions LIST    session counts `pipeline` measures, e.g. 1,8,32

`pipeline` runs in this process against a real pty and a real SessionManager,
so it takes no --server. It re-executes this binary as `emit`, which is the
child under measurement and is not run by hand.

`latency` measures the pane rather than the daemon: the interval between a
cause and the pixels that answer it, ending on the GPU's fence. It needs a GPU
and no display, takes no --server, and re-executes this binary as
`latency-child`. With --gate it exits non-zero when a signal crosses the bound
recorded for it in `vitrum_bench::latency::bound`.

`frame` runs the render path in this process and attributes each frame to
parse, store, damage, upload and submit. The attribution only exists in a
binary built with `--features probe`; without it the run measures the same
frames with no probe compiled in, which is the control the zero-cost claim is
judged against. `harness/frame.sh` builds both and compares them.

`divergence` fuzzes the parser and the grid two ways: one input fed whole
against the same input fed in pieces, and one input fed under many thread
interleavings against the screen it produces alone. Anything it finds is
minimised and committed to crates/vitrum-bench/artifacts/ as a replayable
file, and the artefacts already there are replayed at the start of every run.

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
    // Before anything that could print: these processes ARE the output under
    // measurement, and a line of harness prose on the stream would be measured
    // as if the child had written it.
    #[cfg(feature = "daemon")]
    if cmd == "emit" {
        let flags = Flags::parse(args)?;
        match (flags.u64_or("--bytes", 0)?, flags.u64_or("--idle", 0)?) {
            (0, 0) => bail!("emit needs --bytes N or --idle SECS"),
            (0, secs) => pipeline::idle(secs),
            (bytes, _) => pipeline::emit(bytes as usize)?,
        }
        return Ok(true);
    }
    if cmd == "latency-child" {
        let flags = Flags::parse(args)?;
        latency::child(flags.usize_or("--panes", 8)?, flags.flag("--software"))?;
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
        #[cfg(feature = "daemon")]
        "pipeline" => {
            let spec = pipeline::PipelineSpec {
                megabytes: flags.usize_or("--megabytes", 100)?,
                fanout_megabytes: flags.usize_or("--fanout-megabytes", 8)?,
                sessions: session_counts(&flags)?,
                samples: flags.usize_or("--samples", 200)?,
                scrollback_bytes: flags.usize_or("--scrollback-bytes", 10 * 1024 * 1024)?,
            };
            runtime.block_on(profiled(
                profile_pid,
                interval,
                sampler_limit,
                pipeline::run(&spec),
            ))?
        }
        "latency" => {
            let spec = latency::LatencySpec {
                samples: flags.usize_or("--samples", 2000)?,
                spawns: flags.usize_or("--spawns", 5)?,
                panes: flags.usize_or("--panes", 8)?,
                sessions: flags.usize_or("--sessions", 200)?,
                cpu_windows: flags.usize_or("--cpu-windows", 8)?,
                gate: flags.flag("--gate"),
                software: flags.flag("--software"),
            };
            let report = latency::run(&spec)?;
            // The table before the report body: the point of the run is the
            // seven numbers, and a reader should not have to find them in JSON.
            println!("{}", latency::table(&report));
            report
        }
        "probe" => {
            let spec = probe::ProbeSpec {
                cases: flags.usize_or("--cases", 200_000)?,
                seed: flags.u64_or("--seed", 1)?,
                threads: flags.usize_or("--threads", 4)?,
            };
            probe::run(&spec)?
        }
        "frame" => {
            let spec = frame::FrameSpec {
                frames: flags.usize_or("--frames", 500)?,
                rounds: flags.usize_or("--rounds", 5)?,
                cols: flags.usize_or("--cols", 120)? as u16,
                rows: flags.usize_or("--rows", 40)? as u16,
                seed: flags.u64_or("--seed", 1)?,
                software: flags.flag("--software"),
            };
            let report = frame::run(&spec)?;
            println!("{}", frame::table(&report));
            report
        }
        "divergence" => {
            let spec = divergence::DivergenceSpec {
                cases: flags.usize_or("--cases", 20_000)?,
                schedules: flags.usize_or("--schedules", 2_000)?,
                threads: flags.usize_or("--threads", 4)?,
                seed: flags.u64_or("--seed", 1)?,
                corpus: flags
                    .path("--corpus")?
                    .unwrap_or_else(|| PathBuf::from(divergence::CORPUS_DIR)),
            };
            divergence::run(&spec)?
        }
        "world" => {
            let spec = world::WorldSpec {
                server,
                windows: flags.usize_or("--windows", 3)?,
                sessions_per_window: flags.usize_or("--sessions", 3)?,
                widest_cols: flags.usize_or("--widest", 120)? as u16,
                lines_per_burst: flags.usize_or("--burst-lines", 400)?,
                ssh_host: flags.string("--ssh-host"),
                settle: flags.secs_or("--settle", 2.0)?,
                keystroke_samples: flags.usize_or("--keystrokes", 400)?,
                stream_sessions: flags.usize_or("--streams", 7)?,
                stream_lines: flags.usize_or("--stream-lines", 200_000)?,
            };
            runtime.block_on(profiled(profile_pid, interval, sampler_limit, world::run(&spec)))?
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

/// The session counts `pipeline` sweeps, as a comma separated list.
///
/// A list rather than a single number because the interesting result is the
/// shape of the curve: one session says what the path costs, and thirty two
/// says whether it costs the same each when they are all running at once.
#[cfg(feature = "daemon")]
fn session_counts(flags: &Flags) -> anyhow::Result<Vec<usize>> {
    let Some(raw) = flags.string("--sessions") else {
        return Ok(vec![1, 8, 32]);
    };
    let mut counts = Vec::new();
    for field in raw.split(',') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        let n: usize = field
            .parse()
            .with_context(|| format!("bad --sessions entry `{field}`"))?;
        if n == 0 {
            bail!("--sessions entries must be at least 1");
        }
        counts.push(n);
    }
    if counts.is_empty() {
        bail!("--sessions needs at least one count, e.g. 1,8,32");
    }
    Ok(counts)
}

/// `--name value` pairs and bare `--switch` flags, collected once so lookups do
/// not depend on order.
///
/// A flag whose next token is another flag, or which ends the arguments, is a
/// switch and records `"true"`. Nothing here takes a value beginning with two
/// dashes, so the two forms cannot be confused.
struct Flags(Vec<(String, String)>);

impl Flags {
    fn parse(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut out = Vec::new();
        let mut it = args.peekable();
        while let Some(k) = it.next() {
            if !k.starts_with("--") {
                bail!("expected a --flag, got `{k}`");
            }
            let switch = it.peek().is_none_or(|v| v.starts_with("--"));
            let v = if switch {
                "true".to_string()
            } else {
                it.next().unwrap_or_default()
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

    /// Whether a switch was given. Absent is false; present with any value
    /// other than `false` is true, so `--gate` and `--gate true` agree.
    fn flag(&self, key: &str) -> bool {
        match self.string(key) {
            None => false,
            Some(v) => v != "false",
        }
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
