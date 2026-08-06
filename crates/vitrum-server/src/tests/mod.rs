//! Test suites for vitrum-server, one concern per module.

mod attach_stream;
mod client;
#[cfg(unix)]
mod geometry;
mod handshake;
mod input_resize;
mod lag;
mod lifecycle;
mod listing;
mod multi_client;
#[cfg(not(windows))]
mod observation;
mod project_registry;
mod rename;
mod scrollback_rpc;
mod search_rpc;
mod wire;
