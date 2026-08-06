//! PTY-backed sessions and the registry that owns them.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use vitrum_model::hint::HintDeclaration;
use vitrum_proto::{Attention, SessionId, SessionInfo, SessionStatus, display_safe};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::time::{Instant, timeout_at};

use crate::Scrollback;
use crate::scan::OutputScan;

/// Size of one blocking PTY read. Large enough that a firehose is drained in a
/// few syscalls rather than hundreds.
const READ_CHUNK: usize = 32 * 1024;

/// How long output may sit in the coalescing buffer before it is published.
///
/// This is the whole reason a 0.4 MB/s firehose does not become thousands of
/// broadcast wakeups per second. The timer is armed only while bytes are
/// actually pending, so an idle session costs zero wakeups and zero CPU.
const FLUSH_WINDOW: Duration = Duration::from_millis(6);

/// Publish early once this much output is pending, so a burst is not delayed by
/// the full window and no single chunk grows unbounded.
const FLUSH_BYTES: usize = 64 * 1024;

/// How long a session must be quiet before its foreground process is worth
/// asking about.
///
/// Asking mid-burst would answer about a process that is obviously working and
/// would do it hundreds of times a second. Asking after a short silence answers
/// the question that matters, which is what the agent settled INTO, and does it
/// once per burst. Nothing here is periodic: the timer is armed by activity and
/// disarmed by the answer, so a session that has settled costs nothing at all
/// until it produces output again.
pub(crate) const SETTLE_WINDOW: Duration = Duration::from_millis(150);

/// Chunks of slack per session on the live output channel.
///
/// With `FLUSH_BYTES` this bounds what a stalled client can pin to 4 MB per
/// session, and a client that falls further behind is told about the gap rather
/// than being buffered for indefinitely. Buffering more would reproduce the
/// competitor failure mode of retaining full history in the client path.
const OUTPUT_CHANNEL_CHUNKS: usize = 64;

/// One client's attachments, for tracking geometry per viewer.
///
/// Several windows can look at one session, and a session has exactly one PTY
/// with exactly one size. Whose layout wins therefore has to be a decision
/// rather than an accident, and the answer is "nobody's, the smallest one".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViewerId(pub u64);

/// Everything needed to spawn one terminal session.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub project_id: vitrum_proto::ProjectId,
    pub cwd: PathBuf,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    pub title: Option<String>,
}

/// One coalesced run of PTY output.
///
/// `seq` is the byte offset of `data[0]` in the session's cumulative output
/// stream, matching the data-plane frame header, so a client can detect a gap
/// by arithmetic instead of trusting the transport.
#[derive(Clone, Debug)]
pub struct OutputChunk {
    pub seq: u64,
    pub data: Arc<[u8]>,
}

/// Server-side state for one live or exited session.
pub(crate) struct Session {
    pub(crate) id: SessionId,
    pub(crate) info: RwLock<SessionInfo>,
    pub(crate) scrollback: Mutex<Scrollback>,
    /// Live output fan-out. Kept even with zero receivers so `subscribe` after
    /// the fact is cheap.
    output: broadcast::Sender<OutputChunk>,
    /// Status change notifications. Lets a client learn about an exit without
    /// polling, which is the difference between 0% and permanent idle CPU.
    status: watch::Sender<SessionStatus>,
    /// Bumped whenever the projection changed for a reason `status` does not
    /// report: a new answer from the foreground probe, a hint the agent
    /// declared, or a geometry change caused by some other window attaching or
    /// leaving. Without it a second window's sidebar would silently go stale.
    observations: watch::Sender<u64>,
    pub(crate) master: Mutex<Box<dyn MasterPty + Send>>,
    /// Geometry each attached client can draw, keyed by viewer.
    ///
    /// Also the attachment set: a viewer with no entry here is not drawing this
    /// session and does not constrain its size.
    viewers: Mutex<BTreeMap<ViewerId, (u16, u16)>>,
    /// How many times the PTY has actually been resized. Diagnostic, and what
    /// makes "two windows converge instead of fighting" an assertion rather
    /// than an opinion.
    resizes: AtomicU64,
    /// How many times the foreground has been probed. Diagnostic, and the only
    /// honest way to show that nothing here runs on a timer: a session left
    /// alone must hold this count still forever.
    probes: AtomicU64,
    /// Raised when the operator does something the probe's last answer may not
    /// survive, so a settled session is re-examined without a periodic tick.
    activity: Notify,
    /// Queue to the dedicated writer thread. Input is queued rather than
    /// written inline because a child that has stopped reading its stdin makes
    /// a PTY write block indefinitely, which would wedge a runtime worker.
    ///
    /// Dropped when the session reaches a terminal state, which is what lets the
    /// writer thread exit: an exited session can never accept input again, and a
    /// thread parked on this queue for every finished session would accumulate
    /// for as long as the daemon runs.
    input: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// When the operator last had eyes on this session, as a Unix millisecond.
    ///
    /// Server-side only: it qualifies `Attention` rather than being drawn, so it
    /// is not part of the wire projection. Zero means never focused.
    last_focus_ms: AtomicU64,
    /// Process id of the child this PTY was opened for, when the platform
    /// reported one.
    ///
    /// Captured once at spawn and never refreshed, which fixes exactly what it
    /// means: it identifies the child **while that child is alive**. Once the
    /// session reaches a terminal state the number is a reaped pid that the
    /// kernel is free to hand to an unrelated process, so every reader must
    /// gate on the session still being live. `SessionManager::child_pid`
    /// enforces that; nothing else may read this field.
    ///
    /// `None` is a real outcome, not a failure to try. A platform that does not
    /// report a child pid leaves this unset, and a caller that needs one must
    /// treat the session as unknown rather than substitute a sentinel: a wrong
    /// pid attributes another process's work to this session, which is worse
    /// than admitting we do not know.
    child_pid: Option<u32>,
}

