//! One child on one pseudoterminal, from spawn to reap.
//!
//! Three threads of control per session, and no more:
//!
//! - a **reader** thread parked in `read(2)` on the master, because a blocking
//!   read has no portable async form;
//! - a **writer** thread parked on the input queue, for the same reason;
//! - an async **output task** that coalesces reads into runs and publishes
//!   them.
//!
//! Nothing ticks. Every wait here is parked on a channel or on a deadline that
//! some byte armed, so a session with a quiet child costs zero wakeups and zero
//! syscalls. That is the property [`crate::tests::output_path_cost`] counts,
//! and it is the whole argument for hosting agents in a daemon.
//!
//! # Two latencies, not one
//!
//! Coalescing has two callers with opposite needs and they are handled
//! separately rather than averaged.
//!
//! A **lone keystroke** echoed back is one read of a few bytes and nothing
//! else is coming. Holding it for a fixed window would spend that window on
//! nothing, on the one path the operator can feel. So a run ends on silence:
//! [`FLUSH_IDLE`] after the last read, the run is published.
//!
//! **Bulk output** is thousands of reads arriving without a gap. There the
//! window is the point, because publishing per read turns a firehose into a
//! broadcast message, a ring write and a frame per few hundred bytes. So a run
//! that keeps growing is held until [`FLUSH_WINDOW`] or [`FLUSH_BYTES`],
//! whichever comes first.
//!
//! One `Sleep` serves both. It is created when a run opens and reset on each
//! batch, never re-registered per read.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use bytes::{Bytes, BytesMut};
use portable_pty::{ChildKiller, CommandBuilder, ExitStatus, MasterPty, PtySize};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;
use vitrum_model::{AgentKind, HintDeclaration};
use vitrum_proto::{ProjectId, SessionId, SessionInfo, SessionStatus};

use crate::command_path;
use crate::error::SessionError;
use crate::probe;
use crate::scan::OutputScan;
use crate::scrollback::Scrollback;
use crate::terminfo;

/// One block the reader carves consecutive reads out of.
///
/// The reader used to allocate per read, which at the few hundred bytes a pty
/// returns wrote about seventy times more memory than the session produced.
/// A megabyte is large enough that the allocation and the one zeroing pass
/// amortise to nothing, and small enough that a session holding a partly used
/// arena is not holding a meaningful amount of memory.
pub(crate) const READ_ARENA: usize = 1024 * 1024;

/// Remainder below which the arena is replaced rather than read into.
///
/// This replaces the old `READ_CHUNK`. There is no per-read cap any more: the
/// reader offers the kernel the whole remainder of its arena, so a read is
/// bounded by what the line discipline has, not by a constant here. What the
/// floor does is keep the offer large. Linux hands back at most
/// `N_TTY_BUF_SIZE` (4 KiB) per read, so 32 KiB is eight times the largest
/// answer the kernel can give and no read is ever short because the arena was
/// nearly spent.
pub(crate) const READ_FLOOR: usize = 32 * 1024;

/// Longest a run may be held while output is still arriving.
///
/// The upper bound on added latency for bulk output, and it only applies while
/// reads keep coming: a run that goes quiet is published by [`FLUSH_IDLE`]
/// long before this. Six milliseconds is under half a frame at 60 Hz, so a run
/// capped here is still drawn in the frame it would have been drawn in.
pub(crate) const FLUSH_WINDOW: Duration = Duration::from_millis(6);

/// Silence that ends a run.
///
/// This is the number that decides how a single keystroke feels. The old code
/// had no such number and charged an echoed keystroke a whole [`FLUSH_WINDOW`],
/// which is 6 ms of the operator's latency budget spent waiting for a second
/// read that was never coming.
///
/// 300 us is below the timer resolution Tokio's wheel actually schedules at, so
/// in practice a lone read is published on the next tick. That is deliberate:
/// the value says "as soon as the run is provably over" rather than naming a
/// delay worth having, and a coarser wheel does not make it a fixed cost the
/// way a millisecond constant written here would.
pub(crate) const FLUSH_IDLE: Duration = Duration::from_micros(300);

/// Bytes pending that end a run regardless of time.
///
/// Without it a fast child bounded only by [`FLUSH_WINDOW`] delivers whatever
/// it managed to write in 6 ms as one message, which at pty rates is megabytes
/// and defeats the point of streaming. 64 KiB is a few frames of a full screen
/// and keeps one run inside one arena in the common case.
pub(crate) const FLUSH_BYTES: usize = 64 * 1024;

/// Reads taken from the queue per wakeup.
///
/// The coalescer used to await each read under its own `timeout_at`, arming a
/// timer and rescheduling the task once per read: thousands of timer
/// registrations to publish sixteen runs. `recv_many` with this bound collapses
/// a backlog of `N` reads into `1 + (N-1)/BATCH_READS` wakeups.
pub(crate) const BATCH_READS: usize = 64;

/// Silence after which the foreground process is asked what it is doing.
///
/// The probe is armed by a published run or by operator input and disarmed by
/// its own answer. Nothing re-arms it, so a session nobody touches costs no
/// probes at all. 150 ms is long enough that a program drawing frames is never
/// interrupted mid-stream and short enough that a turn ending feels answered.
pub(crate) const SETTLE_WINDOW: Duration = Duration::from_millis(150);

/// How long a closed session's child is given to be reaped before the group is
/// killed outright, and again before the exit code is given up on.
///
/// A hangup reaches a child in microseconds, so this is not a wait anyone sees.
/// It exists so that closing a session that was about to exit anyway still
/// reports the code the child produced rather than discarding it.
const REAP_GRACE: Duration = Duration::from_millis(250);

/// Reads that may be in flight between the reader thread and the pump.
///
/// Bounded on purpose. An unbounded queue lets a child that outruns the pump
/// grow the daemon's memory without limit; a full queue blocks the reader,
/// which fills the line discipline buffer, which is how a pty is supposed to
/// apply backpressure to a program that writes faster than it is read.
const READ_QUEUE: usize = 1024;

/// Coalesced runs a client may fall behind by before it is told it lagged.
const OUTPUT_QUEUE: usize = 1024;

/// Largest `HEAD` or `.git` pointer file that is read at all.
///
/// A repository is untrusted input: the directory arrives from a client and
/// resolution walks up from it, so the size of the allocation would otherwise
/// be a property of that directory. A real `HEAD` is 41 bytes and a real
/// pointer is under `PATH_MAX`. Anything larger is refused rather than
/// truncated, because the first line of a large file is a perfectly plausible
/// ref name and a truncated read would report a branch the repository is not
/// on.
const HEAD_MAX: u64 = 4096;

/// What the child is told about the terminal it is attached to.
///
/// `TERM` is `vte-256color`. This product renders with libghostty's VT, whose
/// own terminfo entry is not present on a stock Linux install, and the entry a
/// terminal claims has to exist in the database on the machine running the
/// CHILD or every program that calls `setupterm` degrades. `vte-256color`
/// describes the capability set actually implemented here, is shipped by
/// `ncurses-term`, and names an emulator family rather than a multiplexer, so
/// nothing infers a screen or tmux layer that is not there.
///
/// The value never varies by host. A session that got a different `TERM`
/// depending on which machine the daemon happened to run on would render
/// differently for the same agent, which is worse than one wrong-but-constant
/// claim. [`SessionManager::new`] checks the database once and warns, naming
/// the entry and the package that provides it; the child is told the same thing
/// either way and falls back to its own built-in handling of an unknown `TERM`.
///
/// `COLORTERM` is the engine's claim, not this crate's invention. A colour bug
/// fixed by editing the string here would be a fix no test could see.
pub(crate) const DEFAULT_TERM_ENV: &[(&str, &str)] = &[
    ("TERM", "vte-256color"),
    ("COLORTERM", vitrum_vt::COLORTERM),
];

