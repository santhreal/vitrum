//! The vitrum session daemon binary.
//!
//! Everything that can stop this process before it serves is a
//! [`StartupError`], and every one of them names the corrective action. The
//! daemon is started by hand, by a systemd unit, and by the client's autostart
//! path, and none of those three read prose: the exit code is the only thing
//! the last two can act on, so it comes from the one table in
//! [`vitrum_proto::exit`] rather than from `anyhow`'s blanket 1.

use std::fmt;
use std::net::Ipv4Addr;
use std::process::ExitCode;
use std::sync::Arc;

use vitrum_core::SessionManager;
use vitrum_proto::exit::{self, Exit};
use vitrum_server::{DEFAULT_PORT, DEFAULT_SCROLLBACK_BYTES, serve};
use tokio::net::TcpListener;

/// Why the daemon did not get as far as serving.
///
/// One type for the startup boundary. Each variant is a different thing for
/// the operator to go and fix, and each maps to a different exit code, because
/// "the port is taken" and "that flag does not exist" call for opposite
/// responses from whatever started this: retry after freeing the port, versus
/// never retry, the command is wrong.
#[derive(Debug)]
enum StartupError {
    /// A flag, a value, or an environment variable this build cannot read.
    Usage(String),
    /// Something is already listening on the port.
    ///
    /// Overwhelmingly this is a second daemon, which is not a problem: the
    /// client connects to whichever one is up and reuses it. Saying so is what
    /// stops an operator killing a daemon that is holding their sessions.
    PortTaken { port: u16 },
    /// The OS refused the bind for a reason other than the port being in use.
    CannotBind {
        port: u16,
        cause: std::io::Error,
    },
    /// The listener bound and then could not be interrogated.
    NoAddress(std::io::Error),
    /// The serving loop ended in an error.
    Serving(anyhow::Error),
}

impl StartupError {
    /// The process exit code, from the one shared table.
    fn exit(&self) -> Exit {
        match self {
            StartupError::Usage(_) => Exit::Usage,
            // Both are "this machine is not ready": free the port, or run as
            // somebody who may bind it, and the same command works.
            StartupError::PortTaken { .. } => Exit::Unavailable,
            StartupError::CannotBind { cause, .. }
                if cause.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                Exit::Unavailable
            }
            StartupError::CannotBind { .. }
            | StartupError::NoAddress(_)
            | StartupError::Serving(_) => Exit::Failed,
        }
    }
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartupError::Usage(detail) => write!(f, "{detail}\n\n{}", usage()),
            StartupError::PortTaken { port } => write!(
                f,
                "something is already listening on 127.0.0.1:{port}, so this \
                 daemon did not start.\n\
                 That is almost always another vitrum-server, and it is the one \
                 holding your sessions: the client connects to whichever daemon \
                 is up and reuses it, so nothing needs to be done. To run a \
                 second one alongside it, pass --port with a free port."
            ),
            StartupError::CannotBind { port, cause }
                if cause.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                write!(
                    f,
                    "not allowed to bind 127.0.0.1:{port}: {cause}\n\
                     Ports below 1024 need privilege on most systems. Pass \
                     --port with a port above 1024, or set VITRUM_PORT."
                )
            }
            StartupError::CannotBind { port, cause } => write!(
                f,
                "could not bind 127.0.0.1:{port}: {cause}\n\
                 The daemon listens on loopback only. Check that loopback is up \
                 on this machine, then try --port with a different port."
            ),
            StartupError::NoAddress(cause) => write!(
                f,
                "bound the port and could not read the address back: {cause}\n\
                 Nothing is serving. Start the daemon again; if it repeats, the \
                 socket layer on this machine is not answering and the client \
                 will not be able to connect either."
            ),
            StartupError::Serving(cause) => write!(
                f,
                "the daemon stopped serving: {cause:#}\n\
                 Every session it was holding has ended. Start it again, or let \
                 the client start it on the next launch."
            ),
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::from(Exit::Ok.code() as u8),
        Err(e) => {
            // stderr, not `tracing`: the subscriber may not be installed yet,
            // and the client reads this file back when it reports why the
            // daemon it spawned did not come up.
            eprintln!("vitrum-server: {e}");
            ExitCode::from(e.exit().code() as u8)
        }
    }
}

