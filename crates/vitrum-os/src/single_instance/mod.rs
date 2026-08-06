//! One window per user session.
//!
//! A second launch must not open a second copy; it must hand what it was asked
//! to do to the copy that is already running and exit. That needs two things: a
//! race-free way to decide who is first, and a channel for the loser to speak
//! to the winner.
//!
//! - **Unix**: an advisory `flock` on a file in the runtime directory decides,
//!   and a Unix domain socket beside it carries the handoff. `flock` rather
//!   than a pid file because a pid file left by a crash is indistinguishable
//!   from a live instance, whereas a `flock` is released by the kernel when the
//!   process dies however it dies.
//! - **Windows**: a named mutex in the `Local\` namespace decides, and a named
//!   pipe carries the handoff. Same reasoning: the kernel releases the mutex on
//!   process death.
//!
//! The handoff payload is a one-line text protocol, encoded and decoded by pure
//! functions, so a malformed or hostile message is a parse error with a reason
//! rather than a partially applied command.

use core::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::branding::{APP_NAME, BUNDLE_ID};
use crate::capability::Unavailable;
use crate::deeplink::{self, DeepLink, DeepLinkError};
use crate::paths::Platform;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Protocol banner. Bumped if the line format changes, so a mismatched pair of
/// builds refuses rather than misreads.
pub(crate) const ACTIVATION_PROTOCOL: &str = "vitrum-instance/1";

/// Longest activation message accepted, in bytes.
///
/// A deep link is capped at [`deeplink::MAX_URL_LEN`]; this leaves room for the
/// banner and the verb and nothing else, so a peer cannot make the primary
/// allocate.
pub(crate) const MAX_ACTIVATION_LEN: usize = deeplink::MAX_URL_LEN + 64;

/// What a second launch asks the first to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    /// Nothing specific: raise and focus the existing window.
    Focus,
    /// Go to what this URL names, then raise and focus.
    Open(DeepLink),
}

impl Activation {
    /// Read a launch's intent out of its command line.
    ///
    /// The first argument that parses as a `vitrum://` URL wins. Anything else
    /// is ignored rather than rejected, because desktop environments append
    /// their own arguments (`--gapplication-service`, a startup id) and a
    /// launch with an unrecognised flag should still raise the window.
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Self {
        for arg in args {
            if let Ok(link) = deeplink::parse(arg.as_ref()) {
                return Self::Open(link);
            }
        }
        Self::Focus
    }

    /// The deep link this activation carries, if any.
    pub fn link(&self) -> Option<DeepLink> {
        match self {
            Self::Focus => None,
            Self::Open(link) => Some(*link),
        }
    }
}

/// Serialise for the handoff channel. Always ends in a newline.
pub(crate) fn encode_activation(activation: &Activation) -> Vec<u8> {
    let line = match activation {
        Activation::Focus => format!("{ACTIVATION_PROTOCOL} focus\n"),
        Activation::Open(link) => format!("{ACTIVATION_PROTOCOL} open {}\n", link.to_url()),
    };
    line.into_bytes()
}

/// Why an activation message was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActivationError {
    TooLong { len: usize },
    NotUtf8,
    WrongProtocol { found: String },
    UnknownVerb { verb: String },
    MissingUrl,
    BadUrl(DeepLinkError),
    TrailingData { extra: String },
}

impl fmt::Display for ActivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { len } => {
                write!(f, "activation is {len} bytes, limit is {MAX_ACTIVATION_LEN}")
            }
            Self::NotUtf8 => write!(f, "activation is not valid UTF-8"),
            Self::WrongProtocol { found } => {
                write!(f, "expected banner `{ACTIVATION_PROTOCOL}`, found `{found}`")
            }
            Self::UnknownVerb { verb } => write!(f, "unknown verb `{verb}`"),
            Self::MissingUrl => write!(f, "`open` needs a url"),
            Self::BadUrl(e) => write!(f, "bad url: {e}"),
            Self::TrailingData { extra } => write!(f, "unexpected trailing data `{extra}`"),
        }
    }
}

impl core::error::Error for ActivationError {}

