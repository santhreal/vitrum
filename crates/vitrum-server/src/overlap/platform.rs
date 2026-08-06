//! The Linux watcher: inotify for the events, `/proc` for the credit.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use vitrum_proto::{Credit, SessionId};

use super::{Publish, Tracked, Watched, Watcher};

/// Directories watched per session before the walk gives up.
///
/// A checkout with a hundred thousand directories would otherwise spend a
/// minute of `inotify_add_watch` and exhaust the per-user watch limit, which
/// is a system-wide resource this daemon does not own. Hitting the cap is
/// reported as a degradation rather than silently watching a prefix.
pub(super) const DIRS_PER_SESSION: usize = 4096;

/// Opens awaiting a close, before the stalest is dropped.
///
/// One inotify read carries at most a few hundred events, but a burst of opens
/// with no closes spans many reads and only the age prune between them would
/// ever shrink this. 4096 paths is far more than any real editor holds open.
pub(super) const PENDING_OPENS: usize = 4096;

/// Directory names never worth watching.
///
/// Build outputs and VCS internals churn constantly and are never a file two
/// agents are collaborating on by hand. Skipping them is what keeps the watch
/// count proportional to the source tree rather than to the artefacts.
const SKIP: [&str; 8] = [
    ".git",
    "target",
    "node_modules",
    ".venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "dist",
];

/// Start the watcher thread.
pub(super) fn start(
    live: &[(SessionId, PathBuf, u32)],
    _service: Arc<super::OverlapService>,
    publish: Publish,
) -> Watcher {
    let running = Arc::new(AtomicBool::new(true));
    let state = Arc::new(Mutex::new(Tracked::default()));

    let inotify = match rustix::fs::inotify::init(rustix::fs::inotify::CreateFlags::empty()) {
        Ok(fd) => fd,
        Err(e) => {
            // No watcher at all. Said out loud: an empty report from a daemon
            // that could not start a watcher must not read as "nothing
            // collides".
            state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .degraded
                .push(format!("The file watcher could not start ({e}), so no change is seen."));
            return Watcher {
                running: Arc::new(AtomicBool::new(false)),
                state,
                adder: None,
                wds: Arc::new(Mutex::new(HashMap::new())),
            };
        }
    };

    let wds: Arc<Mutex<HashMap<i32, PathBuf>>> = Arc::new(Mutex::new(HashMap::new()));
    let reader = std::fs::File::from(inotify);
    let adder = match reader.try_clone() {
        Ok(f) => Some(Arc::new(f)),
        Err(_) => None,
    };
    let w = Watcher {
        running: Arc::clone(&running),
        state: Arc::clone(&state),
        adder,
        wds: Arc::clone(&wds),
    };

    add_all(&reader, &wds, &state, live);
    {
        let mut t = state.lock().unwrap_or_else(|p| p.into_inner());
        reconcile(&mut t, live);
    }

    std::thread::Builder::new()
        .name("vitrum-overlap".to_string())
        .spawn(move || read_loop(reader, wds, state, running, publish))
        .ok();

    w
}

/// Reconcile the watch set against the sessions that are live now.
///
/// The session list is reconciled, watches for directories under no live root
/// are removed, and anything new is watched. Adding is what has to be prompt,
/// because a session that just started is exactly the second agent this
/// feature exists to catch.
pub(super) fn sync(w: &Watcher, live: &[(SessionId, PathBuf, u32)]) {
    {
        let mut t = w.state.lock().unwrap_or_else(|p| p.into_inner());
        reconcile(&mut t, live);
    }
    // Watch anything not already watched. A client subscribes on connect,
    // which is BEFORE it has started a session, so the roots that matter all
    // appear after the subscription: adding them here is not an optimisation,
    // it is the only reason the watch set is ever non-empty.
    //
    // `add_all` skips a directory it already holds a watch for, so this is
    // idempotent and costs one map lookup per directory on the common path
    // where nothing new appeared.
    if let Some(adder) = w.adder.as_ref() {
        prune(adder, &w.wds, live);
        add_all(adder, &w.wds, &w.state, live);
    }
}

/// Drop watches for directories under no live session root.
///
/// An inotify watch is a kernel resource held until it is removed or the
/// instance is closed, and the instance lives as long as the subscription, so
/// leaving them behind leaked one watch per directory of every session that
/// ever ended, against a per-user limit the daemon does not own. They also sat
/// in `wds` suppressing a re-watch if a later session took the same root.
fn prune(
    inotify: &std::fs::File,
    wds: &Arc<Mutex<HashMap<i32, PathBuf>>>,
    live: &[(SessionId, PathBuf, u32)],
) {
    let mut map = wds.lock().unwrap_or_else(|p| p.into_inner());
    map.retain(|wd, dir| {
        if live.iter().any(|(_, root, _)| dir.starts_with(root)) {
            return true;
        }
        // A watch whose directory was deleted is already gone from the kernel,
        // so a failure here only confirms what was wanted.
        let _ = rustix::fs::inotify::remove_watch(inotify.as_fd(), *wd);
        false
    });
}

