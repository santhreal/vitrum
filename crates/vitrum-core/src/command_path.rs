//! Turning a command name into a program on disk, before a pseudoterminal is
//! created for it.
//!
//! # Why the daemon resolves at all
//!
//! The underlying PTY error for a name that is not installed is `No viable
//! candidates found in PATH` followed by every entry of `PATH`: over a kilobyte
//! of text that does not answer the only question the operator has, which is
//! what to type instead. Resolving first turns that into one sentence, and it
//! happens before a pty is opened, so a failed spawn leaves nothing behind.
//!
//! # Two rule sets, not three
//!
//! Linux and macOS resolve identically: a bare name is searched for in `PATH`
//! and executed exactly as it is spelled. Windows does neither of those things.
//! It appends the suffixes in `PATHEXT` to a name that has none, and it searches
//! the executable's own directory and the working directory before `PATH`. A
//! resolver written for one and compiled for the other is a resolver that
//! refuses commands that work, which is worse than not resolving at all.
//!
//! # Testability
//!
//! [`resolve`] is a pure function of a rule set, a command, and a captured
//! [`Search`]. Existence is a closure, so a Windows filesystem can be described
//! from a Linux box and the Windows arm is exercised on every host rather than
//! only in CI. [`Search::for_host`] is the thin wrapper that reads the real
//! process environment.

use std::path::{Path, PathBuf};

use crate::error::SessionError;
use crate::hostpath;

/// How a host turns a command name into a program on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpawnRules {
    /// Linux, macOS and the other Unixes: `PATH` only, name taken literally.
    Posix,
    /// Windows: `PATHEXT` suffixes, and the working directory searched first.
    Windows,
}

impl SpawnRules {
    /// The rules the running host uses.
    pub(crate) const fn host() -> Self {
        if cfg!(windows) { Self::Windows } else { Self::Posix }
    }

    /// Whether `c` separates path components under these rules.
    const fn is_separator(self, c: char) -> bool {
        match self {
            // A backslash is a legal character in a Unix file name, so treating
            // it as a separator here would refuse a command that exists.
            Self::Posix => c == hostpath::POSIX,
            Self::Windows => c == hostpath::POSIX || c == hostpath::WINDOWS,
        }
    }

    /// The separator a path built under these rules is spelled with.
    const fn separator(self) -> char {
        match self {
            Self::Posix => hostpath::POSIX,
            Self::Windows => hostpath::WINDOWS,
        }
    }

    /// Whether `value` names a location on its own, rather than one relative to
    /// the session's directory.
    fn rooted(self, value: &str) -> bool {
        match self {
            Self::Posix => value.starts_with(hostpath::POSIX),
            Self::Windows => hostpath::windows_rooted(value),
        }
    }

    /// Whether `value` carries a Windows drive qualifier such as `C:`.
    ///
    /// Always false under POSIX rules, where a colon is an ordinary character
    /// in a file name.
    fn drive_qualified(self, value: &str) -> bool {
        self == Self::Windows && hostpath::windows_drive_qualified(value)
    }
}

/// Default `PATHEXT`, used when the variable is absent.
///
/// The value Windows itself ships. A machine with no `PATHEXT` still runs
/// `cmd.exe` and `where.exe`, so an empty list here would refuse every bare
/// command name on a host that works.
const DEFAULT_PATHEXT: &[&str] = &[".COM", ".EXE", ".BAT", ".CMD"];

/// Everything the resolver reads about the machine.
///
/// Captured up front so resolution is a pure function, and so a test never has
/// to mutate process-global environment state another test thread can observe.
pub(crate) struct Search<'a> {
    /// Directories searched for a bare command name, in order. On Windows this
    /// already carries the working directory and the executable's directory at
    /// the front, because that is where the loader looks first.
    dirs: Vec<PathBuf>,
    /// Suffixes tried for a Windows command name that has none. Upper case, each
    /// beginning with a dot. Empty under [`SpawnRules::Posix`].
    extensions: Vec<String>,
    /// Directory a relative command name is resolved against: the directory the
    /// session starts in, never the daemon's own.
    cwd: &'a Path,
    /// Whether a candidate exists and can be executed.
    exists: &'a dyn Fn(&Path) -> bool,
}

