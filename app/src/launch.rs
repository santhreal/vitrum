//! What this machine can start: the agent commands the new-session dialog
//! offers, and the session daemon the whole application depends on.
//!
//! Everything in the first half is pure or does one filesystem stat, and none
//! of it runs unless the dialog is open. There is no probing at startup and no
//! background rescan: an idle window must not be walking `PATH`.
//!
//! The point of the availability check is honesty. A dialog that lets you pick
//! "Claude Code" on a machine with no `claude` binary, and answers with a spawn
//! error from the daemon three seconds later, has wasted the user's time and
//! told them nothing about which of the two machines is missing it. The daemon
//! is bound to loopback, so "on this machine's PATH" is the same question as
//! "will the daemon find it".
//!
//! The second half is [`ensure_daemon`], and it exists because until it did,
//! a first launch on a clean machine painted a red "disconnected" banner with
//! a Retry button that could never succeed. Nothing was listening, nothing was
//! ever going to start, and the UI never said so. Every outcome in
//! [`Autostart`] is a distinct sentence for exactly that reason: "the daemon
//! binary is not installed" and "something else is already on port 7737" are
//! different problems with different fixes, and one shared "disconnected"
//! tells the operator neither.

use serde::{Deserialize, Serialize};
use std::io::Read;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// An agent command this machine actually has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    /// What the suggestion row says on the right.
    pub label: &'static str,
    /// The command the row fills in.
    pub command: &'static str,
}

/// Agent commands worth looking for.
///
/// A fixed list is a starting point, not a limit: the command field is free
/// text and anything on `PATH` works. This is the difference between us and a
/// shell built on per-harness event streams, where an unlisted agent is not
/// merely unlisted but unsupported.
const AGENTS: &[(&str, &str)] = &[
    ("Claude Code", "claude"),
    ("Codex", "codex"),
    ("Gemini CLI", "gemini"),
    ("opencode", "opencode"),
    ("veyyon", "veyyon"),
];

/// The agent binaries resolvable on this machine right now, in table order.
///
/// Only what is really installed. The picker used to render all five with the
/// missing ones greyed, which put four names the operator cannot run in front
/// of them every time the dialog opened and buried the one they could. A name
/// that is not on `PATH` is not a suggestion.
///
/// One `PATH` walk per entry, so callers resolve this once when the dialog
/// opens and never while the window idles.
pub fn detected_agents() -> Vec<Detected> {
    AGENTS
        .iter()
        .filter(|(_, cmd)| on_path(cmd))
        .map(|(label, command)| Detected { label, command })
        .collect()
}

/// Interactive shell to spawn when the user has not chosen anything else.
pub fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

/// Split a command line into the program and its arguments.
///
/// Double quotes group. A backslash escapes only a double quote or another
/// backslash; anywhere else it is a literal backslash, because on Windows it
/// is the path separator and treating it as an escape turns
/// `C:\Program Files\agent.exe` into `C:Program Filesagent.exe`. An argument
/// containing a space is quoted, not backslash-escaped.
///
/// Returns `None` for a line with no program.
///
/// Deliberately not a shell. There is no globbing, no variable expansion and
/// no pipeline here, because the string is handed to `posix_spawn` on the
/// other side and pretending otherwise would make `foo | bar` look like it
/// worked while spawning a program called `foo` with two odd arguments.
pub fn split_command(line: &str) -> Option<(String, Vec<String>)> {
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if matches!(chars.peek(), Some('"') | Some('\\')) => {
                cur.push(chars.next().expect("peeked"));
                has = true;
            }
            '"' => {
                quoted = !quoted;
                // An empty pair of quotes is a real empty argument, so the
                // word exists even though no character was pushed.
                has = true;
            }
            c if c.is_whitespace() && !quoted => {
                if has {
                    words.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        words.push(cur);
    }
    let mut it = words.into_iter();
    let program = it.next()?;
    // A program that cannot be executed is not a command line.
    //
    // `"` alone parses to a single empty word, because a quote pair is a real
    // empty argument and the quote sets the word as present. That made
    // `split_command("\"")` answer `Some(("", []))`: a command whose program
    // is the empty string, which `preset_from_typed` would happily save and
    // which can never launch. A NUL is the same class: `exec` takes a
    // C string, so a program containing one fails at spawn no matter what.
    //
    // Refused here, at the parse, so the operator is told when they type it
    // rather than when they next press the key they bound it to.
    if program.is_empty() || program.contains('\0') {
        return None;
    }
    Some((program, it.collect()))
}

/// Can this machine resolve `command` to an executable?
///
/// A name containing a separator is checked as a path. A bare name is looked
/// up in `PATH`, honouring `PATHEXT` on Windows so `claude` finds `claude.cmd`.
pub fn on_path(command: &str) -> bool {
    if command.is_empty() {
        return false;
    }
    if command.contains(MAIN_SEPARATOR) || command.contains('/') {
        return is_executable(Path::new(command));
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        if dir.as_os_str().is_empty() {
            return false;
        }
        let base = dir.join(command);
        if is_executable(&base) {
            return true;
        }
        cfg!(windows)
            && windows_extensions()
                .iter()
                .any(|ext| is_executable(&dir.join(format!("{command}{ext}"))))
    })
}

/// Extensions Windows treats as executable, from `PATHEXT`.
fn windows_extensions() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_string())
        .collect()
}

/// Is this path a file the OS would run?
///
/// On Unix that means the execute bit is set for somebody; a readable but
/// non-executable file on `PATH` is not a command. Windows has no execute bit,
/// so there the extension check above carries the meaning and existence is
/// enough.
#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// What the new-session dialog will send, or why it will not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub cwd: String,
    pub command: String,
    pub args: Vec<String>,
    pub title: Option<String>,
    /// Set when the launch is legal but something about it is worth saying
    /// before the user commits, such as a command that is not on `PATH`.
    pub warning: Option<String>,
}

/// Normalised form of a directory, for comparing two paths for identity.
///
/// Canonicalised when the path exists, so `/src/vitrum`, `/src/vitrum/`, and a
/// symlink to it all key the same project. A path that does not exist keeps
/// its trimmed text: the dialog still has to show it back to the user, and
/// erroring here would lose what they typed.
///
/// The canonicalisation itself lives in [`crate::inbox::project_key`], which
/// is also what the sidebar folds the daemon's project list with. Two
/// implementations of "is this the same directory" would disagree the first
/// time one of them learned about case-insensitive volumes, which is exactly
/// what happened: this one keyed `/src/Dev` and `/src/dev` apart on macOS, and
/// the daemon then held two projects for one repo.
pub fn project_key(path: &str) -> String {
    crate::inbox::project_key(path)
}