/// Everything needed to start one session.
#[derive(Debug, Clone)]
pub struct SessionSpec {
    pub project_id: ProjectId,
    pub cwd: PathBuf,
    pub command: String,
    pub args: Vec<String>,
    /// Overrides applied after [`DEFAULT_TERM_ENV`], so a session started to
    /// reproduce a rendering bug can claim to be anything it likes.
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
    /// A name chosen by whoever created the session. Pins the title.
    pub title: Option<String>,
}

/// One coalesced run of output, and where it starts in the session's stream.
#[derive(Debug, Clone)]
pub struct OutputChunk {
    /// Cumulative byte offset of the first byte, never restarted.
    pub seq: u64,
    pub data: Bytes,
}

/// One window's view of one session.
///
/// Geometry is per viewer because a session can be open in several windows at
/// once, and the pty gets the minimum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViewerId(pub u64);

/// What the output path spent, counted rather than timed.
///
/// Every field is a thing that was once a real cost in this loop and is cheap
/// to reintroduce by accident. They are counters and not gauges: a test asserts
/// on ratios between them, which is why they are all monotonic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PumpCounts {
    /// `read(2)` calls the reader thread made.
    pub reads: u64,
    /// Times the pump was woken with at least one read waiting.
    pub wakeups: u64,
    /// Runs handed to `publish`.
    pub publishes: u64,
    /// Deadlines armed. One per run, never one per read.
    pub timers: u64,
    /// Bytes that had to be copied to join a run, because the pieces were not
    /// adjacent in one arena.
    pub staged_bytes: u64,
    /// Bytes the daemon's scanner looked at. Must equal published volume.
    pub parsed_bytes: u64,
    /// Reader arenas allocated. The whole allocation cost of the byte path.
    pub arenas: u64,
    /// Runs ended by silence.
    pub idle_flushes: u64,
    /// Runs ended by [`FLUSH_WINDOW`] or [`FLUSH_BYTES`].
    pub capped_flushes: u64,
}

/// Atomic form of [`PumpCounts`], shared by the reader thread and the pump.
#[derive(Debug, Default)]
struct Counts {
    reads: AtomicU64,
    wakeups: AtomicU64,
    publishes: AtomicU64,
    timers: AtomicU64,
    staged_bytes: AtomicU64,
    parsed_bytes: AtomicU64,
    arenas: AtomicU64,
    idle_flushes: AtomicU64,
    capped_flushes: AtomicU64,
}

impl Counts {
    fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    fn snapshot(&self) -> PumpCounts {
        let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
        PumpCounts {
            reads: read(&self.reads),
            wakeups: read(&self.wakeups),
            publishes: read(&self.publishes),
            timers: read(&self.timers),
            staged_bytes: read(&self.staged_bytes),
            parsed_bytes: read(&self.parsed_bytes),
            arenas: read(&self.arenas),
            idle_flushes: read(&self.idle_flushes),
            capped_flushes: read(&self.capped_flushes),
        }
    }
}

/// A latch that a waiter can park on without losing the wakeup.
///
/// `Notify` alone loses a notification sent between two `notified()` futures,
/// which for a close is the difference between a session ending and a thread
/// parked for the life of the daemon. The flag is checked after the waiter is
/// registered, so either the check sees it or the notification wakes it.
#[derive(Debug, Default)]
struct Flag {
    raised: AtomicBool,
    notify: Notify,
}