/// Parse a handoff message.
///
/// Strict: an unrecognised verb is an error, not a fallback to focus. The
/// socket is reachable by anything running as this user, so a message that is
/// not exactly one of the two known forms is a bug or a probe, and quietly
/// treating it as "raise the window" would hide both.
pub(crate) fn decode_activation(bytes: &[u8]) -> Result<Activation, ActivationError> {
    if bytes.len() > MAX_ACTIVATION_LEN {
        return Err(ActivationError::TooLong { len: bytes.len() });
    }
    let text = core::str::from_utf8(bytes).map_err(|_| ActivationError::NotUtf8)?;
    let line = text.trim_end_matches(['\n', '\r']);

    let mut parts = line.splitn(3, ' ');
    let banner = parts.next().unwrap_or("");
    if banner != ACTIVATION_PROTOCOL {
        return Err(ActivationError::WrongProtocol { found: banner.to_string() });
    }
    let verb = parts.next().unwrap_or("");
    match verb {
        "focus" => match parts.next() {
            None => Ok(Activation::Focus),
            Some(extra) => Err(ActivationError::TrailingData { extra: extra.to_string() }),
        },
        "open" => {
            let url = parts.next().ok_or(ActivationError::MissingUrl)?;
            if url.is_empty() {
                return Err(ActivationError::MissingUrl);
            }
            deeplink::parse(url).map(Activation::Open).map_err(ActivationError::BadUrl)
        }
        other => Err(ActivationError::UnknownVerb { verb: other.to_string() }),
    }
}

/// Bytes available for a Unix domain socket path, including the terminator.
///
/// `sun_path` is 108 bytes on Linux and 104 on the BSDs including macOS. This
/// is not a theoretical limit: a socket under a long `$XDG_RUNTIME_DIR` or a
/// sandboxed `$TMPDIR` on macOS gets genuinely close, and `bind` fails with
/// `ENAMETOOLONG` rather than truncating, which is why it is checked up front
/// with a message naming the path.
pub(crate) const fn unix_socket_path_limit(platform: Platform) -> usize {
    match platform {
        Platform::MacOs => 104,
        Platform::Linux => 108,
        // Windows named pipes are not `sun_path` and have their own limit.
        Platform::Windows => 256,
    }
}

/// Reject a socket path that will not fit in `sockaddr_un`.
pub(crate) fn check_socket_path(platform: Platform, path: &Path) -> Result<(), SingleInstanceError> {
    let limit = unix_socket_path_limit(platform);
    let len = path.as_os_str().as_encoded_bytes().len();
    // The stored path must be NUL-terminated, so the usable length is one less.
    if len + 1 > limit {
        return Err(SingleInstanceError::SocketPathTooLong {
            path: path.to_path_buf(),
            len,
            limit: limit - 1,
        });
    }
    Ok(())
}

/// Windows named mutex name.
///
/// The `Local\` namespace is per logon session, which is exactly the scope we
/// want: two users on one machine each get their own instance, and a Remote
/// Desktop session does not steal the console session's window.
pub fn windows_mutex_name() -> String {
    format!("Local\\{BUNDLE_ID}.instance")
}

/// Windows named pipe path for the handoff.
///
/// Pipe names are machine-global regardless of the mutex namespace, so the
/// user name is part of the path. Without it a second user's launch would try
/// to hand off to the first user's process and be refused by the pipe ACL,
/// which looks like a hang.
pub fn windows_pipe_name(user: &str) -> String {
    let sanitised: String = user
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!(r"\\.\pipe\{APP_NAME}-instance-{sanitised}")
}

/// Why single-instance setup failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SingleInstanceError {
    /// The socket path exceeds `sockaddr_un`.
    SocketPathTooLong {
        /// The path that did not fit.
        path: PathBuf,
        /// Its length in bytes.
        len: usize,
        /// The platform's `sun_path` capacity.
        limit: usize,
    },
    /// A filesystem or syscall failure, with what was being attempted.
    Io {
        /// What was being attempted, as a sentence fragment the message prefixes
        /// the detail with.
        context: String,
        /// The underlying error text.
        detail: String,
    },
    /// Another instance holds the lock but did not accept the handoff. Usually
    /// means it is still starting, or wedged.
    PrimaryUnreachable {
        /// What went wrong reaching the primary.
        detail: String,
    },
}