/// Is `path` inside `root`, or the same directory?
///
/// Compared component by component, never as a string prefix: `/src/reg` is a
/// textual prefix of `/src/vitrum` but is a different project, and grouping
/// the two would put sessions under a header they have nothing to do with.
pub fn is_within(root: &str, path: &str) -> bool {
    let root = Path::new(root);
    let path = Path::new(path);
    path.components().count() >= root.components().count()
        && root
            .components()
            .zip(path.components())
            .all(|(a, b)| a == b)
}

/// The project a session in `cwd` belongs to, and whether it is a new one.
///
/// Prefers the deepest known project that contains `cwd`, so a session started
/// in `repo/crates/foo` lands under `repo` rather than minting a second
/// project for a subdirectory. Falls back to a project id derived from the
/// path itself.
///
/// Deriving rather than counting is what makes the id stable. The protocol has
/// no "create project" message: the client owns project identity and the
/// daemon records it on first use. A counter would give the same directory a
/// different id after a restart, and the sidebar would grow a second header
/// for a project the user already has.
pub fn resolve_project(
    projects: &[vitrum_proto::ProjectInfo],
    cwd: &str,
) -> (vitrum_proto::ProjectId, bool) {
    let key = project_key(cwd);
    let best = projects
        .iter()
        .filter(|p| is_within(&project_key(&p.root), &key))
        .max_by_key(|p| project_key(&p.root).len());
    match best {
        Some(p) => (p.id, false),
        None => (vitrum_proto::ProjectId(fnv1a(&key)), true),
    }
}

/// FNV-1a over an arbitrary key.
///
/// Any stable 64-bit function would do; FNV is here because it is six lines
/// and needs no dependency. Two callers: project ids, where a collision
/// merges two projects in the sidebar and is not a correctness problem for
/// the daemon, which keys sessions by session id; and preset ids, where the
/// only writer resolves a collision before the file is written.
fn fnv1a(key: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Drop trailing separators from a directory path, without eating a root.
///
/// `/src/vitrum/` and `/src/vitrum` are the same directory, and the dialog
/// produces the first spelling every time a completion is accepted, because
/// appending the separator is what lets the next Tab descend into it. Passing
/// it on would put two spellings of one directory in front of [`project_key`]
/// and into the daemon's session list, where the sidebar would show them as
/// two places.
///
/// A bare root is left alone. `C:` is not `C:\`: it is the drive's current
/// directory, which is somewhere else and usually somewhere the operator has
/// never been.
pub fn tidy_dir(path: &str) -> String {
    let text = path.trim();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.ends_with(':') {
        return text.to_string();
    }
    trimmed.to_string()
}

/// Validate the dialog's fields.
///
/// Returns the exact message the dialog shows on failure. The messages name
/// the field and the machine, because "invalid input" on a two-field form is
/// the same as no message at all.
pub fn validate(cwd: &str, command_line: &str, title: &str) -> Result<Launch, String> {
    let cwd = tidy_dir(cwd);
    let cwd = cwd.as_str();
    if cwd.is_empty() {
        return Err("Pick a project or type a working directory.".to_string());
    }
    if !Path::new(cwd).is_dir() {
        return Err(format!("{cwd} is not a directory on this machine."));
    }
    let Some((command, args)) = split_command(command_line) else {
        return Err("Type a command to run, or pick one above.".to_string());
    };
    let warning = (!on_path(&command))
        .then(|| format!("{command} is not on this machine's PATH. Launching anyway will fail unless the daemon resolves it differently."));
    let title = title.trim();
    Ok(Launch {
        cwd: cwd.to_string(),
        command,
        args,
        title: (!title.is_empty()).then(|| title.to_string()),
        warning,
    })
}

// ---------------------------------------------------------------------------
// The launch store
// ---------------------------------------------------------------------------

/// The profile file holding presets, command history and the last directory.
///
/// Separate from `ui.json`, and that is deliberate twice over. `ui.json` is
/// the window document and is rewritten whole on every geometry change, so
/// folding command history into it would mean a history write on every window
/// resize. And the presets in here are the one part of the profile a person is
/// expected to open in an editor, which wants a small file with an obvious
/// shape rather than a corner of a large one.
pub const LAUNCH_STORE_FILE: &str = "launch.json";

/// Schema version of [`LaunchStore`].
///
/// A file claiming a higher number is not read at all. Loading it would drop
/// the fields this build does not know about, and the next save would delete
/// them; a preset the operator can no longer see is worse than a profile that
/// looks empty after a downgrade and is intact when they go forward again.
pub const LAUNCH_STORE_VERSION: u32 = 1;

/// History entries that survive a save.
///
/// The list is ranked before it is truncated, so what falls off the end is
/// what was least worth suggesting, not what happens to be oldest.
pub const HISTORY_MAX: usize = 60;

/// A named command the operator saved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPreset {
    /// Stable across an edit so a picker can key on it. Minted once by
    /// [`mint_preset_id`] and never renumbered.
    pub id: u64,
    /// What the button says.
    pub label: String,
    /// Program name or path, exactly as [`split_command`] would yield it.
    pub command: String,
    /// Arguments, already split. Stored split rather than as a line, so a
    /// preset's meaning cannot change under a future edit to the quoting
    /// rules; [`join_command`] renders it back for a one-line field.
    #[serde(default)]
    pub args: Vec<String>,
    /// Directory this preset pins, or `None` to run wherever the dialog
    /// currently points.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Chord that fires this preset while the new-session dialog is open.
    /// Read through [`parse_chord`], so an unparseable string never fires.
    #[serde(default)]
    pub shortcut: Option<String>,
    /// Icon slug from [`crate::ui::icons`], or `None` to draw the one the
    /// command implies. An unknown slug resolves to the same default, so a
    /// profile from a newer build still opens.
    #[serde(default)]
    pub icon: Option<String>,
}

/// One command line this operator has actually launched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Times launched. Saturates rather than wrapping.
    #[serde(default)]
    pub count: u32,
    /// Epoch milliseconds of the most recent launch.
    #[serde(default)]
    pub last_used_ms: u64,
    /// Icon slug the operator chose for this command. See
    /// [`SavedPreset::icon`].
    #[serde(default)]
    pub icon: Option<String>,
}

/// Recent commands kept, at most.
///
/// Small on purpose. This list is meant to be read at a glance, and a
/// twelfth row is already further away than typing the command; the ranked
/// [`HISTORY_MAX`] list behind the launcher's query is where depth lives.
pub const RECENTS_MAX: usize = 12;

/// One command, in one directory, the last time it ran there.
///
/// Separate from [`HistoryEntry`], which is keyed on the command alone and
/// ranked by frequency. The same agent started in two checkouts is one
/// history entry and two recents, because "the thing I was doing in that
/// repo" is the thing this list is for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Where it ran. Part of the identity of the row, not a detail of it.
    #[serde(default)]
    pub cwd: String,
    /// Epoch milliseconds of the most recent run.
    #[serde(default)]
    pub last_used_ms: u64,
    /// Icon slug the operator chose. See [`SavedPreset::icon`].
    #[serde(default)]
    pub icon: Option<String>,
}

