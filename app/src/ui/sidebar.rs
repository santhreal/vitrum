//! Project and session sidebar.
//!
//! Markup only. Every rule the markup expresses lives somewhere else: the
//! layout classes in `app/assets/sidebar.css`, and every decision about what a
//! row IS in `vitrum-model`, reached through [`crate::inbox`]. Nothing in this
//! file decides whether a session is done, urgent, parked or ordered.
//!
//! # Two row shapes, and where the split comes from
//!
//! A row is a **card** when [`SessionView::section`] puts it in
//! [`Section::Active`] and a **slim** row when it puts it in Snoozed or
//! Settled. Nothing else selects the shape: the operator's own parking is what
//! makes the list dense, not the sidebar second-guessing which of their live
//! sessions still matters.
//!
//! A card is two fixed-height lines, title and metadata, and a slim row is
//! one. Both end in `__slot`, a single grid cell that holds the status label
//! (card) or the timestamp (slim) stacked with the close button, which is why
//! nothing on the right of a row can collide at any width.
//!
//! # Five contracts worth stating because breaking them is silent
//!
//! - **Every title starts at the same x.** The only thing before it, on
//!   either shape, is a fixed-width agent mark. Nothing status-shaped may
//!   ever precede it. The status word varies from five characters to eight,
//!   so a status in front of the title gives twenty rows twenty different
//!   title positions, and that is what makes a list read as a table.
//! - **Unread is typographic, never a marker.** Line one already ends in a
//!   status pill carrying its own leading dot, so a second dot beside it was
//!   a dot beside a dot in two hues. The `--unread` modifier brightens the
//!   title and lifts it to semibold, which both row shapes get.
//! - **Recede is the whole design.** A row that wants nothing from the
//!   operator gives up its weight and, if it is still in flight, its opacity;
//!   hover puts both back. Without it every row is equally bright and the
//!   operator has to read all twenty to find the one that needs them.
//! - **A project's rows are split into the model's three bands.** Snoozed and
//!   Done are collapsed by default and always show their count, so nothing is
//!   hidden without a number saying how much.
//! - **Nothing loops.** Hover, selection and disclosure use one-shot
//!   transitions under 150ms. A keyframe animation that repeats is a repaint
//!   per frame forever and costs the most exactly when the most rows are lit.


use vitrum_fmt::path::{self, Place};
use vitrum_fmt::{TimeFormat, Timestamp};
use vitrum_model::{AgentKind, Section, SessionView, SidebarStatus};
use vitrum_proto::{SessionId, SessionStatus};

use crate::inbox::{self, Pill};
use crate::state::{Click, GroupKey, attention_label, attention_modifier};


/// The whole panel, folded from state into a description of widgets.
pub(crate) mod fold;
/// The row store, and the guarantee that an unchanged row costs nothing.
pub(crate) mod rows;
/// The widget description the fold produces and the tests read.
pub(crate) mod tree;
/// The GTK interpreter that turns that description into a window.
pub(crate) mod widgets;

/// Disclosure chevron, U+25BE BLACK DOWN-POINTING SMALL TRIANGLE.
///
/// One glyph for both states. `sidebar.css` rotates it -90deg on
/// `.rg-project--collapsed` and `.rg-project__section--collapsed`, so the
/// markup must NOT swap in a right-pointing glyph as well or a collapsed
/// group points up.
const CHEVRON: &str = "\u{25be}";
/// Search glyph, U+2315 TELEPHONE RECORDER.
const SEARCH_ICON: &str = "\u{2315}";
/// Settings glyph, U+2699 GEAR.
const GEAR_ICON: &str = "\u{2699}";

/// Column budget for a path inside a row tooltip.
///
/// A native tooltip does not wrap, so an unbounded absolute path pushes the
/// tooltip wider than the window and clips the lines under it.
const TOOLTIP_PATH_COLUMNS: usize = 64;

