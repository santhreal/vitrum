//! Workspaces, folders, and which workspace each session is filed into.
//!
//! The top-level partition of the sidebar, and the half of the client model
//! that is authored by the operator rather than reported by the daemon. It
//! knows nothing about connections, windows or rendering: everything here is
//! plain data plus the rules that keep a placement consistent, which is what
//! lets [`WorkspaceSet::normalize`] repair a hand-edited file without needing
//! a running client to ask.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use vitrum_model::Disposition;
use vitrum_proto::{SessionId, SessionInfo};


/// Identifier for a workspace.
///
/// Starts at one. Zero is never minted, so a zero read out of a corrupt file
/// or an uninitialised field is detectable rather than silently valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(pub u64);

/// Identifier for a user-created folder.
///
/// Unique across every workspace, not per workspace. Folder ids key this
/// window's collapse and section state, and per-workspace numbering would make
/// folder 1 of workspace A share its expanded/collapsed bit with folder 1 of
/// workspace B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FolderId(pub u64);

/// The workspace an install starts with, and the one a restore falls back to.
pub const DEFAULT_WORKSPACE: WorkspaceId = WorkspaceId(1);

/// Drawn for a workspace whose name is empty.
///
/// The first workspace is not one the operator picked, it is the one that had
/// to exist. Seeding it with a name states a decision nobody made, so it is
/// created nameless and shows the bare noun until it is renamed.
pub const UNNAMED_WORKSPACE_LABEL: &str = "Workspace";

/// A session identity that survives a daemon restart being wrong about ids.
///
/// Workspace membership is persisted to disk, so it outlives the daemon that
/// minted the ids in it. A daemon that restarts and hands out `SessionId(3)`
/// to a completely different session would inherit the old session's
/// workspace, which files a row somewhere the operator never put it. Pairing
/// the id with the creation stamp the daemon already reports makes that
/// impossible: the pair is unique for as long as the placement is worth
/// keeping, and a mismatched pair simply reads as "not placed yet".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionKey {
    pub id: SessionId,
    pub created_at_ms: u64,
}

impl SessionKey {
    pub fn of(info: &SessionInfo) -> Self {
        SessionKey {
            id: info.id,
            created_at_ms: info.created_at_ms,
        }
    }
}

/// How one workspace buckets its sessions in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Grouping {
    /// By the directory the session runs in.
    ///
    /// A session whose cwd sits under a project root the daemon knows buckets
    /// under that project, keeping the daemon's own name for it. Everything
    /// else buckets by its literal cwd, one bucket per distinct directory,
    /// rather than collapsing into one anonymous lump.
    #[default]
    Directory,
    /// By the folders the user made in this workspace, in the order they put
    /// them in. Sessions in no folder land in one Unfiled bucket.
    Named,
}

impl Grouping {
    pub fn label(self) -> &'static str {
        match self {
            Grouping::Directory => "Filesystem directory",
            Grouping::Named => "Named folders",
        }
    }
}

/// Which disposition bands this workspace shows.
///
/// Keyed on [`Disposition`] rather than [`vitrum_model::Section`] because the operator's
/// four names are the four dispositions: `Woke` is a band member rather than a
/// band, and someone who wants woken rows out of the way should not have to
/// hide the entire inbox to get it.
///
/// There is deliberately no guard against turning all four off. The settings
/// modal that set them is one click from the sidebar header in every state,
/// including the empty one, so an all-off sidebar is recoverable by the same
/// gesture that caused it; a refusal here would instead be a checkbox that
/// silently does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionVisibility {
    pub active: bool,
    pub woke: bool,
    pub snoozed: bool,
    pub settled: bool,
}

impl Default for SectionVisibility {
    fn default() -> Self {
        SectionVisibility {
            active: true,
            woke: true,
            snoozed: true,
            settled: true,
        }
    }
}

