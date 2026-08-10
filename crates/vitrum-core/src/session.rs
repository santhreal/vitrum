//! PTY-backed sessions and the registry that owns them.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use bytes::{Bytes, BytesMut};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use vitrum_model::AgentKind;
use vitrum_model::hint::HintDeclaration;
use vitrum_proto::{Attention, SessionId, SessionInfo, SessionStatus, display_safe};
use tokio::sync::{Notify, broadcast, mpsc, oneshot, watch};
use tokio::time::{Instant, timeout_at};

use crate::SessionError;
use crate::Scrollback;
use crate::scan::OutputScan;

/// Size of one blocking PTY read.
///
/// This is a ceiling on a read, not a promise about one, and now that the
/// engine and the staging copy are both gone it is the second lever on a path
/// whose remaining cost is syscalls: the pipeline benchmark reports about 2100
/// reads per megabyte, which is an average read of roughly 500 bytes.
///
/// The bound that produces that number is the kernel's, not this constant's.
/// A pty master is read through the `N_TTY` line discipline, whose receive
/// buffer `N_TTY_BUF_SIZE` is 4096 bytes, so one `read` cannot return more
/// than 4 KiB however large a slice it is handed; under a firehose it returns
/// far less than that, because the reader is draining the ring about as fast
/// as the child fills it and a read gets only what accumulated since the last
/// one.
///
/// So raising this buys nothing. Reads per megabyte fall only if more bytes
/// are waiting per read, and the ways to arrange that are a reader that
/// deliberately lags, which spends latency to buy throughput, or a different
/// kernel interface. Lowering it is the change that can hurt: a chunk under
/// 4 KiB splits a full line-discipline buffer across several syscalls in
/// exactly the case where the kernel had a large read to give.
///
/// The size is therefore set by the two other things it controls. It is the
/// arena's `READ_FLOOR`, so a tail shorter than this is abandoned rather than
/// read into, which wastes at most 32 KiB of a 1 MiB arena. And it is what a
/// Windows pseudoconsole may return in one go, where there is no line
/// discipline and a read is bounded only by what the pipe holds. 32 KiB is
/// large enough to matter there and costs 3% of an arena here.
const READ_CHUNK: usize = 32 * 1024;

/// One allocation the reader hands out reads from.
///
/// Two things want this large. A big zeroed block comes straight from the
/// kernel as untouched zero pages, so it costs no memset, while a 32 KiB one
/// is served from the heap and is cleared byte by byte. And consecutive reads
/// carved from one block are contiguous, which is what lets the coalescer
/// rejoin a merged run in place instead of copying it: a run only copies when
/// it straddles two arenas, so an arena comfortably larger than `FLUSH_BYTES`
/// makes that the rare case.
const READ_ARENA: usize = 1024 * 1024;

/// Smallest remaining arena worth reading into before it is replaced.
///
/// A read is only as large as what is left, so leaving less than a whole
/// `READ_CHUNK` in place would turn one syscall into several for the same
/// bytes. The tail below this is dropped rather than read into.
const READ_FLOOR: usize = READ_CHUNK;

/// The longest a byte may sit in the coalescing buffer.
///
/// This is the whole reason a firehose does not become thousands of broadcast
/// wakeups per second. It is a ceiling, not a schedule: a run that goes quiet
/// is published before this, and only a child that keeps writing holds a run
/// open all the way to it. The timer is armed only while a run is open, so an
/// idle session costs zero wakeups and zero CPU.
///
/// # What it costs a byte
///
/// A byte waits until whichever deadline ends its run, and the cap is only one
/// of the two. A lone write ends on `FLUSH_IDLE`, so an echoed keystroke's
/// share of this constant is the 300 µs gap and not 6 ms. A run held open by
/// continuing output is bounded by the cap, and a byte arriving uniformly
/// inside such a run waits `FLUSH_WINDOW / 2`, so 3 ms on average and 6 ms at
/// worst.
///
/// The cap also governs only a middle band of rates. At the measured
/// single-session throughput of 181 MB/s a run reaches `FLUSH_BYTES` in about
/// 0.35 ms, so a firehose is published on bytes and never meets the clock at
/// all; the sessions whose runs end on the cap are those writing under about
/// 11 MB/s, which is `FLUSH_BYTES` divided by this window.
///
/// Measured interactive p50 for the whole path — write, pty, line-discipline
/// echo, read, coalesce, scan, broadcast — is 7.14 ms. That is larger than the
/// entire cap, so the cap cannot be most of it, and for the single echoed byte
/// that sample writes the coalescer's own contribution is the idle gap rather
/// than this window. Where the rest of that 7 ms goes is not visible from
/// these counters, and it is not a reason to move this constant: per-stage
/// timestamps on one sample are what would settle it.
///
/// # Why not smaller
///
/// A publish is a broadcast send, a scrollback insert and a frame for every
/// attached client. In the band where the clock governs, publishes scale as
/// the reciprocal of this window, so halving it doubles that bill across every
/// streaming session at once — and buys output whose arrival nobody can
/// perceive being 3 ms earlier. Twenty agents each writing a steady kilobyte
/// of log is where the doubling is paid.
///
/// # Why a constant rather than an adaptive window
///
/// An adaptive window would have to earn its complexity on evidence this loop
/// does not currently collect. It would need to observe, per session and
/// cheaply enough not to cost a timestamp per read: the distribution of gaps
/// between reads, so it can predict whether another read is coming before the
/// deadline instead of discovering it afterwards; how many clients are
/// attached, because that is the multiplier on every publish it saves; and
/// whether any of them is a viewer a human is actually looking at, because a
/// background session gains nothing from a short window.
///
/// `FLUSH_IDLE` is already the cheap one-sample form of the first of those,
/// and it took the keystroke off the cap without measuring anything. What is
/// left — whether a session may hold bytes for longer than 6 ms when nobody is
/// watching it — is a policy about attention rather than a tuning parameter,
/// and it needs the attention signal to exist first.
const FLUSH_WINDOW: Duration = Duration::from_millis(6);

