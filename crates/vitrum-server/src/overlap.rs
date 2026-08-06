//! Same-file collision detection: which files two live sessions are both
//! changing.
//!
//! # What this answers, and what it refuses to
//!
//! Ten agents in a large checkout usually do not conflict. A warning that
//! fires because two sessions share a repository fires constantly and is
//! ignored within a day, so this fires on one condition only: two or more
//! LIVE sessions have both written the same file. That is the failure that
//! actually costs work, because the second write silently discards the first
//! agent's reasoning.
//!
//! It never guesses silently. A change it cannot pin on a session is counted
//! as unattributed and appears in no participant list, and the count is
//! published so a client can qualify an empty result instead of rendering a
//! confident "nothing is colliding". A client that shows "no collisions" when
//! nobody looked, or when half the writes went unattributed, is telling the
//! operator their agents are safe on the strength of an answer nobody has.
//!
//! # Cost, which is the reason it is a subscription
//!
//! The daemon's headline is that it performs no syscalls at all while nothing
//! is happening. Detection means one inotify watch per directory under every
//! session root, so it is off until a client asks. Unsubscribed, this holds no
//! thread, no inotify fd and no watch descriptors: [`OverlapService::inner`] is
//! `None` and the whole feature costs one atomic read on the paths that call
//! [`OverlapService::sync`].
//!
//! # Attribution
//!
//! inotify reports THAT a file changed, never WHO changed it. Linux has no
//! "which process wrote this" event, so the credit is reconstructed:
//!
//! 1. On a close-after-write, walk each live session's process tree and look
//!    for an open descriptor on that path. Exactly one match is
//!    [`Credit::Observed`], the strongest evidence available.
//! 2. If nobody holds it open any more, and exactly one session has written
//!    this file recently, credit that session as [`Credit::Inferred`].
//! 3. Otherwise it is unattributed, and counted as such.
//!
//! Step 1 runs on CLOSE_WRITE rather than on every MODIFY, so a program
//! writing a file in a thousand chunks costs one process-tree walk, not a
//! thousand.
//!
//! Editors that write a temporary file and rename it over the target are the
//! normal case, not an edge case, so MOVED_TO is treated as a write to the
//! destination.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use vitrum_proto::{
    Collision, CollisionParticipant, CollisionSession, Credit, ServerMsg, SessionId,
};

use vitrum_core::SessionManager;

/// Publishes a report to every connected window.
pub type Publish = Arc<dyn Fn(ServerMsg) + Send + Sync>;

/// Files retained per session before the oldest are dropped.
///
/// A bound, not a total: an agent that rewrites ten thousand files must not
/// grow this map without limit. Dropping the oldest is right because a
/// collision is about recent concurrent work, and a file nobody has touched
/// in a thousand writes is not being fought over now.
//
// Gated with the write tracking below: only the inotify reader records writes,
// so on a platform with no watcher this bound bounds nothing and the compiler
// is right to say so. `test` is in the cfg because the eviction rule is tested
// everywhere, and a bound that is only exercised on one platform is a bound
// that breaks on the others.
#[cfg(any(target_os = "linux", test))]
const PATHS_PER_SESSION: usize = 512;

/// How long a write stays eligible to be a collision, in milliseconds.
///
/// Two agents editing one file an hour apart is a handover, not a collision.
/// Ten minutes is long enough to span a slow agent turn and short enough that
/// yesterday's work never lights a row up today.
const WINDOW_MS: u64 = 10 * 60 * 1000;

/// One session's write to one path.
#[derive(Debug, Clone, Copy)]
struct Write {
    first_ms: u64,
    last_ms: u64,
    writes: u32,
    credit: Credit,
}

/// A session being watched, and what has been seen under it.
struct Watched {
    id: SessionId,
    root: PathBuf,
    /// The PTY child. Its descendants are what actually write.
    #[cfg(any(target_os = "linux", test))]
    pid: u32,
    /// Path to this session's writes on it. Bounded by [`PATHS_PER_SESSION`].
    writes: HashMap<PathBuf, Write>,
    /// Changes under this root that could not be pinned on anybody.
    unattributed: u64,
}

