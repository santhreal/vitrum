//! What counts as a collision, and what the daemon refuses to claim.
//!
//! The detection thread needs a kernel and two real agents, so what is proved
//! here is the part that decides an ANSWER from a set of observed writes. That
//! is where the product judgement lives: a rule that fires on two agents
//! sharing a repository is a rule that fires constantly and gets ignored, and
//! a rule that reports "nothing collides" when nobody looked is worse than no
//! rule at all.

use super::*;

const NOW: u64 = 1_772_000_000_000;

fn watched(id: u64, root: &str) -> Watched {
    Watched {
        id: SessionId(id),
        root: PathBuf::from(root),
        pid: 1000 + id as u32,
        writes: HashMap::new(),
        unattributed: 0,
    }
}

fn tracked(sessions: Vec<Watched>) -> Tracked {
    Tracked {
        sessions,
        degraded: Vec::new(),
    }
}

/// TWO sessions on ONE file. Not one, and not two on a shared directory.
///
/// This is the entire product decision. Ten agents in a large checkout
/// normally do not conflict, so a warning keyed on "same repository" or "same
/// directory" fires on every busy machine and is muted within a day. Keyed on
/// the file, it fires on the failure that actually destroys work.
#[test]
fn a_collision_is_two_sessions_on_one_file_and_nothing_looser() {
    let mut t = tracked(vec![watched(1, "/src/repo"), watched(2, "/src/repo")]);
    let shared = Path::new("/src/repo/app.rs");

    // One session writing its own file all day is not news.
    t.record(SessionId(1), shared, NOW, Credit::Observed);
    assert!(t.collisions(NOW).is_empty());

    // Two sessions in the same DIRECTORY, different files, is not news either.
    t.record(SessionId(2), Path::new("/src/repo/other.rs"), NOW, Credit::Observed);
    assert!(
        t.collisions(NOW).is_empty(),
        "sharing a directory was reported as a collision"
    );

    // Two sessions on ONE file is.
    t.record(SessionId(2), shared, NOW, Credit::Observed);
    let found = t.collisions(NOW);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, "/src/repo/app.rs");
    assert_eq!(
        found[0].participants.iter().map(|p| p.session.0).collect::<Vec<_>>(),
        vec![1, 2],
        "participants must be ordered by session id so the row is stable"
    );
}

/// A write that has aged out of the window is not a collision.
///
/// Two agents editing one file an hour apart is a handover, not a fight. A
/// rule with no window lights a row up today because of something that
/// finished yesterday, and an alert that is usually stale is an alert nobody
/// reads.
#[test]
fn an_old_write_stops_counting() {
    let mut t = tracked(vec![watched(1, "/src/repo"), watched(2, "/src/repo")]);
    let shared = Path::new("/src/repo/app.rs");
    t.record(SessionId(1), shared, NOW, Credit::Observed);
    t.record(SessionId(2), shared, NOW, Credit::Observed);
    assert_eq!(t.collisions(NOW).len(), 1);

    // Both writes are now older than the window.
    let later = NOW + WINDOW_MS + 1;
    assert!(
        t.collisions(later).is_empty(),
        "a fight that ended an hour ago is still being reported"
    );
}

/// An observation must outrank an earlier inference, and never the reverse.
///
/// The UI hedges on an inferred credit, so a pair that was once guessed at and
/// has since been directly observed must stop hedging. Going the other way
/// would let a later guess erase a fact.
#[test]
fn an_observation_upgrades_a_guess_and_a_guess_never_downgrades_a_fact() {
    let mut t = tracked(vec![watched(1, "/src/repo")]);
    let p = Path::new("/src/repo/app.rs");

    t.record(SessionId(1), p, NOW, Credit::Inferred);
    assert_eq!(t.sessions[0].writes[p].credit, Credit::Inferred);

    t.record(SessionId(1), p, NOW + 1, Credit::Observed);
    assert_eq!(t.sessions[0].writes[p].credit, Credit::Observed);

    t.record(SessionId(1), p, NOW + 2, Credit::Inferred);
    assert_eq!(
        t.sessions[0].writes[p].credit,
        Credit::Observed,
        "a later guess overwrote something we had actually seen"
    );
}

/// Repeated writes accumulate on one entry rather than multiplying it.
///
/// An agent rewriting one file two hundred times is one contested file, not
/// two hundred. The count is kept because "touched it once" and "has been
/// hammering it" are different situations for the operator to walk into.
#[test]
fn repeated_writes_extend_one_record() {
    let mut t = tracked(vec![watched(1, "/src/repo")]);
    let p = Path::new("/src/repo/app.rs");
    for i in 0..5 {
        t.record(SessionId(1), p, NOW + i, Credit::Observed);
    }
    assert_eq!(t.sessions[0].writes.len(), 1);
    let w = t.sessions[0].writes[p];
    assert_eq!(w.writes, 5);
    assert_eq!(w.first_ms, NOW, "the first sighting must not move");
    assert_eq!(w.last_ms, NOW + 4);
}