impl Flag {
    fn raise(&self) {
        self.raised.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn is_raised(&self) -> bool {
        self.raised.load(Ordering::Acquire)
    }

    async fn wait(&self) {
        loop {
            let waiter = self.notify.notified();
            tokio::pin!(waiter);
            waiter.as_mut().enable();
            if self.is_raised() {
                return;
            }
            waiter.await;
        }
    }
}

/// Milliseconds since the epoch, saturating rather than panicking on a clock
/// set before 1970.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A mutex whose contents outlive a panicking holder.
///
/// Session state is a projection: a panic while it is held leaves it stale, not
/// unsound, and refusing to serve the sidebar afterwards would turn one defect
/// into a dead daemon.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// The pump
// ---------------------------------------------------------------------------

/// Turns a stream of reads into runs, on the two deadlines documented above.
struct Pump {
    rx: mpsc::Receiver<BytesMut>,
    batch: Vec<BytesMut>,
    counts: Arc<Counts>,
    closed: Arc<Flag>,
    /// Set only where [`READER_REPORTS_EOF`] is false, by the reaper.
    eof: Arc<Flag>,
}

impl Pump {
    /// The next run, or `None` when the stream is over or the session closed.
    async fn next_run(&mut self) -> Option<BytesMut> {
        if self.closed.is_raised() {
            return None;
        }
        let first = tokio::select! {
            biased;
            () = self.closed.wait() => return None,
            () = self.eof.wait() => return None,
            read = self.rx.recv() => read?,
        };
        Counts::bump(&self.counts.wakeups);

        let mut run = first;
        let opened = Instant::now();
        let cap_at = opened + FLUSH_WINDOW;
        // ONE timer for the whole run. Reset per batch, never re-registered
        // per read.
        let deadline = tokio::time::sleep_until(cap_at.min(opened + FLUSH_IDLE));
        tokio::pin!(deadline);
        Counts::bump(&self.counts.timers);

        loop {
            if run.len() >= FLUSH_BYTES {
                Counts::bump(&self.counts.capped_flushes);
                break;
            }
            tokio::select! {
                biased;
                () = self.closed.wait() => break,
                taken = self.rx.recv_many(&mut self.batch, BATCH_READS) => {
                    if taken == 0 {
                        // The reader is gone: this run is the end of the
                        // stream and is published rather than discarded.
                        break;
                    }
                    Counts::bump(&self.counts.wakeups);
                    for piece in self.batch.drain(..) {
                        merge(&mut run, piece, &self.counts);
                    }
                    let now = Instant::now();
                    deadline.as_mut().reset(cap_at.min(now + FLUSH_IDLE));
                }
                () = &mut deadline => {
                    if Instant::now() >= cap_at {
                        Counts::bump(&self.counts.capped_flushes);
                    } else {
                        Counts::bump(&self.counts.idle_flushes);
                    }
                    break;
                }
            }
        }

        Counts::bump(&self.counts.publishes);
        Some(run)
    }
}

/// Join `piece` onto `run`, in place when the two are adjacent.
///
/// Consecutive pty reads are consecutive slices of one arena, so the common
/// case decrements a refcount and moves an index. The fallback copies, and only
/// the fallback is counted: a run that straddles two arenas is legitimate and
/// happens at most once per arena.
fn merge(run: &mut BytesMut, piece: BytesMut, counts: &Counts) {
    // SAFETY: one past the end of `run`'s own allocation is a valid address to
    // form, and the pointers are only compared, never read through.
    let adjacent = unsafe { run.as_ptr().add(run.len()) } == piece.as_ptr();
    if !adjacent {
        Counts::add(&counts.staged_bytes, piece.len() as u64);
    }
    run.unsplit(piece);
}

/// Wait for the child to be reaped, or for the close that says it does not
/// matter any more.
///
/// Waiting only on the exit makes the terminal state depend on the child dying.
/// A child that redirected its own output elsewhere and carried on closes the
/// terminal while still running, and the loop would then hold the session, both
/// its threads and the sidebar row forever.
async fn await_exit(
    exit: oneshot::Receiver<ExitStatus>,
    closed: &Flag,
    escalate: impl FnOnce(),
) -> Option<ExitStatus> {
    let mut exit = exit;
    tokio::select! {
        reaped = &mut exit => return reaped.ok(),
        () = closed.wait() => {}
    }
    // Closed. The hangup has gone out; a child that is going to die has
    // already done so, and its real code is worth the quarter second.
    if let Ok(reaped) = tokio::time::timeout(REAP_GRACE, &mut exit).await {
        return reaped.ok();
    }
    escalate();
    tokio::time::timeout(REAP_GRACE, exit)
        .await
        .ok()
        .and_then(Result::ok)
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Everything about a session that is not a handle to the operating system.
struct State {
    project_id: ProjectId,
    cwd: PathBuf,
    command: String,
    args: Vec<String>,
    agent: AgentKind,
    title: String,
    /// The creator named it, or the operator renamed it. Either way the
    /// program does not get to.
    title_pinned: bool,
    term_title: Option<String>,
    status: SessionStatus,
    created_at_ms: u64,
    last_activity_ms: u64,
    /// When output last arrived, and when a window last attached. Unread and
    /// idle are the comparison between them, never a stored flag.
    last_output_ms: u64,
    last_focus_ms: u64,
    cols: u16,
    rows: u16,
    git_branch: Option<String>,
    worktree: Option<String>,
    bell: bool,
    failed: bool,
    waiting: Option<bool>,
    hint: Option<vitrum_proto::AgentHint>,
    /// Attached windows and the geometry each asked for.
    viewers: Vec<(ViewerId, u16, u16)>,
}

impl State {
    fn watched(&self) -> bool {
        !self.viewers.is_empty()
    }

    /// Smallest geometry every attached window can draw, clamped away from
    /// zero. `None` when nothing is attached, which leaves the pty alone rather
    /// than reflowing it for nobody.
    fn wanted_size(&self) -> Option<(u16, u16)> {
        let mut it = self.viewers.iter();
        let (_, mut cols, mut rows) = *it.next()?;
        for &(_, c, r) in it {
            cols = cols.min(c);
            rows = rows.min(r);
        }
        Some((cols.max(1), rows.max(1)))
    }
}

/// One hosted child, its terminal, and everything a client reads about it.
pub(crate) struct Session {
    id: SessionId,
    /// The control end. Public to the crate because "did the ioctl reach the
    /// kernel" is only answerable by asking the kernel.
    pub(crate) master: Mutex<Box<dyn MasterPty + Send>>,
    state: Mutex<State>,
    scroll: Mutex<Scrollback>,
    /// Dropped when the child is reaped, which is what ends the writer thread.
    input: Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
    output: broadcast::Sender<OutputChunk>,
    status_tx: watch::Sender<SessionStatus>,
    /// Bumped whenever the projection changed in a way a sidebar redraws for.
    obs_tx: watch::Sender<u64>,
    /// Bumped by output and by operator input, to restart the settle timer.
    arm_tx: watch::Sender<u64>,
    counts: Arc<Counts>,
    closed: Arc<Flag>,
    probes: AtomicU64,
    resizes: AtomicU64,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    child_pid: Option<u32>,
    #[cfg(windows)]
    job: Mutex<Option<windows_job::JobObject>>,
}

impl Session {
    /// Whether the writer queue is still there to take a keystroke.
    #[cfg(test)]
    pub(crate) fn input_is_open(&self) -> bool {
        lock(&self.input).is_some()
    }

    /// What a client renders for this session.
    pub(crate) fn snapshot(&self) -> SessionInfo {
        let st = lock(&self.state);
        let watched = st.watched();
        let unseen = !watched && st.last_output_ms > st.last_focus_ms;
        SessionInfo {
            id: self.id,
            project_id: st.project_id,
            title: st.title.clone(),
            cwd: st.cwd.to_string_lossy().into_owned(),
            command: st.command.clone(),
            args: st.args.clone(),
            status: st.status.clone(),
            created_at_ms: st.created_at_ms,
            last_activity_ms: st.last_activity_ms,
            cols: st.cols,
            rows: st.rows,
            git_branch: st.git_branch.clone(),
            unread: unseen,
            attention: vitrum_proto::Attention {
                bell: !watched && st.bell,
                idle_ms: if unseen {
                    now_ms().saturating_sub(st.last_output_ms)
                } else {
                    0
                },
                failed: !watched && st.failed,
                waiting: st.waiting,
            },
            hint: st.hint.clone(),
            term_title: st.term_title.clone(),
            worktree: st.worktree.clone(),
        }
    }

    fn observe(&self) {
        self.obs_tx.send_modify(|n| *n += 1);
    }

    fn arm_probe(&self) {
        self.arm_tx.send_modify(|n| *n += 1);
    }

    /// Record one coalesced run: scan it, retain it, hand it out.
    ///
    /// The ring and the broadcast are written from here and nowhere else, which
    /// is what makes a client that backfills and a client that streamed see the
    /// same bytes at the same offsets.
    fn publish(&self, data: Bytes, scan: &mut OutputScan) {
        let len = data.len() as u64;
        let mut hints: Vec<HintDeclaration> = Vec::new();
        let rang = scan.scan(&data, &mut hints);
        Counts::add(&self.counts.parsed_bytes, len);
        let title = scan.take_title();
        let pwd = scan.take_pwd();

        let seq = {
            let mut ring = lock(&self.scroll);
            let seq = ring.head_seq();
            ring.push(&data);
            seq
        };
        let _ = self.output.send(OutputChunk { seq, data });

        let now = now_ms();
        let mut notable = false;
        {
            let mut st = lock(&self.state);
            st.last_activity_ms = now;
            st.last_output_ms = now;
            if matches!(st.status, SessionStatus::Starting) {
                st.status = SessionStatus::Running;
                self.status_tx.send_replace(SessionStatus::Running);
                notable = true;
            }
            let watched = st.watched();
            if rang && !watched && !st.bell {
                st.bell = true;
                notable = true;
            }
            if let Some(announced) = title {
                notable |= apply_announced_title(&mut st, announced);
            }
            if let Some(raw) = pwd {
                notable |= adopt_reported_directory(&mut st, &raw);
            }
            if let Some(declared) = hints.pop() {
                st.hint = Some(declared.into_hint(now));
                notable = true;
            }
        }
        if notable {
            self.observe();
        }
        self.arm_probe();
    }

    /// Record the child's outcome, once.
    fn finish(&self, status: Option<ExitStatus>) {
        // portable-pty synthesises exit code 1 for a signalled child, so the
        // signal has to be consulted first or a killed agent is indistinguishable
        // from one that failed.
        let code = match &status {
            Some(s) if s.signal().is_some() => None,
            Some(s) => Some(s.exit_code() as i32),
            None => None,
        };
        {
            let mut st = lock(&self.state);
            st.status = SessionStatus::Exited { code };
            st.last_activity_ms = now_ms();
            // There is no foreground process left, so the last live answer is a
            // claim about something that no longer exists.
            st.waiting = None;
            if code != Some(0) && !st.watched() {
                st.failed = true;
            }
        }
        // Nothing can be written to a dead child, and the queue is a parked
        // thread. Exited sessions stay listed, so keeping it would leak one
        // thread per finished session for the daemon's lifetime.
        lock(&self.input).take();
        self.status_tx.send_replace(SessionStatus::Exited { code });
        self.observe();
    }

    /// Send the geometry every attached window can draw, if it changed.
    fn apply_geometry(&self, st: &mut State) -> bool {
        let Some((cols, rows)) = st.wanted_size() else {
            return false;
        };
        if (cols, rows) == (st.cols, st.rows) {
            return false;
        }
        st.cols = cols;
        st.rows = rows;
        self.resize_pty(cols, rows);
        true
    }

    fn resize_pty(&self, cols: u16, rows: u16) {
        let sized = lock(&self.master).resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        if let Err(e) = sized {
            tracing::warn!(session = self.id.0, error = %e, "resizing the pty failed");
        }
        Counts::bump(&self.resizes);
    }

    /// Hang up the session's process group.
    fn hang_up(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.child_pid {
            // The child is the pty's session leader, so its pid is its group.
            // A grandchild the child started is in that group and goes with it.
            // SAFETY: a signal to a pid this process spawned; no pointers.
            unsafe { libc::killpg(pid as i32, libc::SIGHUP) };
            return;
        }
        #[cfg(windows)]
        {
            // Closing the job terminates every process in it, which is the only
            // mechanism on Windows that reaches a grandchild.
            lock(&self.job).take();
        }
        let _ = lock(&self.killer).kill();
    }

    /// Escalate past a child that ignored the hangup.
    fn kill_group(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.child_pid {
            // SAFETY: as above.
            unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
            return;
        }
        let _ = lock(&self.killer).kill();
    }
}

/// Apply an OSC 0 or OSC 2 title. Returns whether the projection changed.
///
/// An agent TUI treats the title bar as a status line and rewrites it every
/// turn, so the announcement is always RECORDED and only sometimes taken as a
/// name. Dropping it for an agent would have fixed the row and silently
/// disabled approval detection, which reads the recorded title.
fn apply_announced_title(st: &mut State, announced: String) -> bool {
    if announced.is_empty() {
        // A clear retracts the claim. It never blanks the row: an empty name is
        // strictly worse than the one the session already had.
        return st.term_title.take().is_some();
    }
    let changed = st.term_title.as_deref() != Some(announced.as_str());
    if !st.title_pinned && st.agent.title_is_a_name() && st.title != announced {
        st.title = announced.clone();
    }
    st.term_title = Some(announced);
    changed
}

/// Adopt a directory the shell reported over OSC 7, and everything derived
/// from it.
///
/// The single site where a session's directory changes, so anything else
/// resolved from the directory belongs here rather than beside a second copy of
/// this logic.
fn adopt_reported_directory(st: &mut State, raw: &str) -> bool {
    let Some(path) = vitrum_vt::pwd_path(raw) else {
        return false;
    };
    // A session inside `ssh` reports the remote machine's paths. Adopting one
    // would say the session is somewhere it has never been.
    if !path.is_dir() || path == st.cwd {
        return false;
    }
    let git = git_context(&path);
    st.git_branch = git.branch;
    st.worktree = git.worktree;
    st.cwd = path;
    true
}

// ---------------------------------------------------------------------------
// Branch resolution
// ---------------------------------------------------------------------------

/// A repository's identity as the sidebar shows it.
///
/// Both halves come from one upward walk. Resolving them separately would read
/// `.git` twice per directory change on a path that runs whenever an agent
/// reports a new working directory.
#[derive(Default)]
pub(crate) struct GitContext {
    /// Branch name, or short commit for a detached HEAD.
    pub(crate) branch: Option<String>,
    /// Directory name under `.git/worktrees` when this checkout is a linked
    /// worktree.
    ///
    /// The name, never a path: it is what git itself calls the worktree, it is
    /// unique within the repository, and it does not put a filesystem location
    /// on screen.
    pub(crate) worktree: Option<String>,
}

/// Branch and worktree for the repository `from` sits in.
///
/// Read from `.git` directly. Shelling out to git once per sidebar row is the
/// documented way a session list becomes a CPU hog, and this has to work with
/// no git binary installed at all.
///
/// Resolution stops at the FIRST directory holding a `.git`, exactly as git
/// does. Continuing past a repository with an unreadable HEAD would report an
/// unrelated parent repository's branch for a checkout that is not on it.
///
/// A `.git` that is a file rather than a directory points at the real git
/// directory. For a linked worktree that target is
/// `<repo>/.git/worktrees/<name>` and `<name>` is the worktree. For a
/// submodule it is `<repo>/.git/modules/...`, which is not a worktree and is
/// reported as none.
///
/// An unreadable or dangling pointer yields no worktree. It never suppresses
/// the branch: the two are read from different files, and a `.git` file that
/// does not parse leaves the walk to continue into the parent repository,
/// which still has a HEAD.
pub(crate) fn git_context(from: &Path) -> GitContext {
    let mut dir = from;
    loop {
        if let Some(found) = repository_at(dir) {
            return found;
        }
        let Some(parent) = dir.parent() else {
            return GitContext::default();
        };
        dir = parent;
    }
}

/// `Some(context)` when `dir` holds a repository. `None` when there is no
/// `.git` here at all, which is what tells the walk to keep going up.
fn repository_at(dir: &Path) -> Option<GitContext> {
    let dot_git = dir.join(".git");
    let meta = std::fs::metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(GitContext { branch: head_of(&dot_git), worktree: None });
    }
    // A worktree or submodule stores a pointer instead of a directory.
    let pointer = read_bounded(&dot_git)?;
    let target = pointer.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return Some(GitContext::default());
    }
    let target = Path::new(target);
    let git_dir = if target.is_absolute() { target.to_path_buf() } else { dir.join(target) };
    Some(GitContext { branch: head_of(&git_dir), worktree: worktree_name(&git_dir) })
}

