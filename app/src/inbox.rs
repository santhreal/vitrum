//! Everything the sidebar renders, derived by `vitrum-model` and dressed for
//! the widgets here.
//!
//! This module owns no rules. Status precedence, disposition, ordering,
//! sectioning, rollups, wake labels and traversal all come out of
//! [`vitrum_model`]; what lives here is the mapping from those answers to class
//! names, glyphs and words, plus the one place the client's own clock is
//! converted into the model's.
//!
//! The split matters because the rules are the part with edge cases. A UI that
//! re-derives "is this row settled" from `SessionStatus` beside a crate that
//! already answers it will disagree with that crate the first time a snooze
//! elapses, and the disagreement is invisible until an operator loses a row.
//!
//! # Three channels on a row, and only three
//!
//! - The **pill** is [`SidebarStatus`] plus its [`StatusSource`]: what the agent
//!   is doing, and how sure we are. It sits in the fixed left column where the
//!   status dot used to, because a marker at the same x on every row is
//!   scannable down twenty rows and a marker after a variable-length title is
//!   not.
//! - The **left rail** is `vitrum_proto::Attention`, owned by `sidebar.css`.
//! - The **unread dot** is the daemon's `unread` flag.
//!
//! [`SessionView::has_unseen_completion`] is a fourth fact but not a fourth
//! channel: it rides on line two as a badge, because it is the one indicator
//! that answers "did something finish while I was away", which is a different
//! question from "is there output I have not read".

use std::borrow::Cow;

use vitrum_fmt::TimeFormat;
use vitrum_model::{
    Clock, Disposition, DispositionPolicy, ProjectRollup, Section, SessionView, SidebarStatus,
    StatusSource,
    order::{compare_active, compare_settled, compare_snoozed},
    rollup::rollup_rows,
    snooze::{wake_countdown_label, wake_description},
    tree::preview_sessions,
};
use vitrum_proto::{ProjectId, ProjectInfo, SessionId, SessionInfo};

/// Active rows one project shows before the "show all" affordance, on a fresh
/// profile.
///
/// Eight is the tab strip's budget too, and for the same reason: past that a
/// list stops being scannable and starts needing a search. A project over the
/// limit keeps its focused row visible whatever its position, so the row you
/// are looking at can never be the one behind the affordance.
///
/// The number a paint uses comes from `settings.inbox.previewRows`, which
/// defaults to this. Kept as a constant because the catalogue's default is
/// pinned against it, so retuning one without the other turns the suite red.
#[cfg(test)]
pub const PREVIEW_LIMIT: usize = 8;

/// Rows the Done shelf shows before the "Show more" affordance, on a fresh
/// profile.
///
/// The Active band has had [`PREVIEW_LIMIT`] since the beginning and the Done
/// band had nothing, so it was the one band in the sidebar whose row count was
/// unbounded: every session an operator has ever finished in a bucket is still
/// a row, a comparator and a widget on every paint once the shelf is open.
/// A month of work in one project is not a list, and it is not what anyone
/// opens the shelf to look for.
///
/// Ten, because the shelf answers one question — "what did I just finish" —
/// and the answer is always near the top of a band sorted by when the work
/// ended. Anything older is an archive lookup, which is what the filter is
/// for.
///
/// Whether the cut is undone is a READING gesture and not a preference, which
/// is why the bit that undoes it lives in `WindowState::settled_expanded`
/// beside the Active band's and is deliberately not persisted: coming back to
/// a collapsed tail is the safe default, and the two cuts stay symmetric. How
/// deep the cut is IS a preference, and comes from
/// `settings.inbox.settledRows`.
#[cfg(test)]
pub const SETTLED_TAIL_LIMIT: usize = 10;

/// The render tick's clock, in the model's terms.
///
/// One conversion point. Two call sites reading the system clock independently
/// could straddle a wake instant and render a row as both snoozed and woken in
/// the same paint.
pub fn model_clock(clock: TimeFormat) -> Clock {
    Clock {
        now_ms: clock.now().as_millis().max(0) as u64,
        utc_offset_seconds: clock.utc_offset_secs(),
    }
}

// The sidebar emits NO status character. There is no `status_icon`.
//
// There were five, one per [`SidebarStatus`], and they were the "letter
// icons" complaint: two of them were literally punctuation, `!` and `?`, and
// the other three were a geometric arrow and two dingbats. Measured at the
// shipped 10px inside the shipped 16px box they spanned 6.2x in ink width
// and 1.5x in ink height, so a column of twenty rows had four different mark
// widths and the gap between a mark and its word moved by 3.3px depending on
// which state a row was in.
//
// Picking better characters does not fix that, and this is the part worth
// keeping: a text glyph's painted box is its ADVANCE, which carries side
// bearings this code does not control and which change with the font. A
// declared `gap: 4px` paints as something between 7.36px and 11.34px and
// would still paint as not-4px with a prettier glyph. A mark drawn by the
// stylesheet has the box it declares, in every font, on every platform.
//
// So the mark is `.rg-pill::before`, drawn from the `rg-pill--*` modifier
// [`status_modifier`] already puts on the pill: one 8px dot, in the state's
// hue, painted for `Working` in the expanded panel because Working is the
// only transient state, and painted for every state in the 3rem rail because
// the word is hidden there and the hue is all that is left. No node, no
// character, no per-state geometry.
//
// The rule for the whole sidebar is one sentence: **a state is a hue, and
// the only state that also gets a mark is Working.** T3 Code arrives at the
// same place from the other direction: `SidebarV2.tsx:496`, `:502` and
// `:508` set `icon: null` for approval, input and failed, and only working,
// woke and done carry one.

