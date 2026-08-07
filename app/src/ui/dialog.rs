//! The launcher, and the rename dialog.
//!
//! # The launcher
//!
//! Starting an agent used to cost two clicks from every entry point in the
//! product: one to open a modal form, one to press Launch. The form asked for
//! a working directory (prefilled with a 47-character absolute path), a
//! command and a label, and explained the daemon's naming rules in a caption
//! underneath. Measured against the thing the operator actually wanted, which
//! is "the same agent, in the same project, again", every one of those was a
//! question with a known answer.
//!
//! So the answer moved to the button. [`primary_launch`] is the whole primary
//! path: it ranks what this operator launches, takes the top row, and returns
//! a [`Launch`] that the caller sends without opening anything. No dialog, no
//! confirmation. The sidebar's control carries the name of the thing it is
//! about to start, so it is not a mystery button, and a misfire is undone by
//! closing the tab the way any other session is closed.
//!
//! When there is no confident answer, [`primary_launch`] returns
//! [`Primary::Choose`] carrying ONE concrete sentence: the command that is no
//! longer on `PATH`, or that nothing has been launched here yet. The caller
//! opens this launcher instead and shows that sentence. Guessing is never the
//! fallback.
//!
//! # The list
//!
//! One always-focused query input, and under it rows that are already there
//! before a key is pressed. A row is a launch intent, not a field: what runs,
//! where, and on which branch. Ranked recents in this project first, then
//! saved presets, then agents found on `PATH`, then what is running elsewhere,
//! then the login shell. Typing fuzzy-matches across all three of command,
//! place and branch at once; Enter takes the highlighted row, Ctrl+1 to
//! Ctrl+9 take a numbered one, the arrows move, and Escape closes.
//!
//! The place is shown project-relative (`vitrum/app`) and never as the raw
//! absolute path, which is on the row's `title` where it can be read without
//! spending a line on it. Changing it costs no control: type anything that
//! starts with `/` or `~` and the list becomes directory completions, and
//! picking one makes it the place for every row.
//!
//! # What the open path is allowed to do
//!
//! Nothing that can block. The `PATH` walk behind [`launch::detected_agents`]
//! is five lookups across every directory in `PATH`, so it runs on a thread
//! and the list paints without it; the agents land in a band BELOW the
//! recents, so an answer arriving late can never move the row the highlight is
//! already on. Directory completion is off-thread for the older reason, which
//! is that `read_dir` on a wedged network mount blocks in the kernel for as
//! long as the mount wants.
//!
//! What is left on the open path is one small profile read
//! ([`launch::load_launch_store`]) and two environment reads. The store is not
//! deferred, and that is deliberate: it IS the ranking, so a launcher that
//! painted before it resolved would reshuffle its own top row under the
//! operator's hand.
//!
//! Rendering itself performs no syscall at all. The version of this file
//! before called [`launch::validate`] once per render, which is one `stat` and
//! one full `PATH` walk PER KEYSTROKE; validation now happens only when a row
//! is actually taken.
//!
//! Live edits live in component-local signals rather than in [`UiState`].
//! Typing must not mark the one global signal dirty, or every keystroke
//! re-diffs the sidebar, the tab strip and the flash strip.
//!
//! Escape is not handled here. `bootstrap.js` matches the shell's chord table
//! on `window` in the capture phase and calls `stopPropagation`, so Escape is
//! claimed by [`crate::keymap::KeyAction::Dismiss`] before any handler in this
//! file could run. That is also why a preset chord must clear
//! [`launch::chord_conflict`] before it is stored.

use std::path::MAIN_SEPARATOR;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use dioxus::prelude::*;
use vitrum_proto::{ProjectId, ProjectInfo, SessionId};

use crate::launch::{self, CommandSource, Detected, Launch, LaunchStore, SavedPreset};
use crate::state::{NewSessionSeed, RenameSeed, UiState};

/// Rows the launcher shows at once.
///
/// Nine, because every visible row carries a Ctrl+digit shortcut and there is
/// no tenth digit. A tenth row would be the one row on the surface whose
/// number badge is a lie, and the query is a faster way to reach it anyway.
pub const ROWS_MAX: usize = 9;

/// Directory completions offered while the query is a path.
const DIR_MAX: usize = launch::COMPLETE_MAX;

/// The class a directory completion row carries, unhighlighted and highlighted.
const DIROPT: &str = "rg-launch__diropt";
const DIROPT_ON: &str = "rg-launch__diropt rg-launch__diropt--on";

/// Commands drawn out of [`launch::command_suggestions`] before ranking.
///
/// The whole history plus the agents plus the shell. Truncating here would
/// hide a command from the query that the operator has definitely run.
const SUGGEST_MAX: usize = launch::HISTORY_MAX + 8;

// ---------------------------------------------------------------------------
// Intents
// ---------------------------------------------------------------------------

/// Why a row is in the list, and therefore where it sits.
///
/// The order of the variants is the order of the bands, and
/// [`Band::launchable_now`] is the line between "this operator has decided
/// this before" and "this machine merely happens to have it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// Launched from this profile before, or running in this project now.
    Recent,
    /// A command the operator saved and named.
    Preset,
    /// An agent binary found on `PATH`.
    Agent,
    /// Running, but not in this project.
    Elsewhere,
    /// The login shell.
    Shell,
    /// Exactly what was typed, when nothing else matches it.
    Typed,
}

impl Band {
    /// May the primary control fire this without opening anything?
    ///
    /// Only a band the operator has already chosen from. An agent that merely
    /// exists on `PATH` is a suggestion, and firing a suggestion on a bare
    /// click is the guess this design refuses to make.
    pub fn launchable_now(self) -> bool {
        matches!(self, Band::Recent | Band::Preset)
    }
}

/// One launch the operator could pick: what runs, where, and on what branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    /// The full command line, exactly as it will be split and spawned.
    pub command: String,
    /// The absolute working directory. Shown only in a `title`.
    pub cwd: String,
    /// `cwd` written project-relative, e.g. `vitrum/app`. Never absolute.
    pub place: String,
    /// The branch, when the daemon has resolved one for a session in `cwd`.
    /// `None` is a real answer: nothing here guesses at a checkout.
    pub branch: Option<String>,
    pub band: Band,
    /// The preset this row fires, when it is one. Presets keep their own
    /// launch path so their arguments stay exactly as they were saved.
    pub preset: Option<SavedPreset>,
    /// Why this row is here, for the row's tooltip.
    pub note: String,
    /// `command`, `place` and `branch` lowercased into one string.
    ///
    /// Built once when the row is built, so a keystroke filters the whole list
    /// without allocating. This is most of why typing is free here.
    pub hay: String,
}

impl Intent {
    fn new(
        command: String,
        cwd: String,
        place: String,
        branch: Option<String>,
        band: Band,
        note: String,
        preset: Option<SavedPreset>,
    ) -> Self {
        let mut hay = String::with_capacity(command.len() + place.len() + 24);
        hay.push_str(&command.to_lowercase());
        hay.push(' ');
        hay.push_str(&place.to_lowercase());
        if let Some(b) = &branch {
            hay.push(' ');
            hay.push_str(&b.to_lowercase());
        }
        if let Some(p) = &preset {
            hay.push(' ');
            hay.push_str(&p.label.to_lowercase());
        }
        Intent {
            command,
            cwd,
            place,
            branch,
            band,
            preset,
            note,
            hay,
        }
    }

    /// What the row reads. A preset shows the name the operator gave it;
    /// everything else shows the command line it will run.
    pub fn text(&self) -> &str {
        match &self.preset {
            Some(p) => &p.label,
            None => &self.command,
        }
    }
}

