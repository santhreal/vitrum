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
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use vitrum_proto::{ClientMsg, SessionId};

use crate::client::{Client, Incoming};
use crate::report::Report;
use crate::stats::{Dist, Latencies};

const OP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a connection must receive nothing before it counts as converged.
const QUIET: Duration = Duration::from_millis(300);
/// How long one keystroke may wait for its own bytes to come back before the
/// run says the daemon stopped answering rather than recording a slow sample.
const ECHO_TIMEOUT: Duration = Duration::from_secs(10);

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

pub fn burst_script(lines: usize) -> String {
    crate::load::generator(lines)
}

/// A command that reaches `host` through `/usr/bin/ssh`, emitting `lines`
/// lines. A remote shell exercises the same byte path as local output once
/// the tunnel is up, which is why delivery through it is checked under real
/// geometry rather than assumed.
pub fn ssh_script(host: &str, lines: usize) -> String {
    let remote = crate::load::generator(lines);
    // The remote command carries the quotes `printf '%063d\n'` needs, so it is
    // quoted for the local shell rather than pasted into it.
    format!(
        "exec /usr/bin/ssh -o BatchMode=yes -o ConnectTimeout=5 -- {host} {}",
        sh_quote(&remote)
    )
}

/// `text` as one argument to a POSIX shell.
///
/// Single quotes protect everything except a single quote, which is closed,
/// escaped and reopened. The remote generator contains them.
pub(crate) fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The interactive session every window runs `cat` in, so it echoes input and
/// never exits on its own.
pub fn riddle_script() -> String {
    "exec /bin/cat".to_string()
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
    /// Keystrokes timed in each condition.
    ///
    /// The interesting figure is a tail, and a tail needs samples: a hundred
    /// round trips have no p99 worth printing.
    pub keystroke_samples: usize,
    /// Sessions streaming into the focused window while its keystrokes are
    /// timed. Seven is a working operator's other tabs.
    pub stream_sessions: usize,
    /// Lines each of those produces. Enough to outlast the measurement rather
    /// than a count anyone reads.
    pub stream_lines: usize,
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
            keystroke_samples: 400,
            stream_sessions: 7,
            stream_lines: 200_000,
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
            let script = if k + 1 == spec.sessions_per_window {
                riddle_script()
            } else if let Some(host) = &spec.ssh_host {
                ssh_script(host, spec.lines_per_burst)
            } else {
                burst_script(spec.lines_per_burst)
            };
            let (cols, rows) = geometry(w, spec.widest_cols);
            let (id, d) = conns[w]
                .create_session(
                    &format!("world-{w}-{k}"),
                    &script,
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

    // -----------------------------------------------------------------------
    // The number the operator feels: one keystroke in the focused window,
    // measured quiet, then measured again with the rest of the world running.
    //
    // Averages are not the answer here. A window that is usually fast and
    // occasionally waits 80 ms for a keystroke is a window that feels broken,
    // and only the tail of the distribution says so.
    // -----------------------------------------------------------------------
    let focused = 0usize;
    // Measured before anything is typed: the floor does not depend on the
    // daemon, and taking it first keeps it out of the streaming window where
    // it would be measuring the load instead of the platform.
    let platform_floor = floor(&spec.server, spec.keystroke_samples.min(2000), 64).await?;

    let (typed, _) = conns[focused]
        .create_session("world-typing", &riddle_script(), 120, 40, OP_TIMEOUT)
        .await
        .context("creating the focused window's typing session")?;
    conns[focused].attach(typed, 120, 40, OP_TIMEOUT).await?;
    let mut measured: Vec<SessionId> = vec![typed];
    let mut keystrokes_out = Vec::new();

    let (ns, foreign) = keystrokes(&mut conns[focused], typed, spec.keystroke_samples).await?;
    keystrokes_out.push(row("quiet", ns, &platform_floor, foreign, 0)?);

    // The other tabs. Created from the other windows, because that is where an
    // operator's other work lives, and attached here, because the focused
    // window is showing them too and its socket carries their bytes.
    for k in 0..spec.stream_sessions {
        let owner = 1 + (k % (spec.windows - 1));
        let (id, _) = conns[owner]
            .create_session(
                &format!("world-stream-{k}"),
                &burst_script(spec.stream_lines),
                120,
                40,
                OP_TIMEOUT,
            )
            .await
            .with_context(|| format!("creating streaming session {k}"))?;
        conns[focused].attach(id, 120, 40, OP_TIMEOUT).await?;
        measured.push(id);
    }
    let (ns, foreign) = keystrokes(&mut conns[focused], typed, spec.keystroke_samples).await?;
    if spec.stream_sessions > 0 && foreign == 0 {
        report.failures.push(
            "no other session's output crossed the focused socket while its keystrokes were \
             timed, so the loaded figure was measured on an idle daemon"
                .to_string(),
        );
    }
    keystrokes_out.push(row(
        "loaded",
        ns,
        &platform_floor,
        foreign,
        spec.stream_sessions,
    )?);

    // The same keystroke, through a session that lives on another machine.
    // What ssh adds is the difference between this row and the loaded one, and
    // it is a difference rather than a claim about the network.
    if let Some(host) = &spec.ssh_host {
        let (id, _) = conns[focused]
            .create_session(
                "world-ssh-typing",
                &format!("exec /usr/bin/ssh -tt {} /bin/cat", sh_quote(host)),
                120,
                40,
                OP_TIMEOUT,
            )
            .await
            .with_context(|| format!("creating an ssh session to {host}"))?;
        conns[focused].attach(id, 120, 40, OP_TIMEOUT).await?;
        measured.push(id);
        match keystrokes(&mut conns[focused], id, spec.keystroke_samples).await {
            Ok((ns, foreign)) => keystrokes_out.push(row(
                "ssh",
                ns,
                &platform_floor,
                foreign,
                spec.stream_sessions,
            )?),
            // An unreachable or unauthenticated host is the operator's
            // environment, not a defect in the daemon. It is recorded as a
            // condition that produced no figure rather than failing the run.
            Err(e) => report.checks_passed.push(format!(
                "no ssh keystroke figure: the session to {host} never echoed ({e:#})"
            )),
        }
    }

    for id in &measured {
        conns[focused].close_session(*id, OP_TIMEOUT).await?;
    }

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
    // The keystroke figures are the point of the run, and a caller reading
    // JSON should not have to reconstruct what ssh cost: the difference the
    // conditions were measured to answer is stated, not left as an exercise.
    let p50_of = |c: &str| -> Option<u64> {
        keystrokes_out
            .iter()
            .find(|k| k.condition == c)
            .map(|k| k.raw_ns.p50)
    };
    let load_added = match (p50_of("quiet"), p50_of("loaded")) {
        (Some(q), Some(l)) => Some(l.saturating_sub(q)),
        _ => None,
    };
    let ssh_added = match (p50_of("loaded"), p50_of("ssh")) {
        (Some(l), Some(s)) => Some(s.saturating_sub(l)),
        _ => None,
    };
    report.extra = json!({
        "keystrokes": keystrokes_out,
        "floor": platform_floor,
        "load_added_p50_ns": load_added,
        "ssh_added_p50_ns": ssh_added,
    });
    Ok(report)
}

// ---------------------------------------------------------------------------
// What the focused window feels while the rest of the world runs
// ---------------------------------------------------------------------------

/// One condition's keystroke distribution.
///
/// Both the raw figure and the figure with the platform floor taken off, and a
/// flag saying whether it was taken off at all, because subtracting a locally
/// measured floor from a run against a daemon on another machine would invent
/// a number.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keystroke {
    /// `quiet`, `loaded`, or `ssh`.
    pub condition: String,
    /// Round trips, in nanoseconds: the byte leaving the socket to the same
    /// byte arriving back on it.
    pub raw_ns: Dist,
    /// The same distribution with [`Floor::total_ns`] subtracted, or `None`
    /// when the floor does not apply to this server.
    pub net_ns: Option<Dist>,
    /// Output frames for other sessions that crossed this socket while the
    /// samples were being taken. This is what "under load" is worth: a zero
    /// here means the background sessions were not actually streaming.
    pub foreign_frames: u64,
    /// Sessions streaming into this window while the samples were taken.
    pub streaming_sessions: usize,
}