#[cfg(any(target_os = "linux", test))]
impl Watched {
    /// A session that was not being watched a moment ago.
    fn new(id: SessionId, root: PathBuf, pid: u32) -> Self {
        Self {
            id,
            root,
            pid,
            writes: HashMap::new(),
            unattributed: 0,
        }
    }

    /// The pid changes when the session respawns its child.
    fn set_pid(&mut self, pid: u32) {
        self.pid = pid;
    }
}

/// The live watcher: the sessions, and on Linux an inotify fd and its watch
/// descriptors. The two inotify fields are `cfg`-gated because a watch
/// descriptor is an inotify concept: carrying them everywhere obliged every
/// platform that has no watcher to invent values for them, and the Windows
/// build failed to compile rather than doing so.
struct Watcher {
    /// Set false to make the reader thread exit at its next wake.
    running: Arc<AtomicBool>,
    state: Arc<Mutex<Tracked>>,
    /// A second handle on the same inotify instance, so `sync` can add
    /// watches for a session that started after the subscription did.
    ///
    /// Without this the watch set is whatever existed at subscribe time, and
    /// a client that subscribes on connect subscribes before any session
    /// exists: the set would be empty for the life of the daemon and nothing
    /// would ever be detected. The reader thread owns the other handle.
    #[cfg(target_os = "linux")]
    adder: Option<Arc<std::fs::File>>,
    /// Watch descriptor to the directory it watches, shared with the reader.
    #[cfg(target_os = "linux")]
    wds: Arc<Mutex<std::collections::HashMap<i32, PathBuf>>>,
}

/// Distinct degradations retained.
///
/// Deduplication alone does not bound this: the "tree too large" note names its
/// root, so a daemon that outlives a thousand large checkouts would accumulate a
/// thousand distinct sentences, and the whole list is cloned into every report
/// sent to every window.
const MAX_DEGRADED: usize = 16;

/// Everything the reader thread and the query path share.
#[derive(Default)]
struct Tracked {
    sessions: Vec<Watched>,
    /// Ways detection is currently incomplete, each a finished sentence.
    /// Bounded by [`MAX_DEGRADED`] and deduplicated; see [`Tracked::degrade`].
    degraded: Vec<String>,
}

