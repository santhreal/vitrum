//! Test suites for vitrum-server, one concern per module.

mod attach_stream;
mod authentication;
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
mod naming;
#[cfg(not(windows))]
mod observation;
mod project_registry;
mod rename;
mod scrollback_rpc;
#[cfg(not(windows))]
mod seam_status;
#[cfg(not(windows))]
mod seam_stream;
#[cfg(not(windows))]
mod seam_title;
mod search_rpc;
mod wire;
