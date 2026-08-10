//! The shared secret that proves a client is allowed to talk to the daemon.
//!
//! The daemon listens on loopback and spawns arbitrary commands on request, so
//! reaching it is equivalent to running code as the user who started it. A
//! loopback listener is not a boundary: every other account on the machine can
//! connect to it, and so can any web page the user visits, because a browser
//! may open a WebSocket to `ws://127.0.0.1` cross-origin with no preflight and
//! no same-origin check. Two layers answer that, and this module is the second
//! one. The first is the `Origin` refusal at the HTTP upgrade, which stops the
//! browser case; this stops the other-local-user case, which no header check
//! can.
//!
//! The secret is 32 bytes of operating-system entropy, hex encoded, written
//! once per daemon start to a file only its owner can read. Anything that can
//! read that file could already read the process's memory or its PTYs, so the
//! file is the whole boundary and its mode is the whole enforcement.
//!
//! # Why this crate
//!
//! `vitrum-os` owns application directories, and this path is deliberately the
//! same one it resolves. It cannot own this: the daemon would then have to
//! depend on the desktop-integration crate and link zbus, ksni and the tray
//! into a headless process. `vitrum-os` depends on this crate, not the other
//! way round, so the resolution lives here and `vitrum-os` pins it with a test
//! that fails if the two ever disagree about where the file is.

use std::path::{Path, PathBuf};

/// Characters in a token: 32 bytes, hex encoded.
pub const TOKEN_HEX_LEN: usize = 64;

/// Bytes of entropy behind a token.
const TOKEN_BYTES: usize = TOKEN_HEX_LEN / 2;

// Each of the three is read on exactly one platform, so two of them are dead
// on any given build. They stay together because they are one fact — the
// directory layout `vitrum_os::AppPaths` resolves — and splitting them behind
// `cfg` would hide a drift that a reader can currently see in four lines.
/// Directory name under a runtime or data root. Matches `vitrum_os::AppPaths`.
#[allow(dead_code)]
const APP_NAME: &str = "vitrum";
/// macOS bundle identifier. Matches `vitrum_os::AppPaths`.
#[allow(dead_code)]
const BUNDLE_ID: &str = "dev.santhreal.vitrum";
/// Windows vendor segment. Matches `vitrum_os::AppPaths`.
#[allow(dead_code)]
const ORG_NAME: &str = "santhreal";

/// File name inside the runtime directory.
const FILE_NAME: &str = "token";

/// Why the token could not be located, read, or written.
#[derive(Debug)]
pub enum TokenError {
    /// No directory could be resolved, because the named variable is unset or
    /// holds a relative path and no fallback applied either.
    NoDirectory {
        /// The variable whose absence ended the search.
        var: &'static str,
    },
    /// The file is not there. No daemon has run as this user, or the one
    /// listening was started by a different user.
    Missing {
        /// Where it was looked for.
        path: PathBuf,
    },
    /// The file is there and could not be read.
    Unreadable {
        /// Where it was looked for.
        path: PathBuf,
        /// What the filesystem said.
        cause: std::io::Error,
    },
    /// The file does not hold one hex-encoded token.
    Malformed {
        /// Where it was read from.
        path: PathBuf,
    },
    /// A token supplied as a value rather than read from a file — an
    /// environment variable — is not the shape a token has.
    MalformedValue {
        /// Where the value came from, named so the operator can go and fix
        /// the right thing: a bad `VITRUM_TOKEN` and a bad token file call
        /// for opposite responses.
        source: &'static str,
    },
    /// The file could not be created or replaced.
    Unwritable {
        /// Where it was written.
        path: PathBuf,
        /// What the filesystem said.
        cause: std::io::Error,
    },
    /// Something is already at the token's path that this daemon did not
    /// write, so replacing it would write a secret through a file somebody
    /// else controls.
    Foreign {
        /// Where it is.
        path: PathBuf,
        /// Which check it failed, in a form that finishes the sentence "the
        /// file cannot be replaced because ...".
        reason: &'static str,
    },
    /// The operating system refused to supply entropy.
    NoEntropy {
        /// What the entropy source said.
        cause: getrandom::Error,
    },
}