/// Class modifier for one status. Paired with `rg-pill` in [`Pill::class`].
pub fn status_modifier(status: SidebarStatus) -> &'static str {
    match status {
        SidebarStatus::Approval => "rg-pill--approval",
        SidebarStatus::Input => "rg-pill--input",
        SidebarStatus::Working => "rg-pill--working",
        SidebarStatus::Failed => "rg-pill--failed",
        SidebarStatus::Ready => "rg-pill--ready",
    }
}

/// Every short word the sidebar uses to name a row's state.
///
/// One vocabulary, seven words, one function that produces them. Five come
/// from [`SidebarStatus`] and two from [`Disposition`], and they belong
/// together because the operator reads them in the same column: a completion
/// badge captioned "Finished" sitting above a shelf captioned "Done" reads as
/// two states where there is one. Everything that names a state on screen
/// goes through [`status_word`], so a synonym cannot creep in from a second
/// file.
///
/// The words are short on purpose. [`SidebarStatus::label`] says "Needs
/// approval", which is the right sentence for a tooltip and the wrong one for
/// a slot that also has to hold a close button; the long form stays on the
/// pill's `title`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateWord {
    Approval,
    Input,
    Working,
    Failed,
    Ready,
    /// Came back from a snooze and has not been opened since.
    Woke,
    /// Drained: the operator is finished with it.
    Done,
}

impl StateWord {
    /// The word a resolved status is named by.
    pub fn of(status: SidebarStatus) -> Self {
        match status {
            SidebarStatus::Approval => StateWord::Approval,
            SidebarStatus::Input => StateWord::Input,
            SidebarStatus::Working => StateWord::Working,
            SidebarStatus::Failed => StateWord::Failed,
            SidebarStatus::Ready => StateWord::Ready,
        }
    }
}

/// The one place a state becomes a word.
pub fn status_word(state: StateWord) -> &'static str {
    match state {
        StateWord::Approval => "Approval",
        StateWord::Input => "Input",
        StateWord::Working => "Working",
        StateWord::Failed => "Failed",
        StateWord::Ready => "Ready",
        StateWord::Woke => "Woke",
        StateWord::Done => "Done",
    }
}

/// How many characters a state word may need.
///
/// Derived from the vocabulary rather than written down, so a longer word
/// widens the reservation the day it is added instead of being clipped by a
/// constant nobody revisited. The pill holds this width whatever it currently
/// says: `Ready` becoming `Approval` must not move the title beside it.
pub fn state_word_chars() -> u16 {
    ALL_STATE_WORDS
        .iter()
        .map(|state| status_word(*state).chars().count() as u16)
        .max()
        .unwrap_or(0)
}

/// Every word in the vocabulary.
///
/// Kept honest by `every_state_word_is_in_the_roster`, whose exhaustive match
/// stops compiling when a variant is added and left out.
pub const ALL_STATE_WORDS: [StateWord; 7] = [
    StateWord::Approval,
    StateWord::Input,
    StateWord::Working,
    StateWord::Failed,
    StateWord::Ready,
    StateWord::Woke,
    StateWord::Done,
];

// ═══════════════════════════════════════════════════════════════════════════
// Project identity
// ═══════════════════════════════════════════════════════════════════════════

/// FNV-1a over a byte string.
///
/// Every derived key in the client comes through here: the project id a root
/// maps to, the directory bucket's key, the shape a project is drawn with.
/// Not for security, only for spreading short similar strings like
/// `/src/vitrum` and `/src/vitrum-web` apart, which a sum or a length would
/// not do. It is six lines and needs no dependency, which is why there is no
/// hasher crate here.
#[must_use]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The one key a directory has, whatever a client spelled it.
///
/// A project IS a directory, so two clients naming one directory must land on
/// one project. The daemon's protocol has no "create project" message: the
/// client mints the id and the daemon records it on first use, so nothing
/// stops four clients from minting four ids for one root — and four ids
/// produced four sidebar groups all called `vitrum`, each holding one session.
/// This is the function that makes them one.
///
/// The operating system is asked first, and its answer is not second-guessed.
/// [`std::fs::canonicalize`] resolves every symlink, removes `.` and `..`,
/// removes trailing separators, and on a case-insensitive volume returns the
/// ON-DISK case — so `/src/Vitrum` and `/src/vitrum` come back as the same
/// string on macOS and Windows and as two different strings on a
/// case-SENSITIVE volume, which is the correct answer in both cases and one
/// no amount of string folding can work out by itself.
///
/// A path that does not exist gets the hand-rolled normalisation instead:
/// trailing separators trimmed, `/` unified to `\` on Windows, and the whole
/// thing lowercased on the two platforms whose filesystems ignore case by
/// default. That branch is a best guess by construction, because there is no
/// filesystem to ask. It keeps the trimmed text rather than erroring, because
/// the new-session dialog echoes this string back to the operator and losing
/// what they typed is worse than a slightly wrong grouping key.
#[must_use]
pub fn project_key(path: &str) -> String {
    let trimmed = path.trim();
    match std::fs::canonicalize(trimmed) {
        Ok(resolved) => strip_verbatim(&resolved.to_string_lossy()).to_string(),
        Err(_) => fold_case(trim_separators(&unify_separators(strip_verbatim(trimmed)))),
    }
}