/// What the platform charges for the same round trip with no vitrum in it.
///
/// Two components, both measured on the machine running the harness, in the
/// same run as the figures they qualify:
///
/// - a pseudoterminal echo, which is the kernel line discipline turning a
///   written byte around. Every keystroke crosses it.
/// - a loopback TCP round trip of the same payload, which is the kernel and
///   the scheduler moving a small frame between two processes. Every keystroke
///   crosses one of those in each direction, and both directions are on the
///   one socket, so one round trip is the whole of it.
///
/// Their sum is the floor. It is not an estimate of the daemon's cost and it
/// is not subtracted from anything unless the daemon is on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Floor {
    pub pty_echo_ns: Dist,
    pub loopback_ns: Dist,
    /// The two medians added.
    pub total_ns: u64,
    /// Whether it was subtracted from the keystroke figures.
    pub subtracted: bool,
    /// Why, in one line, so a reader of the JSON does not have to infer it.
    pub note: String,
}

/// Whether `server` names a daemon on this machine.
///
/// Only then does a locally measured floor describe the same hops the
/// keystroke crossed.
pub fn server_is_local(server: &str) -> bool {
    let after_scheme = server.split_once("://").map_or(server, |(_, rest)| rest);
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// Every sample with `floor` taken off, saturating at zero.
///
/// Saturating rather than signed: a sample below the floor means the two
/// measurements disagreed by less than their own noise, and a negative latency
/// in a report is worse than a zero.
pub(crate) fn subtract(samples: &[u64], floor: u64) -> anyhow::Result<Dist> {
    Dist::of(samples.iter().map(|s| s.saturating_sub(floor)).collect())
}

/// Time `samples` keystrokes on `session`, counting everything else that
/// crossed the socket meanwhile.
///
/// Each sample writes a token nothing else can produce and waits for that
/// token to come back, so a measurement cannot be satisfied by another
/// session's output arriving at the right moment. The echo may be split across
/// frames, so the search is over the bytes accumulated for this session rather
/// than over one frame.
async fn keystrokes(
    conn: &mut Client,
    session: SessionId,
    samples: usize,
) -> anyhow::Result<(Vec<u64>, u64)> {
    let mut out = Vec::with_capacity(samples);
    let mut foreign = 0u64;
    let mut pending: Vec<u8> = Vec::with_capacity(4096);
    for i in 0..samples {
        let token = format!("vk{i:07}");
        pending.clear();
        let start = Instant::now();
        conn.send(&ClientMsg::Input {
            session,
            data: format!("{token}\n").into_bytes(),
        })
        .await?;
        let deadline = start + ECHO_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                bail!(
                    "keystroke {i} on session {} was never echoed within {ECHO_TIMEOUT:?}",
                    session.0
                );
            }
            match conn.next(left).await? {
                Some(Incoming::Output(o)) if o.session == session => {
                    pending.extend_from_slice(&o.bytes);
                    if pending
                        .windows(token.len())
                        .any(|w| w == token.as_bytes())
                    {
                        out.push(start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64);
                        break;
                    }
                }
                Some(Incoming::Output(_)) => foreign += 1,
                Some(Incoming::Control(_)) => {}
                None => continue,
            }
        }
    }
    Ok((out, foreign))
}