/// Put `live` into `t`, keeping what is already known about surviving
/// sessions and dropping what is known about ended ones.
fn reconcile(t: &mut Tracked, live: &[(SessionId, PathBuf, u32)]) {
    t.sessions.retain(|s| live.iter().any(|(id, _, _)| *id == s.id));
    for (id, root, pid) in live {
        match t.sessions.iter_mut().find(|s| s.id == *id) {
            // The pid can change if the session respawned its child.
            Some(s) => s.pid = *pid,
            None => t.sessions.push(Watched {
                id: *id,
                root: root.clone(),
                pid: *pid,
                writes: HashMap::new(),
                unattributed: 0,
            }),
        }
    }
}

/// Watch every directory under every session root.
fn add_all(
    inotify: &std::fs::File,
    wds: &Arc<Mutex<HashMap<i32, PathBuf>>>,
    state: &Arc<Mutex<Tracked>>,
    live: &[(SessionId, PathBuf, u32)],
) {
    use rustix::fs::inotify::WatchFlags;
    // CLOSE_WRITE, not MODIFY: one event per file closed after writing rather
    // than one per write() call, so a program emitting a file in a thousand
    // chunks costs one attribution walk instead of a thousand.
    //
    // OPEN as well, and that is what makes attribution work at all. inotify
    // never says WHO changed a file, so the credit is reconstructed by looking
    // for the open descriptor -- and by CLOSE_WRITE the writer has, by
    // definition, already closed it. Scanning at OPEN catches the process
    // while it still holds the file, and the answer is held until the matching
    // CLOSE_WRITE says a write actually happened. Without this, a program that
    // opens, writes and closes quickly is never attributable to anybody.
    let flags = WatchFlags::CLOSE_WRITE
        | WatchFlags::OPEN
        | WatchFlags::MOVED_TO
        | WatchFlags::CREATE;
    // Everything already watched, so a re-sync re-walks the tree cheaply
    // instead of spending a redundant `inotify_add_watch` per directory.
    let mut seen: HashMap<PathBuf, ()> = wds
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .values()
        .map(|d| (d.clone(), ()))
        .collect();
    // The cap is on the watch SET, not on one walk. Counting only what this
    // call added let a tree over the cap take another DIRS_PER_SESSION watches
    // on every sync, which is unbounded growth over a daemon's lifetime.
    let mut held = vec![0usize; live.len()];
    for dir in seen.keys() {
        if let Some(i) = live.iter().position(|(_, root, _)| dir.starts_with(root)) {
            held[i] += 1;
        }
    }
    for (i, (_, root, _)) in live.iter().enumerate() {
        let mut n = held[i];
        let mut stack = vec![root.clone()];
        let mut capped = false;
        while let Some(dir) = stack.pop() {
            if !seen.contains_key(&dir) {
                if n >= DIRS_PER_SESSION {
                    capped = true;
                    break;
                }
                match rustix::fs::inotify::add_watch(inotify.as_fd(), &dir, flags) {
                    Ok(wd) => {
                        wds.lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(wd, dir.clone());
                        seen.insert(dir.clone(), ());
                        n += 1;
                    }
                    Err(_) => continue,
                }
            }
            // Descend even into a directory already watched. Skipping it
            // stopped the walk at the root on every re-sync, so a module an
            // agent created after the client subscribed was never watched and
            // nothing written under it was ever detected.
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                if !ft.is_dir() {
                    continue;
                }
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') && name != "." || SKIP.contains(&name.as_ref()) {
                    continue;
                }
                stack.push(e.path());
            }
        }
        if capped {
            state.lock().unwrap_or_else(|p| p.into_inner()).degrade(format!(
                "{} has more than {DIRS_PER_SESSION} directories, so part of it is not watched.",
                root.display()
            ));
        }
    }
}