impl SectionVisibility {
    /// Does a row with this disposition appear at all?
    pub fn shows(&self, disposition: Disposition) -> bool {
        match disposition {
            Disposition::Active => self.active,
            Disposition::Woke => self.woke,
            Disposition::Snoozed => self.snoozed,
            Disposition::Settled => self.settled,
        }
    }

    pub fn set(&mut self, disposition: Disposition, on: bool) {
        match disposition {
            Disposition::Active => self.active = on,
            Disposition::Woke => self.woke = on,
            Disposition::Snoozed => self.snoozed = on,
            Disposition::Settled => self.settled = on,
        }
    }

    /// How many of the four are off, for a settings summary line.
    pub fn hidden_count(&self) -> usize {
        [self.active, self.woke, self.snoozed, self.settled]
            .into_iter()
            .filter(|on| !on)
            .count()
    }
}

/// One user-created folder inside a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: FolderId,
    pub name: String,
}

/// A completely separate top-level context.
///
/// Not a project and not a tab strip. Creating one gives a blank sidebar; the
/// operator then moves sessions into it. Switching a window to it swaps the
/// entire sidebar, and the window's tab strip with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub grouping: Grouping,
    pub sections: SectionVisibility,
    folders: Vec<Folder>,
    #[serde(with = "session_map", default)]
    folder_of: BTreeMap<SessionKey, FolderId>,
}

impl Workspace {
    fn new(id: WorkspaceId, name: String) -> Self {
        Workspace {
            id,
            name,
            grouping: Grouping::default(),
            sections: SectionVisibility::default(),
            folders: Vec::new(),
            folder_of: BTreeMap::new(),
        }
    }

    /// The word to draw for this workspace.
    ///
    /// The workspace an install starts with has no name, because nobody chose
    /// one for it. Rather than seed a decision the operator never made, it
    /// draws the bare noun until they rename it.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            UNNAMED_WORKSPACE_LABEL
        } else {
            &self.name
        }
    }

    /// This workspace's folders, in display order.
    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    /// Which folder a session sits in, or `None` for Unfiled.
    pub fn folder_of(&self, info: &SessionInfo) -> Option<FolderId> {
        self.folder_of.get(&SessionKey::of(info)).copied()
    }
}

/// Why a workspace or folder operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    /// No workspace with that id.
    Unknown,
    /// No folder with that id in the named workspace.
    UnknownFolder,
    /// A name that is empty once trimmed. An unnamed workspace is
    /// unreachable: the switcher has nothing to draw and nothing to click.
    BlankName,
    /// Refused: the workspace still holds sessions.
    ///
    /// The guard exists because deleting a workspace with rows in it either
    /// destroys the operator's filing silently or moves rows somewhere they
    /// never asked for. Both are worse than saying no and making them empty it.
    NotEmpty { sessions: usize },
    /// Refused: it is the only workspace left, and zero workspaces has no
    /// coherent sidebar.
    LastWorkspace,
    /// A reorder target past the end of the list.
    OutOfRange,
    /// The id counter is at the top of its range, which only a hand-edited
    /// file can produce. Refused rather than minting a duplicate id, because a
    /// duplicate merges two workspaces on the next repair.
    Exhausted,
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkspaceError::Unknown => f.write_str("no such workspace"),
            WorkspaceError::UnknownFolder => f.write_str("no such folder"),
            WorkspaceError::BlankName => f.write_str("a name is required"),
            WorkspaceError::NotEmpty { sessions } => write!(
                f,
                "still holds {sessions} session{}; move them out first",
                if *sessions == 1 { "" } else { "s" }
            ),
            WorkspaceError::LastWorkspace => f.write_str("the last workspace cannot be deleted"),
            WorkspaceError::OutOfRange => f.write_str("position is past the end of the list"),
            WorkspaceError::Exhausted => f.write_str(
                "no identifiers left; the saved file names one at the top of the range",
            ),
        }
    }
}

impl core::error::Error for WorkspaceError {}

