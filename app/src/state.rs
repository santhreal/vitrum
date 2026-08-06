//! The whole client-side model, and the pure folds that move it.
//!
//! Everything here is deliberately free of Dioxus and of the JS bridge so it
//! can be tested as plain data. There is no scrollback here at all: scrollback
//! lives on the server, and the only bytes this process ever holds are the
//! ones currently on a terminal grid.
//!
//! # The split, and why it is two types and not one
//!
//! Client state comes in exactly two kinds, and they are two different types
//! so that which kind you are touching is visible at every call site:
//!
//! - [`DaemonState`] is everything one daemon connection knows: the projects,
//!   the sessions, the operator's rulings about those sessions, the
//!   [`WorkspaceSet`] they are filed into, and the [`Settings`]. One value per
//!   daemon, however many windows are open.
//! - [`WindowState`] is one window's view onto it: which workspace it is
//!   looking at, its tab strip, its focus, its filter, its selection, its one
//!   open layer. One value per window, and two windows may legitimately
//!   disagree about every field.
//!
//! [`UiState`] is the pair, and it is what one window renders from. Writing
//! `st.daemon.sessions` and `st.window.filter` at the call site is the point:
//! the first is agreed across every window and the second is this window's
//! alone, and a reader should never have to look it up.
//!
//! ## Where the shared half actually lives
//!
//! The split is designed so a [`WindowState`] never owns session data and
//! never reaches for a copy: every derivation takes `&DaemonState` or
//! `&mut DaemonState`, so N windows can read one value. The client does not
//! currently exercise that, and the reason is Dioxus rather than this file. A
//! `Signal<UiState>` belongs to exactly one VirtualDom and each desktop window
//! gets its own, so window 2 cannot read window 1's signal at all. Each window
//! therefore holds a whole [`UiState`], keeps its own socket, and stays in
//! agreement because the daemon broadcasts every change to all of them: the
//! sharing happens over loopback instead of over a pointer, at the cost of one
//! fold per window per message and the benefit that a window that dies cannot
//! corrupt another.
//!
//! That does not make the borrow-shaped API decoration. It is what makes the
//! two halves separately testable — every multi-window test in this file runs
//! two [`WindowState`]s against one [`DaemonState`] — and it is what a second
//! consumer in one address space would need. If the per-window socket ever
//! becomes the bottleneck, the sharing is a constructor change and nothing
//! else.
//!
//! # Which side the operator axis lands on
//!
//! `snooze`, `settle_override` and `last_visited_ms` live per row inside
//! [`SessionView`], and [`SessionView`] lives in [`DaemonState::sessions`].
//! They are therefore SHARED across windows, and that is a decision, not an
//! accident of where the vector happened to sit.
//!
//! The argument for shared: those three fields are operator INTENT, not
//! viewport. "I have dealt with this", "park this until 09:00", "I have looked
//! at this" are statements about the work, and the work does not have a
//! window. Snoozing a row in one window and having it still sitting in the
//! inbox shouting in the other is not two views of one truth, it is two
//! truths, and the operator has no way to tell which one the notification
//! badge, the tray count or the next Alt+J jump is counting. It also breaks
//! the one invariant that makes snooze safe: a parked row must come back
//! exactly once, at its wake time. Per-window snoozes give you N wakeups for
//! one park.
//!
//! The argument for per-window, stated fairly: these fields are already
//! client-local rather than daemon-owned, so there is no server to arbitrate,
//! and a second window is often opened precisely to hold a different working
//! set — a review window and a build window. On that reading a snooze is
//! "hide this from THIS list", which is a viewport statement, and the
//! per-window answer is the honest one.
//!
//! Shared wins because the per-window reading is already served, better, by
//! the thing this batch adds: a [`Workspace`] is how you say "not in this
//! list", it is explicit, it is visible in the UI, and it does not silently
//! duplicate a wake timer. Giving the same job to two mechanisms, one of them
//! invisible, is how an operator loses a row.
//!
//! # What this file does NOT decide
//!
//! Status, disposition, sectioning, ordering, rollups and wake labels all come
//! from [`vitrum_model`]. Anywhere a rule about "is this row done" could be
//! written here, it is a call into the model instead: two implementations of
//! that question disagree the first time a snooze elapses, and the
//! disagreement is invisible until an operator loses a row.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vitrum_model::{
    Clock, Disposition, DispositionPolicy, Section, Selection, SelectionFacts, SessionView,
    SettleOverride, Snooze, SnoozePreset, SnoozePresetId,
    snooze::snooze_presets,
    traversal::{Direction, Wrap, adjacent_matching},
};
use vitrum_os::paths::{AppPaths, PathError};
use vitrum_proto::{
    Attention, IDLE_ATTENTION_MS, PROTOCOL_VERSION, ProjectId, ProjectInfo, SearchHit, ServerMsg,
    SessionId, SessionInfo, SessionStatus,
};

use crate::inbox::{self, Group};

/// Prefix the server uses on [`ServerMsg::Error`] when a client fell behind the
/// live stream. Agreed with the server: it never silently splices, it reports
/// the gap so the client can re-request the range.
pub const GAP_ERROR_PREFIX: &str = "output gap:";

/// Narrowest the sidebar may be dragged, in CSS pixels.
///
/// Matches `--rg-sidebar-width-min` (14rem) in `sidebar.css`. Below this the
/// title box on a dense row collapses to a bare ellipsis, so the floor is
/// legibility rather than layout. The stylesheet clamps too; keeping the
/// number here honest stops a drag accumulating width the element will never
/// take, which the user would then have to drag back through.
pub const SIDEBAR_MIN_PX: f64 = 224.0;

/// Widest the sidebar may be dragged, matching `--rg-sidebar-width-max` (28rem).
pub const SIDEBAR_MAX_PX: f64 = 448.0;

/// Most tabs the strip will hold at once.
///
/// The strip is a bounded most-recently-used window onto the session set, not
/// a row per session. At the stated load of twenty concurrent agents a tab per
/// session is unreadable however it scrolls: twenty tabs across a 1024px strip
/// is 51px each, which fits a status dot and nothing else. Eight get 117px,
/// enough for a dot, a truncated title and a close button.
///
/// Eight is a window, not a limit on how many sessions you can reach. Every
/// session outside the strip is one click away in two places: the sidebar,
/// which is the primary switcher and is already grouped and ordered by who
/// needs a human, and the strip's own overflow button, which lists exactly the
/// sessions the strip could not hold. Nothing is ever hidden without a count
/// saying how much.
pub const MAX_TABS: usize = 8;

/// Default sidebar width, matching `--rg-sidebar-width` (16rem).
pub const SIDEBAR_DEFAULT_PX: f64 = 256.0;

/// Largest share of the window the sidebar may take, as a fraction.
///
/// The absolute cap in [`SIDEBAR_MAX_PX`] is about legibility and knows
/// nothing about the window it is in. This is the other half:
/// [`WindowState::set_sidebar_width_in`] clamps against both, so a 448px
/// sidebar is fine on a 3840px display and refused on an 800px one.
pub const SIDEBAR_MAX_FRACTION: f64 = 0.32;

/// Where the control-plane connection stands, as the sidebar reports it.
///
/// A failed connection is a first-class state with its own banner. It is never
/// papered over with fixture data: [`ConnState::Fixture`] is only ever reached
/// by an explicit `--fixture` flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    /// Socket is opening, or was told to reopen. No `Welcome` yet.
    Connecting,
    /// `Welcome` received and the protocol version matched.
    Live { server_version: String },
    /// Socket refused, closed, or the protocol version mismatched.
    Failed { detail: String },
    /// `--fixture` was passed. No socket was ever opened.
    Fixture,
}

impl ConnState {
    /// Modifier class for the sidebar banner.
    pub fn banner_class(&self) -> &'static str {
        match self {
            ConnState::Connecting => "rg-sidebar__status rg-sidebar__status--connecting",
            ConnState::Live { .. } => "rg-sidebar__status rg-sidebar__status--ok",
            ConnState::Failed { .. } => "rg-sidebar__status rg-sidebar__status--failed",
            ConnState::Fixture => "rg-sidebar__status rg-sidebar__status--fixture",
        }
    }

    /// One-line banner text. Failures show the reason verbatim so a refused
    /// socket is distinguishable from a closed one at a glance.
    pub fn banner_text(&self, url: &str) -> String {
        match self {
            ConnState::Connecting => format!("connecting to {url}"),
            ConnState::Live { server_version } => format!("connected - server {server_version}"),
            ConnState::Failed { detail } => format!("disconnected - {detail}"),
            ConnState::Fixture => "FIXTURE DATA - no server connection".to_string(),
        }
    }

    /// True when a manual reconnect button should be offered.
    ///
    /// `Failed` is the terminal state: `sync.rs` backs off from 250ms to 30s
    /// over 25 attempts and then stops, because a loop that never gives up is
    /// a polling timer by another name and this process must be idle when
    /// nothing is happening. The button is what restarts it.
    pub fn is_retryable(&self) -> bool {
        matches!(self, ConnState::Failed { .. })
    }

    /// True when the daemon is actually answering.
    ///
    /// `Fixture` is not live: it never opened a socket, and a first-run sheet
    /// that congratulates you on a connection you do not have is worse than
    /// one that tells you to start the daemon.
    pub fn is_live(&self) -> bool {
        matches!(self, ConnState::Live { .. })
    }
}

/// What the caller must do after folding a [`ServerMsg`] into the state.
///
/// Returned rather than performed so the fold stays pure and testable; the
/// component turns these into bridge commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    /// Nothing beyond the state change.
    None,
    /// Paint `bytes` into the terminal, then resume live frames at `resume_seq`.
    ///
    /// `resume_seq` is the byte offset one past the last backfilled byte. Live
    /// frames already buffered are spliced against it by offset, which is the
    /// only correct way: `Attach` starts the live stream at the head as of the
    /// attach, and the backfill answer is computed at the head as of the
    /// scrollback request, so the two overlap by however many bytes the child
    /// emitted in between.
    ///
    /// `from_seq` is the offset of the first painted byte, and `jump_seq` an
    /// absolute offset the grid should scroll to once the paint lands. Both
    /// exist for search: a hit is a byte offset into the whole stream, and
    /// turning that into a row needs to know where the painted region starts.
    ///
    /// `keep_view` marks a page-back repaint, where the operator is reading a
    /// line and asked for history above it. Snapping to the bottom there
    /// throws away the position they were paging from.
    Backfill {
        session: SessionId,
        from_seq: u64,
        resume_seq: u64,
        bytes: Vec<u8>,
        jump_seq: Option<u64>,
        keep_view: bool,
        /// The daemon still holds bytes older than `from_seq`.
        more: bool,
    },
    /// The server reported an output gap. Repaint this session from scratch.
    Refill { session: SessionId },
}

// ═══════════════════════════════════════════════════════════════════════════
// Workspaces and settings
// ═══════════════════════════════════════════════════════════════════════════

/// The operator-authored half of the model: the workspace partition and the
/// folders inside it. Re-exported so `state::WorkspaceId` stays the one path
/// every caller spells.
mod workspace;
/// Preferences, and the pages of the settings modal.
mod settings;

// Glob, not a hand-maintained list: the split is an internal carve and every
// one of these names is spelled `state::Thing` at its call sites. A list would
// have to be edited every time either module grows a type, and would warn as
// unused for the ones only their own module names today.
pub use settings::*;
pub use workspace::*;

// ═══════════════════════════════════════════════════════════════════════════
// Daemon state
// ═══════════════════════════════════════════════════════════════════════════

/// What one folded [`ServerMsg`] leaves for the windows to decide.
///
/// The daemon fold happens once however many windows are open, so anything
/// window-shaped has to come back out rather than be applied in there. Passed
/// by reference to each window so the backfill bytes are cloned only by the
/// window that will actually paint them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Broadcast {
    /// Nothing for the windows.
    None,
    /// The session list was replaced wholesale. Every window must re-prune its
    /// tabs, focus and selection.
    SessionsChanged,
    /// History for `session`. The window focused on it paints and resumes.
    ///
    /// `from_seq` is the offset of the first byte, and `more` is the daemon
    /// saying it still holds older bytes than these. Both used to be dropped
    /// on the floor here, which is why the client could only ever show one
    /// head-anchored chunk: nothing knew where the painted region started, so
    /// nothing could ask for the region before it.
    Scrollback {
        session: SessionId,
        from_seq: u64,
        resume_seq: u64,
        bytes: Vec<u8>,
        more: bool,
    },
    /// An error to surface. Scoped to a session when the server named one.
    Error {
        session: Option<SessionId>,
        message: String,
    },
    /// The daemon answered a scrollback sweep.
    ///
    /// `truncated` means the hit cap stopped it, so the surface must say
    /// "first n" rather than imply these are all of them.
    SearchResults {
        pattern: String,
        hits: Vec<SearchHit>,
        truncated: bool,
        bytes_scanned: u64,
    },
}

/// The three client-local facts a daemon snapshot must not destroy: whether
/// the operator has a row parked, has ruled on it, and when they last looked.
type OperatorAxis = (Option<Snooze>, Option<SettleOverride>, Option<u64>);

/// A value derived from a [`DaemonState`], held beside the facts it comes from.
///
/// Deliberately invisible to `PartialEq` and to `Debug`. Two states that agree
/// about every fact are the same state whether or not either has been painted
/// yet, and a cache that counted towards identity would make `assert_eq!` on
/// two of them depend on which one had been rendered first.
///
/// `RefCell` because the whole sidebar derives from `&DaemonState` — N windows
/// read one value, which is the split this module opens with — so the fill has
/// to happen through a shared reference. It is never borrowed re-entrantly:
/// the one `borrow_mut` is in [`DaemonState::folded_projects`], which drops
/// its guard before handing out a shared one.
#[derive(Default)]
struct Cache<T>(core::cell::RefCell<T>);

impl<T: Clone> Clone for Cache<T> {
    fn clone(&self) -> Self {
        Cache(self.0.clone())
    }
}

impl<T> PartialEq for Cache<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<T> fmt::Debug for Cache<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cache(..)")
    }
}

/// One distinct canonical root out of the daemon's project list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FoldedGroup {
    /// The canonical directory, from [`inbox::project_key`].
    key: String,
    /// The id this directory always maps to, whatever ids the daemon was
    /// handed for it.
    id: ProjectId,
    /// Index into [`DaemonState::projects`] of the record the header takes its
    /// name and root from: the first one that named this directory.
    lead: usize,
}

/// The daemon's project list, folded by canonical root and kept.
///
/// [`inbox::coalesce_projects`] costs one `realpath` per project, and the
/// sidebar re-derives its whole tree on every paint. At twenty projects that
/// was twenty syscalls per paint per window for an answer that only moves when
/// the daemon sends a different project list.
///
/// Measured on this machine at the stated load of twenty sessions over eight
/// repositories: absorbing one `SessionUpdated` and running the derivation the
/// paint after it needs went from 0.051ms to 0.005ms for one window, and from
/// 0.888ms to 0.105ms across twenty.
/// `one_session_updated_is_absorbed_inside_a_frame` re-measures that pair on
/// every run and fails if the gap closes.
///
/// **Invalidation, in one sentence:** the fold is rebuilt whenever the ids or
/// roots of [`DaemonState::projects`] differ from the ones it was folded from.
///
/// The consequence that sentence carries, stated rather than buried: a root
/// that is MOVED or re-symlinked under a live client now re-keys when the
/// daemon next reports its projects, where before it re-keyed on the next
/// paint. Nothing else changes, because every other input to the fold is the
/// project list itself.
///
/// Names are deliberately not part of the key. [`FoldedGroup::lead`] is an
/// index, so a renamed project is read fresh out of `projects` on the next
/// paint without invalidating anything.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct FoldedProjects {
    /// `(id, root)` of the list this was folded from. Compared field by field
    /// rather than hashed: twenty short string comparisons cost a fraction of
    /// one `realpath` and leave no collision to reason about.
    from: Vec<(ProjectId, String)>,
    /// One entry per distinct canonical root, in the daemon's order of first
    /// appearance.
    groups: Vec<FoldedGroup>,
    /// Every daemon project id, ascending, paired with its group's index.
    index: Vec<(ProjectId, usize)>,
}

impl FoldedProjects {
    /// Was this folded from exactly `projects`?
    fn matches(&self, projects: &[ProjectInfo]) -> bool {
        self.from.len() == projects.len()
            && self
                .from
                .iter()
                .zip(projects)
                .all(|((id, root), p)| *id == p.id && root == &p.root)
    }