/// Read events until the subscription is dropped.
fn read_loop(
    inotify: std::fs::File,
    wds: Arc<Mutex<HashMap<i32, PathBuf>>>,
    state: Arc<Mutex<Tracked>>,
    running: Arc<AtomicBool>,
    publish: Publish,
) {
    let mut file = inotify;
    let mut buf = [0u8; 8192];
    let mut last: Vec<vitrum_proto::Collision> = Vec::new();
    // Path to the session seen holding it open, awaiting a close that says a
    // write happened. An open with no matching close is ordinary (a long-lived
    // reader, or a process that died holding the file), so this is bounded two
    // ways: by age between read batches, and by [`PENDING_OPENS`] within one.
    let mut pending: HashMap<PathBuf, (SessionId, u64)> = HashMap::new();
    const IN_MOVED_TO: u32 = 0x0000_0080;
    while running.load(Ordering::Relaxed) {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if !running.load(Ordering::Relaxed) {
            break;
        }
        let now_ms = now_ms();
        let mut at = 0usize;
        while at + 16 <= n {
            let wd = i32::from_ne_bytes(buf[at..at + 4].try_into().unwrap_or([0; 4]));
            let mask = u32::from_ne_bytes(buf[at + 4..at + 8].try_into().unwrap_or([0; 4]));
            let len = u32::from_ne_bytes(buf[at + 12..at + 16].try_into().unwrap_or([0; 4]))
                as usize;
            let name_at = at + 16;
            let name_end = (name_at + len).min(n);
            let name = buf
                .get(name_at..name_end)
                .map(|b| {
                    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
                    String::from_utf8_lossy(&b[..end]).into_owned()
                })
                .unwrap_or_default();
            at = name_at + len;

            // Q_OVERFLOW: the kernel dropped events. This is the one case
            // where the history genuinely has a hole, so it is reported
            // rather than absorbed.
            const IN_Q_OVERFLOW: u32 = 0x0000_4000;
            if mask & IN_Q_OVERFLOW != 0 {
                state.lock().unwrap_or_else(|p| p.into_inner()).degrade(
                    "The kernel dropped change events, so some writes were never seen."
                        .to_string(),
                );
                continue;
            }
            if name.is_empty() {
                continue;
            }
            let Some(dir) = wds
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get(&wd)
                .cloned()
            else {
                continue;
            };
            let path = dir.join(&name);
            if path.is_dir() {
                continue;
            }
            const IN_OPEN: u32 = 0x0000_0020;
            const IN_CLOSE_WRITE: u32 = 0x0000_0008;
            if mask & IN_OPEN != 0 {
                // Somebody has it open RIGHT NOW. Ask who, and hold the answer
                // until a close tells us a write actually happened; an open
                // for reading must never be recorded as a change.
                if let Some(id) = holder(&state, &path) {
                    remember_open(&mut pending, path.clone(), id, now_ms);
                }
                continue;
            }
            let opened = pending.remove(&path);
            if mask & IN_CLOSE_WRITE != 0 || mask & IN_MOVED_TO != 0 {
                credit(&state, &path, &dir, now_ms, opened.map(|(id, _)| id));
            }
        }

        // An open with no close is normal, so the pending map is pruned by
        // age rather than waiting for a matching event that may never come.
        pending.retain(|_, (_, at)| now_ms.saturating_sub(*at) < 30_000);

        // Publish only when the CONTESTED SET changed. A busy agent produces
        // thousands of writes to files nobody else touches, and rebroadcasting
        // an identical report on each one would repaint every window for no
        // new information.
        let next = {
            let t = state.lock().unwrap_or_else(|p| p.into_inner());
            t.collisions(now_ms)
        };
        if next != last {
            // The contested list is reused rather than rebuilt: the old code
            // ran the whole scan a second time under a second lock just to
            // fill the message, doubling the per-batch cost of the one thing
            // that touches every tracked path.
            let (sessions, degraded) = {
                let t = state.lock().unwrap_or_else(|p| p.into_inner());
                (t.per_session(), t.degraded.clone())
            };
            publish(vitrum_proto::ServerMsg::CollisionReport {
                watching: true,
                collisions: next.clone(),
                sessions,
                degraded,
            });
            last = next;
        }
    }
}

/// Note that `id` holds `path` open, evicting the stalest note at the bound.
///
/// The age prune only runs between read batches, so one burst of opens with no
/// closes must not grow the map without limit.
fn remember_open(
    pending: &mut HashMap<PathBuf, (SessionId, u64)>,
    path: PathBuf,
    id: SessionId,
    now_ms: u64,
) {
    if !pending.contains_key(&path)
        && pending.len() >= PENDING_OPENS
        && let Some(stalest) = pending
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(p, _)| p.clone())
    {
        pending.remove(&stalest);
    }
    pending.insert(path, (id, now_ms));
}

/// Which watched session, if any, currently holds `path` open.
///
/// Used at OPEN time, when the writer provably still has the descriptor. Two
/// sessions holding one file at once is itself the collision, and returning
/// `None` for that case is deliberate: `credit` re-scans and records both.
fn holder(state: &Arc<Mutex<Tracked>>, path: &Path) -> Option<SessionId> {
    let candidates: Vec<(SessionId, u32)> = {
        let t = state.lock().unwrap_or_else(|p| p.into_inner());
        t.sessions
            .iter()
            .filter(|s| path.starts_with(&s.root))
            .map(|s| (s.id, s.pid))
            .collect()
    };
    let mut found = candidates
        .iter()
        .filter(|(_, pid)| tree_has_open(*pid, path))
        .map(|(id, _)| *id);
    let first = found.next()?;
    // Ambiguous: let `credit` do the full scan rather than pick one.
    found.next().is_none().then_some(first)
}