impl Session {
    /// Record and fan out one coalesced run of output.
    fn publish(&self, data: &[u8], wants_operator: bool, hint: Option<&HintDeclaration>) {
        if data.is_empty() {
            return;
        }
        let seq = {
            let mut sb = lock(&self.scrollback);
            let seq = sb.head_seq();
            // Scrollback is filled whether or not anyone is attached: sessions
            // must survive with no GUI connected, and history is the product.
            sb.push(data);
            seq
        };

        // Skipping the Arc allocation when nothing is attached matters: the
        // normal state of 20 agents is 19 unattached ones. A receiver that
        // appears between this check and the send simply starts at the next
        // chunk and backfills from scrollback, which is what it does anyway.
        let watched = self.output.receiver_count() > 0;

        let now = now_ms();
        if watched {
            // Output a client is receiving has been seen by definition, so the
            // focus timestamp advances with it and keeps `idle_ms` at zero.
            self.last_focus_ms.store(now, Ordering::Relaxed);
        }

        let mut became_running = false;
        let mut declared = false;
        {
            let mut info = write_lock(&self.info);
            info.last_activity_ms = now;
            if !watched {
                info.unread = true;
                // A bell is only a signal until the operator looks; raising it
                // for output they are watching would latch the indicator on and
                // make it meaningless.
                info.attention.bell |= wants_operator;
            }
            if let Some(declaration) = hint {
                // Only a different declaration is news. An agent that repeats
                // `working` every second would otherwise push a full projection
                // to every window every second, for no change at all.
                declared = info.hint.as_ref().is_none_or(|current| {
                    current.state != declaration.state || current.label != declaration.label
                });
                info.hint = Some(declaration.clone().into_hint(now));
            }
            if info.status == SessionStatus::Starting {
                info.status = SessionStatus::Running;
                became_running = true;
            }
        }
        if became_running {
            self.status.send_replace(SessionStatus::Running);
        }
        if declared {
            self.bump();
        }

        if watched {
            let _ = self.output.send(OutputChunk {
                seq,
                data: Arc::from(data),
            });
        }
    }

    /// Ask the operating system what the foreground process is doing, and
    /// record the answer if it is news.
    ///
    /// Only the change is published. A session that settles once and stays
    /// settled produces exactly one update, which is what keeps twenty idle
    /// agents from generating any traffic at all.
    fn observe_foreground(&self) {
        if !read_lock(&self.info).status.is_live() {
            return;
        }
        let waiting = crate::probe::waiting(&**lock(&self.master));
        let changed = {
            let mut info = write_lock(&self.info);
            let changed = info.attention.waiting != waiting;
            info.attention.waiting = waiting;
            changed
        };
        // Counted after the answer is stored, so an observer that sees the
        // count move is guaranteed to see the value that produced it.
        self.probes.fetch_add(1, Ordering::Relaxed);
        if changed {
            self.bump();
        }
    }

    /// Size the PTY to the smallest geometry any attached client can draw.
    ///
    /// Minimum over attachments, the way tmux does it, and not "whoever resized
    /// last". Two windows on one session at different sizes would otherwise
    /// fight forever: each lays out, each resizes, each re-resizes on its next
    /// render, and the child reflows until one of them closes. The minimum is
    /// the only fixed point, because it is the only size every attached window
    /// can actually render without clipping.
    ///
    /// With nothing attached the size is left alone. Reflowing a child for a
    /// terminal nobody is looking at is pure cost, and the next attach sets the
    /// size anyway.
    fn apply_geometry(&self, viewers: &BTreeMap<ViewerId, (u16, u16)>) -> anyhow::Result<()> {
        let Some((cols, rows)) = viewers
            .values()
            .copied()
            .reduce(|(min_cols, min_rows), (cols, rows)| (min_cols.min(cols), min_rows.min(rows)))
        else {
            return Ok(());
        };
        let unchanged = {
            let info = read_lock(&self.info);
            (info.cols, info.rows) == (cols, rows)
        };
        // The anti-thrash property, and the reason `resizes` is worth counting:
        // a client re-sending the geometry it already has costs no ioctl, no
        // reflow in the child, and no update to any other window.
        if unchanged {
            return Ok(());
        }
        lock(&self.master)
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .with_context(|| format!("resizing session {}", self.id.0))?;
        self.resizes.fetch_add(1, Ordering::Relaxed);
        {
            let mut info = write_lock(&self.info);
            info.cols = cols;
            info.rows = rows;
        }
        self.bump();
        Ok(())
    }