/// The worktree name in `<repo>/.git/worktrees/<name>`, if that is the shape.
///
/// The last component is taken only when its parent is literally `worktrees`.
/// Without that check a submodule's `.git/modules/<name>` would arrive in the
/// sidebar labelled as a worktree, and a pointer to a plain git directory
/// would arrive labelled `.git`.
fn worktree_name(git_dir: &Path) -> Option<String> {
    if git_dir.parent()?.file_name()? != "worktrees" {
        return None;
    }
    // A pointer that resolves to nothing is a leftover from a deleted
    // worktree. Naming it would put a checkout on screen that is not there.
    if !git_dir.is_dir() {
        return None;
    }
    Some(git_dir.file_name()?.to_string_lossy().into_owned())
}

fn head_of(git_dir: &Path) -> Option<String> {
    let head = read_bounded(&git_dir.join("HEAD"))?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        let name = reference.trim();
        if name.is_empty() {
            return None;
        }
        return Some(vitrum_fmt::git::strip_ref_prefix(name).to_string());
    }
    // A detached HEAD is the object id. Anything that is not one is garbage
    // from an interrupted operation and must not reach the sidebar.
    if head.len() >= 7 && head.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(vitrum_fmt::git::short_commit(head).to_string());
    }
    None
}

/// Read a small metadata file, refusing anything that is not one.
///
/// The size and the type are checked before the open, so a fifo cannot park the
/// spawn path and a directory-sized file cannot be allocated for. `O_NONBLOCK`
/// closes the window between the check and the open on unix, where a fifo
/// swapped in after the `stat` would otherwise still block.
fn read_bounded(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > HEAD_MAX {
        return None;
    }
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .ok()?
    };
    #[cfg(not(unix))]
    let file = std::fs::File::open(path).ok()?;

    let mut text = String::with_capacity(meta.len() as usize);
    let mut file = file.take(HEAD_MAX);
    std::io::Read::read_to_string(&mut file, &mut text).ok()?;
    Some(text)
}