    /// Re-fold, through the one implementation of the question.
    fn rebuild(&mut self, projects: &[ProjectInfo]) {
        let folded = inbox::coalesce_projects(projects);
        self.from.clear();
        self.from
            .extend(projects.iter().map(|p| (p.id, p.root.clone())));
        self.groups.clear();
        self.groups
            .extend(folded.groups().iter().map(|group| FoldedGroup {
                key: group.key.clone(),
                id: group.id,
                lead: group.lead_at,
            }));
        self.index.clear();
        self.index.extend(
            projects
                .iter()
                .filter_map(|p| folded.group_of(p.id).map(|at| (p.id, at))),
        );
        self.index.sort_unstable_by_key(|(id, _)| *id);
    }

    /// Which group a daemon project id belongs to.
    fn group_of(&self, id: ProjectId) -> Option<usize> {
        self.index
            .binary_search_by_key(&id, |(id, _)| *id)
            .ok()
            .map(|at| self.index[at].1)
    }
}

/// Canonical directory keys for the cwds the project list does not cover.
///
/// [`inbox::project_key`] is a `realpath`, and [`WindowState::bucket_by_directory`]
/// calls it once per distinct cwd no project record claims — inside the
/// render, on every paint, in every window. Measured on this machine at the
/// stated load of twenty sessions over eight repositories with three detached
/// directories: 980ns per call and three distinct detached directories, which
/// was 2.9us of a 5.3us paint. **Fifty-five per cent of the sidebar's entire
/// derivation was one syscall asking the kernel a question whose answer had
/// not moved since the session started.**
///
/// **Invalidation, in one sentence:** the keys are rebuilt whenever the cwds
/// of [`DaemonState::sessions`] differ from the ones they were built from.
///
/// The same discipline as [`FoldedProjects`], and the consequence is the same
/// one stated there: a directory MOVED or re-symlinked under a live client
/// re-keys when the daemon next reports a session list with different cwds in
/// it, rather than on the next paint. What is deliberately NOT done is a cache
/// that only ever grows, which would hold the key of every directory any
/// session ever ran in for the life of the process and would never notice a
/// move at all.
///
/// Every cwd is resolved, not only the orphans. Which rows are orphans depends
/// on the project fold, so resolving lazily would need the two memos to know
/// about each other; resolving all of them costs one extra `realpath` per
/// distinct project root per session-list CHANGE, which is a handful of
/// microseconds on a path that already re-lists every session.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct DirKeys {
    /// Every session's cwd, in daemon order, as it was when this was built.
    /// Compared field by field rather than hashed, matching [`FoldedProjects`]:
    /// twenty short string comparisons cost a fraction of one `realpath` and
    /// leave no collision to reason about.
    from: Vec<String>,
    /// Distinct raw cwd paired with its canonical key, ascending by cwd.
    keys: Vec<(String, String)>,
}

impl DirKeys {
    /// Was this built from exactly the cwds these sessions carry?
    fn matches(&self, sessions: &[SessionView]) -> bool {
        self.from.len() == sessions.len()
            && self
                .from
                .iter()
                .zip(sessions)
                .all(|(cwd, row)| *cwd == row.info.cwd)
    }

    /// Re-resolve, through the one implementation of the question.
    fn rebuild(&mut self, sessions: &[SessionView]) {
        // Taken out and put back so the loop can fill `keys` while reading the
        // cwds it was just handed.
        let mut from = core::mem::take(&mut self.from);
        from.clear();
        from.extend(sessions.iter().map(|row| row.info.cwd.clone()));
        self.keys.clear();
        for cwd in &from {
            if let Err(at) = self.find(cwd) {
                self.keys.insert(at, (cwd.clone(), inbox::project_key(cwd)));
            }
        }
        self.from = from;
    }

    fn find(&self, cwd: &str) -> Result<usize, usize> {
        self.keys
            .binary_search_by(|(known, _)| known.as_str().cmp(cwd))
    }

    /// The canonical key for one session's cwd.
    ///
    /// A miss is unreachable while [`DaemonState::dir_keys`] refreshes from the
    /// same session list the rows came from, which is the only way to obtain
    /// this type. It returns the raw text rather than panicking or dropping the
    /// row: a bucket under an unresolved spelling is a cosmetic split, and the
    /// two alternatives are a crash and a session that vanishes from the
    /// sidebar.
    fn key_of<'k>(&'k self, cwd: &'k str) -> &'k str {
        match self.find(cwd) {
            Ok(at) => &self.keys[at].1,
            Err(_) => cwd,
        }
    }
}

/// Everything one daemon connection knows. One value, however many windows.
/// The daemon's collision report, as this window last received it.
///
/// `watching` is carried rather than inferred from an empty list, because the
/// two states are not the same claim. Nobody looked and nothing collides look
/// identical in the data and must never look identical on screen: telling an
/// operator their agents are not fighting when detection is switched off is
/// the one answer this feature must never give.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Collisions {
    pub watching: bool,
    pub collisions: Vec<vitrum_proto::Collision>,
    pub sessions: Vec<vitrum_proto::CollisionSession>,
    /// Ways detection is currently incomplete, each a finished sentence.
    pub degraded: Vec<String>,
}

impl Collisions {
    /// How many files this session is contesting, and with how many others.
    #[must_use]
    pub fn for_session(&self, id: SessionId) -> Option<(usize, usize)> {
        let mut files = 0usize;
        let mut peers = std::collections::BTreeSet::new();
        for c in &self.collisions {
            if c.participants.iter().any(|p| p.session == id) {
                files += 1;
                peers.extend(
                    c.participants
                        .iter()
                        .map(|p| p.session)
                        .filter(|s| *s != id),
                );
            }
        }
        (files > 0).then_some((files, peers.len()))
    }