    /// Announce that the projection changed for a reason no other channel
    /// reports.
    fn bump(&self) {
        self.observations
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    /// Whether this session can still accept input.
    #[cfg(test)]
    pub(crate) fn input_is_open(&self) -> bool {
        lock(&self.input).is_some()
    }

    /// Move the session to a terminal state and wake anyone watching status.
    fn finish(&self, code: Option<i32>) {
        let status = SessionStatus::Exited { code };
        {
            let mut info = write_lock(&self.info);
            info.status = status.clone();
            // A signal (code None) is a failure too: a killed or crashed agent
            // is exactly what the operator needs to see first.
            info.attention.failed = code != Some(0);
            // There is no foreground process left to be blocked or busy, so the
            // last answer is not merely stale, it has no subject. Leaving
            // `Some(true)` on a dead row would claim the operator is being
            // waited on by something that is gone.
            info.attention.waiting = None;
        }
        // Releases the writer thread; nothing can be written to a dead child.
        lock(&self.input).take();
        self.status.send_replace(status);
    }

    /// The client-facing projection, with derived fields filled in.
    ///
    /// `idle_ms` is derived at read time rather than stored, because a stored
    /// staleness would need a timer to stay true and idle sessions are exactly
    /// the ones that must cost nothing.
    ///
    /// It is silence the operator has NOT seen, not raw time since output. Plain
    /// time-since-output lights up a session that was read five seconds ago and
    /// never turns off, and an indicator that is always on trains people to
    /// ignore it.
    pub(crate) fn snapshot(&self) -> SessionInfo {
        let mut info = read_lock(&self.info).clone();
        let focused = self.last_focus_ms.load(Ordering::Relaxed);
        info.attention.idle_ms = if focused >= info.last_activity_ms {
            0
        } else {
            now_ms().saturating_sub(info.last_activity_ms)
        };
        info
    }
}

/// Owns every PTY, its child process, and its bounded scrollback.
///
/// The GUI is a thin client over this: it holds only the viewport it is
/// showing, so its memory stays flat as the agent count grows.
pub struct SessionManager {
    scrollback_bytes: usize,
    next_id: AtomicU64,
    next_viewer: AtomicU64,
    sessions: RwLock<BTreeMap<SessionId, Arc<Session>>>,
}

impl SessionManager {
    /// Create an empty manager retaining `scrollback_bytes_per_session` of
    /// output per session.
    pub fn new(scrollback_bytes_per_session: usize) -> Self {
        Self {
            scrollback_bytes: scrollback_bytes_per_session,
            next_id: AtomicU64::new(1),
            next_viewer: AtomicU64::new(1),
            sessions: RwLock::new(BTreeMap::new()),
        }
    }

    /// Allocate an identity for one client's attachments.
    ///
    /// One per connection, not one per attachment: a window is a single
    /// viewport, and its geometry is a property of the window rather than of
    /// each session it happens to be showing.
    pub fn new_viewer(&self) -> ViewerId {
        ViewerId(self.next_viewer.fetch_add(1, Ordering::Relaxed))
    }

    /// Spawn `spec` in a real PTY and start pumping its output.
    ///
    /// Must be called from a Tokio runtime: the coalescing window needs a timer.
    pub fn spawn(&self, spec: SessionSpec) -> anyhow::Result<SessionId> {
        let handle = tokio::runtime::Handle::try_current()
            .context("SessionManager::spawn must run inside a Tokio runtime")?;

        if spec.command.is_empty() {
            bail!("session command is empty");
        }
        if !spec.cwd.is_dir() {
            bail!("cwd {} is not a directory", spec.cwd.display());
        }

        // A zero-sized PTY makes full-screen programs divide by zero, and a
        // client that has not laid out yet legitimately reports 0.
        let cols = spec.cols.max(1);
        let rows = spec.rows.max(1);

        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .with_context(|| format!("opening a pty for {}", spec.command))?;

        let mut cmd = CommandBuilder::new(&spec.command);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        // Hosted agents render with escape sequences, so an unset or dumb TERM
        // makes them fall back to plain output. The caller can override it.
        if !spec.env.iter().any(|(k, _)| k == "TERM") {
            cmd.env("TERM", "xterm-256color");
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow::anyhow!(spawn_failure(&spec.command, &e)))?;
        // Taken here because `child` moves into the reader thread below, and
        // this is a field read on an already-spawned process: no syscall, no
        // `/proc`, nothing that could run again later.
        let child_pid = child.process_id();
        // The slave handle must go before the read loop starts: while this
        // process holds it open the master never reports EOF, so the session
        // would never be reaped after its child exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("cloning the pty reader")?;
        let writer = pair.master.take_writer().context("taking the pty writer")?;
        let killer = child.clone_killer();

        let id = SessionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let now = now_ms();
        let (output, _) = broadcast::channel(OUTPUT_CHANNEL_CHUNKS);
        let (status_tx, _) = watch::channel(SessionStatus::Starting);
        let (observations_tx, _) = watch::channel(0u64);
        let (input_tx, input_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        let info = SessionInfo {
            id,
            project_id: spec.project_id,
            title: spec
                .title
                .clone()
                .unwrap_or_else(|| default_title(&spec.command)),
            cwd: spec.cwd.to_string_lossy().into_owned(),
            command: spec.command.clone(),
            args: spec.args.clone(),
            status: SessionStatus::Starting,
            created_at_ms: now,
            last_activity_ms: now,
            cols,
            rows,
            git_branch: git_branch(&spec.cwd),
            unread: false,
            attention: Attention::default(),
            // No agent has declared anything yet. `None` is the honest value and
            // the common case: every harness that has never heard of OSC 7373
            // stays `None` for its whole life and must remain fully supported.
            hint: None,
        };

        let session = Arc::new(Session {
            id,
            info: RwLock::new(info),
            scrollback: Mutex::new(Scrollback::with_capacity(self.scrollback_bytes)),
            output,
            status: status_tx,
            observations: observations_tx,
            master: Mutex::new(pair.master),
            viewers: Mutex::new(BTreeMap::new()),
            resizes: AtomicU64::new(0),
            probes: AtomicU64::new(0),
            activity: Notify::new(),
            input: Mutex::new(Some(input_tx)),
            killer: Mutex::new(killer),
            last_focus_ms: AtomicU64::new(0),
            child_pid,
        });

        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (exit_tx, exit_rx) = oneshot::channel::<Option<i32>>();

        // Dedicated threads, not spawn_blocking: these loops live as long as
        // the session, and parking a Tokio blocking-pool slot for a session's
        // whole lifetime would both exhaust that pool and make runtime
        // shutdown wait on a read that only ends when the child exits.
        std::thread::Builder::new()
            .name(format!("vitrum-pty-read-{}", id.0))
            .spawn(move || read_loop(reader, child, raw_tx, exit_tx))
            .context("starting the pty reader thread")?;

        std::thread::Builder::new()
            .name(format!("vitrum-pty-write-{}", id.0))
            .spawn(move || write_loop(writer, input_rx))
            .context("starting the pty writer thread")?;

        handle.spawn(coalesce_loop(Arc::clone(&session), raw_rx, exit_rx));

        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, Arc::clone(&session));
        Ok(id)
    }