impl Tracked {
    /// Record a write to `path`, credited to `id`.
    #[cfg(any(target_os = "linux", test))]
    fn record(&mut self, id: SessionId, path: &Path, now_ms: u64, credit: Credit) {
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) else {
            return;
        };
        match s.writes.get_mut(path) {
            Some(w) => {
                w.last_ms = now_ms;
                w.writes = w.writes.saturating_add(1);
                // An observation outranks an earlier inference: once we have
                // actually seen the descriptor, stop hedging about this pair.
                if credit == Credit::Observed {
                    w.credit = Credit::Observed;
                }
            }
            None => {
                if s.writes.len() >= PATHS_PER_SESSION
                    && let Some(oldest) =
                        s.writes.iter().min_by_key(|(_, w)| w.last_ms).map(|(p, _)| p.clone())
                {
                    s.writes.remove(&oldest);
                }
                s.writes.insert(
                    path.to_path_buf(),
                    Write {
                        first_ms: now_ms,
                        last_ms: now_ms,
                        writes: 1,
                        credit,
                    },
                );
            }
        }
    }

    /// Count a change nobody could be credited with.
    #[cfg(any(target_os = "linux", test))]
    fn unattributed(&mut self, root: &Path) {
        if let Some(s) = self.sessions.iter_mut().find(|s| s.root == root) {
            s.unattributed = s.unattributed.saturating_add(1);
        }
    }

    /// Record a way detection is incomplete, once.
    fn degrade(&mut self, note: String) {
        if self.degraded.len() >= MAX_DEGRADED || self.degraded.contains(&note) {
            return;
        }
        self.degraded.push(note);
    }

    /// Every path two or more live sessions have written inside the window.
    fn collisions(&self, now_ms: u64) -> Vec<Collision> {
        // A collision needs two sessions, so one session is the whole answer.
        if self.sessions.len() < 2 {
            return Vec::new();
        }
        // Count first, build second. Building a participant vector for every
        // tracked path allocated one `Vec` per path per call, up to sessions
        // times PATHS_PER_SESSION of them on every inotify batch, and threw all
        // but the contested ones away. Now only a contested path allocates.
        let mut writers: HashMap<&Path, u32> = HashMap::new();
        for s in &self.sessions {
            for (path, w) in &s.writes {
                if now_ms.saturating_sub(w.last_ms) > WINDOW_MS {
                    continue;
                }
                *writers.entry(path.as_path()).or_insert(0) += 1;
            }
        }
        let mut by_path: BTreeMap<&Path, Vec<CollisionParticipant>> = BTreeMap::new();
        for s in &self.sessions {
            for (path, w) in &s.writes {
                if now_ms.saturating_sub(w.last_ms) > WINDOW_MS {
                    continue;
                }
                // TWO OR MORE. One session writing its own file all day is the
                // normal case and is not news.
                if writers.get(path.as_path()).copied().unwrap_or(0) < 2 {
                    continue;
                }
                by_path.entry(path).or_default().push(CollisionParticipant {
                    session: s.id,
                    first_ms: w.first_ms,
                    last_ms: w.last_ms,
                    writes: w.writes,
                    credit: w.credit,
                });
            }
        }
        by_path
            .into_iter()
            .map(|(path, mut who)| {
                who.sort_by_key(|p| p.session.0);
                Collision {
                    path: path.to_string_lossy().into_owned(),
                    participants: who,
                }
            })
            .collect()
    }

    fn per_session(&self) -> Vec<CollisionSession> {
        self.sessions
            .iter()
            .map(|s| CollisionSession {
                session: s.id,
                root: s.root.to_string_lossy().into_owned(),
                tracked_paths: s.writes.len() as u32,
                unattributed: s.unattributed,
            })
            .collect()
    }
}

/// Daemon-wide collision detection, off until a client subscribes.
#[derive(Default)]
pub struct OverlapService {
    inner: RwLock<Option<Watcher>>,
}

impl OverlapService {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Is anything being watched right now?
    ///
    /// One read lock and a discriminant check, so every caller can ask before
    /// doing work. This is what makes [`OverlapService::sync`] free while
    /// nobody has subscribed.
    #[must_use]
    pub fn is_watching(&self) -> bool {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// The current report, watching or not.
    #[must_use]
    pub fn report(&self, now_ms: u64) -> ServerMsg {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let Some(w) = guard.as_ref() else {
            return ServerMsg::CollisionReport {
                watching: false,
                collisions: Vec::new(),
                sessions: Vec::new(),
                degraded: Vec::new(),
            };
        };
        let t = w.state.lock().unwrap_or_else(|e| e.into_inner());
        ServerMsg::CollisionReport {
            watching: true,
            collisions: t.collisions(now_ms),
            sessions: t.per_session(),
            degraded: t.degraded.clone(),
        }
    }

    /// Turn detection on or off, and answer with the resulting report.
    pub fn set_watching(
        self: &Arc<Self>,
        enabled: bool,
        live: &[(SessionId, PathBuf, u32)],
        publish: &Publish,
        now_ms: u64,
    ) -> ServerMsg {
        {
            let mut guard = self.inner.write().unwrap_or_else(|e| e.into_inner());
            match (enabled, guard.as_ref()) {
                // Already in the requested state.
                (true, Some(_)) | (false, None) => {}
                (false, Some(w)) => {
                    // Drop everything: the thread exits, the inotify fd
                    // closes with it, and every watch descriptor goes with the
                    // fd. Unsubscribed must cost exactly nothing.
                    w.running.store(false, Ordering::Relaxed);
                    *guard = None;
                }
                (true, None) => {
                    *guard = Some(platform::start(live, Arc::clone(self), publish.clone()));
                }
            }
        }
        if enabled {
            self.sync(live);
        }
        self.report(now_ms)
    }

    /// How many directories are being watched right now.
    ///
    /// Exists for one test, and that test earns it: the watch set is the link
    /// between "subscribed" and "detects anything", and losing it is invisible
    /// to the compiler and to every pure-function test in this file. It
    /// shipped broken exactly once, because a client subscribes on connect and
    /// therefore subscribes before any session exists.
    #[cfg(all(test, target_os = "linux"))]
    #[must_use]
    pub fn watch_count(&self) -> usize {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|w| w.wds.lock().unwrap_or_else(|p| p.into_inner()).len())
            .unwrap_or(0)
    }

