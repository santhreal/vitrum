//! Tests for the OS integration layer.
//!
//! The split is deliberate. Everything that can be a pure function of its
//! inputs is one, and is tested here for all three platforms regardless of
//! which platform the test run is on: directory resolution, deep-link parsing,
//! window clamping, notification payloads for D-Bus, WinRT and
//! UserNotifications, icon rasterisation, and the tray menu model.
//!
//! What genuinely needs a live desktop service lives in [`live_linux`], which
//! asserts against the real session bus on Linux and asserts the *reported
//! reason* when a service is absent. Those tests never skip: the "service
//! missing" branch is a real assertion about a real code path, not a shrug.

mod badge_label;
mod capability;
mod deeplink;
mod icon;
mod mark;
mod notify_payload;
mod paths;
mod registration;
mod single_instance;
mod support;
mod theme_decode;
mod timezone;
mod tray_menu;
mod window_state;

#[cfg(target_os = "linux")]
mod live_linux;