// ---------------------------------------------------------------------------
// The manager
// ---------------------------------------------------------------------------

/// Every session this daemon hosts.
pub struct SessionManager {
    sessions: Mutex<BTreeMap<SessionId, Arc<Session>>>,
    next_id: AtomicU64,
    next_viewer: AtomicU64,
    scrollback_bytes: usize,
}

impl SessionManager {
    /// A manager whose sessions each retain `scrollback_bytes` of output.
    pub fn new(scrollback_bytes: usize) -> Self {
        warn_once_about_terminfo();
        Self {
            sessions: Mutex::new(BTreeMap::new()),
            // Zero is reserved as "no session" by every client that stores an
            // optional focus.
            next_id: AtomicU64::new(1),
            next_viewer: AtomicU64::new(1),
            scrollback_bytes,
        }
    }

    /// Start a child under a new pseudoterminal.
    ///
    /// Every refusal here happens before anything is created, so a failed spawn
    /// leaves no half-session in the registry.
    pub fn spawn(&self, spec: SessionSpec) -> anyhow::Result<SessionId> {
        if spec.command.trim().is_empty() {
            return Err(SessionError::EmptyCommand.into());
        }
        if !spec.cwd.is_dir() {
            return Err(SessionError::MissingCwd {
                cwd: spec.cwd.to_string_lossy().into_owned(),
            }
            .into());
        }
        // The coalescing window needs a timer. Without this the caller gets a
        // panic from deep inside Tokio with no hint about the cause.
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| anyhow!("a session needs a Tokio runtime to coalesce its output; \
                                  call spawn from inside one"))?;
        resolvable(&spec.command, &spec.cwd)?;