/// Column budget for a bucket label that is a filesystem path.
///
/// Only directory and folder buckets hit it; a daemon project's label is
/// already a bare name. An unshortened absolute path in a 14rem column is all
/// prefix and no name, which is the one thing a header must not be.
const GROUP_LABEL_COLUMNS: usize = 28;

/// DOM id of one session row, so the keyboard can scroll it into view.
///
/// Traversal that moves focus to a row thirty rows below the fold, without
/// scrolling, looks exactly like traversal that did nothing.
pub fn row_id(id: SessionId) -> String {
    format!("rg-row-{}", id.0)
}

/// What an empty bucket says instead of drawing a header over a void.
///
/// Every bucket kind can legitimately be empty and they are empty for
/// different reasons, so one sentence would be wrong for two of them. A
/// folder is the case that matters: `bucket_by_folder` keeps an empty folder
/// deliberately, because a folder you just made is exactly the one you are
/// about to file into and hiding it makes it unreachable — but the sidebar
/// then drew a header, a zero, and nothing, with no hint anywhere on screen
/// that the way to fill it is the row context menu. The operator's only
/// evidence that named grouping works at all was a bucket that looked broken.
fn empty_bucket_hint(key: GroupKey) -> &'static str {
    match key {
        GroupKey::Folder(_) => "Empty. Right-click a session and use Move to folder.",
        GroupKey::Unfiled => "Nothing unfiled.",
        GroupKey::Project(_) | GroupKey::Directory(_) => "No sessions here yet.",
    }
}

/// The coarsest clock that renders this row byte-identically to `clock`.
///
/// # Why a row gets its own clock
///
/// [`SessionRow`] is memoized on its props, and the clock is one of them, so
/// a clock that moves rebuilds the row. A whole-second clock already stops the
/// rebuild inside one second, but it still rebuilds EVERY row on every second
/// boundary, forever. Most of those rows had nothing to say: a row reading
/// `5h ago` repeats that answer 3600 times before it changes, and at the
/// stated load of twenty sessions that is twenty rebuilds a second buying one
/// changed character an hour.
///
/// So the clock handed to a row is floored to the coarsest instant that row
/// cannot tell apart from now. Two things vary with time in a row and each
/// owns its own grid:
///
/// - the LABEL it draws, whose thresholds live in `TimeFormat::relative_floor`
/// - its STATE, whose transitions live in `SessionView::clock_floor_ms`
///
/// The later of the two wins, because that is the one that changes soonest.
/// The result sits inside both intervals, so every string and every
/// disposition is what it would have been at the real clock, and the row is
/// rebuilt exactly when something it shows actually moves.
///
/// A row with a live timer, a countdown, or a pending transition therefore
/// keeps a per-second clock and rebuilds as before. That is the point: this
/// buys precision back only where it was never being spent.
fn row_clock(clock: TimeFormat, row: &SessionView) -> TimeFormat {
    let policy = vitrum_model::DispositionPolicy::default();
    let state = row.clock_floor_ms(inbox::model_clock(clock), policy);
    let label = clock
        .relative_floor(Timestamp::from_millis(row.info.last_activity_ms as i64))
        .as_millis();
    let floor = label.max(state as i64).min(clock.now().as_millis());
    clock.at(Timestamp::from_millis(floor))
}

/// The row elements the operator can switch off in Settings.
///
/// One `Copy` value rather than four props, read from
/// [`crate::state::Settings`] once per paint rather than once per row. Three
/// of them do not change the row's SHAPE: `__branch` is still emitted when it
/// is off, empty, because it is the flex spacer that pushes line one's tail
/// right, and `__word` off leaves `__icon`, which is exactly what the
/// collapsed sidebar already draws. `always_slim` is the one that does, and
/// it is the operator asking for every agent on screen at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowFields {
    branch: bool,
    time: bool,
    status_word: bool,
    /// Draw the session's working directory when it is not the project's own.
    place: bool,
    /// Draw the name of the linked git worktree the session's files are in.
    ///
    /// Off is not the same as absent: with the chip off the element is still
    /// emitted and still empty, so switching the preference cannot move a
    /// row's other elements.
    worktree: bool,
    /// Force every row to the slim shape, whatever band it is in.
    always_slim: bool,
}