/// The per-session path map is bounded, and drops the STALEST first.
///
/// An agent that rewrites a hundred thousand files must not grow this map
/// without limit inside a long-lived daemon. Evicting the oldest is right
/// because a collision is about concurrent work: the file nobody has touched
/// in five hundred writes is not the one being fought over now.
#[test]
fn the_path_map_is_bounded_and_evicts_the_stalest() {
    let mut t = tracked(vec![watched(1, "/src/repo")]);
    let hot = PathBuf::from("/src/repo/hot.rs");
    t.record(SessionId(1), &hot, NOW, Credit::Observed);
    for i in 0..PATHS_PER_SESSION + 20 {
        let p = PathBuf::from(format!("/src/repo/f{i}.rs"));
        // Every one of these is NEWER than the first write to `hot`, so `hot`
        // is the eviction candidate until it is touched again.
        t.record(SessionId(1), &p, NOW + 10 + i as u64, Credit::Observed);
    }
    assert!(
        t.sessions[0].writes.len() <= PATHS_PER_SESSION,
        "the map grew past its bound: {}",
        t.sessions[0].writes.len()
    );
    assert!(
        !t.sessions[0].writes.contains_key(&hot),
        "the stalest entry survived while newer ones were dropped"
    );
}

/// An unattributed change is counted, never guessed at.
///
/// This is the number that stops a client rendering a confident "nothing is
/// colliding". A session whose writes were mostly too short to catch has an
/// empty collision list for a reason that is not safety, and the count is the
/// honest denominator beside it.
#[test]
fn a_change_nobody_can_be_credited_with_is_counted() {
    let mut t = tracked(vec![watched(1, "/src/repo")]);
    assert_eq!(t.per_session()[0].unattributed, 0);
    t.unattributed(Path::new("/src/repo"));
    t.unattributed(Path::new("/src/repo"));
    assert_eq!(t.per_session()[0].unattributed, 2);
    assert!(
        t.collisions(NOW).is_empty(),
        "an unattributed change must never invent a participant"
    );
}

/// A session that ends leaves detection, and takes its history with it.
///
/// A dead session cannot be fighting over a file, and its pid is reapable and
/// reusable, so leaving it in would let a recycled pid be credited with a
/// stranger's writes.
#[test]
fn an_ended_session_is_dropped_from_the_watch_set() {
    let mut t = tracked(vec![watched(1, "/src/repo"), watched(2, "/src/repo")]);
    let shared = Path::new("/src/repo/app.rs");
    t.record(SessionId(1), shared, NOW, Credit::Observed);
    t.record(SessionId(2), shared, NOW, Credit::Observed);
    assert_eq!(t.collisions(NOW).len(), 1);

    // Session 2 ends. What is left is one session writing its own file.
    platform_reconcile(&mut t, &[(SessionId(1), PathBuf::from("/src/repo"), 1001)]);
    assert_eq!(t.sessions.len(), 1);
    assert!(
        t.collisions(NOW).is_empty(),
        "a session that ended is still listed as contesting a file"
    );
}

/// A surviving session keeps its history across a reconcile.
///
/// `sync` runs whenever any session starts or ends, which on a busy machine is
/// often. Rebuilding the set from scratch each time would erase the write
/// history that a collision is made of, and the feature would only ever fire
/// in the gaps between session churn.
#[test]
fn a_reconcile_preserves_what_surviving_sessions_have_written() {
    let mut t = tracked(vec![watched(1, "/src/repo")]);
    t.record(SessionId(1), Path::new("/src/repo/app.rs"), NOW, Credit::Observed);

    platform_reconcile(
        &mut t,
        &[
            (SessionId(1), PathBuf::from("/src/repo"), 1001),
            (SessionId(2), PathBuf::from("/src/repo"), 1002),
        ],
    );
    assert_eq!(t.sessions.len(), 2);
    assert_eq!(
        t.sessions
            .iter()
            .find(|s| s.id == SessionId(1))
            .map(|s| s.writes.len()),
        Some(1),
        "the surviving session's history was thrown away by a reconcile"
    );

    // And the newcomer writing that same file is now a collision.
    t.record(SessionId(2), Path::new("/src/repo/app.rs"), NOW + 1, Credit::Observed);
    assert_eq!(t.collisions(NOW + 1).len(), 1);
}