/// Drop Windows' `\\?\` verbatim prefix.
///
/// `canonicalize` returns one and a client that did not canonicalise does not,
/// so the two spellings of one directory would key apart on exactly the
/// platform this function exists for.
fn strip_verbatim(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

/// Both separators mean the same thing on Windows and only there.
///
/// `\` is a legal character in a Linux filename, so folding it would merge two
/// genuinely different directories.
fn unify_separators(path: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(windows) && path.contains('/') {
        std::borrow::Cow::Owned(path.replace('/', "\\"))
    } else {
        std::borrow::Cow::Borrowed(path)
    }
}

/// Trim trailing separators without eating a root.
fn trim_separators(path: &str) -> &str {
    let cut = path.trim_end_matches(['/', '\\']);
    if cut.len() == path.len() {
        return path;
    }
    if cut.is_empty() {
        // The root IS a separator: `/` on Unix, `\` on a UNC-less Windows path.
        return &path[..1];
    }
    if cut.ends_with(':') {
        // `C:` is the drive's CURRENT directory and `C:\` is the drive root.
        // They are different places, so this separator is not decoration.
        return &path[..cut.len() + 1];
    }
    cut
}

/// Lowercase a key on the platforms whose filesystems ignore case.
///
/// Only reached for a path that does not exist, where there is no volume to
/// ask. Unicode lowering is not NTFS's or HFS+'s case-folding table, but it
/// agrees with them on every ASCII path and it is applied to both sides of
/// every comparison, so two spellings still meet.
fn fold_case(path: &str) -> String {
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        path.to_lowercase()
    } else {
        path.to_string()
    }
}

/// One directory, and the project record the sidebar draws it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootedProject<'a> {
    /// The canonical directory, from [`project_key`].
    pub key: String,
    /// The id this directory always maps to, whatever ids the daemon was
    /// handed for it. Derived from `key`, so it survives a restart, a second
    /// window and a client that minted its own.
    pub id: ProjectId,
    /// The record the header takes its name and root from: the lowest daemon
    /// id for this directory, because the daemon lists projects in id order
    /// and a header must not be renamed by a second client registering the
    /// directory a second time.
    pub lead: &'a ProjectInfo,
    /// Where [`RootedProject::lead`] sits in the list this was folded from.
    ///
    /// Carried rather than recovered, because the only way to recover it from
    /// the reference is a `ptr::eq` scan of the whole list per group, which is
    /// quadratic and has to `expect` on a case the fold makes unreachable.
    pub lead_at: usize,
}

/// The daemon's project list, folded so one directory is one project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RootedProjects<'a> {
    groups: Vec<RootedProject<'a>>,
    /// Every daemon id, ascending, paired with its group's index. One
    /// allocation for the whole mapping rather than a `Vec` per group, because
    /// this is rebuilt on every paint.
    index: Vec<(ProjectId, usize)>,
}

impl<'a> RootedProjects<'a> {
    /// The folded projects, in the daemon's order of first appearance.
    pub fn groups(&self) -> &[RootedProject<'a>] {
        &self.groups
    }

    /// Which group a daemon project id belongs to.
    pub fn group_of(&self, id: ProjectId) -> Option<usize> {
        self.index
            .binary_search_by_key(&id, |(id, _)| *id)
            .ok()
            .map(|at| self.index[at].1)
    }
}

/// Fold a daemon project list by canonical root.
///
/// Costs one [`project_key`] per project, which is one `realpath` each:
/// measured at 0.8us for `/tmp` and 4.9us for a six-component path on this
/// machine's filesystem. With the handful of projects a window has open that
/// is a rounding error against the cost of rebuilding the rows a single
/// `SessionUpdated` touches, and nothing calls it while the window idles.
#[must_use]
pub fn coalesce_projects(projects: &[ProjectInfo]) -> RootedProjects<'_> {
    let mut groups: Vec<RootedProject> = Vec::new();
    let mut index: Vec<(ProjectId, usize)> = Vec::with_capacity(projects.len());
    for (at, project) in projects.iter().enumerate() {
        let key = project_key(&project.root);
        let group = match groups.iter().position(|group| group.key == key) {
            Some(group) => group,
            None => {
                groups.push(RootedProject {
                    id: ProjectId(fnv1a(key.as_bytes())),
                    key,
                    lead: project,
                    lead_at: at,
                });
                groups.len() - 1
            }
        };
        index.push((project.id, group));
    }
    index.sort_unstable_by_key(|(id, _)| *id);
    RootedProjects { groups, index }
}

// ═══════════════════════════════════════════════════════════════════════════
// Row titles
// ═══════════════════════════════════════════════════════════════════════════

