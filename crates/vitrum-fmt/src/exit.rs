//! How a process ended: `exited 0`, `exited 101`, `killed (SIGKILL)`.
//!
//! # Why this is a type and not a number
//!
//! "The process exited with 137" is not the same fact as "the process was
//! killed", even though a POSIX shell reports both as 137. The shell's
//! `128 + signal` convention is lossy: a program is free to call `exit(137)`
//! for its own reasons. [`Termination`] keeps the two apart because the daemon
//! waits on the child itself and knows which one happened, and nothing here
//! ever infers a signal from an exit code.
//!
//! # Platforms
//!
//! [`Termination::from_status`] reads signals and core dumps through
//! `ExitStatusExt` on Unix and falls back to the raw code elsewhere. Signal
//! *numbering* is not portable (`SIGUSR1` is 10 on Linux and 30 on macOS), so
//! [`signal_name`] uses the host's numbering: signals 1-15 that every POSIX
//! platform agrees on are shared, and the rest are selected per target family.
//! An unrecognised number renders as `signal 77` rather than a wrong name.
//!
//! On Windows there are no signals, and an abnormal termination arrives as an
//! NTSTATUS-shaped exit code with the high bit set. Those are rendered in hex
//! with their meaning where it is known (`exited 0xc0000005 (access
//! violation)`), because `exited -1073741819` tells a user nothing. This
//! decoding is driven purely by the value, so it is identical and testable on
//! every host.

use std::fmt;

/// The reason a child process is no longer running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Termination {
    /// The process returned from `main` or called `exit`.
    Exited(i32),
    /// The process was terminated by a signal (Unix only).
    Signaled {
        /// Host signal number.
        signal: i32,
        /// Whether the kernel wrote a core file.
        core_dumped: bool,
    },
    /// The process was stopped rather than terminated (Unix job control).
    Stopped {
        /// Host signal number.
        signal: i32,
    },
}

impl Termination {
    /// A clean exit: code zero, and not a signal.
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Exited(0))
    }

    /// Read a [`std::process::ExitStatus`] without losing the signal.
    #[must_use]
    pub fn from_status(status: &std::process::ExitStatus) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return Self::Signaled {
                    signal,
                    core_dumped: status.core_dumped(),
                };
            }
            if let Some(signal) = status.stopped_signal() {
                return Self::Stopped { signal };
            }
        }
        Self::Exited(status.code().unwrap_or_default())
    }
}

impl fmt::Display for Termination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Termination::Exited(code) => write!(f, "{}", ExitCode(code)),
            Termination::Signaled {
                signal,
                core_dumped,
            } => match (signal_name(signal), core_dumped) {
                (Some(name), false) => write!(f, "killed ({name})"),
                (Some(name), true) => write!(f, "killed ({name}, core dumped)"),
                (None, false) => write!(f, "killed (signal {signal})"),
                (None, true) => write!(f, "killed (signal {signal}, core dumped)"),
            },
            Termination::Stopped { signal } => match signal_name(signal) {
                Some(name) => write!(f, "stopped ({name})"),
                None => write!(f, "stopped (signal {signal})"),
            },
        }
    }
}

/// The user-facing sentence fragment for a termination.
///
/// `exited 0`, `exited 101`, `exited 0xc0000005 (access violation)`,
/// `killed (SIGKILL)`, `killed (SIGSEGV, core dumped)`, `killed (signal 77)`,
/// `stopped (SIGTSTP)`.
///
/// A caller splicing this into a larger message should write the [`Display`]
/// impl instead; this is the shorthand for one that genuinely wants a `String`.
#[must_use]
pub fn describe(termination: Termination) -> String {
    termination.to_string()
}

/// An exit code with no signal information, as a [`Display`].
///
/// Rendering through a formatter rather than returning a `String` is what lets
/// [`Termination`] splice a code into a status line without allocating one
/// buffer to build it and a second to hold the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExitCode(pub i32);

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let raw = self.0 as u32;
        if raw < 0x8000_0000 {
            return write!(f, "exited {}", self.0);
        }
        match ntstatus_name(raw) {
            Some(name) => write!(f, "exited 0x{raw:08x} ({name})"),
            None => write!(f, "exited 0x{raw:08x}"),
        }
    }
}

/// The user-facing fragment for an exit CODE alone: `exited 0`, `exited 101`,
/// `exited 0xc0000005 (access violation)`.
///
/// Separate from [`describe`] for callers whose source only ever carries a
/// code, such as a wire protocol that cannot express a signal.
#[must_use]
pub fn describe_code(code: i32) -> String {
    ExitCode(code).to_string()
}

/// The `SIG*` name for a host signal number, or `None` if it is unknown here.
///
/// Signals 1-15 that POSIX fixes identically everywhere are answered on all
/// targets; the rest are answered from the target family's own table, because
/// answering with another platform's numbering would be a confident lie.
#[must_use]
pub fn signal_name(signal: i32) -> Option<&'static str> {
    let shared = match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return platform_signal_name(signal),
    };
    Some(shared)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn platform_signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        7 => "SIGBUS",
        10 => "SIGUSR1",
        12 => "SIGUSR2",
        16 => "SIGSTKFLT",
        17 => "SIGCHLD",
        18 => "SIGCONT",
        19 => "SIGSTOP",
        20 => "SIGTSTP",
        21 => "SIGTTIN",
        22 => "SIGTTOU",
        23 => "SIGURG",
        24 => "SIGXCPU",
        25 => "SIGXFSZ",
        26 => "SIGVTALRM",
        27 => "SIGPROF",
        28 => "SIGWINCH",
        29 => "SIGIO",
        30 => "SIGPWR",
        31 => "SIGSYS",
        _ => return None,
    })
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn platform_signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        7 => "SIGEMT",
        10 => "SIGBUS",
        12 => "SIGSYS",
        16 => "SIGURG",
        17 => "SIGSTOP",
        18 => "SIGTSTP",
        19 => "SIGCONT",
        20 => "SIGCHLD",
        21 => "SIGTTIN",
        22 => "SIGTTOU",
        23 => "SIGIO",
        24 => "SIGXCPU",
        25 => "SIGXFSZ",
        26 => "SIGVTALRM",
        27 => "SIGPROF",
        28 => "SIGWINCH",
        29 => "SIGINFO",
        30 => "SIGUSR1",
        31 => "SIGUSR2",
        _ => return None,
    })
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn platform_signal_name(_signal: i32) -> Option<&'static str> {
    None
}

/// Plain-English meaning of a Windows NTSTATUS-shaped exit code.
///
/// Lowercase, no trailing punctuation: it is spliced into `exited 0x... (...)`.
#[must_use]
pub fn ntstatus_name(status: u32) -> Option<&'static str> {
    Some(match status {
        0x8000_0003 => "breakpoint",
        0xC000_0005 => "access violation",
        0xC000_0006 => "in-page error",
        0xC000_001D => "illegal instruction",
        0xC000_0025 => "noncontinuable exception",
        0xC000_008C => "array bounds exceeded",
        0xC000_008E => "float divide by zero",
        0xC000_0094 => "integer divide by zero",
        0xC000_0096 => "privileged instruction",
        0xC000_00FD => "stack overflow",
        0xC000_0135 => "dll not found",
        0xC000_0139 => "entry point not found",
        0xC000_013A => "interrupted",
        0xC000_0142 => "dll initialization failed",
        0xC000_0374 => "heap corruption",
        0xC000_0409 => "stack buffer overrun",
        _ => return None,
    })
}