/// Everything the new-session dialog remembers between launches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchStore {
    #[serde(default = "current_store_version")]
    pub version: u32,
    #[serde(default)]
    pub presets: Vec<SavedPreset>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    /// The last few distinct commands, newest first. Written by [`remember`]
    /// alongside `history`, and read without ranking or a clock.
    #[serde(default)]
    pub recents: Vec<RecentEntry>,
    /// The directory the last session was started in. Survives a restart, and
    /// is the dialog's fallback when nothing is focused and no project exists.
    #[serde(default)]
    pub last_cwd: String,
}

fn current_store_version() -> u32 {
    LAUNCH_STORE_VERSION
}

impl Default for LaunchStore {
    fn default() -> Self {
        Self {
            version: LAUNCH_STORE_VERSION,
            presets: Vec::new(),
            history: Vec::new(),
            recents: Vec::new(),
            last_cwd: String::new(),
        }
    }
}

/// Serialise the store. Pure: touches no path and reads no environment.
pub fn encode_launch_store(store: &LaunchStore) -> String {
    serde_json::to_string_pretty(store).expect("strings and integers always serialise")
}

/// Parse the store, defaulting anything unreadable.
///
/// Never fails, and that is the point. This file holds convenience, not
/// truth: a corrupt one costs the operator their command history, and
/// refusing to open the dialog over it would cost them the ability to start a
/// session at all.
pub fn parse_launch_store(text: &str) -> LaunchStore {
    let Ok(store) = serde_json::from_str::<LaunchStore>(text) else {
        return LaunchStore::default();
    };
    if store.version > LAUNCH_STORE_VERSION {
        return LaunchStore::default();
    }
    store
}

/// Where the launch store lives, beside `ui.json`.
pub fn launch_store_path() -> Result<PathBuf, vitrum_os::PathError> {
    Ok(vitrum_os::AppPaths::for_current_platform()?
        .config_dir
        .join(LAUNCH_STORE_FILE))
}

/// Read the launch store, or defaults.
pub fn load_launch_store() -> LaunchStore {
    let Ok(path) = launch_store_path() else {
        return LaunchStore::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_launch_store(&text),
        Err(_) => LaunchStore::default(),
    }
}

/// Write the launch store atomically.
///
/// Write-then-rename, for the same reason `ui.json` does it: a machine that
/// dies mid-write would otherwise leave a truncated file that reads back as
/// defaults forever, silently discarding every preset the operator saved.
pub fn save_launch_store(store: &LaunchStore) -> Result<(), String> {
    let path = launch_store_path().map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, encode_launch_store(store))
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The saved presets, in display order.
pub fn presets_saved() -> Vec<SavedPreset> {
    load_launch_store().presets
}

/// Replace the preset list, keeping history and the last directory.
///
/// Read-modify-write, and the read cannot fail: an unparseable or
/// future-version file defaults the rest of the document rather than
/// abandoning the save. The only error returned is a real write failure, so a
/// settings panel showing it is showing something the operator can fix.
pub fn save_presets(presets: &[SavedPreset]) -> Result<(), String> {
    let mut store = load_launch_store();
    store.version = LAUNCH_STORE_VERSION;
    store.presets = presets.to_vec();
    save_launch_store(&store)
}

/// A stable id for a new preset.
pub fn mint_preset_id(label: &str, command: &str) -> u64 {
    let mut key = String::with_capacity(label.len() + command.len() + 1);
    key.push_str(label);
    key.push('\u{1f}');
    key.push_str(command);
    fnv1a(&key)
}