/// Every launch worth offering from `here`, best first.
///
/// The command dimension comes straight out of [`launch::command_suggestions`]
/// with an empty query, which is already the operator's ranked history
/// followed by the agents really on `PATH` followed by the login shell. This
/// function adds the two things that function cannot know: WHERE, and the
/// sessions the daemon already has running.
///
/// `detected` may be empty. That is the normal state for the first few
/// milliseconds a launcher is open, and the reason agents sit below recents:
/// a band that fills in late must never displace the highlighted row.
pub fn intents(
    st: &UiState,
    store: &LaunchStore,
    detected: &[Detected],
    shell: &str,
    here: &str,
    home: &str,
    now_ms: u64,
) -> Vec<Intent> {
    let projects = &st.daemon.projects;
    let here = launch::tidy_dir(here);
    let here_place = place_of(projects, &here, home);
    let here_branch = branch_in(st, &here);

    let mut recent: Vec<Intent> = Vec::new();
    let mut agents: Vec<Intent> = Vec::new();
    let mut shells: Vec<Intent> = Vec::new();
    for s in launch::command_suggestions(store, detected, shell, "", now_ms, SUGGEST_MAX) {
        let band = match s.source {
            CommandSource::History => Band::Recent,
            CommandSource::Detected => Band::Agent,
            CommandSource::Shell => Band::Shell,
        };
        let row = Intent::new(
            s.line,
            here.clone(),
            here_place.clone(),
            here_branch.clone(),
            band,
            s.note,
            None,
        );
        match band {
            Band::Recent => recent.push(row),
            Band::Agent => agents.push(row),
            _ => shells.push(row),
        }
    }

    let presets: Vec<Intent> = store
        .presets
        .iter()
        .map(|p| {
            let cwd = p
                .cwd
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(launch::tidy_dir)
                .unwrap_or_else(|| here.clone());
            let same = cwd == here;
            Intent::new(
                launch::join_command(&p.command, &p.args),
                cwd.clone(),
                if same {
                    here_place.clone()
                } else {
                    place_of(projects, &cwd, home)
                },
                if same {
                    here_branch.clone()
                } else {
                    branch_in(st, &cwd)
                },
                Band::Preset,
                "saved".to_string(),
                Some(p.clone()),
            )
        })
        .collect();

    // What the daemon is already running, in the directory it runs in. Sorted
    // on the daemon's own activity stamp, so the checkout touched last leads
    // rather than whichever session happens to be first in the list.
    let mut live: Vec<&vitrum_model::SessionView> = st.daemon.sessions.iter().collect();
    live.sort_by_key(|r| std::cmp::Reverse(r.info.last_activity_ms.max(r.info.created_at_ms)));
    let mut here_live: Vec<Intent> = Vec::new();
    let mut away: Vec<Intent> = Vec::new();
    for r in live {
        let cwd = launch::tidy_dir(&r.info.cwd);
        let near = same_project(projects, &here, &cwd);
        let same = cwd == here;
        let row = Intent::new(
            launch::join_command(&r.info.command, &r.info.args),
            cwd.clone(),
            if same {
                here_place.clone()
            } else {
                place_of(projects, &cwd, home)
            },
            r.info.git_branch.clone(),
            if near { Band::Recent } else { Band::Elsewhere },
            "running".to_string(),
            None,
        );
        if near {
            here_live.push(row);
        } else {
            away.push(row);
        }
    }

    let mut out = recent;
    out.extend(here_live);
    out.extend(presets);
    out.extend(agents);
    out.extend(away);
    out.extend(shells);

    // One row per thing that can be launched. Twenty agents in one repo would
    // otherwise put twenty identical rows in front of the operator, and each
    // of them would do the same thing.
    let mut seen: Vec<(String, String)> = Vec::with_capacity(out.len());
    out.retain(|i| {
        let key = (i.command.clone(), i.cwd.clone());
        let fresh = !seen.contains(&key);
        if fresh {
            seen.push(key);
        }
        fresh
    });
    out
}

/// Is this query asking for a directory, rather than naming a command?
///
/// The single definition of the rule, because it decides three things that must
/// agree: whether the list holds directories or commands, whether a typed row
/// is offered, and what an Enter with nothing to take says. Judging on the
/// first character alone made `/bin/sh -c "printf hi"` a directory search, so
/// the list went empty, no row was offered and Enter was silent.
///
/// A path stops being a path as soon as it carries an argument: `/usr/bin/env
/// FOO=1 agent` names a program that happens to live at an absolute path.
pub fn is_dir_search(query: &str) -> bool {
    let q = query.trim();
    looks_like_path(q) && launch::split_command(q).is_some_and(|(_, args)| args.is_empty())
}

/// The row that runs exactly what was typed, when no other row does.
///
/// The command field used to be free text and this is where that went. It is
/// the last row rather than the first, because a query is far more often a
/// filter over things that exist than a command nobody has run here.
///
/// A rooted query is a directory only while it is JUST a path. Once it carries
/// an argument it is a command line, whatever its first character: the launcher
/// itself offers `/bin/bash` as a row, so a program named by absolute path is
/// ordinary, and `/bin/sh -c "..."` was being classified as a directory. That
/// left no row to take, and Enter is guarded on there being one, so typing a
/// perfectly good command line and pressing Enter did nothing whatsoever and
/// said only that nothing matched.
pub fn typed_intent(
    all: &[Intent],
    st: &UiState,
    here: &str,
    query: &str,
    home: &str,
) -> Option<Intent> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    if is_dir_search(q) {
        return None;
    }
    launch::split_command(q)?;
    if all.iter().any(|i| i.command == q) {
        return None;
    }
    let here = launch::tidy_dir(here);
    Some(Intent::new(
        q.to_string(),
        here.clone(),
        place_of(&st.daemon.projects, &here, home),
        branch_in(st, &here),
        Band::Typed,
        "typed".to_string(),
        None,
    ))
}

/// Why a query produced no row, for an Enter that has nothing to take.
///
/// Enter is guarded on the list being non-empty, which is correct, but a
/// guarded key that does nothing and says nothing is indistinguishable from a
/// dead keyboard. Every state that can leave the list empty has a reason the
/// operator can act on, so it is said out loud.
pub fn no_row_reason(query: &str) -> String {
    let q = query.trim();
    if q.is_empty() {
        return "Type a command, or pick a row.".to_string();
    }
    // Unbalanced quotes and an empty program both land here.
    if launch::split_command(q).is_none() {
        return format!("\u{201c}{q}\u{201d} names no program to run.");
    }
    if is_dir_search(q) {
        return format!("\u{201c}{q}\u{201d} is not a directory that exists.");
    }
    format!("\u{201c}{q}\u{201d} cannot be launched from here.")
}

/// Indices into `all` that match `query`, best first, capped at [`ROWS_MAX`].
///
/// An empty query scores every row zero, so the ranking falls through to the
/// band order and the list the operator saw before typing is the list they
/// still see. Ties break on the original index for the same reason.
pub fn ranked(all: &[Intent], query: &str) -> Vec<usize> {
    let mut hits: Vec<(u32, usize)> = all
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy(&it.hay, query).map(|s| (s, i)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().take(ROWS_MAX).map(|(_, i)| i).collect()
}

/// Score `query` against an already-lowercased haystack, or `None` for a miss.
///
/// Whitespace splits the query into terms and every term must match, so
/// `claude main` narrows to a branch as well as an agent. Each term matches as
/// a subsequence, which is what lets `vtm` find `vitrum` and `cpm` find
/// `claude --permission-mode`.
pub fn fuzzy(hay: &str, query: &str) -> Option<u32> {
    let q = query.trim();
    if q.is_empty() {
        return Some(0);
    }
    let lower = q.to_lowercase();
    let mut total = 0u32;
    for term in lower.split_whitespace() {
        total = total.saturating_add(term_score(hay, term)?);
    }
    Some(total)
}

/// One term against the haystack. Contiguous beats a word start beats a hit.
///
/// Deliberately allocation-free: this runs once per row per keystroke, and the
/// obvious `hay.chars().collect()` would allocate a vector per row per term.
fn term_score(hay: &str, term: &str) -> Option<u32> {
    let mut score = 0u32;
    let mut offset = 0usize;
    let mut prev_end: Option<usize> = None;
    for c in term.chars() {
        let idx = hay.get(offset..)?.find(c)?;
        let abs = offset + idx;
        let boundary = abs == 0
            || hay[..abs]
                .chars()
                .next_back()
                .is_some_and(|p| matches!(p, ' ' | '/' | '\\' | '-' | '_' | '.' | ':'));
        score += if prev_end == Some(abs) {
            4
        } else if boundary {
            3
        } else {
            1
        };
        offset = abs + c.len_utf8();
        prev_end = Some(offset);
    }
    Some(score)
}

/// Is this query a directory rather than a command?
///
/// The one test that turns the query input into a path field. Anything rooted
/// or home-relative is a path; a bare word is a command, because `bin` is a
/// far more likely agent name than a relative directory the daemon could not
/// resolve anyway.
pub fn looks_like_path(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('/')
        || t.starts_with('~')
        || t.starts_with("./")
        || t.starts_with("../")
        || t.starts_with('\\')
        || (t.len() >= 2 && t.as_bytes()[1] == b':' && t.as_bytes()[0].is_ascii_alphabetic())
}

// ---------------------------------------------------------------------------
// Places
// ---------------------------------------------------------------------------

/// `cwd` written the way a person names it: `vitrum/app`, never the absolute
/// path.
///
/// A 47-character path in a 512px row is a string nobody reads and the reason
/// the old dialog's first field looked like configuration. Inside a known
/// project this is the project's own name plus whatever is below it, which is
/// the same word the sidebar header uses. Outside one it is the last two
/// components, which is enough to tell `/tmp/scratch` from `/var/scratch`.
///
/// Pure text. [`launch::project_key`] would canonicalise, which is a syscall,
/// and this is called once per row.
pub fn place_of(projects: &[ProjectInfo], cwd: &str, home: &str) -> String {
    let cwd = launch::tidy_dir(cwd);
    match root_of(projects, &cwd) {
        Some(p) => {
            let rest = relative(&launch::tidy_dir(&p.root), &cwd);
            if rest.is_empty() {
                p.name.clone()
            } else {
                format!("{}/{rest}", p.name)
            }
        }
        // Home is the one directory every operator reads as a single glyph, so
        // it is written the way they write it. `tail(cwd, 2)` rendered it as
        // `home/user`: two components of an absolute path with the
        // leading slash cut off, which does not look like the home directory
        // and does not even look absolute. On a fresh machine that string was
        // on EVERY row of the launcher, because a machine with no projects
        // yet suggests everything in home.
        None if !home.is_empty() && launch::is_within(home, &cwd) => shorten_home(&cwd, home),
        None => tail(&cwd, 2),
    }
}

/// The deepest known project containing `cwd`.
fn root_of<'a>(projects: &'a [ProjectInfo], cwd: &str) -> Option<&'a ProjectInfo> {
    projects
        .iter()
        .filter(|p| launch::is_within(&launch::tidy_dir(&p.root), cwd))
        .max_by_key(|p| launch::tidy_dir(&p.root).len())
}

