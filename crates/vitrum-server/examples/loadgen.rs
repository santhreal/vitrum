//! Load harness for the daemon, used to measure what it costs under load.
//!
//! It drives a daemon you started yourself, so the numbers come from a real
//! process you can also watch with `top`. Every scenario reports the same four
//! things, sampled from `/proc` around the run: CPU seconds the daemon burned,
//! its peak RSS, wall time, and how many times the kernel scheduled it. The
//! context-switch count is the one that settles "is an idle daemon really
//! idle", because a wakeup with no output behind it shows up there and nowhere
//! else.
//!
//! # Running it
//!
//! ```text
//! cargo build --release -p vitrum-server
//! ./target/release/vitrum-server --port 7737 &
//! cargo run --release -p vitrum-server --example loadgen -- \
//!     --pid $! --port 7737 stream --sessions 20 --mb 25
//! ```
//!
//! Scenarios:
//!
//! - `stream --sessions N --mb M` — N sessions each emitting exactly M MiB.
//!   Deterministic total work, so CPU per byte is comparable across changes.
//! - `burst --mb M` — one session emitting M MiB in a single unbroken run.
//! - `idle --sessions N --secs S` — N sessions that produce nothing. The
//!   interesting numbers are CPU and context switches, both of which should be
//!   near zero.
//! - `search --sessions N --mb M --pattern P` — fill N rings, then time one
//!   cross-session sweep over all of them.
//!
//! `--viewers K` attaches K extra connections to the first session, which is
//! how the broadcast fanout is measured.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use vitrum_server::DEFAULT_SCROLLBACK_BYTES;
use vitrum_proto::{
    ClientMsg, OUTPUT_HEADER_LEN, PROTOCOL_VERSION, ProjectId, ServerMsg, SessionId,
};

/// Linux `USER_HZ`. Fixed at 100 on every supported configuration, and the
/// kernel reports `/proc/<pid>/stat` times in these units regardless of `HZ`.
const USER_HZ: f64 = 100.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opts = Options::parse(std::env::args().skip(1))?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(opts))
}

struct Options {
    pid: u32,
    port: u16,
    scenario: String,
    sessions: usize,
    viewers: usize,
    mib: usize,
    secs: u64,
    pattern: String,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut opts = Options {
            pid: 0,
            port: 7737,
            scenario: String::new(),
            sessions: 20,
            viewers: 0,
            mib: 25,
            secs: 60,
            pattern: "needle-not-present".to_string(),
        };
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--pid" => opts.pid = value()?.parse().map_err(|e| format!("--pid: {e}"))?,
                "--port" => opts.port = value()?.parse().map_err(|e| format!("--port: {e}"))?,
                "--sessions" => {
                    opts.sessions = value()?.parse().map_err(|e| format!("--sessions: {e}"))?;
                }
                "--viewers" => {
                    opts.viewers = value()?.parse().map_err(|e| format!("--viewers: {e}"))?;
                }
                "--mb" => opts.mib = value()?.parse().map_err(|e| format!("--mb: {e}"))?,
                "--secs" => opts.secs = value()?.parse().map_err(|e| format!("--secs: {e}"))?,
                "--pattern" => opts.pattern = value()?,
                other if other.starts_with("--") => return Err(format!("unknown flag {other}")),
                other => opts.scenario = other.to_string(),
            }
        }
        if opts.pid == 0 {
            return Err("--pid is required: the daemon process to measure".to_string());
        }
        if opts.scenario.is_empty() {
            return Err("name a scenario: stream, burst, idle or search".to_string());
        }
        Ok(opts)
    }
}

/// One reading of what the daemon has cost so far.
#[derive(Clone, Copy)]
struct Sample {
    cpu_s: f64,
    rss_kib: u64,
    peak_rss_kib: u64,
    switches: u64,
    /// `read`-family syscalls, from `/proc/<pid>/io`. Divided into the bytes
    /// read it gives the real PTY read size, which is what decides whether the
    /// per-read allocation in the reader thread is worth anything.
    reads: u64,
    read_bytes: u64,
}

