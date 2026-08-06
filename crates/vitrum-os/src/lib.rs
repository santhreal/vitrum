//! Desktop operating-system integration for vitrum.
//!
//! Eight things separate a window with a terminal in it from an application the
//! operating system treats as a citizen: notifications, a badge, a tray icon,
//! single-instance behaviour, theme following, window state that survives a
//! restart, a URL scheme, and directories in the places each platform expects.
//! This crate is all eight, on Linux, macOS and Windows.
//!
//! # The rule this crate is built around
//!
//! **A capability that is not available says so.** There is no silent no-op
//! anywhere in here. Every backend either does the thing or returns
//! [`Unavailable`] carrying a [`UnavailableKind`] and a sentence naming the
//! missing piece, and [`probe`] reports the lot in one pass. That distinction
//! is not pedantry: "macOS has no taskbar overlay" and "your Linux desktop has
//! no notification daemon running" call for completely different UI, and a
//! `bool` gives the caller neither.
//!
//! # Testability
//!
//! The parts that are easy to get wrong are pure functions of their inputs, so
//! they are tested for all three platforms from whichever one you happen to be
//! on: [`paths`] resolves against a captured environment rather than the live
//! one, [`window_state::clamp_to_monitors`] takes a monitor list, [`deeplink`]
//! parses a string, [`notify`] builds the D-Bus arguments, the toast XML and
//! the `UNNotificationRequest` fields as values, and [`icon`] rasterises
//! without a font stack. Only the last few lines of each backend touch the
//! operating system.
//!
//! # Idle cost
//!
//! Nothing here polls. The theme watcher parks a thread on a D-Bus signal, a
//! distributed notification, or `RegNotifyChangeKeyValue`; the notification
//! activation listener parks on a socket read; the single-instance listener
//! parks in `accept`. There is no timer anywhere in this crate.

#![deny(missing_docs)]

pub mod badge;
pub mod branding;
pub mod capability;
pub mod deeplink;
pub mod icon;
pub mod notify;
pub mod paths;
pub mod single_instance;
pub mod theme;
pub mod time;
pub mod tray;
pub mod window_state;

pub use badge::WindowHandle;
pub use capability::{CapabilityReport, Feature, Support, Unavailable, UnavailableKind};
pub use paths::{AppPaths, PathEnv, PathError, Platform};

#[cfg(test)]
mod tests;

/// Ask every backend whether it can work on this machine right now.
///
/// Cheap and side-effect free: it opens connections and reads settings, but it
/// never shows a notification, creates a tray icon, or takes the instance lock.
///
/// `window` is the main window handle, needed only for the Windows taskbar
/// overlay. Pass `None` before the window exists; the badge is then reported as
/// unavailable with that reason, which is accurate.
pub fn probe(window: Option<WindowHandle>) -> CapabilityReport {
    let paths = AppPaths::for_current_platform();

    let paths_support = match &paths {
        Ok(_) => Support::Available,
        Err(e) => Support::Missing(Unavailable::runtime_error(e.to_string())),
    };

    let window_state_support = match &paths {
        Ok(p) => match std::fs::create_dir_all(&p.state_dir) {
            Ok(()) => Support::Available,
            Err(e) => Support::Missing(Unavailable::permission_denied(format!(
                "cannot create the state directory {}: {e}",
                p.state_dir.display()
            ))),
        },
        Err(e) => Support::Missing(Unavailable::runtime_error(e.to_string())),
    };

    let single_instance_support = match &paths {
        Ok(p) => single_instance::probe(p),
        Err(e) => Support::Missing(Unavailable::runtime_error(e.to_string())),
    };

    CapabilityReport::new(vec![
        (
            Feature::Notifications,
            Support::from_result(notify::notifier()),
        ),
        (Feature::Badge, Support::from_result(badge::badge(window))),
        (Feature::Tray, tray::probe()),
        (Feature::SingleInstance, single_instance_support),
        (Feature::Theme, Support::from_result(theme::theme_watcher())),
        (Feature::WindowState, window_state_support),
        (Feature::DeepLinks, deeplink_support()),
        (Feature::Paths, paths_support),
    ])
}

/// Whether this build can register itself as the `vitrum://` handler.
fn deeplink_support() -> Support {
    #[cfg(target_os = "macos")]
    {
        Support::Missing(Unavailable::not_implemented(
            "macOS resolves URL schemes from CFBundleURLTypes in the app bundle's Info.plist at \
             install time; there is no runtime registration. Use \
             deeplink::plan_registration(Platform::MacOs, ..) to get the fragment and the \
             lsregister step.",
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Support::Available
    }
}