    /// One sentence for what detection is currently doing.
    ///
    /// "Nobody looked" and "nothing collides" are different answers and this
    /// is the only place that says which. The daemon's own reasons follow from
    /// [`Collisions::reasons`]; this is the headline above them.
    #[must_use]
    pub fn summary(&self) -> &'static str {
        if !self.watching {
            "Off. Nothing is being watched, so no contested file can be reported."
        } else if self.degraded.is_empty() {
            "On, and complete."
        } else {
            "On, but incomplete. What it cannot see:"
        }
    }

    /// The daemon's finished sentences explaining what detection is missing.
    ///
    /// Rendered verbatim. The daemon knows why its watcher is partial and this
    /// client does not, so paraphrasing here would be inventing a reason.
    #[must_use]
    pub fn reasons(&self) -> &[String] {
        &self.degraded
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeMemoKey {
    pub revision_sum: u64,
    pub workspace_id: WorkspaceId,
    pub ws_info: Option<(Grouping, usize, SectionVisibility)>,
    pub session_fp: u64,
    pub projects_fp: u64,
    pub tabs: Vec<SessionId>,
    pub tab_mru: Vec<SessionId>,
    pub filter: String,
    pub focused: Option<SessionId>,
    pub collapsed_len: usize,
    pub sections_expanded_len: usize,
    pub previews_expanded_len: usize,
    pub settled_expanded_len: usize,
    pub clock_sec: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreeMemo {
    pub key: TreeMemoKey,
    pub cached_groups_indices: Vec<CachedSidebarGroup>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedSidebarGroup {
    pub key: GroupKey,
    pub label: String,
    pub root: Option<String>,
    pub project_index: Option<usize>,
    pub current: bool,
    pub active_indices: Vec<usize>,
    pub hidden_indices: Vec<usize>,
    pub snoozed_indices: Vec<usize>,
    pub settled_indices: Vec<usize>,
    pub rollup: Option<vitrum_model::ProjectRollup>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonState {
    pub conn: ConnState,
    pub projects: Vec<ProjectInfo>,
    /// Every session the daemon lists, each wrapped in the client-local facts
    /// the daemon has no business knowing: whether the operator has it parked,
    /// has ruled on it, and when they last looked at it. Shared across windows
    /// on purpose; see this module's header for the argument.
    pub sessions: Vec<SessionView>,
    pub workspaces: WorkspaceSet,
    pub settings: Settings,
    /// Which files two or more live sessions are both changing.
    ///
    /// Daemon state and not one window's answer: a collision is between two
    /// SESSIONS, so every window has to render the same contested set or a
    /// second window marks the wrong rows. Kept verbatim as the daemon sent
    /// it, `watching` included: "nobody looked" and "nothing collides" are
    /// different answers and the surface must be able to draw them
    /// differently.
    pub collisions: Collisions,
    /// [`DaemonState::projects`] folded by canonical root. Derived, not a
    /// fact: see [`FoldedProjects`] for what rebuilds it and when.
    folded: Cache<FoldedProjects>,
    /// [`DaemonState::sessions`] cwds resolved to canonical directory keys.
    /// Derived, not a fact: see [`DirKeys`] for what rebuilds it and when.
    dirs: Cache<DirKeys>,
    pub revision: u64,
    pub sessions_revision: u64,
}

impl Default for DaemonState {
    fn default() -> Self {
        DaemonState {
            conn: ConnState::Connecting,
            projects: Vec::new(),
            sessions: Vec::new(),
            workspaces: WorkspaceSet::default(),
            settings: Settings::default(),
            collisions: Collisions::default(),
            dirs: Cache::default(),
            folded: Cache::default(),
            revision: 0,
            sessions_revision: 0,
        }
    }
}

impl DaemonState {
    /// Fold the daemon-shaped half of one control-plane message.
    pub fn apply(&mut self, msg: ServerMsg) -> Broadcast {
        match msg {
            ServerMsg::Welcome {
                protocol,
                server_version,
            } => {
                self.conn = if protocol == PROTOCOL_VERSION {
                    ConnState::Live { server_version }
                } else {
                    ConnState::Failed {
                        detail: format!(
                            "protocol mismatch: server speaks {protocol}, client speaks {PROTOCOL_VERSION}"
                        ),
                    }
                };
                Broadcast::None
            }
            ServerMsg::Projects { projects } => {
                self.projects = projects;
                Broadcast::None
            }
            ServerMsg::Sessions { sessions } => {
                self.replace_sessions(sessions);
                let live: BTreeSet<SessionKey> = self
                    .sessions
                    .iter()
                    .map(|row| SessionKey::of(&row.info))
                    .collect();
                self.workspaces.retain_sessions(&live);
                // Disjoint field borrows rather than a clone of every
                // `SessionInfo`: `adopt` only reads, and at twenty sessions
                // the clone was four strings and a vector per row for nothing.
                let DaemonState {
                    sessions,
                    workspaces,
                    ..
                } = self;
                workspaces.adopt(sessions.iter().map(|row| &row.info));
                Broadcast::SessionsChanged
            }
            ServerMsg::SessionCreated(info) | ServerMsg::SessionUpdated(info) => {
                // An upsert never removes a session, so no window has anything
                // to prune. Returning `SessionsChanged` here would put a full
                // re-prune on the once-a-second update path for nothing.
                self.workspaces.adopt(core::iter::once(&info));
                self.upsert(info);
                Broadcast::None
            }
            ServerMsg::Exited { session, code } => {
                if let Some(row) = self.row_mut(session) {
                    row.info.status = SessionStatus::Exited { code };
                }
                Broadcast::None
            }
            ServerMsg::SessionRemoved { session } => {
                let key = self.row(session).map(|row| SessionKey::of(&row.info));
                self.sessions.retain(|row| row.id() != session);
                // The placement goes with the row. Leaving it behind would
                // keep the workspace counting a session nobody can see, which
                // is exactly the state the delete guard refuses to allow.
                //
                // One targeted removal, not a rebuild: this used to collect a
                // `BTreeSet` of every surviving key and re-`retain` every
                // placement map against it, which is a tree node per live
                // session plus a full walk of `home` and of every workspace's
                // `folder_of`, to delete one entry.
                if let Some(key) = key {
                    self.workspaces.forget_session(key);
                }
                Broadcast::SessionsChanged
            }
            // Not a daemon-state change: the answer belongs to whichever
            // window asked for it, so it is fanned out rather than folded.
            // This used to return `Broadcast::None` under a comment about a
            // search overlay holding its own results signal; there was no such
            // overlay and no such signal, and the line dropped every answer
            // the daemon ever sent.
            ServerMsg::SearchResults {
                pattern,
                hits,
                truncated,
                bytes_scanned,
            } => Broadcast::SearchResults {
                pattern,
                hits,
                truncated,
                bytes_scanned,
            },
            ServerMsg::ScrollbackChunk {
                session,
                from_seq,
                data,
                more,
            } => Broadcast::Scrollback {
                // The daemon owns `from_seq` and a `u64` add is a panic in a
                // debug build. A saturating end offset is wrong by nothing any
                // real stream can reach and cannot take the window down.
                resume_seq: from_seq.saturating_add(data.len() as u64),
                session,
                from_seq,
                bytes: data,
                more,
            },
            ServerMsg::Error {
                session, message, ..
            } => Broadcast::Error { session, message },
            // Folded, not fanned out, for the reason on the field. The
            // broadcast is `None` because there is no EXTRA window reaction to
            // run: each window owns its own `UiState` and the write itself is
            // what repaints it.
            ServerMsg::CollisionReport {
                watching,
                collisions,
                sessions,
                degraded,
            } => {
                self.collisions = Collisions {
                    watching,
                    collisions,
                    sessions,
                    degraded,
                };
                Broadcast::None
            }
        }
    }

    /// Look up one row by id.
    pub fn row(&self, id: SessionId) -> Option<&SessionView> {
        self.sessions.iter().find(|row| row.id() == id)
    }

    /// How many sessions want the operator, across every workspace.
    ///
    /// Deliberately not [`WindowState::attention_count`]: that one answers
    /// "how many rows are on screen in this window", which is the right
    /// number for the jump affordance beside those rows. A dock badge or a
    /// launcher entry is one per process, so it has to count every session
    /// the daemon holds, including the ones filed in a workspace nobody is
    /// currently looking at. Those are exactly the ones a badge exists to
    /// tell you about.
    pub fn attention_total(&self, clock: Clock) -> usize {
        let policy = self.policy();
        self.sessions
            .iter()
            .filter(|row| inbox::wants_operator(row, clock, policy))
            .count()
    }

    pub fn row_mut(&mut self, id: SessionId) -> Option<&mut SessionView> {
        self.sessions.iter_mut().find(|row| row.id() == id)
    }

    /// The project list folded by canonical root, refreshing the memo first if
    /// the daemon has sent a different list since it was built.
    ///
    /// The returned guard must not be held across another call to this
    /// method: the refresh takes a mutable borrow. There is exactly one caller
    /// ([`WindowState::bucket_by_directory`]) and it does not recurse.
    fn folded_projects(&self) -> core::cell::Ref<'_, FoldedProjects> {
        let stale = !self.folded.0.borrow().matches(&self.projects);
        if stale {
            self.folded.0.borrow_mut().rebuild(&self.projects);
        }
        self.folded.0.borrow()
    }

    /// Throw the folded-project memo away, so the next derivation refolds from
    /// [`DaemonState::projects`].
    ///
    /// This is the FULL RE-DERIVATION the memo replaced, reached through the
    /// same one implementation rather than a second copy of it. Two callers,
    /// both tests: the benchmark's before-and-after pair, and the equivalence
    /// test that drives a sequence of updates through both paths.
    #[cfg(test)]
    fn forget_folded(&self) {
        *self.folded.0.borrow_mut() = FoldedProjects::default();
    }

    /// Session cwds resolved to canonical directory keys, refreshing the memo
    /// first if the daemon has listed different cwds since it was built.
    ///
    /// Same guard shape as [`DaemonState::folded_projects`], and the same
    /// rule: do not hold the returned guard across another call to this
    /// method. One caller, and it does not recurse.
    fn dir_keys(&self) -> core::cell::Ref<'_, DirKeys> {
        let stale = !self.dirs.0.borrow().matches(&self.sessions);
        if stale {
            self.dirs.0.borrow_mut().rebuild(&self.sessions);
        }
        self.dirs.0.borrow()
    }

    /// Throw the directory-key memo away, so the next derivation re-resolves
    /// every cwd from the filesystem.
    ///
    /// The counterpart of [`DaemonState::forget_folded`], and it exists for the
    /// same two callers: the benchmark's before-and-after pair and the
    /// equivalence test that proves the memo draws the tree a full
    /// re-derivation draws.
    #[cfg(test)]
    fn forget_dir_keys(&self) {
        *self.dirs.0.borrow_mut() = DirKeys::default();
    }

    /// Look up one session's daemon projection by id.
    pub fn session(&self, id: SessionId) -> Option<&SessionInfo> {
        self.row(id).map(|row| &row.info)
    }

    /// The disposition policy in force. Lives in [`Settings`] because it is
    /// one, and is reached through here because every band decision needs it.
    pub fn policy(&self) -> DispositionPolicy {
        self.settings.policy
    }

    /// Which workspace a session is filed into, or `None` if the daemon does
    /// not list it.
    pub fn workspace_of(&self, id: SessionId) -> Option<WorkspaceId> {
        self.session(id)
            .map(|info| self.workspaces.workspace_of(info))
    }

    /// Every row filed into one workspace, in daemon order.
    pub fn workspace_rows(&self, workspace: WorkspaceId) -> Vec<&SessionView> {
        self.sessions
            .iter()
            .filter(|row| self.workspaces.workspace_of(&row.info) == workspace)
            .collect()
    }

    /// Move sessions into a workspace, returning how many moved.
    pub fn move_to_workspace(
        &mut self,
        ids: &[SessionId],
        to: WorkspaceId,
    ) -> Result<usize, WorkspaceError> {
        if !self.workspaces.contains(to) {
            return Err(WorkspaceError::Unknown);
        }
        // Keys, not clones. A `SessionKey` is two `u64`s and `Copy`; the
        // previous form cloned a whole `SessionInfo` per id (four strings and
        // an argument vector) purely so the assign loop could borrow `self`
        // mutably, which is up to twenty allocations for a bulk move that
        // changes one map entry each.
        let keys: Vec<SessionKey> = ids
            .iter()
            .filter_map(|id| self.session(*id).map(SessionKey::of))
            .collect();
        let moved = keys.len();
        for key in keys {
            self.workspaces.assign_key(key, to)?;
        }
        Ok(moved)
    }

    /// File sessions into a named folder of whichever workspace they live in,
    /// or out of every folder when `folder` is `None`.
    pub fn move_to_folder(
        &mut self,
        ids: &[SessionId],
        folder: Option<FolderId>,
    ) -> Result<usize, WorkspaceError> {
        // Keys, not clones, for the reason on `move_to_workspace`.
        let keys: Vec<SessionKey> = ids
            .iter()
            .filter_map(|id| self.session(*id).map(SessionKey::of))
            .collect();
        let mut moved = 0;
        for key in keys {
            self.workspaces.assign_folder_key(key, folder)?;
            moved += 1;
        }
        Ok(moved)
    }

    /// True when the server is connected and will answer a command.
    ///
    /// Menu entries that need a round trip are disabled rather than hidden
    /// when this is false, so a disconnected window still shows what it would
    /// be able to do.
    pub fn server_ready(&self) -> bool {
        matches!(self.conn, ConnState::Live { .. })
    }

    /// The snooze choices to offer right now.
    ///
    /// Straight from the model: "this evening" disappears once evening is less
    /// than an hour away, and everything below the hour preset advances by
    /// calendar days rather than by adding milliseconds, so a snooze set the
    /// night before a clock change still lands at 9:00 on the intended date.
    pub fn snooze_presets(&self, clock: Clock) -> Vec<SnoozePreset> {
        snooze_presets(clock)
    }

    /// Replace the whole list from a snapshot, keeping the client-local facts.
    ///
    /// A snapshot is the daemon's view and the daemon does not know about
    /// snoozes, settles or what has been looked at. Taking the snapshot
    /// wholesale would silently un-snooze every parked row the first time the
    /// client reconnected, which is the worst possible moment to lose them.
    fn replace_sessions(&mut self, sessions: Vec<SessionInfo>) {
        let local: BTreeMap<SessionId, OperatorAxis> = self
            .sessions
            .iter()
            .map(|row| {
                (
                    row.id(),
                    (row.snooze, row.settle_override, row.last_visited_ms),
                )
            })
            .collect();
        self.sessions = sessions
            .into_iter()
            .map(|info| {
                let mut row = SessionView::new(info);
                if let Some((snooze, settle_override, last_visited_ms)) = local.get(&row.id()) {
                    row.snooze = *snooze;
                    row.settle_override = *settle_override;
                    row.last_visited_ms = *last_visited_ms;
                }
                row
            })
            .collect();
    }

    /// Replace a session's daemon projection in place, or append it, keeping
    /// list order and the client-local facts stable.
    ///
    /// In-place matters twice. The sidebar's static order is derived from
    /// creation time, and remove-then-push would drop the operator's snooze
    /// every time the daemon pushed an update for a parked row.
    fn upsert(&mut self, info: SessionInfo) {
        match self.sessions.iter_mut().find(|row| row.id() == info.id) {
            Some(row) => row.info = info,
            None => self.sessions.push(SessionView::new(info)),
        }
    }

    /// Stamp `id` as looked at.
    ///
    /// Never moves backwards: an out-of-order stamp from a slow message would
    /// otherwise resurrect a badge the operator has already cleared.
    pub fn visit(&mut self, id: SessionId, now_ms: u64) {
        if let Some(row) = self.row_mut(id) {
            row.last_visited_ms = Some(row.last_visited_ms.map_or(now_ms, |was| was.max(now_ms)));
        }
    }

    /// Park `ids` until `wake_at_ms`.
    ///
    /// Rows blocked on the operator are skipped rather than parked. Hiding a
    /// pending approval defeats the request, and the row would raise its hand
    /// and come straight back, so honouring the click would be theatre. The
    /// menu already disables the entry when the whole selection is refused;
    /// this is the backstop for a row whose status flipped between the
    /// right-click and the pick.
    pub fn snooze(&mut self, ids: &[SessionId], wake_at_ms: u64, now_ms: u64) -> usize {
        let mut parked = 0;
        for id in ids {
            let Some(row) = self.row_mut(*id) else {
                continue;
            };
            if !row.can_snooze() {
                continue;
            }
            row.snooze = Some(Snooze {
                snoozed_at_ms: now_ms,
                wake_at_ms,
            });
            // A snooze is an explicit ruling about where the row belongs, so a
            // stale settle override from earlier must not survive it.
            row.settle_override = None;
            parked += 1;
        }
        parked
    }

    /// Un-park `ids`, dropping the snooze entirely.
    ///
    /// Clearing rather than expiring: a woken row must not leave stale snooze
    /// fields behind, or it would mint a fresh Woke badge on its next
    /// completion for the rest of its life.
    pub fn wake(&mut self, ids: &[SessionId], now_ms: u64) {
        for id in ids {
            if let Some(row) = self.row_mut(*id) {
                row.snooze = None;
                row.last_visited_ms =
                    Some(row.last_visited_ms.map_or(now_ms, |was| was.max(now_ms)));
            }
        }
    }

    /// Drain `ids` out of the inbox.
    pub fn settle(&mut self, ids: &[SessionId], now_ms: u64) -> usize {
        let mut drained = 0;
        for id in ids {
            let Some(row) = self.row_mut(*id) else {
                continue;
            };
            if !row.can_settle() {
                continue;
            }
            row.settle_override = Some(SettleOverride::Settled);
            // Settling is the operator saying they are finished, which is a
            // stronger statement than having looked. Without the stamp an
            // unseen completion would pull the row straight back.
            row.last_visited_ms = Some(row.last_visited_ms.map_or(now_ms, |was| was.max(now_ms)));
            drained += 1;
        }
        drained
    }

    /// Pin `ids` back into the inbox and suppress auto-settle on them.
    pub fn unsettle(&mut self, ids: &[SessionId]) {
        for id in ids {
            if let Some(row) = self.row_mut(*id) {
                row.settle_override = Some(SettleOverride::Active);
                row.snooze = None;
            }
        }
    }

    /// Clear the unseen markers tracked for `ids`.
    ///
    /// This is the client's half of "mark read" and it is honest about being
    /// only that half. The daemon owns `unread` and clears it when a session is
    /// attached; nothing in the protocol lets a client set it. What a client
    /// can say is when the operator last looked, which is what drives the
    /// unseen-completion and Woke badges, so those do clear.
    pub fn mark_seen(&mut self, ids: &[SessionId], now_ms: u64) {
        for id in ids {
            self.visit(*id, now_ms);
        }
    }

    /// Re-arm the unseen markers for `ids` by forgetting the last visit.
    pub fn mark_unseen(&mut self, ids: &[SessionId]) {
        for id in ids {
            if let Some(row) = self.row_mut(*id) {
                row.last_visited_ms = None;
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The sidebar tree
// ═══════════════════════════════════════════════════════════════════════════

/// Stable identity for one bucket in the sidebar, across both grouping modes.
///
/// This is what the collapse set, the section set and the preview set are
/// keyed on. It has to be one type rather than one per mode, because a window
/// remembers what it collapsed while the operator flips grouping back and
/// forth, and two key spaces would silently share bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GroupKey {
    /// A project the daemon named.
    Project(ProjectId),
    /// A directory with no project, identified by a hash of its path.
    Directory(u64),
    /// A folder the operator made.
    Folder(FolderId),
    /// Sessions in no folder, in [`Grouping::Named`].
    Unfiled,
}

/// FNV-1a over a canonical directory path.
///
/// A hash rather than the path itself so [`GroupKey`] stays `Copy` and the
/// collapse sets stay cheap; the label the operator reads comes from
/// [`SidebarGroup::label`], not from the key. A collision would make two
/// directories share one collapse bit and nothing else, which is why 64 bits
/// of FNV is enough and a cryptographic hash would be theatre.
///
/// **`path` must already have been through [`inbox::project_key`].** That is
/// what makes one directory one bucket: `/tmp/x`, `/tmp/x/` and a symlink to
/// it have to land on the same bit, and hashing the raw text gave three. This
/// used to canonicalise for its caller, which meant every orphan bucket paid a
/// second `realpath` on a string that had just come out of the first one — and
/// worse, the two calls straddled a window in which the directory could be
/// created, which would split a bucket from its own key.
fn directory_key(path: &str) -> u64 {
    inbox::fnv1a(path.as_bytes())
}

/// One bucket of the sidebar: a header and its banded rows.
///
/// Wraps [`inbox::Group`] rather than replacing it, so every comparator, band
/// split and preview cut still has exactly one implementation. What this adds
/// is the identity and the label, which `Group` cannot carry because it names
/// its bucket with a `&ProjectInfo` and two of the four bucket kinds here are
/// not projects.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarGroup<'a> {
    pub key: GroupKey,
    /// Header text: a project name, a directory path, a folder name, or
    /// "Unfiled".
    pub label: String,
    /// Filesystem root behind this bucket, when it has one, canonicalised. The
    /// sidebar shortens it for display, seeds "new session here" from it, and
    /// derives the bucket's mark from it.
    pub root: Option<String>,
    /// The daemon project this bucket came from, when it came from one. The
    /// lowest-numbered record for the root when several ids named it.
    pub project: Option<&'a ProjectInfo>,
    /// Is this the bucket the operator is working in right now?
    ///
    /// Set on exactly one bucket, and only in [`Grouping::Directory`]. Drawn
    /// differently and pinned to the top of the list by
    /// [`WindowState::pin_current_bucket`].
    pub current: bool,
    /// Rows, split into the model's three bands.
    pub bands: Group<'a>,
}

impl SidebarGroup<'_> {
    pub fn len(&self) -> usize {
        self.bands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bands.is_empty()
    }

    /// Rows in one band.
    pub fn section(&self, section: Section) -> &[&SessionView] {
        self.bands.section(section)
    }

    /// Does this bucket hold `id`?
    pub fn holds(&self, id: SessionId) -> bool {
        self.bands.holds(id)
    }

    /// Can this header be collapsed?
    ///
    /// Everything except Unfiled. Collapsing the Unfiled bucket would hide
    /// rows behind a header that has no name to look for them under.
    pub fn collapsible(&self) -> bool {
        self.key != GroupKey::Unfiled
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Window state
// ═══════════════════════════════════════════════════════════════════════════

/// One window's tab strip for one workspace.
///
/// Parked whole when the window switches workspace and restored when it
/// switches back, which is what makes a workspace behave like a virtual
/// desktop rather than a filter: you leave and come back to what you had open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Strip {
    /// Open tabs, in strip order.
    pub tabs: Vec<SessionId>,
    /// Tabs by recency, oldest first. Only eviction reads it.
    pub tab_mru: Vec<SessionId>,
    pub focused: Option<SessionId>,
}

/// What the terminal's painted history currently covers.
///
/// Reset whenever focus moves, because the grid is reset too. `span` is the
/// byte count the last request actually returned, which is what a page-back
/// grows: the daemon answers "the last N bytes before this point", so asking
/// for a bigger N from the same anchor is how you see further back without a
/// prepend the terminal cannot do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HistoryWindow {
    pub session: Option<SessionId>,
    /// Offset of the first painted byte.
    pub from_seq: u64,
    /// Bytes the daemon returned.
    pub span: u64,
    /// The daemon still holds bytes older than `from_seq`.
    pub more: bool,
}

/// Why history was asked for.
///
/// The answer to a scrollback request is the same message whichever gesture
/// caused it, so the reason has to be remembered at the request and consumed
/// at the answer. Without it a search jump and an attach are indistinguishable
/// on arrival, and the client would either scroll on every attach or never
/// scroll at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryIntent {
    /// Focus moved. Paint at the bottom and stay there.
    #[default]
    Attach,
    /// The operator scrolled to the top and asked for more. Keep the viewport
    /// on the line they were reading.
    PageBack,
    /// A search hit was activated. Scroll to this absolute byte offset.
    Jump(u64),
}

impl HistoryIntent {
    /// The offset to scroll to, if this request was a jump.
    #[must_use]
    pub const fn jump_seq(self) -> Option<u64> {
        match self {
            HistoryIntent::Jump(seq) => Some(seq),
            _ => None,
        }
    }
}

/// One window's view onto a [`DaemonState`]. One value per window.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowState {
    /// Which window this is, zero-based, in the order the process opened them.
    ///
    /// It is the window's slot in the persisted document. A window has to know
    /// its own slot because nothing else can tell it: each desktop window runs
    /// its own VirtualDom and cannot see another's state, so a save that did
    /// not name a slot would write itself into slot 0 and delete every other
    /// window's saved layout.
    pub index: usize,
    /// The workspace this window is looking at. Two windows may differ.
    pub workspace: WorkspaceId,
    /// Is the workspace bar showing its full list rather than just the current
    /// workspace?
    pub workspace_bar_open: bool,
    /// Open tabs for [`WindowState::workspace`], in strip order.
    pub tabs: Vec<SessionId>,
    /// Tabs by recency, oldest first. Only eviction reads it; strip order is
    /// `tabs`, which never reorders under the pointer.
    pub tab_mru: Vec<SessionId>,
    /// The one session whose output is streaming in this window.
    pub focused: Option<SessionId>,
    /// Strips belonging to workspaces this window is not currently viewing.
    parked: BTreeMap<WorkspaceId, Strip>,
    /// Buckets the user collapsed. Collapsed by user action only, never by a
    /// snapshot, so a `Sessions` push cannot reopen a group the user shut.
    pub collapsed: BTreeSet<GroupKey>,
    pub sidebar_collapsed: bool,
    /// Sections the user opened, keyed by bucket and band. Snoozed and Settled
    /// are collapsed by default; Active has no head and is always drawn.
    pub sections_expanded: BTreeSet<(GroupKey, Section)>,
    /// Buckets whose inbox is showing every row rather than the first
    /// [`inbox::PREVIEW_LIMIT`].
    pub previews_expanded: BTreeSet<GroupKey>,
    /// Buckets whose Done shelf is showing its whole tail rather than the
    /// first [`inbox::SETTLED_TAIL_LIMIT`] rows. Separate from
    /// `previews_expanded`, which is the Active band's cut: one operator can
    /// want every live row and still not want three hundred drained ones.
    ///
    /// Not persisted, matching `previews_expanded`. A tail cut is a reading
    /// gesture rather than a preference, and collapsed is the safe default to
    /// come back to.
    pub settled_expanded: BTreeSet<GroupKey>,
    /// Rows the operator has marked for a bulk action, with the anchor a
    /// shift-click ranges from.
    pub selection: Selection,
    pub sidebar_width: f64,
    /// Terminal geometry as last measured by the fit addon.
    pub cols: u16,
    pub rows: u16,
    /// Sidebar filter query. Empty means no filtering.
    ///
    /// Held here rather than in a component-local signal so the filter and the
    /// grouping that consumes it stay one testable unit.
    pub filter: String,
    /// The cross-session scrollback search surface's state. Window-scoped
    /// because the question is one operator's, and two windows sweeping for
    /// different patterns is the normal case rather than a conflict.
    pub search: SearchState,
    /// Most recent message for the strip above the terminal.
    pub flash: Option<Flash>,
    /// The one transient layer that is open, if any.
    ///
    /// Exactly one, never a stack. A context menu on top of a modal on top of
    /// the shortcut overlay has no correct Escape behaviour, and every
    /// combination would need its own focus rule. Opening a layer closes
    /// whichever one was open.
    pub layer: Layer,
    /// What the painted history covers, for the focused session.
    ///
    /// The client used to paint one head-anchored chunk and forget everything
    /// about it, so there was no way to ask for the region before it and the
    /// daemon's `more` flag had no reader. This is the anchor that makes
    /// paging back possible.
    pub history: HistoryWindow,
    /// Why the outstanding scrollback request was made.
    ///
    /// One at a time is enough: there is one terminal per window, requests are
    /// issued from `reconcile` and from the two explicit gestures below, and a
    /// second request supersedes the first rather than queueing behind it.
    pub history_intent: HistoryIntent,
    pub revision: u64,
    pub filter_revision: u64,
    pub tree_memo: std::cell::RefCell<Option<TreeMemo>>,
}

impl Default for WindowState {
    fn default() -> Self {
        WindowState {
            index: 0,
            workspace: DEFAULT_WORKSPACE,
            workspace_bar_open: false,
            tabs: Vec::new(),
            tab_mru: Vec::new(),
            focused: None,
            parked: BTreeMap::new(),
            collapsed: BTreeSet::new(),
            sidebar_collapsed: false,
            sections_expanded: BTreeSet::new(),
            previews_expanded: BTreeSet::new(),
            settled_expanded: BTreeSet::new(),
            selection: Selection::new(),
            sidebar_width: SIDEBAR_DEFAULT_PX,
            cols: 80,
            rows: 24,
            filter: String::new(),
            search: SearchState::default(),
            flash: None,
            layer: Layer::None,
            history: HistoryWindow::default(),
            history_intent: HistoryIntent::default(),
            revision: 0,
            filter_revision: 0,
            tree_memo: std::cell::RefCell::new(None),
        }
    }
}

impl WindowState {
    pub fn invalidate_tree_memo(&self) {
        if let Ok(mut borrow) = self.tree_memo.try_borrow_mut() {
            *borrow = None;
        }
    }

    /// Turn one [`Broadcast`] into this window's reaction.
    pub fn receive(
        &mut self,
        daemon: &mut DaemonState,
        broadcast: &Broadcast,
        now_ms: u64,
    ) -> Reaction {
        self.invalidate_tree_memo();
        match broadcast {
            Broadcast::None => Reaction::None,
            Broadcast::SessionsChanged => {
                self.prune(daemon, now_ms);
                Reaction::None
            }
            Broadcast::Scrollback {
                session,
                from_seq,
                resume_seq,
                bytes,
                more,
            } => {
                // A chunk for a session this window already navigated away
                // from is stale by definition: the terminal it was meant for
                // has been reset and repointed. Painting it would corrupt the
                // new one. With several windows open at most one is focused on
                // the session, and the rest correctly do nothing.
                if self.focused != Some(*session) {
                    return Reaction::None;
                }
                // Consumed here, so a second chunk arriving for any reason
                // paints as a plain attach rather than repeating a jump the
                // operator has already been taken to.
                let asked = std::mem::take(&mut self.history_intent);
                self.history = HistoryWindow {
                    session: Some(*session),
                    from_seq: *from_seq,
                    more: *more,
                    span: bytes.len() as u64,
                };
                Reaction::Backfill {
                    session: *session,
                    from_seq: *from_seq,
                    resume_seq: *resume_seq,
                    bytes: bytes.clone(),
                    jump_seq: asked.jump_seq(),
                    keep_view: asked == HistoryIntent::PageBack,
                    more: *more,
                }
            }
            Broadcast::Error { session, message } => {
                if message.starts_with(GAP_ERROR_PREFIX)
                    && let Some(id) = session
                    && self.focused == Some(*id)
                {
                    return Reaction::Refill { session: *id };
                }
                // A session-scoped error belongs in the windows that can see
                // the session. Flashing it in a window whose sidebar does not
                // contain the row names a session the operator cannot find.
                if let Some(id) = session
                    && daemon
                        .workspace_of(*id)
                        .is_some_and(|w| w != self.workspace)
                {
                    return Reaction::None;
                }
                self.flash = Some(Flash::error(match session {
                    Some(SessionId(id)) => format!("session {id}: {message}"),
                    None => message.clone(),
                }));
                Reaction::None
            }
            Broadcast::SearchResults {
                pattern,
                hits,
                truncated,
                bytes_scanned,
            } => {
                // Cleared even when `hits` is empty. A sweep that found
                // nothing would otherwise leave the summary reading
                // "Sweeping every session's scrollback" forever, which reports
                // an answered question as a hung one.
                self.search.searching = false;
                self.search.answer = Some(crate::ui::search::Answer {
                    pattern: pattern.clone(),
                    hits: hits.clone(),
                    truncated: *truncated,
                    bytes_scanned: *bytes_scanned,
                });
                Reaction::None
            }
        }
    }

    /// Point this window at another workspace.
    ///
    /// Swaps the tab strip, drops the filter and the selection, and moves
    /// intake so the next session launched appears where the operator is
    /// looking. The filter goes because the sidebar's whole contents are being
    /// replaced: a query carried across would answer a fresh workspace with
    /// "No sessions match" and read as a broken switch.
    pub fn set_workspace(
        &mut self,
        daemon: &mut DaemonState,
        to: WorkspaceId,
        now_ms: u64,
    ) -> Result<(), WorkspaceError> {
        if !daemon.workspaces.contains(to) {
            return Err(WorkspaceError::Unknown);
        }
        if self.workspace == to {
            return Ok(());
        }
        self.park_strip();
        self.workspace = to;
        let strip = self.parked.remove(&to).unwrap_or_default();
        self.tabs = strip.tabs;
        self.tab_mru = strip.tab_mru;
        self.focused = strip.focused;
        self.filter.clear();
        self.selection = Selection::new();
        daemon.workspaces.set_intake(to)?;
        self.prune(daemon, now_ms);
        Ok(())
    }

    fn park_strip(&mut self) {
        let strip = Strip {
            tabs: core::mem::take(&mut self.tabs),
            tab_mru: core::mem::take(&mut self.tab_mru),
            focused: self.focused.take(),
        };
        if strip == Strip::default() {
            self.parked.remove(&self.workspace);
        } else {
            self.parked.insert(self.workspace, strip);
        }
    }

    /// Sessions this window can reach: filed into its workspace, and admitted
    /// by the workspace's band visibility.
    ///
    /// An iterator rather than a `Vec`, because its one caller immediately
    /// filters it again and then collects: materialising here meant two
    /// unsized `collect`s, and an unsized `collect` doubles its way up from
    /// four. Seven allocations for one list of twenty pointers.
    fn admitted<'s, 'a>(
        &'s self,
        daemon: &'a DaemonState,
        clock: Clock,
    ) -> impl Iterator<Item = &'a SessionView> + use<'s, 'a> {
        let ws = daemon.workspaces.get(self.workspace);
        let policy = daemon.policy();
        daemon.sessions.iter().filter(move |row| {
            ws.is_some_and(|ws| {
                daemon.workspaces.workspace_of(&row.info) == self.workspace
                    && ws.sections.shows(row.disposition(clock, policy))
            })
        })
    }

    /// The sidebar's buckets for this window, in draw order.
    ///
    /// Filtered three ways before any bucketing happens: by workspace, by the
    /// workspace's band visibility, and by the window's filter query. Every
    /// ordering decision inside a band belongs to [`vitrum_model::order`]. The
    /// inbox is deliberately static, newest first, so no row ever moves under
    /// the cursor because its status changed; the parked band sorts by soonest
    /// wake, which is the only useful question about a parked row; the drained
    /// band sorts by when the work ended.

    pub fn tree<'a>(&self, daemon: &'a DaemonState, clock: Clock) -> Vec<SidebarGroup<'a>> {
        let ws_info = daemon.workspaces.get(self.workspace).map(|w| (w.grouping, w.folders().len(), w.sections));
        let projects_fp: u64 = daemon.projects.iter().fold(0u64, |acc, p| {
            acc.wrapping_mul(31)
                .wrapping_add(p.id.0)
                .wrapping_add(p.name.len() as u64)
        });
        let session_fp: u64 = daemon.sessions.iter().fold(0u64, |acc, s| {
            let cwd_hash = s.info.cwd.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
            let settle_val = match s.settle_override {
                None => 0u64,
                Some(SettleOverride::Settled) => 1u64,
                Some(SettleOverride::Active) => 2u64,
            };
            acc.wrapping_mul(31)
                .wrapping_add(s.id().0)
                .wrapping_add(daemon.workspaces.workspace_of(&s.info).0 as u64)
                .wrapping_add(s.info.last_activity_ms)
                .wrapping_add(s.info.unread as u64)
                .wrapping_add(s.snooze.map_or(0, |sn| sn.wake_at_ms))
                .wrapping_add(s.settled_at_ms())
                .wrapping_add(s.last_visited_ms.unwrap_or(0))
                .wrapping_add(cwd_hash)
                .wrapping_add(settle_val)
        });

        let memo_key = TreeMemoKey {
            revision_sum: daemon.revision.wrapping_add(daemon.sessions_revision).wrapping_add(self.revision).wrapping_add(self.filter_revision),
            workspace_id: self.workspace,
            ws_info,
            session_fp,
            projects_fp,
            tabs: self.tabs.clone(),
            tab_mru: self.tab_mru.clone(),
            filter: self.filter.clone(),
            focused: self.focused,
            collapsed_len: self.collapsed.len(),
            sections_expanded_len: self.sections_expanded.len(),
            previews_expanded_len: self.previews_expanded.len(),
            settled_expanded_len: self.settled_expanded.len(),
            clock_sec: clock.now_ms / 1000,
        };
        if let Ok(borrow) = self.tree_memo.try_borrow() {
            if let Some(ref memo) = *borrow {
                if memo.key == memo_key {
                    let mut groups = Vec::with_capacity(memo.cached_groups_indices.len());
                    for cg in &memo.cached_groups_indices {
                        let project = cg.project_index.and_then(|idx| daemon.projects.get(idx));
                        let active = cg.active_indices.iter().filter_map(|&i| daemon.sessions.get(i)).collect();
                        let hidden = cg.hidden_indices.iter().filter_map(|&i| daemon.sessions.get(i)).collect();
                        let snoozed = cg.snoozed_indices.iter().filter_map(|&i| daemon.sessions.get(i)).collect();
                        let settled = cg.settled_indices.iter().filter_map(|&i| daemon.sessions.get(i)).collect();
                        groups.push(SidebarGroup {
                            key: cg.key,
                            label: cg.label.clone(),
                            root: cg.root.clone(),
                            project,
                            current: cg.current,
                            bands: crate::inbox::Group {
                                project,
                                active,
                                hidden,
                                snoozed,
                                settled,
                                rollup: cg.rollup.clone(),
                            },
                        });
                    }
                    return groups;
                }
            }
        }

        let out = {
            let Some(ws) = daemon.workspaces.get(self.workspace) else {
                return Vec::new();
            };
            let query = self.filter.trim().to_lowercase();
            let mut rows: Vec<&SessionView> = Vec::with_capacity(daemon.sessions.len());
            rows.extend(
                self.admitted(daemon, clock)
                    .filter(|row| matches_filter(&row.info, &query)),
            );

            let mut out = match ws.grouping {
                Grouping::Directory => self.bucket_by_directory(daemon, rows, clock),
                Grouping::Named => self.bucket_by_folder(daemon, ws, rows, clock),
            };
            if !query.is_empty() {
                out.retain(|g| !g.is_empty());
            }
            if ws.grouping == Grouping::Directory {
                self.pin_current_bucket(&mut out);
            }
            out
        };

        if let Ok(mut borrow) = self.tree_memo.try_borrow_mut() {
            let mut cached_groups = Vec::with_capacity(out.len());
            for g in &out {
                let project_index = g.project.and_then(|p| daemon.projects.iter().position(|proj| proj.id == p.id));
                let active_indices = g.bands.active.iter().filter_map(|s| daemon.sessions.iter().position(|sess| sess.id() == s.id())).collect();
                let hidden_indices = g.bands.hidden.iter().filter_map(|s| daemon.sessions.iter().position(|sess| sess.id() == s.id())).collect();
                let snoozed_indices = g.bands.snoozed.iter().filter_map(|s| daemon.sessions.iter().position(|sess| sess.id() == s.id())).collect();
                let settled_indices = g.bands.settled.iter().filter_map(|s| daemon.sessions.iter().position(|sess| sess.id() == s.id())).collect();
                cached_groups.push(CachedSidebarGroup {
                    key: g.key,
                    label: g.label.clone(),
                    root: g.root.clone(),
                    project_index,
                    current: g.current,
                    active_indices,
                    hidden_indices,
                    snoozed_indices,
                    settled_indices,
                    rollup: g.bands.rollup.clone(),
                });
            }
            *borrow = Some(TreeMemo {
                key: memo_key,
                cached_groups_indices: cached_groups,
            });
        }

        out
    }

    /// Lift the bucket the operator is working in to the top, and mark it.
    ///
    /// The project you are in right now is the one you come back to, and in a
    /// twenty-agent list it was wherever the daemon happened to list it. That
    /// list is not even creation-ordered: a project's id is an FNV hash of its
    /// root, the daemon keeps its registry in a `BTreeMap` keyed on that id, so
    /// opening a new project inserts it at an arbitrary point in the sidebar.
    /// Pinning puts the one bucket that has a claim to the top slot in it.
    ///
    /// It is a rotate and not a sort, and only ever moves ONE bucket, so no
    /// other header changes position. And "current" comes from focus and tab
    /// recency alone ([`inbox::current_session`]) — never from activity — so
    /// another project going red cannot displace it while the operator reads
    /// it. Both signals already live in `Strip`, so this survives a restart
    /// and a workspace switch without a byte of new persisted state.
    ///
    /// [`Grouping::Named`] is deliberately left alone. Folders have an order
    /// the operator arranged in `Settings > Workspaces`, and overriding an
    /// explicit arrangement is a different thing from imposing one on a list
    /// that had none.
    fn pin_current_bucket(&self, out: &mut [SidebarGroup<'_>]) {
        let Some(current) = inbox::current_session(self.focused, &self.tab_mru) else {
            return;
        };
        let holds = |group: &SidebarGroup<'_>| group.holds(current);
        // A filter can hide the current session's bucket entirely, and a
        // window can be focused on a session in another workspace.
        if !out.iter().any(holds) {
            return;
        }
        inbox::pin_current(out, holds);
        out[0].current = true;
    }

    /// Bucket by the filesystem folder each session runs in.
    ///
    /// A project IS a filesystem folder: the daemon defines one as a named
    /// working root and has already answered "which root does this session run
    /// under" in `SessionInfo::project_id`. Re-deriving that here from a path
    /// prefix would be a second implementation of a question that already has
    /// an authoritative answer, and the two would disagree the first time a
    /// worktree moved.
    ///
    /// So: projects first, in server order, keeping the daemon's names. A
    /// session whose project the daemon does not list gets a bucket for its
    /// literal cwd, one per distinct directory, in first-appearance order.
    /// That is strictly better than the single anonymous orphan lump it
    /// replaces — twelve stray sessions in four directories now read as four
    /// named groups — and it is the only place this mode looks at a path.
    ///
    /// A PROJECT IS ITS DIRECTORY, not the id a client happened to mint for
    /// it. The protocol has no "create project" message, so the client owns
    /// project identity and the daemon records whatever it is handed on first
    /// use: four clients starting four sessions in one repo registered four
    /// ids for one root and drew four groups all called `vitrum`, each holding
    /// one session. So the daemon's list is folded by canonical root first
    /// ([`inbox::coalesce_projects`], memoised in [`FoldedProjects`]) and a row
    /// joins the group its OWN id maps to. The same fold applies to the cwd
    /// buckets, for the same reason: `/tmp/x` and `/tmp/x/` are one directory.
    ///
    /// A project with no rows IN THIS WORKSPACE is not drawn.
    ///
    /// `daemon.projects` is daemon-wide, so zipping it against the buckets
    /// emitted a header for every project that exists anywhere, including
    /// ones whose sessions all live in another workspace. Switching to a
    /// freshly created workspace therefore showed a project header reading
    /// "No sessions here yet" over nothing, which is the opposite of the
    /// separate top-level context a workspace is supposed to be.
    ///
    /// The old rationale was that "a project the daemon reports with no
    /// sessions is a place to start one". That case no longer exists: the
    /// registry only holds a project while a session references it, so an
    /// empty bucket here always means the sessions are somewhere else.
    fn bucket_by_directory<'a>(
        &self,
        daemon: &'a DaemonState,
        rows: Vec<&'a SessionView>,
        clock: Clock,
    ) -> Vec<SidebarGroup<'a>> {
        let folded = daemon.folded_projects();
        // Canonicalising is a `realpath`, and this used to do one per distinct
        // orphan cwd per paint, inside the render. The memo answers from
        // memory and only re-resolves when the daemon lists different cwds;
        // see [`DirKeys`] for the measurement and the invalidation rule.
        let cwds = daemon.dir_keys();
        let mut projects: Vec<Vec<&'a SessionView>> = vec![Vec::new(); folded.groups.len()];
        // Borrowed from the memo rather than owned: the bucket key is a string
        // the memo is already holding, and cloning it here would put the
        // allocation back that resolving it from memory just removed.
        let mut dirs: Vec<(&str, Vec<&'a SessionView>)> = Vec::new();

        for row in rows {
            match folded.group_of(row.project_id()) {
                Some(at) => projects[at].push(row),
                None => {
                    let dir = cwds.key_of(&row.info.cwd);
                    let at = match dirs.iter().position(|(known, _)| *known == dir) {
                        Some(at) => at,
                        None => {
                            dirs.push((dir, Vec::new()));
                            dirs.len() - 1
                        }
                    };
                    dirs[at].1.push(row);
                }
            }
        }

        let mut out = Vec::with_capacity(projects.len() + dirs.len());
        for (group, bucket) in folded.groups.iter().zip(projects) {
            if bucket.is_empty() {
                continue;
            }
            let key = GroupKey::Project(group.id);
            let lead = &daemon.projects[group.lead];
            out.push(SidebarGroup {
                key,
                label: lead.name.clone(),
                root: Some(group.key.clone()),
                project: Some(lead),
                current: false,
                bands: self.bands(daemon, key, Some(lead), bucket, clock),
            });
        }
        for (dir, bucket) in dirs {
            // `dir` is already what `project_key` returns, so it goes straight
            // to the hash. `project_key` is idempotent, so this is the same
            // key a raw path would produce, one `realpath` cheaper and without
            // the window in which the two calls could disagree.
            let key = GroupKey::Directory(directory_key(dir));
            out.push(SidebarGroup {
                key,
                label: dir.to_string(),
                root: Some(dir.to_string()),
                project: None,
                current: false,
                bands: self.bands(daemon, key, None, bucket, clock),
            });
        }
        out
    }

    /// Bucket by the workspace's named folders, in the operator's order.
    ///
    /// Empty folders are kept: a folder you just made and have not filled is
    /// exactly the folder you are about to drop something into, and hiding it
    /// until it has contents makes it unreachable.
    fn bucket_by_folder<'a>(
        &self,
        daemon: &'a DaemonState,
        ws: &Workspace,
        rows: Vec<&'a SessionView>,
        clock: Clock,
    ) -> Vec<SidebarGroup<'a>> {
        let mut buckets: Vec<(Option<&Folder>, Vec<&'a SessionView>)> =
            ws.folders().iter().map(|f| (Some(f), Vec::new())).collect();
        let mut unfiled: Vec<&'a SessionView> = Vec::new();

        for row in rows {
            match ws.folder_of(&row.info) {
                Some(id) => match buckets
                    .iter_mut()
                    .find(|(f, _)| f.is_some_and(|f| f.id == id))
                {
                    Some((_, bucket)) => bucket.push(row),
                    None => unfiled.push(row),
                },
                None => unfiled.push(row),
            }
        }
        if !unfiled.is_empty() {
            buckets.push((None, unfiled));
        }

        buckets
            .into_iter()
            .map(|(folder, bucket)| {
                let key = folder.map_or(GroupKey::Unfiled, |f| GroupKey::Folder(f.id));
                SidebarGroup {
                    key,
                    label: folder.map_or_else(|| "Unfiled".to_string(), |f| f.name.clone()),
                    root: None,
                    project: None,
                    current: false,
                    bands: self.bands(daemon, key, None, bucket, clock),
                }
            })
            .collect()
    }

    /// Split one bucket into bands, and roll it up for its collapsed header.
    ///
    /// The rollup's [`ProjectId`] is a LABEL and not a filter — see
    /// [`inbox::build_group`] — which is what lets one bucket carry rows from
    /// several daemon project ids, and what lets the two bucket kinds here
    /// that are not projects at all roll up under a synthesised key.
    fn bands<'a>(
        &self,
        daemon: &'a DaemonState,
        key: GroupKey,
        project: Option<&'a ProjectInfo>,
        rows: Vec<&'a SessionView>,
        clock: Clock,
    ) -> Group<'a> {
        let policy = daemon.policy();
        inbox::build_group(
            rollup_label(key),
            project,
            rows,
            self.focused,
            self.preview_expanded(key),
            clock,
            policy,
        )
    }

    /// Is one band of one bucket showing its rows?
    ///
    /// [`Section::Active`] is always open: it has no head, so there would be
    /// nothing on screen to open it again with. The other two are collapsed by
    /// default, because twenty drained agents above four live ones is the
    /// defect the bands exist to fix and an open-by-default band reintroduces
    /// it with an extra heading. Both heads carry their count, so nothing is
    /// hidden without a number saying how much.
    ///
    /// A filter forces every band open: the user asked for those rows by name,
    /// and answering a search with a collapsed band reads as "no results".
    pub fn section_open(&self, key: GroupKey, section: Section) -> bool {
        section == Section::Active
            || !self.filter.trim().is_empty()
            || self.sections_expanded.contains(&(key, section))
    }

    /// Show or hide one band.
    pub fn toggle_section(&mut self, key: GroupKey, section: Section) {
        if section == Section::Active {
            return;
        }
        if !self.sections_expanded.remove(&(key, section)) {
            self.sections_expanded.insert((key, section));
        }
    }

    /// Is this bucket's inbox showing every row rather than a preview?
    ///
    /// A filter forces it open for the same reason a band opens: the rows were
    /// asked for by name.
    pub fn preview_expanded(&self, key: GroupKey) -> bool {
        !self.filter.trim().is_empty() || self.previews_expanded.contains(&key)
    }

    /// Show all of a bucket's inbox, or fall back to the preview.
    pub fn toggle_preview(&mut self, key: GroupKey) {
        if !self.previews_expanded.remove(&key) {
            self.previews_expanded.insert(key);
        }
    }

    /// Is this bucket's Done shelf showing its whole tail?
    ///
    /// A filter forces it open for the same reason the preview opens: the rows
    /// were asked for by name.
    pub fn settled_expanded(&self, key: GroupKey) -> bool {
        !self.filter.trim().is_empty() || self.settled_expanded.contains(&key)
    }

    /// Show or hide the deep end of one bucket's Done shelf.
    pub fn toggle_settled_tail(&mut self, key: GroupKey) {
        if !self.settled_expanded.remove(&key) {
            self.settled_expanded.insert(key);
        }
    }

    /// True when a filter is active but the tree it produced is empty.
    ///
    /// Takes `tree_is_empty` rather than computing the tree, because the only
    /// live caller has already built it and a second `tree()` per paint is not
    /// free at twenty sessions. The RULE lives here so the sidebar and the
    /// tests cannot drift: this predicate was previously written out longhand
    /// in `ui/sidebar.rs` while a method of the same name sat unused in this
    /// file, so the tests were asserting a parallel implementation of
    /// something the product computed for itself.
    ///
    /// The distinction it draws is real: a failed search and an empty server
    /// both render nothing, and only one of them is the operator's doing.
    #[must_use]
    pub fn filter_matched_nothing_in(&self, tree_is_empty: bool) -> bool {
        !self.filter.trim().is_empty() && tree_is_empty
    }

    /// Clamp and store a dragged sidebar width, with the window's own width as
    /// an extra ceiling.
    ///
    /// The only width setter. An unbounded variant used to sit beside it,
    /// clamping to the stylesheet bounds alone, and nothing in the product
    /// called it: every live path needs the window ceiling too.
    ///
    /// The absolute cap is a legibility number and says nothing about the
    /// window it sits in. On a 3840px display a 448px sidebar is 12% of the
    /// window, which is fine; on an 800px one it is 56%, which leaves a
    /// terminal narrower than the sidebar beside it. The fraction is the
    /// ceiling that actually matters, and the absolute floor still wins over
    /// it, so a very narrow window gets a cramped terminal rather than an
    /// illegible sidebar.
    pub fn set_sidebar_width_in(&mut self, px: f64, window_px: f64) {
        let ceiling = (window_px * SIDEBAR_MAX_FRACTION).clamp(SIDEBAR_MIN_PX, SIDEBAR_MAX_PX);
        self.sidebar_width = px.clamp(SIDEBAR_MIN_PX, ceiling);
    }

    /// Every row a keypress or a shift-click can actually reach, in draw
    /// order.
    ///
    /// One definition of "visible", shared by traversal, range selection and
    /// selection pruning. Two definitions is how a shortcut lands on a row the
    /// operator cannot see. A collapsed bucket, a collapsed band and the
    /// preview cut all remove rows from this list, because none of them are on
    /// screen; [`WindowState::reveal`] is what a jump uses to bring one back.
    pub fn visible_ids(&self, daemon: &DaemonState, clock: Clock) -> Vec<SessionId> {
        self.visible_ids_of(&self.tree(daemon, clock))
    }

    /// [`WindowState::visible_ids`] over an already-arranged tree.
    ///
    /// Arranging is the expensive half: it resolves a status and a disposition
    /// for every row and then sorts three bands. The sidebar needs the tree
    /// AND the visible list AND the attention count in one paint, and deriving
    /// the tree three times is three times the work for an answer that cannot
    /// have changed between them. At thirty sessions on a daemon that pushes
    /// an update a second, that difference is most of the client's CPU.
    pub fn visible_ids_of(&self, tree: &[SidebarGroup<'_>]) -> Vec<SessionId> {
        // An upper bound rather than an exact count: `len` includes the rows
        // the preview cut hid, which never reach the list. One allocation that
        // is sometimes too big beats a `flat_map` collect that doubles.
        let mut ids = Vec::with_capacity(tree.iter().map(SidebarGroup::len).sum());
        ids.extend(self.visible_rows_of(tree).map(SessionView::id));
        ids
    }

    /// Every row this window is actually showing, in draw order.
    ///
    /// THE one definition of visibility, and both public answers are taken
    /// from it so they cannot drift apart. Three things remove a row: a
    /// collapsed bucket, a closed band, and the preview cut, which
    /// [`inbox::build_group`] has already applied by leaving those rows out of
    /// the bands and in `hidden`.
    ///
    /// It yields ROWS and not ids on purpose. `attention_count_of` used to
    /// flatten this tree into an owned `Vec<SessionId>` and then ask
    /// `DaemonState::row` to find each id again, which is a linear scan of the
    /// session list per visible row: four hundred comparisons and sixteen
    /// allocations per paint at twenty sessions, to re-find rows the tree was
    /// already holding pointers to.
    fn visible_rows_of<'t>(
        &'t self,
        tree: &'t [SidebarGroup<'t>],
    ) -> impl Iterator<Item = &'t SessionView> {
        tree.iter().flat_map(move |group| {
            let bucket_collapsed = group.collapsible() && self.collapsed.contains(&group.key);
            [Section::Active, Section::Snoozed, Section::Settled]
                .into_iter()
                .filter(move |section| !bucket_collapsed && self.section_open(group.key, *section))
                .flat_map(move |section| group.section(section).iter().copied())
        })
    }

    /// The next visible row in `direction` that wants the operator, or `None`
    /// when nothing does.
    ///
    /// This is the answer to twenty agents, and it is why the inbox order is
    /// deliberately static. Reordering the list so the urgent row floats to
    /// the top moves every other row under the operator's cursor; leaving the
    /// list alone and giving them one key that jumps costs nothing and moves
    /// nothing. [`adjacent_matching`] guarantees the jump never lands on the
    /// row it started from, so pressing it with one matching row reports "no
    /// more" rather than pretending to move.
    ///
    /// Wraps, because the queue is a queue: reaching the last blocked row and
    /// pressing again should return to the first, not stop.
    pub fn attention_target(
        &self,
        daemon: &DaemonState,
        clock: Clock,
        direction: Direction,
    ) -> Option<SessionId> {
        // The predicate is answered from a set built in one pass over the tree
        // the visible list already came from. It used to call
        // `DaemonState::row`, which is a linear scan of the whole session
        // list, once per candidate `adjacent_matching` walked: O(visible x
        // sessions) comparisons to re-find rows the tree was holding pointers
        // to, and the whole point of the key is that it works at twenty rows.
        let policy = daemon.policy();
        let tree = self.tree(daemon, clock);
        let mut visible = Vec::with_capacity(tree.iter().map(SidebarGroup::len).sum());
        let mut wanted: BTreeSet<SessionId> = BTreeSet::new();
        for row in self.visible_rows_of(&tree) {
            if inbox::wants_operator(row, clock, policy) {
                wanted.insert(row.id());
            }
            visible.push(row.id());
        }
        adjacent_matching(&visible, self.focused, direction, Wrap::Around, |id| {
            wanted.contains(&id)
        })
    }

    /// The next visible row in `direction`, for plain arrow traversal.
    pub fn step_target(
        &self,
        daemon: &DaemonState,
        clock: Clock,
        direction: Direction,
    ) -> Option<SessionId> {
        vitrum_model::adjacent(
            &self.visible_ids(daemon, clock),
            self.focused,
            direction,
            Wrap::Clamp,
        )
    }

    /// How many visible rows are on the attention queue.
    ///
    /// Rendered on the jump affordance so an operator can see there is
    /// something to jump to before pressing anything.
    pub fn attention_count(&self, daemon: &DaemonState, clock: Clock) -> usize {
        self.attention_count_of(daemon, &self.tree(daemon, clock), clock)
    }

    /// [`WindowState::attention_count`] over an already-arranged tree.
    ///
    /// Allocates nothing: the tree is already holding every row this needs.
    pub fn attention_count_of(
        &self,
        daemon: &DaemonState,
        tree: &[SidebarGroup<'_>],
        clock: Clock,
    ) -> usize {
        let policy = daemon.policy();
        self.visible_rows_of(tree)
            .filter(|row| inbox::wants_operator(row, clock, policy))
            .count()
    }

    /// Open whatever is hiding `id`, so a row reached by keyboard is actually
    /// on screen rather than focused inside something collapsed.
    ///
    /// Three things can hide a row and all three have to give way, or the jump
    /// key silently moves focus somewhere invisible: the bucket, the band it
    /// sits in, and the preview cut.
    pub fn reveal(&mut self, daemon: &DaemonState, id: SessionId, clock: Clock) {
        let Some(row) = daemon.row(id) else { return };
        let section = row.section(clock, daemon.policy());
        let Some(key) = self.bucket_of(daemon, id, clock) else {
            return;
        };
        self.collapsed.remove(&key);
        self.sections_expanded.insert((key, section));
        self.previews_expanded.insert(key);
    }

    /// Which bucket a row draws in, under the current grouping.
    pub fn bucket_of(&self, daemon: &DaemonState, id: SessionId, clock: Clock) -> Option<GroupKey> {
        self.tree(daemon, clock).into_iter().find_map(|group| {
            let banded = [Section::Active, Section::Snoozed, Section::Settled]
                .into_iter()
                .any(|s| group.section(s).iter().any(|row| row.id() == id));
            // Hidden rows are past the preview cut, still in the bucket, and
            // are exactly the ones `reveal` has to find.
            let hidden = group.bands.hidden.iter().any(|row| row.id() == id);
            (banded || hidden).then_some(group.key)
        })
    }

    /// Open `id` as a tab if it is not already open, focus it, and record the
    /// visit.
    ///
    /// Recording the visit is what retires the Woke badge and the
    /// unseen-completion badge: looking at a row IS the acknowledgement, and a
    /// badge that needed a second explicit dismissal would never be cleared.
    ///
    /// Over [`MAX_TABS`] this evicts the least recently used tab. Eviction
    /// closes nothing: the child keeps running on the server, the row stays in
    /// the sidebar, and the session moves into [`WindowState::overflow`], which
    /// is what the strip's overflow button counts and lists. There is no state
    /// in which a session exists and no affordance reaches it.
    pub fn open(&mut self, daemon: &mut DaemonState, id: SessionId, now_ms: u64) {
        if !self.tabs.contains(&id) {
            self.tabs.push(id);
        }
        // Focus moving resets the grid, so whatever was painted no longer
        // exists. Keeping the old anchor would let a page-back ask for the
        // region before the PREVIOUS session's history.
        if self.focused != Some(id) {
            self.history = HistoryWindow::default();
            self.history_intent = HistoryIntent::Attach;
        }
        self.focused = Some(id);
        self.touch(id);
        daemon.visit(id, now_ms);
        self.selection.select_one(id);
        self.evict_stale_tabs();
    }

    /// Record `id` as the most recently used tab.
    ///
    /// Recency is tracked separately from strip order on purpose. Reordering
    /// the strip on every focus would move a tab out from under the pointer
    /// between the mousedown and the next click, so the strip stays in the
    /// order tabs were opened and only eviction consults recency.
    fn touch(&mut self, id: SessionId) {
        self.tab_mru.retain(|t| *t != id);
        self.tab_mru.push(id);
    }

    /// Drop least-recently-used tabs until the strip is back within [`MAX_TABS`].
    fn evict_stale_tabs(&mut self) {
        while self.tabs.len() > MAX_TABS {
            let victim = self
                .tab_mru
                .iter()
                .find(|t| self.tabs.contains(t) && Some(**t) != self.focused)
                .copied();
            // Unreachable while MAX_TABS >= 1, because at least one tab other
            // than the focused one exists whenever the strip is over budget.
            // Breaking rather than looping forever is the safe answer anyway.
            let Some(victim) = victim else { break };
            self.tabs.retain(|t| *t != victim);
            self.tab_mru.retain(|t| *t != victim);
        }
    }

    /// The tab that should take focus when `id` goes away: the one to its
    /// right, else the one to its left, else nothing.
    fn neighbour_of(&self, id: SessionId) -> Option<SessionId> {
        let i = self.tabs.iter().position(|t| *t == id)?;
        self.tabs
            .get(i + 1)
            .or_else(|| i.checked_sub(1).and_then(|p| self.tabs.get(p)))
            .copied()
    }

    /// Close the tab for `id`, moving focus to a neighbour if it was focused.
    ///
    /// Closing a tab does not close the session: the child keeps running on the
    /// server and keeps filling its ring.
    pub fn close_tab(&mut self, id: SessionId) {
        let next = self.neighbour_of(id);
        self.tabs.retain(|t| *t != id);
        self.tab_mru.retain(|t| *t != id);
        if self.focused == Some(id) {
            self.focused = next.filter(|n| self.tabs.contains(n));
            if let Some(f) = self.focused {
                self.touch(f);
            }
        }
    }

    /// Close every tab except `keep`. A no-op when `keep` is not in the strip,
    /// which would otherwise empty it.
    pub fn close_other_tabs(&mut self, keep: SessionId) {
        if !self.tabs.contains(&keep) {
            return;
        }
        self.tabs.retain(|t| *t == keep);
        self.tab_mru.retain(|t| *t == keep);
        self.focused = Some(keep);
        self.touch(keep);
    }

    /// Focus the tab `delta` positions away, wrapping in both directions.
    pub fn cycle(&mut self, delta: isize) {
        if self.tabs.is_empty() {
            self.focused = None;
            return;
        }
        let len = self.tabs.len() as isize;
        let cur = self
            .focused
            .and_then(|f| self.tabs.iter().position(|t| *t == f))
            .map(|i| i as isize)
            .unwrap_or(0);
        let next = (cur + delta).rem_euclid(len) as usize;
        let id = self.tabs[next];
        self.focused = Some(id);
        self.touch(id);
    }

    /// Focus the tab at strip position `i`. Out of range is a no-op, because
    /// Alt+7 with three tabs open should do nothing, not jump to the last tab.
    pub fn focus_index(&mut self, i: usize) {
        if let Some(id) = self.tabs.get(i).copied() {
            self.focused = Some(id);
            self.touch(id);
        }
    }

    /// Drop tabs, focus and selection that point at sessions this window can
    /// no longer reach.
    ///
    /// "Reach" is per window, not per daemon: a session the daemon still lists
    /// but which now lives in another workspace is as gone from this window as
    /// one that exited. Parked strips are pruned too, or switching back to a
    /// workspace would restore tabs for sessions that no longer exist.
    pub fn prune(&mut self, daemon: &mut DaemonState, now_ms: u64) {
        // One sorted vector, not a `BTreeSet` and then a flattened copy of it.
        // The set cost a tree node per session and `retain_visible` needs a
        // slice anyway, so the copy was pure overhead on a path that runs on
        // every session change in every window.
        let mut mine: Vec<SessionId> = daemon
            .sessions
            .iter()
            .filter(|row| daemon.workspaces.workspace_of(&row.info) == self.workspace)
            .map(|row| row.id())
            .collect();
        mine.sort_unstable();
        let holds = |id: &SessionId| mine.binary_search(id).is_ok();
        if let Some(f) = self.focused
            && !holds(&f)
        {
            // Pick the neighbour before the tab vanishes, so focus lands next
            // to where the user was rather than at the start of the strip.
            let next = self.neighbour_of(f);
            self.focused = next.filter(holds);
        }
        self.tabs.retain(holds);
        self.tab_mru.retain(holds);
        // A selection holding closed sessions makes a bulk action operate on
        // rows that no longer exist, and puts a count in a menu label that
        // does not match what the operator can see.
        self.selection.retain_visible(&mine);
        if self.focused.is_none() {
            self.focused = self.tabs.first().copied();
        }
        if let Some(f) = self.focused {
            self.touch(f);
            daemon.visit(f, now_ms);
        }
        // One pass over the session list for every parked strip together. This
        // used to re-scan every session and build a fresh `BTreeSet` per
        // parked workspace, which is O(parked x sessions) placement lookups
        // and a set allocation each, to prune strips holding at most
        // [`MAX_TABS`] entries.
        if !self.parked.is_empty() {
            let mut by_workspace: BTreeMap<WorkspaceId, Vec<SessionId>> = BTreeMap::new();
            for row in &daemon.sessions {
                by_workspace
                    .entry(daemon.workspaces.workspace_of(&row.info))
                    .or_default()
                    .push(row.id());
            }
            for (workspace, strip) in &mut self.parked {
                let theirs: &[SessionId] =
                    by_workspace.get(workspace).map_or(&[], Vec::as_slice);
                strip.tabs.retain(|id| theirs.contains(id));
                strip.tab_mru.retain(|id| theirs.contains(id));
                strip.focused = strip.focused.filter(|id| theirs.contains(id));
            }
        }
        self.parked.retain(|w, _| daemon.workspaces.contains(*w));
    }

    /// Repoint this window if the workspace it was looking at is gone.
    ///
    /// Returns true when it had to move. Deleting a workspace is a daemon-level
    /// act and cannot reach into the windows, so every window checks after one.
    pub fn reconcile_workspace(&mut self, daemon: &mut DaemonState, now_ms: u64) -> bool {
        if daemon.workspaces.contains(self.workspace) {
            return false;
        }
        self.workspace = daemon.workspaces.first();
        let strip = self.parked.remove(&self.workspace).unwrap_or_default();
        self.tabs = strip.tabs;
        self.tab_mru = strip.tab_mru;
        self.focused = strip.focused;
        self.filter.clear();
        self.selection = Selection::new();
        self.prune(daemon, now_ms);
        true
    }

    /// Apply one click on a row to the selection.
    ///
    /// The anchor is what makes shift-click feel right, and it is the model's
    /// job: a plain click sets it, a shift-click ranges from it without moving
    /// it, so widening and narrowing by repeated shift-clicks pivots around the
    /// row you started on rather than the one you touched last.
    pub fn click_row(&mut self, daemon: &DaemonState, id: SessionId, click: Click, clock: Clock) {
        let visible = self.visible_ids(daemon, clock);
        match click {
            Click::Plain => self.selection.select_one(id),
            Click::Toggle => self.selection.toggle(id),
            Click::Range => self.selection.extend_to(&visible, id),
            Click::RangeAdditive => self.selection.extend_to_additive(&visible, id),
        }
    }

    /// Select every row currently on screen.
    pub fn select_all_visible(&mut self, daemon: &DaemonState, clock: Clock) {
        let visible = self.visible_ids(daemon, clock);
        self.selection.select_all(&visible);
    }

    /// The rows a context menu opened on `target` acts on.
    ///
    /// Right-clicking inside a multi-selection acts on the whole selection;
    /// right-clicking outside it acts on the one row, which is what every file
    /// manager does and what stops a stray right-click from operating on
    /// nineteen sessions.
    pub fn menu_targets(
        &self,
        daemon: &DaemonState,
        target: SessionId,
        clock: Clock,
    ) -> Vec<SessionId> {
        let id = target;
        if self.selection.len() > 1 && self.selection.contains(id) {
            return self.selection.ordered(&self.visible_ids(daemon, clock));
        }
        vec![id]
    }

    /// The context menu for one target, as data.
    ///
    /// Pure and returned rather than rendered so the exact labels, order and
    /// enablement are testable. A menu that silently drops an entry because a
    /// condition flipped is invisible in a screenshot and obvious in a test.
    pub fn menu_items(
        &self,
        daemon: &DaemonState,
        target: SessionId,
        clock: Clock,
    ) -> Vec<MenuItem> {
        let ids = self.menu_targets(daemon, target, clock);
        if ids.len() > 1 {
            return self.bulk_menu(daemon, &ids, clock);
        }
        self.single_menu(daemon, target, clock)
    }

    /// The menu over a multi-selection.
    ///
    /// Labels come from [`vitrum_model::context_menu`], which carries the count
    /// on every one of them, because a bulk action with no visible count is how
    /// you close nineteen sessions meaning to close one.
    fn bulk_menu(&self, daemon: &DaemonState, ids: &[SessionId], clock: Clock) -> Vec<MenuItem> {
        let policy = daemon.policy();
        // Facts over the rows this menu will actually act on, not over the
        // whole selection. `menu_targets` drops selected rows the tree is not
        // currently showing (a collapsed bucket, a closed band, the preview
        // cut), so counting the selection made every label promise more than
        // the action delivers, and made the refusal counts below subtract a
        // larger number from a smaller one, which is a panic.
        let mut targets = Selection::new();
        targets.select_all(ids);
        let facts = SelectionFacts::collect(&targets, &daemon.sessions, clock, policy);
        let ready = daemon.server_ready();
        let mut out = Vec::with_capacity(8);
        for item in vitrum_model::context_menu(facts) {
            match item.action {
                vitrum_model::MenuAction::MarkRead => {
                    out.push(MenuItem::new(MenuAction::MarkRead, item.label).hint("clears badges"))
                }
                vitrum_model::MenuAction::MarkUnread => out
                    .push(MenuItem::new(MenuAction::MarkUnread, item.label).hint("re-arms badges")),
                vitrum_model::MenuAction::Wake => {
                    out.push(MenuItem::new(MenuAction::Wake, item.label).sep())
                }
                vitrum_model::MenuAction::Snooze => {
                    let refused = ids.len() - facts.snoozable;
                    let head = MenuItem::new(MenuAction::SnoozeHeader, item.label)
                        .enable(!item.disabled)
                        .sep();
                    out.push(if item.disabled {
                        head.hint(format!("{refused} blocked on you"))
                    } else {
                        head
                    });
                    if !item.disabled {
                        out.extend(self.preset_items(daemon, clock));
                    }
                }
                vitrum_model::MenuAction::Settle => {
                    let refused = ids.len() - facts.settleable;
                    let entry = MenuItem::new(MenuAction::Settle, item.label)
                        .enable(!item.disabled)
                        .sep();
                    out.push(if item.disabled {
                        entry.hint(format!("{refused} not finished"))
                    } else {
                        entry
                    });
                }
                vitrum_model::MenuAction::Unsettle => {
                    out.push(MenuItem::new(MenuAction::Unsettle, item.label))
                }
                vitrum_model::MenuAction::Close => out.push(
                    MenuItem::new(MenuAction::Terminate, item.label)
                        .enable(ready)
                        .danger()
                        .sep(),
                ),
            }
        }
        out.extend(self.filing_items(daemon, ids.len()));
        out
    }

    /// The four snooze presets as menu entries.
    fn preset_items(&self, daemon: &DaemonState, clock: Clock) -> Vec<MenuItem> {
        daemon
            .snooze_presets(clock)
            .into_iter()
            .map(|preset| {
                MenuItem::new(MenuAction::Snooze(preset.id), format!("  {}", preset.label))
                    .hint(preset.when_label)
            })
            .collect()
    }

    /// "Move to workspace" and, in named grouping, "Move to folder".
    ///
    /// This is the only way a session gets into a workspace, so it is not
    /// optional garnish: without it a new workspace is a blank sidebar with no
    /// way to fill it. The workspace this window is already showing is left
    /// out, because moving a row to where it already is does nothing and reads
    /// as a broken menu entry.
    fn filing_items(&self, daemon: &DaemonState, count: usize) -> Vec<MenuItem> {
        let mut out = Vec::new();
        let others: Vec<&Workspace> = daemon
            .workspaces
            .iter()
            .filter(|w| w.id != self.workspace)
            .collect();
        if !others.is_empty() {
            let label = if count > 1 {
                format!("Move {count} to workspace")
            } else {
                "Move to workspace".to_string()
            };
            out.push(MenuItem::new(MenuAction::MoveToWorkspaceHeader, label).sep());
            for ws in others {
                out.push(MenuItem::new(
                    MenuAction::MoveToWorkspace(ws.id),
                    format!("  {}", ws.name),
                ));
            }
        }
        let Some(ws) = daemon.workspaces.get(self.workspace) else {
            return out;
        };
        if ws.grouping != Grouping::Named {
            return out;
        }
        out.push(MenuItem::new(MenuAction::MoveToFolderHeader, "Move to folder").sep());
        out.push(MenuItem::new(MenuAction::MoveToFolder(None), "  Unfiled"));
        for folder in ws.folders() {
            out.push(MenuItem::new(
                MenuAction::MoveToFolder(Some(folder.id)),
                format!("  {}", folder.name),
            ));
        }
        out
    }

    /// The menu over exactly one row.
    fn single_menu(&self, daemon: &DaemonState, target: SessionId, clock: Clock) -> Vec<MenuItem> {
        let id = target;
        let Some(row) = daemon.row(id) else {
            return Vec::new();
        };
        let info = &row.info;
        let open = self.tabs.contains(&id);
        let ready = daemon.server_ready();
        let disposition = row.disposition(clock, daemon.policy());
        let mut v = Vec::with_capacity(16);

        v.push(MenuItem::new(MenuAction::Focus, "Open").enable(self.focused != Some(id)));
        v.push(MenuItem::new(MenuAction::CloseTab, "Close tab").enable(open));
        v.push(
            MenuItem::new(MenuAction::CloseOthers, "Close other tabs")
                .enable(open && self.tabs.len() > 1),
        );

        if disposition == Disposition::Snoozed {
            let until = row
                .snooze
                .map(|snooze| vitrum_model::wake_description(snooze.wake_at_ms, clock));
            let mut wake = MenuItem::new(MenuAction::Wake, "Wake now").sep();
            if let Some(until) = until {
                wake = wake.hint(format!("parked until {until}"));
            }
            v.push(wake);
        } else {
            let refusal = inbox::snooze_refusal(row);
            let mut head = MenuItem::new(MenuAction::SnoozeHeader, "Snooze until")
                .enable(refusal.is_none())
                .sep();
            if let Some(reason) = refusal {
                head = head.hint(reason);
            }
            v.push(head);
            if refusal.is_none() {
                v.extend(self.preset_items(daemon, clock));
            }
        }

        if disposition == Disposition::Settled {
            v.push(MenuItem::new(MenuAction::Unsettle, "Back to inbox"));
        } else {
            let refusal = inbox::settle_refusal(row);
            let mut settle = MenuItem::new(MenuAction::Settle, "Settle").enable(refusal.is_none());
            if let Some(reason) = refusal {
                settle = settle.hint(reason);
            }
            v.push(settle);
        }
        if row.has_unseen_completion() {
            v.push(MenuItem::new(MenuAction::MarkRead, "Mark seen"));
        } else {
            v.push(MenuItem::new(MenuAction::MarkUnread, "Mark unseen"));
        }

        v.extend(self.filing_items(daemon, 1));

        v.push(
            MenuItem::new(MenuAction::Rename, "Rename\u{2026}")
                .enable(ready)
                .sep(),
        );
        v.push(MenuItem::new(MenuAction::CopyPath, "Copy path"));
        v.push(
            MenuItem::new(MenuAction::CopyBranch, "Copy branch").enable(info.git_branch.is_some()),
        );
        v.push(MenuItem::new(MenuAction::CopyCommand, "Copy command"));

        v.push(MenuItem::new(MenuAction::NewSessionHere, "New session here").enable(ready));
        v.push(
            MenuItem::new(MenuAction::Duplicate, "Duplicate session")
                .enable(ready)
                .sep(),
        );
        // "Terminate" for a child that is still alive, "Remove" for one that
        // already exited: the server message is the same, but telling a user
        // you are about to kill a process that died ten minutes ago reads as a
        // shell that is not paying attention.
        let kill = if info.status.is_live() {
            "Terminate session"
        } else {
            "Remove session"
        };
        v.push(
            MenuItem::new(MenuAction::Terminate, kill)
                .enable(ready)
                .danger(),
        );
        v
    }
}