/// Do these two directories belong to the same project?
///
/// Two known roots must be the same root. With no project to appeal to, one
/// containing the other is the honest answer: a session in `repo/crates/foo`
/// is in the same place as one in `repo` whether or not the daemon has minted
/// a project for it yet.
fn same_project(projects: &[ProjectInfo], a: &str, b: &str) -> bool {
    match (root_of(projects, a), root_of(projects, b)) {
        (Some(x), Some(y)) => launch::tidy_dir(&x.root) == launch::tidy_dir(&y.root),
        _ => launch::is_within(a, b) || launch::is_within(b, a),
    }
}

/// The part of `cwd` below `root`, with forward slashes.
fn relative(root: &str, cwd: &str) -> String {
    let r: Vec<&str> = root.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let c: Vec<&str> = cwd.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if c.len() < r.len() || c[..r.len()] != r[..] {
        return String::new();
    }
    c[r.len()..].join("/")
}

/// The last `n` components of a path, with no leading separator.
fn tail(path: &str, n: usize) -> String {
    let names: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if names.is_empty() {
        return path.to_string();
    }
    names[names.len().saturating_sub(n)..].join("/")
}

/// `~/src/vitrum` rather than the absolute path, when it is under `home`.
fn shorten_home(path: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() || !launch::is_within(home, path) {
        return path.to_string();
    }
    let rest = relative(home, path);
    if rest.is_empty() {
        "~".to_string()
    } else {
        format!("~/{rest}")
    }
}

/// The branch the daemon has resolved for a session in `cwd`, if any.
fn branch_in(st: &UiState, cwd: &str) -> Option<String> {
    st.daemon
        .sessions
        .iter()
        .find(|r| r.info.git_branch.is_some() && launch::tidy_dir(&r.info.cwd) == cwd)
        .and_then(|r| r.info.git_branch.clone())
}

// ---------------------------------------------------------------------------
// Taking a row
// ---------------------------------------------------------------------------

/// What happens when a row is taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    /// Send it.
    Go(Launch),
    /// Legal, but say this first. Taking the same row again goes anyway.
    Warn(String),
    /// Cannot run at all, whatever the operator does next.
    Refuse(String),
}

/// Turn a row into a launch, or into the sentence that stops it.
///
/// The one place in this file that touches the filesystem, and it runs on a
/// click rather than on a render. [`launch::validate`] is one `stat` for the
/// directory and one `PATH` walk for the program; paying that per keystroke,
/// which is what the previous dialog did, is why typing in it was slow.
pub fn attempt(intent: &Intent, armed: bool) -> Attempt {
    if let Some(p) = &intent.preset {
        if let Some(fault) = launch::preset_fault(p) {
            return Attempt::Refuse(fault.sentence());
        }
        return match launch::preset_launch(p, &intent.cwd) {
            Ok(l) => Attempt::Go(l),
            Err(why) => Attempt::Refuse(why),
        };
    }
    match launch::validate(&intent.cwd, &intent.command, "") {
        Err(why) => Attempt::Refuse(why),
        Ok(l) if l.warning.is_some() && !armed => Attempt::Warn(not_on_path(&intent.command)),
        Ok(l) => Attempt::Go(l),
    }
}

/// The short form of "this will not spawn", naming the program.
///
/// [`launch::validate`]'s own warning is a two-clause sentence about how the
/// daemon resolves commands, which is daemon mechanics and belongs nowhere
/// near a launch. The operator needs the name of the binary that is missing.
fn not_on_path(command_line: &str) -> String {
    let program = launch::split_command(command_line)
        .map(|(c, _)| c)
        .unwrap_or_else(|| command_line.to_string());
    format!("{program} is not on this machine's PATH.")
}

/// What the primary control does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Primary {
    /// Send this now. No dialog, no confirmation.
    Ready(Launch),
    /// Open the launcher, and show this one sentence in it.
    Choose(String),
}

/// The whole primary path: what a click on the new-session control does.
///
/// Reads the profile, ranks, and either hands back a launch to send or the
/// concrete reason it will not guess. The caller opens no layer in the first
/// case, which is the difference between one click and two.
pub fn primary_launch(st: &UiState, here: &str) -> Primary {
    let store = launch::load_launch_store();
    let home = launch::user_home();
    let here = launch::seed_cwd(here, &store, &home);
    primary_of(st, &store, &here, &home, launch::now_ms())
}

/// The decision itself, with every reading passed in.
///
/// Split out from [`primary_launch`] so the rule can be tested without a
/// profile directory, a clock or a `PATH`.
pub fn primary_of(
    st: &UiState,
    store: &LaunchStore,
    here: &str,
    home: &str,
    now_ms: u64,
) -> Primary {
    let rows = intents(st, store, &[], "", here, home, now_ms);
    let Some(top) = rows.into_iter().find(|i| i.band.launchable_now()) else {
        return Primary::Choose("Nothing has been launched here yet.".to_string());
    };
    match attempt(&top, false) {
        Attempt::Go(l) => Primary::Ready(l),
        Attempt::Warn(why) | Attempt::Refuse(why) => Primary::Choose(why),
    }
}

/// The program a command line names, without its directory.
pub fn basename(command: &str) -> &str {
    command.rsplit(['/', '\\']).next().unwrap_or(command)
}

/// The word the primary control wears: the agent it is about to start.
///
/// A bare `+` is a mystery button once the first click launches, so the
/// control says what it will do. The program name alone, not the whole
/// command line: `claude --permission-mode plan` does not fit a sidebar at its
/// 224px floor, and the argument is not what distinguishes one launch from
/// another.
///
/// `None` means there is nothing this operator has chosen before, so the
/// control will open the list instead of launching and says so.
///
/// Reads the profile and NOTHING else. The sidebar draws on every daemon
/// message, so the label cannot afford the `stat` and the `PATH` walk
/// [`primary_of`] does; those belong on the click. The label is therefore what
/// the control will TRY, and [`primary_launch`] reports it if it cannot.
pub fn top_word(store: &LaunchStore, now_ms: u64) -> Option<String> {
    if let Some(e) = launch::ranked_history(store, now_ms).first() {
        return Some(basename(&e.command).to_string());
    }
    store.presets.first().map(|p| p.label.clone())
}