/// How long a run waits for more output before giving up on it.
///
/// Set below the gap between two reads of a child that is still writing and
/// far below the cap, so a firehose still batches to the cap while an echoed
/// keystroke, which is one read and then silence, is published as soon as the
/// silence is established rather than at the end of a fixed window.
const FLUSH_IDLE: Duration = Duration::from_micros(300);

/// Publish early once this much output is pending, so a burst is not delayed by
/// the full window and no single chunk grows unbounded.
const FLUSH_BYTES: usize = 64 * 1024;

/// Reads the coalescer takes from the channel in one wakeup.
///
/// Enough that a firehose is drained in a few passes and small enough that the
/// vector behind it stays a fixed, tiny allocation. It bounds work per wakeup,
/// not work per window: the loop goes round again while the window is open.
pub(crate) const BATCH_READS: usize = 64;

/// Whether the reader thread reaches end of stream on its own.
///
/// On Unix it does: the master reports EOF once the child's last descriptor
/// closes, so the raw channel closing IS end of output and is worth waiting
/// for. A Windows pseudoconsole keeps the read side open for as long as the
/// session holds its master, so the reader there stays parked until the
/// session is closed and the exit has to be the end of the stream instead.
const READER_REPORTS_EOF: bool = cfg!(not(windows));

/// What every hosted child is told about the terminal it is attached to.
///
/// `TERM` because an unset or `dumb` value makes an agent drop to plain output;
/// this is the whole reason a hosted TUI renders at all.
///
/// `COLORTERM` because the renderer is 24-bit end to end — the engine stores
/// every cell as RGBA and `38;2;r;g;b` is parsed exactly — and the convention
/// for saying so is this variable, not the `TERM` name. Without it agents
/// quantise themselves to the 256-colour cube on purpose: Gemini CLI prints
/// "True color (24-bit) support not detected" and dims its own output, and
/// nothing about the pixels we can draw had changed. Advertising a capability
/// we do not have would be worse than silence, so this is asserted against the
/// engine rather than against a comment.
///
/// `TERM_PROGRAM` because agents branch on the host terminal for hyperlinks,
/// image protocols and paste behaviour, and an unidentified host gets the
/// conservative path. It is also what a bug report from an agent will name.
pub(crate) const DEFAULT_TERM_ENV: [(&str, &str); 3] = [
    ("TERM", "xterm-256color"),
    // Taken from the engine rather than written here, so the promise and the
    // code that has to keep it cannot drift apart.
    ("COLORTERM", vitrum_vt::COLORTERM),
    ("TERM_PROGRAM", "vitrum"),
];

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
    pub data: Bytes,
}

/// What one session's output path did, counted rather than timed.
///
/// Every field is a structural property of the pipeline, chosen so that a
/// regression in it is a change in a whole number rather than a change in a
/// duration:
///
/// - `reads` is PTY read syscalls that returned bytes. It divides the byte
///   count into how many trips to the kernel it took.
/// - `publishes` is coalesced runs handed to the broadcast channel, which is
///   one wakeup per attached client each. This is what `FLUSH_WINDOW` and
///   `FLUSH_BYTES` exist to hold down.
/// - `wakeups` is how many times the coalescing task was scheduled to collect
///   output. It is the difference between draining a channel and being woken
///   once per item in it, and it is the one cost that grows with how SMALL a
///   pty's reads are rather than with how much a session writes.
/// - `timers` is how many run windows were armed. One `sleep_until` covers a
///   whole run, so this tracks published runs and not read syscalls; the
///   defect it exists to catch — awaiting each read under its own
///   `timeout_at` — makes it track reads instead. Unlike `wakeups`, which
///   moves with how far ahead of the coalescer the reader thread happens to
///   get, this is a whole number the shape of the loop fixes.
/// - `staged_bytes` is bytes copied into the coalescer's staging buffer on
///   their way to being published. A run that consists of a single read is
///   published without being copied at all, so this stays BELOW the byte
///   count; it reaching parity means the copy came back.
/// - `parsed_bytes` is bytes the daemon walks itself, in the single scan that
///   finds the bell, the agent hint, the title and the directory. Every one of
///   these is parsed again by the client's own emulator, so it is the
///   duplicated work in the design and worth being able to see. It must stay
///   at one pass per published byte.
/// - `arenas` is reader-side allocations: one 1 MiB `READ_ARENA` block, taken
///   when the previous one is too short to read into again. It is the whole
///   allocation cost of the byte path, because nothing downstream of it
///   allocates per read or per run, so it stays at about one per megabyte of
///   output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PumpCounts {
    pub reads: u64,
    pub publishes: u64,
    pub wakeups: u64,
    pub timers: u64,
    pub staged_bytes: u64,
    pub parsed_bytes: u64,
    pub arenas: u64,
    /// Runs published because the child stopped writing.
    pub idle_flushes: u64,
    /// Runs published because they had been open for the whole cap.
    pub capped_flushes: u64,
}

/// The live counters behind [`PumpCounts`].
///
/// Relaxed throughout: each is a monotonic tally read for diagnosis, and no
/// decision anywhere depends on two of them being consistent with each other
/// at an instant. Ordering here would be paid on every 32 KiB read for an
/// answer nobody asks.
#[derive(Default)]
struct PumpTally {
    reads: AtomicU64,
    publishes: AtomicU64,
    wakeups: AtomicU64,
    timers: AtomicU64,
    staged_bytes: AtomicU64,
    parsed_bytes: AtomicU64,
    arenas: AtomicU64,
    idle_flushes: AtomicU64,
    capped_flushes: AtomicU64,
}

