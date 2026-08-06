//! Every user-visible name the operating system sees, in one place.
//!
//! The crate is called `vitrum` and the product is called `vitrum`. The OS does
//! not care what the crate is called; it cares about the URL scheme, the
//! desktop file name, the bundle identifier and the directory names, and those
//! are all product identity. Renaming the product means editing this file and
//! nothing else, which is the only reason these are constants rather than
//! string literals scattered through eight backends.

/// Lowercase machine identity: XDG directory name, desktop file stem, icon
/// theme name, and the URL scheme.
pub(crate) const APP_NAME: &str = "vitrum";

/// Human-facing name: notification app name, tray title, window class.
pub(crate) const APP_DISPLAY_NAME: &str = "Vitrum";

/// The wordmark drawn inside the window, always lowercase.
///
/// Separate from [`APP_DISPLAY_NAME`], which is what the OS shows in a
/// launcher, a notification and a window class and is therefore titlecased by
/// convention. Inside the window the product sets its own convention, and
/// every piece of prose this project ships writes it lowercase.
pub const APP_WORDMARK: &str = "vitrum";

/// One-line description used in the generated desktop entry.
pub const APP_COMMENT: &str = "Terminal shell for coding agents";

/// Vendor segment for the Windows and macOS directory conventions, both of
/// which nest application data under an organisation.
pub(crate) const ORG_NAME: &str = "santhreal";

/// Reverse-DNS identity. macOS uses it for `Application Support`, the bundle
/// identifier and the notification thread; Windows uses it as the
/// AppUserModelID a toast must be sent under.
pub(crate) const BUNDLE_ID: &str = "dev.santhreal.vitrum";

/// URL scheme handled by [`crate::deeplink`]. Registered per OS.
pub const URL_SCHEME: &str = "vitrum";

/// File name of the freedesktop desktop entry. The notification `desktop-entry`
/// hint and the Unity launcher URI both key off this stem.
pub(crate) const DESKTOP_FILE_NAME: &str = "vitrum.desktop";

/// Icon theme name requested from the notification daemon and the tray.
pub(crate) const ICON_NAME: &str = "vitrum";
