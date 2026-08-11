//! A connection that stopped existing must be released.
//!
//! A window that is quit cleanly closes its socket and everything is tidied on
//! the close frame. A window that is not, because the machine suspended, the
//! network dropped or the process was killed on a host that then went away,
//! leaves a TCP connection that will never carry another byte. Nothing above
//! notices: the read parks, the connection's attachments stay attached, and its
//! geometry stays registered, so every other window is held to the layout of a
//! window that no longer exists. The kernel's own keepalive is two hours and is
//! not even enabled on this socket.
//!
//! Silence is not the signal. An operator reading an agent's output sends
//! nothing for minutes and is entirely present. The signal is silence that does
//! not answer a ping, which every conforming WebSocket peer replies to from
//! inside its own read loop without its application code being involved.

use std::time::Duration;

use vitrum_proto::ClientMsg;

use crate::conn::{IDLE_PROBE, PROBE_DEADLINE};
use crate::tests::client::Harness;

/// How long past the two deadlines a test waits before calling it a failure.
///
/// Only reached when the connection is not being released at all, so it only
/// has to cover scheduling, not the probe.
const SLACK: Duration = Duration::from_secs(5);

/// A client that stops answering must be dropped, not held forever.
///
/// The assertion is that the socket ends. A daemon that never probes fails
/// this by timing out, which is the shape the defect had: nothing crashed,
/// nothing errored, the connection simply stayed.
#[tokio::test]
async fn a_client_that_stops_answering_is_released() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;

    // Never polled again until the deadlines have passed, so no pong is ever
    // sent. This is what a vanished peer looks like from the daemon: an open
    // socket behind a client that is not running.
    tokio::time::sleep(IDLE_PROBE + PROBE_DEADLINE + SLACK).await;

    c.until_closed().await;
}

/// A client that is merely quiet must be kept.
///
/// This is the other half, and it is the half a naive fix breaks: an operator
/// who is reading rather than typing sends no control message for minutes.
/// Closing on silence would disconnect the window the moment it stopped being
/// interacted with.
#[tokio::test]
async fn a_quiet_client_that_answers_the_probe_is_kept() {
    let h = Harness::start(4096).await;
    let mut c = h.greeted().await;

    // Polled throughout, which is all a real client does while idle: reading
    // is what sends the pong.
    let until = tokio::time::Instant::now() + IDLE_PROBE + PROBE_DEADLINE + SLACK;
    while tokio::time::Instant::now() < until {
        c.quiet().await;
        assert!(!c.closed, "a client that answers the probe was disconnected");
    }

    // Still serving, which is the real claim: the connection is usable, not
    // merely un-closed.
    c.send(ClientMsg::List).await;
    c.until("the list after a long quiet spell", |s| {
        s.has(|m| matches!(m, vitrum_proto::ServerMsg::Sessions { .. }))
    })
    .await;
}