/// [`top_word`], read off the UI thread.
///
/// One small profile file, but the sidebar is the hottest surface in the
/// product and it is on a filesystem somebody may have mounted over a network.
/// The caller re-runs this when the session list changes, which is the only
/// moment the answer can have moved.
pub async fn primary_word_now() -> Option<String> {
    off_thread(|| top_word(&launch::load_launch_store(), launch::now_ms())).await
}

/// What the primary half of the control reads.
///
/// Collapsed to the 3rem rail there is room for the glyph and nothing else,
/// and the tooltip carries the word there. The place is deliberately NOT in
/// the label: the sidebar's own project headers are directly underneath it, so
/// a project name on the button is the one thing on that surface that is
/// already said somewhere else.
pub fn go_label(word: Option<&str>, wide: bool) -> String {
    if !wide {
        return "+".to_string();
    }
    match word {
        Some(w) => format!("+ {w}"),
        None => "New session".to_string(),
    }
}

/// The tooltip on the primary half: the whole sentence, place included.
pub fn go_tip(word: Option<&str>, place: &str) -> String {
    match word {
        Some(w) if place.is_empty() => {
            format!("Start {w}. Ctrl+Shift+N to choose something else.")
        }
        Some(w) => format!("Start {w} in {place}. Ctrl+Shift+N to choose something else."),
        None => "Choose what to start (Ctrl+Shift+N).".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// Directories worth offering in the `in` field, most recently used first.
///
/// Where you have worked, not a walk of `$HOME`: the operator is nearly always
/// going back somewhere they have already been, and a walk to answer that
/// would be a syscall storm for a question the daemon has already answered.
/// Distinct, because twenty sessions in one checkout is one place to offer.
pub fn recent_dirs(state: &UiState, last_cwd: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut rows: Vec<&vitrum_model::SessionView> = state.daemon.sessions.iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.info.last_activity_ms.max(r.info.created_at_ms)));
    for r in rows {
        let d = launch::tidy_dir(&r.info.cwd);
        if !seen.contains(&d) {
            seen.push(d);
        }
    }
    for pr in &state.daemon.projects {
        let d = launch::tidy_dir(&pr.root);
        if !seen.contains(&d) {
            seen.push(d);
        }
    }
    let last = launch::tidy_dir(last_cwd);
    if !last.is_empty() && !seen.contains(&last) {
        seen.push(last);
    }
    seen.truncate(DIR_MAX);
    seen
}

/// One thing a row does when it is taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Launch this.
    Go(Intent),
    /// Make this the place, and go back to picking an agent.
    Cd(String),
}

/// What one row draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowView {
    /// Which agent this row would start, drawn.
    ///
    /// The same mark the sidebar puts on a running session, because it is the
    /// same question: which agent is this. Without it the launcher is a list
    /// of words that all look alike at a glance, and the operator reads five
    /// command names to find the one they meant. A directory row carries
    /// `None`: it starts nothing, so claiming an agent for it would be a
    /// confident wrong answer.
    pub mark: Option<crate::agent::AgentMark>,
    /// The row's leading text.
    pub text: String,
    /// The place chip, and the absolute path behind its `title`.
    pub place: Option<(String, String)>,
    pub branch: Option<String>,
    /// The row's tooltip: the exact thing it will do.
    pub tip: String,
}

/// Draw one pick.
pub fn view(pick: &Pick, home: &str) -> RowView {
    match pick {
        Pick::Go(i) => RowView {
            // The PROGRAM, not the command line. `Intent::command` holds the
            // whole line, quoting and arguments included, and the resolver
            // matches a program name exactly on purpose: it will not guess
            // that `claudex` is Claude. Handing it `bash -l` therefore drew
            // the unknown mark on a shell, which is a confident wrong answer
            // about the one row every operator has.
            mark: Some(
                crate::agent::AgentKind::of(
                    &launch::split_command(&i.command)
                        .map(|(program, _)| program)
                        .unwrap_or_else(|| i.command.clone()),
                )
                .mark(),
            ),
            text: i.text().to_string(),
            place: Some((i.place.clone(), i.cwd.clone())),
            branch: i.branch.clone(),
            tip: match &i.preset {
                Some(p) => preset_tip(p),
                None if i.note.is_empty() => format!("{} in {}", i.command, i.cwd),
                None => format!("{} in {}, {}", i.command, i.cwd, i.note),
            },
        },
        Pick::Cd(path) => RowView {
            mark: None,
            text: tail(path, 1),
            place: Some((shorten_home(&parent(path), home), path.clone())),
            branch: None,
            tip: path.clone(),
        },
    }
}

/// Everything above the last component of `path`.
fn parent(path: &str) -> String {
    match path.trim_end_matches(['/', '\\']).rfind(['/', '\\']) {
        Some(0) => "/".to_string(),
        Some(at) => path[..at].to_string(),
        None => path.to_string(),
    }
}

/// The one line the launcher is allowed to say, or nothing at all.
///
/// The old dialog carried three permanent captions explaining the daemon's
/// naming rules, how ranking worked, and which project a directory joined.
/// None of them changed anything the operator could do. This says something
/// only when the surface is in a state that is genuinely unusual.
///
/// "This directory is not a project yet" was a FOURTH such caption and is
/// gone. It fired whenever the working directory was not already a known
/// project, which on a new machine is every directory and on any machine is
/// the normal state before the first launch: the daemon mints the project as
/// part of starting the session, so there was nothing for the operator to do
/// about it and nothing they would have done differently. A line that is
/// present by default is a permanent caption however it is phrased.
pub fn note(said: Option<&str>, rows: usize, query: &str) -> Option<String> {
    if let Some(msg) = said {
        return Some(msg.to_string());
    }
    if rows == 0 {
        let q = query.trim();
        return Some(if q.is_empty() {
            "Nothing launched before, and no agent found on PATH.".to_string()
        } else {
            format!("Nothing matches \u{201c}{q}\u{201d}.")
        });
    }
    None
}

/// Does row `n` (one-based) get to show its Ctrl+digit badge?
///
/// A preset the operator bound to Ctrl+3 is checked before the row numbers, so
/// on that one launcher Ctrl+3 fires the preset. Drawing a 3 on the third row
/// anyway would be a shortcut the surface displays and the product never
/// fires, which is exactly the defect the preset chord parser exists to avoid.
/// The row is still reachable by arrow, by click and by query.
pub fn digit_free(presets: &[SavedPreset], n: usize) -> bool {
    let chord = launch::Chord {
        key: n.to_string(),
        ctrl: true,
        alt: false,
        shift: false,
    };
    launch::preset_for_chord(presets, &chord).is_none()
}

/// The digit drawn on row `i`, or nothing at all.
///
/// Empty rather than absent. The slot is always emitted, so a row that cannot
/// carry a number does not pull its own text 24px left and put two left edges
/// in one list.
pub fn key_of(presets: &[SavedPreset], i: usize) -> String {
    let n = i + 1;
    if n <= ROWS_MAX && digit_free(presets, n) {
        n.to_string()
    } else {
        String::new()
    }
}

/// The digit a keydown means, from the physical key.
///
/// `code` rather than `key` for the same reason [`chord_of`] reads it there:
/// the top row's digits are layout- and Shift-dependent in `key` and are not
/// in `code`.
fn digit_of(code: &str) -> Option<usize> {
    let d = code.strip_prefix("Digit")?;
    let n: usize = d.parse().ok()?;
    (d.len() == 1 && (1..=9).contains(&n)).then_some(n)
}

// ---------------------------------------------------------------------------
// The component
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
pub struct NewSessionProps {
    pub state: Signal<UiState>,
    pub seed: NewSessionSeed,
    pub on_launch: EventHandler<(ProjectId, Launch)>,
    pub on_dismiss: EventHandler<()>,
}