fn sample(pid: u32) -> Result<Sample, String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map_err(|e| format!("reading /proc/{pid}/stat: {e}"))?;
    // The comm field is parenthesised and may contain spaces, so field parsing
    // starts after the last ')' rather than at the first space.
    let tail = stat
        .rsplit_once(')')
        .ok_or("malformed /proc stat: no comm terminator")?
        .1;
    let fields: Vec<&str> = tail.split_whitespace().collect();
    // `tail` starts at field 3 (state), so field N is at index N - 3.
    let ticks = |n: usize| -> Result<f64, String> {
        fields
            .get(n - 3)
            .ok_or_else(|| format!("/proc stat has no field {n}"))?
            .parse::<f64>()
            .map_err(|e| format!("/proc stat field {n}: {e}"))
    };
    let cpu_s = (ticks(14)? + ticks(15)?) / USER_HZ;

    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .map_err(|e| format!("reading /proc/{pid}/status: {e}"))?;
    let io = std::fs::read_to_string(format!("/proc/{pid}/io"))
        .map_err(|e| format!("reading /proc/{pid}/io: {e}"))?;
    Ok(Sample {
        cpu_s,
        rss_kib: field(&status, "VmRSS:"),
        peak_rss_kib: field(&status, "VmHWM:"),
        switches: thread_switches(pid)?,
        reads: field(&io, "syscr:"),
        read_bytes: field(&io, "rchar:"),
    })
}

/// First whitespace-delimited number after `key`, or 0 when absent.
fn field(status: &str, key: &str) -> u64 {
    status
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Context switches summed over every thread in the process.
///
/// `/proc/<pid>/status` reports the counters for the group leader alone, which
/// is the one thread in this daemon that does no work. Idle-wakeup accounting
/// has to walk `task/` or it reads zero no matter what the runtime is doing.
fn thread_switches(pid: u32) -> Result<u64, String> {
    let tasks = std::fs::read_dir(format!("/proc/{pid}/task"))
        .map_err(|e| format!("listing /proc/{pid}/task: {e}"))?;
    let mut total = 0;
    for task in tasks {
        let path = task.map_err(|e| format!("reading a task entry: {e}"))?.path();
        // A thread that exits between the listing and the read is normal.
        let Ok(status) = std::fs::read_to_string(path.join("status")) else {
            continue;
        };
        total += field(&status, "voluntary_ctxt_switches:")
            + field(&status, "nonvoluntary_ctxt_switches:");
    }
    Ok(total)
}

/// Print the run.
///
/// `ingested` is what the children wrote, which is the work the daemon's read
/// path actually did. `delivered` is what reached a client, and is smaller
/// whenever the broadcast queue laps a viewer. Dividing CPU by `delivered`
/// would flatter a change that simply dropped more frames, so the per-byte cost
/// is always taken against `ingested`.
fn report(
    label: &str,
    before: Sample,
    after: Sample,
    wall: Duration,
    ingested: u64,
    delivered: u64,
) {
    let cpu = after.cpu_s - before.cpu_s;
    let switches = after.switches - before.switches;
    println!("--- {label}");
    println!("wall            {:.3} s", wall.as_secs_f64());
    println!("daemon cpu      {cpu:.3} s");
    println!("cpu / wall      {:.1} %", 100.0 * cpu / wall.as_secs_f64());
    println!("rss end         {:.1} MiB", after.rss_kib as f64 / 1024.0);
    println!(
        "rss peak        {:.1} MiB",
        after.peak_rss_kib as f64 / 1024.0
    );
    println!("ctx switches    {switches}");
    let reads = after.reads - before.reads;
    let read_bytes = after.read_bytes - before.read_bytes;
    println!("read syscalls   {reads}");
    if reads > 0 {
        println!("bytes / read    {}", read_bytes / reads);
    }
    if ingested > 0 {
        let mib = ingested as f64 / (1024.0 * 1024.0);
        println!("bytes ingested  {mib:.1} MiB");
        println!("bytes delivered {:.1} MiB", delivered as f64 / 1048576.0);
        println!("cpu / MiB       {:.3} ms", 1000.0 * cpu / mib);
        println!("throughput      {:.1} MiB/s", mib / wall.as_secs_f64());
        println!("switches / MiB  {:.1}", switches as f64 / mib);
    }
}

/// A websocket connection that has already greeted the daemon.
struct Client {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    /// Binary output payload bytes received so far, header excluded.
    output_bytes: u64,
}

impl Client {
    async fn connect(port: u16) -> Result<Self, Box<dyn std::error::Error>> {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}")).await?;
        let mut client = Client {
            ws,
            output_bytes: 0,
        };
        client
            .send(&ClientMsg::Hello {
                protocol: PROTOCOL_VERSION,
            })
            .await?;
        client.until(|msg| matches!(msg, ServerMsg::Welcome { .. })).await?;
        Ok(client)
    }

    async fn send(&mut self, msg: &ClientMsg<'_>) -> Result<(), Box<dyn std::error::Error>> {
        self.ws
            .send(Message::Text(serde_json::to_string(msg)?))
            .await?;
        Ok(())
    }

    /// Read frames until `stop` accepts one, returning it.
    async fn until(
        &mut self,
        mut stop: impl FnMut(&ServerMsg) -> bool,
    ) -> Result<ServerMsg, Box<dyn std::error::Error>> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let msg: ServerMsg = serde_json::from_str(&text)?;
                    if stop(&msg) {
                        return Ok(msg);
                    }
                }
                Some(Ok(Message::Binary(data))) => {
                    self.output_bytes += data.len().saturating_sub(OUTPUT_HEADER_LEN) as u64;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => return Err("daemon closed the connection".into()),
            }
        }
    }

    /// Drain frames until `deadline`, counting output.
    async fn drain_until(&mut self, deadline: Instant) {
        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            match tokio::time::timeout(left, self.ws.next()).await {
                Ok(Some(Ok(Message::Binary(data)))) => {
                    self.output_bytes += data.len().saturating_sub(OUTPUT_HEADER_LEN) as u64;
                }
                Ok(Some(Ok(_))) => {}
                Ok(_) | Err(_) => return,
            }
        }
    }

    async fn create(
        &mut self,
        project: u64,
        script: &str,
    ) -> Result<SessionId, Box<dyn std::error::Error>> {
        self.send(&ClientMsg::CreateSession {
            project_id: ProjectId(project),
            cwd: std::env::temp_dir().to_string_lossy().into_owned().into(),
            command: "sh".into(),
            args: vec!["-c".into(), script.into()],
            cols: 80,
            rows: 24,
            title: None,
        })
        .await?;
        let created = self
            .until(|msg| matches!(msg, ServerMsg::SessionCreated(_)))
            .await?;
        match created {
            ServerMsg::SessionCreated(info) => Ok(info.id),
            _ => unreachable!("until only returns the frame it accepted"),
        }
    }
}