/// The id a bucket's rollup is labelled with.
///
/// [`ProjectRollup::project_id`] is a label here, not an identity: only the
/// project buckets can fill it honestly. Nothing in this file reads it, and a
/// caller that keys a map on it owns the uniqueness — two buckets that happen
/// to share a label produce rollups that compare unequal but key equal.
fn rollup_label(key: GroupKey) -> ProjectId {
    match key {
        GroupKey::Project(id) => id,
        GroupKey::Directory(hash) => ProjectId(hash),
        GroupKey::Folder(FolderId(id)) => ProjectId(id),
        GroupKey::Unfiled => ProjectId(u64::MAX),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The composite one window renders from
// ═══════════════════════════════════════════════════════════════════════════

/// One window's complete state: the daemon it observes, and its own view.
///
/// A single-window process holds one of these. A multi-window process holds
/// one [`DaemonState`] and N [`WindowState`]s and calls the [`WindowState`]
/// methods directly with `&daemon` / `&mut daemon`; every method here is a
/// one-line delegation to exactly those, so there is no second implementation
/// of anything to drift.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UiState {
    pub daemon: DaemonState,
    pub window: WindowState,
}

impl UiState {
    /// Fold one control-plane message into the model.
    ///
    /// `now_ms` stamps the visit a refocus implies. Passed in rather than read
    /// here so the fold stays a pure function and a test can put the clock
    /// exactly where a boundary is.
    pub fn apply(&mut self, msg: ServerMsg, now_ms: u64) -> Reaction {
        self.daemon.sessions_revision = self.daemon.sessions_revision.wrapping_add(1);
        self.window.invalidate_tree_memo();
        let broadcast = self.daemon.apply(msg);
        self.window.receive(&mut self.daemon, &broadcast, now_ms)
    }

    pub fn row(&self, id: SessionId) -> Option<&SessionView> {
        self.daemon.row(id)
    }

    pub fn session(&self, id: SessionId) -> Option<&SessionInfo> {
        self.daemon.session(id)
    }

    pub fn server_ready(&self) -> bool {
        self.daemon.server_ready()
    }
    pub fn state_revision(&self) -> u64 {
        self.daemon
            .revision
            .wrapping_add(self.daemon.sessions_revision)
            .wrapping_add(self.window.revision)
            .wrapping_add(self.window.filter_revision)
    }

    pub fn select_sessions(&self) -> &[SessionView] {
        &self.daemon.sessions
    }

    pub fn select_session(&self, id: SessionId) -> Option<&SessionView> {
        self.row(id)
    }

    pub fn select_workspace_id(&self) -> WorkspaceId {
        self.window.workspace
    }

    pub fn select_filter_query(&self) -> &str {
        &self.window.filter
    }

    pub fn select_sessions_revision(&self) -> u64 {
        self.daemon.sessions_revision
    }

    pub fn select_window_revision(&self) -> u64 {
        self.window.revision
    }

    pub fn select_state_revision(&self) -> u64 {
        self.state_revision()
    }

    pub fn has_changed_since(&self, last_revision: u64) -> bool {
        self.state_revision() != last_revision
    }

    pub fn snooze_presets(&self, clock: Clock) -> Vec<SnoozePreset> {
        self.daemon.snooze_presets(clock)
    }

    pub fn tree(&self, clock: Clock) -> Vec<SidebarGroup<'_>> {
        self.window.tree(&self.daemon, clock)
    }

    pub fn visible_ids(&self, clock: Clock) -> Vec<SessionId> {
        self.window.visible_ids(&self.daemon, clock)
    }

    pub fn attention_count(&self, clock: Clock) -> usize {
        self.window.attention_count(&self.daemon, clock)
    }

    pub fn attention_count_of(&self, tree: &[SidebarGroup<'_>], clock: Clock) -> usize {
        self.window.attention_count_of(&self.daemon, tree, clock)
    }

    pub fn attention_target(&self, clock: Clock, direction: Direction) -> Option<SessionId> {
        self.window.attention_target(&self.daemon, clock, direction)
    }

    pub fn step_target(&self, clock: Clock, direction: Direction) -> Option<SessionId> {
        self.window.step_target(&self.daemon, clock, direction)
    }

    /// [`WindowState::filter_matched_nothing_in`], building the tree first.
    ///
    /// For callers that do not already have one. The sidebar does, and uses
    /// the other form so it does not pay for a second traversal per paint.
    /// Test-only: the sidebar has already built its tree and uses the other
    /// form. Kept because the tests that prove a failed search is not an empty
    /// server read more clearly without threading a tree through.
    #[cfg(test)]
    #[must_use]
    pub fn filter_matched_nothing(&self, clock: Clock) -> bool {
        self.window
            .filter_matched_nothing_in(self.tree(clock).is_empty())
    }

    pub fn menu_targets(&self, target: SessionId, clock: Clock) -> Vec<SessionId> {
        self.window.menu_targets(&self.daemon, target, clock)
    }

    pub fn menu_items(&self, target: SessionId, clock: Clock) -> Vec<MenuItem> {
        self.window.menu_items(&self.daemon, target, clock)
    }

    pub fn open(&mut self, id: SessionId, now_ms: u64) {
        self.window.open(&mut self.daemon, id, now_ms);
    }

    pub fn close_tab(&mut self, id: SessionId) {
        self.window.close_tab(id);
    }

    pub fn close_other_tabs(&mut self, keep: SessionId) {
        self.window.close_other_tabs(keep);
    }

    pub fn cycle(&mut self, delta: isize) {
        self.window.cycle(delta);
    }

    pub fn focus_index(&mut self, i: usize) {
        self.window.focus_index(i);
    }

    pub fn reveal(&mut self, id: SessionId, clock: Clock) {
        self.window.reveal(&self.daemon, id, clock);
    }

    pub fn click_row(&mut self, id: SessionId, click: Click, clock: Clock) {
        self.window.click_row(&self.daemon, id, click, clock);
    }

    pub fn select_all_visible(&mut self, clock: Clock) {
        self.window.select_all_visible(&self.daemon, clock);
    }

    pub fn snooze(&mut self, ids: &[SessionId], wake_at_ms: u64, now_ms: u64) -> usize {
        self.daemon.snooze(ids, wake_at_ms, now_ms)
    }

    pub fn wake(&mut self, ids: &[SessionId], now_ms: u64) {
        self.daemon.wake(ids, now_ms);
    }

    pub fn settle(&mut self, ids: &[SessionId], now_ms: u64) -> usize {
        self.daemon.settle(ids, now_ms)
    }

    pub fn unsettle(&mut self, ids: &[SessionId]) {
        self.daemon.unsettle(ids);
    }

    pub fn mark_seen(&mut self, ids: &[SessionId], now_ms: u64) {
        self.daemon.mark_seen(ids, now_ms);
    }

    pub fn mark_unseen(&mut self, ids: &[SessionId]) {
        self.daemon.mark_unseen(ids);
    }

    pub fn section_open(&self, key: GroupKey, section: Section) -> bool {
        self.window.section_open(key, section)
    }

    pub fn toggle_section(&mut self, key: GroupKey, section: Section) {
        self.window.toggle_section(key, section);
    }

    pub fn toggle_preview(&mut self, key: GroupKey) {
        self.window.toggle_preview(key);
    }

    /// Point this window at another workspace.
    pub fn set_workspace(&mut self, to: WorkspaceId, now_ms: u64) -> Result<(), WorkspaceError> {
        self.window.set_workspace(&mut self.daemon, to, now_ms)
    }

    /// Create a workspace. It starts blank; nothing moves into it by itself.
    pub fn create_workspace(&mut self, name: &str) -> Result<WorkspaceId, WorkspaceError> {
        self.daemon.workspaces.create(name)
    }

    /// Delete a workspace and repoint this window if it was the one showing.
    pub fn delete_workspace(&mut self, id: WorkspaceId, now_ms: u64) -> Result<(), WorkspaceError> {
        self.daemon.workspaces.delete(id)?;
        self.window.reconcile_workspace(&mut self.daemon, now_ms);
        Ok(())
    }

    pub fn move_to_workspace(
        &mut self,
        ids: &[SessionId],
        to: WorkspaceId,
        now_ms: u64,
    ) -> Result<usize, WorkspaceError> {
        let moved = self.daemon.move_to_workspace(ids, to)?;
        self.window.prune(&mut self.daemon, now_ms);
        Ok(moved)
    }

    pub fn move_to_folder(
        &mut self,
        ids: &[SessionId],
        folder: Option<FolderId>,
    ) -> Result<usize, WorkspaceError> {
        self.daemon.move_to_folder(ids, folder)
    }

    /// Switch pages inside an already-open settings modal. A no-op when it is
    /// not open, so a stray shortcut cannot summon it sideways.
    pub fn set_settings_tab(&mut self, tab: SettingsTab) {
        if matches!(self.window.layer, Layer::Settings(_)) {
            self.window.layer = Layer::Settings(tab);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Persistence
// ═══════════════════════════════════════════════════════════════════════════

/// Format version written into the UI state file.
pub const UI_STATE_VERSION: u32 = 1;

/// File name under the platform config directory.
///
/// Config rather than state, because workspaces and settings are things the
/// operator authored and would expect to survive clearing a cache, unlike
/// window geometry which [`vitrum_os::window_state`] keeps separately.
pub const UI_STATE_FILE: &str = "ui.json";

/// One window's share of the persisted document.
///
/// `default` on the CONTAINER, matching [`Settings`], [`TerminalPrefs`],
/// [`NotifyPrefs`], [`KeyboardPrefs`] and [`Strip`]. Without it a field added
/// here is REQUIRED, so every `ui.json` written before the upgrade fails to
/// deserialize, [`parse_ui_state`] reports [`UiStateLoad::Corrupt`],
/// [`load_prefs`] answers with defaults and [`Persisted::restore_daemon`]
/// writes those defaults over the operator's workspaces, folders, placements
/// and layouts. One missing attribute between an added field and losing
/// everything the operator arranged, which is the exact failure
/// [`UiStateLoad`]'s five variants exist to prevent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WindowSnapshot {
    pub workspace: WorkspaceId,
    pub sidebar_width: f64,
    pub sidebar_collapsed: bool,
    pub workspace_bar_open: bool,
    /// The strip for `workspace`.
    pub strip: Strip,
    /// Strips for the workspaces this window was not showing, so switching
    /// back after a restart still finds what was open.
    pub parked: Vec<(WorkspaceId, Strip)>,
    /// The three scrollback-search switches.
    ///
    /// Not the query and not the hits. A query is the question being asked
    /// right now, and an answer is up to five hundred rows with their context
    /// lines; neither belongs in a profile. Taking only this field keeps them
    /// out by construction rather than by remembering to skip them, which is
    /// why [`SearchState`] is not persisted whole.
    pub search_options: crate::ui::search::Options,
}

impl Default for WindowSnapshot {
    fn default() -> Self {
        WindowSnapshot::of(&WindowState::default())
    }
}

/// Everything that survives a restart.
///
/// `default` on the container for the same reason as [`WindowSnapshot`]. The
/// version guard is unaffected: [`parse_ui_state`] reads `version` off the raw
/// JSON before this type is ever constructed, so a file with no version is
/// still refused by name rather than silently defaulted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Persisted {
    pub version: u32,
    pub settings: Settings,
    pub workspaces: WorkspaceSet,
    pub windows: Vec<WindowSnapshot>,
}

impl Default for Persisted {
    fn default() -> Self {
        Persisted {
            version: UI_STATE_VERSION,
            settings: Settings::default(),
            workspaces: WorkspaceSet::default(),
            windows: Vec::new(),
        }
    }
}

impl Persisted {
    /// Snapshot the parts of a running process worth keeping.
    /// Test-only. Production writes through [`save_prefs`], which re-reads the
    /// file, applies one window with `put_window` and writes back, so a second
    /// window cannot clobber the first's slot. This snapshots several windows
    /// at once, which no live path wants.
    #[cfg(test)]
    pub fn capture<'a>(
        daemon: &DaemonState,
        windows: impl IntoIterator<Item = &'a WindowState>,
    ) -> Self {
        let mut doc = Persisted {
            version: UI_STATE_VERSION,
            settings: daemon.settings.clone(),
            workspaces: daemon.workspaces.clone(),
            windows: Vec::new(),
        };
        for window in windows {
            doc.put_window(window);
        }
        doc
    }

    /// Put the daemon-scoped half back.
    pub fn restore_daemon(&self, daemon: &mut DaemonState) {
        daemon.settings = self.settings.clone();
        daemon.workspaces = self.workspaces.clone();
        daemon.workspaces.normalize();
    }

    /// Replace one window's entry, growing the list if this window has never
    /// saved before.
    ///
    /// Padding is a placeholder rather than a copy of `window`: a slot that
    /// belongs to a window which has not saved yet must not come back on the
    /// next launch wearing another window's tabs.
    pub fn put_window(&mut self, window: &WindowState) {
        let first = self.workspaces.first();
        while self.windows.len() < window.index {
            self.windows.push(WindowSnapshot::placeholder(first));
        }
        let snapshot = WindowSnapshot::of(window);
        match self.windows.get_mut(window.index) {
            Some(slot) => *slot = snapshot,
            None => self.windows.push(snapshot),
        }
    }

    /// Put one window's half back, returning false when the document has no
    /// entry for that window (a second window opened since the last save).
    ///
    /// Restored tabs are not trusted: the daemon may have been restarted and
    /// no longer list any of them. They survive until the first `Sessions`
    /// snapshot, which prunes whatever is gone.
    pub fn restore_window(&self, window: &mut WindowState) -> bool {
        let Some(snapshot) = self.windows.get(window.index) else {
            return false;
        };
        window.workspace = if self.workspaces.contains(snapshot.workspace) {
            snapshot.workspace
        } else {
            self.workspaces.first()
        };
        window.sidebar_width = snapshot.sidebar_width.clamp(SIDEBAR_MIN_PX, SIDEBAR_MAX_PX);
        window.sidebar_collapsed = snapshot.sidebar_collapsed;
        window.workspace_bar_open = snapshot.workspace_bar_open;
        window.search.options = snapshot.search_options;
        window.tabs = snapshot.strip.tabs.clone();
        window.tab_mru = snapshot.strip.tab_mru.clone();
        window.focused = snapshot.strip.focused;
        window.parked = snapshot
            .parked
            .iter()
            .filter(|(w, _)| self.workspaces.contains(*w) && *w != window.workspace)
            .cloned()
            .collect();
        true
    }
}

impl WindowSnapshot {
    /// An entry for a window that exists but has never saved.
    fn placeholder(workspace: WorkspaceId) -> Self {
        WindowSnapshot {
            workspace,
            ..WindowSnapshot::default()
        }
    }

    fn of(window: &WindowState) -> Self {
        WindowSnapshot {
            workspace: window.workspace,
            sidebar_width: window.sidebar_width,
            sidebar_collapsed: window.sidebar_collapsed,
            workspace_bar_open: window.workspace_bar_open,
            strip: Strip {
                tabs: window.tabs.clone(),
                tab_mru: window.tab_mru.clone(),
                focused: window.focused,
            },
            parked: window.parked.iter().map(|(w, s)| (*w, s.clone())).collect(),
            search_options: window.search.options,
        }
    }
}

/// Outcome of reading the UI state file.
///
/// Five variants rather than `Option`, matching [`vitrum_os::window_state`]:
/// "the file is not there" is a first launch, and "the file is there and I
/// could not read it" is a bug or a permissions problem the operator should
/// see. Collapsing them into a silent default is how a product loses a user's
/// workspaces every launch without ever saying why.
#[derive(Debug, Clone, PartialEq)]
pub enum UiStateLoad {
    /// No file. First launch.
    Missing,
    /// Read, and repaired if it needed it.
    Loaded(Box<Persisted>),
    /// Present but not JSON, or not the right shape.
    Corrupt { detail: String },
    /// Present but written by a newer build.
    Unsupported { version: u32 },
    /// Present, and the read itself failed.
    Unreadable { detail: String },
}

impl UiStateLoad {
    /// The document, or defaults, plus the message worth showing when the file
    /// was there and unusable.
    pub fn or_default(self) -> (Persisted, Option<String>) {
        match self {
            UiStateLoad::Missing => (Persisted::default(), None),
            UiStateLoad::Loaded(doc) => (*doc, None),
            other => {
                let detail = other.to_string();
                (Persisted::default(), Some(detail))
            }
        }
    }
}

impl fmt::Display for UiStateLoad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiStateLoad::Missing => f.write_str("no saved workspaces"),
            UiStateLoad::Loaded(_) => f.write_str("loaded"),
            UiStateLoad::Corrupt { detail } => write!(f, "workspace file is corrupt: {detail}"),
            UiStateLoad::Unsupported { version } => write!(
                f,
                "workspace file is version {version}, this build understands {UI_STATE_VERSION}"
            ),
            UiStateLoad::Unreadable { detail } => {
                write!(f, "workspace file could not be read: {detail}")
            }
        }
    }
}