    /// Every known session, ordered by id so the sidebar is stable.
    pub fn list(&self) -> Vec<SessionInfo> {
        read_lock(&self.sessions)
            .values()
            .map(|s| s.snapshot())
            .collect()
    }

    /// Snapshot of one session, or `None` once it has been closed.
    pub fn info(&self, id: SessionId) -> Option<SessionInfo> {
        self.get(id).map(|s| s.snapshot())
    }

    /// Attach `viewer` at `cols` by `rows` and subscribe to live output,
    /// starting at the next published chunk.
    ///
    /// Attaching is the operator looking: it marks the session read and
    /// acknowledges every pending attention signal. `failed` is deliberately
    /// cleared here too, because it means "unacknowledged failure" rather than
    /// "it failed"; `status` keeps the historical fact. The two are independent
    /// axes: status is what the process did, attention is whether the operator
    /// still needs to look.
    ///
    /// It is also what makes this viewer constrain the PTY's size. The geometry
    /// is registered before anything else changes, so a session left at a size
    /// no attached window can draw is not a state this can produce.
    pub fn attach(
        &self,
        id: SessionId,
        viewer: ViewerId,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<broadcast::Receiver<OutputChunk>> {
        let s = self.require(id)?;
        {
            let mut viewers = lock(&s.viewers);
            let previous = viewers.insert(viewer, (cols.max(1), rows.max(1)));
            if let Err(e) = s.apply_geometry(&viewers) {
                // Nothing was applied, so leave nothing behind either.
                match previous {
                    Some(geometry) => viewers.insert(viewer, geometry),
                    None => viewers.remove(&viewer),
                };
                return Err(e);
            }
        }
        s.last_focus_ms.store(now_ms(), Ordering::Relaxed);
        {
            let mut info = write_lock(&s.info);
            info.unread = false;
            info.attention.bell = false;
            info.attention.failed = false;
        }
        Ok(s.output.subscribe())
    }

    /// Stop `viewer` from constraining the session's geometry.
    ///
    /// Idempotent and infallible for an unknown session or viewer: a tab switch
    /// may detach something already detached, and a client that disconnects
    /// after its session was closed must not be an error path. A window holding
    /// a session in a background tab has detached, so it stops pinning the PTY
    /// to a size it is not drawing and the session grows back for whoever is.
    pub fn detach(&self, id: SessionId, viewer: ViewerId) {
        let Some(s) = self.get(id) else {
            return;
        };
        let mut viewers = lock(&s.viewers);
        if viewers.remove(&viewer).is_none() {
            return;
        }
        if let Err(e) = s.apply_geometry(&viewers) {
            tracing::debug!(session = id.0, error = %e, "resizing after a detach");
        }
    }

    /// Watch for status transitions, so an exit is observed without polling.
    pub fn subscribe_status(&self, id: SessionId) -> Option<watch::Receiver<SessionStatus>> {
        Some(self.get(id)?.status.subscribe())
    }

    /// Watch for projection changes that the status channel does not report:
    /// the foreground probe's answer, an agent hint, and geometry.
    ///
    /// A revision counter rather than the value, because the projection is read
    /// with [`SessionManager::info`] and duplicating it in the channel would
    /// give two sources of truth that can disagree.
    pub fn subscribe_observations(&self, id: SessionId) -> Option<watch::Receiver<u64>> {
        Some(self.get(id)?.observations.subscribe())
    }

    /// Retained output older than `before_seq`, newest-first paging.
    ///
    /// Returns `(from_seq, bytes, more)` where `more` is true when bytes older
    /// than `from_seq` are still retained. Pass `u64::MAX` for `before_seq` to
    /// mean "from the current head".
    pub fn scrollback(
        &self,
        id: SessionId,
        before_seq: u64,
        max_bytes: usize,
    ) -> Option<(u64, Vec<u8>, bool)> {
        let s = self.get(id)?;
        let sb = lock(&s.scrollback);
        let oldest = sb.oldest_seq();
        let end = before_seq.min(sb.head_seq());
        if end <= oldest {
            return Some((oldest, Vec::new(), false));
        }
        let from = end.saturating_sub(max_bytes as u64).max(oldest);
        let bytes = sb
            .range(from, (end - from) as usize)
            .expect("from is clamped to the retained range");
        Some((from, bytes, from > oldest))
    }

    /// Run `f` over one session's retained bytes without copying them.
    ///
    /// `f` receives the seq of the oldest retained byte and the ring as two
    /// contiguous runs, oldest first; either may be empty. `None` means the
    /// session is gone, which happens routinely when a sweep races a close and
    /// is a reason to skip that session rather than to fail the whole query.
    ///
    /// A closure rather than borrowed slices, because the ring's lock must not
    /// escape this crate: a caller holding a guard could hold every session's
    /// guard at once, and a full 200 MB sweep across twenty rings takes about
    /// 105 ms, which would be 105 ms of stalled PTY pumps to answer a search.
    /// One session per call keeps the hold at about 5 ms and staggered.
    ///
    /// So `f` must be a bounded sweep and nothing else. It runs under the lock.
    pub fn with_scrollback<R>(
        &self,
        id: SessionId,
        f: impl FnOnce(u64, &[u8], &[u8]) -> R,
    ) -> Option<R> {
        let s = self.get(id)?;
        let sb = lock(&s.scrollback);
        let (first, second) = sb.halves();
        Some(f(sb.oldest_seq(), first, second))
    }

    /// Queue `data` for the session's PTY, preserving order.
    ///
    /// This returns once the bytes are queued. Writing inline would block the
    /// caller for as long as the child ignores its stdin.
    pub fn write(&self, id: SessionId, data: &[u8]) -> anyhow::Result<()> {
        let s = self.require(id)?;
        if !read_lock(&s.info).status.is_live() {
            bail!("session {} has exited", id.0);
        }
        {
            let queue = lock(&s.input);
            queue
                .as_ref()
                .ok_or_else(|| anyhow!("session {} pty writer is gone", id.0))?
                .send(data.to_vec())
                .map_err(|_| anyhow!("session {} pty writer is gone", id.0))?;
        }
        // Input is the one change the probe cannot see coming. A child reading
        // with echo off answers a password prompt without emitting a byte, so
        // without this its last answer would stand until it happened to print
        // something. One wakeup per burst of typing, and none while idle.
        s.activity.notify_one();
        Ok(())
    }

    /// Record `viewer`'s new geometry and resize the PTY if that changed the
    /// smallest attached size.
    ///
    /// A viewer that is not attached is ignored rather than rejected: a client
    /// may lay out before it attaches, and a window that is not drawing this
    /// session has no business pinning its size.
    pub fn resize(
        &self,
        id: SessionId,
        viewer: ViewerId,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<()> {
        let s = self.require(id)?;
        let mut viewers = lock(&s.viewers);
        let Some(slot) = viewers.get_mut(&viewer) else {
            return Ok(());
        };
        *slot = (cols.max(1), rows.max(1));
        s.apply_geometry(&viewers)
    }

    /// How many times this session's PTY has actually been resized.
    ///
    /// Diagnostic. Two windows negotiating a size should converge on one ioctl
    /// and then stop, and this is what makes that assertable instead of
    /// eyeballed from a flickering child.
    pub fn resize_count(&self, id: SessionId) -> Option<u64> {
        Some(self.get(id)?.resizes.load(Ordering::Relaxed))
    }

    /// How many times this session's foreground process has been probed.
    ///
    /// Diagnostic, and the assertion behind "idle costs nothing": the probe is
    /// armed by activity and disarmed by its own answer, so a session nobody is
    /// touching must hold this number still no matter how long you watch it. A
    /// count that climbs on its own is a timer, and a timer is the bug.
    pub fn probe_count(&self, id: SessionId) -> Option<u64> {
        Some(self.get(id)?.probes.load(Ordering::Relaxed))
    }

    /// Process id of this session's child, while that child is still live.
    ///
    /// `None` covers three distinct cases and deliberately collapses them,
    /// because a caller can act on none of them differently: no such session,
    /// a platform that reported no pid, and a session whose child has exited.
    /// The last is the one that matters. A reaped pid can be reused by an
    /// unrelated process within seconds on a busy machine, so answering with
    /// the stored number after an exit would hand a caller a live process that
    /// has nothing to do with this session.
    ///
    /// Two loads and a hash lookup. Nothing here reads `/proc` or the process
    /// table, so it is safe to call from anywhere, including a path that runs
    /// per session per report.
    pub fn child_pid(&self, id: SessionId) -> Option<u32> {
        let s = self.get(id)?;
        if !read_lock(&s.info).status.is_live() {
            return None;
        }
        s.child_pid
    }

    /// How many live subscribers this session's output has.
    ///
    /// Zero means nobody is drawing it, which is what makes new output count as
    /// unread and lets its bell raise. It is the same number `push` consults to
    /// decide whether a chunk is being watched.
    ///
    /// Exposed because a disconnect is ASYNCHRONOUS and a test that assumes
    /// otherwise is flaky by construction. Closing a client socket does not
    /// release the attachment; the daemon releases it when it notices, and
    /// output produced in that gap is still delivered to a live receiver and so
    /// is still marked read. `a_disconnect_releases_the_attachment` asserted
    /// `unread` immediately after dropping its connection and failed roughly
    /// one run in twenty, because it was asserting a consequence of a
    /// precondition it never established. Wait on this instead, then act.
    pub fn watchers(&self, id: SessionId) -> Option<usize> {
        Some(self.get(id)?.output.receiver_count())
    }

    /// Set the operator-chosen title for a session.
    ///
    /// The daemon owns session identity, so it owns the name. A title held only
    /// in one client vanishes on restart and is invisible to a second window,
    /// which is why this is a protocol operation rather than local UI state.
    ///
    /// An all-whitespace title is rejected rather than accepted and rendered as
    /// a blank row: a session you cannot identify is worse than one with a
    /// generated name.
    pub fn rename(&self, id: SessionId, title: &str) -> anyhow::Result<()> {
        let trimmed = title.trim();
        anyhow::ensure!(
            !trimmed.is_empty(),
            "renaming session {}: title is empty",
            id.0
        );
        let s = self.require(id)?;
        let mut info = write_lock(&s.info);
        info.title = trimmed.to_string();
        Ok(())
    }

    /// Kill the session's child and drop the session from the registry.
    ///
    /// The PTY and its threads unwind on their own once the child dies and the
    /// master reports EOF, so this never blocks on the child.
    pub fn close(&self, id: SessionId) -> anyhow::Result<()> {
        let s = self
            .sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
            .ok_or_else(|| anyhow!("no session {}", id.0))?;
        // An already-exited child gives ESRCH here, which is not a failure to
        // close: the session is gone either way.
        if let Err(e) = lock(&s.killer).kill() {
            tracing::debug!(session = s.id.0, error = %e, "kill on close");
        }
        Ok(())
    }

    pub(crate) fn get(&self, id: SessionId) -> Option<Arc<Session>> {
        read_lock(&self.sessions).get(&id).map(Arc::clone)
    }

    fn require(&self, id: SessionId) -> anyhow::Result<Arc<Session>> {
        self.get(id).ok_or_else(|| anyhow!("no session {}", id.0))
    }
}

/// Blocking PTY read loop, then reap the child.
///
/// Reaping here rather than in a separate task is what guarantees every byte is
/// published before the session reports `Exited`: this thread stops reading,
/// hands over the exit code, and only then drops `out`, so the coalescer drains
/// what is left before it observes the exit.
fn read_loop(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    out: mpsc::UnboundedSender<Vec<u8>>,
    exit: oneshot::Sender<Option<i32>>,
) {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if out.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            // A closed pty reports EIO on Linux and EBADF elsewhere; both mean
            // the same thing here, which is that the child is done writing.
            Err(_) => break,
        }
    }
    let code = match child.wait() {
        // A signalled child has no exit code to report. portable-pty
        // synthesises 1 for it, which would be indistinguishable from a real
        // `exit 1`, so the signal name is what decides.
        Ok(status) if status.signal().is_some() => None,
        // Windows exit codes use the full u32 range and are conventionally read
        // as signed, e.g. 0xC0000005 shown as -1073741819.
        Ok(status) => Some(status.exit_code() as i32),
        Err(e) => {
            tracing::warn!(error = %e, "waiting on pty child");
            None
        }
    };
    let _ = exit.send(code);
}

/// Blocking PTY write loop. Ends when the session drops its queue.
fn write_loop(mut writer: Box<dyn Write + Send>, mut input: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(data) = input.blocking_recv() {
        if writer
            .write_all(&data)
            .and_then(|()| writer.flush())
            .is_err()
        {
            break;
        }
    }
}

/// Coalesce raw reads into a few large chunks, then publish them.
async fn coalesce_loop(
    session: Arc<Session>,
    mut raw: mpsc::UnboundedReceiver<Vec<u8>>,
    exit: oneshot::Receiver<Option<i32>>,
) {
    let mut buf: Vec<u8> = Vec::new();
    let mut scan = OutputScan::new();
    let mut hints: Vec<HintDeclaration> = Vec::new();
    // Armed once at spawn so a child that never writes a byte is still
    // classified, and re-armed only by activity from then on.
    let mut settle_at = Some(Instant::now() + SETTLE_WINDOW);

    loop {
        let Some(first) = next_read(&session, &mut raw, &mut settle_at).await else {
            break;
        };
        buf.clear();
        buf.extend_from_slice(&first);
        let deadline = Instant::now() + FLUSH_WINDOW;
        let mut ended = false;
        while buf.len() < FLUSH_BYTES {
            match timeout_at(deadline, raw.recv()).await {
                Ok(Some(more)) => buf.extend_from_slice(&more),
                Ok(None) => {
                    ended = true;
                    break;
                }
                Err(_) => break,
            }
        }
        hints.clear();
        let wants_operator = scan.scan(&buf, &mut hints);
        // Only the last declaration in a run matters: an agent that says
        // `working` and then `ready` in the same burst has finished, and
        // publishing the intermediate state would flash a stale badge.
        session.publish(&buf, wants_operator, hints.last());
        // This burst may have changed what the foreground process is doing, so
        // the answer is worth taking again once it stops.
        settle_at = Some(Instant::now() + SETTLE_WINDOW);
        if ended {
            break;
        }
    }
    if scan.rejected_hints() > 0 {
        tracing::debug!(
            session = session.id.0,
            rejected = scan.rejected_hints(),
            "malformed agent hint sequences were dropped"
        );
    }
    // The reader thread has stopped and every byte it produced is published.
    let code = exit.await.unwrap_or(None);
    session.finish(code);
}

/// Wait for the next raw read, probing the foreground process if the stream
/// goes quiet first.
///
/// This is the only place a timer exists for the probe, and it is armed by
/// activity rather than by a clock: once the probe has run, `settle_at` is
/// cleared and the wait becomes an unbounded park on the channel. A session
/// nobody is touching therefore costs zero wakeups, which is the difference
/// between an idle daemon at 0% and a competitor's terminal that never reaches
/// it.
async fn next_read(
    session: &Session,
    raw: &mut mpsc::UnboundedReceiver<Vec<u8>>,
    settle_at: &mut Option<Instant>,
) -> Option<Vec<u8>> {
    loop {
        match *settle_at {
            None => {
                tokio::select! {
                    chunk = raw.recv() => return chunk,
                    () = session.activity.notified() => {
                        *settle_at = Some(Instant::now() + SETTLE_WINDOW);
                    }
                }
            }
            Some(deadline) => {
                tokio::select! {
                    chunk = timeout_at(deadline, raw.recv()) => match chunk {
                        Ok(chunk) => return chunk,
                        Err(_) => {
                            session.observe_foreground();
                            *settle_at = None;
                        }
                    },
                    () = session.activity.notified() => {
                        *settle_at = Some(Instant::now() + SETTLE_WINDOW);
                    }
                }
            }
        }
    }
}

/// Milliseconds since the Unix epoch, the clock the wire protocol speaks.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Tab label for a session whose creator did not name it.
fn default_title(command: &str) -> String {
    Path::new(command)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| command.to_string())
}

