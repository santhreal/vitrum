//! The project list the sidebar groups sessions under.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::RwLock;

use vitrum_proto::{ProjectId, ProjectInfo};

/// Projects known to this daemon, keyed by the id clients use.
///
/// The protocol has no "create project" message, so a project comes into
/// existence the first time a session is created under its id: the client owns
/// the identity, the server owns the record. Registering on first use keeps
/// `Projects` answerable without inventing a second source of truth.
///
/// A PROJECT IS NOT A THING THE OPERATOR CREATED. It is a fact about where
/// sessions are running, so it lasts exactly as long as that fact does. This
/// used to be a plain append-only map with `ensure` and `list` and no remover,
/// and the result was a daemon that greeted every new window with folders for
/// every directory anything had ever run in. After a few hours the sidebar was
/// mostly empty folders, which does not read as residue, it reads as invented.
///
/// So there is no way to ask this type for its contents WITHOUT saying which
/// projects still have a session in them. That is the whole design: not a
/// reclaim step that each removal path has to remember to call, which is
/// correct only until one of them forgets, but a single accessor that cannot
/// return a project no session is rooted in. The map underneath is a cache of
/// names and roots; [`ProjectRegistry::live`] is the only reader, and it prunes
/// as it reads.
#[derive(Default)]
pub struct ProjectRegistry {
    inner: RwLock<BTreeMap<ProjectId, ProjectInfo>>,
}

impl ProjectRegistry {
    /// Record `id` rooted at `root` unless it is already known.
    ///
    /// Returns true only when the project set actually changed, so a caller can
    /// push a project list as a delta instead of re-sending it on every session
    /// create. Existing entries are left alone: a second session in the same
    /// project must not rename or re-root it just because it was started from a
    /// subdirectory.
    pub fn ensure(&self, id: ProjectId, root: &str) -> bool {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        match inner.entry(id) {
            std::collections::btree_map::Entry::Occupied(_) => false,
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(ProjectInfo {
                    id,
                    name: project_name(root),
                    root: root.to_string(),
                });
                true
            }
        }
    }

    /// Every project that still has a session in it, ordered by id so the
    /// sidebar is stable.
    ///
    /// `live` is the set of project ids the session manager currently holds.
    /// Anything outside it is dropped here rather than reported: this is the
    /// only reader, so a project with no sessions cannot reach a client no
    /// matter which code path asked. Pruning on read also means the map does
    /// not grow for the life of a long-running daemon.
    ///
    /// Note that a session whose CHILD has exited is still a session: it stays
    /// listed with its scrollback, so its id is still in `live` and its folder
    /// correctly stays on screen. Only removal from the manager retires a
    /// project.
    pub fn live(&self, live: &HashSet<ProjectId>) -> Vec<ProjectInfo> {
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        inner.retain(|id, _| live.contains(id));
        inner.values().cloned().collect()
    }
}

/// A display name for a project root.
///
/// The last path component, because a sidebar row shows about twenty characters
/// and an absolute path is unreadable at that width. A root with no usable
/// component keeps the raw string rather than becoming blank.
fn project_name(root: &str) -> String {
    Path::new(root)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| root.to_string())
}

/// A project's LIFETIME, which is the whole of what this file gets wrong when
/// it gets anything wrong.
///
/// The registry shipped with `ensure` and `list` and no remover, so it only
/// ever grew. The operator-visible result was a window opening onto a folder
/// named after a repository nobody was working in, because a session had run
/// there hours earlier and the daemon had not been restarted since. The
/// question it produced was "why does it open with a vitrum folder by
/// default", and the answer the operator gave when told it was stale state
/// rather than invented state was that the distinction does not exist from
/// their side of the screen.
#[cfg(test)]
mod lifetime {
    use super::*;

    fn ids(r: &ProjectRegistry, live: &[u64]) -> Vec<u64> {
        let set: HashSet<ProjectId> = live.iter().copied().map(ProjectId).collect();
        r.live(&set).into_iter().map(|p| p.id.0).collect()
    }

    /// The bug, stated directly: a project must not outlive its last session.
    #[test]
    fn a_project_is_never_reported_once_its_last_session_is_gone() {
        let r = ProjectRegistry::default();
        assert!(r.ensure(ProjectId(1), "/src/vitrum"));
        assert!(r.ensure(ProjectId(2), "/src/veyyon"));
        assert_eq!(ids(&r, &[1, 2]), vec![1, 2]);
        assert_eq!(
            ids(&r, &[2]),
            vec![2],
            "the abandoned project is still in the sidebar"
        );
    }

    /// The last session leaving must produce an EMPTY sidebar.
    ///
    /// Not one stale folder, not a placeholder. This is the state a fresh
    /// window opens into once every session has been closed, and it has to be
    /// blank or the operator is looking at a project they cannot get rid of.
    #[test]
    fn a_daemon_with_no_sessions_offers_no_projects() {
        let r = ProjectRegistry::default();
        r.ensure(ProjectId(1), "/src/vitrum");
        r.ensure(ProjectId(2), "/src/veyyon");
        assert!(
            r.live(&HashSet::new()).is_empty(),
            "a daemon with no sessions still offers projects"
        );
    }

    /// Reading must PRUNE, not merely filter.
    ///
    /// If a dead entry stayed in the map, `ensure` would keep returning false
    /// for it, so when work resumed in that directory the project would never
    /// be re-announced and the new session would appear under nothing. This is
    /// the failure that makes "filter on read" and "prune on read" different
    /// designs rather than the same one.
    #[test]
    fn a_project_whose_sessions_left_can_be_registered_again() {
        let r = ProjectRegistry::default();
        assert!(r.ensure(ProjectId(1), "/src/vitrum"));
        assert!(r.live(&HashSet::new()).is_empty());
        assert!(
            r.ensure(ProjectId(1), "/src/vitrum"),
            "re-registering after the project emptied must announce it again"
        );
        assert_eq!(ids(&r, &[1]), vec![1]);
    }

    /// A live session's project must survive a read that prunes another.
    ///
    /// The pruning writer touches the whole map, so an off-by-one in the
    /// predicate takes out every folder on screen at once rather than the one
    /// that emptied.
    #[test]
    fn pruning_one_project_leaves_the_others_alone() {
        let r = ProjectRegistry::default();
        for i in 1..=4 {
            r.ensure(ProjectId(i), &format!("/src/p{i}"));
        }
        assert_eq!(ids(&r, &[1, 3, 4]), vec![1, 3, 4]);
        assert_eq!(ids(&r, &[1, 3, 4]), vec![1, 3, 4], "the read is idempotent");
    }

    /// An id the registry never recorded must not conjure a project.
    ///
    /// `live` is built from the session manager, which can legitimately hold a
    /// session whose project was never registered. Reporting a folder for it
    /// would mean inventing a name and a root, which is the one thing this
    /// file must never do.
    #[test]
    fn a_live_id_with_no_record_produces_no_folder() {
        let r = ProjectRegistry::default();
        r.ensure(ProjectId(1), "/src/vitrum");
        assert_eq!(ids(&r, &[1, 99]), vec![1]);
    }
}