        let cols = spec.cols.max(1);
        let rows = spec.rows.max(1);
        let pair = portable_pty::native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| SessionError::CannotStart {
                command: spec.command.clone(),
                detail: e.to_string(),
            })?;

        let mut cmd = CommandBuilder::new(&spec.command);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        for (key, value) in DEFAULT_TERM_ENV {
            cmd.env(key, value);
        }
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| SessionError::CannotStart {
                command: spec.command.clone(),
                detail: e.to_string(),
            })?;
        // The slave descriptor has to go, or the reader never sees end of
        // stream because this process is still holding the terminal open.
        drop(pair.slave);

        let child_pid = child.process_id();
        let killer = child.clone_killer();
        #[cfg(windows)]
        let job = windows_job::JobObject::containing(&*child);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| SessionError::CannotStart {
                command: spec.command.clone(),
                detail: e.to_string(),
            })?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| SessionError::CannotStart {
                command: spec.command.clone(),
                detail: e.to_string(),
            })?;

        let id = SessionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let now = now_ms();
        let agent = AgentKind::of(&spec.command);
        let title_pinned = spec.title.is_some();
        let title = spec
            .title
            .unwrap_or_else(|| basename(&spec.command).to_string());

        let (status_tx, _) = watch::channel(SessionStatus::Starting);
        let (obs_tx, _) = watch::channel(0u64);
        let (arm_tx, arm_rx) = watch::channel(0u64);
        let (output, _) = broadcast::channel(OUTPUT_QUEUE);
        let (input_tx, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        let counts = Arc::new(Counts::default());
        let closed = Arc::new(Flag::default());
        let eof = Arc::new(Flag::default());

        let git = git_context(&spec.cwd);
        let session = Arc::new(Session {
            id,
            master: Mutex::new(pair.master),
            state: Mutex::new(State {
                project_id: spec.project_id,
                git_branch: git.branch,
                worktree: git.worktree,
                cwd: spec.cwd,
                command: spec.command,
                args: spec.args,
                agent,
                title,
                title_pinned,
                term_title: None,
                status: SessionStatus::Starting,
                created_at_ms: now,
                last_activity_ms: now,
                last_output_ms: 0,
                last_focus_ms: 0,
                cols,
                rows,
                bell: false,
                failed: false,
                waiting: None,
                hint: None,
                viewers: Vec::new(),
            }),
            scroll: Mutex::new(Scrollback::with_capacity(self.scrollback_bytes)),
            input: Mutex::new(Some(input_tx)),
            output,
            status_tx,
            obs_tx,
            arm_tx,
            counts: Arc::clone(&counts),
            closed: Arc::clone(&closed),
            probes: AtomicU64::new(0),
            resizes: AtomicU64::new(0),
            killer: Mutex::new(killer),
            child_pid,
            #[cfg(windows)]
            job: Mutex::new(job),
        });

        let (read_tx, read_rx) = mpsc::channel::<BytesMut>(READ_QUEUE);
        let (exit_tx, exit_rx) = oneshot::channel::<ExitStatus>();

        // Who reaps the child and ends the stream differs by platform, and it
        // has to be a `cfg` rather than a runtime branch because both arms own
        // the child and the exit channel.
        //
        // On unix the reader reaches end of stream when the last descriptor for
        // the slave closes, so it reaps the child itself: end of stream first,
        // THEN the exit code, because a client that stops streaming on `Exited`
        // must not lose the child's last words, which are usually the error
        // message explaining the exit.
        //
        // On Windows nothing closes the read side while this process holds the
        // pseudoconsole master, so the reader never returns. A second thread
        // waits on the child and ends the stream one flush window after the
        // exit.
        //
        // Written as one thread with a runtime `if`, both closures captured
        // `child` and `exit_tx` and the Windows build did not compile at all.
        #[cfg(not(windows))]
        std::thread::Builder::new()
            .name(format!("vitrum-pty-read-{}", id.0))
            .spawn({
                let counts = Arc::clone(&counts);
                move || {
                    read_loop(reader, read_tx, &counts);
                    let status = child.wait().ok();
                    let _ = exit_tx.send(status.unwrap_or_else(|| ExitStatus::with_exit_code(1)));
                }
            })
            .map_err(|e| anyhow!("could not start the pty reader thread: {e}"))?;

        #[cfg(windows)]
        {
            std::thread::Builder::new()
                .name(format!("vitrum-pty-read-{}", id.0))
                .spawn({
                    let counts = Arc::clone(&counts);
                    move || read_loop(reader, read_tx, &counts)
                })
                .map_err(|e| anyhow!("could not start the pty reader thread: {e}"))?;

            std::thread::Builder::new()
                .name(format!("vitrum-pty-reap-{}", id.0))
                .spawn({
                    let eof = Arc::clone(&eof);
                    move || {
                        let status = child.wait().ok();
                        std::thread::sleep(FLUSH_WINDOW);
                        eof.raise();
                        let _ = exit_tx
                            .send(status.unwrap_or_else(|| ExitStatus::with_exit_code(1)));
                    }
                })
                .map_err(|e| anyhow!("could not start the pty reaper thread: {e}"))?;
        }

        std::thread::Builder::new()
            .name(format!("vitrum-pty-write-{}", id.0))
            .spawn(move || write_loop(writer, input_rx))
            .map_err(|e| anyhow!("could not start the pty writer thread: {e}"))?;

        let pump = Pump {
            rx: read_rx,
            batch: Vec::with_capacity(BATCH_READS),
            counts,
            closed,
            eof,
        };
        runtime.spawn(output_loop(Arc::clone(&session), pump, exit_rx));
        runtime.spawn(probe_loop(Arc::clone(&session), arm_rx));
        // Arm the probe once for a session that has not written anything yet.
        // An agent that opens on a prompt, and a child that reads before it
        // prints, both produce no output at all, and the only other arming
        // sites are a published run and a keystroke. Without this the first
        // observation of a blocked session would wait for activity that is not
        // coming, and the row would spin instead of saying it is your turn.
        // The probe disarms itself after answering, so a session nobody touches
        // is still probed exactly once.
        session.arm_probe();

        lock(&self.sessions).insert(id, session);
        Ok(id)
    }

    /// Every session, ordered by id so the sidebar does not reshuffle.
    pub fn list(&self) -> Vec<SessionInfo> {
        lock(&self.sessions)
            .values()
            .map(|s| s.snapshot())
            .collect()
    }

    pub fn info(&self, id: SessionId) -> Option<SessionInfo> {
        self.get(id).map(|s| s.snapshot())
    }

    pub(crate) fn get(&self, id: SessionId) -> Option<Arc<Session>> {
        lock(&self.sessions).get(&id).cloned()
    }

    fn require(&self, id: SessionId) -> Result<Arc<Session>, SessionError> {
        self.get(id).ok_or(SessionError::NoSuchSession { id })
    }

    /// Remove a session and end its child.
    ///
    /// The row leaves the sidebar immediately. Reaping is asynchronous, because
    /// the operator clicked a button and has no reason to wait for a child that
    /// may be ignoring its hangup.
    pub fn close(&self, id: SessionId) -> anyhow::Result<()> {
        let session = lock(&self.sessions)
            .remove(&id)
            .ok_or(SessionError::NoSuchSession { id })?;
        session.closed.raise();
        session.hang_up();
        Ok(())
    }

    /// Close every session, and say how many there were.
    pub fn close_all(&self) -> usize {
        let all: Vec<Arc<Session>> = lock(&self.sessions).values().cloned().collect();
        lock(&self.sessions).clear();
        for session in &all {
            session.closed.raise();
            session.hang_up();
        }
        all.len()
    }

    /// Queue bytes for the child.
    ///
    /// Never blocks. A pty write blocks once the terminal's input buffer fills,
    /// so writing inline would wedge a runtime worker on a keystroke and a
    /// single paste into a stopped agent would stall unrelated sessions.
    pub fn write(&self, id: SessionId, data: &[u8]) -> anyhow::Result<()> {
        let session = self.require(id)?;
        // An empty write puts not a byte into the terminal, so the child cannot
        // see it. It is still operator activity, and that is the re-arm.
        if data.is_empty() {
            if !lock(&session.state).status.is_live() {
                return Err(SessionError::Exited { id }.into());
            }
            session.arm_probe();
            return Ok(());
        }
        let queued = {
            let queue = lock(&session.input);
            match queue.as_ref() {
                Some(tx) => tx.send(data.to_vec()).is_ok(),
                None => false,
            }
        };
        if !queued {
            return Err(SessionError::Exited { id }.into());
        }
        session.arm_probe();
        Ok(())
    }

    /// Rename a session. The name is the daemon's, so the daemon validates it.
    pub fn rename(&self, id: SessionId, title: &str) -> anyhow::Result<()> {
        let session = self.require(id)?;
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(anyhow!(
                "session {} cannot be renamed to an empty or whitespace-only \
                 title. Type a name, or leave the session with the one it has.",
                id.0
            ));
        }
        {
            let mut st = lock(&session.state);
            st.title = trimmed.to_string();
            // The operator renamed it precisely because they did not like what
            // the program called it. The next prompt does not get to undo that.
            st.title_pinned = true;
        }
        session.observe();
        Ok(())
    }

    /// A fresh window identity. Geometry is per window, so each needs one.
    pub fn new_viewer(&self) -> ViewerId {
        ViewerId(self.next_viewer.fetch_add(1, Ordering::Relaxed))
    }

    /// How many windows are attached.
    ///
    /// The daemon releases an attachment when the connection that made it goes
    /// away, and nothing else observes that: a client that vanished still holds
    /// the session's geometry down until it is dropped.
    pub fn watchers(&self, id: SessionId) -> Option<usize> {
        self.get(id).map(|s| lock(&s.state).viewers.len())
    }

    /// The child's process id, while it has one.
    ///
    /// Exposed because two things outside this crate have to name the process
    /// rather than the session: the overlap tracker, which asks the operating
    /// system what a session's child is holding open, and shutdown, which has
    /// to be able to prove a child that ignores its hangup is gone.
    pub fn child_pid(&self, id: SessionId) -> Option<u32> {
        self.get(id).and_then(|s| s.child_pid)
    }

    /// Attach a window and start receiving output from the next run on.
    ///
    /// No backlog: history is an explicit scrollback request, so a client that
    /// also backfills does not paint every byte twice.
    pub fn attach(
        &self,
        id: SessionId,
        viewer: ViewerId,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<broadcast::Receiver<OutputChunk>> {
        let session = self.require(id)?;
        let rx = session.output.subscribe();
        let changed = {
            let mut st = lock(&session.state);
            st.viewers.retain(|(v, _, _)| *v != viewer);
            st.viewers.push((viewer, cols.max(1), rows.max(1)));
            // Somebody is looking, which acknowledges everything that was
            // waiting for them to look.
            st.last_focus_ms = now_ms();
            st.bell = false;
            st.failed = false;
            session.apply_geometry(&mut st)
        };
        if changed {
            session.observe();
        }
        Ok(rx)
    }

    /// Drop a window's constraint. Silent for a window that is not attached,
    /// because a tab switch legitimately detaches twice.
    pub fn detach(&self, id: SessionId, viewer: ViewerId) {
        let Some(session) = self.get(id) else {
            return;
        };
        let changed = {
            let mut st = lock(&session.state);
            let before = st.viewers.len();
            st.viewers.retain(|(v, _, _)| *v != viewer);
            if st.viewers.len() == before {
                return;
            }
            session.apply_geometry(&mut st)
        };
        if changed {
            session.observe();
        }
    }

    /// Restate a window's geometry.
    ///
    /// A viewer that never attached changes nothing: a window laying out a
    /// session it is not drawing must not reflow the child for whoever is.
    pub fn resize(
        &self,
        id: SessionId,
        viewer: ViewerId,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<()> {
        let session = self.require(id)?;
        let changed = {
            let mut st = lock(&session.state);
            let Some(slot) = st.viewers.iter_mut().find(|(v, _, _)| *v == viewer) else {
                return Ok(());
            };
            slot.1 = cols.max(1);
            slot.2 = rows.max(1);
            session.apply_geometry(&mut st)
        };
        if changed {
            session.observe();
        }
        Ok(())
    }

    pub fn subscribe_status(&self, id: SessionId) -> Option<watch::Receiver<SessionStatus>> {
        self.get(id).map(|s| s.status_tx.subscribe())
    }

    /// Wakes whenever the projection changed in a way a sidebar redraws for.
    pub fn subscribe_observations(&self, id: SessionId) -> Option<watch::Receiver<u64>> {
        self.get(id).map(|s| s.obs_tx.subscribe())
    }

    /// Retained bytes ending just before `before_seq`, newest page first.
    ///
    /// Returns the offset of the first byte handed back, the bytes, and whether
    /// anything older is still retained. `u64::MAX` is the agreed way to say
    /// "everything up to now".
    pub fn scrollback(
        &self,
        id: SessionId,
        before_seq: u64,
        max_bytes: usize,
    ) -> Option<(u64, Vec<u8>, bool)> {
        let session = self.get(id)?;
        let ring = lock(&session.scroll);
        let oldest = ring.oldest_seq();
        let end = before_seq.min(ring.head_seq()).max(oldest);
        let take = ((end - oldest) as usize).min(max_bytes);
        let from = end - take as u64;
        let bytes = ring.range(from, take).unwrap_or_default();
        Some((from, bytes, from > oldest))
    }

    /// Read the whole retained ring in place, without copying it.
    ///
    /// The two slices are the ring's halves in order. Searching a session's
    /// history has no business allocating a copy of it first.
    pub fn with_scrollback<R>(
        &self,
        id: SessionId,
        f: impl FnOnce(u64, &[u8], &[u8]) -> R,
    ) -> Option<R> {
        let session = self.get(id)?;
        let ring = lock(&session.scroll);
        let (first, second) = ring.halves();
        Some(f(ring.oldest_seq(), first, second))
    }

    /// How many times the foreground process has been asked what it is doing.
    pub fn probe_count(&self, id: SessionId) -> Option<u64> {
        self.get(id)
            .map(|s| s.probes.load(Ordering::Relaxed))
    }

    /// How many resize ioctls this session's pty has taken.
    pub fn resize_count(&self, id: SessionId) -> Option<u64> {
        self.get(id)
            .map(|s| s.resizes.load(Ordering::Relaxed))
    }

    /// What the output path has spent on this session.
    pub fn pump_counts(&self, id: SessionId) -> Option<PumpCounts> {
        self.get(id).map(|s| s.counts.snapshot())
    }
}

impl Drop for SessionManager {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// The command's file name, split on BOTH separators.
///
/// `Path` uses the host's rules, so on Linux a Windows-style command has no
/// components at all and would be its own basename.
fn basename(command: &str) -> &str {
    command
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(command)
}

/// Refuse a command the daemon cannot find before a pty is created for it.
///
/// The rules and the search both come from [`crate::command_path`], which
/// implements Windows resolution as well as POSIX resolution and is exercised
/// for both on every host. `cwd` is the directory the session will start in,
/// because that is what a relative command name is resolved against.
fn resolvable(command: &str, cwd: &Path) -> Result<(), SessionError> {
    let rules = command_path::SpawnRules::host();
    let exists = |p: &Path| p.is_file();
    let search = command_path::Search::for_host(rules, cwd, &exists);
    command_path::resolve(rules, command, &search)
}

/// Warn once if the host cannot describe the terminal children are told they
/// have.
///
/// The value is not changed to suit the host: a session that rendered
/// differently depending on which machine the daemon runs on would be worse
/// than one constant claim. A child on a host without the entry falls back to
/// its own handling of an unknown `TERM`.
///
/// The search order and the fix are per-host and come from [`crate::terminfo`].
fn warn_once_about_terminfo() {
    static CHECKED: std::sync::Once = std::sync::Once::new();
    CHECKED.call_once(|| {
        let Some((_, name)) = DEFAULT_TERM_ENV.iter().find(|(k, _)| *k == "TERM") else {
            return;
        };
        let env = terminfo::TermEnv::from_process();
        let exists = |p: &Path| p.exists();
        match terminfo::check(std::env::consts::OS, name, &env, &exists) {
            terminfo::TerminfoCheck::Present => {}
            terminfo::TerminfoCheck::Absent { advice } => tracing::warn!(
                term = name,
                "no terminfo entry for the terminal type sessions are told they have; {advice}"
            ),
            terminfo::TerminfoCheck::Unguided { host } => tracing::warn!(
                term = name,
                host = %host,
                guided = %terminfo::guided_hosts().collect::<Vec<_>>().join(", "),
                "no terminfo entry for the terminal type sessions are told they have, and vitrum \
                 has no guidance for this host; install the entry the way this platform expects, \
                 or full-screen programs will fall back to their built-in handling of an unknown \
                 TERM"
            ),
        }
    });
}

// ---------------------------------------------------------------------------
// The three loops
// ---------------------------------------------------------------------------

/// Carve consecutive reads out of one arena and hand them on.
fn read_loop(mut reader: Box<dyn Read + Send>, tx: mpsc::Sender<BytesMut>, counts: &Counts) {
    // Zeroed once per arena rather than once per read. A megabyte of output
    // therefore costs about one megabyte of writes to initialise, not the
    // seventy it cost when every read allocated its own buffer.
    let mut arena = BytesMut::zeroed(READ_ARENA);
    Counts::bump(&counts.arenas);
    loop {
        if arena.len() < READ_FLOOR {
            arena = BytesMut::zeroed(READ_ARENA);
            Counts::bump(&counts.arenas);
        }
        let taken = match reader.read(&mut arena[..]) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        Counts::bump(&counts.reads);
        // The remainder stays adjacent to what was just carved off, which is
        // what lets the pump rejoin a run without copying it.
        let chunk = arena.split_to(taken);
        if tx.blocking_send(chunk).is_err() {
            break;
        }
    }
}

/// Drain the input queue onto the terminal.
fn write_loop(mut writer: Box<dyn Write + Send>, rx: std::sync::mpsc::Receiver<Vec<u8>>) {
    while let Ok(data) = rx.recv() {
        if writer.write_all(&data).and_then(|()| writer.flush()).is_err() {
            break;
        }
    }
}

/// Publish runs until the stream ends, then record the outcome.
async fn output_loop(
    session: Arc<Session>,
    mut pump: Pump,
    exit: oneshot::Receiver<ExitStatus>,
) {
    let mut scan = OutputScan::new();
    let mut refused = 0;
    while let Some(run) = pump.next_run().await {
        session.publish(run.freeze(), &mut scan);
        // A harness that emits a declaration the daemon will not take gets no
        // feedback in band, so the count is surfaced here or nowhere: an agent
        // author debugging a sequence that does nothing needs to see that it
        // arrived and was refused, not silence.
        let rejected = scan.rejected_hints();
        if rejected > refused {
            tracing::debug!(
                session = session.id.0,
                rejected,
                "the session declared a hint this daemon could not read"
            );
            refused = rejected;
        }
    }
    let closed = Arc::clone(&session.closed);
    let status = await_exit(exit, &closed, || session.kill_group()).await;
    session.finish(status);
}

/// Ask the operating system what the foreground process is doing, once the
/// session has been quiet for a settle window.
///
/// Armed by a published run and by operator input; disarmed by its own answer.
/// It never re-arms itself, so a session nobody is touching costs nothing.
async fn probe_loop(session: Arc<Session>, mut armed: watch::Receiver<u64>) {
    loop {
        if armed.changed().await.is_err() {
            return;
        }
        // Coalesce: while the arm keeps being renewed, the session is still
        // producing and a probe would be measuring the wrong instant.
        loop {
            match tokio::time::timeout(SETTLE_WINDOW, armed.changed()).await {
                Ok(Ok(())) => continue,
                Ok(Err(_)) => return,
                Err(_) => break,
            }
        }
        if session.closed.is_raised() {
            return;
        }
        let live = {
            let st = lock(&session.state);
            st.status.is_live()
        };
        if !live {
            return;
        }
        // The master lock is released before the state lock is taken. Resize
        // takes them the other way round, so overlapping them here would be an
        // inversion.
        let answer = probe::waiting(&**lock(&session.master));
        Counts::bump(&session.probes);
        let changed = {
            let mut st = lock(&session.state);
            let changed = st.waiting != answer;
            st.waiting = answer;
            changed
        };
        if changed {
            session.observe();
        }
    }
}

// ---------------------------------------------------------------------------
// Windows job objects
// ---------------------------------------------------------------------------

/// A Windows process is not the parent of anything the kernel tracks, so
/// killing a session's child leaves its children running. The child is put in a
/// job at spawn and the job is closed when the session is, which is the only
/// mechanism that reaches a grandchild.
#[cfg(windows)]
mod windows_job {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// An open job handle. Dropping it terminates every process inside.
    #[derive(Debug)]
    pub(super) struct JobObject(HANDLE);

    // SAFETY: a job handle is a kernel object usable from any thread.
    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}

    impl JobObject {
        /// A job holding `child`, or `None` if any step failed.
        ///
        /// Failure only logs: a session that cannot get a job is still a
        /// session, and the close then reaches the child but not its
        /// descendants.
        pub(super) fn containing(child: &dyn portable_pty::Child) -> Option<Self> {
            let pid = child.process_id()?;
            // SAFETY: no pointers in; a null name makes an anonymous job.
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                tracing::warn!("could not create a job object; a grandchild may outlive its session");
                return None;
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` is a live local of exactly the named size.
            let set = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_mut(&mut limits).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            // SAFETY: no pointers in.
            let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            if set == 0 || handle.is_null() {
                // SAFETY: closing handles this function opened.
                unsafe { CloseHandle(job) };
                tracing::warn!("could not arm a job object; a grandchild may outlive its session");
                return None;
            }
            // SAFETY: both handles are open and owned here.
            let assigned = unsafe { AssignProcessToJobObject(job, handle) };
            // SAFETY: the process handle is not used again.
            unsafe { CloseHandle(handle) };
            if assigned == 0 {
                // SAFETY: closing a handle this function opened.
                unsafe { CloseHandle(job) };
                tracing::warn!("could not assign the child to a job object; a grandchild may outlive its session");
                return None;
            }
            Some(JobObject(job))
        }
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            // SAFETY: the handle was opened here and is dropped once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// The output pump with the child taken away, so its cost can be counted
/// exactly.
///
/// Against a live session the number of reads waiting when `recv_many` runs is
/// a scheduling outcome, which makes every ratio a property of the machine.
/// Here the reads are queued before anything polls the channel, so the bounds
/// are exact.
#[cfg(all(test, not(windows)))]
pub(crate) struct Coalescer {
    pump: tokio::sync::Mutex<Option<Pump>>,
    tx: Option<mpsc::Sender<BytesMut>>,
    arena: Mutex<BytesMut>,
    counts: Arc<Counts>,
    closed: Arc<Flag>,
    /// Opened and held so the harness owns a real terminal, exactly as a
    /// session does. Never read from.
    _master: Box<dyn MasterPty + Send>,
}

#[cfg(all(test, not(windows)))]
impl Coalescer {
    pub(crate) fn new() -> anyhow::Result<Self> {
        let pair = portable_pty::native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        drop(pair.slave);
        let (tx, rx) = mpsc::channel::<BytesMut>(READ_QUEUE);
        let counts = Arc::new(Counts::default());
        let closed = Arc::new(Flag::default());
        Ok(Self {
            pump: tokio::sync::Mutex::new(Some(Pump {
                rx,
                batch: Vec::with_capacity(BATCH_READS),
                counts: Arc::clone(&counts),
                closed: Arc::clone(&closed),
                eof: Arc::new(Flag::default()),
            })),
            tx: Some(tx),
            arena: Mutex::new(BytesMut::zeroed(READ_ARENA)),
            counts,
            closed,
            _master: pair.master,
        })
    }

    /// Put one read on the queue, carved out of an arena the way the reader
    /// thread carves one. Not counted as a read: no syscall happened.
    pub(crate) fn queue(&self, data: &[u8]) {
        let chunk = {
            let mut arena = lock(&self.arena);
            if arena.len() < data.len() {
                *arena = BytesMut::zeroed(READ_ARENA);
            }
            arena[..data.len()].copy_from_slice(data);
            arena.split_to(data.len())
        };
        self.tx
            .as_ref()
            .expect("the harness queue is open")
            .try_send(chunk)
            .expect("the harness queue is deeper than any test fills it");
    }

    /// Take one run and report what it cost.
    pub(crate) async fn drain(&self) -> PumpCounts {
        let mut held = self.pump.lock().await;
        let pump = held.as_mut().expect("the pump has not been taken");
        pump.next_run().await;
        self.counts.snapshot()
    }

    /// Close a coalescer whose terminal has ended but whose child nothing will
    /// reap, and report whether the loop came back within `within`.
    ///
    /// Against a wait that can only observe the exit this does not fail, it
    /// hangs, which is why the bound is here rather than in an assertion.
    pub(crate) async fn close_while_unreaped(&self, within: Duration) -> bool {
        self.run_tail(within, true).await
    }

    /// The same, for a coalescer still parked on a terminal that is open.
    pub(crate) async fn close_while_reading(&self, within: Duration) -> bool {
        self.run_tail(within, false).await
    }

    /// Run the whole output loop with no child behind it, closing it partway.
    ///
    /// `end_stream` drops the queue so the read loop reaches end of stream; not
    /// dropping it leaves the loop parked on a reader that will never speak,
    /// which is what a live session looks like.
    async fn run_tail(&self, within: Duration, end_stream: bool) -> bool {
        let mut pump = self
            .pump
            .lock()
            .await
            .take()
            .expect("the pump has not been taken");
        if end_stream {
            // Cloned senders are all that keep the channel open; the harness
            // holds the only one.
            pump.rx.close();
        }
        // Held for the whole run, so nothing can reap the child.
        let (_never_reaped, exit) = oneshot::channel::<ExitStatus>();
        let closed = Arc::clone(&self.closed);
        tokio::spawn({
            let closed = Arc::clone(&closed);
            async move {
                // Long enough for the loop to reach its park, short enough that
                // the bound above is not what ends the test.
                tokio::time::sleep(Duration::from_millis(20)).await;
                closed.raise();
            }
        });
        let loop_done = async {
            while pump.next_run().await.is_some() {}
            await_exit(exit, &closed, || {}).await;
        };
        tokio::time::timeout(within, loop_done).await.is_ok()
    }
}
