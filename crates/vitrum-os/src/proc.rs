//! Low-Overhead Process Tree Polling & PID Status Caching.
//!
//! Provides cached, generation-tracked process status lookups and efficient
//! process hierarchy (parent/child) traversal without redundant system calls.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Process state representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessState {
    /// Actively executing on CPU.
    Running,
    /// Sleeping / waiting for event or I/O.
    Sleeping,
    /// Stopped by signal or debugger.
    Stopped,
    /// Zombie / terminated process waiting for parent reap.
    Zombie,
    /// Dead or terminated process.
    Dead,
    /// Unknown or unclassifiable process state.
    Unknown,
}

impl fmt::Display for ProcessState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "Running"),
            Self::Sleeping => write!(f, "Sleeping"),
            Self::Stopped => write!(f, "Stopped"),
            Self::Zombie => write!(f, "Zombie"),
            Self::Dead => write!(f, "Dead"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Metadata and state snapshot for a single process.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    /// Process ID.
    pub pid: u32,
    /// Parent Process ID.
    pub ppid: u32,
    /// Executable or command name.
    pub name: String,
    /// Process state.
    pub state: ProcessState,
    /// Resident Set Size (RSS) memory in bytes.
    pub rss_bytes: u64,
    /// CPU usage percentage (0.0 to 100.0 * num_cores).
    pub cpu_percent: f32,
    /// Cache generation when this process info was last updated.
    pub generation: u64,
    /// Timestamp in nanoseconds when this snapshot was created.
    pub updated_at_ns: u64,
}

/// Low-overhead PID status cache and process tree index.
#[derive(Debug)]
pub struct ProcStatusCache {
    procs: BTreeMap<u32, ProcInfo>,
    children_map: BTreeMap<u32, Vec<u32>>,
    generation: u64,
    ttl_ns: u64,
}

impl Default for ProcStatusCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcStatusCache {
    /// Create cache with default 500ms TTL.
    pub fn new() -> Self {
        Self {
            procs: BTreeMap::new(),
            children_map: BTreeMap::new(),
            generation: 1,
            ttl_ns: 500_000_000, // 500 ms
        }
    }

    /// Customize TTL window in nanoseconds.
    pub fn with_ttl_ns(mut self, ttl_ns: u64) -> Self {
        self.ttl_ns = ttl_ns;
        self
    }

    /// Advance cache generation counter for a new polling sweep.
    pub fn advance_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.generation = 1;
        }
        self.generation
    }

    /// Current cache generation.
    pub fn current_generation(&self) -> u64 {
        self.generation
    }

    /// Upsert process metadata snapshot into the cache.
    pub fn upsert(&mut self, mut info: ProcInfo) {
        info.generation = self.generation;
        let pid = info.pid;
        let ppid = info.ppid;

        // If parent changed or new process, update children index
        if let Some(old) = self.procs.insert(pid, info) {
            if old.ppid != ppid {
                if let Some(children) = self.children_map.get_mut(&old.ppid) {
                    children.retain(|&child_pid| child_pid != pid);
                }
                self.children_map.entry(ppid).or_default().push(pid);
            }
        } else {
            self.children_map.entry(ppid).or_default().push(pid);
        }
    }

    /// Fetch process info by PID if present and fresh according to TTL.
    pub fn get(&self, pid: u32, now_ns: u64) -> Option<&ProcInfo> {
        let info = self.procs.get(&pid)?;
        if now_ns.saturating_sub(info.updated_at_ns) <= self.ttl_ns {
            Some(info)
        } else {
            None
        }
    }

    /// Fetch process info ignoring TTL staleness check.
    pub fn get_cached(&self, pid: u32) -> Option<&ProcInfo> {
        self.procs.get(&pid)
    }

    /// Direct children PIDs of a given parent PID.
    pub fn children_of(&self, parent_pid: u32) -> &[u32] {
        self.children_map
            .get(&parent_pid)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Traverse ancestors up to init / process 1.
    pub fn ancestors_of(&self, mut pid: u32) -> Vec<u32> {
        let mut ancestors = Vec::new();
        let mut visited = BTreeSet::new();

        while let Some(info) = self.procs.get(&pid) {
            if info.ppid == 0 || info.ppid == pid || visited.contains(&info.ppid) {
                break;
            }
            ancestors.push(info.ppid);
            visited.insert(info.ppid);
            pid = info.ppid;
        }

        ancestors
    }

    /// Recursively collect all descendant PIDs under a root PID.
    pub fn descendants_of(&self, root_pid: u32) -> Vec<u32> {
        let mut descendants = Vec::new();
        let mut stack = self.children_of(root_pid).to_vec();

        while let Some(pid) = stack.pop() {
            descendants.push(pid);
            if let Some(children) = self.children_map.get(&pid) {
                stack.extend_from_slice(children);
            }
        }

        descendants
    }

    /// Invalidate/remove a specific PID from the cache.
    pub fn invalidate(&mut self, pid: u32) -> bool {
        if let Some(info) = self.procs.remove(&pid) {
            if let Some(children) = self.children_map.get_mut(&info.ppid) {
                children.retain(|&c| c != pid);
            }
            true
        } else {
            false
        }
    }

    /// Prune any process entry older than TTL relative to `now_ns`.
    pub fn prune_stale(&mut self, now_ns: u64) -> usize {
        let ttl = self.ttl_ns;
        let stale_pids: Vec<u32> = self
            .procs
            .iter()
            .filter(|(_, info)| now_ns.saturating_sub(info.updated_at_ns) > ttl)
            .map(|(&pid, _)| pid)
            .collect();

        let count = stale_pids.len();
        for pid in stale_pids {
            self.invalidate(pid);
        }
        count
    }

    /// Total process entries cached.
    pub fn len(&self) -> usize {
        self.procs.len()
    }

    /// Returns true if cache is empty.
    pub fn is_empty(&self) -> bool {
        self.procs.is_empty()
    }
}
