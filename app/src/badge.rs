//! The attention count, pushed to wherever this desktop shows counts.
//!
//! One number for the whole process, because that is what a dock icon or a
//! launcher entry is: there is one of them however many windows are open, and
//! it has to answer "how many of my agents want me" rather than "how many in
//! the workspace whichever window happens to be focused is looking at".
//!
//! Backends live in [`vitrum_os::badge`]. This is the only thing that calls
//! them, and it is deliberately dumb: connect once, send only on change, and
//! say nothing at all on a desktop that has no badge to set.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use vitrum_os::badge::{Badge, badge};

/// The platform backend, or `None` on a desktop with nowhere to put a count.
///
/// Connected once, on the first publish rather than at startup, so a desktop
/// with no launcher listening costs a single probe and never a retry. On Linux
/// that probe is a D-Bus name lookup: absent an owner for
/// `com.canonical.Unity` the backend refuses at connect rather than emitting
/// signals into the void.
static BACKEND: LazyLock<Option<Box<dyn Badge>>> = LazyLock::new(|| match badge(None) {
    Ok(backend) => Some(backend),
    Err(why) => {
        tracing::info!("no desktop badge on this session: {why}");
        None
    }
});

/// The last count sent, so an unchanged number costs nothing.
///
/// Seeded past any real count so the first publish always sends, including a
/// first publish of zero: a launcher that survived the last run may still be
/// showing a stale badge, and clearing it is the point.
static LAST: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Show `count` on the dock, taskbar or launcher entry.
///
/// Called after every server message, so both guards matter: the backend is
/// connected once, and a count equal to the one already displayed sends
/// nothing. A backend that refuses is reported once per changed count and
/// never disables anything: the badge is decoration, and losing it is not a
/// reason to interrupt the operator.
pub fn publish(count: usize) {
    let Some(backend) = BACKEND.as_ref() else {
        return;
    };
    if LAST.swap(count, Ordering::Relaxed) == count {
        return;
    }
    // Saturating rather than wrapping: a hypothetical count past `u32::MAX`
    // must read as "very many", never as a small number or zero, which is the
    // one value that means "hide it".
    if let Err(why) = backend.set_count(u32::try_from(count).unwrap_or(u32::MAX)) {
        tracing::warn!("the desktop badge refused a count of {count}: {why}");
    }
}

/// Take the badge down.
///
/// A launcher entry is driven by a signal, not by a property it can re-read,
/// so nothing tells it the process is gone. Without this the count the
/// operator last saw stays on the launcher after the last window closes, and
/// the next thing they learn from it is wrong.
pub fn clear() {
    let Some(backend) = BACKEND.as_ref() else {
        return;
    };
    LAST.store(0, Ordering::Relaxed);
    if let Err(why) = backend.clear() {
        tracing::warn!("the desktop badge could not be cleared: {why}");
    }
}