impl core::fmt::Display for TokenError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TokenError::NoDirectory { var } => write!(
                f,
                "cannot locate the vitrum token file: ${var} is unset or not an absolute path. \
                 Set it, or run vitrum from a normal desktop session"
            ),
            TokenError::Missing { path } => write!(
                f,
                "no vitrum token at {}. Start vitrum-server as this user; a daemon started by \
                 another user writes a token you cannot read",
                path.display()
            ),
            TokenError::Unreadable { path, cause } => write!(
                f,
                "cannot read the vitrum token at {}: {cause}. Check that you own the file and \
                 that it is mode 0600",
                path.display()
            ),
            TokenError::Malformed { path } => write!(
                f,
                "the vitrum token at {} is not {TOKEN_HEX_LEN} hex characters. Delete it and \
                 restart vitrum-server, which writes a fresh one",
                path.display()
            ),
            TokenError::MalformedValue { source } => write!(
                f,
                "the token in {source} is not {TOKEN_HEX_LEN} hex characters. Copy it from the \
                 token file vitrum-server wrote, or unset {source} to use that file directly"
            ),
            TokenError::Unwritable { path, cause } => write!(
                f,
                "cannot write the vitrum token to {}: {cause}. Check that the directory exists \
                 and is writable by this user",
                path.display()
            ),
            TokenError::Foreign { path, reason } => write!(
                f,
                "refusing to replace {}: {reason}. Delete that path yourself and start \
                 vitrum-server again; it will not write a secret through a file it does \
                 not own",
                path.display()
            ),
            TokenError::NoEntropy { cause } => write!(
                f,
                "the operating system refused to supply random bytes: {cause}. vitrum-server \
                 cannot authenticate clients without them and will not start"
            ),
        }
    }
}

impl core::error::Error for TokenError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            TokenError::Unreadable { cause, .. } | TokenError::Unwritable { cause, .. } => {
                Some(cause)
            }
            TokenError::NoEntropy { cause } => Some(cause),
            _ => None,
        }
    }
}

/// Where this user's token file is.
///
/// Resolved from the process environment. The runtime directory is preferred
/// because a token is per boot and per login session and has no business
/// surviving a reboot in a backup; the data directory is the fallback for a
/// machine with no logind session, where the alternative would be `/tmp`.
pub fn path() -> Result<PathBuf, TokenError> {
    Ok(directory()?.join(FILE_NAME))
}

/// Read and validate this user's token from the default location.
///
/// The client half, and the last of the three ways a token reaches a client:
/// the environment, an explicit file, then this. All three end in
/// [`validate`], so there is exactly one definition of what a token is.
pub fn load() -> Result<String, TokenError> {
    load_from(&path()?)
}

/// Generate a fresh token and write it where [`path`] says, replacing any
/// previous one.
///
/// The daemon half, called once per start. A new token per start is
/// deliberate: it invalidates every client of a daemon that has gone away,
/// which is the same restart the protocol-version skew already forces.
///
/// The file is created mode 0600 inside a directory created mode 0700, and it
/// is replaced by an atomic rename rather than truncated in place, so a client
/// reading while the daemon restarts sees a whole token and never half of one.
/// Replacing the PREVIOUS daemon's token is the ordinary case, because the
/// runtime directory survives until logout; what is refused is a path that is
/// a symlink, is not a regular file, belongs to another user, or is not mode
/// 0600, none of which this code can have produced. Windows has no mode bits
/// and relies on the per-user ACL that `%LOCALAPPDATA%` already carries.
pub fn create() -> Result<String, TokenError> {
    let dir = directory()?;
    create_dir_private(&dir)?;
    let path = dir.join(FILE_NAME);
    let token = generate()?;
    write_private(&path, &token)?;
    Ok(token)
}

/// Whether `presented` is `expected`, in time that does not depend on how much
/// of it matched.
///
/// A byte-by-byte comparison that returns early tells a peer, through timing,
/// how long a prefix it guessed, which turns 2^256 guesses into 64 rounds of
/// 16. The loop below reads every byte of both strings whatever they contain.
/// Lengths are compared first and separately, because the length of a token is
/// not a secret: it is a published constant.
#[inline(never)]
pub fn matches(expected: &str, presented: &str) -> bool {
    let expected = expected.as_bytes();
    let presented = presented.as_bytes();
    if expected.len() != presented.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (a, b) in expected.iter().zip(presented.iter()) {
        diff |= a ^ b;
    }
    // Keeps the accumulator from being folded into a branch by an optimiser
    // that can see both operands come from the same comparison.
    std::hint::black_box(diff) == 0
}