/// Every workspace, their folders, and which workspace each session is in.
///
/// Daemon-scoped: one per connection, shared by every window. Two windows may
/// look at different workspaces, but they cannot disagree about which
/// workspace a session is IN, because a session belongs to exactly one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSet {
    /// Workspaces in display order. Never empty.
    list: Vec<Workspace>,
    #[serde(with = "session_map", default)]
    home: BTreeMap<SessionKey, WorkspaceId>,
    /// Where a session the client has never seen before is filed.
    intake: WorkspaceId,
    next_workspace: u64,
    next_folder: u64,
}

impl Default for WorkspaceSet {
    fn default() -> Self {
        WorkspaceSet {
            list: vec![Workspace::new(DEFAULT_WORKSPACE, String::new())],
            home: BTreeMap::new(),
            intake: DEFAULT_WORKSPACE,
            next_workspace: DEFAULT_WORKSPACE.0 + 1,
            next_folder: 1,
        }
    }
}

impl WorkspaceSet {
    /// Workspaces in display order.
    pub fn iter(&self) -> impl Iterator<Item = &Workspace> {
        self.list.iter()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn get(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.list.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.list.iter_mut().find(|w| w.id == id)
    }

    pub fn contains(&self, id: WorkspaceId) -> bool {
        self.list.iter().any(|w| w.id == id)
    }

    /// The first workspace in display order. The fallback for a window whose
    /// workspace was deleted underneath it.
    pub fn first(&self) -> WorkspaceId {
        self.list.first().map(|w| w.id).unwrap_or(DEFAULT_WORKSPACE)
    }

    pub fn position(&self, id: WorkspaceId) -> Option<usize> {
        self.list.iter().position(|w| w.id == id)
    }

    /// A name for the next workspace that is not already taken.
    ///
    /// The model owns this because uniqueness is a fact about the set, and a
    /// caller counting its own clicks cannot know it. Doing it at the call
    /// site produced "Workspace 17": a counter appended to a name that already
    /// had a counter in it, because the UI numbered its button presses instead
    /// of asking what existed.
    ///
    /// Numbering starts at 2, so the first workspace an operator makes beside
    /// the built-in one is "Workspace 2" and never "Workspace 1" sitting next
    /// to the nameless one that already draws as "Workspace".
    pub fn suggested_name(&self) -> String {
        let taken: BTreeSet<&str> = self.list.iter().map(|w| w.display_name()).collect();
        (2..)
            .map(|n| format!("Workspace {n}"))
            .find(|name| !taken.contains(name.as_str()))
            .unwrap_or_else(|| UNNAMED_WORKSPACE_LABEL.to_string())
    }

    /// Add an empty workspace at the end of the list.
    pub fn create(&mut self, name: &str) -> Result<WorkspaceId, WorkspaceError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(WorkspaceError::BlankName);
        }
        let id = WorkspaceId(self.next_workspace);
        // A hand-edited `ui.json` can leave the counter at the top of the
        // range. Minting a duplicate would silently merge two workspaces on
        // the next `normalize`, and `+= 1` past it would panic in a debug
        // build, so an exhausted counter is refused by name instead.
        if self.contains(id) {
            return Err(WorkspaceError::Exhausted);
        }
        self.next_workspace = self.next_workspace.saturating_add(1);
        self.list.push(Workspace::new(id, name.to_string()));
        Ok(id)
    }

    pub fn rename(&mut self, id: WorkspaceId, name: &str) -> Result<(), WorkspaceError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(WorkspaceError::BlankName);
        }
        let ws = self.get_mut(id).ok_or(WorkspaceError::Unknown)?;
        ws.name = name.to_string();
        Ok(())
    }

    /// Remove a workspace, refusing while it still holds sessions.
    pub fn delete(&mut self, id: WorkspaceId) -> Result<(), WorkspaceError> {
        let at = self.position(id).ok_or(WorkspaceError::Unknown)?;
        if self.list.len() == 1 {
            return Err(WorkspaceError::LastWorkspace);
        }
        let sessions = self.home.values().filter(|w| **w == id).count();
        if sessions > 0 {
            return Err(WorkspaceError::NotEmpty { sessions });
        }
        self.list.remove(at);
        // Intake must always name a workspace that exists, or the next session
        // the daemon reports has nowhere to land.
        if self.intake == id {
            self.intake = self.first();
        }
        Ok(())
    }

    /// Move a workspace to position `to` in the display order.
    pub fn move_to(&mut self, id: WorkspaceId, to: usize) -> Result<(), WorkspaceError> {
        let from = self.position(id).ok_or(WorkspaceError::Unknown)?;
        if to >= self.list.len() {
            return Err(WorkspaceError::OutOfRange);
        }
        let ws = self.list.remove(from);
        self.list.insert(to, ws);
        Ok(())
    }

    /// Which workspace this session is in.
    ///
    /// Falls back to [`WorkspaceSet::intake`] for a session no one has filed
    /// yet, which is the state between the daemon reporting it and
    /// [`WorkspaceSet::adopt`] running over the snapshot.
    pub fn workspace_of(&self, info: &SessionInfo) -> WorkspaceId {
        self.workspace_of_key(SessionKey::of(info))
    }

    /// [`WorkspaceSet::workspace_of`] for a caller that already holds the key.
    ///
    /// The key is the whole identity a placement has, so the paths that move
    /// rows in bulk take it directly rather than cloning a `SessionInfo` (four
    /// strings and a vector) per row to satisfy the borrow checker.
    pub fn workspace_of_key(&self, key: SessionKey) -> WorkspaceId {
        self.home
            .get(&key)
            .copied()
            .filter(|id| self.contains(*id))
            .unwrap_or(self.intake)
    }

    /// File a session into a workspace.
    pub fn assign(&mut self, info: &SessionInfo, to: WorkspaceId) -> Result<(), WorkspaceError> {
        self.assign_key(SessionKey::of(info), to)
    }

    /// [`WorkspaceSet::assign`] for a caller that already holds the key.
    pub fn assign_key(&mut self, key: SessionKey, to: WorkspaceId) -> Result<(), WorkspaceError> {
        if !self.contains(to) {
            return Err(WorkspaceError::Unknown);
        }
        // A session that leaves a workspace leaves its folders too. Folder ids
        // are unique across the set, so a stale entry would otherwise keep the
        // row filed under a folder its new workspace does not own.
        for ws in &mut self.list {
            ws.folder_of.remove(&key);
        }
        self.home.insert(key, to);
        Ok(())
    }

    /// Where a session nobody has filed lands.
    pub fn intake(&self) -> WorkspaceId {
        self.intake
    }

    /// Point intake at a workspace.
    ///
    /// A window does this when it switches workspace, so a session launched
    /// next appears in the workspace the operator is looking at rather than in
    /// whichever one happens to be first. With two windows the last one to
    /// switch wins, which is the same "most recent intent" rule every other
    /// last-write-wins field in this file uses.
    pub fn set_intake(&mut self, id: WorkspaceId) -> Result<(), WorkspaceError> {
        if !self.contains(id) {
            return Err(WorkspaceError::Unknown);
        }
        self.intake = id;
        Ok(())
    }

    /// Give every session that has no placement the intake workspace.
    ///
    /// Called on each daemon snapshot. Without it, unplaced sessions would
    /// follow intake around: switching a window to a new workspace would drag
    /// every unfiled session with it, which is the opposite of a blank sidebar.
    pub fn adopt<'a>(&mut self, sessions: impl IntoIterator<Item = &'a SessionInfo>) {
        let intake = self.intake;
        for info in sessions {
            self.home.entry(SessionKey::of(info)).or_insert(intake);
        }
    }

    /// Drop placements for sessions the daemon no longer lists.
    pub fn retain_sessions(&mut self, live: &BTreeSet<SessionKey>) {
        self.home.retain(|key, _| live.contains(key));
        for ws in &mut self.list {
            ws.folder_of.retain(|key, _| live.contains(key));
        }
    }

    /// Drop every placement for one session the daemon has stopped listing.
    ///
    /// The targeted counterpart of [`WorkspaceSet::retain_sessions`], for the
    /// single-removal path: one map removal per workspace plus one from `home`,
    /// rather than a full walk of every placement map against a freshly built
    /// set of every surviving key.
    pub fn forget_session(&mut self, key: SessionKey) {
        self.home.remove(&key);
        for ws in &mut self.list {
            ws.folder_of.remove(&key);
        }
    }

    /// How many sessions are filed into this workspace.
    pub fn session_count(&self, id: WorkspaceId) -> usize {
        self.home.values().filter(|w| **w == id).count()
    }

    /// Add a folder to a workspace, at the end of its folder list.
    pub fn create_folder(
        &mut self,
        workspace: WorkspaceId,
        name: &str,
    ) -> Result<FolderId, WorkspaceError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(WorkspaceError::BlankName);
        }
        if !self.contains(workspace) {
            return Err(WorkspaceError::Unknown);
        }
        let id = FolderId(self.next_folder);
        // Same exhausted-counter guard as `create`, and for the same reason: a
        // duplicate folder id would file rows under a folder in the wrong
        // workspace, because folder ids are unique across the whole set.
        if self.list.iter().any(|ws| ws.folders.iter().any(|f| f.id == id)) {
            return Err(WorkspaceError::Exhausted);
        }
        self.next_folder = self.next_folder.saturating_add(1);
        let ws = self.get_mut(workspace).ok_or(WorkspaceError::Unknown)?;
        ws.folders.push(Folder {
            id,
            name: name.to_string(),
        });
        Ok(id)
    }

    pub fn rename_folder(
        &mut self,
        workspace: WorkspaceId,
        folder: FolderId,
        name: &str,
    ) -> Result<(), WorkspaceError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(WorkspaceError::BlankName);
        }
        let ws = self.get_mut(workspace).ok_or(WorkspaceError::Unknown)?;
        let f = ws
            .folders
            .iter_mut()
            .find(|f| f.id == folder)
            .ok_or(WorkspaceError::UnknownFolder)?;
        f.name = name.to_string();
        Ok(())
    }

    /// Remove a folder. Its sessions become Unfiled rather than disappearing,
    /// so there is no guard: nothing can be lost.
    pub fn delete_folder(
        &mut self,
        workspace: WorkspaceId,
        folder: FolderId,
    ) -> Result<(), WorkspaceError> {
        let ws = self.get_mut(workspace).ok_or(WorkspaceError::Unknown)?;
        let at = ws
            .folders
            .iter()
            .position(|f| f.id == folder)
            .ok_or(WorkspaceError::UnknownFolder)?;
        ws.folders.remove(at);
        ws.folder_of.retain(|_, f| *f != folder);
        Ok(())
    }

    pub fn move_folder(
        &mut self,
        workspace: WorkspaceId,
        folder: FolderId,
        to: usize,
    ) -> Result<(), WorkspaceError> {
        let ws = self.get_mut(workspace).ok_or(WorkspaceError::Unknown)?;
        let from = ws
            .folders
            .iter()
            .position(|f| f.id == folder)
            .ok_or(WorkspaceError::UnknownFolder)?;
        if to >= ws.folders.len() {
            return Err(WorkspaceError::OutOfRange);
        }
        let f = ws.folders.remove(from);
        ws.folders.insert(to, f);
        Ok(())
    }

    /// File a session into a folder of the workspace it already lives in, or
    /// out of every folder when `folder` is `None`.
    pub fn assign_folder(
        &mut self,
        info: &SessionInfo,
        folder: Option<FolderId>,
    ) -> Result<(), WorkspaceError> {
        self.assign_folder_key(SessionKey::of(info), folder)
    }

    /// [`WorkspaceSet::assign_folder`] for a caller that already holds the key.
    pub fn assign_folder_key(
        &mut self,
        key: SessionKey,
        folder: Option<FolderId>,
    ) -> Result<(), WorkspaceError> {
        let home = self.workspace_of_key(key);
        let ws = self.get_mut(home).ok_or(WorkspaceError::Unknown)?;
        match folder {
            Some(id) => {
                if !ws.folders.iter().any(|f| f.id == id) {
                    return Err(WorkspaceError::UnknownFolder);
                }
                ws.folder_of.insert(key, id);
            }
            None => {
                ws.folder_of.remove(&key);
            }
        }
        Ok(())
    }

    /// Repair a set read back from disk.
    ///
    /// A hand-edited or half-written file can name a workspace twice, hold
    /// zero workspaces, point intake at nothing, or file a session into a
    /// workspace that is gone. Every one of those has a defined repair, and
    /// applying them is strictly better than either trusting the file or
    /// throwing the operator's filing away over one bad field.
    pub fn normalize(&mut self) {
        let mut seen: BTreeSet<WorkspaceId> = BTreeSet::new();
        self.list.retain(|w| w.id.0 != 0 && seen.insert(w.id));
        if self.list.is_empty() {
            *self = WorkspaceSet::default();
            return;
        }
        let live: BTreeSet<WorkspaceId> = self.list.iter().map(|w| w.id).collect();
        if !live.contains(&self.intake) {
            self.intake = self.list[0].id;
        }
        self.home.retain(|_, id| live.contains(id));
        let highest = self.list.iter().map(|w| w.id.0).max().unwrap_or(0);
        // Saturating: a hand-edited file can name workspace `u64::MAX`, and
        // `+ 1` on it is a panic in a debug build. `create` refuses an
        // exhausted counter rather than minting a duplicate.
        self.next_workspace = self.next_workspace.max(highest.saturating_add(1));

        let mut highest_folder = 0;
        for ws in &mut self.list {
            let mut seen_folders: BTreeSet<FolderId> = BTreeSet::new();
            ws.folders
                .retain(|f| f.id.0 != 0 && seen_folders.insert(f.id));
            highest_folder =
                highest_folder.max(seen_folders.iter().map(|f| f.0).max().unwrap_or(0));
            ws.folder_of.retain(|_, f| seen_folders.contains(f));
        }
        self.next_folder = self.next_folder.max(highest_folder.saturating_add(1));

        // A session filed into a folder of a workspace it does not live in is
        // the one inconsistency `assign` cannot produce and a file can.
        //
        // Answered straight out of `home` through a disjoint field borrow. The
        // previous form materialised every placement into a `Vec` and then
        // linear-scanned it once per filed session, which is one allocation
        // plus O(filed x placed) work on a path that runs on every load.
        let WorkspaceSet { list, home, .. } = self;
        for ws in list {
            let id = ws.id;
            ws.folder_of
                .retain(|key, _| home.get(key).is_none_or(|placed| *placed == id));
        }
    }
}

