//! The count of sessions wanting attention, shown where the OS puts counts.
//!
//! Three platforms, three unrelated mechanisms and one real gap:
//!
//! - **macOS**: `NSApp.dockTile.badgeLabel`, the actual system feature.
//! - **Windows**: `ITaskbarList3::SetOverlayIcon`, which is per-window and
//!   needs the main window's `HWND`, so the caller must supply one.
//! - **Linux**: there is no desktop-wide badge protocol. The Unity
//!   `com.canonical.Unity.LauncherEntry` signal is the closest thing and is
//!   honoured by Dash to Dock, Ubuntu Dock, Plank, KDE's task manager and
//!   Latte. On a desktop where nothing claims `com.canonical.Unity` the signal
//!   goes nowhere, so the backend probes for an owner and reports
//!   [`crate::capability::UnavailableKind::ServiceMissing`] instead of emitting
//!   into the void and returning `Ok`.
//!
//! The label logic is shared and pure so the "99+" and "9+" rules are asserted
//! once rather than reimplemented per backend.

use std::sync::atomic::{AtomicU64, Ordering};

// Only `unity_app_uri` needs this, and that is gated to the same platforms.
#[cfg(any(target_os = "linux", test))]
use crate::branding::DESKTOP_FILE_NAME;
use crate::capability::{Support, Unavailable};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Text for the macOS dock badge.
///
/// `None` clears it. Three digits do not fit a dock badge legibly, so anything
/// past 99 becomes `99+`, which is what Mail and Messages do.
pub fn dock_badge_label(count: u32) -> Option<String> {
    match count {
        0 => None,
        1..=99 => Some(count.to_string()),
        _ => Some("99+".to_string()),
    }
}

/// Accessible description for the Windows taskbar overlay icon.
///
/// Screen readers announce this, so it is a sentence, not a number.
pub fn overlay_description(count: u32) -> String {
    match count {
        0 => String::new(),
        1 => "1 session needs attention".to_string(),
        n => format!("{n} sessions need attention"),
    }
}

// The LauncherEntry protocol below is Linux-only: `badge/linux.rs` is its only
// caller, and a Windows or macOS build has no launcher that speaks it. `test`
// is in each cfg because the wire contract is asserted on every platform, and
// a D-Bus name that only gets checked on Linux is a name that drifts on the
// day someone edits it from a Mac.

/// A value in the Unity LauncherEntry property dictionary.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnityValue {
    Int64(i64),
    Bool(bool),
}

/// `application://<desktop file>`, the URI the LauncherEntry signal is keyed
/// by. A launcher matches it against the desktop file of the window it is
/// showing, so it must be the file name, not a path.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn unity_app_uri() -> String {
    format!("application://{DESKTOP_FILE_NAME}")
}

/// D-Bus object path the LauncherEntry signal is emitted from.
///
/// The protocol does not care about the path, only the interface and the app
/// URI, but it must be a valid, stable, app-specific path.
#[cfg(any(target_os = "linux", test))]
pub(crate) const UNITY_OBJECT_PATH: &str = "/com/canonical/Unity/LauncherEntry";
/// Interface carrying the `Update` signal.
#[cfg(any(target_os = "linux", test))]
pub(crate) const UNITY_INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
/// Well-known name whose presence means something is listening.
#[cfg(any(target_os = "linux", test))]
pub(crate) const UNITY_BUS_NAME: &str = "com.canonical.Unity";

/// Properties for the LauncherEntry `Update` signal.
///
/// `count-visible` is separate from `count` on purpose: a launcher that is only
/// sent `count = 0` keeps showing a "0" badge, so zero has to be expressed as
/// "hide it".
#[cfg(any(target_os = "linux", test))]
pub(crate) fn unity_properties(count: u32) -> Vec<(&'static str, UnityValue)> {
    vec![
        ("count", UnityValue::Int64(i64::from(count))),
        ("count-visible", UnityValue::Bool(count > 0)),
    ]
}

/// Opaque native window handle.
///
/// Only Windows needs one: its overlay icon belongs to a taskbar button, not to
/// the process. Carried as a `u64` so the type is identical on every platform
/// and the API does not fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowHandle(pub u64);

/// The main window's handle, once the window exists. Zero means unregistered.
static MAIN_WINDOW: AtomicU64 = AtomicU64::new(0);

/// Tell the badge layer which window owns the taskbar button.
///
/// The Windows overlay icon is set on a window, not on a process, but the
/// count itself is process-wide and is published from a place that has no
/// window in hand. Registering the handle once, from the window, is what lets
/// [`badge`] be called later with `None` and still find an HWND.
///
/// Idempotent, and safe to call before or after the first [`badge`] call: a
/// backend that already connected keeps the handle it connected with.
/// Ignored on every platform whose badge is process-wide.
pub fn register_main_window(handle: WindowHandle) {
    MAIN_WINDOW.store(handle.0, Ordering::Relaxed);
}

/// The registered main window, or `None` if no window has registered yet.
#[must_use]
pub(crate) fn main_window() -> Option<WindowHandle> {
    match MAIN_WINDOW.load(Ordering::Relaxed) {
        0 => None,
        raw => Some(WindowHandle(raw)),
    }
}

/// Which handle a badge connection should use.
///
/// An explicit argument wins over the registered window, so a second window
/// can badge its own taskbar button; otherwise the registered main window
/// fills in. Pure and two-argument so the precedence is testable without
/// mutating process-wide state.
#[must_use]
pub(crate) fn resolve_window(
    explicit: Option<WindowHandle>,
    registered: Option<WindowHandle>,
) -> Option<WindowHandle> {
    explicit.or(registered)
}

/// Setting the attention count where the OS shows counts.
pub trait Badge: Send + Sync {
    /// Whether the count can actually be displayed right now.
    fn capability(&self) -> Support;

    /// Show `count`, or hide the badge when `count` is zero.
    fn set_count(&self, count: u32) -> Result<(), Unavailable>;

    /// Hide the badge.
    fn clear(&self) -> Result<(), Unavailable> {
        self.set_count(0)
    }
}

/// Connect to this platform's badge mechanism.
///
/// `window` is required on Windows and ignored elsewhere. Passing `None` falls
/// back to the handle given to [`register_main_window`], which is how a caller
/// with no window in hand still reaches the taskbar overlay.
pub fn badge(window: Option<WindowHandle>) -> Result<Box<dyn Badge>, Unavailable> {
    let window = resolve_window(window, main_window());
    #[cfg(target_os = "linux")]
    {
        let _ = window;
        linux::UnityBadge::connect().map(|b| Box::new(b) as Box<dyn Badge>)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        macos::DockBadge::connect().map(|b| Box::new(b) as Box<dyn Badge>)
    }
    #[cfg(target_os = "windows")]
    {
        let window = window.ok_or_else(|| {
            Unavailable::runtime_error(
                "the taskbar count appears once the vitrum window is open; no window has \
                 registered yet",
            )
        })?;
        windows::TaskbarOverlayBadge::connect(window).map(|b| Box::new(b) as Box<dyn Badge>)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = window;
        Err(Unavailable::not_implemented(format!(
            "no badge backend is compiled for {}",
            std::env::consts::OS
        )))
    }
}