/// Read and validate a token from an explicit path.
///
/// Separated from [`load`] so a caller can name a file and so the round trip
/// can be proved against a directory a test owns, rather than against the
/// environment the suite happens to run under.
pub fn load_from(path: &Path) -> Result<String, TokenError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            return Err(TokenError::Missing {
                path: path.to_path_buf(),
            });
        }
        Err(cause) => {
            return Err(TokenError::Unreadable {
                path: path.to_path_buf(),
                cause,
            });
        }
    };
    let token = raw.trim();
    if !is_well_formed(token) {
        return Err(TokenError::Malformed {
            path: path.to_path_buf(),
        });
    }
    Ok(token.to_string())
}

/// Trim and check a token that arrived as a value rather than as a file.
///
/// `source` names where it came from — `"VITRUM_TOKEN"` — because a bad
/// environment variable and a corrupt token file call for opposite responses
/// from the operator, and an error that does not say which is which sends
/// them to the wrong one.
pub fn validate(raw: &str, source: &'static str) -> Result<String, TokenError> {
    let token = raw.trim();
    if !is_well_formed(token) {
        return Err(TokenError::MalformedValue { source });
    }
    Ok(token.to_string())
}

/// Generate and write a token at an explicit path, creating its directory.
///
/// The counterpart to [`load_from`], and what [`create`] is once the
/// directory is resolved.
pub fn create_at(path: &Path) -> Result<String, TokenError> {
    if let Some(dir) = path.parent() {
        create_dir_private(dir)?;
    }
    let token = generate()?;
    write_private(path, &token)?;
    Ok(token)
}

/// Whether `s` is the shape a token has: exactly [`TOKEN_HEX_LEN`] lowercase
/// hex characters.
///
/// Uppercase is refused rather than folded. Two spellings of one secret would
/// make [`matches`] answer differently for two strings that mean the same
/// thing, and there is no reader of this file that is not also its writer.
pub fn is_well_formed(s: &str) -> bool {
    s.len() == TOKEN_HEX_LEN
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn generate() -> Result<String, TokenError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|cause| TokenError::NoEntropy { cause })?;
    let mut out = String::with_capacity(TOKEN_HEX_LEN);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
    }
    Ok(out)
}

#[cfg(unix)]
fn create_dir_private(dir: &Path) -> Result<(), TokenError> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder.create(dir).map_err(|cause| TokenError::Unwritable {
        path: dir.to_path_buf(),
        cause,
    })
}

#[cfg(not(unix))]
fn create_dir_private(dir: &Path) -> Result<(), TokenError> {
    std::fs::create_dir_all(dir).map_err(|cause| TokenError::Unwritable {
        path: dir.to_path_buf(),
        cause,
    })
}

/// Refuse to replace anything at `path` that this process did not write.
///
/// A daemon restart is routine — the product's own protocol-skew message
/// tells the operator to do it — and `$XDG_RUNTIME_DIR` survives until logout,
/// so the previous daemon's token is normally still sitting there. Replacing
/// it is therefore the common path and must work. What must not work is
/// writing a fresh secret through something that is not our file: a symlink
/// pointing at a file the attacker can read, or a regular file another
/// account planted with a mode we did not choose.
///
/// `symlink_metadata`, never `metadata`: following the link is the whole bug.
#[cfg(unix)]
fn refuse_a_foreign_file(path: &Path) -> Result<(), TokenError> {
    use std::os::unix::fs::MetadataExt;

    let existing = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(cause) => {
            return Err(TokenError::Unwritable {
                path: path.to_path_buf(),
                cause,
            });
        }
    };
    let reason = if existing.file_type().is_symlink() {
        "it is a symbolic link"
    } else if !existing.file_type().is_file() {
        "it is not a regular file"
    } else if existing.uid() != rustix::process::geteuid().as_raw() {
        "it belongs to another user"
    } else if existing.mode() & 0o777 != 0o600 {
        // Ours, but not as we left it. Every token this code writes is 0600,
        // so a different mode means something else has been at it, and
        // widening the secret's audience silently is the failure this whole
        // file exists to prevent.
        "it is not mode 0600, so this daemon did not write it"
    } else {
        return Ok(());
    };
    Err(TokenError::Foreign {
        path: path.to_path_buf(),
        reason,
    })
}

