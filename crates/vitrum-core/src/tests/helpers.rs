//! Shared scaffolding: portable child commands, bounded waits, byte collection.
//!
//! Nothing here sleeps for a fixed duration as a synchronisation mechanism. Every
//! wait is either a channel or a bounded poll with a deadline, so a passing test
//! finishes as fast as the PTY does and a broken one fails with a message
//! instead of hanging a suite.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use vitrum_proto::{ProjectId, SessionId, SessionStatus};
use tokio::sync::{broadcast, watch};
use tokio::time::{Instant, timeout_at};

use crate::{OutputChunk, SessionManager, SessionSpec};

/// Upper bound on any single wait. Only reached when something is broken, so it
/// is generous: a passing test never waits anywhere near this long.
pub(crate) const DEADLINE: Duration = Duration::from_secs(10);

/// Window used by negative assertions ("nothing should arrive"). Deliberately
/// short because it is a bound on absence, not a wait for a result.
pub(crate) const QUIET: Duration = Duration::from_millis(150);

/// Run `script` through the platform shell.
///
/// The two shells agree on `echo` and `exit`, which is all the portable tests
/// need; anything shell-specific is behind a `cfg` in the test that uses it.
pub(crate) fn shell(script: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), script.to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        ("sh".to_string(), vec!["-c".to_string(), script.to_string()])
    }
}

/// A spec running `script` in the platform shell from a directory that exists.
pub(crate) fn shell_spec(script: &str) -> SessionSpec {
    let (command, args) = shell(script);
    SessionSpec {
        project_id: ProjectId(7),
        cwd: std::env::temp_dir(),
        command,
        args,
        env: Vec::new(),
        cols: 80,
        rows: 24,
        title: None,
    }
}

/// Attach a fresh viewer without disturbing the session's geometry.
///
/// The size is read back from the session so an attach in a test that is not
/// about geometry cannot silently resize the child. Tests that ARE about
/// geometry call `SessionManager::attach` directly with the size they mean.
pub(crate) fn attach(mgr: &SessionManager, id: SessionId) -> broadcast::Receiver<OutputChunk> {
    let info = mgr.info(id).expect("attaching needs a live session");
    mgr.attach(id, mgr.new_viewer(), info.cols, info.rows)
        .expect("attach")
}

/// A collector over a freshly attached viewer.
pub(crate) fn collect(mgr: &SessionManager, id: SessionId) -> Collector {
    Collector::new(attach(mgr, id))
}

/// Force one more foreground probe and wait for it to finish.
///
/// An empty write puts not a single byte into the PTY, so the child cannot see
/// it, but it does count as operator activity, which is exactly the re-arm the
/// probe needs. That is how a test gets a fresh answer about a child that has
/// already settled, without sleeping for one and without disturbing it.
pub(crate) async fn probe_now(mgr: &SessionManager, id: SessionId) -> vitrum_proto::SessionInfo {
    let before = mgr
        .probe_count(id)
        .expect("session must exist to be probed");
    mgr.write(id, b"")
        .expect("an empty write is still activity");
    let deadline = Instant::now() + DEADLINE;
    while mgr.probe_count(id).unwrap_or(u64::MAX) <= before {
        assert!(
            Instant::now() < deadline,
            "the foreground probe never ran for session {}",
            id.0
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    mgr.info(id).expect("the session must still exist")
}

/// Probe until the answer is `want`, or fail with what it actually said.
///
/// Retries because a child caught in the microseconds between `fork` and its
/// steady state can legitimately answer something else once. It never masks a
/// wrong answer: a classification that is simply incorrect never converges and
/// the assertion fires with the value it kept giving.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) async fn waiting_settles_on(
    mgr: &SessionManager,
    id: SessionId,
    want: Option<bool>,
    what: &str,
) {
    let deadline = Instant::now() + DEADLINE;
    let mut last = None;
    while Instant::now() < deadline {
        last = probe_now(mgr, id).await.attention.waiting;
        if last == want {
            return;
        }
    }
    panic!("{what}: expected waiting {want:?}, the probe kept answering {last:?}");
}

/// Wait for `id` to reach a terminal state and yield its exit code.
pub(crate) async fn wait_exit(mgr: &SessionManager, id: SessionId) -> Option<i32> {
    let rx = mgr
        .subscribe_status(id)
        .expect("session must exist to be awaited");
    wait_exit_on(rx).await
}

/// Wait for a terminal state on an already-held status receiver.
///
/// Needed for `close`, which unregisters the session, so its receiver has to be
/// taken before the close call.
pub(crate) async fn wait_exit_on(mut rx: watch::Receiver<SessionStatus>) -> Option<i32> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let current = rx.borrow_and_update().clone();
        if let SessionStatus::Exited { code } = current {
            return code;
        }
        timeout_at(deadline, rx.changed())
            .await
            .expect("session did not reach a terminal state before the deadline")
            .expect("status channel closed while the session was still live");
    }
}

/// Accumulates broadcast output while checking the stream's own invariants.
pub(crate) struct Collector {
    rx: broadcast::Receiver<OutputChunk>,
    /// Seq the next chunk must carry. `None` until the first chunk is seen.
    next_seq: Option<u64>,
    /// Seq of the very first chunk observed.
    pub(crate) first_seq: Option<u64>,
    pub(crate) bytes: Vec<u8>,
    pub(crate) chunks: usize,
}