/// `BTreeMap<SessionKey, V>` as a list of pairs.
///
/// JSON object keys must be strings and [`SessionKey`] is a struct, so the
/// derive cannot round-trip these maps. A list of pairs is the honest wire
/// form; encoding the key as `"3:1772580600000"` would work too and would put
/// a second parser in the file for no gain.
mod session_map {
    use super::SessionKey;
    use serde::de::Deserialize;
    use serde::ser::{Serialize, SerializeSeq};
    use std::collections::BTreeMap;

    pub fn serialize<S, V>(map: &BTreeMap<SessionKey, V>, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
        V: Serialize,
    {
        let mut seq = s.serialize_seq(Some(map.len()))?;
        for pair in map {
            seq.serialize_element(&pair)?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D, V>(d: D) -> Result<BTreeMap<SessionKey, V>, D::Error>
    where
        D: serde::Deserializer<'de>,
        V: Deserialize<'de>,
    {
        let pairs: Vec<(SessionKey, V)> = Vec::deserialize(d)?;
        Ok(pairs.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::info;

    fn ws_ids(set: &WorkspaceSet) -> Vec<WorkspaceId> {
        set.iter().map(|w| w.id).collect()
    }

    /// A hand-edited or half-written file must be repaired, not trusted and
    /// not thrown away. Every one of these has a defined answer, and refusing
    /// the whole document over one bad field loses the operator's filing.
    #[test]
    fn a_damaged_workspace_set_is_repaired_rather_than_trusted() {
        let mut set = WorkspaceSet::default();
        let b = set.create("B").unwrap();
        let folder = set.create_folder(b, "F").unwrap();
        let info = info(10);
        set.assign(&info, b).unwrap();
        set.assign_folder(&info, Some(folder)).unwrap();

        // The workspace holding that session disappears, as a truncated write
        // or a hand edit can do.
        set.list.retain(|w| w.id != b);
        set.intake = b;
        set.normalize();

        assert_eq!(ws_ids(&set), vec![DEFAULT_WORKSPACE]);
        assert_eq!(
            set.intake(),
            DEFAULT_WORKSPACE,
            "intake must name something real"
        );
        assert_eq!(
            set.workspace_of(&info),
            DEFAULT_WORKSPACE,
            "a session filed into a workspace that is gone falls back to intake"
        );
        assert!(
            set.next_workspace > b.0,
            "ids must not be reissued over the hole the delete left"
        );
        assert!(set.next_folder > folder.0);

        // Every workspace gone at once: rebuild rather than render nothing.
        set.list.clear();
        set.normalize();
        assert_eq!(set.len(), 1);
        assert_eq!(ws_ids(&set), vec![DEFAULT_WORKSPACE]);
    }

    /// A duplicated workspace id would give one workspace two entries in the
    /// switcher and make `get` non-deterministic.
    #[test]
    fn normalize_drops_duplicate_and_zero_ids() {
        let mut set = WorkspaceSet::default();
        let dup = set.list[0].clone();
        set.list.push(dup);
        set.list.push(Workspace::new(WorkspaceId(0), "zero".into()));
        set.normalize();
        assert_eq!(ws_ids(&set), vec![DEFAULT_WORKSPACE]);
    }

    /// WHY: `normalize` raised its id counters with `highest + 1`, which is an
    /// overflow panic in a debug build the moment a `ui.json` names workspace
    /// or folder `u64::MAX`. Repairing a corrupt file is the one path whose
    /// entire job is to survive nonsense, and it took the window down instead.
    ///
    /// The saturating counter then has to refuse rather than reissue: minting
    /// `u64::MAX` a second time gives two workspaces one id, and the very next
    /// `normalize` deletes one of them along with everything filed in it.
    #[test]
    fn a_file_naming_the_last_possible_id_is_repaired_rather_than_fatal() {
        let mut set = WorkspaceSet::default();
        set.list.push(Workspace::new(WorkspaceId(u64::MAX), "top".into()));
        set.normalize();

        assert_eq!(
            ws_ids(&set),
            vec![DEFAULT_WORKSPACE, WorkspaceId(u64::MAX)],
            "both workspaces are real and neither is discarded"
        );
        assert_eq!(set.next_workspace, u64::MAX, "the counter saturates");
        assert_eq!(
            set.create("Another"),
            Err(WorkspaceError::Exhausted),
            "an exhausted counter refuses rather than reissuing a live id"
        );
        assert_eq!(set.len(), 2, "the refusal left the list alone");
        assert_eq!(
            WorkspaceError::Exhausted.to_string(),
            "no identifiers left; the saved file names one at the top of the range"
        );
    }

    /// WHY: the same `+ 1` overflow on the folder counter, which `normalize`
    /// raises from the highest folder id in ANY workspace. Folder ids are
    /// unique across the whole set, so a reissued one files rows into a folder
    /// belonging to a workspace they do not live in.
    #[test]
    fn a_folder_at_the_last_possible_id_is_repaired_rather_than_fatal() {
        let mut set = WorkspaceSet::default();
        set.list[0].folders.push(Folder {
            id: FolderId(u64::MAX),
            name: "top".to_string(),
        });
        set.normalize();

        assert_eq!(set.next_folder, u64::MAX, "the counter saturates");
        assert_eq!(
            set.create_folder(DEFAULT_WORKSPACE, "Another"),
            Err(WorkspaceError::Exhausted)
        );
        assert_eq!(
            set.get(DEFAULT_WORKSPACE).unwrap().folders().len(),
            1,
            "the refusal left the folder list alone"
        );
    }

    /// WHY: `normalize` drops a folder placement whose session lives in
    /// another workspace, which is the one inconsistency `assign` cannot
    /// produce and a hand-edited file can. It used to answer that by
    /// materialising every placement into a `Vec` and linear-scanning it per
    /// filed session; this pins the behaviour across the rewrite to a direct
    /// `home` lookup, including the case the scan got right by accident: a
    /// session with NO placement at all keeps its folder, because `home` falls
    /// back to intake and intake is where it draws.
    #[test]
    fn a_folder_placement_in_the_wrong_workspace_is_dropped_and_a_right_one_kept() {
        let mut set = WorkspaceSet::default();
        let other = set.create("Other").unwrap();
        let here = set.create_folder(DEFAULT_WORKSPACE, "Here").unwrap();
        let there = set.create_folder(other, "There").unwrap();

        let stays = info(10);
        set.assign(&stays, DEFAULT_WORKSPACE).unwrap();
        set.assign_folder(&stays, Some(here)).unwrap();

        // Filed into `other`'s folder while `home` says the default workspace.
        // Only a file can say this, so it is written directly.
        let strays = info(11);
        set.assign(&strays, DEFAULT_WORKSPACE).unwrap();
        set.get_mut(other)
            .unwrap()
            .folder_of
            .insert(SessionKey::of(&strays), there);

        // Never placed at all: `home` has no entry, so nothing contradicts the
        // folder and it must survive.
        let unplaced = info(12);
        set.get_mut(DEFAULT_WORKSPACE)
            .unwrap()
            .folder_of
            .insert(SessionKey::of(&unplaced), here);

        set.normalize();

        assert_eq!(
            set.get(DEFAULT_WORKSPACE).unwrap().folder_of(&stays),
            Some(here)
        );
        assert_eq!(
            set.get(other).unwrap().folder_of(&strays),
            None,
            "a folder of a workspace the session does not live in is not a placement"
        );
        assert_eq!(
            set.get(DEFAULT_WORKSPACE).unwrap().folder_of(&unplaced),
            Some(here),
            "an unplaced session contradicts nothing, so its folder stands"
        );
    }

    /// WHY: `SessionRemoved` used to drop one session's placement by rebuilding
    /// a set of every surviving key and re-`retain`ing every placement map
    /// against it. `forget_session` replaces that with two targeted removals,
    /// and this pins what must not change: the removed session loses both its
    /// workspace placement and its folder placement, and no other session does.
    #[test]
    fn forgetting_one_session_leaves_every_other_placement_alone() {
        let mut set = WorkspaceSet::default();
        let other = set.create("Other").unwrap();
        let folder = set.create_folder(other, "F").unwrap();

        let gone = info(10);
        let kept = info(11);
        for who in [&gone, &kept] {
            set.assign(who, other).unwrap();
            set.assign_folder(who, Some(folder)).unwrap();
        }
        assert_eq!(set.session_count(other), 2);

        set.forget_session(SessionKey::of(&gone));

        assert_eq!(set.session_count(other), 1, "the placement went with the row");
        assert_eq!(
            set.workspace_of(&gone),
            set.intake(),
            "an unplaced session falls back to intake"
        );
        assert_eq!(set.get(other).unwrap().folder_of(&gone), None);
        assert_eq!(set.get(other).unwrap().folder_of(&kept), Some(folder));
        assert_eq!(set.workspace_of(&kept), other);
    }
}
