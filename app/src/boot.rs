//! The startup timeline, and the order it is required to happen in.
//!
//! # Why the timings are a trace and the order is a contract
//!
//! What a start COSTS is a property of the machine: the disk the profile came
//! off, whether the toolkit's caches are warm, what else is compiling at the
//! time. A test that asserted "under 250 ms" would pass on an idle desktop,
//! fail on a loaded builder, and teach everyone to rerun it until it went
//! green, which is the same as deleting it. So the numbers are emitted, never
//! asserted, and `VITRUM_BOOT_TRACE=1` is what turns them on.
//!
//! What a start does IN WHAT ORDER is a property of this program, and it does
//! not move under load. [`PHASES`] states it: every phase names the phase that
//! must already have happened. Two of those rows are the whole reason this
//! module is not just a pair of `eprintln!`s.
//!
//! - `styles.built` before `window.created`. The stylesheet bundle is
//!   assembled on the prewarm thread while the toolkit comes up, and the
//!   window is built from the finished string. Reversing them puts a
//!   several-hundred-kilobyte string build between the window appearing and
//!   its first frame, which is time the operator spends looking at an empty
//!   rectangle.
//! - `window.created` before `shell.mounted`. The pane is installed with the
//!   OS window, so it is parsing and holding a grid for the whole interval the
//!   shell is still being built. A pane installed at mount instead would start
//!   that clock hundreds of milliseconds later and drop the bytes that arrived
//!   in between.
//! - `shell.mounted` before `pane.first-paint`. The pane's GPU handshake is
//!   not allowed in front of the window's first frame. Reversing them is what
//!   made a start take four times as long as the window needed.
//!
//! A mark that arrives before its prerequisite is recorded as a violation
//! rather than a panic. A launch is not improved by refusing to start; the
//! run says what went wrong, [`violations`] hands the list to the trace, and
//! [`out_of_order`] is what the suite checks so the regression is caught
//! before a build ships with it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Every phase of a start, in the order it must happen, with the phase that
/// must precede it.
///
/// One list. The trace reads it, the ordering guard reads it, and
/// [`mark`] rejects a name that is not in it, so a phase cannot be traced
/// without also being placed. A new phase added with the wrong prerequisite,
/// or none, turns the suite red rather than quietly widening what a start is
/// allowed to do.
pub(crate) const PHASES: [(&str, Option<&str>); 11] = [
    // The first line of `main`, before the logging subscriber is built: the
    // subscriber parses a filter and constructs a writer, and on a cold start
    // that is not free.
    ("process.start", None),
    ("logging.ready", Some("process.start")),
    // The single-instance slot. A second launch never reaches any later
    // phase; it hands its intent over and exits.
    ("instance.claimed", Some("logging.ready")),
    // The document, assembled once for the process.
    ("styles.built", Some("instance.claimed")),
    // The OS window exists and the pane is installed on it.
    ("window.created", Some("styles.built")),
    // The window's widget tree is realized: the toplevel, the paned, the
    // sidebar, the titlebar, the bar, and the container the pane presents in.
    // This is what replaced a document being mounted, and it is the mark the
    // startup claim is measured against.
    ("frame.realized", Some("window.created")),
    // The shell's panels are mounted and have seen the state once.
    ("shell.mounted", Some("frame.realized")),
    // The profile has been folded into this window's state.
    ("settings.restored", Some("shell.mounted")),
    // The daemon has been asked for a connection.
    ("daemon.dialled", Some("settings.restored")),
    // The pane put its first frame on screen. After the shell is mounted, not
    // merely after the window exists: building a swapchain means an instance,
    // an adapter, a device, a surface configuration and a shader pipeline, and
    // done inside the realize handler all of that lands between `show_all` and
    // the window's first frame. It is built on a worker thread instead and
    // adopted from the main loop, which cannot run until `open_on` has
    // returned, so this mark cannot precede `shell.mounted` without the
    // handshake having moved back onto the toolkit's thread.
    ("pane.first-paint", Some("shell.mounted")),
    // History for the focused session reached the pane.
    ("scrollback.restored", Some("daemon.dialled")),
];

/// Whether the trace is on. Read once, while `main` is still the only thread.
static ON: AtomicBool = AtomicBool::new(false);

/// Phases recorded so far, in arrival order.
static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Everything that arrived in the wrong order, or under a name no phase has.
static VIOLATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// When the process started, for anything that needs an elapsed time rather
/// than a wall clock.
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);

/// Read the trace switch, once.
///
/// Reading it per mark would be a `getenv` on the startup path, which is the
/// thing being measured, and would race the prewarm thread against any later
/// environment write.
pub(crate) fn arm() {
    ON.store(
        std::env::var_os("VITRUM_BOOT_TRACE").is_some(),
        Ordering::Relaxed,
    );
    if let Ok(mut started) = STARTED.lock() {
        started.get_or_insert_with(Instant::now);
    }
}

