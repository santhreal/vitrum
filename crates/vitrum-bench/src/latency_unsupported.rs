//! The latency harness on a platform that cannot run it.
//!
//! [`crate::latency`] measures a keystroke through a real pseudoterminal, a
//! `poll` on its descriptors and the process's own `getrusage`, and it paints
//! into a pane that exists on X11 only. None of that is Windows, and a harness
//! that reported numbers there would be reporting something other than the
//! product.
//!
//! So the module keeps its shape and refuses. The caller is one `bench latency`
//! invocation and one line of [`crate::world`]; both get a sentence naming what
//! is missing rather than a build that does not compile.

use anyhow::{Result, anyhow};

use crate::report::Report;
use crate::stats::Dist;

/// What a run would have been asked to do.
#[derive(Debug, Clone)]
pub struct LatencySpec {
    /// Samples per frame-level signal.
    pub samples: usize,
    /// Child spawns for the first-frame signal.
    pub spawns: usize,
    /// Extra panes built for the pane-memory signal.
    pub panes: usize,
    /// Sessions in the snapshot for the sidebar signal.
    pub sessions: usize,
    /// Quarter-second windows the paint-CPU signal averages over.
    pub cpu_windows: usize,
    /// Fail the process when a bound is crossed.
    pub gate: bool,
    /// Force the software rasteriser, for a machine with no usable GPU.
    pub software: bool,
}

impl Default for LatencySpec {
    fn default() -> Self {
        LatencySpec {
            samples: 2000,
            spawns: 5,
            panes: 8,
            sessions: 200,
            cpu_windows: 8,
            gate: false,
            software: false,
        }
    }
}

/// Why nothing can be measured here.
fn unsupported(what: &str) -> anyhow::Error {
    anyhow!(
        "{what} needs a pseudoterminal, poll(2) and an X11 pane, and this \
         platform has none of them. Run the latency harness on Linux."
    )
}

/// Refuses, because there is no pty to type into.
pub fn run(_spec: &LatencySpec) -> Result<Report> {
    Err(unsupported("the latency harness"))
}

/// Refuses, because there is no report to tabulate.
#[must_use]
pub fn table(_report: &Report) -> String {
    String::new()
}

/// Refuses, because the child paints a pane this platform cannot create.
pub fn child(_panes: usize, _software: bool) -> Result<()> {
    Err(unsupported("the first-frame child"))
}

/// Refuses, so [`crate::world`] reports a missing floor rather than a wrong one.
pub(crate) fn pty_echo(_samples: usize) -> Result<Dist> {
    Err(unsupported("the pty echo floor"))
}
