//! OS light/dark preference, with a change subscription that never polls.
//!
//! Every platform has a push notification for this and none of them needs a
//! timer: a D-Bus signal on Linux, a distributed notification on macOS, and
//! `RegNotifyChangeKeyValue` on Windows, which blocks a thread in the kernel
//! until the key actually changes. A one-second poll would cost 86,400 wakeups
//! a day to observe an event that happens twice.
//!
//! The three raw encodings are decoded by pure functions so their edge cases,
//! particularly the portal's "no preference", are pinned by tests rather than
//! rediscovered on each platform.

use std::sync::Arc;

use crate::capability::{Support, Unavailable};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// The two appearances an application has to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Theme {
    Light,
    Dark,
}

impl Theme {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

impl core::fmt::Display for Theme {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // `pad`, not `write_str`: a report column uses `{:<16}` and
        // `write_str` silently discards the width.
        f.pad(self.as_str())
    }
}

/// What to use when the desktop explicitly says it has no preference.
///
/// A terminal-hosting shell is a dark-first product, and a user who has not
/// chosen is better served by the appearance the app was designed in than by
/// a white terminal.
pub const NO_PREFERENCE_THEME: Theme = Theme::Dark;

/// Decode the freedesktop portal's `org.freedesktop.appearance` `color-scheme`.
///
/// The spec defines exactly three values: `0` no preference, `1` prefer dark,
/// `2` prefer light. `None` means "the user has not chosen", which is
/// deliberately distinct from light: a caller that wants a concrete answer
/// applies [`NO_PREFERENCE_THEME`], and a caller offering a
/// "follow the system" setting can grey it out instead.
///
/// Values outside 0..=2 are treated as no preference, because the portal is
/// versioned and a future value must not be guessed at.
pub fn theme_from_portal_color_scheme(value: u32) -> Option<Theme> {
    match value {
        1 => Some(Theme::Dark),
        2 => Some(Theme::Light),
        _ => None,
    }
}

/// Decode `HKCU\...\Themes\Personalize\AppsUseLightTheme`.
///
/// A `DWORD`: zero means dark, anything else light. Note the polarity, which is
/// the opposite of what the name suggests to a reader in a hurry, and note that
/// there is a sibling `SystemUsesLightTheme` for the taskbar; the *apps* value
/// is the one that governs an application window.
pub fn theme_from_apps_use_light_theme(value: u32) -> Theme {
    if value == 0 { Theme::Dark } else { Theme::Light }
}

/// Decode an `NSAppearance` name.
///
/// The names are `NSAppearanceNameAqua`, `NSAppearanceNameDarkAqua` and the
/// high-contrast and vibrant variants, all of which spell dark as `Dark`.
/// Substring matching rather than an exact list because Apple has added
/// variants (`NSAppearanceNameAccessibilityHighContrastDarkAqua`) and an exact
/// list would silently report light for a user in high-contrast dark mode.
pub fn theme_from_ns_appearance_name(name: &str) -> Theme {
    if name.contains("Dark") { Theme::Dark } else { Theme::Light }
}

/// Called on the platform's notification thread when the theme changes.
pub type ThemeHandler = Arc<dyn Fn(Theme) + Send + Sync>;

/// Wrap a handler so it fires only when the resolved theme actually changed.
///
/// Every platform delivers duplicates, for different reasons, and all three
/// would otherwise make the application rebuild its stylesheet several times
/// per user action:
///
/// - **Linux**: a desktop with more than one portal backend registered (GNOME
///   ships both `xdg-desktop-portal-gnome` and `-gtk`) relays each
///   `SettingChanged` once per backend. Measured on GNOME 46: two signals per
///   appearance change.
/// - **Windows**: `RegNotifyChangeKeyValue` wakes on any change under
///   `Personalize`, including `SystemUsesLightTheme` and `ColorPrevalence`,
///   neither of which changes the app appearance.
/// - **macOS**: distributed notifications are best-effort and may be delivered
///   more than once.
pub fn deduplicate(initial: Option<Theme>, handler: ThemeHandler) -> ThemeHandler {
    use core::sync::atomic::{AtomicU8, Ordering};

    const UNKNOWN: u8 = 0;
    const LIGHT: u8 = 1;
    const DARK: u8 = 2;

    fn code(theme: Option<Theme>) -> u8 {
        match theme {
            None => UNKNOWN,
            Some(Theme::Light) => LIGHT,
            Some(Theme::Dark) => DARK,
        }
    }

    let last = Arc::new(AtomicU8::new(code(initial)));
    Arc::new(move |theme| {
        if last.swap(code(Some(theme)), Ordering::SeqCst) != code(Some(theme)) {
            handler(theme);
        }
    })
}

/// Reads the OS appearance and reports changes.
pub trait ThemeWatcher: Send + Sync {
    /// Whether the appearance can be read right now.
    fn capability(&self) -> Support;

    /// The current appearance, resolving "no preference" through
    /// [`NO_PREFERENCE_THEME`].
    fn current(&self) -> Result<Theme, Unavailable>;

    /// The raw preference, where `None` means the desktop reported none.
    ///
    /// Only Linux can report this; the other two platforms always have a
    /// concrete value, so they return `Some`.
    fn preference(&self) -> Result<Option<Theme>, Unavailable>;

    /// Install a change handler. Replaces any previous one. Starts the
    /// platform's notification mechanism on first call.
    fn subscribe(&self, handler: ThemeHandler) -> Result<(), Unavailable>;
}

/// Connect to this platform's appearance setting.
pub fn theme_watcher() -> Result<Box<dyn ThemeWatcher>, Unavailable> {
    #[cfg(target_os = "linux")]
    {
        linux::PortalThemeWatcher::connect().map(|w| Box::new(w) as Box<dyn ThemeWatcher>)
    }
    #[cfg(target_os = "macos")]
    {
        macos::AppKitThemeWatcher::connect().map(|w| Box::new(w) as Box<dyn ThemeWatcher>)
    }
    #[cfg(target_os = "windows")]
    {
        windows::RegistryThemeWatcher::connect().map(|w| Box::new(w) as Box<dyn ThemeWatcher>)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err(Unavailable::not_implemented(format!(
            "no theme backend is compiled for {}",
            std::env::consts::OS
        )))
    }
}
