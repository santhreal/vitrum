use crate::proc::{ProcInfo, ProcStatusCache, ProcessState};

#[test]
fn proc_status_cache_basic_operations() {
    let mut cache = ProcStatusCache::new().with_ttl_ns(100_000_000); // 100ms
    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());

    let p1 = ProcInfo {
        pid: 100,
        ppid: 1,
        name: "vitrum-main".to_string(),
        state: ProcessState::Running,
        rss_bytes: 1024 * 1024 * 32,
        cpu_percent: 5.5,
        generation: 0,
        updated_at_ns: 1_000_000,
    };

    let p2 = ProcInfo {
        pid: 101,
        ppid: 100,
        name: "vitrum-worker-1".to_string(),
        state: ProcessState::Sleeping,
        rss_bytes: 1024 * 1024 * 16,
        cpu_percent: 1.2,
        generation: 0,
        updated_at_ns: 1_000_000,
    };

    let p3 = ProcInfo {
        pid: 102,
        ppid: 100,
        name: "vitrum-worker-2".to_string(),
        state: ProcessState::Running,
        rss_bytes: 1024 * 1024 * 24,
        cpu_percent: 12.0,
        generation: 0,
        updated_at_ns: 1_000_000,
    };

    cache.upsert(p1);
    cache.upsert(p2);
    cache.upsert(p3);

    assert_eq!(cache.len(), 3);
    assert_eq!(cache.children_of(100), &[101, 102]);
    assert_eq!(cache.ancestors_of(101), vec![100, 1]);

    let mut descendants = cache.descendants_of(100);
    descendants.sort_unstable();
    assert_eq!(descendants, vec![101, 102]);

    // Fresh TTL check
    assert!(cache.get(100, 50_000_000).is_some());
    // Stale TTL check (>100ms)
    assert!(cache.get(100, 200_000_000).is_none());
    assert!(cache.get_cached(100).is_some());

    // Prune stale entries
    let pruned = cache.prune_stale(200_000_000);
    assert_eq!(pruned, 3);
    assert_eq!(cache.len(), 0);
}
