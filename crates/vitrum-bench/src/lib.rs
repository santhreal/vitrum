//! Load, concurrency, fuzz and profiling harness for the vitrum daemon.
//!
//! This is a client, not a test double. Every workload connects over the real
//! WebSocket transport and speaks the same [`vitrum_proto`] messages a window
//! does, so what it measures is the product rather than a rig standing in for
//! it. The daemon under test is an ordinary `vitrum-server`, started however the
//! operator wants and named by `--server`.
//!
//! Six workloads, one report format:
//!
//! - [`load`]: many sessions streaming at once. Cost and delivery.
//! - [`race`]: many connections mutating shared state. Correctness under
//!   concurrency, checked through invariants a single client cannot break.
//! - [`fuzz`]: hostile input, with a second healthy connection as the oracle.
//! - [`pipeline`]: the daemon's own output path, in process and with no socket
//!   in the middle. Where a megabyte's time and allocations go.
//! - [`profile`]: samples the daemon's process tree from `/proc` while any of
//!   the above runs.
//! - [`latency`]: the pane, not the daemon. The interval between a cause and
//!   the pixels that answer it, ending on the GPU's fence.
//!
//! Every run writes `report.json` and `report.md` into its own directory, so a
//! result is a file someone can read later rather than terminal scrollback.
//!
//! # Features
//!
//! `daemon` is on by default and brings in [`vitrum_core`], which [`pipeline`]
//! drives directly. [`latency`] needs a GPU and no daemon at all, so
//! `--no-default-features` builds a harness that measures the pane on a host
//! where the daemon's crate is not wanted.

pub mod client;
pub mod divergence;
pub mod frame;
pub mod fuzz;
pub mod latency;
pub mod load;
#[cfg(feature = "daemon")]
pub mod pipeline;
pub mod probe;
pub mod profile;
pub mod race;
pub mod report;
mod rng;
pub mod stats;
pub mod world;

#[cfg(test)]
mod tests;
