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

use vitrum_proto::ProjectInfo;

use crate::launch::{self, CommandSource, Detected, Launch, LaunchStore, SavedPreset};
use crate::state::{Layer, UiState};

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
/// The whole ranked history plus the agents plus the shell. Truncating here
/// would hide a command from the query that the operator has definitely run,
/// so the ceiling follows `launcher.historyLimit` rather than the count a
/// fresh profile happens to keep.
fn suggest_max(st: &UiState) -> usize {
    st.daemon.settings.launcher.history_max() + 8
}

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
/// followed by the agents really on `PATH`. This function adds the two things
/// that function cannot know: WHERE, and the sessions the daemon already has
/// running.
///
/// NO SHELL ROW. The list used to end with the login shell, so a fresh
/// profile on a machine with no agent installed opened the launcher on a
/// single row reading `/bin/bash`. That is a launcher for a terminal
/// multiplexer, and any screenshot of this surface shipped the claim; see
/// `AGENTS.md`, "Demos show agents, not shell output". A shell is still
/// launchable — type it, and `Band::Typed` takes it exactly as written —
/// because running one is ordinary. Offering it is the product arguing for
/// itself in the wrong category.
///
/// `detected` may be empty. That is the normal state for the first few
/// milliseconds a launcher is open, and the reason agents sit below recents:
/// a band that fills in late must never displace the highlighted row.
pub fn intents(
    st: &UiState,
    store: &LaunchStore,
    detected: &[Detected],
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
    for s in launch::command_suggestions(store, detected, "", now_ms, suggest_max(st)) {
        let band = match s.source {
            CommandSource::History => Band::Recent,
            CommandSource::Detected => Band::Agent,
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
            Band::Agent => agents.push(row),
            _ => recent.push(row),
        }
    }

    // A preset's caption says "saved", except when the preset names an agent
    // vitrum knows and this machine does not have. A fresh profile is seeded
    // with every known agent, so on most machines some of these rows cannot
    // run, and the launcher validates a preset on the CLICK rather than on
    // every render — `preset_fault` is a stat and a PATH walk, and paying for
    // it while drawing would put both on every keystroke. Without a caption
    // those rows look launchable and refuse when taken.
    //
    // `detected` is the answer to the same question, already computed once
    // when the dialog opened, so this costs nothing. It is applied only to
    // commands in the known-agent table: any other program missing from
    // `detected` merely means vitrum was not looking for it, which is not
    // evidence that it is absent.
    let missing = |command: &str| {
        launch::is_known_agent(command) && !detected.iter().any(|d| d.command == command)
    };

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
                if missing(&p.command) {
                    "not installed".to_string()
                } else {
                    "saved".to_string()
                },
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
    let rows = intents(st, store, &[], here, home, now_ms);
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
#[cfg(test)]
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
                crate::agent::AgentMarks::mark(vitrum_model::AgentKind::of(
                    &launch::split_command(&i.command)
                        .map(|(program, _)| program)
                        .unwrap_or_else(|| i.command.clone()),
                )),
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
#[cfg(test)]
fn digit_of(code: &str) -> Option<usize> {
    let d = code.strip_prefix("Digit")?;
    let n: usize = d.parse().ok()?;
    (d.len() == 1 && (1..=9).contains(&n)).then_some(n)
}

// ---------------------------------------------------------------------------
// The component
// ---------------------------------------------------------------------------

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
/// Three callers, all for the same reason. `read_dir` on a stale network
/// mount blocks in the kernel for as long as the mount wants, and no timer
/// inside [`launch::list_dirs`] can shorten a syscall that has not returned;
/// the `PATH` walk behind [`launch::detected_agents`] is one lookup per known
/// agent across every directory in `PATH`, any one of which can be an
/// automounted share, and [`crate::ui::firstrun::read_machine`] does that walk
/// plus a profile read. On the UI thread any of them is a frozen window; here
/// it is a thread nobody waits on.
///
/// The thread exits when the job returns. An answer that arrives after the
/// surface has closed is dropped by [`use_resource`], which has already
/// cancelled the future that would have received it.
pub(crate) async fn off_thread<T, F>(job: F) -> T
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

/// Put the surface `layer` names on screen, or take down whatever is on it.
///
/// The one entry point the shell calls when [`crate::state::WindowState::layer`]
/// moves. The match is exhaustive, so a variant added to [`Layer`] stops the
/// build rather than opening nothing, and every arm ends in exactly one
/// [`crate::shell::Shell::present`] or one dismiss: a layer is one surface,
/// never a stack.
pub(crate) fn present_layer(shell: &crate::shell::Shell, layer: &Layer) {
    use std::rc::Rc;

    use crate::shell::Dialog;

    match layer {
        Layer::None => shell.dismiss(),
        Layer::Shortcuts => {
            let prefs = shell.peek(|st| st.daemon.settings.keyboard.clone());
            shell.present(crate::ui::shortcuts::native::build(shell, &prefs) as Rc<dyn Dialog>);
        }
        Layer::Search => {
            shell.present(crate::ui::search::native::build(shell) as Rc<dyn Dialog>);
        }
        // Owned by the settings module, which builds its own sheet and knows
        // which tab it is on.
        Layer::Settings(_) => crate::ui::settings::sheet::present_layer(shell, layer),
        Layer::Onboarding => {
            shell.present(crate::ui::onboarding::native::build(shell) as Rc<dyn Dialog>);
        }
        Layer::WhatsNew => {
            let releases = shell.peek(|st| {
                crate::ui::whatsnew::whats_new(st.daemon.settings.last_seen_version().as_ref())
            });
            let seen = shell.clone();
            let sheet = crate::ui::whatsnew::native::build(shell, &releases, move || {
                seen.update(|st| {
                    st.daemon
                        .settings
                        .mark_seen(&crate::update::current_version());
                    st.window.layer = Layer::None;
                });
                seen.peek(crate::ui::settings::commit);
            });
            shell.present(sheet as Rc<dyn Dialog>);
        }
        // A menu is positioned, so it is presented as a popover at the point
        // that was clicked and GTK does the clamping. An empty menu opens
        // nothing: a popover with no items is a surface that swallows the next
        // click for nothing.
        Layer::Menu(menu) => {
            if let Some(sheet) = crate::ui::menu::native::build(shell, menu.clone()) {
                shell.present_at(sheet as Rc<dyn Dialog>, menu.x as i32, menu.y as i32);
            }
        }
        Layer::NewSession(seed) => {
            shell.present(native::build(shell, seed) as Rc<dyn Dialog>);
        }
        Layer::Rename(seed) => {
            shell.present(native::rename(shell, seed) as Rc<dyn Dialog>);
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

/// The launcher itself, built as GTK widgets.
pub mod native;