impl PumpTally {
    fn snapshot(&self) -> PumpCounts {
        PumpCounts {
            reads: self.reads.load(Ordering::Relaxed),
            publishes: self.publishes.load(Ordering::Relaxed),
            wakeups: self.wakeups.load(Ordering::Relaxed),
            timers: self.timers.load(Ordering::Relaxed),
            staged_bytes: self.staged_bytes.load(Ordering::Relaxed),
            parsed_bytes: self.parsed_bytes.load(Ordering::Relaxed),
            arenas: self.arenas.load(Ordering::Relaxed),
            idle_flushes: self.idle_flushes.load(Ordering::Relaxed),
            capped_flushes: self.capped_flushes.load(Ordering::Relaxed),
        }
    }
}

/// Server-side state for one live or exited session.
pub(crate) struct Session {
    pub(crate) id: SessionId,
    pub(crate) info: RwLock<SessionInfo>,
    /// Whether the name belongs to the operator rather than to the program.
    ///
    /// A shell sets its window title on every prompt, so a session the
    /// operator deliberately named would be renamed back by the next command
    /// they ran. Whoever named it last with intent keeps it: the creator when
    /// it was spawned with a title, the operator the moment they rename it.
    pub(crate) title_pinned: AtomicBool,
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
    /// Raised once by `close`, so a discarded session reaches a terminal state
    /// without depending on the master reporting EOF.
    ///
    /// Linux does not need it: the kernel hangs the controlling terminal up
    /// when the session leader exits, so a backgrounded grandchild still
    /// holding the slave cannot hold the stream open, which was measured
    /// rather than assumed. That guarantee is the tty layer's, not POSIX's,
    /// and `close` is where the operator has already said the session is
    /// finished, so it does not wait to find out whether this kernel offers
    /// it.
    closed: Notify,
    /// Queue to the dedicated writer thread. Input is queued rather than
    /// written inline because a child that has stopped reading its stdin makes
    /// a PTY write block indefinitely, which would wedge a runtime worker.
    ///
    /// Dropped when the session reaches a terminal state, which is what lets the
    /// writer thread exit: an exited session can never accept input again, and a
    /// thread parked on this queue for every finished session would accumulate
    /// for as long as the daemon runs.
    input: Mutex<Option<mpsc::UnboundedSender<Bytes>>>,
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
    /// What the output path actually did, in units nothing about it can fake.
    ///
    /// Wall clock cannot be asserted on: a loaded CI box makes any threshold
    /// either flaky or meaningless. These four counters are the structural
    /// shape of the path instead, and a change that reintroduces a copy, a
    /// syscall, or a wakeup moves one of them whatever the machine is doing.
    pump: PumpTally,
}