/// Where the UI state file lives on this platform.
pub fn ui_state_path() -> Result<PathBuf, PathError> {
    Ok(AppPaths::for_current_platform()?
        .config_dir
        .join(UI_STATE_FILE))
}

/// Serialise exactly as [`save_ui_state`] writes it.
pub fn encode_ui_state(doc: &Persisted) -> String {
    // Pretty rather than compact: this file is under the config directory and
    // an operator who opens it should be able to read it.
    serde_json::to_string_pretty(doc).unwrap_or_else(|_| "{}".to_string())
}

/// Parse file contents, repairing what can be repaired.
pub fn parse_ui_state(text: &str) -> UiStateLoad {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            return UiStateLoad::Corrupt {
                detail: e.to_string(),
            };
        }
    };
    // Version first, so a newer file reports its version rather than a pile of
    // unknown-field errors that say nothing useful.
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(v) if v == UI_STATE_VERSION as u64 => {}
        // Saturating, not truncating: `4294967297 as u32` is 1, so a file
        // claiming that version reported "version 1, this build understands 1"
        // and told the operator nothing.
        Some(v) => {
            return UiStateLoad::Unsupported {
                version: u32::try_from(v).unwrap_or(u32::MAX),
            };
        }
        None => {
            return UiStateLoad::Corrupt {
                detail: "no version field".to_string(),
            };
        }
    }
    match serde_json::from_value::<Persisted>(value) {
        Ok(mut doc) => {
            doc.workspaces.normalize();
            // Repaired here rather than at the controls, because this is the
            // path a hand-edited file takes and the controls are not on it.
            doc.settings.appearance.clamp();
            UiStateLoad::Loaded(Box::new(doc))
        }
        Err(e) => UiStateLoad::Corrupt {
            detail: e.to_string(),
        },
    }
}