/// What to tell the operator when a session could not start.
///
/// The underlying error for the overwhelmingly common case, a mistyped or
/// uninstalled command, is `Unable to spawn X because:\nNo viable candidates
/// found in PATH "..."` followed by every entry of `PATH`. Measured on this
/// machine that is over a kilobyte, and the operator's banner ended in
/// `/snap/bin:/snap/bin"`. None of it answers the only question they have,
/// which is what to type instead.
///
/// Every other spawn failure is rare and genuinely needs its reason, so it is
/// passed through and merely bounded by the wire layer.
fn spawn_failure(command: &str, e: &anyhow::Error) -> String {
    let raw = e.to_string();
    if raw.contains("No viable candidates found in PATH") {
        return format!(
            "no command named {} on PATH; use an absolute path or install it",
            display_safe(command)
        );
    }
    format!("could not start {}: {}", display_safe(command), raw)
}

/// Current branch of the repository containing `dir`, if any.
///
/// This reads `.git/HEAD` directly and walks up at most as far as the
/// filesystem root. Shelling out to git per session, let alone per visible
/// sidebar row, is the documented way to turn a sidebar into a CPU hog.
pub(crate) fn git_branch(dir: &Path) -> Option<String> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let dot = d.join(".git");
        let git_dir = match std::fs::metadata(&dot) {
            Ok(m) if m.is_dir() => Some(dot.clone()),
            // A worktree or submodule has `.git` as a file pointing elsewhere.
            Ok(m) if m.is_file() => std::fs::read_to_string(&dot).ok().and_then(|s| {
                let p = Path::new(s.trim().strip_prefix("gitdir:")?.trim()).to_path_buf();
                Some(if p.is_absolute() { p } else { d.join(p) })
            }),
            _ => None,
        };
        if let Some(git_dir) = git_dir {
            let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
            let head = head.trim();
            return match head.strip_prefix("ref:") {
                // The whole branch name, not its last component. Dropping the
                // namespace made `feature/login` and `fix/login` the same word
                // in the sidebar, which is the one place the cell exists to
                // tell two agents apart. Width is the row's problem: it
                // truncates from the end, so the part that differs survives.
                Some(r) => {
                    let name = display_safe(vitrum_fmt::git::strip_ref_prefix(r.trim()));
                    (!name.is_empty()).then_some(name)
                }
                // Detached HEAD holds a raw object id; the short form is what a
                // sidebar can actually display. `short_commit` cuts only a hex
                // id, so a HEAD holding anything else is left whole and fails
                // the length check below rather than being shown as a stub of
                // itself. It also never splits a character, which `head[..7]`
                // did: a file holding `ééééééé` took the spawn down with it.
                None => {
                    let short = vitrum_fmt::git::short_commit(head);
                    (short.chars().count() == 7).then(|| display_safe(short))
                }
            };
        }
        cur = d.parent();
    }
    None
}

