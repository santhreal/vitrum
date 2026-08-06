//! The vitrum session daemon binary.

use std::net::Ipv4Addr;
use std::sync::Arc;

use anyhow::{Context, bail};
use vitrum_core::SessionManager;
use vitrum_server::{DEFAULT_PORT, DEFAULT_SCROLLBACK_BYTES, serve};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        .with_context(|| format!("binding 127.0.0.1:{}", config.port))?;
    let addr = listener.local_addr().context("reading the bound address")?;

    let manager = Arc::new(SessionManager::new(config.scrollback_bytes));
    tracing::info!(
        %addr,
        scrollback_bytes = config.scrollback_bytes,
        "vitrum session server listening"
    );
    serve(listener, manager).await
}

/// What `--help` prints.
///
/// Lists the environment variables as well as the flags. It used to name only
/// the two flags, which left `VITRUM_PORT`, `VITRUM_SCROLLBACK_BYTES` and
/// `VITRUM_LOG` readable by the daemon and undiscoverable by the operator.
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
         VITRUM_LOG               trace, debug, warn or error; default info"
    )
}

struct Config {
    port: u16,
    scrollback_bytes: usize,
    log_level: tracing::Level,
}

impl Config {
    /// Arguments win over the environment, which wins over the defaults.
    fn from_args_and_env(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut port = env_parsed("VITRUM_PORT")?.unwrap_or(DEFAULT_PORT);
        let mut scrollback_bytes =
            env_parsed("VITRUM_SCROLLBACK_BYTES")?.unwrap_or(DEFAULT_SCROLLBACK_BYTES);

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--port" => {
                    let v = args.next().context("--port needs a value")?;
                    port = v.parse().with_context(|| format!("bad --port {v}"))?;
                }
                "--scrollback-bytes" => {
                    let v = args.next().context("--scrollback-bytes needs a value")?;
                    scrollback_bytes = v
                        .parse()
                        .with_context(|| format!("bad --scrollback-bytes {v}"))?;
                }
                "--help" | "-h" => {
                    println!("{}", usage());
                    std::process::exit(0);
                }
                // The client answers `--version` and the updater compares the
                // two binaries by version, so a daemon that cannot state its
                // own is the one piece of the pair a packager cannot check.
                "--version" | "-V" => {
                    println!("vitrum-server {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                }
                other => bail!("unknown argument {other}"),
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

fn env_parsed<T: std::str::FromStr>(key: &str) -> anyhow::Result<Option<T>>
where
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(v) => v
            .parse()
            .map(Some)
            .map_err(|e| anyhow::anyhow!("bad {key}={v}: {e}")),
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
}