#[component]
pub fn NewSessionDialog(props: NewSessionProps) -> Element {
    let state = props.state;
    let on_launch = props.on_launch;
    let on_dismiss = props.on_dismiss;
    let seeded = props.seed.cwd.clone();

    // Resolved once when the launcher opens. One small profile read and two
    // environment reads; no directory walk and no `PATH` walk.
    let store = use_signal(launch::load_launch_store);
    let home = use_signal(launch::user_home);
    let shell = use_signal(launch::default_shell);
    let opened_ms = use_signal(launch::now_ms);

    // The one unbounded read on this surface. Five `PATH` lookups, each across
    // every directory in `PATH`, so it happens on a thread and the list paints
    // without it.
    let found = use_resource(move || async move { off_thread(launch::detected_agents).await });

    let mut here = use_signal(move || launch::seed_cwd(&seeded, &store.read(), &home.read()));
    let mut query = use_signal(String::new);
    let mut hi = use_signal(|| 0usize);
    let mut armed = use_signal(|| false);
    let mut said = use_signal(|| None::<String>);

    // Keyed on the directory to scan, not on the whole query, so typing
    // further into one folder spawns no thread and re-runs no syscall.
    let scan_dir = use_memo(move || {
        let text = query.read().clone();
        if looks_like_path(&text) {
            launch::split_dir_input(&text, &home.read()).0
        } else {
            String::new()
        }
    });
    let entries = scanned_dirs(scan_dir);

    // What is typed in the `in` field, which is not the same thing as `here`.
    // `here` is the resolved directory a launch would use; this is the text,
    // including the trailing separator that means "show me what is inside".
    // Completing against `here` would drop that separator and re-offer the
    // folder you just descended into.
    let mut dir_text = use_signal(move || shorten_home(&here.read(), &home.read()));
    let dir_scan = use_memo(move || launch::split_dir_input(&dir_text.read(), &home.read()).0);
    let dir_entries = scanned_dirs(dir_scan);

    // Completions for the `in` field, or the directories you have launched in
    // before when there is nothing to complete against.
    //
    // One list, not a filesystem popup beside a recents datalist. Two lists
    // under one field is two things to learn and two ways to be surprised, and
    // the platform datalist could not be keyboard-driven from here anyway.
    let dir_picks = use_memo(move || {
        let typed = dir_text.read().clone();
        // Nothing typed is not "no answer": it is the moment the operator has
        // said least and where you last worked is the most useful thing on
        // screen. Completing an empty field against the filesystem would offer
        // the root's children, which is never where anybody is going.
        if typed.trim().is_empty() {
            return recent_dirs(&state.peek(), &store.read().last_cwd);
        }
        let fragment = launch::split_dir_input(&typed, &home.read()).1;
        let scanned = dir_entries.read();
        let list: &[String] = match (*scanned).as_ref() {
            Some(l) => l.as_slice(),
            None => &[],
        };
        let hits = launch::filter_dirs(list, &fragment, DIR_MAX);
        // An exact, complete directory name is not a suggestion: offering
        // `software/` while `software/` is what the field already says makes
        // Tab a no-op and the list a mirror of the input.
        let whole = launch::tidy_dir(&launch::expand_home(&typed, &home.read()));
        if hits.len() == 1 && launch::tidy_dir(&hits[0]) == whole {
            return Vec::new();
        }
        hits
    });
    let mut dir_hi = use_signal(|| 0usize);

    // Two memos rather than one. Ranking depends on the profile, the place and
    // the PATH answer; filtering depends only on the query. Split, a keystroke
    // re-runs the second alone, which allocates nothing.
    //
    // The daemon is read through `peek`, NOT `read`, and that is the difference
    // between this surface costing nothing and costing 65us per PTY frame.
    // `read` subscribes, so a daemon pushing output twenty times a second would
    // rebuild every row twenty times a second for an answer that has not
    // changed. It is also the better behaviour: a session appearing in another
    // window must not reshuffle the list under the operator's hand, for exactly
    // the reason the late `PATH` answer is ranked below the recents. The
    // launcher is a point-in-time surface and says so.
    let all = use_memo(move || {
        let st = state.peek();
        let found = found.read();
        let agents: &[Detected] = match (*found).as_ref() {
            Some(v) => v.as_slice(),
            None => &[],
        };
        intents(
            &st,
            &store.read(),
            agents,
            &shell.read(),
            &here.read(),
            &home.read(),
            opened_ms(),
        )
    });

    let picks = use_memo(move || {
        let text = query.read().clone();
        if is_dir_search(&text) {
            let fragment = launch::split_dir_input(&text, &home.read()).1;
            let scanned = entries.read();
            let list: &[String] = match (*scanned).as_ref() {
                Some(l) => l.as_slice(),
                None => &[],
            };
            return launch::filter_dirs(list, &fragment, DIR_MAX)
                .into_iter()
                .map(Pick::Cd)
                .collect::<Vec<_>>();
        }
        let rows = all.read();
        let mut out: Vec<Pick> = listed(&rows, &text)
            .into_iter()
            .map(|i| Pick::Go(rows[i].clone()))
            .collect();
        if let Some(extra) = typed_intent(&rows, &state.peek(), &here.read(), &text, &home.read()) {
            if out.len() >= ROWS_MAX {
                out.truncate(ROWS_MAX - 1);
            }
            out.push(Pick::Go(extra));
        }
        out
    });

    // Take a row: launch it, or make it the place.
    let mut take = move |i: usize| {
        let pick = match picks.read().get(i) {
            Some(p) => p.clone(),
            None => return,
        };
        match pick {
            Pick::Cd(path) => {
                here.set(path);
                query.set(String::new());
                push_query("");
                hi.set(0);
                armed.set(false);
                said.set(None);
            }
            // Peeked into a local BEFORE the match. A scrutinee's temporary
            // lives to the end of the match, so reading it inline holds a
            // `GenerationalRef` across every arm and the two arms below
            // cannot then borrow `armed` mutably to move the state on.
            Pick::Go(intent) => {
                let was_armed = *armed.peek();
                match attempt(&intent, was_armed) {
                    Attempt::Go(l) => {
                        // Sent, not recorded. `main.rs::start_session` is the
                        // single place a launch leaves this client, so the
                        // history write, the flash and the focus correlation
                        // are identical whether the launch came from here, from
                        // the sidebar's control or from the context menu.
                        // Recording it here too was a double bump: one launch
                        // counted twice, which quietly skews the ranking this
                        // whole surface is built on.
                        let pid = {
                            let st = state.peek();
                            launch::resolve_project(&st.daemon.projects, &l.cwd).0
                        };
                        on_launch.call((pid, l));
                    }
                    Attempt::Warn(why) => {
                        armed.set(true);
                        said.set(Some(format!("{why} Take it again to run it anyway.")));
                    }
                    Attempt::Refuse(why) => {
                        armed.set(false);
                        said.set(Some(why));
                    }
                }
            }
        }
    };

    let home_now = home.read().clone();
    // The rows are read through the guard and drawn straight out of it. This
    // used to clone the whole `Vec<Pick>` first, which on a full list is nine
    // `Intent`s and their five owned strings each, deep-copied on every
    // keystroke to build a `Vec<RowView>` that is itself owned.
    let (count, views): (usize, Vec<RowView>) = {
        let rows = picks.read();
        (rows.len(), rows.iter().map(|p| view(p, &home_now)).collect())
    };
    let cur = if count == 0 {
        0
    } else {
        (*hi.read()).min(count - 1)
    };
    let presets = store.read().presets.clone();
    let line = note(said.read().as_deref(), count, &query.read());
    // The one place every row shares, or `None` when they differ.
    //
    // The place chip exists to tell two otherwise identical rows apart. When
    // every row carries the SAME place it tells nothing apart, and the panel
    // renders one string five times down its right edge. That is the state a
    // fresh machine opens in, because with no projects yet every suggestion
    // runs in home: five rows reading `~`, none of which was a fact the
    // operator needed per row. Said once, above the list, it is context; said
    // on every row it is noise.
    let here_now = launch::tidy_dir(&here.read());

    // Which completion row is highlighted, asked once per row instead of twice.
    // The clamp is what keeps a stale highlight from naming a row that a newer,
    // shorter list no longer has.
    let dir_selected =
        move |i: usize| i == dir_hi().min(dir_picks.read().len().saturating_sub(1));

    rsx! {
        div {
            class: "rg-layer rg-layer--dim",
            onclick: move |_| on_dismiss.call(()),
            div {
                class: "rg-sheet rg-sheet--launcher",
                role: "dialog",
                aria_label: "Start a session",
                onclick: move |e| e.stop_propagation(),

                // WHERE, then WHAT. Two fields, each labelled, each holding
                // one thing.
                //
                // This surface used to be a single box reading "Agent,
                // project, branch, or a /path": one field that silently
                // changed mode depending on whether what you typed looked
                // like a path, so the operator could not tell what they were
                // setting or how to set the other half. A session is a
                // command and a directory. Saying so in two fields is the
                // whole design.
                div { class: "rg-launch__field",
                    label { class: "rg-launch__label", r#for: "rg-launch-dir", "in" }
                    input {
                        class: "rg-launch__dir",
                        id: "rg-launch-dir",
                        r#type: "text",
                        spellcheck: false,
                        autocomplete: "off",
                        role: "combobox",
                        aria_expanded: if dir_picks.read().is_empty() { "false" } else { "true" },
                        aria_controls: "rg-launch-dirs",
                        placeholder: "Directory",
                        initial_value: "{shorten_home(&here.read(), &home_now)}",
                        oninput: move |e| {
                            let typed = e.value();
                            // `~` is what the operator types and what every
                            // recent is offered as, so it has to be accepted
                            // back: storing the literal string would spawn the
                            // session in a directory called `~`.
                            let full = launch::expand_home(&typed, &home.read());
                            here.set(launch::tidy_dir(&full));
                            dir_text.set(typed);
                            dir_hi.set(0);
                            said.set(None);
                        },
                        onkeydown: move |e: KeyboardEvent| {
                            let hits = dir_picks.read().clone();
                            let count = hits.len();
                            let cur = dir_hi().min(count.saturating_sub(1));
                            match e.key() {
                                Key::ArrowDown if count > 0 => {
                                    e.prevent_default();
                                    dir_hi.set((cur + 1) % count);
                                }
                                Key::ArrowUp if count > 0 => {
                                    e.prevent_default();
                                    dir_hi.set((cur + count - 1) % count);
                                }
                                // Descend, exactly as a shell does. The
                                // separator is what makes the next Tab offer
                                // what is INSIDE rather than re-offer this.
                                Key::Tab if !e.modifiers().shift() && count > 0 => {
                                    e.prevent_default();
                                    let mut next = shorten_home(&hits[cur], &home.read());
                                    next.push(MAIN_SEPARATOR);
                                    here.set(launch::tidy_dir(&hits[cur]));
                                    push_dir(&next);
                                    dir_text.set(next);
                                    dir_hi.set(0);
                                    said.set(None);
                                }
                                // Tab with nothing to complete moves to `run`,
                                // because a dead key in a two-field form reads
                                // as the field being broken.
                                Key::Tab if !e.modifiers().shift() => {
                                    e.prevent_default();
                                    push_query(&query.read().clone());
                                }
                                // The directory is set as you type, so Enter
                                // here means "done with this field", not
                                // "launch": launching from the place field
                                // would start whatever the other field happens
                                // to hold.
                                Key::Enter => {
                                    e.prevent_default();
                                    if count > 0 {
                                        let mut next = shorten_home(&hits[cur], &home.read());
                                        next.push(MAIN_SEPARATOR);
                                        here.set(launch::tidy_dir(&hits[cur]));
                                        push_dir(&next);
                                        dir_text.set(next);
                                        dir_hi.set(0);
                                    }
                                    push_query(&query.read().clone());
                                }
                                _ => {}
                            }
                        },
                    }
                    if !dir_picks.read().is_empty() {
                        ul {
                            class: "rg-launch__dirs",
                            id: "rg-launch-dirs",
                            role: "listbox",
                            aria_label: "Directories",
                            for (i, full) in dir_picks.read().iter().enumerate() {
                                li {
                                    class: if dir_selected(i) { DIROPT_ON } else { DIROPT },
                                    key: "{full}",
                                    role: "option",
                                    aria_selected: dir_selected(i),
                                    title: "{full}",
                                    // Kept off mousedown so the field never
                                    // loses focus: a blur would move the caret
                                    // out from under the click.
                                    onmousedown: move |e| e.prevent_default(),
                                    onclick: {
                                        let full = full.clone();
                                        move |_| {
                                            let mut next = shorten_home(&full, &home.read());
                                            next.push(MAIN_SEPARATOR);
                                            here.set(launch::tidy_dir(&full));
                                            push_dir(&next);
                                            dir_text.set(next);
                                            dir_hi.set(0);
                                            said.set(None);
                                        }
                                    },
                                    span { class: "rg-launch__dirleaf", "{launch::leaf(full)}" }
                                }
                            }
                        }
                    }
                }

                div { class: "rg-launch__field",
                    label { class: "rg-launch__label", r#for: "rg-launch-q", "run" }
                input {
                    class: "rg-launch__query",
                    id: "rg-launch-q",
                    r#type: "text",
                    spellcheck: false,
                    autocomplete: "off",
                    role: "combobox",
                    aria_expanded: "true",
                    aria_autocomplete: "list",
                    aria_controls: "rg-launch-list",
                    aria_activedescendant: "rg-launch-r{cur}",
                    placeholder: "Command, or an agent name",
                    initial_value: "",
                    onmounted: move |e| {
                        spawn(async move {
                            let _ = e.set_focus(true).await;
                        });
                    },
                    oninput: move |e| {
                        query.set(e.value());
                        hi.set(0);
                        armed.set(false);
                        said.set(None);
                    },
                    onkeydown: move |e: KeyboardEvent| {
                        let m = e.modifiers();
                        if m.meta() {
                            return;
                        }
                        // A saved preset's chord is checked before the row
                        // numbers, because it is the operator's own binding and
                        // this surface is the only place it ever fires.
                        if m.ctrl() || m.alt() {
                            let code = format!("{:?}", e.code());
                            let chord = crate::keymap::chord_from_event(
                                &e.key().to_string(),
                                &code,
                                m.ctrl(),
                                m.alt(),
                                m.shift(),
                            );
                            let hit = launch::preset_for_chord(&store.read().presets, &chord)
                                .cloned();
                            if let Some(preset) = hit {
                                e.prevent_default();
                                e.stop_propagation();
                                match launch::preset_fault(&preset) {
                                    Some(fault) => said.set(Some(fault.sentence())),
                                    None => match launch::preset_launch(&preset, &here.read()) {
                                        Ok(l) => {
                                            let pid = {
                                                let st = state.peek();
                                                launch::resolve_project(&st.daemon.projects, &l.cwd).0
                                            };
                                            on_launch.call((pid, l));
                                        }
                                        Err(why) => said.set(Some(why)),
                                    },
                                }
                                return;
                            }
                            // Ctrl+S: keep this one.
                            //
                            // Saving used to live only in Settings, which
                            // means the moment you know a command is worth
                            // keeping is the moment you have to leave the
                            // surface you are on and retype it. It is saved
                            // from here, where you just typed it, with the
                            // directory you just chose.
                            //
                            // The label is the command line, because asking
                            // for a name is a second question at the exact
                            // moment the operator wanted to start working.
                            // Settings renames it and binds a chord to it.
                            if m.ctrl() && !m.alt() && e.key() == Key::Character("s".to_string()) {
                                e.prevent_default();
                                let line = query.read().trim().to_string();
                                let cwd = launch::tidy_dir(&here.read());
                                save_typed(store, said, &line, &cwd);
                                return;
                            }
                            if m.ctrl() && !m.alt()
                                && let Some(n) = digit_of(&code) {
                                    e.prevent_default();
                                    take(n - 1);
                                    return;
                                }
                        }
                        match e.key() {
                            Key::ArrowDown if count > 0 => {
                                e.prevent_default();
                                hi.set((cur + 1) % count);
                            }
                            Key::ArrowUp if count > 0 => {
                                e.prevent_default();
                                hi.set((cur + count - 1) % count);
                            }
                            // Complete the highlighted row into the field
                            // rather than committing it: a directory gains a
                            // separator so the next Tab offers what is inside,
                            // exactly as a shell does, and a command is filled
                            // in whole with the caret left at the end so a flag
                            // can be added to it without retyping the line.
                            //
                            // Tab used to do nothing at all on a command row.
                            // A dead key in a form reads as the form being
                            // broken, which is the same reason the `in` field's
                            // Tab falls through to this field instead of
                            // sitting there doing nothing.
                            Key::Tab if !m.shift() => {
                                // Always swallowed. There is nothing else
                                // focusable on this surface, so a Tab that got
                                // through would move focus off the query and
                                // out of the launcher entirely.
                                e.prevent_default();
                                // Both cloned out before the field is
                                // written. The picks memo reads `query`, so
                                // holding either guard across the write
                                // borrows the same generational slot twice.
                                let chosen = picks.read().get(cur).cloned();
                                let typed = query.read().clone();
                                if let Some(pick) = chosen
                                    && let Some(next) = completion(&pick, &typed)
                                {
                                    push_query(&next);
                                    query.set(next);
                                    hi.set(0);
                                    said.set(None);
                                }
                            }
                            Key::Enter => {
                                e.prevent_default();
                                if count > 0 {
                                    take(cur);
                                } else {
                                    said.set(Some(no_row_reason(&query.read())));
                                }
                            }
                            _ => {}
                        }
                    },
                }
                    // Saving lives where the command was typed. Before this it
                    // was Ctrl+S and nothing else, and before that it was only
                    // in Settings, so the moment an operator decided a line was
                    // worth keeping they had to leave the surface they were on
                    // and retype it. The chord still works and is named in the
                    // tooltip, so this control teaches it instead of replacing
                    // it.
                    button {
                        class: "rg-launch__save",
                        r#type: "button",
                        disabled: query.read().trim().is_empty(),
                        title: "Save this command as a preset (Ctrl+S)",
                        "aria-label": "Save this command as a preset",
                        // Off mousedown, so the field does not blur and take
                        // the launcher down before the click lands.
                        onmousedown: move |e| e.prevent_default(),
                        onclick: move |_| {
                            let line = query.read().trim().to_string();
                            let cwd = launch::tidy_dir(&here.read());
                            save_typed(store, said, &line, &cwd);
                        },
                        "Save"
                    }
                }

                // The operator's own saved choices, above everything ranked,
                // because a preset is a decision already made rather than a
                // guess. Only with an empty query, for the same reason the
                // recents below carry: once you are typing, the ranked list is
                // the answer and the presets rank into it.
                if query.read().is_empty() {
                    crate::ui::presets::Presets {
                        presets: store.read().presets.clone(),
                        here: here_now.clone(),
                        on_launch: move |l: launch::Launch| {
                            let pid = launch::resolve_project(
                                &state.peek().daemon.projects,
                                &l.cwd,
                            )
                            .0;
                            on_launch.call((pid, l));
                        },
                    }
                }

                // Where you were, not just what you ran. The suggestion list
                // below ranks COMMANDS and carries one directory each, so it
                // cannot offer "the same command in the other checkout". Only
                // with an empty query: once you are typing, the list below is
                // the answer and a second list is noise.
                if query.read().is_empty() {
                    crate::ui::recents::Recents {
                        entries: launch::recents(&store.read()).to_vec(),
                        // `peek`, not `read`, for the reason the two memos
                        // above give: `read` here subscribed the whole
                        // launcher to `UiState`, so a daemon streaming output
                        // twenty times a second rebuilt this surface and
                        // re-cloned the project list twenty times a second
                        // while somebody was typing into it.
                        projects: state.peek().daemon.projects.clone(),
                        home: home.read().clone(),
                        on_launch: move |l: launch::Launch| {
                            let pid = launch::resolve_project(
                                &state.read().daemon.projects,
                                &l.cwd,
                            )
                            .0;
                            on_launch.call((pid, l));
                        },
                    }
                }

                ul {
                    class: "rg-launch__list",
                    id: "rg-launch-list",
                    role: "listbox",
                    aria_label: "Launch",
                    for (i, v) in views.iter().enumerate() {
                        li {
                            class: if i == cur { "rg-launch__row rg-launch__row--on" } else { "rg-launch__row" },
                            key: "{v.text}|{i}",
                            id: "rg-launch-r{i}",
                            role: "option",
                            aria_selected: (i == cur).to_string(),
                            title: "{v.tip}",
                            // Kept off mousedown so the query never loses
                            // focus: a blur would close the surface before the
                            // click landed.
                            onmousedown: move |e| e.prevent_default(),
                            onclick: move |_| take(i),
                            // A reserved slot, never a conditional element. A
                            // row whose Ctrl+digit a saved preset already owns
                            // draws no digit, and if the slot collapsed with it
                            // the rows either side would sit on two different
                            // left edges.
                            span { class: "rg-launch__key", "{key_of(&presets, i)}" }
                            if let Some(mark) = v.mark {
                                svg {
                                    class: "rg-launch__agent",
                                    view_box: "0 0 16 16",
                                    fill: "none",
                                    stroke: "currentColor",
                                    stroke_width: "1.25",
                                    stroke_linecap: "round",
                                    stroke_linejoin: "round",
                                    "aria-hidden": "true",
                                    path { d: "{mark.stroke}" }
                                    if !mark.fill.is_empty() {
                                        path { d: "{mark.fill}", fill: "currentColor", stroke: "none" }
                                    }
                                }
                            } else {
                                // A directory row. The box is held so the two
                                // kinds of row share one text column; dropping
                                // the element would step every path row 24px
                                // left of every agent row.
                                span { class: "rg-launch__agent" }
                            }
                            span { class: "rg-launch__text", "{v.text}" }
                            // A place chip only when the row would run
                            // somewhere OTHER than the `in` field says. The
                            // field already states the common case; repeating
                            // it on every row is the same number twice.
                            if let Some((place, full)) = &v.place
                                && launch::tidy_dir(full) != here_now
                            {
                                span { class: "rg-launch__place", title: "{full}", "{place}" }
                            }
                            if let Some(branch) = &v.branch {
                                span { class: "rg-launch__branch", "{branch}" }
                            }
                        }
                    }
                }

                if let Some(msg) = line {
                    div { class: "rg-launch__note", "{msg}" }
                }
            }
        }
    }
}

/// The directories inside `dir`, scanned off the UI thread, empty while `dir`
/// is empty.
///
/// One scanner, called once per field. The `run` field completes a path typed
/// as a query and the `in` field completes the place, which are two questions
/// with one answer: what is inside this directory. Written twice they were two
/// `read_dir` walks that could disagree, and the second one was free to be the
/// one that forgot [`off_thread`]. A directory on an unreachable mount blocks
/// in the kernel for as long as the mount wants, so that is a frozen window,
/// not a slow list.
fn scanned_dirs(dir: Memo<String>) -> Resource<Vec<String>> {
    use_resource(move || {
        let dir = dir();
        async move {
            if dir.is_empty() {
                Vec::new()
            } else {
                off_thread(move || launch::list_dirs(&dir)).await
            }
        }
    })
}

/// Write `value` into the query element and put the caret after it.
///
/// The query input is UNCONTROLLED: it carries `initial_value`, which sets
/// `defaultValue` once, rather than `value`, which Dioxus marks volatile and
/// re-asserts on every render with the rule "if the element differs from what
/// was rendered, overwrite the element". That is not a style preference. While
/// somebody is typing, the element is always ahead of the signal by however
/// many `oninput` events are still crossing the IPC bridge, so any render NOT
/// caused by the newest keystroke rolls the element back and the characters in
/// between are gone.
///
/// Measured on the real display before this changed: typing
/// `/tmp/newsession-tree/proj1` into the controlled field at 120ms per
/// character produced `/tmp/esesso-tree/proj1`, four characters short. The
/// directory scan resolving mid-burst is what supplies the extra renders, and
/// making the render cheaper does not close the race, it only narrows it.
///
/// So the element owns the text and the signal follows it through `oninput`.
/// The one direction that still has to go the other way is this function, for
/// when the launcher itself sets the query from a Tab or a directory pick.
/// That only ever happens on a key or a click, with no keystrokes in flight.
fn push_query(value: &str) {
    push_input("rg-launch-q", value);
}

/// Write `value` into the `in` field and put the caret after it.
///
/// Same reason as [`push_query`]: the directory field is uncontrolled, so a
/// completion the launcher chooses has to be written to the element.
fn push_dir(value: &str) {
    push_input("rg-launch-dir", value);
}

/// Write `value` into the element with `id` and put the caret after it.
fn push_input(id: &str, value: &str) {
    // Escaped through serde rather than by hand: `~/it's "here"` is a legal
    // directory name and pasting it into a script raw is a syntax error.
    let text = serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    let el = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_string());
    document::eval(&format!(
        "{{const el=document.getElementById({el});\
         if(el){{el.value={text};\
         el.setSelectionRange(el.value.length,el.value.length);el.focus();}}}}"
    ));
}

/// Keep the typed line as a preset, and report what happened.
///
/// One function behind two controls. Ctrl+S and the Save button beside the
/// field must not be able to drift into saving different things, or into
/// explaining the same refusal two different ways.
///
/// The profile is taken from the signal the launcher already loaded when it
/// opened, never re-read here: a second file read on a keypress is exactly
/// what `the_open_path_never_walks_path_or_the_filesystem` exists to stop.
///
/// The label is the command line itself, because asking for a name is a second
/// question at the exact moment the operator wanted to start working. Settings
/// renames it and binds a chord to it.
fn save_typed(
    mut store: Signal<LaunchStore>,
    mut said: Signal<Option<String>>,
    line: &str,
    cwd: &str,
) {
    let existing = store.read().presets.clone();
    said.set(Some(
        match launch::preset_from_typed(line, cwd, &existing) {
            Ok(preset) => {
                let label = preset.label.clone();
                let mut next = existing;
                next.push(preset);
                match launch::save_presets(&next) {
                    Ok(()) => {
                        store.write().presets = next;
                        format!("Saved \u{201c}{label}\u{201d}. Bind a key to it in Settings.")
                    }
                    Err(why) => why,
                }
            }
            Err(why) => why,
        },
    ));
}

/// The rows the list is allowed to draw for `text`, best first, as indices
/// into `rows`.
///
/// PURE, and separate from [`ranked`] on purpose. `ranked` answers "what
/// matches, in what order"; this answers "what may be shown", and those are
/// two different questions the moment a row can also be drawn somewhere else
/// on the surface.
///
/// With nothing typed, the saved presets are drawn as their own band of chips
/// above this list, so ranking them in here as well would put every preset on
/// screen twice under two different affordances. The moment the operator
/// types, the band is gone and this list is the only answer, so presets rank
/// back into it and stay searchable by the label they were given.
pub fn listed(rows: &[Intent], text: &str) -> Vec<usize> {
    let banded = text.is_empty();
    ranked(rows, text)
        .into_iter()
        .filter(|&i| !(banded && rows[i].band == Band::Preset))
        .collect()
}

/// What Tab writes into the `run` field for `pick`, or `None` when the field
/// already says it.
///
/// PURE, so the completion rule is provable without a DOM: the handler does
/// nothing except write what this returns.
///
/// A directory gains a separator, because the separator is what makes the next
/// Tab offer what is INSIDE it rather than re-offer the folder itself. A
/// command is filled in whole, caret left at the end, so a flag can be added
/// to a remembered line without retyping it.
///
/// `None` rather than the same string back: rewriting the field with what it
/// already holds moves the caret and resets the highlight for nothing.
pub fn completion(pick: &Pick, typed: &str) -> Option<String> {
    let next = match pick {
        Pick::Cd(path) => {
            let mut next = path.clone();
            if !next.ends_with(['/', '\\']) {
                next.push(MAIN_SEPARATOR);
            }
            next
        }
        Pick::Go(intent) => intent.command.clone(),
    };
    (next != typed).then_some(next)
}

/// The tooltip on a preset row or chip: the command, or why it will not run.
pub fn preset_tip(preset: &SavedPreset) -> String {
    match launch::preset_fault(preset) {
        Some(fault) => fault.sentence(),
        None => match preset
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            Some(cwd) => format!(
                "{} in {cwd}",
                launch::join_command(&preset.command, &preset.args)
            ),
            None => launch::join_command(&preset.command, &preset.args),
        },
    }
}

