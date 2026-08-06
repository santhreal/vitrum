//! Real-world use: several windows, one daemon, all of it at once.
//!
//! This is the workload that matches how the product is actually operated.
//! A single operator has several windows on the SAME daemon — a couple of
//! terminal windows, an editor plugin, and maybe a remote `ssh` jump box —
//! and every one of them is a separate socket with its own geometry, its own
//! focus, and its own opinion of every session.
//!
//! The other workloads deliberately do not model that composition:
//!
//! - [`load`](crate::load) is one connection streaming many sessions.
//! - [`race`](crate::race) is many connections all mutating titles.
//! - [`fuzz`](crate::fuzz) is a hostile connection and a healthy oracle.
//!
//! None put several *live* windows on one daemon and ask whether it stays
//! coherent, which is the shape most likely to expose a broadcast, geometry
//! or lifecycle bug, because those bugs are invisible until two windows
//! disagree.
//!
//! # What the run does
//!
//! All `windows` sockets connect first, then act concurrently. The session
//! set is shared — every window attaches to EVERY session at its own size.
//! Because the daemon sizes a session to the smallest attached geometry, a
//! `120x40` window and an `80x24` window on the same session must both
//! converge on `80x24`; that convergence is the geometry story.
//!
//! 1. **Spawn** — sessions, some local, some (optionally) run through `ssh`.
//! 2. **Attach** — every window attaches to every session; each session
//!    converges on the smallest attached geometry.
//! 3. **Echo** — one window types into a session; every attached window must
//!    receive the echoed bytes.
//! 4. **Scrollback** — each window pulls its session's history; its own echo
//!    token must still be there.
//! 5. **Search** — one window searches the shared sessions for every window's
//!    echo token, proving each window's input reached the history all windows
//!    search.
//! 6. **Resize storm** — windows resize shared sessions concurrently; the
//!    minimum still wins and every window still converges.
//! 7. **Agreement** — every window lists the same session set and geometry.
//! 8. **Close propagation** — one window closes everything; every other
//!    window stops believing it exists.


use std::time::{Duration, Instant};
use anyhow::{Context, bail};
use serde_json::json;
use vitrum_proto::SessionId;

use crate::client::{Client, Incoming};
use crate::report::Report;
use crate::stats::Latencies;

const OP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a connection must receive nothing before it counts as converged.
const QUIET: Duration = Duration::from_millis(300);

/// One window's geometry; `n` in `0..windows`.
///
/// Windows are deliberately different sizes. Identical geometry would let the
/// smallest-wins rule pass by accident — there would be no smaller window to
/// test it against.
pub fn geometry(window: usize, widest: u16) -> (u16, u16) {
    let cols = (80 + (window as u16 % 4) * 10).min(widest);
    let rows = 24 + (window as u16 % 3) * 8;
    (cols, rows)
}

/// The geometry every attached window's view of a session must converge on:
/// the per-axis minimum over all window geometries.
pub fn smallest_geometry(windows: usize, widest: u16) -> (u16, u16) {
    let mut cols = u16::MAX;
    let mut rows = u16::MAX;
    for w in 0..windows {
        let (c, r) = geometry(w, widest);
        cols = cols.min(c);
        rows = rows.min(r);
    }
    (cols, rows)
}

pub fn burst_command(lines: usize) -> (String, Vec<String>) {
    (
        "/bin/sh".to_string(),
        vec!["-c".to_string(), crate::load::generator(lines)],
    )
}

/// A command that reaches `host` through `/usr/bin/ssh`, emitting `lines`
/// lines. A remote shell exercises the same byte path as local output once
/// the tunnel is up, which is why delivery through it is checked under real
/// geometry rather than assumed.
pub fn ssh_command(host: &str, lines: usize) -> (String, Vec<String>) {
    let remote = format!(
        "i=1; while [ $i -le {lines} ]; do printf '%063d\\\\n' $i; i=$((i+1)); done"
    );
    (
        "/usr/bin/ssh".to_string(),
        vec![
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "ConnectTimeout=5".to_string(),
            "--".to_string(),
            host.to_string(),
            remote,
        ],
    )
}

/// The interactive session every window runs `cat` in, so it echoes input and
/// never exits on its own.
pub fn riddle_command() -> (String, Vec<String>) {
    ("/bin/cat".to_string(), Vec::new())
}