/// Wall clock in epoch milliseconds, for history timestamps.
///
/// Not the render clock. Nothing here is drawn per row and nothing here is
/// compared against another row's reading; this is the stamp written to the
/// profile when a session is actually launched, and the one the dialog takes
/// once when it opens so ranking cannot cost a syscall per keystroke.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// This user's home directory, for `~` in the directory field.
///
/// Empty when the platform cannot answer, which [`expand_home`] treats as "no
/// home to expand" rather than as an error. Resolved once per dialog, not per
/// keystroke.
pub fn user_home() -> String {
    vitrum_os::paths::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Join a program and its arguments back into one line.
///
/// The exact inverse of [`split_command`], which is what lets a preset be
/// stored as `{command, args}` and edited as a single field. A word is left
/// bare only when splitting it gives it back unchanged; anything else is
/// quoted, with both quote and backslash escaped inside. Checking rather than
/// pattern-matching is what handles the trap: `C:\tools\x` is safe bare
/// because a backslash before an ordinary letter is literal, while `C:\\x` is
/// not, because that pair collapses to one backslash on the way back.
pub fn join_command(command: &str, args: &[String]) -> String {
    let mut out = quote_word(command);
    for a in args {
        out.push(' ');
        out.push_str(&quote_word(a));
    }
    out
}

fn quote_word(word: &str) -> String {
    let bare = !word.is_empty()
        && !word.chars().any(char::is_whitespace)
        && split_command(word).is_some_and(|(p, a)| p == word && a.is_empty());
    if bare {
        return word.to_string();
    }
    let mut s = String::with_capacity(word.len() + 2);
    s.push('"');
    for c in word.chars() {
        if c == '"' || c == '\\' {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('"');
    s
}

/// Age brackets the recency multiplier uses, most recent first.
const RECENCY: &[(u64, u64)] = &[
    (3_600_000, 100),
    (86_400_000, 70),
    (7 * 86_400_000, 50),
    (30 * 86_400_000, 30),
];

/// Multiplier for anything older than the last bracket.
const RECENCY_FLOOR: u64 = 10;

/// Rank one history entry. Higher sorts first.
///
/// Frequency times a recency multiplier, rather than either alone. Pure
/// recency puts a command run once yesterday above one run four hundred times
/// over a year; pure frequency freezes the list against whatever the operator
/// used to do six months ago. Brackets rather than a decay curve because the
/// result has to be an integer a test can name exactly, and because the
/// difference between "an hour ago" and "ninety minutes ago" is not a
/// difference anybody wants reflected in a menu order.
pub fn history_score(entry: &HistoryEntry, now_ms: u64) -> u64 {
    let age = now_ms.saturating_sub(entry.last_used_ms);
    let mult = RECENCY
        .iter()
        .find(|(limit, _)| age <= *limit)
        .map_or(RECENCY_FLOOR, |(_, m)| *m);
    u64::from(entry.count).saturating_mul(mult)
}

/// History, best first.
///
/// Fully ordered, never merely sorted by score: two entries with the same
/// score fall back to recency and then to the text, so the dropdown does not
/// reshuffle between two openings of the same dialog.
pub fn ranked_history(store: &LaunchStore, now_ms: u64) -> Vec<&HistoryEntry> {
    let mut v: Vec<&HistoryEntry> = store.history.iter().collect();
    v.sort_by(|a, b| {
        history_score(b, now_ms)
            .cmp(&history_score(a, now_ms))
            .then(b.last_used_ms.cmp(&a.last_used_ms))
            .then_with(|| a.command.cmp(&b.command))
            .then_with(|| a.args.cmp(&b.args))
    });
    v
}

/// Record one launch in `store`, in memory.
///
/// An existing entry is bumped rather than duplicated, keyed on the program
/// and its arguments together: `claude` and `claude --permission-mode plan`
/// are two different things to offer, and merging them would suggest a
/// command the operator has never run.
pub fn remember(store: &mut LaunchStore, command: &str, args: &[String], cwd: &str, now_ms: u64) {
    store.version = LAUNCH_STORE_VERSION;
    let cwd = cwd.trim();
    if !cwd.is_empty() {
        store.last_cwd = cwd.to_string();
    }
    match store
        .history
        .iter_mut()
        .find(|e| e.command == command && e.args == args)
    {
        Some(e) => {
            e.count = e.count.saturating_add(1);
            e.last_used_ms = now_ms;
        }
        None => store.history.push(HistoryEntry {
            command: command.to_string(),
            args: args.to_vec(),
            count: 1,
            last_used_ms: now_ms,
            icon: None,
        }),
    }
    if store.history.len() > HISTORY_MAX {
        store.history = ranked_history(store, now_ms)
            .into_iter()
            .take(HISTORY_MAX)
            .cloned()
            .collect();
    }
    remember_recent(store, command, args, cwd, now_ms);
}

/// Move one command to the top of the recents list, in memory.
///
/// Keyed on the command, its arguments and the directory together. Bumping
/// rather than appending is what keeps the list distinct: an operator who
/// restarts the same agent in the same repo eleven times wants one row, not
/// eleven identical ones pushing everything else off the end.
///
/// The chosen icon survives the bump. It belongs to the command, not to the
/// run, and losing it on the next launch would make the picker look broken.
fn remember_recent(
    store: &mut LaunchStore,
    command: &str,
    args: &[String],
    cwd: &str,
    now_ms: u64,
) {
    let cwd = tidy_dir(cwd);
    let existing = store
        .recents
        .iter()
        .position(|e| e.command == command && e.args == args && e.cwd == cwd);
    let mut entry = match existing {
        Some(i) => store.recents.remove(i),
        None => RecentEntry {
            command: command.to_string(),
            args: args.to_vec(),
            cwd,
            ..RecentEntry::default()
        },
    };
    entry.last_used_ms = now_ms;
    store.recents.insert(0, entry);
    store.recents.truncate(RECENTS_MAX);
}

/// The recents list, newest first.
///
/// A borrow and no work at all: the order is the stored order, so a surface
/// that draws this on every render pays nothing for it.
pub fn recents(store: &LaunchStore) -> &[RecentEntry] {
    &store.recents
}

/// The one-line form of a recent command, for a row and for a tooltip.
pub fn recent_line(entry: &RecentEntry) -> String {
    join_command(&entry.command, &entry.args)
}

/// The launch a recent row describes, in the directory it last ran in.
///
/// Goes through [`validate`], so a checkout deleted since the last run
/// reports the sentence the launcher would rather than a spawn failure three
/// seconds later.
pub fn recent_launch(entry: &RecentEntry) -> Result<Launch, String> {
    validate(&entry.cwd, &recent_line(entry), "")
}

/// Record one launch on disk.
///
/// Best effort by design: a profile directory that cannot be written costs
/// the operator their suggestions, not their session, so the caller reports
/// the error and launches anyway.
pub fn record_launch(command: &str, args: &[String], cwd: &str, now_ms: u64) -> Result<(), String> {
    let mut store = load_launch_store();
    remember(&mut store, command, args, cwd, now_ms);
    save_launch_store(&store)
}

/// Where a command suggestion came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    /// Launched before, on this machine, by this operator.
    History,
    /// An agent binary found on `PATH`.
    Detected,
    /// The login shell.
    Shell,
}

/// One row of the command field's dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSuggestion {
    /// The line the field is filled with.
    pub line: String,
    /// The caption on the right: why this row is here.
    pub note: String,
    pub source: CommandSource,
}

/// Commands to offer for `query`, best first.
///
/// History first, ranked, because what this operator launches predicts the
/// next launch better than what this machine happens to have installed. A
/// history line that starts with the query beats one that merely contains it.
/// Then agent binaries actually on `PATH`, then the login shell.
///
/// Nothing is invented. With an empty history the list is the detected agents
/// and the shell; on a machine with neither it is empty, and an empty
/// dropdown is the honest answer.
///
/// `detected` and `shell` are parameters rather than calls so the dialog pays
/// for the `PATH` walk once when it opens instead of once per keystroke.
pub fn command_suggestions(
    store: &LaunchStore,
    detected: &[Detected],
    shell: &str,
    query: &str,
    now_ms: u64,
    limit: usize,
) -> Vec<CommandSuggestion> {
    let q = query.trim().to_lowercase();
    let mut seen: Vec<String> = Vec::new();
    let mut starts: Vec<CommandSuggestion> = Vec::new();
    let mut contains: Vec<CommandSuggestion> = Vec::new();

    for e in ranked_history(store, now_ms) {
        let line = join_command(&e.command, &e.args);
        if seen.contains(&line) {
            continue;
        }
        let lower = line.to_lowercase();
        let bucket = if q.is_empty() || lower.starts_with(&q) {
            &mut starts
        } else if lower.contains(&q) {
            &mut contains
        } else {
            continue;
        };
        seen.push(line.clone());
        bucket.push(CommandSuggestion {
            line,
            note: uses(e.count),
            source: CommandSource::History,
        });
    }
    let mut out = starts;
    out.append(&mut contains);

    for d in detected {
        let line = d.command.to_string();
        if seen.contains(&line) {
            continue;
        }
        if !q.is_empty()
            && !line.to_lowercase().contains(&q)
            && !d.label.to_lowercase().contains(&q)
        {
            continue;
        }
        seen.push(line.clone());
        out.push(CommandSuggestion {
            line,
            note: d.label.to_string(),
            source: CommandSource::Detected,
        });
    }

    let shell = shell.trim();
    if !shell.is_empty()
        && !seen.iter().any(|s| s == shell)
        && (q.is_empty() || shell.to_lowercase().contains(&q))
    {
        out.push(CommandSuggestion {
            line: shell.to_string(),
            note: "login shell".to_string(),
            source: CommandSource::Shell,
        });
    }

    out.truncate(limit);
    out
}

/// "used once" / "used 7 times", for a history row's caption.
fn uses(count: u32) -> String {
    match count {
        0 | 1 => "used once".to_string(),
        n => format!("used {n} times"),
    }
}

// ---------------------------------------------------------------------------
// Directory completion
// ---------------------------------------------------------------------------

/// Longest walk [`list_dirs`] will do before giving up on a directory.
///
/// A budget, not a promise. `read_dir` itself can block for as long as the
/// filesystem wants and no timer inside this function shortens a syscall
/// wedged on a dead network mount; that is why the caller runs this on a
/// thread of its own and drops an answer that arrives after the field has
/// moved on. What this bounds is a directory that is merely enormous.
const COMPLETE_BUDGET: Duration = Duration::from_millis(150);

/// Entries examined before the walk stops, however fast they arrive.
const COMPLETE_SCAN_MAX: usize = 20_000;

/// Rows the directory dropdown shows.
pub const COMPLETE_MAX: usize = 8;

/// Expand a leading `~` against `home`.
///
/// An empty `home` means the platform could not answer, and the text is left
/// exactly as typed rather than turned into a path rooted at nothing.
pub fn expand_home(input: &str, home: &str) -> String {
    if home.is_empty() {
        return input.to_string();
    }
    if input == "~" {
        return home.to_string();
    }
    match input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        Some(rest) => format!(
            "{}{MAIN_SEPARATOR}{rest}",
            home.trim_end_matches(['/', '\\'])
        ),
        None => input.to_string(),
    }
}

