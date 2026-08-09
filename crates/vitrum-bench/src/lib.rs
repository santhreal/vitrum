//! Load, concurrency, fuzz and profiling harness for the vitrum daemon.
//!
//! This is a client, not a test double. Every workload connects over the real
//! WebSocket transport and speaks the same [`vitrum_proto`] messages a window
//! does, so what it measures is the product rather than a rig standing in for
//! it. The daemon under test is an ordinary `vitrum-server`, started however the
//! operator wants and named by `--server`.
//!
//! Five workloads, one report format:
//!
//! - [`load`]: many sessions streaming at once. Cost and delivery.
//! - [`race`]: many connections mutating shared state. Correctness under
//!   concurrency, checked through invariants a single client cannot break.
//! - [`fuzz`]: hostile input, with a second healthy connection as the oracle.
//! - [`pipeline`]: the daemon's own output path, in process and with no socket
//!   in the middle. Where a megabyte's time and allocations go.
//! - [`profile`]: samples the daemon's process tree from `/proc` while any of
//!   the above runs.
//!
//! Every run writes `report.json` and `report.md` into its own directory, so a
//! result is a file someone can read later rather than terminal scrollback.

pub mod client;
pub mod fuzz;
pub mod load;
pub mod profile;
pub mod pipeline;
pub mod probe;
pub mod race;
mod rng;
pub mod report;
pub mod world;
pub mod stats;

#[cfg(test)]
mod tests;
