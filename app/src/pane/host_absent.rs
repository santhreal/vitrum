//! The pane on a platform that has no window to present to.
//!
//! The pane is a wgpu swapchain on an X window belonging to a GTK drawing
//! area, reached through `gdk_x11_window_get_xid`. There is no such window on
//! macOS or Windows, so this module carries the same shapes as [`super::host`]
//! and refuses instead of painting: the window opens with its sidebar, its bar
//! and its settings, and the rectangle the pane would own says why it is
//! empty.
//!
//! Nothing here is reachable at run time past the refusal. [`PaneHost`] has no
//! variant, so a value of it cannot exist, and every method on it is written
//! against that fact rather than returning a plausible answer.

use anyhow::{Result, anyhow};

use super::pacing::FrameStats;
use super::{InputSink, ReportSink};
use crate::WindowId;

/// Install a pane in `parent`.
///
/// # Errors
///
/// Always, naming the surface this platform does not have.
pub(crate) fn install_in(
    _parent: &gtk::Box,
    _ordinal: WindowId,
    _input: InputSink,
) -> Result<PaneHost> {
    Err(anyhow!(
        "the terminal pane presents to an X11 window, and this platform has none"
    ))
}

/// A pane that was installed in a window.
///
/// Uninhabited here, because [`install_in`] never returns one.
pub(crate) enum PaneHost {}

impl PaneHost {
    /// The pane in `window`, if it has one. Never one here.
    pub(crate) fn for_window(_window: WindowId) -> Option<Self> {
        None
    }

    /// Forget the pane in `window`. There is none to forget.
    pub(crate) fn forget(_window: WindowId) {}

    /// Where the daemon's bytes go.
    pub(crate) fn sink(&self) -> std::rc::Rc<std::cell::RefCell<dyn crate::socket::PaneSink>> {
        match *self {}
    }

    /// Where the pane sends what only the shell can act on.
    pub(crate) fn on_report(&self, _report: ReportSink) {
        match *self {}
    }

    /// Dim the pane while a sheet is over it.
    pub(crate) fn set_dimmed(&self, _dimmed: bool) {
        match *self {}
    }

    /// What the frame clock has been doing.
    pub(crate) fn frame_summary(&self) -> FrameStats {
        match *self {}
    }
}