/// A respawned child's new pid replaces the old one.
///
/// Attribution reads `/proc/<pid>`, so a stale pid means either no credit at
/// all or, once the kernel recycles it, credit given to an unrelated process.
#[test]
fn a_reconcile_updates_a_changed_child_pid() {
    let mut t = tracked(vec![watched(1, "/src/repo")]);
    assert_eq!(t.sessions[0].pid, 1001);
    platform_reconcile(&mut t, &[(SessionId(1), PathBuf::from("/src/repo"), 2222)]);
    assert_eq!(t.sessions[0].pid, 2222);
}

/// An unsubscribed service reports `watching: false`, not an empty answer.
///
/// The two are the same data and opposite claims. Rendering them the same way
/// tells an operator their agents are not fighting when nothing has looked,
/// which is the one answer this feature must never give.
#[test]
fn an_unsubscribed_service_says_nobody_looked() {
    let s = OverlapService::new();
    assert!(!s.is_watching());
    let ServerMsg::CollisionReport {
        watching,
        collisions,
        sessions,
        ..
    } = s.report(NOW)
    else {
        panic!("report is not a CollisionReport");
    };
    assert!(!watching, "an unsubscribed daemon claimed to be watching");
    assert!(collisions.is_empty());
    assert!(sessions.is_empty());
}

/// `reconcile` under test, without the inotify half.
#[cfg(target_os = "linux")]
fn platform_reconcile(t: &mut Tracked, live: &[(SessionId, PathBuf, u32)]) {
    super::platform::reconcile_for_test(t, live);
}

#[cfg(not(target_os = "linux"))]
fn platform_reconcile(_t: &mut Tracked, _live: &[(SessionId, PathBuf, u32)]) {}