/// Split typed text into the directory to scan and the fragment to match.
///
/// A trailing separator means "list this directory"; anything else means the
/// last component is a partial name. So does a path ending in `.` or `..`,
/// which has no last component to complete.
///
/// A relative path yields an empty directory and therefore no completions.
/// Relative to what is genuinely undecidable here: the daemon resolves it in
/// its own process, which is not this one, and guessing this process's
/// current directory would complete against a tree the session will never
/// run in.
pub fn split_dir_input(input: &str, home: &str) -> (String, String) {
    let text = expand_home(input.trim(), home);
    if text.is_empty() {
        return (String::new(), String::new());
    }
    if text.ends_with(['/', '\\']) {
        return (text, String::new());
    }
    let path = Path::new(&text);
    let Some(name) = path.file_name() else {
        return (text, String::new());
    };
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_string_lossy().into_owned(),
        _ => String::new(),
    };
    (dir, name.to_string_lossy().into_owned())
}

/// Every subdirectory of `dir`, as full paths.
///
/// Empty for a directory that does not exist, cannot be read, or is not one.
/// An unreadable directory is a normal thing to type past on the way to a
/// readable one, so it reports nothing rather than an error.
///
/// Symlinks are followed at the cost of one `stat` each, and only for the
/// entries that are symlinks: a checkout reached through a symlinked `~/src`
/// is exactly the case this exists to complete.
pub fn list_dirs(dir: &str) -> Vec<String> {
    if dir.is_empty() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let started = Instant::now();
    let mut out = Vec::new();
    for (n, entry) in entries.enumerate() {
        if n >= COMPLETE_SCAN_MAX || started.elapsed() > COMPLETE_BUDGET {
            break;
        }
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if !(kind.is_dir() || (kind.is_symlink() && path.is_dir())) {
            continue;
        }
        out.push(path.to_string_lossy().into_owned());
    }
    out
}

/// Directories from `entries` whose last component matches `fragment`.
///
/// Case-insensitive prefix match, because that is what every shell does and a
/// path field that behaved differently would be a small daily surprise.
/// Hidden directories are offered only when the fragment itself starts with a
/// dot: a home directory has dozens of them and they would otherwise bury the
/// two the operator keeps work in. Sorted case-insensitively so the order does
/// not depend on the order the filesystem happened to hand entries back in.
pub fn filter_dirs(entries: &[String], fragment: &str, limit: usize) -> Vec<String> {
    let want = fragment.to_lowercase();
    let hidden_ok = want.starts_with('.');
    let mut out: Vec<&String> = entries
        .iter()
        .filter(|full| {
            let name = leaf(full);
            (hidden_ok || !name.starts_with('.')) && name.to_lowercase().starts_with(&want)
        })
        .collect();
    out.sort_by(|a, b| {
        leaf(a)
            .to_lowercase()
            .cmp(&leaf(b).to_lowercase())
            .then_with(|| a.cmp(b))
    });
    out.into_iter().take(limit).cloned().collect()
}

/// The last path component of `path`, without allocating.
///
/// The completion list draws leaves, not full paths: every row under one
/// directory shares the same prefix, so drawing it repeats the field's own
/// contents on every line and pushes the part that differs off the edge.
pub fn leaf(path: &str) -> &str {
    match path.rfind(['/', '\\']) {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

// ---------------------------------------------------------------------------
// Presets: faults, chords, duplication
// ---------------------------------------------------------------------------

/// Why a saved preset would not launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetFault {
    /// Nothing to run.
    EmptyCommand,
    /// The program is not resolvable on this machine.
    NotOnPath(String),
    /// The preset pins a directory that is not there.
    MissingCwd(String),
}

impl PresetFault {
    /// The sentence to show. Names the thing and the machine, because
    /// "invalid preset" tells an operator nothing they can act on.
    pub fn sentence(&self) -> String {
        match self {
            PresetFault::EmptyCommand => "This preset has no command to run.".to_string(),
            PresetFault::NotOnPath(c) => format!("{c} is not on this machine's PATH."),
            PresetFault::MissingCwd(d) => format!("{d} is not a directory on this machine."),
        }
    }
}

/// The first thing wrong with `preset`, or `None`.
///
/// `None` means nothing was found, not that the launch will succeed. `PATH`
/// can change between this call and the spawn and a pinned directory can be
/// unmounted in between, which is why the dialog asks again on the way out
/// rather than trusting an answer from when the panel was drawn.
pub fn preset_fault(preset: &SavedPreset) -> Option<PresetFault> {
    let command = preset.command.trim();
    if command.is_empty() {
        return Some(PresetFault::EmptyCommand);
    }
    if let Some(cwd) = preset.cwd.as_deref() {
        let cwd = cwd.trim();
        if !cwd.is_empty() && !Path::new(cwd).is_dir() {
            return Some(PresetFault::MissingCwd(cwd.to_string()));
        }
    }
    if !on_path(command) {
        return Some(PresetFault::NotOnPath(command.to_string()));
    }
    None
}

/// A key combination an operator typed into the preset editor.
///
/// Not [`crate::keymap::Chord`], which is a compile-time row in the shell's
/// fixed table carrying a scope and a help entry. This one is data in a
/// profile, so it has to survive somebody typing nonsense into it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chord {
    /// DOM key name, lowercased.
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// Parse `"Ctrl+Shift+K"`.
///
/// Requires exactly one non-modifier part and either Ctrl or Alt. Shift alone
/// is not enough and a bare key is rejected outright, because these are
/// matched against keydown while a text field has focus: a preset bound to
/// `k` or to `Shift+K` would eat the letter every time the operator typed a
/// path containing one. A repeated modifier is rejected too, because
/// `Ctrl+Ctrl+K` is a typo and storing it would bind something the editor
/// never showed back.
pub fn parse_chord(text: &str) -> Option<Chord> {
    let mut chord = Chord::default();
    for part in text.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        let slot = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => &mut chord.ctrl,
            "alt" | "option" => &mut chord.alt,
            "shift" => &mut chord.shift,
            key => {
                if !chord.key.is_empty() {
                    return None;
                }
                chord.key = key.to_string();
                continue;
            }
        };
        if *slot {
            return None;
        }
        *slot = true;
    }
    (!chord.key.is_empty() && (chord.ctrl || chord.alt)).then_some(chord)
}