impl<'a> Search<'a> {
    /// Describe a search explicitly. The order of `dirs` is the search order.
    #[cfg(test)]
    pub(crate) fn new(
        dirs: Vec<PathBuf>,
        extensions: Vec<String>,
        cwd: &'a Path,
        exists: &'a dyn Fn(&Path) -> bool,
    ) -> Self {
        Self { dirs, extensions, cwd, exists }
    }

    /// Capture the running host's search, for a session starting in `cwd`.
    pub(crate) fn for_host(rules: SpawnRules, cwd: &'a Path, exists: &'a dyn Fn(&Path) -> bool) -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if rules == SpawnRules::Windows {
            // `CreateProcessW` searches the calling image's directory and then
            // the working directory before `PATH`. Omitting them refuses a
            // command that the spawn would have found.
            if let Ok(exe) = std::env::current_exe()
                && let Some(parent) = exe.parent()
            {
                dirs.push(parent.to_path_buf());
            }
            dirs.push(cwd.to_path_buf());
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path).filter(|d| !d.as_os_str().is_empty()));
        }

        let extensions = match rules {
            SpawnRules::Posix => Vec::new(),
            SpawnRules::Windows => pathext(std::env::var("PATHEXT").ok().as_deref()),
        };

        Self { dirs, extensions, cwd, exists }
    }
}

/// Split a `PATHEXT` value into normalised suffixes.
///
/// Upper-cased and dot-prefixed, because Windows compares them case-insensitively
/// and a hand-edited value is as likely to read `EXE;BAT` as `.EXE;.BAT`.
fn pathext(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.filter(|v| !v.trim().is_empty()) else {
        return DEFAULT_PATHEXT.iter().map(|e| (*e).to_string()).collect();
    };
    let mut out: Vec<String> = raw
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty() && *e != ".")
        .map(|e| {
            let e = e.to_uppercase();
            if e.starts_with('.') { e } else { format!(".{e}") }
        })
        .collect();
    if out.is_empty() {
        out = DEFAULT_PATHEXT.iter().map(|e| (*e).to_string()).collect();
    }
    out
}

/// Whether `command` already ends in one of `extensions`.
fn has_known_extension(command: &str, extensions: &[String]) -> bool {
    let upper = command.to_uppercase();
    extensions.iter().any(|e| upper.ends_with(e.as_str()))
}

/// Refuse a command this host cannot find, naming it.
///
/// `Ok(())` means a program was found, not merely that nothing objected.
pub(crate) fn resolve(
    rules: SpawnRules,
    command: &str,
    search: &Search<'_>,
) -> Result<(), SessionError> {
    let refuse = || SessionError::NotOnPath { command: command.to_string() };

    // A name carrying a separator is a path, not something to look up. It is
    // resolved against the directory the SESSION starts in: the child is spawned
    // there, so `./run.sh` means a file there and not one beside the daemon.
    if command.chars().any(|c| rules.is_separator(c)) || rules.drive_qualified(command) {
        let candidate = if rules.rooted(command) || rules.drive_qualified(command) {
            PathBuf::from(hostpath::spell(rules.separator(), command))
        } else {
            hostpath::join(rules.separator(), search.cwd, command)
        };
        return if any_spelling(&candidate, rules, &search.extensions, search.exists) {
            Ok(())
        } else {
            Err(refuse())
        };
    }

    for dir in &search.dirs {
        let candidate = hostpath::join(rules.separator(), dir, command);
        if any_spelling(&candidate, rules, &search.extensions, search.exists) {
            return Ok(());
        }
    }
    Err(refuse())
}

/// Whether `candidate`, or `candidate` plus a `PATHEXT` suffix, exists.
fn any_spelling(
    candidate: &Path,
    rules: SpawnRules,
    extensions: &[String],
    exists: &dyn Fn(&Path) -> bool,
) -> bool {
    if exists(candidate) {
        return true;
    }
    if rules == SpawnRules::Posix {
        return false;
    }
    let Some(name) = candidate.to_str() else {
        return false;
    };
    if has_known_extension(name, extensions) {
        // Already spelled with a suffix the loader recognises, and it was not
        // there. Appending a second suffix would look for `git.exe.exe`.
        return false;
    }
    // Both spellings, because the file system is case-insensitive there but the
    // predicate this is handed need not be: `PATHEXT` is upper case by
    // convention and installers write `claude.cmd`.
    extensions.iter().any(|ext| {
        exists(Path::new(&format!("{name}{}", ext.to_lowercase())))
            || exists(Path::new(&format!("{name}{ext}")))
    })
}
