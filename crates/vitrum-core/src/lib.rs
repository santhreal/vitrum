//! PTY hosting and bounded scrollback for vitrum sessions.
//!
//! This crate is the half of vitrum that owns real processes. A [`SessionSpec`]
//! becomes a child running under a pseudoterminal, its output is coalesced and
//! fanned out over a [`tokio::sync::broadcast`] channel, and a bounded
//! [`Scrollback`] ring keeps recent history whether or not any client is
//! attached.
//!
//! Two properties drive the design:
//!
//! - **The client stays thin.** Scrollback lives here, so a GUI showing one of
//!   twenty sessions holds one viewport rather than twenty histories.
//! - **Idle is free.** Nothing here polls or ticks. Every loop is parked on a
//!   channel, and the only timer is the coalescing window, which exists only
//!   while output is actually pending.
//!
//! [`portable_pty`] is used deliberately rather than a Unix-only PTY: it is
//! what makes ConPTY on Windows work without a terminal multiplexer anywhere in
//! the product.

mod error;
mod probe;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod scan;
mod scrollback;
mod session;

pub use error::SessionError;
pub use scrollback::Scrollback;
pub use session::{OutputChunk, PumpCounts, SessionManager, SessionSpec, ViewerId};

#[cfg(test)]
mod tests;
