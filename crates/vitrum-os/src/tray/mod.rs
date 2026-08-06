//! System tray icon carrying the attention count, with a menu.
//!
//! The menu model is a plain value produced by [`tray_menu`], and the icon is
//! rendered by [`crate::icon`]. Both are shared by all three backends, so the
//! Linux StatusNotifierItem, the macOS `NSStatusItem` and the Windows
//! `Shell_NotifyIcon` tray show the same items in the same order with the same
//! labels, and that fact is asserted by a test rather than by three
//! reimplementations that drift.
//!
//! The trait is deliberately **not** `Send + Sync`. On macOS an `NSStatusItem`
//! is main-thread only and on Windows the tray owns a message-only window whose
//! messages are pumped by the thread that created it. A `Send` bound would be a
//! lie held up by an `unsafe impl`; requiring the handle to stay on the UI
//! thread is the truth.

use std::sync::Arc;

use crate::branding::APP_DISPLAY_NAME;
use crate::capability::{Support, Unavailable};

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod desktop;
#[cfg(target_os = "linux")]
mod linux;

/// What a menu item does when clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrayCommand {
    /// Raise and focus the window, or hide it if it is already frontmost.
    ToggleWindow,
    /// Start a session, raising the window first.
    NewSession,
    /// Exit the application.
    Quit,
}

/// One row of the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuItem {
    /// Stable identifier, used by the backends to route clicks. Never
    /// localised.
    pub id: &'static str,
    pub label: String,
    /// `None` for a purely informational row.
    pub command: Option<TrayCommand>,
    pub enabled: bool,
}

/// A row or a divider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEntry {
    Item(TrayMenuItem),
    Separator,
}

/// Menu id for the show/hide row.
pub const ID_TOGGLE_WINDOW: &str = "toggle-window";
/// Menu id for the new session row.
pub const ID_NEW_SESSION: &str = "new-session";
/// Menu id for the disabled attention summary row.
pub const ID_ATTENTION: &str = "attention";
/// Menu id for the quit row.
pub const ID_QUIT: &str = "quit";

/// Build the menu for a given window visibility and attention count.
///
/// The attention row is present only when there is something to report, and is
/// disabled: it is a status line, not a button. A permanently visible
/// "0 sessions need attention" row would be noise in a menu this short.
pub fn tray_menu(window_visible: bool, count: u32) -> Vec<TrayEntry> {
    let mut entries = Vec::with_capacity(6);
    if count > 0 {
        entries.push(TrayEntry::Item(TrayMenuItem {
            id: ID_ATTENTION,
            label: attention_summary(count),
            command: None,
            enabled: false,
        }));
        entries.push(TrayEntry::Separator);
    }
    entries.push(TrayEntry::Item(TrayMenuItem {
        id: ID_TOGGLE_WINDOW,
        label: if window_visible { "Hide Window" } else { "Show Window" }.to_string(),
        command: Some(TrayCommand::ToggleWindow),
        enabled: true,
    }));
    entries.push(TrayEntry::Item(TrayMenuItem {
        id: ID_NEW_SESSION,
        label: "New Session".to_string(),
        command: Some(TrayCommand::NewSession),
        enabled: true,
    }));
    entries.push(TrayEntry::Separator);
    entries.push(TrayEntry::Item(TrayMenuItem {
        id: ID_QUIT,
        label: format!("Quit {APP_DISPLAY_NAME}"),
        command: Some(TrayCommand::Quit),
        enabled: true,
    }));
    entries
}

/// Which command a menu id maps to, or `None` for an inert row.
pub fn command_for_id(id: &str) -> Option<TrayCommand> {
    match id {
        ID_TOGGLE_WINDOW => Some(TrayCommand::ToggleWindow),
        ID_NEW_SESSION => Some(TrayCommand::NewSession),
        ID_QUIT => Some(TrayCommand::Quit),
        _ => None,
    }
}

/// "3 sessions need attention", correctly singular at one.
pub fn attention_summary(count: u32) -> String {
    match count {
        1 => "1 session needs attention".to_string(),
        n => format!("{n} sessions need attention"),
    }
}

/// Hover text for the tray icon.
pub fn tray_tooltip(count: u32) -> String {
    match count {
        0 => APP_DISPLAY_NAME.to_string(),
        n => format!("{APP_DISPLAY_NAME}: {}", attention_summary(n)),
    }
}

/// Icon edge length per platform, in logical pixels.
///
/// 22 is the freedesktop tray size and the macOS menu bar height; the Windows
/// notification area is 16.
pub const fn tray_icon_size() -> u32 {
    if cfg!(target_os = "windows") { 16 } else { 22 }
}

/// Called when the user picks a menu item or activates the icon.
pub type TrayCommandHandler = Arc<dyn Fn(TrayCommand) + Send + Sync>;

/// A live tray icon.
pub trait Tray {
    /// Whether the tray is actually showing.
    fn capability(&self) -> Support;

    /// Update the attention count: icon, tooltip and menu summary.
    fn set_count(&mut self, count: u32) -> Result<(), Unavailable>;

    /// Tell the tray whether the window is currently visible, so the toggle row
    /// reads correctly.
    fn set_window_visible(&mut self, visible: bool) -> Result<(), Unavailable>;

    /// Remove the icon.
    fn shutdown(&mut self);
}

/// Create the tray icon.
///
/// Must be called from the thread that owns the platform's UI event loop.
pub fn tray(handler: TrayCommandHandler) -> Result<Box<dyn Tray>, Unavailable> {
    #[cfg(target_os = "linux")]
    {
        linux::SniTrayHandle::start(handler).map(|t| Box::new(t) as Box<dyn Tray>)
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        desktop::DesktopTray::start(handler).map(|t| Box::new(t) as Box<dyn Tray>)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = handler;
        Err(Unavailable::not_implemented(format!(
            "no tray backend is compiled for {}",
            std::env::consts::OS
        )))
    }
}

/// Whether a tray icon would actually appear, without creating one.
pub fn probe() -> Support {
    #[cfg(target_os = "linux")]
    {
        linux::probe()
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        desktop::probe()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Support::Missing(Unavailable::not_implemented(format!(
            "no tray backend is compiled for {}",
            std::env::consts::OS
        )))
    }
}
