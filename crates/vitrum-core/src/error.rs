//! What the session manager refuses, and what to do about it.
//!
//! One type for the boundary between a request and a PTY. Every variant here
//! is something an operator can reach by working normally — closing a tab while
//! a keystroke is in flight, opening a project whose checkout was moved, typing
//! the name of an agent this machine does not have — and every one of them
//! carries the corrective action rather than only the fault.
//!
//! It is deliberately not every failure the manager can produce. Cloning a PTY
//! reader or taking its writer can fail, and when they do there is nothing for
//! the operator to do differently; those stay `anyhow` with context on them.
//! What is here is the set a person can act on, which is also the set that
//! reaches the client's error banner and gets read.
//!
//! The sentences are constrained by where they end up.
//! [`vitrum_proto::MAX_ERROR_CHARS`] bounds what survives the wire, and the
//! wire layer removes the MIDDLE of anything longer, so a message that runs
//! past it loses its fault or its fix rather than its tail. Each one here fits.

use std::fmt;

use vitrum_proto::{SessionId, display_safe};

/// Why the session manager would not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// No session with that id, on this daemon, now.
    ///
    /// Ordinary rather than exceptional: a window closing a tab while a resize
    /// is in flight produces one, and so does a client holding ids from before
    /// the daemon was restarted.
    NoSuchSession { id: SessionId },
    /// The session exists and its child is gone.
    Exited { id: SessionId },
    /// A session was requested with nothing to run.
    EmptyCommand,
    /// The working directory does not exist on the machine running the daemon.
    ///
    /// Which is not necessarily the machine the operator is looking at, and
    /// the message says so: a path that is plainly there in their file manager
    /// is not there for a daemon in a container or on another host.
    MissingCwd { cwd: String },
    /// The program is not resolvable on the daemon's `PATH`.
    NotOnPath { command: String },
    /// The child could not be started, for a reason the PTY layer named.
    CannotStart { command: String, detail: String },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // "no session {id}" leads, because that phrase is what a client
            // matches on and what an operator searching a log types.
            SessionError::NoSuchSession { id } => write!(
                f,
                "no session {}; it was closed, or this window is holding an id \
                 from before the daemon restarted. Reload the window to \
                 resynchronise with the daemon.",
                id.0
            ),
            SessionError::Exited { id } => write!(
                f,
                "session {} has exited, so there is nowhere for this to go. \
                 Start a new session in the same directory to carry on.",
                id.0
            ),
            SessionError::EmptyCommand => f.write_str(
                "a session needs a command to run and none was given. Pick an \
                 agent from the launcher, or type the program to start.",
            ),
            SessionError::MissingCwd { cwd } => write!(
                f,
                "cwd {} is not a directory on the machine running the daemon, \
                 so nothing was started. Create it, mount it, or pick a \
                 directory that is there.",
                display_safe(cwd)
            ),
            // The underlying error for this case is `No viable candidates
            // found in PATH "..."` followed by every entry of PATH: over a
            // kilobyte on a normal machine, and none of it answers the only
            // question the operator has, which is what to type instead.
            SessionError::NotOnPath { command } => write!(
                f,
                "no command named {} on the daemon's PATH. Give an absolute \
                 path, or install it and restart vitrum-server, which reads \
                 PATH once when it starts.",
                display_safe(command)
            ),
            SessionError::CannotStart { command, detail } => write!(
                f,
                "could not start {}: {}",
                display_safe(command),
                display_safe(detail)
            ),
        }
    }
}

impl std::error::Error for SessionError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<SessionError> {
        vec![
            SessionError::NoSuchSession {
                id: SessionId(7),
            },
            SessionError::Exited {
                id: SessionId(7),
            },
            SessionError::EmptyCommand,
            SessionError::MissingCwd {
                cwd: "/src/vitrum".into(),
            },
            SessionError::NotOnPath {
                command: "claude".into(),
            },
            SessionError::CannotStart {
                command: "claude".into(),
                detail: "permission denied".into(),
            },
        ]
    }

    /// Every variant names the fault AND what to do next.
    ///
    /// The one exception is [`SessionError::CannotStart`], whose detail comes
    /// from the platform and cannot be predicted; it is checked separately for
    /// naming the command instead.
    ///
    /// This is the whole point of the type. Before it, an operator whose
    /// checkout had moved got `cwd /src/x is not a directory` and no hint that
    /// the machine being asked was the daemon's, which on a container install
    /// is a different filesystem entirely.
    #[test]
    fn every_message_carries_a_next_step() {
        for e in all() {
            let text = e.to_string();
            let actionable = ["Reload", "Start a new", "Pick an", "Create it", "Give an"]
                .iter()
                .any(|hint| text.contains(hint));
            if matches!(e, SessionError::CannotStart { .. }) {
                assert!(text.contains("claude"), "{text}");
                continue;
            }
            assert!(actionable, "{e:?} says what went wrong and not what to do: {text}");
        }
    }

    /// Nothing here may be cut in half by the wire layer.
    ///
    /// `ServerMsg::error` removes the MIDDLE of anything past
    /// [`vitrum_proto::MAX_ERROR_CHARS`], keeping both ends. A message that
    /// overruns therefore loses the sentence in the middle, which in every one
    /// of these is where the fault stops and the fix begins.
    #[test]
    fn every_message_survives_the_wire_intact() {
        for e in all() {
            let text = e.to_string();
            assert_eq!(
                vitrum_proto::error_text(&text),
                text,
                "{e:?} is {} characters and the wire cap is {}",
                text.chars().count(),
                vitrum_proto::MAX_ERROR_CHARS
            );
        }
    }

    /// The phrases clients and logs match on are load-bearing and stay.
    #[test]
    fn the_matched_phrases_are_kept() {
        assert!(
            SessionError::NoSuchSession { id: SessionId(3) }
                .to_string()
                .starts_with("no session 3"),
        );
        assert!(
            SessionError::Exited { id: SessionId(3) }
                .to_string()
                .contains("has exited")
        );
        assert!(
            SessionError::NotOnPath {
                command: "codex".into()
            }
            .to_string()
            .starts_with("no command named codex")
        );
    }
}