/// A shell command emitting exactly `mib` MiB of line-oriented output.
fn emitter(mib: usize) -> String {
    format!(
        "yes 'vitrum load line 0123456789 abcdefghijklmnopqrstuvwxyz' | head -c {}",
        mib * 1024 * 1024
    )
}

async fn run(opts: Options) -> Result<(), Box<dyn std::error::Error>> {
    match opts.scenario.as_str() {
        "stream" => stream(&opts).await,
        "burst" => burst(&opts).await,
        "idle" => idle(&opts).await,
        "search" => search(&opts).await,
        other => Err(format!("unknown scenario {other}").into()),
    }
}

/// Create the sessions, attach one viewer to each, and wait for every child to
/// finish emitting its share.
async fn stream(opts: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut control = Client::connect(opts.port).await?;
    let mut ids = Vec::new();
    for i in 0..opts.sessions {
        ids.push(control.create(i as u64 % 4, &emitter(opts.mib)).await?);
    }

    let mut viewers = Vec::new();
    for id in &ids {
        let mut viewer = Client::connect(opts.port).await?;
        viewer
            .send(&ClientMsg::Attach {
                session: *id,
                cols: 80,
                rows: 24,
            })
            .await?;
        viewers.push(viewer);
    }
    for _ in 0..opts.viewers {
        let mut extra = Client::connect(opts.port).await?;
        extra
            .send(&ClientMsg::Attach {
                session: ids[0],
                cols: 80,
                rows: 24,
            })
            .await?;
        viewers.push(extra);
    }

    let before = sample(opts.pid)?;
    let start = Instant::now();
    let mut handles = Vec::new();
    for mut viewer in viewers {
        handles.push(tokio::spawn(async move {
            // Each viewer's own session ends with an `Exited` frame; the extra
            // fanout viewers on session 0 see the same one.
            let _ = viewer
                .until(|msg| matches!(msg, ServerMsg::Exited { .. }))
                .await;
            viewer.output_bytes
        }));
    }
    let mut bytes = 0u64;
    for handle in handles {
        bytes += handle.await?;
    }
    let wall = start.elapsed();
    let after = sample(opts.pid)?;
    report(
        &format!(
            "stream: {} sessions x {} MiB, {} extra viewers",
            opts.sessions, opts.mib, opts.viewers
        ),
        before,
        after,
        wall,
        (opts.sessions * opts.mib * 1024 * 1024) as u64,
        bytes,
    );
    close_all(&mut control, &ids).await
}