#[cfg(not(unix))]
fn refuse_a_foreign_file(path: &Path) -> Result<(), TokenError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(TokenError::Foreign {
            path: path.to_path_buf(),
            reason: "it is a symbolic link",
        }),
        Ok(m) if !m.file_type().is_file() => Err(TokenError::Foreign {
            path: path.to_path_buf(),
            reason: "it is not a regular file",
        }),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(TokenError::Unwritable {
            path: path.to_path_buf(),
            cause,
        }),
    }
}

/// Write `token` at `path`, replacing what is there in one step.
///
/// Through a temporary name in the same directory and a rename, so a client
/// reading concurrently sees either the whole old token or the whole new one.
/// Unlinking and recreating, which is what this did first, leaves two windows
/// a reader can land in: one where the file does not exist, and one where it
/// exists and is empty. Both surface to the operator as an authentication
/// failure against a daemon that is working perfectly.
///
/// The temporary is created with the final mode rather than chmodded
/// afterwards, so the secret is never on disk world-readable even for an
/// instant, and it is created in the same directory because a rename across
/// filesystems is not atomic and would not be a rename at all.
fn write_private(path: &Path, token: &str) -> Result<(), TokenError> {
    use std::io::Write;

    refuse_a_foreign_file(path)?;

    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(FILE_NAME);
    // The pid makes two daemons racing to start write to different temporary
    // names, so the loser's rename replaces the winner's token rather than
    // corrupting it. One of them wins outright, which is the same outcome as
    // the port bind that is about to refuse the loser anyway.
    let temp = dir.join(format!(".{name}.{}", std::process::id()));

    let unwritable = |cause: std::io::Error| TokenError::Unwritable {
        path: path.to_path_buf(),
        cause,
    };

    // A leftover from a process killed between create and rename. Removing it
    // is safe: the name carries our own pid.
    let _ = std::fs::remove_file(&temp);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(&unwritable)?;
    let written = file
        .write_all(token.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(cause) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(unwritable(cause));
    }
    if let Err(cause) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(unwritable(cause));
    }
    Ok(())
}

/// The directory the token file lives in, for the platform this was built for.
fn directory() -> Result<PathBuf, TokenError> {
    #[cfg(target_os = "macos")]
    {
        if let Some(tmp) = absolute_var("TMPDIR") {
            return Ok(tmp.join(BUNDLE_ID));
        }
        let home = absolute_var("HOME").ok_or(TokenError::NoDirectory { var: "HOME" })?;
        return Ok(home
            .join("Library/Application Support")
            .join(BUNDLE_ID)
            .join("run"));
    }
    #[cfg(windows)]
    {
        let local =
            absolute_var("LOCALAPPDATA").ok_or(TokenError::NoDirectory { var: "LOCALAPPDATA" })?;
        return Ok(local.join(ORG_NAME).join(APP_NAME).join("run"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(run) = absolute_var("XDG_RUNTIME_DIR") {
            return Ok(run.join(APP_NAME));
        }
        // No logind session. `vitrum-os` falls back to the cache directory
        // rather than `/tmp`, which is world-writable, and this follows it
        // exactly so the two resolve the same file.
        let cache = match absolute_var("XDG_CACHE_HOME") {
            Some(cache) => cache.join(APP_NAME),
            None => absolute_var("HOME")
                .ok_or(TokenError::NoDirectory {
                    var: "XDG_RUNTIME_DIR",
                })?
                .join(".cache")
                .join(APP_NAME),
        };
        return Ok(cache.join("run"));
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(TokenError::NoDirectory { var: "HOME" })
    }
}

/// An environment variable, if it holds an absolute path.
///
/// A relative value is refused rather than resolved against the working
/// directory, which is the XDG specification's own rule and keeps a token from
/// landing wherever a process happened to be launched.
#[allow(dead_code)]
fn absolute_var(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    let path = PathBuf::from(value);
    if path.is_absolute() { Some(path) } else { None }
}

#[cfg(test)]
mod a_token_is_the_whole_boundary;