/// A loopback TCP round trip of `payload` bytes, `samples` times.
///
/// A real listener and a real connection over 127.0.0.1, because the question
/// is what the kernel and the scheduler charge to move a small frame between
/// two endpoints, and an in-process channel would answer a different one.
async fn loopback(samples: usize, payload: usize) -> anyhow::Result<Vec<u64>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .context("binding a loopback listener for the floor measurement")?;
    let addr = listener.local_addr()?;
    let echo = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await?;
        sock.set_nodelay(true)?;
        let mut buf = vec![0u8; 4096];
        loop {
            let n = sock.read(&mut buf).await?;
            if n == 0 {
                return Ok::<(), std::io::Error>(());
            }
            sock.write_all(&buf[..n]).await?;
        }
    });

    let mut sock = tokio::net::TcpStream::connect(addr)
        .await
        .context("connecting to the loopback listener")?;
    sock.set_nodelay(true)?;
    let msg = vec![b'k'; payload.max(1)];
    let mut buf = vec![0u8; msg.len()];
    let mut out = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        sock.write_all(&msg).await?;
        let mut got = 0;
        while got < msg.len() {
            let n = sock.read(&mut buf[got..]).await?;
            if n == 0 {
                bail!("the loopback echo closed mid-measurement");
            }
            got += n;
        }
        out.push(start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64);
    }
    drop(sock);
    // The echo task ends when the connection closes; joining it keeps a
    // measurement from leaving a task behind for the next one to be charged.
    let _ = echo.await;
    Ok(out)
}

/// The floor for this run.
async fn floor(server: &str, samples: usize, payload: usize) -> anyhow::Result<Floor> {
    let pty = crate::latency::pty_echo(samples)?;
    let loop_ns = Dist::of(loopback(samples, payload).await?)?;
    let local = server_is_local(server);
    Ok(Floor {
        total_ns: pty.p50 + loop_ns.p50,
        pty_echo_ns: pty,
        loopback_ns: loop_ns,
        subtracted: local,
        note: if local {
            "the daemon is on this machine, so the locally measured floor is the floor these \
             keystrokes crossed and it is subtracted"
                .to_string()
        } else {
            format!(
                "the daemon at {server} is not on this machine, so the local floor describes \
                 different hops and is reported without being subtracted"
            )
        },
    })
}

/// One measured condition, with the floor taken off when it applies.
fn row(
    condition: &str,
    ns: Vec<u64>,
    floor: &Floor,
    foreign: u64,
    streaming: usize,
) -> anyhow::Result<Keystroke> {
    let net_ns = if floor.subtracted {
        Some(subtract(&ns, floor.total_ns)?)
    } else {
        None
    };
    Ok(Keystroke {
        condition: condition.to_string(),
        raw_ns: Dist::of(ns)?,
        net_ns,
        foreign_frames: foreign,
        streaming_sessions: streaming,
    })
}