impl Collector {
    pub(crate) fn new(rx: broadcast::Receiver<OutputChunk>) -> Self {
        Self {
            rx,
            next_seq: None,
            first_seq: None,
            bytes: Vec::new(),
            chunks: 0,
        }
    }

    /// Collect until `stop` accepts what has been gathered.
    ///
    /// Panics on a sequence discontinuity or on lag, because a test that
    /// silently tolerates lost output would pass while the product loses bytes.
    pub(crate) async fn until(&mut self, mut stop: impl FnMut(&[u8]) -> bool) -> &[u8] {
        let deadline = Instant::now() + DEADLINE;
        while !stop(&self.bytes) {
            let chunk = timeout_at(deadline, self.rx.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "deadline passed with {} bytes collected: {:?}",
                        self.bytes.len(),
                        String::from_utf8_lossy(&self.bytes)
                    )
                });
            match chunk {
                Ok(c) => {
                    if let Some(expected) = self.next_seq {
                        assert_eq!(
                            c.seq, expected,
                            "output seq must be the cumulative byte offset with no gaps"
                        );
                    }
                    self.first_seq = self.first_seq.or(Some(c.seq));
                    self.next_seq = Some(c.seq + c.data.len() as u64);
                    self.bytes.extend_from_slice(&c.data);
                    self.chunks += 1;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    panic!("test client lagged and lost {n} chunks")
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        &self.bytes
    }

    /// Assert that nothing more arrives within the quiet window.
    pub(crate) async fn expect_quiet(&mut self) {
        let r = tokio::time::timeout(QUIET, self.rx.recv()).await;
        assert!(
            r.is_err(),
            "expected no further output, got {:?}",
            r.map(|c| c.map(|c| c.seq))
        );
    }
}

/// A uniquely named temporary directory that removes itself.
pub(crate) struct TempDir {
    pub(crate) path: PathBuf,
}

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("vitrum-test-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("creating a temp dir");
        Self { path }
    }

    pub(crate) fn join(&self, rel: &str) -> PathBuf {
        self.path.join(rel)
    }

    /// Write `contents` to `rel`, creating parent directories.
    pub(crate) fn write(&self, rel: &str, contents: &str) {
        let p = self.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("creating parent dirs");
        }
        std::fs::write(p, contents).expect("writing a temp file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Deterministic byte pattern, so an assertion can name the exact expected
/// bytes for any offset without embedding a literal.
pub(crate) fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// Read `id`'s whole retained stream once `done` accepts it and it has stopped
/// growing. Returns the oldest retained offset and the bytes.
///
/// A terminal state is not the end of the stream. The process is reaped by one
/// thread and the pty is drained by another, so `wait_exit` can return while the
/// last coalesced chunk is still on its way into the ring. A test that reads at
/// that moment compares a full stream against a truncated one and blames the
/// ring.
///
/// Quiet alone is not enough to say the stream is finished, because ConPTY writes
/// its preamble and then pauses long enough to look finished before the child has
/// produced a byte. So the caller states what it is waiting for, and quiet is
/// only the confirmation that nothing further arrived.
pub(crate) async fn settled(
    mgr: &SessionManager,
    id: SessionId,
    done: impl Fn(u64, &[u8]) -> bool,
) -> (u64, Vec<u8>) {
    let deadline = Instant::now() + DEADLINE;
    let mut previous = None;
    loop {
        let (from, bytes, _) = mgr
            .scrollback(id, u64::MAX, 1 << 20)
            .expect("session exists");
        let head = from + bytes.len() as u64;
        if previous == Some(head) && done(from, &bytes) {
            return (from, bytes);
        }
        assert!(
            Instant::now() < deadline,
            "session {} never settled (head {head}): {:?}",
            id.0,
            String::from_utf8_lossy(&bytes)
        );
        previous = Some(head);
        tokio::time::sleep(QUIET).await;
    }
}

/// True when `haystack` contains `needle`.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The whole byte stream one run of `script` produces, from a ring big enough to
/// keep all of it. `marker` is the last thing the child writes.
///
/// A test cannot compute this from the command. A Unix pty hands over the
/// child's bytes and nothing else, but ConPTY opens every session with its own
/// preamble: mode sets, an SGR reset, an OSC 0 naming the shell, and a cursor
/// show. Those are terminal bytes and the frontend needs them, so they belong in
/// the stream and they count towards the ring like any other byte.
///
/// So the reference is measured rather than assumed: the same command in a
/// manager that evicts nothing. What the capacity tests then assert is that a
/// small ring holds exactly the tail of what a large one holds, which is the
/// real contract and is the same sentence on every platform.
pub(crate) async fn whole_stream(script: &str, marker: &[u8]) -> Vec<u8> {
    let mgr = SessionManager::new(1 << 20);
    let id = mgr.spawn(shell_spec(script)).expect("spawn");
    assert_eq!(wait_exit(&mgr, id).await, Some(0));
    let (from, bytes) = settled(&mgr, id, |_, b| contains(b, marker)).await;
    assert_eq!(from, 0, "a ring this size evicted something");
    bytes
}

/// A script that blocks waiting for the operator to type something.
///
/// `read -r answer` is a POSIX shell builtin, and `cmd` answers it with "not
/// recognized" and exits. A test that wants a session sitting on the keyboard
/// therefore has to ask for it in the local dialect, or it measures a session
/// that already died.
pub(crate) fn blocking_read() -> &'static str {
    if cfg!(windows) { "pause" } else { "read -r answer" }
}
