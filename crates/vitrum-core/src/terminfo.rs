//! Whether the host can describe the terminal sessions are told they have.
//!
//! # What is being checked
//!
//! Every child is started with `TERM` set to one constant value. The value is
//! not adjusted to suit the host: a session that rendered differently depending
//! on which machine the daemon runs on would be worse than one constant claim.
//! What the daemon can do is notice that the host has no description of that
//! terminal and say so once, with the fix for THAT host.
//!
//! # Why the host matters
//!
//! The search order, the variable names and the corrective action are all
//! per-platform, and getting them wrong is worse than saying nothing:
//!
//! - **Linux** reads `$TERMINFO`, `$HOME/.terminfo`, `$TERMINFO_DIRS` and then
//!   the system trees under `/etc`, `/lib` and `/usr/share`. The entry ships in
//!   a distribution package.
//! - **macOS** has the same ncurses search order, but there is no `ncurses-term`
//!   package: the system database is whatever Apple shipped, and a missing entry
//!   is added through Homebrew's ncurses and `TERMINFO_DIRS`.
//! - **Windows** has no system terminfo tree at all, and no `$HOME`. Only a
//!   per-user database under `%USERPROFILE%\.terminfo` can exist, and it matters
//!   only to MSYS, Cygwin and WSL programs run inside a session; a native
//!   console program ignores `TERM` entirely.
//!
//! The check used to be the Linux one, compiled and executed on all three. On
//! Windows it read a variable that is never set, searched three directories that
//! never exist, and then told the operator to install a Debian package.
//!
//! # Testability
//!
//! Everything here is a pure function of a host token, a captured environment
//! and an existence predicate, so all three hosts are exercised from whichever
//! one runs the suite. [`advice_for`] takes the token rather than reading
//! `cfg!`, which is what makes the macOS and Windows sentences reviewable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::hostpath;

/// The environment terminfo resolution reads, captured rather than read per
/// lookup so the rule is deterministic and testable.
#[derive(Debug, Clone, Default)]
pub(crate) struct TermEnv {
    vars: BTreeMap<String, String>,
}

impl TermEnv {
    /// Capture the variables that matter from the real process environment.
    pub(crate) fn from_process() -> Self {
        let keys = ["TERMINFO", "TERMINFO_DIRS", "HOME", "USERPROFILE"];
        Self {
            vars: keys
                .iter()
                .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v)))
                .collect(),
        }
    }

    /// Build one explicitly.
    #[cfg(test)]
    pub(crate) fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            vars: pairs.into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str).filter(|v| !v.is_empty())
    }
}

/// The terminfo trees `host` searches, in order.
///
/// Empty is a meaningful answer: it means this host has no terminfo database
/// for the daemon to consult, which is not the same as one that is empty.
pub(crate) fn roots(host: &str, env: &TermEnv) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(dir) = env.get("TERMINFO") {
        roots.push(PathBuf::from(dir));
    }
    // The per-user tree hangs off the home directory, which Windows spells
    // differently and does not put in `HOME`.
    let home = match host {
        "windows" => env.get("USERPROFILE"),
        _ => env.get("HOME"),
    };
    if let Some(home) = home {
        roots.push(hostpath::join(separator(host), Path::new(home), ".terminfo"));
    }
    if let Some(dirs) = env.get("TERMINFO_DIRS") {
        roots.extend(
            dirs.split(if host == "windows" { ';' } else { ':' })
                .filter(|p| !p.is_empty())
                .map(PathBuf::from),
        );
    }
    match host {
        // The ncurses system trees, in ncurses' own order.
        "linux" | "macos" => {
            roots.push(PathBuf::from("/etc/terminfo"));
            roots.push(PathBuf::from("/lib/terminfo"));
            roots.push(PathBuf::from("/usr/share/terminfo"));
        }
        // No system database exists. Whatever the user installed is all there
        // is, and adding Unix paths would be three stat calls that can only
        // fail.
        "windows" => {}
        _ => {}
    }
    roots
}

