//! Test suites for vitrum-core, one concern per module.

mod agent_title;
mod attention;
mod close_tree;
mod git_branch;
mod helpers;
#[cfg(not(windows))]
mod hint_session;
mod manager_registry;
mod osc_bound;
mod osc_capture;
mod output_path_cost;
mod output_scan;
mod pty_burst_exit;
mod pty_capacity;
mod pty_detached;
mod pty_escapes;
mod pty_exit;
mod pty_geometry;
mod pty_input;
mod pty_output;
mod pty_resize;
mod rename;
mod scrollback_capacity;
mod scrollback_range;
mod scrollback_seq;
mod title;
mod waiting_probe;