/// Read the UI state file. Never panics, never silently defaults.
pub fn load_ui_state(path: &Path) -> UiStateLoad {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_ui_state(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => UiStateLoad::Missing,
        Err(e) => UiStateLoad::Unreadable {
            detail: e.to_string(),
        },
    }
}

/// Write the UI state file atomically.
///
/// Write-then-rename because the alternative loses the operator's workspaces
/// whenever the machine dies mid-write, and a truncated JSON file reads back
/// as [`UiStateLoad::Corrupt`] on every subsequent launch until someone
/// deletes it.
pub fn save_ui_state(path: &Path, doc: &Persisted) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, encode_ui_state(doc))?;
    std::fs::rename(&tmp, path)
}

/// Read the saved workspaces and settings at startup.
///
/// Best effort by design: a missing, corrupt or future-version file gives
/// defaults plus a reason, and the reason is returned rather than swallowed so
/// the caller can flash it. The caller then applies the document with
/// [`Persisted::restore_daemon`] and [`Persisted::restore_window`].
pub fn load_prefs() -> (Persisted, Option<String>) {
    match ui_state_path() {
        Ok(path) => load_ui_state(&path).or_default(),
        Err(e) => (Persisted::default(), Some(e.to_string())),
    }
}

/// Write one window's slice of the document, keeping every other window's.
///
/// Each desktop window has its own VirtualDom and therefore its own
/// [`UiState`], so no window can see another's layout. A window that captured
/// the whole document would write itself into slot 0 and silently delete every
/// other window's entry, which is a layout the operator loses on the next
/// restart with nothing on screen to say why. This reads what is on disk,
/// replaces [`WindowState::index`], and writes the result back atomically.
///
/// The daemon half is written from this window's copy, which is correct
/// because every window folds the same broadcasts from the same daemon and so
/// agrees about workspaces and settings; last writer wins and they are all
/// writing the same thing.
///
/// Returns the failure rather than logging it, because this file has no logger
/// and the caller has both a `tracing` span and a flash strip.
pub fn save_prefs(daemon: &DaemonState, window: &WindowState) -> Result<(), String> {
    let path = ui_state_path().map_err(|e| e.to_string())?;
    let mut doc = match load_ui_state(&path) {
        UiStateLoad::Loaded(doc) => *doc,
        // A missing, corrupt or future-version file is not a reason to refuse
        // to save: the operator's current layout is the better of the two.
        _ => Persisted::default(),
    };
    doc.settings = daemon.settings.clone();
    doc.workspaces = daemon.workspaces.clone();
    doc.put_window(window);
    save_ui_state(&path, &doc).map_err(|e| e.to_string())
}