/// Pin one change on a session, or count it as unattributed.
///
/// `opened` is the session seen holding the file when it was opened, which is
/// the only reliable evidence for a write that finishes in microseconds.
fn credit(
    state: &Arc<Mutex<Tracked>>,
    path: &Path,
    dir: &Path,
    now_ms: u64,
    opened: Option<SessionId>,
) {
    if let Some(id) = opened {
        let mut t = state.lock().unwrap_or_else(|p| p.into_inner());
        t.record(id, path, now_ms, Credit::Observed);
        return;
    }
    let candidates: Vec<(SessionId, u32, PathBuf)> = {
        let t = state.lock().unwrap_or_else(|p| p.into_inner());
        t.sessions
            .iter()
            .filter(|s| path.starts_with(&s.root))
            .map(|s| (s.id, s.pid, s.root.clone()))
            .collect()
    };
    if candidates.is_empty() {
        return;
    }
    // Only one session could possibly have written it. No /proc walk needed,
    // and no inference either: within one root, one candidate IS the answer.
    if candidates.len() == 1 {
        let mut t = state.lock().unwrap_or_else(|p| p.into_inner());
        t.record(candidates[0].0, path, now_ms, Credit::Observed);
        return;
    }

    // Two or more sessions share this tree, which is the whole interesting
    // case. Ask the kernel who is holding the file open.
    let holders: Vec<SessionId> = candidates
        .iter()
        .filter(|(_, pid, _)| tree_has_open(*pid, path))
        .map(|(id, _, _)| *id)
        .collect();
    let mut t = state.lock().unwrap_or_else(|p| p.into_inner());
    match holders.as_slice() {
        [one] => t.record(*one, path, now_ms, Credit::Observed),
        // Nobody still holds it. Fall back to "who has been writing this file
        // recently", and only when the answer is unambiguous.
        [] => {
            let recent: Vec<SessionId> = t
                .sessions
                .iter()
                .filter(|s| s.writes.contains_key(path))
                .map(|s| s.id)
                .collect();
            match recent.as_slice() {
                [one] => {
                    let id = *one;
                    t.record(id, path, now_ms, Credit::Inferred);
                }
                _ => t.unattributed(dir),
            }
        }
        // Two sessions holding one file open at once is itself the collision,
        // and each is genuinely observed.
        many => {
            let ids: Vec<SessionId> = many.to_vec();
            for id in ids {
                t.record(id, path, now_ms, Credit::Observed);
            }
        }
    }
}

/// Does any process in `pid`'s tree hold `path` open?
///
/// `/proc/<pid>/fd` is readable only for our own processes, which these are:
/// the daemon spawned them. A denial is treated as "no", and the change falls
/// through to inference rather than being credited to a guess.
fn tree_has_open(pid: u32, path: &Path) -> bool {
    let mut stack = vec![pid];
    let mut seen = 0usize;
    while let Some(p) = stack.pop() {
        // A bound, because a runaway fork bomb must not turn one file event
        // into an unbounded walk on the watcher thread.
        seen += 1;
        if seen > 256 {
            return false;
        }
        if has_open(p, path) {
            return true;
        }
        stack.extend(children_of(p));
    }
    false
}

fn has_open(pid: u32, path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return false;
    };
    for e in entries.flatten() {
        if std::fs::read_link(e.path()).is_ok_and(|target| target == path) {
            return true;
        }
    }
    false
}

/// Direct children of `pid`, from `/proc/<pid>/task/*/children`.
fn children_of(pid: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return out;
    };
    for t in tasks.flatten() {
        let Ok(text) = std::fs::read_to_string(t.path().join("children")) else {
            continue;
        };
        out.extend(text.split_ascii_whitespace().filter_map(|s| s.parse::<u32>().ok()));
    }
    out
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `reconcile` for the unit tests, which have no inotify fd.
///
/// The reconcile rule is the interesting half: what survives a session
/// starting or ending, and what must not. Exposing it keeps that testable
/// without a kernel.
#[cfg(test)]
pub(super) fn reconcile_for_test(t: &mut Tracked, live: &[(SessionId, PathBuf, u32)]) {
    reconcile(t, live);
}

/// `remember_open` for the unit tests, which have no inotify fd.
///
/// The eviction rule is the interesting half: what a burst of opens with no
/// closes costs, and which entry goes when the bound is reached.
#[cfg(test)]
pub(super) fn remember_open_for_test(
    pending: &mut HashMap<PathBuf, (SessionId, u64)>,
    path: PathBuf,
    id: SessionId,
    now_ms: u64,
) {
    remember_open(pending, path, id, now_ms);
}