/// Canonical text for a chord: modifiers in a fixed order, then the key.
pub fn format_chord(chord: &Chord) -> String {
    let mut s = String::new();
    if chord.ctrl {
        s.push_str("Ctrl+");
    }
    if chord.alt {
        s.push_str("Alt+");
    }
    if chord.shift {
        s.push_str("Shift+");
    }
    let mut chars = chord.key.chars();
    if let Some(first) = chars.next() {
        s.extend(first.to_uppercase());
        s.push_str(chars.as_str());
    }
    s
}

/// The first preset `chord` fires, if any.
///
/// First rather than only, because two presets can carry the same shortcut:
/// the editor rejects a duplicate as it is typed, but a hand-edited file has
/// no such gate, and firing the earlier one beats firing neither.
pub fn preset_for_chord<'a>(presets: &'a [SavedPreset], chord: &Chord) -> Option<&'a SavedPreset> {
    presets
        .iter()
        .find(|p| p.shortcut.as_deref().and_then(parse_chord).as_ref() == Some(chord))
}

/// The shell action that would swallow `chord` before a preset ever saw it.
///
/// `bootstrap.js` matches the shell's own table on `window` in the capture
/// phase and calls `stopPropagation`, so a chord in [`crate::keymap::CHORDS`]
/// never reaches a Dioxus keydown handler at all. A preset bound to one would
/// be a shortcut the editor displays and the product never fires, which is
/// worse than refusing the binding, so the editor refuses it and says which
/// action already owns the keys.
///
/// Scopes that cannot allow a dialog text field are not conflicts:
/// `NotTextInput` and `SessionList` both fail the moment focus is in an input,
/// which is the only place a preset chord is listened for.
pub fn chord_conflict(chord: &Chord) -> Option<String> {
    crate::keymap::claims(&chord.key, chord.ctrl, chord.alt, chord.shift)
        .map(|c| format!("{} is already {}.", c.rendered(), c.describes()))
}

/// The launch a preset describes, in `fallback_cwd` when it pins none.
pub fn preset_launch(preset: &SavedPreset, fallback_cwd: &str) -> Result<Launch, String> {
    let cwd = match preset.cwd.as_deref().map(str::trim) {
        Some(c) if !c.is_empty() => c,
        _ => fallback_cwd,
    };
    validate(
        cwd,
        &join_command(&preset.command, &preset.args),
        &preset.label,
    )
}

/// The launch that reproduces `session`: same command, same directory, same
/// title, a new PTY.
///
/// Goes through [`validate`], so duplicating a session whose checkout was
/// deleted after it started reports the same sentence the dialog would rather
/// than sending the daemon a spawn that fails three seconds later.
pub fn duplicate_of(session: &vitrum_proto::SessionInfo) -> Result<Launch, String> {
    validate(
        &session.cwd,
        &join_command(&session.command, &session.args),
        &session.title,
    )
}

/// The directory the new-session dialog should open on. Never blank.
///
/// `seed` is whatever the caller knew: the focused session's directory, or
/// the root of the project the operator right-clicked. Failing that, the last
/// directory a session was started in, which survives a restart. Failing that,
/// home. A directory that no longer exists loses to one that does, but still
/// beats nothing, because a blank field asks the operator to type a path they
/// can already see on screen.
///
/// A RELATIVE candidate is never used, and that is not tidiness. The daemon
/// resolves a relative cwd in its own process, so `.` means the directory the
/// daemon was started in, which is not this window's and not anywhere the
/// operator chose. Observed on the real daemon: another client created a
/// session with cwd `.`, the daemon minted a project whose root was `.`, and
/// this dialog then opened on `.` and wrote `.` into `last_cwd`, propagating
/// one client's working directory into every later launch. A relative path
/// the operator types themselves is still honoured; one is just never
/// guessed on their behalf.
pub fn seed_cwd(seed: &str, store: &LaunchStore, home: &str) -> String {
    let candidates = [seed.trim(), store.last_cwd.trim(), home.trim()];
    let usable = |c: &str| !c.is_empty() && Path::new(c).is_absolute();
    for c in candidates {
        if usable(c) && Path::new(c).is_dir() {
            return c.to_string();
        }
    }
    for c in candidates {
        if usable(c) {
            return c.to_string();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// The session daemon
// ---------------------------------------------------------------------------

/// Executable name of the session daemon.
pub const DAEMON_BIN: &str = "vitrum-server";

/// How long to wait for a freshly spawned daemon to accept a connection.
///
/// It binds a loopback port and nothing else before listening, so this is
/// generous. It exists to bound the wait, not to accommodate a slow start: a
/// daemon that has not bound in three seconds is not coming up, and saying so
/// beats a UI that sits on "connecting" forever.
const DAEMON_START_TIMEOUT: Duration = Duration::from_millis(3000);

/// Gap between connection attempts while waiting for a daemon to bind.
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a probe waits for a TCP connection before calling the port closed.
///
/// Loopback either connects immediately or refuses immediately, so the timeout
/// only matters when a firewall drops packets on the local interface, which is
/// rare and is exactly the case where waiting forever is worst.
const DAEMON_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// Bytes of the daemon's log read back when it dies during startup.
const DAEMON_LOG_TAIL: u64 = 4096;

/// Serialises the decision to spawn.
///
/// Two windows opening at once, or a window and a reconnect, must not both
/// decide the daemon is missing. The lock is held across the port re-test and
/// the spawn, so the second caller through sees the port the first one just
/// filled.
static SPAWN: Mutex<()> = Mutex::new(());

/// Where the daemon binary was found, or where it was looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonBinary {
    Found(PathBuf),
    /// Every place that was tried, in order, so the message can name them.
    Missing {
        looked: Vec<PathBuf>,
    },
}

/// Locate the daemon: beside this executable first, then `PATH`.
///
/// Beside first, and that order is load-bearing. A development tree has
/// `target/debug/vitrum` next to `target/debug/vitrum-server`, and a
/// packaged install puts both in the same directory; in both cases the sibling
/// is the build that matches this client. A `PATH` hit could be a different
/// version entirely, so it is the fallback rather than the first answer.
pub fn find_daemon() -> DaemonBinary {
    let mut looked = Vec::new();

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for name in daemon_file_names() {
            let candidate = dir.join(&name);
            if is_executable(&candidate) {
                return DaemonBinary::Found(candidate);
            }
            looked.push(candidate);
        }
    }

    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in daemon_file_names() {
            let candidate = dir.join(&name);
            if is_executable(&candidate) {
                return DaemonBinary::Found(candidate);
            }
        }
    }
    // The `PATH` entries are deliberately not listed: on a normal machine that
    // is thirty directories and the message becomes unreadable. The sibling
    // paths are the actionable ones.
    DaemonBinary::Missing { looked }
}