/// The file name of a command, which is what the daemon defaults a title to.
///
/// Deliberately identical to `vitrum_core::session::default_title`, and that
/// duplication is the price of not putting a tenth field on [`SessionInfo`].
/// The alternative is a wire flag saying "this title was generated", which
/// means nine construction sites across six files and a protocol change while
/// three lanes are open in these files; the cost of being wrong here is that
/// a session an operator deliberately renamed to exactly its own command name
/// gets a disambiguator it did not need, which still leaves the row readable
/// and unique.
#[must_use]
pub fn command_name(command: &str) -> &str {
    std::path::Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(command)
}

/// What one row calls itself.
///
/// A title that does not tell one row from another is not a title. The daemon
/// names an unlabelled session after its command, so sixty real sessions in
/// one directory produced fifty-seven rows reading `bash`: a list where the
/// only way to find the session you want is to click all of them.
///
/// So a title the daemon GENERATED gets the session id appended and a title a
/// person CHOSE is passed through untouched. The operator's label, from the
/// new-session dialog or from rename, always wins.
///
/// The id and not something friendlier, and this is the whole decision:
///
/// - It is unique by construction. Creation time is nicer to read and is NOT
///   unique — sixty sessions started by a script share a second — and a
///   "distinguishing" suffix that collides is the bug again with extra steps.
/// - It never changes. A position in the list ("bash 3") renumbers every row
///   below it the moment one session closes, so the handle an operator has
///   learned silently moves to a different session. That is the same defect a
///   mark derived from a project's index would have, and the reason the row
///   mark is derived from the root instead.
/// - It is what the rest of the product already calls a session. A
///   notification activates `session/7`; the row for it now reads `#7`.
///
/// AND A SESSION WITH NO TITLE AT ALL IS NAMED HERE TOO. `title` is a plain
/// string on the wire and the empty string is a legal value of it: the create
/// request carries `title: None` whenever the operator leaves the field
/// blank, and a daemon that has not yet observed a name sends `""` until it
/// does. Passed through, that draws a row whose name element is present, laid
/// out, and holding nothing — a line with an agent mark, a status pill and a
/// blank where the session's identity goes, on every row shape and in every
/// band. `ui/search.rs` already refused to do that and fell back to
/// `Session {n}`; the sidebar, the row menu and the notification title all
/// read this function instead, so all three drew the blank. The fallback goes
/// here rather than at those four call sites because this is the one place a
/// row's displayed name is decided.
///
/// Whitespace counts as absent. A name of three spaces is a blank row with
/// extra steps.
///
/// Borrowed for a chosen title, which is the case that costs nothing, and
/// allocated only for a generated or a missing one.
#[must_use]
pub fn row_title(info: &SessionInfo) -> Cow<'_, str> {
    if info.title.trim().is_empty() {
        return Cow::Owned(format!("Session #{}", info.id.0));
    }
    if info.title == command_name(&info.command) {
        Cow::Owned(format!("{} #{}", info.title, info.id.0))
    } else {
        Cow::Borrowed(&info.title)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The bucket the operator is in
// ═══════════════════════════════════════════════════════════════════════════

/// The session whose bucket the operator is working in, or `None`.
///
/// Two real signals and no third. The focused session is where the keystrokes
/// are going, so it is the answer whenever there is one. With nothing focused
/// — the window has just restored, or the focused session was closed — the
/// most recently touched tab is the last place the operator actually was.
/// Both are already persisted per window in `Strip`, so this needs no new
/// state and survives a restart for free.
///
/// Nothing here looks at activity, unread counts or status. A "current"
/// project chosen by which agent shouted last is a section that moves while
/// you read it, which is worse than no section.
#[must_use]
pub fn current_session(focused: Option<SessionId>, mru: &[SessionId]) -> Option<SessionId> {
    focused.or_else(|| mru.last().copied())
}

/// Move the bucket the operator is working in to the front.
///
/// Returns whether anything moved. A rotate rather than a sort: every other
/// bucket keeps its exact relative position, so the only row that can move
/// under the cursor is the one the operator just acted on. A comparator that
/// ranked "is current" highest would be re-run against a changing list and
/// would reorder the tail as a side effect.
pub fn pin_current<T>(groups: &mut [T], current: impl FnMut(&T) -> bool) -> bool {
    match groups.iter().position(current) {
        Some(0) | None => false,
        Some(at) => {
            groups[..=at].rotate_right(1);
            true
        }
    }
}

/// Where a resolved status came from, in words the operator can act on.
///
/// The inferred branch says the platform cannot answer rather than naming a
/// signal, because "inferred from output timing" is only useful if you already
/// know the shell probes the kernel elsewhere.
pub fn source_note(source: StatusSource) -> &'static str {
    match source {
        StatusSource::Exit => "the child process exited",
        StatusSource::Waiting => "the OS reports it blocked reading the terminal",
        StatusSource::Foreground => "the OS reports it running, not blocked on the terminal",
        StatusSource::Bell => "it rang the terminal bell",
        StatusSource::Idle => {
            "this platform cannot probe the child, so this is a guess from silence and may be wrong"
        }
        StatusSource::Output => {
            "this platform cannot probe the child, so this is a guess from recent output and may be wrong"
        }
        StatusSource::Hint => "the agent declared it",
        // Named as a reading of the agent's own words, because that is exactly
        // what it is and it is what tells the operator how to check it: the
        // banner is on screen, in the pane, above the prompt.
        StatusSource::Title => "the agent says so in its terminal title, which we matched",
    }
}

