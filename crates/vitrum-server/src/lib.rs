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

/// Accept clients until the listener fails.
///
/// Connections share the hub and nothing else: dropping one never disturbs a
/// session or another client, but every one of them sees the same registry.
pub async fn serve(listener: TcpListener, manager: Arc<SessionManager>) -> anyhow::Result<()> {
    let hub = Hub::new(manager);
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            // A connection lost during the accept, or a signal interrupting it,
            // says nothing about the listener. Returning here would take the
            // whole daemon down with every hosted agent for a transient error.
            Err(e) if matches!(e.kind(), ErrorKind::ConnectionAborted | ErrorKind::Interrupted) => {
                tracing::debug!(error = %e, "transient accept failure");
                continue;
            }
            Err(e) => return Err(e).context("accepting a client connection"),
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
    }
}

#[cfg(test)]
mod tests;