impl fmt::Display for SingleInstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SocketPathTooLong { path, len, limit } => write!(
                f,
                "socket path {} is {len} bytes, the platform limit is {limit}",
                path.display()
            ),
            Self::Io { context, detail } => write!(f, "{context}: {detail}"),
            Self::PrimaryUnreachable { detail } => {
                write!(f, "another instance holds the lock but is not accepting handoffs: {detail}")
            }
        }
    }
}

impl core::error::Error for SingleInstanceError {}

impl SingleInstanceError {
    /// Classify for a capability report. Only an unreachable primary is
    /// transient: the other two will fail the same way on a retry.
    pub fn to_unavailable(&self) -> Unavailable {
        match self {
            Self::SocketPathTooLong { .. } => Unavailable::runtime_error(self.to_string()),
            Self::Io { .. } => Unavailable::runtime_error(self.to_string()),
            Self::PrimaryUnreachable { .. } => Unavailable::service_missing(self.to_string()),
        }
    }
}

/// Called on the primary when a second launch hands off.
pub type ActivationSink = Arc<dyn Fn(Activation) + Send + Sync>;

/// Which side of the race this process is on.
///
/// `Debug` reports only the side, never the guard, because a guard holds live
/// kernel handles that have no useful textual form.
pub enum Acquisition {
    /// This process is the one and only instance. Hold the guard for the
    /// process lifetime; dropping it releases the claim.
    Primary(InstanceGuard),
    /// Another instance was already running and has been handed the
    /// activation. This process should exit.
    HandedOff,
}

impl Acquisition {
    /// True when this process won the race and must keep its guard alive for
    /// the rest of its life.
    pub fn is_primary(&self) -> bool {
        matches!(self, Self::Primary(_))
    }
}

impl fmt::Debug for Acquisition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primary(_) => f.write_str("Primary"),
            Self::HandedOff => f.write_str("HandedOff"),
        }
    }
}

/// The live claim on being the single instance.
pub struct InstanceGuard {
    #[cfg(unix)]
    inner: unix::UnixGuard,
    #[cfg(windows)]
    inner: windows::WindowsGuard,
}

impl InstanceGuard {
    /// Start accepting handoffs from later launches.
    ///
    /// Spawns one thread parked in `accept`. It exits when the guard is
    /// dropped.
    pub fn listen(&self, sink: ActivationSink) -> Result<(), SingleInstanceError> {
        self.inner.listen(sink)
    }
}

/// Claim the single-instance slot, or hand `activation` to whoever holds it.
///
/// `lock_path` and `socket_path` come from [`crate::paths::AppPaths`]. On
/// Windows `socket_path` is unused; the named pipe is derived from the user
/// name instead, because a pipe is not a filesystem object.
pub fn acquire(
    lock_path: &Path,
    socket_path: &Path,
    activation: &Activation,
) -> Result<Acquisition, SingleInstanceError> {
    #[cfg(unix)]
    {
        unix::acquire(lock_path, socket_path, activation)
    }
    #[cfg(windows)]
    {
        let _ = socket_path;
        windows::acquire(lock_path, activation)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (lock_path, socket_path, activation);
        Err(SingleInstanceError::Io {
            context: "single instance".to_string(),
            detail: format!("unsupported platform {}", std::env::consts::OS),
        })
    }
}

/// Whether the single-instance mechanism can work, without taking the claim.
pub fn probe(paths: &crate::paths::AppPaths) -> crate::capability::Support {
    use crate::capability::Support;

    let socket = paths.instance_socket_path();
    if let Err(e) = check_socket_path(Platform::current(), &socket) {
        return Support::Missing(e.to_unavailable());
    }
    match std::fs::create_dir_all(&paths.runtime_dir) {
        Ok(()) => Support::Available,
        Err(e) => Support::Missing(Unavailable::permission_denied(format!(
            "cannot create the runtime directory {}: {e}",
            paths.runtime_dir.display()
        ))),
    }
}