impl RowFields {
    fn of(settings: &crate::state::Settings) -> Self {
        RowFields {
            branch: settings.show_branch,
            time: settings.show_time,
            status_word: settings.show_status_word,
            place: settings.show_place,
            worktree: settings.show_worktree,
            always_slim: settings.always_slim,
        }
    }
}

/// Class list for one shelf.
///
/// The band modifier is not decoration: `--snoozed` tints its head in the
/// snooze hue and `--settled` drains it, and those are the only two cues that
/// say which shelf a run of rows belongs to once it is scrolled away from its
/// caption.
fn section_class(section: Section, open: bool) -> String {
    let mut class = String::from("rg-project__section ");
    class.push_str(match section {
        Section::Active => "rg-project__section--active",
        Section::Snoozed => "rg-project__section--snoozed",
        Section::Settled => "rg-project__section--settled",
    });
    if !open {
        class.push_str(" rg-project__section--collapsed");
    }
    class
}

/// Which of the two shapes a band's rows take.
///
/// Cards for the inbox, slim rows for the tail. The operator's own parking is
/// what makes the list dense; a sidebar that decided on their behalf which
/// live sessions deserved a card would be guessing at the one thing they have
/// already told it.
///
/// `always_slim` is the operator saying it anyway. At 4K the card yields
/// thirteen rows and the slim row nineteen, so that one switch is the
/// difference between "most of my agents" and "all of them"; it is theirs to
/// throw, not ours to guess.
fn row_variant(section: Section, always_slim: bool) -> &'static str {
    if always_slim {
        return "rg-session--slim";
    }
    match section {
        Section::Active => "rg-session--card",
        Section::Snoozed | Section::Settled => "rg-session--slim",
    }
}

/// Does this row draw the card's markup?
///
/// The SHAPE half of the decision [`row_variant`] makes about the CLASS, and
/// they have to agree. They did not: this was `section == Section::Active`
/// spelled out at the call site while `row_variant` also consulted
/// `always_slim`, so with the operator's "every row slim" switch thrown an
/// Active row wore the slim class and then rendered the card's markup inside
/// it. A box and its contents disagreeing, produced by a control the operator
/// had deliberately used.
///
/// One function, called by both, is the only arrangement in which the two
/// cannot drift again.
fn draws_card(section: Section, always_slim: bool) -> bool {
    !always_slim && section == Section::Active
}

/// How many columns a row spends on its working directory.
///
/// Line two also carries the branch, a badge and a timestamp, and the row's
/// measured floor is 224px. Eighteen columns is two short components; past
/// that the middle is elided rather than the tail, because the leaf is the
/// part that says which of a project's crates the session is in.
const PLACE_COLUMNS: usize = 18;

/// What a row draws for its working directory, given the project it is under.
///
/// The rule is that a row says only what its group header does not, and only
/// for as long as the rest of the line is still saying something. A session
/// sitting at the project root repeats the header, so it yields the space to
/// the branch.
///
/// `line_says_more` is what keeps the second half of that true. A group
/// header carries a project NAME, not a path, so silence on a root row is
/// readable only when something else on the line is not also silent. A root
/// row with nothing beside it drew an empty line and said nothing at all
/// about where its agent was working, and the commonest shape of that is an
/// agent started in a home directory that the client then minted a project
/// for: the header reads as a username and the directory is not a
/// repository, so both halves went blank together.
///
/// It is "something else" and not "a branch" because the line now has two
/// other elements that can carry the answer. A worktree name says where the
/// files are more precisely than a directory does, so a root row inside a
/// linked worktree yields to it exactly as it yields to a branch. The caller
/// decides which of them is present; the rule is that the line is never
/// silent on all three at once.
///
/// The `Outside` arm is the case the element was added for. A git worktree
/// lives beside its project rather than inside it, on another branch, and a
/// row for one used to show a branch with no hint that the files were
/// somewhere else. So does an agent that moved itself: sessions follow OSC 7,
/// so a row's directory is where the agent is now, not where it was launched.
fn place_label(cwd: &str, root: &str, home: &str, line_says_more: bool) -> String {
    match path::under(cwd, root) {
        Place::At if line_says_more => String::new(),
        Place::At | Place::Outside => path::shorten_home_relative(cwd, home, PLACE_COLUMNS),
        Place::Under(rest) => path::shorten(rest, PLACE_COLUMNS),
    }
}