async fn run() -> Result<(), StartupError> {
    let config = Config::from_args_and_env(std::env::args().skip(1))?;

    tracing_subscriber::fmt()
        .with_max_level(config.log_level)
        .with_target(false)
        .init();

    // Loopback only, never 0.0.0.0. The daemon spawns arbitrary processes on
    // request, so a listener reachable from the network would be remote code
    // execution rather than a feature.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))
        .await
        .map_err(|cause| {
            // The common case by a wide margin, and the only one where the
            // right answer is "nothing is wrong". Two clients racing to start
            // one daemon both reach here and the loser must not read as a
            // broken install.
            if cause.kind() == std::io::ErrorKind::AddrInUse {
                StartupError::PortTaken { port: config.port }
            } else {
                StartupError::CannotBind {
                    port: config.port,
                    cause,
                }
            }
        })?;
    let addr = listener.local_addr().map_err(StartupError::NoAddress)?;

    let manager = Arc::new(SessionManager::new(config.scrollback_bytes));
    tracing::info!(
        "vitrum-server {} listening on ws://{} with {} bytes of scrollback per session",
        env!("CARGO_PKG_VERSION"),
        addr,
        config.scrollback_bytes
    );
    serve(listener, manager).await.map_err(StartupError::Serving)
}

/// What `--help` prints.
///
/// Lists the environment variables as well as the flags. It used to name only
/// the two flags, which left `VITRUM_PORT`, `VITRUM_SCROLLBACK_BYTES` and
/// `VITRUM_LOG` readable by the daemon and undiscoverable by the operator.
///
/// The exit codes are rendered from the shared table rather than written out,
/// so the daemon and the client cannot come to disagree about what a 3 means.
fn usage() -> String {
    format!(
        "vitrum-server [--port {DEFAULT_PORT}] [--scrollback-bytes {DEFAULT_SCROLLBACK_BYTES}]\n\
         \n\
         Arguments win over the environment, which wins over these defaults.\n\
         \n\
         options:\n  \
         --port                   port to bind on loopback\n  \
         --scrollback-bytes       retained output per session\n  \
         -h, --help               show this message\n  \
         -V, --version            print the version and exit\n\
         \n\
         environment:\n  \
         VITRUM_PORT              same as --port\n  \
         VITRUM_SCROLLBACK_BYTES  same as --scrollback-bytes\n  \
         VITRUM_LOG               trace, debug, warn or error; default info\n\
         \n\
         exit status:\n\
         {}",
        exit::status_lines(EXIT_CODES)
    )
}

/// Every code `vitrum-server` can exit with.
const EXIT_CODES: &[Exit] = &[Exit::Ok, Exit::Failed, Exit::Usage, Exit::Unavailable];

#[derive(Debug)]
struct Config {
    port: u16,
    scrollback_bytes: usize,
    log_level: tracing::Level,
}

impl Config {
    /// Arguments win over the environment, which wins over the defaults.
    fn from_args_and_env(args: impl Iterator<Item = String>) -> Result<Self, StartupError> {
        let mut port = env_parsed("VITRUM_PORT")?.unwrap_or(DEFAULT_PORT);
        let mut scrollback_bytes =
            env_parsed("VITRUM_SCROLLBACK_BYTES")?.unwrap_or(DEFAULT_SCROLLBACK_BYTES);

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--port" => {
                    let v = args
                        .next()
                        .ok_or_else(|| misuse("--port needs a value: a port number"))?;
                    port = v.parse().map_err(|_| {
                        misuse(format!("--port {v} is not a port number (0 to 65535)"))
                    })?;
                }
                "--scrollback-bytes" => {
                    let v = args.next().ok_or_else(|| {
                        misuse("--scrollback-bytes needs a value: a byte count")
                    })?;
                    scrollback_bytes = v.parse().map_err(|_| {
                        misuse(format!("--scrollback-bytes {v} is not a byte count"))
                    })?;
                }
                "--help" | "-h" => {
                    println!("{}", usage());
                    std::process::exit(Exit::Ok.code());
                }
                // The client answers `--version` and the updater compares the
                // two binaries by version, so a daemon that cannot state its
                // own is the one piece of the pair a packager cannot check.
                "--version" | "-V" => {
                    println!("vitrum-server {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(Exit::Ok.code());
                }
                other => return Err(misuse(format!("unknown argument {other}"))),
            }
        }

        let log_level = match std::env::var("VITRUM_LOG").ok().as_deref() {
            Some("trace") => tracing::Level::TRACE,
            Some("debug") => tracing::Level::DEBUG,
            Some("warn") => tracing::Level::WARN,
            Some("error") => tracing::Level::ERROR,
            _ => tracing::Level::INFO,
        };

        Ok(Self {
            port,
            scrollback_bytes,
            log_level,
        })
    }
}

/// A command line this build cannot act on. The help follows it, once, from
/// [`StartupError`]'s own `Display`.
fn misuse(detail: impl Into<String>) -> StartupError {
    StartupError::Usage(detail.into())
}