    /// Reconcile the watch set against the sessions that are live now.
    ///
    /// A no-op while nothing is watching, so every caller can invoke it
    /// unconditionally.
    pub fn sync(&self, live: &[(SessionId, PathBuf, u32)]) {
        let guard = self.inner.read().unwrap_or_else(|e| e.into_inner());
        let Some(w) = guard.as_ref() else {
            return;
        };
        platform::sync(w, live);
    }
}

/// Every live session as (id, canonical cwd, pty child pid).
///
/// A session with no child pid is skipped rather than guessed at: without a
/// pid there is no process tree to look in, so nothing it wrote could ever be
/// attributed and watching its tree would only inflate the unattributed count.
#[must_use]
pub fn live_sessions(manager: &SessionManager) -> Vec<(SessionId, PathBuf, u32)> {
    manager
        .list()
        .into_iter()
        .filter(|s| s.status.is_live())
        .filter_map(|s| {
            let pid = manager.child_pid(s.id)?;
            let root = std::fs::canonicalize(&s.cwd).unwrap_or_else(|_| PathBuf::from(&s.cwd));
            Some((s.id, root, pid))
        })
        .collect()
}

/// Put `live` into `t`, keeping what is already known about surviving
/// sessions and dropping what is known about ended ones.
///
/// This is bookkeeping over `Tracked` and touches no kernel interface, so it
/// lives beside the state it edits rather than inside the Linux watcher. Its
/// tests then run everywhere instead of describing one platform.
/// Gated with the `pid` field it maintains: only the Linux watcher drives a
/// reconcile in production, but the rule itself is portable and is proven on
/// every platform's test build rather than on one.
#[cfg(any(target_os = "linux", test))]
fn reconcile(t: &mut Tracked, live: &[(SessionId, PathBuf, u32)]) {
    t.sessions.retain(|s| live.iter().any(|(id, _, _)| *id == s.id));
    for (id, root, pid) in live {
        match t.sessions.iter_mut().find(|s| s.id == *id) {
            Some(s) => s.set_pid(*pid),
            None => t.sessions.push(Watched::new(*id, root.clone(), *pid)),
        }
    }
}

#[cfg(target_os = "linux")]
mod platform;

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;

    /// No watcher on this platform.
    ///
    /// Reported as a degradation rather than silently returning an empty
    /// report, because "nothing collides" and "this build cannot tell" are
    /// different answers and the surface draws them differently.
    pub(super) fn start(
        _live: &[(SessionId, PathBuf, u32)],
        _service: Arc<OverlapService>,
        _publish: Publish,
    ) -> Watcher {
        let mut tracked = Tracked::default();
        tracked.degrade(
            "This build has no file watcher for this platform, so no change is seen."
                .to_string(),
        );
        let state = Arc::new(Mutex::new(tracked));
        Watcher {
            running: Arc::new(AtomicBool::new(false)),
            state,
        }
    }

    pub(super) fn sync(_w: &Watcher, _live: &[(SessionId, PathBuf, u32)]) {}
}

#[cfg(test)]
mod tests;