/// Everything about a row that changes its class list.
///
/// A struct rather than eight positional booleans: `row_class(true, false,
/// false, true, ...)` is unreadable at the call site and silently wrong when
/// two arguments swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowState {
    section: Section,
    status: SidebarStatus,
    active: bool,
    picked: bool,
    unread: bool,
    woke: bool,
    /// Finished while nobody was looking. Not the same as unread: a working
    /// agent is unread constantly and wants nothing.
    finished_unseen: bool,
    attention: Option<&'static str>,
    /// Every row is slim, whatever band it is in.
    always_slim: bool,
}

/// Does this row give up its prominence?
///
/// The single idea that stops the list reading as a table. Twenty rows at
/// equal weight force the operator to read all twenty; a row that wants
/// nothing yet drops to the muted foreground and normal weight, and hover
/// puts it back, so the quiet majority stays legible without competing.
///
/// A row keeps its weight for exactly four reasons, all of them "a human is
/// implicated": it failed, there is output nobody has read, it came back from
/// a snooze, or it finished unseen. Selection and focus also hold it, because
/// dimming the row the operator is looking at is absurd.
fn recedes(s: RowState) -> bool {
    matches!(
        s.status,
        SidebarStatus::Ready
            | SidebarStatus::Working
            | SidebarStatus::Approval
            | SidebarStatus::Input
    ) && !s.unread
        && !s.woke
        && !s.finished_unseen
        && !s.active
        && !s.picked
}

/// Does the whole row fade, on top of receding?
///
/// Working, approval and input are all "not your problem yet": the agent is
/// mid-turn and the next event will come from it, not from you. Ready is
/// excluded because a finished turn IS your problem even when nothing has
/// flagged it.
fn in_flight(s: RowState) -> bool {
    matches!(
        s.status,
        SidebarStatus::Working | SidebarStatus::Approval | SidebarStatus::Input
    ) && !s.active
        && !s.picked
}

/// Class list for a session row.
///
/// Pulled out of the markup so the exact emitted class names are testable; the
/// stylesheet keys every visual state off this string and a typo in one
/// modifier silently drops that state with no error anywhere.
///
/// `picked` is deliberately not the same thing as `active`: `--active` is the
/// row whose PTY is in the main pane, and `--picked` is membership of a
/// multi-selection. A row can be either, both, or neither.
#[cfg(test)]
fn row_class(s: RowState) -> String {
    let mut class = String::from("rg-session ");
    class.push_str(row_variant(s.section, s.always_slim));
    if recedes(s) {
        class.push_str(" rg-session--recede");
    }
    if in_flight(s) {
        class.push_str(" rg-session--inflight");
    }
    if s.unread {
        class.push_str(" rg-session--unread");
    }
    if s.woke {
        class.push_str(" rg-session--woke");
    }
    if s.picked {
        class.push_str(" rg-session--picked");
    }
    if s.active {
        class.push_str(" rg-session--active");
    }
    if let Some(rail) = s.attention {
        class.push(' ');
        class.push_str(rail);
    }
    class
}