/// An environment variable, parsed, or a usage error naming the variable.
///
/// A bad `VITRUM_PORT` is the same class of mistake as a bad `--port` and gets
/// the same code: an exported typo is not going to fix itself on a retry.
fn env_parsed<T: std::str::FromStr>(key: &str) -> Result<Option<T>, StartupError>
where
    T::Err: fmt::Display,
{
    match std::env::var(key) {
        Ok(v) => v
            .parse()
            .map(Some)
            .map_err(|e| misuse(format!("{key}={v} in the environment is not usable: {e}"))),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every environment variable the daemon reads must appear in `--help`.
    ///
    /// The help used to list the two flags and nothing else, so `VITRUM_PORT`,
    /// `VITRUM_SCROLLBACK_BYTES` and `VITRUM_LOG` all changed the daemon's
    /// behaviour while being undiscoverable from the binary. An operator
    /// cannot use a knob they cannot find, and a knob nobody can find is
    /// indistinguishable from one that does not work.
    ///
    /// This reads the source rather than a hand-written list, so adding a
    /// fourth variable and forgetting the help fails here instead of shipping.
    /// It matches the two call shapes that actually read the environment, so a
    /// variable merely NAMED in a comment does not count as a read.
    #[test]
    fn every_environment_variable_is_documented_in_help() {
        let src = include_str!("main.rs");
        let help = usage();
        let mut found = Vec::new();
        for opener in ["env_parsed(\"", "env::var(\""] {
            let mut rest = src;
            while let Some(at) = rest.find(opener) {
                rest = &rest[at + opener.len()..];
                let Some(end) = rest.find('"') else { break };
                let name = &rest[..end];
                if name.starts_with("VITRUM_") {
                    found.push(name.to_string());
                }
            }
        }
        found.sort();
        found.dedup();
        assert_eq!(
            found,
            ["VITRUM_LOG", "VITRUM_PORT", "VITRUM_SCROLLBACK_BYTES"],
            "the set of environment variables the daemon reads changed"
        );
        for name in &found {
            assert!(
                help.contains(name.as_str()),
                "{name} changes daemon behaviour but --help never mentions it:\n{help}"
            );
        }
    }

    /// Help is printed verbatim, so a mis-escaped format placeholder reaches
    /// the operator. The client binary shipped a literal `%%` this way; this
    /// is the same guard on the daemon's own help.
    #[test]
    fn help_text_contains_no_unrendered_escapes() {
        let help = usage();
        assert!(!help.contains("%%"), "{help}");
        assert!(!help.contains('{'), "{help}");
        assert!(!help.contains('}'), "{help}");
        assert!(
            help.contains(&DEFAULT_PORT.to_string()),
            "the default port placeholder did not render: {help}"
        );
    }

    /// Every flag the parser accepts must appear in the help.
    ///
    /// The daemon answered `--help` and rejected `--version` with `unknown
    /// argument`, while the client and `vitrum-replay` both answered it. The
    /// pair is installed together and the updater compares them by version,
    /// so the one binary a packager could not ask was the daemon.
    ///
    /// Read out of the parser rather than from a list, so adding an arm and
    /// forgetting the help fails here.
    #[test]
    fn every_flag_the_parser_accepts_is_in_the_help() {
        let src = include_str!("main.rs");
        let help = usage();
        let arms = src
            .split("=> {")
            .filter_map(|chunk| chunk.rsplit_once('\n'))
            .map(|(_, arm)| arm.trim())
            .filter(|arm| arm.starts_with('"'));
        let mut found: Vec<String> = arms
            .flat_map(|arm| arm.split('|'))
            .filter_map(|word| word.trim().trim_matches('"').strip_prefix('-'))
            .map(|flag| format!("-{flag}"))
            .collect();
        found.sort();
        found.dedup();
        assert_eq!(
            found,
            ["--help", "--port", "--scrollback-bytes", "--version", "-V", "-h"],
            "the set of flags the daemon accepts changed"
        );
        for flag in &found {
            assert!(
                help.contains(flag.as_str()),
                "{flag} is accepted but --help never mentions it:\n{help}"
            );
        }
    }

    /// The daemon declares the codes it returns, and the source agrees.
    ///
    /// Read out of `StartupError::exit` and `main` rather than from a list, so
    /// a new startup failure that returns a code nobody documented turns this
    /// red. Both halves of the product render their `exit status:` block from
    /// [`vitrum_proto::exit`], so the meanings cannot diverge; what can
    /// diverge is which subset each one claims.
    #[test]
    fn the_declared_codes_are_the_codes_the_source_returns() {
        // Only the shipped half. This module's own code names every variant
        // while decoding them, and counting those would make the test agree
        // with itself instead of with the daemon.
        let src = include_str!("main.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("main.rs has a body before its tests");
        let mut found: Vec<Exit> = Vec::new();
        for line in src.lines() {
            if line.contains("EXIT_CODES") {
                continue;
            }
            let mut rest = line;
            while let Some(at) = rest.find("Exit::") {
                rest = &rest[at + "Exit::".len()..];
                let end = rest
                    .find(|c: char| !c.is_ascii_alphabetic())
                    .unwrap_or(rest.len());
                let code = match &rest[..end] {
                    "Ok" => Exit::Ok,
                    "Failed" => Exit::Failed,
                    "Usage" => Exit::Usage,
                    "Unavailable" => Exit::Unavailable,
                    "Offline" => Exit::Offline,
                    "Corrupt" => Exit::Corrupt,
                    _ => continue,
                };
                if !found.contains(&code) {
                    found.push(code);
                }
            }
        }
        found.sort();
        let mut declared = EXIT_CODES.to_vec();
        declared.sort();
        assert_eq!(
            found, declared,
            "vitrum-server returns {found:?} and declares {declared:?}"
        );

        let help = usage();
        for code in EXIT_CODES {
            assert!(
                help.contains(&format!("  {}", code.code())),
                "exit {} is never documented:\n{help}",
                code.code()
            );
            assert!(help.contains(code.meaning()), "{help}");
        }
    }

    /// A port that is already taken is not a broken install, and the exit code
    /// says so.
    ///
    /// This is the two-clients-one-daemon race seen from the loser's side. The
    /// client serialises its own spawns, but a systemd unit racing a manual
    /// start, or a second machine's client over a forwarded port, both land
    /// here. Reporting it as a generic failure is what makes an operator kill
    /// the daemon that is holding their twenty sessions.
    #[test]
    fn a_taken_port_is_unavailable_and_says_the_other_daemon_is_fine() {
        let e = StartupError::PortTaken { port: 7737 };
        assert_eq!(e.exit(), Exit::Unavailable);
        assert_ne!(e.exit().code(), Exit::Failed.code());
        let m = e.to_string();
        assert!(m.contains("7737"), "{m}");
        assert!(m.contains("--port"), "no way out is offered: {m}");
        assert!(
            m.contains("reuses it"),
            "does not say the running daemon is the one to keep: {m}"
        );
    }

    /// A privileged port and an unusable one are different problems.
    ///
    /// Both are refusals to bind, and only one of them is fixed by picking a
    /// different port; collapsing them sends an operator to the wrong place.
    #[test]
    fn a_refused_bind_distinguishes_privilege_from_everything_else() {
        use std::io::{Error, ErrorKind};

        let denied = StartupError::CannotBind {
            port: 80,
            cause: Error::new(ErrorKind::PermissionDenied, "permission denied"),
        };
        assert_eq!(denied.exit(), Exit::Unavailable);
        assert!(denied.to_string().contains("1024"), "{denied}");

        let broken = StartupError::CannotBind {
            port: 7737,
            cause: Error::new(ErrorKind::AddrNotAvailable, "cannot assign"),
        };
        assert_eq!(broken.exit(), Exit::Failed);
        assert!(broken.to_string().contains("loopback"), "{broken}");
    }

    /// A wrong flag and a wrong environment variable are the same mistake and
    /// get the same code, and both name the variable or flag at fault.
    #[test]
    fn a_bad_invocation_is_a_usage_error_wherever_it_came_from() {
        let flag = Config::from_args_and_env(["--port".to_string()].into_iter())
            .expect_err("a value is required");
        assert_eq!(flag.exit(), Exit::Usage);
        assert!(flag.to_string().starts_with("--port needs a value"), "{flag}");

        let value = Config::from_args_and_env(
            ["--port".to_string(), "eighty".to_string()].into_iter(),
        )
        .expect_err("not a number");
        assert_eq!(value.exit(), Exit::Usage);
        assert!(value.to_string().contains("eighty"), "{value}");

        let unknown = Config::from_args_and_env(["--turbo".to_string()].into_iter())
            .expect_err("no such flag");
        assert_eq!(unknown.exit(), Exit::Usage);
        assert!(unknown.to_string().contains("--turbo"), "{unknown}");

        // Every usage error shows how to call the daemon, once. A refusal with
        // no help is a refusal the operator has to go and look up.
        for e in [flag, value, unknown] {
            assert!(e.to_string().contains("vitrum-server [--port"), "{e}");
        }
    }
}