/// Filenames the daemon could have on this platform.
fn daemon_file_names() -> Vec<String> {
    let mut names = vec![DAEMON_BIN.to_string()];
    if cfg!(windows) {
        names.insert(0, format!("{DAEMON_BIN}.exe"));
    }
    names
}

/// What an autostart attempt did, or why it could not.
///
/// Seven variants rather than a `Result<(), String>` because the UI treats
/// them differently: three are success, one is a configuration choice, and the
/// three failures each name a different thing for the operator to go and fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Autostart {
    /// Something was already listening. Reused, never duplicated.
    AlreadyRunning,
    /// This process started it, and it is accepting connections.
    Started { pid: u32, path: PathBuf },
    /// `--no-autostart`. Not a failure; the operator runs it themselves.
    Disabled,
    /// The binary is not on this machine.
    NotFound {
        looked: Vec<PathBuf>,
        /// What to type to start it by hand.
        command: String,
    },
    /// It was spawned and did not survive. Carries what it said on the way out.
    Died {
        detail: String,
        log: Option<PathBuf>,
    },
    /// The URL does not name somewhere we can connect to.
    BadAddress { url: String, detail: String },
    /// It was spawned and never bound the port.
    Unresponsive { address: String, waited_ms: u64 },
}

impl Autostart {
    /// Whether a connection attempt is now worth making.
    pub fn connectable(&self) -> bool {
        matches!(
            self,
            Autostart::AlreadyRunning | Autostart::Started { .. } | Autostart::Disabled
        )
    }

    /// The sentence the UI shows when this outcome means no daemon.
    ///
    /// `None` for the three outcomes that leave something to connect to,
    /// because a banner over a working connection is worse than no banner.
    pub fn failure(&self) -> Option<String> {
        match self {
            Autostart::AlreadyRunning | Autostart::Started { .. } | Autostart::Disabled => None,
            Autostart::NotFound { looked, command } => {
                let mut msg = format!("The session daemon {DAEMON_BIN} is not installed.");
                if !looked.is_empty() {
                    msg.push_str(" Looked beside this program at ");
                    msg.push_str(
                        &looked
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    msg.push_str(", then on PATH.");
                }
                msg.push_str(&format!(" Start it yourself with: {command}"));
                Some(msg)
            }
            Autostart::Died { detail, log } => {
                let mut msg = format!("The session daemon started and exited: {detail}");
                if let Some(log) = log {
                    msg.push_str(&format!(" Full output: {}", log.display()));
                }
                Some(msg)
            }
            Autostart::BadAddress { url, detail } => Some(format!("Cannot reach {url}: {detail}")),
            Autostart::Unresponsive { address, waited_ms } => Some(format!(
                "The session daemon started but did not accept a connection on {address} within {waited_ms} ms."
            )),
        }
    }
}

/// The `host:port` a `ws://` or `wss://` URL names.
///
/// Hand-parsed rather than pulled from a URL crate: the only shapes this
/// program ever produces are `ws://host:port` and `wss://host:port`, with an
/// optional path, and a dependency for that is not a trade worth making.
pub fn ws_authority(url: &str) -> Result<String, String> {
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))
        .ok_or_else(|| format!("{url} is not a ws:// or wss:// URL"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(format!("{url} has no host"));
    }
    // Credentials are not something this program emits, and silently dropping
    // them would connect somewhere the URL did not name.
    if authority.contains('@') {
        return Err(format!(
            "{url} carries credentials, which are not supported"
        ));
    }
    Ok(if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:{}", default_ws_port(url))
    })
}

fn default_ws_port(url: &str) -> u16 {
    if url.starts_with("wss://") { 443 } else { 80 }
}

/// Is anything accepting connections there right now?
fn port_is_open(address: &str) -> bool {
    let Ok(addrs) = address.to_socket_addrs() else {
        return false;
    };
    let addrs: Vec<SocketAddr> = addrs.collect();
    addrs
        .iter()
        .any(|addr| TcpStream::connect_timeout(addr, DAEMON_PROBE_TIMEOUT).is_ok())
}

/// Make sure something is listening at `url`, starting the daemon if not.
///
/// Connect-first, spawn-second, and never the other way round. A daemon
/// started by hand, by a previous run of the GUI, or by another window is the
/// same daemon, and every session inside it belongs to the user. Spawning
/// first would either fail on the bound port or, worse, succeed on a different
/// one and hide twenty running agents behind an empty sidebar.
pub fn ensure_daemon(url: &str, allow_spawn: bool) -> Autostart {
    let address = match ws_authority(url) {
        Ok(a) => a,
        Err(detail) => {
            return Autostart::BadAddress {
                url: url.to_string(),
                detail,
            };
        }
    };

    if port_is_open(&address) {
        return Autostart::AlreadyRunning;
    }
    if !allow_spawn {
        return Autostart::Disabled;
    }

    // Everything past here is one at a time. Without the lock, two windows
    // mounting together both see a closed port and both spawn; the loser then
    // fails to bind and reports an error for a daemon that is running fine.
    let _serialised = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
    if port_is_open(&address) {
        return Autostart::AlreadyRunning;
    }

    let path = match find_daemon() {
        DaemonBinary::Found(p) => p,
        DaemonBinary::Missing { looked } => {
            return Autostart::NotFound {
                looked,
                command: manual_command(&address),
            };
        }
    };

    let log = daemon_log_path();
    let mut child = match spawn_daemon(&path, &address, log.as_deref()) {
        Ok(child) => child,
        Err(e) => {
            return Autostart::Died {
                detail: format!("could not run {}: {e}", path.display()),
                log: None,
            };
        }
    };
    let pid = child.id();

    let deadline = Instant::now() + DAEMON_START_TIMEOUT;
    while Instant::now() < deadline {
        if port_is_open(&address) {
            return Autostart::Started { pid, path };
        }
        // A child that has already gone is not going to bind. Checked before
        // sleeping again so a daemon that dies in 5 ms reports in 5 ms rather
        // than after the full timeout.
        match child.try_wait() {
            Ok(Some(status)) => {
                // Losing a bind race is a success wearing a failure's clothes:
                // somebody else's daemon is up, which is all the user wanted.
                if port_is_open(&address) {
                    return Autostart::AlreadyRunning;
                }
                return Autostart::Died {
                    detail: describe_exit(status, log.as_deref()),
                    log,
                };
            }
            Ok(None) => {}
            Err(e) => {
                return Autostart::Died {
                    detail: format!("lost track of the daemon process: {e}"),
                    log,
                };
            }
        }
        std::thread::sleep(DAEMON_POLL_INTERVAL);
    }

    Autostart::Unresponsive {
        address,
        waited_ms: DAEMON_START_TIMEOUT.as_millis() as u64,
    }
}