/// A poisoned lock here means a panic while moving bytes. Recovering is right
/// for a daemon hosting 20 agents: one panicking session must not take the
/// other 19 down with it.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn read_lock<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

fn write_lock<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}

/// What a REPOSITORY can make this daemon do.
///
/// `.git/HEAD` is a file in a directory the operator cloned from somewhere.
/// Everything read out of it is attacker-influenced in the ordinary case of
/// "I cloned a repo and started an agent in it", and it is rendered on a
/// sidebar row and in a tooltip. Every case here was found by feeding real
/// crafted files to `git_branch`, and two of them took the process down.
#[cfg(test)]
mod a_repository_is_untrusted_input {
    use super::*;

    fn head(content: &str) -> Option<String> {
        let dir = std::env::temp_dir().join(format!(
            "vitrum-head-{}-{:x}",
            std::process::id(),
            content.as_ptr() as usize
        ));
        let git = dir.join(".git");
        std::fs::create_dir_all(&git).expect("temp repo");
        std::fs::write(git.join("HEAD"), content).expect("write HEAD");
        let out = git_branch(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    /// A detached HEAD with multibyte text must not panic.
    ///
    /// This is the bug: the short form was `head[..7]`, seven BYTES of a
    /// `str`. A HEAD holding `ééééééé` is seven characters and fourteen bytes,
    /// so the slice landed inside a character and panicked. Reached by
    /// starting a session in a directory with a corrupt or crafted HEAD, on
    /// the spawn path, taking the spawn with it.
    #[test]
    fn a_multibyte_head_does_not_panic() {
        assert_eq!(head("ééééééé").map(|b| b.chars().count()), Some(7));
        assert_eq!(head("🙂🙂🙂🙂🙂🙂🙂").map(|b| b.chars().count()), Some(7));
        // Fewer than seven characters is not an object id and yields nothing,
        // rather than a truncated fragment presented as a commit.
        assert_eq!(head("abc"), None);
        assert_eq!(head("🙂"), None);
        assert_eq!(head(""), None);
    }

    /// A real detached HEAD still shortens to the usual seven hex characters.
    #[test]
    fn a_real_object_id_still_shortens_to_seven() {
        assert_eq!(
            head("0be8fdac3ab1234567890abcdef1234567890abcd"),
            Some("0be8fda".to_string())
        );
    }

    /// A branch name cannot inject a line into the row tooltip.
    ///
    /// The tooltip is newline-separated: title, path, agent and status each on
    /// their own line. A branch containing a newline would put a sentence of
    /// the repository's choosing where the daemon's own status line goes.
    #[test]
    fn a_branch_name_cannot_forge_a_tooltip_line() {
        assert_eq!(head("ref: refs/heads/a\nb"), Some("ab".to_string()));
        assert_eq!(head("ref: refs/heads/a\rb"), Some("ab".to_string()));
        assert_eq!(head("ref: refs/heads/a\tb"), Some("ab".to_string()));
        assert_eq!(head("ref: refs/heads/a\u{7}b"), Some("ab".to_string()));
    }

    /// A branch name cannot reorder what is drawn after it.
    ///
    /// Trojan Source, pointed at a sidebar. Git permits U+202E in a ref name,
    /// and a terminal or a webview renders everything after it right to left,
    /// so a branch can display as text it is not. Removed, not escaped: the
    /// operator's question is which branch, not which invisible character.
    #[test]
    fn a_branch_name_cannot_reorder_the_row() {
        assert_eq!(head("ref: refs/heads/ma\u{202e}nice"), Some("manice".to_string()));
        for bad in ['\u{200e}', '\u{200f}', '\u{061c}', '\u{202a}', '\u{202d}', '\u{2066}', '\u{2069}'] {
            let got = head(&format!("ref: refs/heads/a{bad}b")).expect("a branch");
            assert_eq!(got, "ab", "{bad:?} survived into a rendered branch name");
        }
    }

    /// Ordinary non-ASCII branch names survive untouched.
    ///
    /// The sanitiser removes two narrow classes and must not become "strip
    /// anything unfamiliar": a team whose branches are in Cyrillic or Japanese
    /// has to see its own branch names.
    #[test]
    fn an_ordinary_unicode_branch_is_left_alone() {
        for name in ["main", "функция", "機能", "café", "a-b_c.d", "v1.2.3"] {
            assert_eq!(
                head(&format!("ref: refs/heads/{name}")),
                Some(name.to_string()),
                "an ordinary branch name was mangled"
            );
        }
    }

    /// Only the ref prefix is removed. The namespace stays.
    ///
    /// Showing the leaf alone fitted a 224px row and made `wip/deploy` and
    /// `fix/deploy` the same word, in the one cell whose job is telling two
    /// agents apart. The row truncates from the end, so keeping the whole name
    /// costs the tail nobody reads and saves the head that identifies it.
    #[test]
    fn only_the_ref_prefix_is_stripped_from_a_branch() {
        assert_eq!(
            head("ref: refs/heads/wip/feature"),
            Some("wip/feature".to_string())
        );
        assert_eq!(
            head("ref: refs/heads/feature/JIRA-1234/thing"),
            Some("feature/JIRA-1234/thing".to_string())
        );
    }

    /// A branch made entirely of removed characters yields nothing, not "".
    ///
    /// An empty string would render as a branch that exists and has no name,
    /// which is a row saying something false. `None` is the honest answer.
    #[test]
    fn a_branch_of_nothing_but_controls_is_no_branch() {
        assert_eq!(head("ref: refs/heads/\u{202e}\u{200f}"), None);
        assert_eq!(head("ref: refs/heads/"), None);
    }
}

/// A failed spawn must say what to type instead.
///
/// Every case here was produced by a live daemon refusing a real
/// `createSession`, not by imagining what an error might look like.
#[cfg(test)]
mod a_failed_spawn_tells_the_operator_what_to_change {
    use super::*;

    fn path_error(command: &str) -> anyhow::Error {
        anyhow::anyhow!(
            "Unable to spawn {command} because:\nNo viable candidates found in PATH \"{}\"",
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".repeat(20)
        )
    }

    /// The common failure is a typo, and it used to answer with the PATH.
    ///
    /// Measured: `createSession` with `nope-9f3a` returned 1,092 characters
    /// ending in `/snap/bin:/snap/bin"`. The operator's question is what to
    /// type instead, and not one character of the PATH dump answered it.
    #[test]
    fn a_missing_command_does_not_recite_the_path() {
        let m = spawn_failure("claud", &path_error("claud"));
        assert_eq!(
            m,
            "no command named claud on PATH; use an absolute path or install it"
        );
        assert!(!m.contains("/usr/bin"), "the PATH came back: {m}");
        assert!(m.len() < 100, "still {} characters", m.len());
    }

    /// Every other spawn failure keeps its reason.
    ///
    /// The narrow rewrite must not become "replace all spawn errors with a
    /// friendly sentence": a permission error and a directory-as-command are
    /// different problems with different fixes, and the OS text is the fix.
    #[test]
    fn an_unusual_failure_keeps_the_reason_it_came_with() {
        let m = spawn_failure("bash", &anyhow::anyhow!("Permission denied (os error 13)"));
        assert_eq!(m, "could not start bash: Permission denied (os error 13)");

        let d = spawn_failure("/tmp", &anyhow::anyhow!("Unable to spawn /tmp because it is a directory"));
        assert_eq!(
            d,
            "could not start /tmp: Unable to spawn /tmp because it is a directory"
        );
    }

    /// The command name in the message is untrusted.
    ///
    /// It is whatever was typed into the run field or stored in a preset, and
    /// it is formatted straight into a banner. A newline forges a second line
    /// and U+202E reverses the rest, exactly as they did through `cwd`.
    #[test]
    fn the_command_cannot_forge_the_banner() {
        let hostile = "ba\u{202e}sh\nStatus: ok";
        let m = spawn_failure(hostile, &path_error(hostile));
        assert!(!m.contains('\n'), "newline survived: {m:?}");
        assert!(!m.contains('\u{202e}'), "override survived: {m:?}");
        assert_eq!(
            m,
            "no command named bashStatus: ok on PATH; use an absolute path or install it"
        );
    }

    /// A command in another language is reported as written.
    #[test]
    fn a_non_ascii_command_is_named_intact() {
        let m = spawn_failure("機能", &path_error("機能"));
        assert_eq!(
            m,
            "no command named 機能 on PATH; use an absolute path or install it"
        );
    }
}
