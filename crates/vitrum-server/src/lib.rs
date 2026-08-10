//! The vitrum session daemon.
//!
//! One WebSocket connection carries both planes of the protocol:
//!
//! - **Control**: JSON text frames of [`vitrum_proto::ClientMsg`] and
//!   [`vitrum_proto::ServerMsg`].
//! - **Data**: binary frames of raw PTY bytes, framed by
//!   [`vitrum_proto::encode_output`].
//!
//! The daemon owns every PTY and every scrollback ring, so sessions survive with
//! no client connected and a reconnecting client can ask for exactly the byte
//! range it missed.
//!
//! State is shared along one axis and private along another. Registry events (a
//! session appeared, changed, exited) reach every connected client, because a
//! second window must show sessions it did not start. Output frames and
//! scrollback stay per-attachment, because that traffic belongs to whoever is
//! drawing the pane.

use std::future::Future;
use std::io::ErrorKind;
use std::sync::Arc;

use anyhow::Context;
use vitrum_core::SessionManager;
use tokio::net::TcpListener;

mod conn;
mod hub;
mod overlap;
mod projects;
mod search;

pub use conn::{gap_notice, pump_events, pump_output, serve_connection};
pub use hub::Hub;
pub use overlap::{OverlapService, Publish, live_sessions};
pub use projects::ProjectRegistry;

/// Default port for the session daemon.
pub const DEFAULT_PORT: u16 = 7737;

/// Scrollback retained per session by the binary.
///
/// Measured at roughly 10k lines of agent output, which is the history a user
/// actually scrolls back through. tmux costs about 23 MB for the same.
pub const DEFAULT_SCROLLBACK_BYTES: usize = 10 * 1024 * 1024;

/// Unix milliseconds, the clock every timestamp the daemon stamps is read from.
///
/// One reading site for the crate. The overlap watcher ages its pending opens
/// against the same epoch a collision report is stamped with, so the two have
/// to agree on what a clock that reads before the epoch means; they did so by
/// coincidence, as two identical private copies, until this became the one.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Accept clients until the listener fails or `shutdown` resolves.
///
/// Connections share the hub and nothing else: dropping one never disturbs a
/// session or another client, but every one of them sees the same registry.
///
/// `token` is the shared secret every client must present. It is passed in
/// rather than read here so the binary owns the decision to write a fresh one
/// at startup, and so a test can serve with a token it knows.
pub async fn serve(
    listener: TcpListener,
    manager: Arc<SessionManager>,
    token: String,
) -> anyhow::Result<()> {
    serve_until(listener, manager, token, std::future::pending()).await
}

/// [`serve`], ending when `shutdown` resolves.
///
/// Every session is closed on the way out. Process exit alone very nearly
/// does that on Unix, because the last master descriptor closing hangs each
/// terminal up, but a child that ignores `SIGHUP` outlives it holding a dead
/// terminal, and the operator has no way left to reach it. Ending them here
/// makes the outcome the same on every platform and for every child.
///
/// `shutdown` is a future rather than a channel so the binary can hand it a
/// signal and a test can hand it something it controls, without either of
/// them learning about the other's mechanism.
pub async fn serve_until(
    listener: TcpListener,
    manager: Arc<SessionManager>,
    token: String,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let hub = Hub::new(Arc::clone(&manager), token);
    tokio::pin!(shutdown);
    let outcome = loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = &mut shutdown => break Ok(()),
        };
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            // A connection lost during the accept, or a signal interrupting it,
            // says nothing about the listener. Returning here would take the
            // whole daemon down with every hosted agent for a transient error.
            Err(e) if matches!(e.kind(), ErrorKind::ConnectionAborted | ErrorKind::Interrupted) => {
                tracing::debug!(error = %e, "transient accept failure");
                continue;
            }
            Err(e) => break Err(e).context("accepting a client connection"),
        };
        // Nagle would add up to 40ms to every keystroke echo on a loopback
        // socket, which is exactly the latency a terminal must not have.
        if let Err(e) = stream.set_nodelay(true) {
            tracing::debug!(error = %e, "could not disable Nagle");
        }

        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            if let Err(e) = serve_connection(stream, hub).await {
                tracing::warn!(%peer, error = %format!("{e:#}"), "connection failed");
            }
        });
    };
    let ended = manager.close_all();
    if ended > 0 {
        tracing::info!(sessions = ended, "ended every hosted session on shutdown");
    }
    outcome
}

#[cfg(test)]
mod tests;