/// One row's status pill, fully resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pill {
    pub status: SidebarStatus,
    pub source: StatusSource,
    /// Full class list, `rg-pill` plus modifiers.
    pub class: String,
    /// The short word, from the one vocabulary in [`status_word`]. Static
    /// rather than owned: it comes from a closed set, and a twenty-row list
    /// that repaints on every daemon frame should not allocate seven strings
    /// to say seven words it already knows.
    pub word: &'static str,
    /// Tooltip and accessible name, naming the state and its provenance.
    pub title: String,
}

impl Pill {
    /// Resolve the pill for one row.
    ///
    /// A declared label replaces the generic word: an agent that says
    /// "Approve force-push to main?" has told you more than "Needs approval"
    /// ever will, and dropping it in favour of the enum would waste the one
    /// channel the hint protocol exists to open.
    pub fn of(row: &SessionView) -> Self {
        let resolution = row.resolve_status();
        let status = resolution.status;
        let source = resolution.source;
        let inferred = source.is_inferred();

        let mut class = String::from("rg-pill ");
        class.push_str(status_modifier(status));
        if inferred {
            class.push_str(" rg-pill--inferred");
        }

        let word = status_word(StateWord::of(status));

        let mut title = String::from(status.label());
        title.push_str(" \u{2014} ");
        title.push_str(source_note(source));
        if let Some(label) = row.hint_label() {
            title.push_str("\nagent says: ");
            title.push_str(label);
        }

        Pill {
            status,
            source,
            class,
            word,
            title,
        }
    }
}

/// The live duration of the turn that is running right now, or `None`.
///
/// The one number on a row that answers "has this agent been stuck for forty
/// minutes or did it start ten seconds ago", which is the question a
/// twenty-agent list makes unanswerable by any other means. Distinct from the
/// row's timestamp, which is when the agent last SPOKE: an agent that has been
/// silently computing for an hour has a fresh-looking timestamp and is exactly
/// the row you want to find.
///
/// Quiet by construction, and this is the whole reason it is a separate
/// function rather than a field on [`Pill`]. It exists only while a turn is
/// genuinely live: [`SessionView::working_since_ms`] returns `None` unless the
/// resolved status is `Working` AND the agent declared when the stretch began,
/// so a row at rest emits nothing at all and there is no element to reserve
/// width for. It also cannot animate: the string changes when the daemon
/// pushes a frame, which is the same paint the rest of the row already costs.
pub fn working_aux(row: &SessionView, clock: Clock) -> Option<String> {
    row.working_elapsed_ms(clock)
        .map(vitrum_model::format_duration_label)
}

/// A small labelled mark: a disposition, an unseen completion, or the return
/// ticket a parked row shows in place of its age.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub class: String,
    /// The mark, when this badge earns one.
    pub icon: Option<&'static str>,
    pub text: String,
    pub title: String,
}

/// The disposition badge, or `None` for a plain active row.
///
/// [`Disposition::Settled`] gets none either: every row under the Done head is
/// settled, so a badge on each of them is a badge on none of them. `Woke` and
/// `Snoozed` earn theirs because they sit among rows that are neither.
pub fn disposition_badge(
    row: &SessionView,
    clock: Clock,
    policy: DispositionPolicy,
) -> Option<Badge> {
    match row.disposition(clock, policy) {
        Disposition::Woke => {
            let woke_at_ms = row.woke_at(clock)?;
            Some(Badge {
                // `--pulse` is a one-shot: the row reappears exactly where it
                // was, because the inbox sort is deliberately static, so the
                // badge is the only thing that can announce the return.
                class: "rg-badge rg-badge--woke rg-badge--pulse".to_string(),
                icon: Some("\u{21bb}"),
                text: status_word(StateWord::Woke).to_string(),
                title: format!(
                    "Came back from a snooze at {}. Open it and this clears.",
                    wake_description(woke_at_ms, clock)
                ),
            })
        }
        // The countdown, and no mark. A parked row's whole message is the
        // number and the snooze hue; U+25D1 in front of "2h" is a second
        // thing to look at that says what the first thing already said. T3
        // renders theirs as bare text for the same reason
        // (`SidebarV2.tsx:837`).
        Disposition::Snoozed => {
            let snooze = row.snooze?;
            Some(Badge {
                class: "rg-badge rg-badge--snoozed".to_string(),
                icon: None,
                text: wake_countdown_label(snooze.wake_at_ms, clock.now_ms),
                title: format!(
                    "Parked until {}",
                    wake_description(snooze.wake_at_ms, clock)
                ),
            })
        }
        Disposition::Active | Disposition::Settled => None,
    }
}