// ---------------------------------------------------------------------------
// Off-thread work
// ---------------------------------------------------------------------------

/// Run a blocking job on its own thread and await the answer.
///
/// Two callers, both for the same reason. `read_dir` on a stale network mount
/// blocks in the kernel for as long as the mount wants, and no timer inside
/// [`launch::list_dirs`] can shorten a syscall that has not returned; the
/// `PATH` walk behind [`launch::detected_agents`] is five lookups across every
/// directory in `PATH`, any one of which can be an automounted share. On the
/// UI thread either is a frozen window; here it is a thread nobody waits on.
///
/// The thread exits when the job returns. An answer that arrives after the
/// launcher has closed is dropped by [`use_resource`], which has already
/// cancelled the future that would have received it.
async fn off_thread<T, F>(job: F) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let slot = Arc::new(Slot::new());
    let worker = Arc::clone(&slot);
    std::thread::spawn(move || worker.fill(job()));
    Take { slot }.await
}

/// A one-shot handoff from a worker thread to a task.
struct Slot<T> {
    inner: Mutex<SlotInner<T>>,
}

struct SlotInner<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

impl<T> Slot<T> {
    fn new() -> Self {
        Self {
            inner: Mutex::new(SlotInner {
                value: None,
                waker: None,
            }),
        }
    }