/// The completion badge a row actually draws.
///
/// A failed row does not also announce a finish. The pill reads "Failed" in
/// red and the completion badge reads "Done" in green, and the two sat beside
/// each other on one line saying opposite things about the same turn.
/// [`inbox::completion_badge`] answers "finished while you were not looking",
/// which is as true of a crash as of a success, so the suppression belongs
/// here at the row and not inside that function. The unseen-versus-seen
/// distinction still reaches the row through `finished_unseen` in
/// [`recedes`], which holds an unseen failure at full strength.
fn completion_shown(status: SidebarStatus, badge: Option<inbox::Badge>) -> Option<inbox::Badge> {
    match status {
        SidebarStatus::Failed => None,
        _ => badge,
    }
}

/// Which mouse gesture a click on a row was.
///
/// Shift wins over Ctrl when both are held, matching every file manager:
/// Ctrl+Shift is the additive range, and that is what `RangeAdditive` is.
fn click_kind(ctrl: bool, shift: bool) -> Click {
    match (ctrl, shift) {
        (true, true) => Click::RangeAdditive,
        (false, true) => Click::Range,
        (true, false) => Click::Toggle,
        (false, false) => Click::Plain,
    }
}

/// Everything a row's hover detail says, in one string.
///
/// Assembled here rather than in the markup so the exact text is testable.
///
/// It carries EVERY fact the row used to spread across four separate `title`
/// attributes — the surface, the status pill, the disposition badge and the
/// contest mark. That consolidation is the fix for the black rectangle: see
/// `.rg-session__tip` in `sidebar.css` for why a row inside this panel may
/// not ask the platform for a tooltip at all.
///
/// The state word comes from [`Pill`], not from `status_label`, so this
/// string and the pill 8px above it cannot name one state two ways.
fn row_tooltip(row: &SessionView, home: &str, pill: &Pill) -> String {
    let info = &row.info;
    let mut s = format!(
        "{}\n{}\n{} \u{2022} {}",
        inbox::row_title(info),
        vitrum_fmt::path::shorten_home_relative(&info.cwd, home, TOOLTIP_PATH_COLUMNS),
        AgentKind::of(&info.command).label(),
        pill.word
    );
    // The worktree, on its own line under the directory it qualifies.
    //
    // The chip on the row is three characters at its floor and elides. The
    // hover detail is the only place the whole name is guaranteed, and the
    // whole name is what an operator needs to run a command against the
    // right checkout.
    if let Some(wt) = crate::ui::terminal::worktree_of(row) {
        s.push_str("\nWorktree ");
        s.push_str(&wt);
    }
    // How the state was decided. An inferred status and a probed one look
    // identical apart from this sentence, and it is the one thing the pill's
    // own tooltip carried that nothing else on the row says. It also answers
    // the blocked question for every platform, including the one that cannot
    // probe, so the row does not say it twice in two vocabularies.
    s.push('\n');
    s.push_str(inbox::source_note(pill.source));
    if attention_modifier(&info.attention).is_some() {
        s.push('\n');
        s.push_str(&attention_label(&info.attention));
    }
    s.push_str("\nRight-click for more");
    s
}

/// What a contested row says on hover.
///
/// Names the thing that is actually wrong, in the operator's terms: files,
/// and who else is in them. "Conflict" is avoided on purpose, because that is
/// what version control calls a merge outcome and this is not one: two agents
/// writing one file lose work without anything ever reporting a conflict.
fn contest_title(files: usize, peers: usize) -> String {
    let f = if files == 1 { "file" } else { "files" };
    let p = if peers == 1 { "session" } else { "sessions" };
    format!(
        "{files} {f} also being changed by {peers} other {p}. \
         Whichever writes last wins, silently."
    )
}

/// Status hue for a row's agent mark.
///
/// The mark carries WHO is running; the hue carries WHETHER it is running. One
/// element answers both, which is the whole reason it can afford 16px in a
/// 224px row. Same four tiers as the pill, so a row can never say two things.
fn agent_class(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Starting => "rg-session__agent--starting",
        SessionStatus::Running => "rg-session__agent--running",
        SessionStatus::Exited { code: Some(0) } => "rg-session__agent--exited",
        SessionStatus::Exited { .. } => "rg-session__agent--exited-error",
    }
}

#[cfg(test)]
mod tests;