/// How long this process has been running.
///
/// `None` before [`arm`], which is only the case inside a test binary that
/// never called it. A caller that needs a duration in that case has nothing
/// to measure from and must say so rather than invent a zero.
pub(crate) fn since_start() -> Option<std::time::Duration> {
    STARTED.lock().ok()?.map(|at| at.elapsed())
}

/// Record that the process reached `phase`.
///
/// Absolute microseconds since the epoch rather than a delta, so a harness
/// that stamped the clock before `exec` can subtract its own zero and see the
/// dynamic-link and toolkit-init cost this process cannot observe from inside
/// itself.
///
/// Idempotent per phase: the first arrival is the one that counts, because
/// the phases that can happen more than once are the ones whose FIRST time is
/// the measurement. The pane presents a frame many times a second and only
/// the first is `pane.first-paint`.
pub(crate) fn mark(phase: &str) {
    let Some(&(name, _)) = PHASES.iter().find(|(name, _)| *name == phase) else {
        note(format!(
            "boot phase {phase:?} is traced and is not in PHASES, so nothing \
             states when it is allowed to happen"
        ));
        return;
    };
    {
        let Ok(mut seen) = SEEN.lock() else { return };
        if seen.contains(&name) {
            return;
        }
        seen.push(name);
        for problem in out_of_order(&seen) {
            note(problem);
        }
    }
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros());
    eprintln!("vitrum-boot {name} {us}");
}

/// One stretch of a start, named for the work inside it.
///
/// A phase says WHEN something happened and is ordered against its
/// prerequisite. A span says WHAT A STRETCH COST, and the two answer different
/// questions: a timeline of eleven phases can show 264 ms between two of them
/// without naming a single thing that spent it, and a mark added to close that
/// gap only moves the gap. So the gap is measured directly, by whoever does
/// the work, and the attribution is the name they gave it.
///
/// Spans are not ordered and not idempotent. Nesting is allowed and expected:
/// an outer span is the sum plus whatever its children did not claim, which
/// is how an unattributed remainder becomes visible instead of invisible.
///
/// Off when the trace is off, down to not reading the clock. A start that
/// nobody is measuring pays two atomic loads for the whole facility.
pub(crate) struct Span {
    name: &'static str,
    at: Option<Instant>,
}

/// Begin measuring `name`.
///
/// The stretch ends when the returned value is dropped, so a span covers
/// exactly the scope it was opened in and cannot be left open by an early
/// return.
pub(crate) fn span(name: &'static str) -> Span {
    Span {
        name,
        at: ON.load(Ordering::Relaxed).then(Instant::now),
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(at) = self.at {
            eprintln!(
                "vitrum-boot-span {} {}",
                self.name,
                at.elapsed().as_micros()
            );
        }
    }
}

/// Every ordering rule `seen` breaks, as sentences.
///
/// Pure, and the same function the live recorder uses, so a test drives the
/// real rule rather than a copy of it. Only the LAST phase in `seen` is
/// judged: the recorder calls this after each arrival, so an earlier
/// violation has already been reported and repeating it would fill the trace
/// with one problem written once per later phase.
pub(crate) fn out_of_order(seen: &[&str]) -> Vec<String> {
    let Some(last) = seen.last() else {
        return Vec::new();
    };
    let Some((_, needs)) = PHASES.iter().find(|(name, _)| name == last) else {
        return vec![format!("{last} is not a phase")];
    };
    let Some(needs) = needs else {
        return Vec::new();
    };
    if seen[..seen.len() - 1].contains(needs) {
        return Vec::new();
    }
    vec![format!(
        "{last} happened before {needs}. The start does its work in the order \
         PHASES states, and this one did not."
    )]
}

/// Everything this run got wrong about its own order.
pub(crate) fn violations() -> Vec<String> {
    VIOLATIONS.lock().map(|v| v.clone()).unwrap_or_default()
}

/// Record a problem, and say it once whether or not the trace is on.
///
/// An ordering fault is a defect in this program rather than a measurement,
/// so it is not gated behind the trace switch: a build that reversed two
/// phases would otherwise ship silently to everyone who never set the
/// variable.
fn note(problem: String) {
    tracing::warn!("{problem}");
    if let Ok(mut violations) = VIOLATIONS.lock() {
        violations.push(problem);
    }
}

/// Record a counter's standing at the end of a run.
///
/// The counters are what the suite asserts on, and this is what makes the
/// same numbers readable from a real session on a real machine: a trace that
/// ends with one profile read and one mark rasterised is the claim, checked
/// where it is made rather than only in a unit test.
pub(crate) fn tally(name: &str, n: usize) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    eprintln!("vitrum-boot count.{name} {n}");
    for problem in violations() {
        eprintln!("vitrum-boot violation {problem}");
    }
}