/// The separator `host` spells a path with.
pub(crate) const fn separator(host: &str) -> char {
    // `str` has no const equality, so this compares the one byte that differs.
    // The token set is [`GUIDED_HOSTS`] plus whatever `std::env::consts::OS`
    // reports, and only Windows is not POSIX-spelled.
    match host.as_bytes() {
        b"windows" => hostpath::WINDOWS,
        _ => hostpath::POSIX,
    }
}

/// Whether an entry named `name` is present in any of `roots`.
///
/// Both database layouts are checked: a directory per initial letter, and the
/// hashed form built with `--enable-term-driver`, where the directory is the
/// initial's hex code point.
pub(crate) fn entry_present(
    roots: &[PathBuf],
    name: &str,
    separator: char,
    exists: &dyn Fn(&Path) -> bool,
) -> bool {
    let Some(initial) = name.chars().next() else {
        return false;
    };
    let hashed = format!("{:x}", initial as u32);
    roots.iter().any(|root| {
        let letter = hostpath::join(separator, root, &initial.to_string());
        let hex = hostpath::join(separator, root, &hashed);
        exists(&hostpath::join(separator, &letter, name))
            || exists(&hostpath::join(separator, &hex, name))
    })
}

/// Host token, and what to tell the operator on it when the entry is absent.
///
/// One row per platform the product claims to run on, and the only place a host
/// is named. [`guided_hosts`] reads the tokens back out, so a test walking the
/// variant space cannot go stale against a table someone extended.
///
/// Each sentence is imperative, names the thing to install, and says what
/// happens without it. There is deliberately no default row: handing a FreeBSD
/// operator a `dpkg` package name is worse than an honest blank.
const GUIDANCE: &[(&str, &str)] = &[
    (
        "linux",
        "install the ncurses-term package, or full-screen programs fall back to their built-in \
         handling of an unknown TERM",
    ),
    (
        "macos",
        "install a current ncurses (brew install ncurses) and point TERMINFO_DIRS at its \
         share/terminfo, or full-screen programs fall back to their built-in handling of an \
         unknown TERM",
    ),
    (
        "windows",
        "compile the entry with tic into %USERPROFILE%\\.terminfo if this machine runs MSYS, \
         Cygwin or WSL programs in a session; native console programs ignore TERM and need \
         nothing",
    ),
];

/// What to tell the operator on `host` when the entry is absent.
pub(crate) fn advice_for(host: &str) -> Option<&'static str> {
    GUIDANCE.iter().find(|(h, _)| *h == host).map(|(_, advice)| *advice)
}

/// Every host this crate has terminfo guidance for.
///
/// The values `std::env::consts::OS` takes on the platforms the product claims
/// to run on. Anything outside this set is reported as unguided rather than
/// given Linux's answer.
pub(crate) fn guided_hosts() -> impl Iterator<Item = &'static str> {
    GUIDANCE.iter().map(|(host, _)| *host)
}

/// The whole check, as a value: what was looked for, whether it was found, and
/// what to say about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminfoCheck {
    /// The entry is in the host's database.
    Present,
    /// The entry is absent and this host has an answer for that.
    Absent {
        /// Imperative sentence naming the fix on this host.
        advice: &'static str,
    },
    /// The entry is absent and this crate has no guidance for this host.
    ///
    /// Reported rather than swallowed: an operator on a platform nobody wrote a
    /// line for should learn that from the daemon, not from a terminal that
    /// renders wrong.
    Unguided {
        /// The `std::env::consts::OS` token that has no entry here.
        host: String,
    },
}

/// Run the check for `name` on `host`.
pub(crate) fn check(
    host: &str,
    name: &str,
    env: &TermEnv,
    exists: &dyn Fn(&Path) -> bool,
) -> TerminfoCheck {
    if entry_present(&roots(host, env), name, separator(host), exists) {
        return TerminfoCheck::Present;
    }
    match advice_for(host) {
        Some(advice) => TerminfoCheck::Absent { advice },
        None => TerminfoCheck::Unguided { host: host.to_string() },
    }
}