/// One session, one unbroken run of output.
async fn burst(opts: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut control = Client::connect(opts.port).await?;
    let id = control.create(0, &emitter(opts.mib)).await?;
    let mut viewer = Client::connect(opts.port).await?;
    viewer
        .send(&ClientMsg::Attach {
            session: id,
            cols: 80,
            rows: 24,
        })
        .await?;

    let before = sample(opts.pid)?;
    let start = Instant::now();
    viewer
        .until(|msg| matches!(msg, ServerMsg::Exited { .. }))
        .await?;
    let wall = start.elapsed();
    let after = sample(opts.pid)?;
    report(
        &format!("burst: 1 session x {} MiB", opts.mib),
        before,
        after,
        wall,
        (opts.mib * 1024 * 1024) as u64,
        viewer.output_bytes,
    );
    close_all(&mut control, &[id]).await
}

/// Sessions that emit nothing at all, held for `--secs`.
///
/// The children sleep rather than exit, so the daemon really is holding N live
/// PTYs with N live coalescers for the whole window.
async fn idle(opts: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut control = Client::connect(opts.port).await?;
    let mut ids = Vec::new();
    for i in 0..opts.sessions {
        ids.push(
            control
                .create(i as u64 % 4, &format!("sleep {}", opts.secs + 30))
                .await?,
        );
    }
    // Let the initial settle probe fire before the window opens, so what is
    // measured is steady-state idle rather than session startup.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let before = sample(opts.pid)?;
    let start = Instant::now();
    control
        .drain_until(Instant::now() + Duration::from_secs(opts.secs))
        .await;
    let wall = start.elapsed();
    let after = sample(opts.pid)?;
    report(
        &format!("idle: {} quiet sessions for {} s", opts.sessions, opts.secs),
        before,
        after,
        wall,
        0,
        0,
    );
    println!(
        "switches / session / s  {:.3}",
        (after.switches - before.switches) as f64 / (opts.sessions as f64 * wall.as_secs_f64())
    );
    close_all(&mut control, &ids).await
}

/// Fill every ring, then time one cross-session sweep over all of them.
async fn search(opts: &Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut control = Client::connect(opts.port).await?;
    let mut ids = Vec::new();
    for i in 0..opts.sessions {
        ids.push(control.create(i as u64 % 4, &emitter(opts.mib)).await?);
    }
    // Every child must have finished writing before the sweep, or the sweep
    // measures a half-full ring.
    let mut exited = 0;
    while exited < opts.sessions {
        control
            .until(|msg| matches!(msg, ServerMsg::Exited { .. }))
            .await?;
        exited += 1;
    }

    let before = sample(opts.pid)?;
    let start = Instant::now();
    control
        .send(&ClientMsg::Search {
            sessions: Vec::new(),
            pattern: opts.pattern.clone().into(),
            regex: false,
            case_insensitive: false,
            whole_word: false,
            context_lines: 2,
            max_hits: 100,
        })
        .await?;
    let results = control
        .until(|msg| matches!(msg, ServerMsg::SearchResults { .. }))
        .await?;
    let wall = start.elapsed();
    let after = sample(opts.pid)?;
    if let ServerMsg::SearchResults { hits, truncated, .. } = &results {
        println!("hits {} truncated {truncated}", hits.len());
    }
    report(
        &format!(
            "search: {:?} over {} sessions x {} MiB",
            opts.pattern, opts.sessions, opts.mib
        ),
        before,
        after,
        wall,
        // Only what the rings still hold is swept, however much was emitted.
        (opts.sessions * opts.mib.min(DEFAULT_SCROLLBACK_BYTES / (1024 * 1024)) * 1024 * 1024)
            as u64,
        0,
    );
    close_all(&mut control, &ids).await
}

async fn close_all(
    control: &mut Client,
    ids: &[SessionId],
) -> Result<(), Box<dyn std::error::Error>> {
    for id in ids {
        control.send(&ClientMsg::Close { session: *id }).await?;
    }
    Ok(())
}