/// The command line a user would type to start the daemon themselves.
fn manual_command(address: &str) -> String {
    match address.rsplit_once(':') {
        Some((_, port)) if port != "7737" => format!("{DAEMON_BIN} --port {port}"),
        _ => DAEMON_BIN.to_string(),
    }
}

/// Where the daemon's output goes.
fn daemon_log_path() -> Option<PathBuf> {
    vitrum_os::AppPaths::for_current_platform()
        .ok()
        .map(|paths| paths.state_dir.join("daemon.log"))
}

/// Start the daemon so that it outlives this process.
///
/// This is the part that must not be got wrong. The entire reason the daemon
/// is a separate process is that agents survive the GUI: closing a window must
/// not kill twenty running children. So there is no `Child` kept to wait on,
/// no kill on drop, and on Unix a `setsid` that puts the daemon in its own
/// session, out of this process's group, so a Ctrl-C in the terminal that
/// launched the GUI does not take the daemon with it.
///
/// Output goes to a file rather than a pipe, and that is also deliberate: a
/// pipe whose read end closes when the GUI exits leaves the daemon writing
/// into a broken pipe for the rest of its life.
fn spawn_daemon(
    path: &Path,
    address: &str,
    log: Option<&Path>,
) -> std::io::Result<std::process::Child> {
    let mut cmd = Command::new(path);
    if let Some((_, port)) = address.rsplit_once(':') {
        cmd.arg("--port").arg(port);
    }
    cmd.stdin(Stdio::null());

    match log.and_then(|p| {
        p.parent().map(std::fs::create_dir_all);
        std::fs::File::create(p).ok()
    }) {
        Some(file) => {
            let dup = file.try_clone()?;
            cmd.stdout(Stdio::from(file)).stderr(Stdio::from(dup));
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Safety: `setsid` is async-signal-safe and touches nothing but the
        // calling process's session, which is the freshly forked child.
        unsafe {
            cmd.pre_exec(|| {
                if libc_setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    cmd.spawn()
}

// `setsid(2)`.
//
// Declared here rather than pulled in with a `libc` dependency: this is the
// only libc call in the whole client, and one `extern` line is a smaller
// commitment than a crate.
#[cfg(unix)]
unsafe extern "C" {
    /// Put the calling process in a new session, detaching it from this
    /// process's group and controlling terminal.
    #[link_name = "setsid"]
    fn libc_setsid() -> i32;
}

/// Turn an exit status into something worth showing a person.
fn describe_exit(status: std::process::ExitStatus, log: Option<&Path>) -> String {
    let code = match status.code() {
        Some(c) => format!("exit status {c}"),
        None => "killed by a signal".to_string(),
    };
    match log.and_then(log_tail) {
        Some(tail) if !tail.is_empty() => format!("{code}: {tail}"),
        _ => code,
    }
}

/// The last few lines the daemon wrote.
fn log_tail(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let skip = len.saturating_sub(DAEMON_LOG_TAIL);
    let mut text = String::new();
    file.take(skip + DAEMON_LOG_TAIL)
        .read_to_string(&mut text)
        .ok()?;
    let tail: Vec<&str> = text
        .lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .take(3)
        .collect();
    Some(
        tail.into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("; ")
            .trim()
            .to_string(),
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod project_tests;

#[cfg(test)]
mod daemon_tests;

#[cfg(test)]
mod store_tests;

/// The recents list, and the icon slug that rides along with a command.
#[cfg(test)]
mod recents_and_icons;

/// The preset a typed line and directory describe, given what is already
/// saved.
///
/// PURE, and that is deliberate: the writing half touches the operator's real
/// profile, so a test that exercised it would edit the profile of whoever ran
/// the suite. That is not hypothetical -- writing this function's test the
/// other way around put a junk preset called `"unterminated` into a real
/// `launch.json`. Everything worth proving is here, where there is no file.
pub fn preset_from_typed(
    line: &str,
    cwd: &str,
    existing: &[SavedPreset],
) -> Result<SavedPreset, String> {
    let line = line.trim();
    if line.is_empty() {
        return Err("Type a command first, then Ctrl+S keeps it.".to_string());
    }
    let Some((command, args)) = split_command(line) else {
        return Err(format!("{line} is not a command this can save."));
    };
    // The launcher already lists every saved preset, so a second copy of one
    // would put two identical rows in the list the operator is looking at.
    if existing
        .iter()
        .any(|p| p.command == command && p.args == args && p.cwd.as_deref() == Some(cwd))
    {
        return Err(format!("{line} is already saved here."));
    }
    let label = line.to_string();
    Ok(SavedPreset {
        id: mint_preset_id(&label, line),
        label,
        command,
        args,
        cwd: (!cwd.is_empty()).then(|| cwd.to_string()),
        shortcut: None,
        icon: None,
    })
}

/// Saving a command from the launcher, where the operator just typed it.
///
/// Presets used to be creatable only in `Settings > Presets`, so the moment a
/// command proved worth keeping was the moment you had to leave the surface
/// you were on and retype it, directory included. Almost nobody does that,
/// which is why the launcher's preset band was empty on every machine.
///
/// Every test here targets [`preset_from_typed`], which takes the existing
/// list as an argument and touches no file. The writing half is one `load`,
/// this call, and one `save`.
#[cfg(test)]
mod saving_from_the_launcher;

/// Command lines that are not command lines.
///
/// Everything here was found by feeding hostile strings to `split_command` and
/// looking at what came back, not by imagining what might. Each one produced a
/// value the rest of the program would have accepted and could never have run.
#[cfg(test)]
mod an_unrunnable_line_is_refused;