impl Session {
    /// Record and fan out one coalesced run of output.
    ///
    /// Takes the run by value. The bytes arrived from the reader in an
    /// allocation nobody else owns, so handing that allocation to the
    /// broadcast channel is a move; copying it out again would be a memcpy of
    /// the entire stream to produce a second copy of what is already here.
    fn publish(&self, data: Bytes, wants_operator: bool, hint: Option<&HintDeclaration>) {
        if data.is_empty() {
            return;
        }
        self.pump.publishes.fetch_add(1, Ordering::Relaxed);
        let seq = {
            let mut sb = lock(&self.scrollback);
            let seq = sb.head_seq();
            // Scrollback is filled whether or not anyone is attached: sessions
            // must survive with no GUI connected, and history is the product.
            sb.push(&data);
            seq
        };

        // Skipping the fan-out when nothing is attached matters: the normal
        // state of 20 agents is 19 unattached ones. A receiver that appears
        // between this check and the send simply starts at the next chunk and
        // backfills from scrollback, which is what it does anyway.
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
            let _ = self.output.send(OutputChunk { seq, data });
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
            return Err(anyhow!(SessionError::EmptyCommand));
        }
        if !spec.cwd.is_dir() {
            return Err(anyhow!(SessionError::MissingCwd {
                cwd: spec.cwd.display().to_string(),
            }));
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
        // What the terminal on the other end can do. Every one of these is
        // overridable by the caller, which is why each is guarded rather than
        // set unconditionally: a session started to reproduce a rendering bug
        // needs to be able to lie about all three.
        for (key, value) in DEFAULT_TERM_ENV {
            if !spec.env.iter().any(|(k, _)| k == key) {
                cmd.env(key, value);
            }
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
        let (input_tx, input_rx) = mpsc::unbounded_channel::<Bytes>();

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
            // Nothing announced yet either. A program that never sets a title
            // keeps this `None` for its whole life.
            term_title: None,
        };

        let session = Arc::new(Session {
            id,
            info: RwLock::new(info),
            title_pinned: AtomicBool::new(spec.title.is_some()),
            scrollback: Mutex::new(Scrollback::with_capacity(self.scrollback_bytes)),
            output,
            status: status_tx,
            observations: observations_tx,
            master: Mutex::new(pair.master),
            viewers: Mutex::new(BTreeMap::new()),
            resizes: AtomicU64::new(0),
            probes: AtomicU64::new(0),
            activity: Notify::new(),
            closed: Notify::new(),
            input: Mutex::new(Some(input_tx)),
            killer: Mutex::new(killer),
            last_focus_ms: AtomicU64::new(0),
            child_pid,
            pump: PumpTally::default(),
        });

        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<BytesMut>();
        let (exit_tx, exit_rx) = oneshot::channel::<Option<i32>>();

        // Dedicated threads, not spawn_blocking: these loops live as long as
        // the session, and parking a Tokio blocking-pool slot for a session's
        // whole lifetime would both exhaust that pool and make runtime
        // shutdown wait on a read that only ends when the child exits.
        std::thread::Builder::new()
            .name(format!("vitrum-pty-read-{}", id.0))
            .spawn({
                let session = Arc::clone(&session);
                move || read_loop(session, reader, child, raw_tx, exit_tx)
            })
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
            return Err(anyhow!(SessionError::Exited { id }));
        }
        {
            let queue = lock(&s.input);
            queue
                .as_ref()
                .ok_or_else(|| anyhow!("session {} pty writer is gone", id.0))?
                .send(Bytes::copy_from_slice(data))
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

    /// What this session's output path has done so far.
    ///
    /// Diagnostic, and the only honest way to assert that the pipeline still
    /// has the shape it was measured with: a duration says what this machine
    /// managed today, a count says what the code does.
    pub fn pump_counts(&self, id: SessionId) -> Option<PumpCounts> {
        Some(self.get(id)?.pump.snapshot())
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
        // From here the program's own title is ignored for this session. The
        // operator has said what it is called.
        s.title_pinned.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Kill the session's child and drop the session from the registry.
    ///
    /// The PTY and its threads unwind on their own once the child dies and the
    /// master reports EOF, so this never blocks on the child. The coalescer is
    /// told outright that the session is gone, rather than left waiting for an
    /// EOF whose timing belongs to the platform. See `Session::closed`.
    ///
    /// The hangup is followed by a kill on its own thread, because a child that
    /// handles `SIGHUP` would otherwise keep running with no session left to
    /// reach it through. Closing must not wait for that, so the escalation does
    /// not run here.
    pub fn close(&self, id: SessionId) -> anyhow::Result<()> {
        let s = self
            .sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
            .ok_or_else(|| anyhow!(SessionError::NoSuchSession { id }))?;
        // An already-exited child gives ESRCH here, which is not a failure to
        // close: the session is gone either way.
        if let Err(e) = lock(&s.killer).kill() {
            tracing::debug!(session = s.id.0, error = %e, "kill on close");
        }
        s.closed.notify_one();
        #[cfg(unix)]
        if let Some(pid) = s.child_pid {
            std::thread::spawn(move || {
                std::thread::sleep(CHILD_EXIT_GRACE);
                kill_survivors(&[pid]);
            });
        }
        Ok(())
    }

    /// Close every session, and report how many there were.
    ///
    /// What the daemon does on its way out. Process exit alone is nearly
    /// enough on Unix — the last master descriptor closing hangs the terminal
    /// up and the kernel signals each session leader — but nearly is the
    /// problem: a child that ignores `SIGHUP`, which is every agent started
    /// under `nohup` and anything that installs a handler, survives with a
    /// dead terminal and no way for the operator to reach it again. Sessions
    /// are ended deliberately here instead of being left to the kernel's
    /// courtesy.
    ///
    /// Takes the whole registry in one lock and then signals outside it, so a
    /// slow child cannot hold up the next one's. The escalation is waited for
    /// here rather than detached, because the caller is a process that is about
    /// to exit and a kill scheduled on a thread that never runs again is no
    /// kill at all. It costs one grace period for all sessions, not one each.
    pub fn close_all(&self) -> usize {
        let sessions: Vec<Arc<Session>> = std::mem::take(&mut *write_lock(&self.sessions))
            .into_values()
            .collect();
        for s in &sessions {
            if let Err(e) = lock(&s.killer).kill() {
                tracing::debug!(session = s.id.0, error = %e, "kill on shutdown");
            }
            s.closed.notify_one();
        }
        #[cfg(unix)]
        {
            // `child_pid` the method refuses a session that is no longer live,
            // which is every session by the time this runs, so the field is
            // read directly. This is the one caller that wants the pid of a
            // session it has just ended.
            let pids: Vec<u32> = sessions.iter().filter_map(|s| s.child_pid).collect();
            wait_then_kill_survivors(&pids, CHILD_EXIT_GRACE);
        }
        sessions.len()
    }

    pub(crate) fn get(&self, id: SessionId) -> Option<Arc<Session>> {
        read_lock(&self.sessions).get(&id).map(Arc::clone)
    }

    fn require(&self, id: SessionId) -> anyhow::Result<Arc<Session>> {
        self.get(id)
            .ok_or_else(|| anyhow!(SessionError::NoSuchSession { id }))
    }
}

/// How long a child gets to act on the hangup before it is killed outright.
///
/// Long enough for an agent to flush a partial write and exit, short enough
/// that daemon shutdown is not something the operator waits on. It is spent
/// once for every session being closed, not once per session.
#[cfg(unix)]
const CHILD_EXIT_GRACE: std::time::Duration = std::time::Duration::from_millis(750);

/// Wait for `pids` to go away, then `SIGKILL` whatever is left.
///
/// portable-pty's cloned killer sends `SIGHUP` and exposes no way to send
/// anything else, and `SIGHUP` is a signal a process is allowed to handle or
/// ignore. Every agent started under `nohup`, and anything that installs its
/// own shutdown handler, therefore survives the only signal the session ever
/// sent it, holding a terminal that nothing is attached to any more. Closing a
/// session and stopping the daemon both promise the child is gone, so the
/// promise is kept with a signal that cannot be refused.
///
/// Returns as soon as they have all exited, so the grace is an upper bound
/// rather than a delay that is always paid.
///
/// `kill(pid, 0)` is true of a zombie as well as a running process, so a child
/// that has already died but not yet been reaped is signalled a second time.
/// That is a no-op, and it is the safe direction: a zombie holds its pid, so
/// nothing else can have taken it. The window this cannot close is a child
/// reaped inside the grace whose pid is immediately reused, which needs a
/// pidfd to rule out and which no signal-by-pid design avoids.
#[cfg(unix)]
fn wait_then_kill_survivors(pids: &[u32], grace: std::time::Duration) {
    if pids.is_empty() {
        return;
    }
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if !pids.iter().copied().any(present) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    kill_survivors(pids);
}

/// `SIGKILL` every pid still present.
#[cfg(unix)]
fn kill_survivors(pids: &[u32]) {
    for pid in pids.iter().copied().filter(|p| present(*p)) {
        tracing::debug!(pid, "child ignored the hangup; killing it");
        // SAFETY: `kill` is async-signal-safe and takes no pointer. A pid that
        // has gone away answers ESRCH, which is the outcome being asked for.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
}

/// Whether `pid` is still in the process table, running or a zombie.
#[cfg(unix)]
fn present(pid: u32) -> bool {
    // SAFETY: signal 0 performs the permission and existence checks and
    // delivers nothing.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Blocking PTY read loop, feeding raw bytes to the coalescer.
///
/// The child is reaped on its own thread, which publishes the exit code as
/// soon as the process is gone rather than when this loop ends. Those are the
/// same moment on Unix, where the master reports EOF once the child's last
/// descriptor closes. They are not on Windows: the pseudoconsole keeps the
/// read side open for as long as the session holds its master, so this loop
/// only ends when the session is closed. Waiting for it before reporting the
/// exit is what left every Windows session running forever.
///
/// Output still precedes the exit. The coalescer keeps draining this channel
/// after the code arrives, so the ordering is enforced where the bytes are
/// published instead of by which thread stops first. Where this loop can end
/// on its own it drains to real end of stream; where it cannot, the exit plus
/// a quiet window is the end. See `READER_REPORTS_EOF`.
fn read_loop(
    session: Arc<Session>,
    mut reader: Box<dyn Read + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    out: mpsc::UnboundedSender<BytesMut>,
    exit: oneshot::Sender<Option<i32>>,
) {
    if let Err(e) = std::thread::Builder::new()
        .name("vitrum-pty-wait".to_string())
        .spawn(move || {
            let mut child = child;
            let _ = exit.send(reap(&mut *child));
            drop(child);
        })
    {
        // The closure was dropped with the child inside it, so nothing will
        // ever answer and the session cannot report an exit code.
        tracing::warn!(error = %e, "no thread to reap the pty child");
    }
    // Nothing is parsed here. This thread's whole job is to get bytes off the
    // pty and into the coalescer, because it is the only thread that can, and
    // anything else it does is time the child spends blocked on a full pty
    // buffer.
    //
    // A full terminal engine used to live on this thread, fed every byte, and
    // was read for exactly two strings: the window title and the reported
    // working directory. Measured end to end through a real pty that parse was
    // 57% of everything a session spent moving a megabyte, and the screen it
    // maintained was never looked at -- the client renders from the raw bytes
    // with an emulator of its own. Both strings now come out of the single
    // scan the coalescer already makes over the same bytes, which is a state
    // machine with five states instead of a terminal.

    // One zeroed arena, carved up by `split_to` and refilled only when what is
    // left is too small to be worth a read. Each read hands its bytes on
    // without a copy, and the bytes behind them are already initialised, so
    // the loop pays one allocation per megabyte of output rather than one
    // zero-fill per read.
    //
    // Per read was the old shape and it was expensive in exactly the case that
    // matters. A pty hands back whatever the child has written so far, which
    // measured a few hundred bytes under a firehose, so zero-filling a whole
    // 32 KiB buffer before every read wrote about seventy times more memory
    // than the session was producing.
    //
    // The read itself stays capped at `READ_CHUNK`: the point of the arena is
    // contiguity and cheap zeroing, not a megabyte-deep syscall.
    let mut pool = BytesMut::zeroed(READ_ARENA);
    session.pump.arenas.fetch_add(1, Ordering::Relaxed);
    loop {
        if pool.len() < READ_FLOOR {
            pool = BytesMut::zeroed(READ_ARENA);
            session.pump.arenas.fetch_add(1, Ordering::Relaxed);
        }
        // The floor above guarantees the arena still holds a whole chunk.
        match reader.read(&mut pool[..READ_CHUNK]) {
            Ok(0) => break,
            Ok(n) => {
                session.pump.reads.fetch_add(1, Ordering::Relaxed);
                let chunk = pool.split_to(n);
                // Nothing else happens to the bytes on this thread.
                if out.send(chunk).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            // A closed pty reports EIO on Linux and EBADF elsewhere; both mean
            // the same thing here, which is that the child is done writing.
            Err(_) => break,
        }
    }
}

/// Wait for a PTY child and turn its status into a reportable exit code.
fn reap(child: &mut (dyn portable_pty::Child + Send + Sync)) -> Option<i32> {
    match child.wait() {
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
    }
}

/// Blocking PTY write loop. Ends when the session drops its queue.
///
/// A write that fails ends the loop, because the master is gone and every
/// keystroke after it would fail the same way. It is reported rather than
/// swallowed: a silent break here is indistinguishable from a session whose
/// input is simply being ignored, which is a bug that can survive for a long
/// time precisely because nothing says anything.
fn write_loop(mut writer: Box<dyn Write + Send>, mut input: mpsc::UnboundedReceiver<Bytes>) {
    while let Some(data) = input.blocking_recv() {
        if let Err(e) = writer.write_all(&data).and_then(|()| writer.flush()) {
            tracing::warn!(error = %e, bytes = data.len(), "writing to the pty");
            break;
        }
    }
}

/// The coalescer, fed by hand, with no reader thread racing it.
///
/// WHY: every count in this loop that is worth asserting on is a count of what
/// it did with a BACKLOG, and a live session cannot be made to have one. The
/// reader thread and the coalescer run concurrently, so how many reads are
/// queued when `recv_many` is polled is a scheduling outcome: on an idle
/// machine the reader gets far ahead and a burst batches, and on a loaded one
/// it does not. A bound written against that ratio measures the machine, which
/// is the flake this harness exists to remove — one such bound failed on the
/// self-hosted CI runner at 475 wakeups for 756 reads and passed on the same
/// commit locally.
///
/// Filling the channel before anything polls it makes the backlog a fact
/// rather than a hope, so the wakeup and timer counts a burst costs become
/// exact whole numbers.
// Only the non-Windows suite calls this: the Windows path publishes on a
// different schedule and `a_backlog_costs_one_timer_and_a_batch_of_wakeups`
// is `cfg(not(windows))` for it. The cfg is theirs rather than an `allow`, so
// that this goes dead the day that test does.
#[cfg(all(test, not(windows)))]
pub(crate) struct Coalescer {
    session: Arc<Session>,
    raw: mpsc::UnboundedSender<BytesMut>,
    raw_rx: mpsc::UnboundedReceiver<BytesMut>,
    exit_rx: oneshot::Receiver<Option<i32>>,
}

#[cfg(all(test, not(windows)))]
impl Coalescer {
    /// A session with a real PTY and a real child, and nothing reading it.
    ///
    /// The PTY is real because `Session` owns a master and a killer and this
    /// must be the same struct the daemon runs, not a parallel one that could
    /// drift from it. Nothing writes to the child and nothing reads the
    /// master: the bytes under test go into the channel directly.
    pub(crate) fn new() -> anyhow::Result<Self> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening a pty for the coalescer harness")?;
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("60");
        cmd.cwd(std::env::current_dir().context("no working directory")?);
        let child = pair.slave.spawn_command(cmd).context("spawning sleep")?;
        let child_pid = child.process_id();
        let killer = child.clone_killer();
        drop(pair.slave);

        let id = SessionId(1);
        let now = now_ms();
        let (output, _) = broadcast::channel(OUTPUT_CHANNEL_CHUNKS);
        let (status_tx, _) = watch::channel(SessionStatus::Starting);
        let (observations_tx, _) = watch::channel(0u64);
        let (input_tx, _input_rx) = mpsc::unbounded_channel::<Bytes>();
        let info = SessionInfo {
            id,
            project_id: vitrum_proto::ProjectId(1),
            title: "coalescer".into(),
            cwd: String::new(),
            command: "sleep".into(),
            args: vec!["60".into()],
            status: SessionStatus::Starting,
            created_at_ms: now,
            last_activity_ms: now,
            cols: 80,
            rows: 24,
            git_branch: None,
            unread: false,
            attention: Attention::default(),
            hint: None,
            term_title: None,
        };
        let session = Arc::new(Session {
            id,
            info: RwLock::new(info),
            title_pinned: AtomicBool::new(false),
            scrollback: Mutex::new(Scrollback::with_capacity(1 << 24)),
            output,
            status: status_tx,
            observations: observations_tx,
            master: Mutex::new(pair.master),
            viewers: Mutex::new(BTreeMap::new()),
            resizes: AtomicU64::new(0),
            probes: AtomicU64::new(0),
            activity: Notify::new(),
            closed: Notify::new(),
            input: Mutex::new(Some(input_tx)),
            killer: Mutex::new(killer),
            last_focus_ms: AtomicU64::new(0),
            child_pid,
            pump: PumpTally::default(),
        });
        let (raw, raw_rx) = mpsc::unbounded_channel::<BytesMut>();
        let (_exit_tx, exit_rx) = oneshot::channel::<Option<i32>>();
        Ok(Self {
            session,
            raw,
            raw_rx,
            exit_rx,
        })
    }

    /// Queue one read, exactly as the reader thread would hand one over.
    pub(crate) fn queue(&self, bytes: &[u8]) {
        self.raw
            .send(BytesMut::from(bytes))
            .expect("the coalescer has not started yet");
    }

    /// Run the loop to end of stream and report what it spent.
    ///
    /// The sender is dropped first, so everything queued is already there when
    /// the first poll happens and the loop ends on end of stream rather than
    /// on a deadline.
    pub(crate) async fn drain(self) -> PumpCounts {
        let Coalescer {
            session,
            raw,
            raw_rx,
            exit_rx,
        } = self;
        drop(raw);
        coalesce_loop(Arc::clone(&session), raw_rx, exit_rx).await;
        let counts = session.pump.snapshot();
        let _ = lock(&session.killer).kill();
        counts
    }
}

/// Coalesce raw reads into a few large chunks, then publish them.
async fn coalesce_loop(
    session: Arc<Session>,
    mut raw: mpsc::UnboundedReceiver<BytesMut>,
    exit: oneshot::Receiver<Option<i32>>,
) {
    // No staging buffer. Consecutive pty reads are consecutive slices of the
    // reader's pool, so a run that merges several of them is rejoined in
    // place: `try_unsplit` on adjacent halves of the same allocation only
    // moves two indices. Copying them into a separate buffer, which is what
    // this loop used to do, moved every byte of the firehose a second time
    // for nothing.
    //
    // The fallback is the reader crossing into a fresh pool mid-run. That is
    // the only case that copies, and `staged_bytes` counts exactly it.
    // Reused for the lifetime of the session: one allocation, drained rather
    // than rebuilt, so batching reads costs no allocation of its own.
    let mut batch: Vec<BytesMut> = Vec::with_capacity(BATCH_READS);
    let mut scan = OutputScan::new();
    let mut hints: Vec<HintDeclaration> = Vec::new();
    // Armed once at spawn so a child that never writes a byte is still
    // classified, and re-armed only by activity from then on.
    let mut settle_at = Some(Instant::now() + SETTLE_WINDOW);

    let mut exit = exit;
    // Set the moment the child is reaped, which on Windows is long before the
    // byte channel closes.
    let mut code: Option<Option<i32>> = None;

    loop {
        let Some(first) = next_read(&session, &mut raw, &mut settle_at, &mut exit, &mut code).await
        else {
            break;
        };
        session.pump.wakeups.fetch_add(1, Ordering::Relaxed);
        // The run is the reader's own allocation, extended in place while the
        // window is open and handed to subscribers at the end of it. Nothing
        // on this path copies the bytes.
        let mut run = first;
        let mut pending = run.len();
        // Two deadlines, and the run ends at whichever comes first.
        //
        // `cap` bounds how long a byte may wait, which is what a fixed window
        // was for. `quiet` ends the run as soon as the child stops writing,
        // which is what a fixed window got wrong: an echoed keystroke is one
        // read followed by silence, and waiting the whole window for a second
        // read that is never coming charged interactive typing the price of
        // batching a firehose.
        //
        // Nothing polls. Both deadlines are timers, and a timer only exists
        // while a run is open, so a session with no output arms nothing and an
        // idle daemon does no work at all.
        let cap = Instant::now() + FLUSH_WINDOW;
        let mut ended = false;
        // One timer for the whole window, and every read the channel already
        // holds taken in one wakeup.
        //
        // Awaiting each read under its own `timeout_at` armed a timer and
        // scheduled the task once per read, and a pty hands back a few hundred
        // bytes at a time under load: measured, that was about 2500 timer
        // registrations and 2500 wakeups for every megabyte, to publish
        // sixteen runs.
        let window = tokio::time::sleep_until((Instant::now() + FLUSH_IDLE).min(cap));
        session.pump.timers.fetch_add(1, Ordering::Relaxed);
        tokio::pin!(window);
        let mut hit_cap = false;
        while pending < FLUSH_BYTES {
            batch.clear();
            let taken = tokio::select! {
                // Biased so pending output always beats the deadline: bytes
                // that are already here belong in this run rather than the next.
                biased;
                taken = raw.recv_many(&mut batch, BATCH_READS) => taken,
                () = &mut window => break,
            };
            if taken == 0 {
                ended = true;
                break;
            }
            session.pump.wakeups.fetch_add(1, Ordering::Relaxed);
            for more in batch.drain(..) {
                let len = more.len();
                if let Err(more) = run.try_unsplit(more) {
                    session
                        .pump
                        .staged_bytes
                        .fetch_add(len as u64, Ordering::Relaxed);
                    run.extend_from_slice(&more);
                }
            }
            pending = run.len();
            // Output is still arriving, so hold the run open for another idle
            // gap, but never past the cap.
            let now = Instant::now();
            if now >= cap {
                hit_cap = true;
                break;
            }
            window.as_mut().reset((now + FLUSH_IDLE).min(cap));
        }
        if hit_cap || Instant::now() >= cap {
            session.pump.capped_flushes.fetch_add(1, Ordering::Relaxed);
        } else if pending < FLUSH_BYTES && !ended {
            session.pump.idle_flushes.fetch_add(1, Ordering::Relaxed);
        }
        let run = run.freeze();
        hints.clear();
        session
            .pump
            .parsed_bytes
            .fetch_add(run.len() as u64, Ordering::Relaxed);
        let wants_operator = scan.scan(&run, &mut hints);
        // Only the last declaration in a run matters: an agent that says
        // `working` and then `ready` in the same burst has finished, and
        // publishing the intermediate state would flash a stale badge.
        session.publish(run, wants_operator, hints.last());
        // A client learns a session changed when its observation revision
        // moves, so the revision has to move here. The foreground probe would
        // eventually push a fresh projection on its own, but it reports
        // children that have settled and deliberately leaves a streaming
        // session alone, which is exactly the session most likely to be
        // retitling itself while it works. Saying so when the name changes
        // costs one watch send on a path that already decided something is
        // different.
        //
        // After the bytes, never before. What the program announced about
        // itself is worth strictly less than the output it announced it in,
        // and a client waiting on a chunk must not wait on a title lookup.
        let mut changed = false;
        if let Some(title) = scan.take_title() {
            changed |= apply_engine_title(&session, &title);
        }
        if let Some(pwd) = scan.take_pwd() {
            changed |= apply_engine_pwd(&session, &pwd);
        }
        if changed {
            session.bump();
        }
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
    // Everything the reader produced is published. Either the exit was already
    // seen and drained past, or the channel closed first and it is due now.
    let code = match code {
        Some(code) => code,
        None => exit.await.unwrap_or(None),
    };
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
    raw: &mut mpsc::UnboundedReceiver<BytesMut>,
    settle_at: &mut Option<Instant>,
    exit: &mut oneshot::Receiver<Option<i32>>,
    code: &mut Option<Option<i32>>,
) -> Option<BytesMut> {
    loop {
        // Once the child is gone there is nothing left to classify, only the
        // bytes it already wrote.
        if code.is_some() {
            if READER_REPORTS_EOF {
                // Park on the channel until the reader closes it. A child
                // exiting says nothing about how much it wrote on the way out:
                // a whole burst can still be sitting in the pty buffer, or in
                // the reader's hands, when the exit is reaped. Ending the
                // stream on a stopwatch instead of on end of stream threw all
                // of it away, which is how a shell printing a few kilobytes
                // and exiting published nothing at all.
                //
                // Biased so a pending chunk always beats the close: a session
                // being discarded still publishes what it already read.
                tokio::select! {
                    biased;
                    chunk = raw.recv() => return chunk,
                    () = session.closed.notified() => return None,
                }
            }
            // A Windows pseudoconsole never closes the channel while the
            // session holds its master, so quiet after the exit is the only
            // end of output there is.
            return timeout_at(Instant::now() + FLUSH_WINDOW, raw.recv())
                .await
                .unwrap_or_default();
        }
        match *settle_at {
            None => {
                tokio::select! {
                    chunk = raw.recv() => return chunk,
                    reaped = &mut *exit => *code = Some(reaped.unwrap_or(None)),
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
                    reaped = &mut *exit => *code = Some(reaped.unwrap_or(None)),
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

/// Record the title the program announced, and take it as the name if the
/// program is the kind that names itself.
///
/// Returns whether anything changed, so the caller only wakes watchers when
/// there is something to see. A shell re-announces the same title on every
/// prompt, so "changed" has to mean the string differs, not that a sequence
/// arrived.
///
/// The announced title is always recorded, because the status resolver reads
/// it: an agent that puts `[ ! ] Action Required` in its title bar is how a
/// blocked Codex session is detected at all. Whether it also becomes the
/// session's *name* is [`AgentKind::title_is_a_name`]'s call, and for every
/// agent TUI the answer is no — their title bar is a status line, and a row
/// named from it reads `Ready (kernel-n…` next to a pill already saying Ready.
///
/// An empty title is refused for the name. `OSC 2 ST` with nothing in it is how
/// some programs clear the title on exit, and honouring it would blank a row in
/// the sidebar rather than leave the name the session already had. It is still
/// recorded, because a cleared title is a real retraction of whatever the
/// program was claiming.
fn apply_engine_title(session: &Session, title: &str) -> bool {
    let trimmed = title.trim();
    let mut info = write_lock(&session.info);

    let announced = (!trimmed.is_empty()).then(|| trimmed.to_string());
    let mut changed = false;
    if info.term_title != announced {
        info.term_title = announced;
        changed = true;
    }

    // The pin is on the NAME, not on the channel. An operator who renames a
    // session is watching that one in particular, and silencing its approval
    // banner would take the state they renamed it to follow.
    if session.title_pinned.load(Ordering::Relaxed) {
        return changed;
    }
    if trimmed.is_empty() || !AgentKind::of(&info.command).title_is_a_name() {
        return changed;
    }
    if info.title == trimmed {
        return changed;
    }
    info.title = trimmed.to_string();
    true
}

/// Take the directory the shell reported, if it is one this machine has.
///
/// Returns whether anything changed. A shell re-announces the same directory on
/// every prompt, so the comparison is what keeps this from re-walking the
/// filesystem for a git branch several times a second.
///
/// The directory has to exist here. The hostname in an OSC 7 report is whatever
/// the sending program chose to write, so it proves nothing on its own, and a
/// session sitting inside `ssh` reports paths that belong to another machine
/// entirely. Asking whether the directory is actually here is the check that
/// the two things depending on it, the branch lookup and what the operator is
/// told about where the session is, actually need.
///
/// Unlike the title, this is not something the operator can own. Where a
/// session is is a fact about the session rather than a name someone chose, and
/// a shell that has moved has moved.
fn apply_engine_pwd(session: &Session, raw: &str) -> bool {
    let Some(dir) = vitrum_vt::pwd_path(raw) else {
        return false;
    };
    let mut info = write_lock(&session.info);
    if info.cwd == dir.to_string_lossy() {
        return false;
    }
    if !dir.is_dir() {
        return false;
    }
    info.cwd = dir.to_string_lossy().into_owned();
    // The branch is resolved from the directory, so a session that moved into
    // another repository is in another repository. This walk is why the
    // comparison above happens first.
    info.git_branch = git_branch(&dir);
    true
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
fn spawn_failure(command: &str, e: &anyhow::Error) -> SessionError {
    let raw = e.to_string();
    if raw.contains("No viable candidates found in PATH") {
        return SessionError::NotOnPath {
            command: command.to_string(),
        };
    }
    SessionError::CannotStart {
        command: command.to_string(),
        detail: raw,
    }
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
        let e = spawn_failure("claud", &path_error("claud"));
        assert_eq!(
            e,
            SessionError::NotOnPath {
                command: "claud".into()
            }
        );
        let m = e.to_string();
        assert!(m.starts_with("no command named claud"), "{m}");
        assert!(!m.contains("/usr/bin"), "the PATH came back: {m}");
        assert!(
            m.chars().count() < vitrum_proto::MAX_ERROR_CHARS,
            "still {} characters",
            m.chars().count()
        );
    }

    /// Every other spawn failure keeps its reason.
    ///
    /// The narrow rewrite must not become "replace all spawn errors with a
    /// friendly sentence": a permission error and a directory-as-command are
    /// different problems with different fixes, and the OS text is the fix.
    #[test]
    fn an_unusual_failure_keeps_the_reason_it_came_with() {
        let m = spawn_failure("bash", &anyhow::anyhow!("Permission denied (os error 13)"))
            .to_string();
        assert_eq!(m, "could not start bash: Permission denied (os error 13)");

        let d = spawn_failure(
            "/tmp",
            &anyhow::anyhow!("Unable to spawn /tmp because it is a directory"),
        )
        .to_string();
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
        let m = spawn_failure(hostile, &path_error(hostile)).to_string();
        assert!(!m.contains('\n'), "newline survived: {m:?}");
        assert!(!m.contains('\u{202e}'), "override survived: {m:?}");
        assert!(
            m.starts_with("no command named bashStatus: ok on the daemon's PATH."),
            "{m}"
        );

        // The same rule on the free-text half, which carries the platform's
        // words rather than the operator's.
        let forged = spawn_failure("sh", &anyhow::anyhow!("denied\nStatus: ok")).to_string();
        assert_eq!(forged, "could not start sh: deniedStatus: ok");
    }

    /// A command in another language is reported as written.
    #[test]
    fn a_non_ascii_command_is_named_intact() {
        let m = spawn_failure("機能", &path_error("機能")).to_string();
        assert!(m.starts_with("no command named 機能 on the daemon's PATH."), "{m}");
    }
}