/// The unseen-completion badge, or `None`.
///
/// Deliberately separate from unread. A working agent produces unread output
/// constantly and wants nothing; a session that FINISHED while nobody was
/// looking is the row the operator opened the sidebar to find, and collapsing
/// the two loses exactly that row in the noise.
///
/// No mark, by the same rule [`parked_label`] states: a glyph in front of a
/// word that already says the thing is a second mark saying what the first
/// already said. The star this badge used to carry meant favourite everywhere
/// else a reader has met one, and nothing at all here — it had to be learned
/// from the word beside it, which is the word it was decorating.
pub fn completion_badge(row: &SessionView) -> Option<Badge> {
    row.has_unseen_completion().then(|| Badge {
        class: "rg-badge rg-badge--done".to_string(),
        icon: None,
        text: status_word(StateWord::Done).to_string(),
        title: "Finished while you were not looking".to_string(),
    })
}

/// What a parked row shows in its one right-hand cell, or `None` when it
/// should show its age instead.
///
/// A snoozed row's whole story is when it comes back, so the cell spends
/// itself on the return ticket rather than on how long ago the agent last
/// spoke — which, for a row parked until tomorrow morning, is a number nobody
/// asked for. A settled row has no ticket and falls through to its age, which
/// is the only thing left that orders the tail.
///
/// It is styled as a pill rather than as a badge because it sits in the slot
/// the status label occupies on an active row, and two different shapes in
/// one column would read as two columns. It carries no mark, for the same
/// reason the snoozed disposition badge does not: the countdown and the
/// snooze hue are the message, and a glyph in front of "2h" is a second mark
/// saying what the first already said.
pub fn parked_label(row: &SessionView, clock: Clock, policy: DispositionPolicy) -> Option<Badge> {
    if row.disposition(clock, policy) != Disposition::Snoozed {
        return None;
    }
    let snooze = row.snooze?;
    Some(Badge {
        class: "rg-pill rg-pill--snoozed".to_string(),
        icon: None,
        text: wake_countdown_label(snooze.wake_at_ms, clock.now_ms),
        title: format!(
            "Parked until {}",
            wake_description(snooze.wake_at_ms, clock)
        ),
    })
}

/// Why the snooze action is refused, or `None` when it is allowed.
///
/// Shown on the disabled menu entry rather than hiding the entry. An action
/// that vanishes teaches nothing; one that says why teaches the rule once.
pub fn snooze_refusal(row: &SessionView) -> Option<&'static str> {
    (!row.can_snooze()).then_some("blocked on you \u{2014} it would wake immediately")
}

/// Why the settle action is refused, or `None`.
pub fn settle_refusal(row: &SessionView) -> Option<&'static str> {
    if row.blocks_on_operator() {
        return Some("blocked on you \u{2014} answer it first");
    }
    (!row.can_settle()).then_some("still working \u{2014} wait for the turn to end")
}

/// Caption for one band, and the tooltip that says what is in it.
///
/// All three bands have one, including Active. Whether a caption is WORTH a
/// line is a markup decision and lives in `ui::sidebar`, which draws the
/// Active head only when a Snoozed or Settled shelf sits under it: a caption
/// that marks no boundary is a line of nothing, but a caption that does mark
/// one has to come from the same place as the other two or the three shelves
/// drift into three vocabularies.
pub fn section_head(section: Section) -> (&'static str, &'static str) {
    match section {
        Section::Active => (
            "Active",
            "Live work: everything you have not parked or drained",
        ),
        Section::Snoozed => (
            "Snoozed",
            "Parked until their wake time, or until they ask for you",
        ),
        Section::Settled => (
            status_word(StateWord::Done),
            "Drained: you are finished with these",
        ),
    }
}

/// Is this row on the queue of work blocked on the operator?
///
/// Narrower than [`SidebarStatus::wants_operator`], which is true for
/// everything except `Working` and would therefore match almost every row in a
/// twenty-agent list. The jump key has to land somewhere useful on the first
/// press, so a row qualifies only when the operator is genuinely the
/// bottleneck: the agent declared a block, it failed, or it finished unseen. A
/// `Ready` row you have already looked at is finished business.
///
/// Snoozed rows are excluded on purpose. Parking a row is the operator saying
/// "not now", and jumping to it anyway would undo that decision on their
/// behalf; a row that has something urgent raises its hand and stops being
/// snoozed by itself.
pub fn wants_operator(row: &SessionView, clock: Clock, policy: DispositionPolicy) -> bool {
    if row.disposition(clock, policy) == Disposition::Snoozed {
        return false;
    }
    row.blocks_on_operator() || row.status() == SidebarStatus::Failed || row.has_unseen_completion()
}

// ═══════════════════════════════════════════════════════════════════════════
// Why the sidebar is empty
// ═══════════════════════════════════════════════════════════════════════════

/// How many sessions survived each cut between the daemon and the sidebar.
///
/// Three numbers, each the input to the next filter, taken in the same order
/// `WindowState::tree` applies them: everything the daemon reports, then the
/// subset filed into this window's workspace, then the subset of THOSE whose
/// disposition this workspace still shows. The filter query is deliberately
/// not in here: a search that matched nothing already has its own surface,
/// and folding it in would make one sentence answer two questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Census {
    /// Every session the daemon lists, in every workspace.
    pub total: usize,
    /// Filed into the workspace this window is looking at.
    pub in_workspace: usize,
    /// Of those, still admitted by the workspace's band visibility.
    pub admitted: usize,
}