/// Which gesture produced a click on a sidebar row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// Plain click: replace the selection and re-anchor.
    Plain,
    /// Ctrl or Cmd: add or remove one row.
    Toggle,
    /// Shift: select the range from the anchor.
    Range,
    /// Ctrl and Shift: union the anchored range into what is already selected.
    RangeAdditive,
}

/// A one-line message in the strip above the terminal.
///
/// One strip, two kinds. A copy confirmation and a spawn failure are both
/// "something just happened that you did not see on screen", and giving them
/// separate strips means the window grows a row for good news.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flash {
    pub kind: FlashKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashKind {
    /// Something failed. Red.
    Error,
    /// Something succeeded, or did nothing for a reason worth saying.
    Notice,
}

impl Flash {
    pub fn error(text: impl Into<String>) -> Self {
        Flash {
            kind: FlashKind::Error,
            text: text.into(),
        }
    }

    pub fn notice(text: impl Into<String>) -> Self {
        Flash {
            kind: FlashKind::Notice,
            text: text.into(),
        }
    }

    pub fn class(&self) -> &'static str {
        match self.kind {
            FlashKind::Error => "rg-flash rg-flash--error",
            FlashKind::Notice => "rg-flash rg-flash--notice",
        }
    }
}

/// The one transient layer that can be open over the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum Layer {
    None,
    /// The keyboard reference.
    Shortcuts,
    /// A right-click menu on a session row.
    Menu(MenuState),
    /// The new-session dialog, seeded from wherever it was opened.
    NewSession(NewSessionSeed),
    /// The settings modal, on one of its pages.
    Settings(SettingsTab),
    /// The rename dialog for one session.
    Rename(RenameSeed),
    /// Cross-session scrollback search.
    ///
    /// A unit variant, deliberately. The answer arrives long after the chord
    /// and has to outlive several re-renders, and a [`Layer`] is compared and
    /// cloned on every window paint: up to five hundred `SearchHit`s carried
    /// inside the enum would be five hundred clones a frame. The answer lives
    /// in [`WindowState::search`] instead, which the surface reads from.
    Search,
    /// The first-run sheet. Opened once on a fresh profile and never again,
    /// however it is closed.
    Onboarding,
    /// The release notes for versions installed since the last time this
    /// profile saw them.
    WhatsNew,
}