/// A session that starts AFTER the subscription must still be watched.
///
/// This is the link that shipped broken, and the only test here that touches
/// a real kernel, because it is the only way to see it. The watch set was
/// established once when a client subscribed, and `sync` updated the session
/// list without ever adding a watch. A client subscribes on connect, which is
/// before it has started anything, so the set stayed empty for the life of the
/// daemon: every pure-function test below passed, the watcher thread ran, the
/// inotify fd existed, and nothing was ever detected.
///
/// Deleting the add from `sync` still compiles and still passes every other
/// test in this file. That is why this one exists.
#[cfg(target_os = "linux")]
#[test]
fn a_session_that_appears_after_the_subscription_gets_watched() {
    let root = std::env::temp_dir().join(format!(
        "vitrum-overlap-{}-{}",
        std::process::id(),
        NOW
    ));
    std::fs::create_dir_all(root.join("nested")).expect("temp tree");

    let service = OverlapService::new();
    let publish: Publish = Arc::new(|_| {});

    // Subscribe with NOTHING live, exactly as a window does on connect.
    service.set_watching(true, &[], &publish, NOW);
    assert!(service.is_watching());
    assert_eq!(
        service.watch_count(),
        0,
        "there is nothing to watch yet, so nothing should be watched"
    );

    // A session starts. This is the moment the link has to fire.
    let live = [(SessionId(1), root.clone(), std::process::id())];
    service.sync(&live);
    assert!(
        service.watch_count() >= 1,
        "a session that started after the subscription is not being watched, \
         so nothing it does will ever be detected"
    );

    // Unsubscribing must release everything: no thread, no watches.
    service.set_watching(false, &live, &publish, NOW);
    assert!(!service.is_watching());
    assert_eq!(
        service.watch_count(),
        0,
        "unsubscribing left watches behind, so the idle claim is false"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// WHY: the "tree too large" note names its root and was pushed on every sync,
/// so a long-lived daemon accumulated one copy per sync forever, and the whole
/// list is cloned into every report sent to every window.
#[test]
fn a_degradation_is_recorded_once_and_the_list_is_bounded() {
    let mut t = tracked(vec![]);
    for _ in 0..50 {
        t.degrade("/src/repo has too many directories.".to_string());
    }
    assert_eq!(t.degraded.len(), 1, "the same degradation was recorded twice");

    for i in 0..MAX_DEGRADED + 20 {
        t.degrade(format!("/src/repo{i} has too many directories."));
    }
    assert_eq!(
        t.degraded.len(),
        MAX_DEGRADED,
        "distinct degradations grew past the bound"
    );
}

/// WHY: the age prune only runs between inotify read batches, so a burst of
/// opens with no closes grew this map unbounded on the watcher thread.
#[cfg(target_os = "linux")]
#[test]
fn pending_opens_are_bounded_and_evict_the_stalest() {
    use super::platform::remember_open_for_test as remember;

    let mut pending = HashMap::new();
    let stale = PathBuf::from("/src/repo/stale.rs");
    remember(&mut pending, stale.clone(), SessionId(1), NOW);
    for i in 0..super::platform::PENDING_OPENS + 20 {
        let p = PathBuf::from(format!("/src/repo/f{i}.rs"));
        remember(&mut pending, p, SessionId(1), NOW + 10 + i as u64);
    }
    assert!(
        pending.len() <= super::platform::PENDING_OPENS,
        "the pending map grew past its bound: {}",
        pending.len()
    );
    assert!(
        !pending.contains_key(&stale),
        "the stalest open survived while newer ones were dropped"
    );
}

/// WHY: an inotify watch is a kernel resource held until it is removed or the
/// instance closes, and the instance lives as long as the subscription. Watches
/// for a session that ended were never removed, so every session a long-running
/// daemon hosted leaked one watch per directory of its tree against a per-user
/// limit the daemon does not own.
#[cfg(target_os = "linux")]
#[test]
fn an_ended_session_releases_its_inotify_watches() {
    let base = std::env::temp_dir().join(format!("vitrum-prune-{}", std::process::id()));
    let keep = base.join("keep");
    let ends = base.join("ends");
    std::fs::create_dir_all(keep.join("nested")).expect("temp tree");
    std::fs::create_dir_all(ends.join("a/b")).expect("temp tree");

    let service = OverlapService::new();
    let publish: Publish = Arc::new(|_| {});
    service.set_watching(true, &[], &publish, NOW);

    let both = [
        (SessionId(1), keep.clone(), std::process::id()),
        (SessionId(2), ends.clone(), std::process::id()),
    ];
    service.sync(&both);
    let watched_both = service.watch_count();
    assert_eq!(watched_both, 5, "both trees must be watched to start with");

    // Session 2 ends. Its three directories must leave the watch set.
    service.sync(&both[..1]);
    assert_eq!(
        service.watch_count(),
        2,
        "the ended session's watches were leaked into the kernel"
    );

    service.set_watching(false, &both[..1], &publish, NOW);
    let _ = std::fs::remove_dir_all(&base);
}

/// WHY: the per-session directory cap counted only the watches one walk added,
/// so a tree over the cap took another DIRS_PER_SESSION watches on every sync,
/// and sync runs whenever any session starts or ends.
#[cfg(target_os = "linux")]
#[test]
fn the_directory_cap_bounds_the_watch_set_not_one_walk() {
    let root = std::env::temp_dir().join(format!("vitrum-cap-{}", std::process::id()));
    let over = super::platform::DIRS_PER_SESSION + 200;
    for i in 0..over {
        std::fs::create_dir_all(root.join(format!("d{i}"))).expect("temp tree");
    }

    let service = OverlapService::new();
    let publish: Publish = Arc::new(|_| {});
    service.set_watching(true, &[], &publish, NOW);
    let live = [(SessionId(1), root.clone(), std::process::id())];

    service.sync(&live);
    service.sync(&live);
    assert!(
        service.watch_count() <= super::platform::DIRS_PER_SESSION,
        "a second sync took the watch set past its cap: {}",
        service.watch_count()
    );

    service.set_watching(false, &live, &publish, NOW);
    let _ = std::fs::remove_dir_all(&root);
}

/// WHY: the walk skipped any directory it already held a watch for, and it only
/// descended into directories it had just watched. On every re-sync it
/// therefore stopped dead at the session root, so a module an agent created
/// after the client subscribed was never watched and nothing written under it
/// was ever detected.
#[cfg(target_os = "linux")]
#[test]
fn a_directory_created_after_the_last_sync_gets_watched() {
    let root = std::env::temp_dir().join(format!("vitrum-late-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("temp tree");

    let service = OverlapService::new();
    let publish: Publish = Arc::new(|_| {});
    service.set_watching(true, &[], &publish, NOW);
    let live = [(SessionId(1), root.clone(), std::process::id())];
    service.sync(&live);
    assert_eq!(service.watch_count(), 1, "only the root exists so far");

    // The agent creates a module. Nothing about the session list changed.
    std::fs::create_dir_all(root.join("newmod/inner")).expect("temp tree");
    service.sync(&live);
    assert_eq!(
        service.watch_count(),
        3,
        "directories created after the last sync are not being watched, so \
         nothing written under them can ever be detected"
    );

    service.set_watching(false, &live, &publish, NOW);
    let _ = std::fs::remove_dir_all(&root);
}