/// Why there is nothing to draw.
///
/// The sidebar used to answer every empty tree with one sentence: "Projects
/// appear here as soon as a session runs in one." That sentence is TRUE for
/// exactly one of the three ways a tree gets to be empty, and it is a flat
/// lie for the other two, both of which are features working correctly:
///
/// - An operator who has just made a second workspace has a blank sidebar
///   *because a new workspace is blank*, which is the whole point of one. The
///   old sentence told them the daemon had no sessions while twenty of them
///   were running one keystroke away.
/// - An operator who switched Active off in `Settings > Workspaces` hid every
///   row in the workspace. The old sentence told them the daemon had no
///   sessions, rather than that they had turned them off, and gave them no
///   way back.
///
/// Both cases read as "the sidebar is broken and my sessions are gone". Two
/// features that work end to end have been reported as missing on the
/// strength of one string, which is why this is an enum and not a `bool`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Empty {
    /// Nothing anywhere. The honest first-run state, and the only one the old
    /// sentence was ever right about.
    NoSessions,
    /// Sessions exist, none of them are filed here. A blank workspace.
    ElsewhereFiled {
        /// How many are in other workspaces, so the sentence can be specific.
        elsewhere: usize,
    },
    /// Sessions are filed here and every one of them is hidden by this
    /// workspace's band visibility.
    BandsHidden {
        /// How many rows the toggles are hiding.
        hidden: usize,
    },
}

impl Empty {
    /// Classify an empty tree.
    ///
    /// Ordered widest cut first, because a session lost to the workspace
    /// filter never reaches the band filter and naming the band toggles for it
    /// would send the operator to the wrong settings page.
    #[must_use]
    pub fn of(census: Census) -> Self {
        if census.total == 0 {
            return Empty::NoSessions;
        }
        if census.in_workspace == 0 {
            return Empty::ElsewhereFiled {
                elsewhere: census.total,
            };
        }
        if census.admitted == 0 {
            return Empty::BandsHidden {
                hidden: census.in_workspace,
            };
        }
        // Every cut passed and the tree is still empty. Unreachable through
        // `tree`, which only drops a non-empty bucket for a filter query, and
        // the filter has its own empty surface. Saying "nothing here yet" is
        // the one answer that is never actively misleading.
        Empty::NoSessions
    }

    /// The heading, and the sentence under it.
    ///
    /// Assembled here rather than in the markup so the exact words are
    /// testable, and so the two new ones cannot drift back into being the old
    /// one. Each names its cause and the way back: an operator who reads one
    /// of these knows which surface to open next.
    #[must_use]
    pub fn words(&self, workspace: &str) -> (String, String) {
        match *self {
            Empty::NoSessions => (
                String::new(),
                "Projects appear here as soon as a session runs in one.".to_string(),
            ),
            Empty::ElsewhereFiled { elsewhere } => (
                format!("{workspace} is empty"),
                format!(
                    "{elsewhere} {} in other workspaces. Start one here, or right-click a row and use Move to workspace.",
                    plural(elsewhere, "session is", "sessions are")
                ),
            ),
            Empty::BandsHidden { hidden } => (
                format!("Every row in {workspace} is hidden"),
                format!(
                    "{hidden} {} filed here and this workspace is showing none of {}. Turn a band back on in Settings \u{203a} Workspaces.",
                    plural(hidden, "session is", "sessions are"),
                    plural(hidden, "it", "them")
                ),
            ),
        }
    }
}

/// Pick the singular or the plural form.
fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 { one } else { many }
}

/// One project header and its rows, split into the model's three bands.
///
/// `project` is `None` for the orphan bucket, which holds sessions whose
/// `project_id` matches no [`ProjectInfo`]. Dropping those would make sessions
/// silently invisible whenever the two snapshots race.
#[derive(Debug, Clone, PartialEq)]
pub struct Group<'a> {
    pub project: Option<&'a ProjectInfo>,
    /// Inbox rows on screen, newest first. Includes woken rows, in place.
    pub active: Vec<&'a SessionView>,
    /// Inbox rows past [`PREVIEW_LIMIT`], behind the "show all" affordance.
    pub hidden: Vec<&'a SessionView>,
    /// Parked rows, soonest wake first.
    pub snoozed: Vec<&'a SessionView>,
    /// Drained rows, most recently ended first.
    pub settled: Vec<&'a SessionView>,
    /// What a collapsed header shows. Always present: every bucket the sidebar
    /// draws is collapsible except Unfiled, and a header that can collapse has
    /// to be able to say what it is holding.
    pub rollup: Option<ProjectRollup>,
}

impl Group<'_> {
    /// Rows in this group.
    pub fn len(&self) -> usize {
        self.active.len() + self.hidden.len() + self.snoozed.len() + self.settled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Rows in one band.
    pub fn section(&self, section: Section) -> &[&SessionView] {
        match section {
            Section::Active => &self.active,
            Section::Snoozed => &self.snoozed,
            Section::Settled => &self.settled,
        }
    }

    /// Is `id` one of this group's rows?
    ///
    /// Every band, and the rows the preview cut hid. A bucket that holds the
    /// focused session behind its "show all" affordance is still the bucket
    /// the operator is working in, and answering `false` there would unpin the
    /// current project the moment its inbox grew past eight rows.
    pub fn holds(&self, id: SessionId) -> bool {
        [&self.active, &self.hidden, &self.snoozed, &self.settled]
            .into_iter()
            .any(|band| band.iter().any(|row| row.id() == id))
    }
}