impl Layer {
    pub fn is_open(&self) -> bool {
        !matches!(self, Layer::None)
    }
}

/// The scrollback search surface's live state.
///
/// What is in the field, the three switches the protocol carries, whether a
/// sweep is in flight, and the last answer. `answer: None` means nothing has
/// been swept, which the summary renders differently from a sweep that found
/// nothing — a distinction that only exists because both are otherwise "no
/// hits on screen".
///
/// [`crate::ui::search::Options`] and [`crate::ui::search::Answer`] are reused
/// rather than redeclared. Both are plain data — no Dioxus type reaches this
/// file through them — and a second declaration of the same four fields would
/// be one more thing to keep in agreement with the wire.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchState {
    pub query: String,
    pub options: crate::ui::search::Options,
    pub searching: bool,
    pub answer: Option<crate::ui::search::Answer>,
    /// Sessions the last sweep was restricted to, empty for every session.
    ///
    /// Kept so the summary can say what was searched. A count of hits with no
    /// statement of scope is the same number for "three of twenty" and "three
    /// of three", which are different answers.
    pub scope: Vec<SessionId>,
}

/// Where a context menu was opened, and on what.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuState {
    /// Viewport coordinates of the click, in CSS pixels.
    pub x: f64,
    pub y: f64,
    pub target: SessionId,
}

/// Starting values for the new-session dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionSeed {
    /// Pre-selected project, or `None` when the daemon knows no projects yet
    /// and the user is about to define the first one by picking a directory.
    pub project: Option<ProjectId>,
    pub cwd: String,
}

/// Starting values for the rename dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameSeed {
    pub session: SessionId,
    pub title: String,
}

/// One entry in a context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Focus,
    CloseTab,
    CloseOthers,
    /// The "Snooze until" caption. Not clickable itself; the presets under it
    /// are. It exists as an entry rather than a bare label so it can be
    /// disabled and carry the reason when the row is blocked on the operator.
    SnoozeHeader,
    /// Park until the preset's wake instant.
    Snooze(SnoozePresetId),
    Wake,
    Settle,
    Unsettle,
    /// Stamp the row as looked at, clearing this window's unseen markers.
    MarkRead,
    /// Forget the last visit, re-arming this window's unseen markers.
    MarkUnread,
    Rename,
    CopyPath,
    CopyBranch,
    CopyCommand,
    NewSessionHere,
    /// Start a second session with this one's command, directory and title.
    Duplicate,
    /// The "Move to workspace" caption. Not clickable; the workspaces under
    /// it are.
    MoveToWorkspaceHeader,
    /// File the targets into another workspace.
    MoveToWorkspace(WorkspaceId),
    /// The "Move to folder" caption, in named grouping only.
    MoveToFolderHeader,
    /// File the targets into a named folder, or out of every folder.
    MoveToFolder(Option<FolderId>),
    Terminate,
}

impl MenuAction {
    /// True for an entry that only captions the ones under it.
    pub fn is_caption(self) -> bool {
        matches!(
            self,
            MenuAction::SnoozeHeader
                | MenuAction::MoveToWorkspaceHeader
                | MenuAction::MoveToFolderHeader
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub action: MenuAction,
    /// Owned rather than static because the model's labels carry counts and
    /// the snooze presets carry wall-clock times.
    pub label: String,
    /// Right-aligned secondary text: a preset's resulting time, or the reason
    /// a disabled entry is disabled. An action that greys out with no
    /// explanation teaches nothing and reads as a bug.
    pub hint: Option<String>,
    /// Disabled entries stay visible and unclickable. Hiding them instead
    /// makes the menu change height between openings, so the entry you want is
    /// never in the same place twice.
    pub enabled: bool,
    /// Destructive. Rendered in the failure colour.
    pub danger: bool,
    /// Draw a rule above this entry.
    pub sep_before: bool,
}

impl MenuItem {
    fn new(action: MenuAction, label: impl Into<String>) -> Self {
        MenuItem {
            action,
            label: label.into(),
            hint: None,
            enabled: true,
            danger: false,
            sep_before: false,
        }
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn enable(mut self, yes: bool) -> Self {
        self.enabled = yes;
        self
    }

    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    fn sep(mut self) -> Self {
        self.sep_before = true;
        self
    }
}

/// Does this session match the sidebar filter?
///
/// `query` must already be trimmed and lowercased; an empty query matches
/// everything. Title, command and branch are searched because those are the
/// three things a user actually remembers about a session, and cwd because at
/// twenty agents several sessions share a title but not a directory.
fn matches_filter(s: &SessionInfo, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    s.title.to_lowercase().contains(query)
        || s.command.to_lowercase().contains(query)
        || s.cwd.to_lowercase().contains(query)
        || s.git_branch
            .as_deref()
            .is_some_and(|b| b.to_lowercase().contains(query))
}

/// Attention rail modifier for a session row, or `None` when the session is
/// working and needs nobody.
///
/// The rail is the row's own left border, not a badge: the sidebar's horizontal
/// budget at 14rem does not have 14px to spare for a second marker beside the
/// unread dot. Exactly one tier is ever returned, taken from the server's
/// priority ladder, so two rails can never contend.
///
/// Discrete, never animated. A pulsing "needs you" indicator is a repaint of
/// the whole window at the refresh rate, forever, for a fact that changes a few
/// times an hour, and it is at its worst exactly when the most rows are lit.
pub fn attention_modifier(a: &Attention) -> Option<&'static str> {
    if a.failed {
        Some("rg-session--attention-failed")
    } else if a.waiting == Some(true) {
        Some("rg-session--attention-waiting")
    } else if a.bell {
        Some("rg-session--attention-bell")
    } else if a.idle_ms >= IDLE_ATTENTION_MS {
        Some("rg-session--attention-idle")
    } else {
        None
    }
}

/// Tooltip text explaining why a row is marked.
///
/// The waiting tier names what was observed, not a conclusion. "Blocked
/// reading input" is a fact about a syscall; "waiting for your approval" would
/// be a guess about intent that only the agent can make, and the shell does
/// not make it.
pub fn attention_label(a: &Attention) -> String {
    if a.failed {
        "failed - needs you".to_string()
    } else if a.waiting == Some(true) {
        "blocked reading input - needs you".to_string()
    } else if a.bell {
        "rang the bell - needs you".to_string()
    } else {
        let idle = vitrum_fmt::duration::terse(core::time::Duration::from_millis(a.idle_ms));
        format!("silent for {idle} - needs you")
    }
}

/// What the row can say about whether the agent is blocked.
///
/// `waiting == None` means this daemon's platform cannot answer the question,
/// which is Windows today. The row says so rather than implying the agent is
/// working, because a shell that silently reports "working" for every blocked
/// Windows session is worse than one that admits the gap.
pub fn waiting_note(a: &Attention) -> Option<&'static str> {
    match a.waiting {
        Some(true) => Some("blocked reading input"),
        Some(false) => None,
        None => Some("this platform cannot tell whether the agent is blocked"),
    }
}

/// Human-readable status for a row's `title` tooltip.
///
/// Exit text comes from [`vitrum_fmt::exit`] so the client, the daemon logs and
/// anything else that reports a termination agree on the wording.
///
/// The protocol carries `Option<i32>` and nothing else, so a signalled child
/// arrives as `None` with the signal number already discarded by the daemon.
/// The label says exactly that rather than the bare word "signalled", which
/// left an operator unable to tell a Ctrl+C from an out-of-memory kill.
pub fn status_label(status: &SessionStatus) -> String {
    match status {
        SessionStatus::Starting => "starting".to_string(),
        SessionStatus::Running => "running".to_string(),
        SessionStatus::Exited { code: Some(c) } => vitrum_fmt::exit::describe_code(*c),
        SessionStatus::Exited { code: None } => {
            "killed by a signal (the number is not carried on the wire)".to_string()
        }
    }
}

#[cfg(test)]
mod tests;

/// A flash's LIFETIME, which is the difference between a confirmation and a
/// banner.
///
/// `Flash` shipped with no expiry of any kind. A notice was cleared only by an
/// explicit Dismiss click or by another flash overwriting it, so on a real
/// window "Started bash in tmp. Ctrl+Shift+X stops it." was still occupying a
/// full-width band above the terminal twenty-nine minutes after the session
/// started. Nothing in the type or in any test said a notice was supposed to
/// be temporary, which is why it never was.
///
/// The retirement itself is a one-shot in `main.rs`, because the model has no
/// clock. What is provable here is the part that decides WHICH flashes retire,
/// and that the two kinds are distinguishable at all.
#[cfg(test)]
mod flash_lifetime;

/// What the model does when the daemon or `ui.json` hands it nonsense.
#[cfg(test)]
mod hardening;