    /// Poisoning cannot lose data here: the only code inside the lock is two
    /// field moves, neither of which can panic, so a poisoned lock means some
    /// other thread died holding it and the contents are still sound.
    fn lock(&self) -> MutexGuard<'_, SlotInner<T>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Called on the worker thread, never on the UI thread.
    fn fill(&self, value: T) {
        let waker = {
            let mut inner = self.lock();
            inner.value = Some(value);
            inner.waker.take()
        };
        // Woken outside the lock: a waker that resumes its task inline would
        // otherwise re-enter `poll` and deadlock on a lock this thread holds.
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// The future [`off_thread`] awaits.
struct Take<T> {
    slot: Arc<Slot<T>>,
}

impl<T> Future for Take<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut inner = self.slot.lock();
        if let Some(value) = inner.value.take() {
            return Poll::Ready(value);
        }
        // Replaced rather than pushed: one task ever waits on one slot, and a
        // re-poll under a different waker must leave the newest one behind or
        // the wake goes to a task that has been moved off this future.
        if !inner
            .waker
            .as_ref()
            .is_some_and(|w| w.will_wake(cx.waker()))
        {
            inner.waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct RenameProps {
    pub seed: RenameSeed,
    pub on_rename: EventHandler<(SessionId, String)>,
    pub on_dismiss: EventHandler<()>,
}

/// Rename one session.
///
/// The new title goes to the daemon, not into a client-side map. A title only
/// this window knows vanishes on restart and is invisible to a second window,
/// which is the prototype smell this pass exists to remove; the server owns
/// session identity, so it owns the name.
///
/// This is also where the launcher's optional label went. Naming a session is
/// something an operator does to a session they can see, once, well after it
/// started; asking for it on the way in put a third field on the fastest path
/// in the product to serve a case that already had its own surface.
#[component]
pub fn RenameDialog(props: RenameProps) -> Element {
    let session = props.seed.session;
    let mut value = use_signal(|| props.seed.title.clone());
    let mut error = use_signal(|| None::<String>);

    let mut commit = move || {
        let next = value.read().trim().to_string();
        if next.is_empty() {
            error.set(Some(
                "A session needs a name. Type one, or cancel to keep the current title."
                    .to_string(),
            ));
            return;
        }
        props.on_rename.call((session, next));
    };

    rsx! {
        div {
            class: "rg-layer rg-layer--dim",
            onclick: move |_| props.on_dismiss.call(()),
            div {
                class: "rg-sheet rg-sheet--narrow",
                role: "dialog",
                aria_label: "Rename session",
                onclick: move |e| e.stop_propagation(),

                div { class: "rg-sheet__head",
                    span { class: "rg-sheet__title", "Rename session" }
                }
                div { class: "rg-field",
                    input {
                        class: "rg-field__input",
                        id: "rg-rename",
                        r#type: "text",
                        autocomplete: "off",
                        value: "{value}",
                        oninput: move |e| {
                            value.set(e.value());
                            error.set(None);
                        },
                        onkeydown: move |e| {
                            if e.key() == Key::Enter {
                                e.prevent_default();
                                commit();
                            }
                        },
                    }
                    span { class: "rg-field__hint",
                        "Saved on the daemon, so every window sees it."
                    }
                }
                if let Some(msg) = error.read().clone() {
                    div { class: "rg-sheet__error", "{msg}" }
                }
                div { class: "rg-sheet__foot",
                    button {
                        class: "rg-btn",
                        r#type: "button",
                        onclick: move |_| props.on_dismiss.call(()),
                        "Cancel"
                    }
                    button {
                        class: "rg-btn rg-btn--primary",
                        r#type: "button",
                        onclick: move |_| commit(),
                        "Rename"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

/// What the launcher SHOWS, as opposed to what it ranks.
///
/// This surface was rebuilt after being judged poorly designed, and the two
/// defects behind that were both about repetition and about paths written in
/// a way nobody writes them.
#[cfg(test)]
mod what_the_launcher_shows;