/// Build one group's bands from the rows that belong to it.
///
/// `focused` is rescued from the preview cut: a row you are looking at must
/// never disappear behind a "show all" affordance because it aged past the
/// limit.
///
/// `label` names the bucket the rollup is reported under, and the fold is
/// [`rollup_rows`] rather than `rollup_project` precisely because the label is
/// a name and not a filter. A bucket is one DIRECTORY, and one directory can
/// carry several daemon project ids (see [`coalesce_projects`]); a fold that
/// filtered on one of them would count a quarter of the rows and put that
/// number on the collapsed header.
pub(crate) fn build_group<'a>(
    label: ProjectId,
    project: Option<&'a ProjectInfo>,
    rows: Vec<&'a SessionView>,
    focused: Option<SessionId>,
    preview_expanded: bool,
    clock: Clock,
    policy: DispositionPolicy,
    // Rows the Active band keeps before the cut, from
    // `settings.inbox.previewRows`. Floored at one: a cut of zero hides every
    // live row behind an affordance whose label counts rows nobody can see.
    preview_limit: usize,
) -> Group<'a> {
    let mut active = Vec::new();
    let mut snoozed = Vec::new();
    let mut settled = Vec::new();
    for row in &rows {
        match row.section(clock, policy) {
            Section::Active => active.push(*row),
            Section::Snoozed => snoozed.push(*row),
            Section::Settled => settled.push(*row),
        }
    }

    // The model owns every comparator. Sorting borrowed rows rather than
    // calling `arrange` keeps `UiState` immutable during a render, which is
    // what lets the sidebar derive its whole shape from a `&self`.
    active.sort_by(|left, right| compare_active(left, right, Default::default()));
    snoozed.sort_by(|left, right| compare_snoozed(left, right));
    settled.sort_by(|left, right| compare_settled(left, right));

    let rollup = Some(rollup_rows(label, rows.iter().copied(), clock, policy));

    // ONE pass, and only when there is a cut to make.
    //
    // `preview_sessions` copies the whole id list into `split.visible`, and
    // this reads only `split.hidden`, so the uncut case used to allocate two
    // vectors of every active id per bucket per paint and discard both. The
    // limit is eight and most buckets are under it, so that was the common
    // case: at 20 sessions in 11 buckets, 22 allocations a paint for a
    // partition that was always empty.
    //
    // When there IS a cut, `preview_sessions` emits both halves in the
    // caller's order, which IS this vector's order, so a single cursor into
    // `split.hidden` answers "was this row cut" in O(1) as `retain` walks.
    // Doing it with `contains` was two O(n*h) scans and a third allocation,
    // measured at 5.445us and 92 allocations for one arrangement.
    let mut hidden: Vec<&SessionView> = Vec::new();
    let preview_limit = preview_limit.max(1);
    if !preview_expanded && active.len() > preview_limit {
        let ids: Vec<SessionId> = active.iter().map(|row| row.id()).collect();
        let split = preview_sessions(&ids, focused, false, preview_limit);
        hidden.reserve_exact(split.hidden.len());
        let mut cut = 0usize;
        active.retain(|row| {
            if cut < split.hidden.len() && split.hidden[cut] == row.id() {
                cut += 1;
                hidden.push(row);
                return false;
            }
            true
        });
    }

    Group {
        project,
        active,
        hidden,
        snoozed,
        settled,
        rollup,
    }
}

/// Per-state counts on a collapsed project header, most urgent first.
///
/// Only non-zero states appear. A header that always shows five numbers, four
/// of them zero, is four numbers of noise on the row that has the least space.
pub fn rollup_chips(rollup: &ProjectRollup) -> Vec<(SidebarStatus, usize)> {
    let mut chips: Vec<(SidebarStatus, usize)> = vitrum_model::ALL_STATUSES
        .into_iter()
        .map(|status| (status, rollup.counts.get(status)))
        .filter(|(_, count)| *count > 0)
        .collect();
    chips.sort_by_key(|(status, _)| std::cmp::Reverse(status.urgency()));
    chips
}

/// One line naming everything a collapsed project is holding.
pub fn rollup_title(rollup: &ProjectRollup) -> String {
    let mut parts = Vec::with_capacity(4);
    for (status, count) in rollup_chips(rollup) {
        parts.push(format!("{count} {}", status.label().to_lowercase()));
    }
    if rollup.woke > 0 {
        parts.push(format!("{} woke", rollup.woke));
    }
    if rollup.snoozed > 0 {
        parts.push(format!("{} snoozed", rollup.snoozed));
    }
    if rollup.settled > 0 {
        parts.push(format!("{} done", rollup.settled));
    }
    if parts.is_empty() {
        return "No sessions".to_string();
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod project_identity_tests;