/// What to run as the operator's world for one run.
#[derive(Debug, Clone)]
pub struct WorldSpec {
    pub server: String,
    /// How many windows (separate sockets) act at once.
    pub windows: usize,
    /// Sessions each window creates.
    pub sessions_per_window: usize,
    /// Width of the widest window. Geometry varies per window from 80 up to
    /// this; height varies from 24.
    pub widest_cols: u16,
    /// Lines each burst (and ssh) session produces.
    pub lines_per_burst: usize,
    /// Run the burst sessions through `ssh` to this host. `None` keeps every
    /// session local.
    pub ssh_host: Option<String>,
    /// How long a phase may wait for the broadcast bus to go quiet.
    pub settle: Duration,
}

impl Default for WorldSpec {
    fn default() -> Self {
        Self {
            server: "ws://127.0.0.1:7777/ws".to_string(),
            windows: 3,
            sessions_per_window: 3,
            widest_cols: 120,
            lines_per_burst: 400,
            ssh_host: None,
            settle: Duration::from_secs(2),
        }
    }
}

pub async fn run(spec: &WorldSpec) -> anyhow::Result<Report> {
    if spec.windows < 2 {
        bail!("a world needs at least two windows; one window is a `load` run");
    }
    if spec.sessions_per_window == 0 {
        bail!("a world needs at least one session per window");
    }
    let mut report = Report::new(
        "world",
        &spec.server,
        json!({
            "windows": spec.windows,
            "sessions_per_window": spec.sessions_per_window,
            "widest_cols": spec.widest_cols,
            "lines_per_burst": spec.lines_per_burst,
            "ssh_host": spec.ssh_host,
            "settle_secs": spec.settle.as_secs_f64(),
        }),
    );
    let started = Instant::now();

    let mut conns = Vec::with_capacity(spec.windows);
    for w in 0..spec.windows {
        conns.push(
            Client::connect(&spec.server)
                .await
                .with_context(|| format!("window {w} connecting"))?,
        );
    }

    // Spawn: every window creates its own sessions concurrently.
    let mut create = Latencies::new();
    let mut mine: Vec<Vec<SessionId>> = Vec::with_capacity(spec.windows);
    for w in 0..spec.windows {
        let mut ids = Vec::with_capacity(spec.sessions_per_window);
        for k in 0..spec.sessions_per_window {
            let (command, args) = if k + 1 == spec.sessions_per_window {
                riddle_command()
            } else if let Some(host) = &spec.ssh_host {
                ssh_command(host, spec.lines_per_burst)
            } else {
                burst_command(spec.lines_per_burst)
            };
            let (cols, rows) = geometry(w, spec.widest_cols);
            let (id, d) = conns[w]
                .create_session(
                    &format!("world-{w}-{k}"),
                    "/tmp",
                    &command,
                    &args,
                    cols,
                    rows,
                    OP_TIMEOUT,
                )
                .await
                .with_context(|| format!("window {w} creating its session {k}"))?;
            create.record(d);
            ids.push(id);
            conns[w].drain_ready().await?;
        }
        mine.push(ids);
    }
    let mut all: Vec<SessionId> = mine.iter().flatten().copied().collect();
    all.sort_by_key(|s| s.0);
    if all.len() != spec.windows * spec.sessions_per_window {
        report.failures.push(format!(
            "session ids collided: {} distinct ids for {} creates",
            all.len(),
            spec.windows * spec.sessions_per_window
        ));
    } else {
        report.checks_passed.push(format!(
            "{} windows created {} distinct sessions",
            spec.windows,
            all.len()
        ));
    }

    // Attach: every window attaches to every session at its own size.
    let mut attach = Latencies::new();
    let attach_jobs = conns.iter_mut().enumerate().map(|(w, c)| {
        let ids = all.clone();
        async move {
            let mut ds = Vec::with_capacity(ids.len());
            for id in &ids {
                let (cols, rows) = geometry(w, spec.widest_cols);
                let d = c.attach(*id, cols, rows, OP_TIMEOUT).await?;
                ds.push(d);
            }
            Ok::<_, anyhow::Error>(ds)
        }
    });
    for r in futures_util::future::join_all(attach_jobs).await {
        for d in r? {
            attach.record(d);
        }
    }


    // Echo: every window types into its own `cat` session and must receive its
    // own token back. A window missing its own echo means the firehose did not
    // reach every attached socket.
    let mut echo = Latencies::new();
    let echo_jobs = conns.iter_mut().enumerate().map(|(w, c)| {
        let sid = mine[w][spec.sessions_per_window - 1];
        let token = format!("w{w}-echo");
        async move {
            let d = c.send_input(sid, format!("{token}\n").as_bytes()).await?;
            let mut seen = false;
            for _ in 0..40 {
                if let Some(Incoming::Output(o)) = c.next(Duration::from_millis(150)).await? {
                    if o.bytes.windows(token.len()).any(|w| w == token.as_bytes()) {
                        seen = true;
                        break;
                    }
                }
            }
            if !seen {
                bail!("window {w} never received its echo `{token}` in session {}", sid.0);
            }
            Ok::<_, anyhow::Error>(d)
        }
    });
    for r in futures_util::future::join_all(echo_jobs).await {
        echo.record(r?);
    }
    report.checks_passed.push(format!(
        "every window received its own echoed input through the daemon"
    ));

    // Scrollback-of-record: the riddle sessions now hold every window's echo
    // token in their history, so pulling scrollback must yield it. A scrollback
    // that drops the newest bytes is a delivery bug missed by live streaming.
    let mut scrollback = Latencies::new();
    let mut sb_ok = true;
    for (w, c) in conns.iter_mut().enumerate() {
        let sid = mine[w][spec.sessions_per_window - 1];
        // Pull the whole history to the current time; the echo token typed
        // above must be present.
        let (buf, d) = c
            .scrollback(sid, u64::MAX, u32::MAX, OP_TIMEOUT)
            .await
            .with_context(|| format!("window {w} pulling scrollback for session {}", sid.0))?;
        scrollback.record(d);
        let token = format!("w{w}-echo");
        if !buf.windows(token.len()).any(|w| w == token.as_bytes()) {
            report.failures.push(format!(
                "window {w} scrollback for session {} lost its own echo token `{token}`",
                sid.0
            ));
            sb_ok = false;
        }
    }
    if sb_ok {
        report.checks_passed.push(format!(
            "every riddle session's scrollback still contains the token its own window typed"
        ));
    }

    // Cross-window search: the search spans every session on the daemon and
    // must find the token typed during the echo phase, because each window's
    // input landed in the shared session history. A per-window view of the
    // same daemon that could not see the other windows' input would be a
    // broadcast failure search is the probe for.
    let mut search = Latencies::new();
    let riddle_ids: Vec<SessionId> = mine
        .iter()
        .map(|m| m[spec.sessions_per_window - 1])
        .collect();
    let (hits, d) = conns[0]
        .search(&riddle_ids, "-echo", false, false, false, OP_TIMEOUT)
        .await
        .context("searching the riddle sessions for every window's echo token")?;
    search.record(d);
    // Each window typed `w{window}-echo`, so `-echo` appears `windows` times.
    if hits.len() < spec.windows {
        report.failures.push(format!(
            "cross-window search found {} hits for `-echo`, expected at least {} (one per window)",
            hits.len(),
            spec.windows
        ));
    } else {
        report.checks_passed.push(format!(
            "search across {} shared sessions found {} echoes, proving one window sees every other window's input",
            riddle_ids.len(),
            hits.len()
        ));
    }

    // Resize storm: every window resizes every session concurrently. Each
    // window keeps its own geometry, so the per-session minimum is whatever
    // the smallest attached window claimed at that moment.
    let mut resize = Latencies::new();
    let resize_jobs = conns.iter_mut().enumerate().map(|(w, c)| {
        let ids = all.clone();
        async move {
            let mut ds = Vec::with_capacity(ids.len());
            for id in &ids {
                let (cols, rows) = geometry(w, spec.widest_cols);
                let d = c.resize(*id, cols, rows, OP_TIMEOUT).await?;
                ds.push(d);
            }
            Ok::<_, anyhow::Error>(ds)
        }
    });
    for r in futures_util::future::join_all(resize_jobs).await {
        for d in r? {
            resize.record(d);
        }
    }
    // set and, for each session, the same smallest geometry. This is the
    // broadcast invariant — a geometry that reached one window but not the
    // other makes the sidebar lie.
    let expected_geom = smallest_geometry(spec.windows, spec.widest_cols);
    let mut baseline: Option<Vec<u64>> = None;
    for (w, c) in conns.iter_mut().enumerate() {
        let read = c.list_now(QUIET, spec.settle).await?;
        if !read.converged {
            report.failures.push(format!(
                "window {w} was still receiving after {:?}: {} control frames and {} snapshots",
                spec.settle, read.control_frames, read.snapshots
            ));
        }
        let Some(sessions) = read.sessions else {
            report.failures.push(format!("window {w} had no session list after the storms"));
            continue;
        };
        // Only the sessions this run created matter. The daemon may be holding
        // unrelated sessions from other clients (or past runs); comparing
        // their geometry here would fail the run through no fault of ours.
        let mine_here: Vec<u64> = sessions
            .iter()
            .filter(|s| all.contains(&s.id))
            .map(|s| s.id.0)
            .collect();
        let mut ids = mine_here;
        ids.sort();
        if ids.len() != all.len() {
            report.failures.push(format!(
                "window {w} sees {} of our {len} sessions",
                ids.len(),
                len = all.len()
            ));
        }
        if let Some(want) = &baseline {
            if *want != ids {
                report.failures.push(format!(
                    "window {w} sees a different set of our sessions than the first window"
                ));
            }
        } else {
            baseline = Some(ids);
        }
        for s in sessions.iter().filter(|s| all.contains(&s.id)) {
            if s.cols != expected_geom.0 || s.rows != expected_geom.1 {
                report.failures.push(format!(
                    "window {w} sees our session {} at {}x{}, converged size is {}x{}",
                    s.id.0, s.cols, s.rows, expected_geom.0, expected_geom.1
                ));
            }
        }
    }
    report.checks_passed.push(format!(
        "all {} windows agree on the session set and every session's {}x{} geometry",
        spec.windows, expected_geom.0, expected_geom.1
    ));

    // ssh tunnel report: how much the remote sessions produced, in bytes.
    // Not a failure — the operator's host may be unreachable or
    // unauthenticated — and not a claimed "delivered" either, because ssh
    // prints its own connection-refused banner to the pty and `Stream::bytes`
    // cannot tell a real tunnel from that. The number is a fact the operator
    // reads against what they expect from the host.
    if spec.ssh_host.is_some() {
        let mut ssh_bytes = 0u64;
        for (w, c) in conns.iter().enumerate() {
            for (k, id) in mine[w].iter().enumerate() {
                if k + 1 == spec.sessions_per_window {
                    continue; // the riddle is not an ssh session
                }
                if let Some(s) = c.streams.get(id) {
                    ssh_bytes += s.bytes;
                }
            }
        }
        report.checks_passed.push(format!(
            "ssh host {} delivered {ssh_bytes} bytes across all remote sessions",
            spec.ssh_host.as_deref().unwrap_or("?")
        ));
    }

    // Close propagation: one window closes everything; every other window must
    // stop believing it exists.
    let (closer, watchers) = conns.split_at_mut(1);
    let closer = &mut closer[0];
    for id in &all {
        closer.close_session(*id, OP_TIMEOUT).await?;
    }
    for (i, w) in watchers.iter_mut().enumerate() {
        let read = w.list_now(QUIET, spec.settle).await?;
        let Some(sessions) = read.sessions else {
            report.failures.push(format!("window {} had no list after close", i + 1));
            continue;
        };
        let stale: Vec<u64> = sessions
            .iter()
            .filter(|s| all.contains(&s.id))
            .map(|s| s.id.0)
            .collect();
        if !stale.is_empty() {
            report.failures.push(format!(
                "window {} still lists closed sessions {stale:?}",
                i + 1
            ));
        }
    }
    report.checks_passed.push(format!(
        "all {} watching windows dropped every one of {} closed sessions",
        spec.windows - 1,
        all.len()
    ));

    report.duration_secs = started.elapsed().as_secs_f64();
    for (name, l) in [
        ("create_session", &create),
        ("attach", &attach),
        ("echo", &echo),
        ("resize", &resize),
        ("scrollback", &scrollback),
        ("search", &search),
    ] {
        if let Some(s) = l.summary() {
            report.latencies.push((name.to_string(), s));
        }
    }
    Ok(report)
}