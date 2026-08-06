//! The macOS dock badge.
//!
//! `NSDockTile` is AppKit, so it is main-thread only. That is not something to
//! paper over with a dispatch hop hidden inside a setter: a caller that sets
//! the badge from a worker thread has a bug, and reporting it is how they find
//! out. `MainThreadMarker::new()` returning `None` is the check.

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_foundation::NSString;

use crate::badge::{Badge, dock_badge_label};
use crate::capability::{Support, Unavailable};

pub struct DockBadge;

impl DockBadge {
    pub fn connect() -> Result<Self, Unavailable> {
        Ok(Self)
    }

    fn main_thread() -> Result<MainThreadMarker, Unavailable> {
        MainThreadMarker::new().ok_or_else(|| {
            Unavailable::runtime_error(
                "NSDockTile is main-thread only; call set_count from the main thread",
            )
        })
    }
}

impl Badge for DockBadge {
    fn capability(&self) -> Support {
        match Self::main_thread() {
            Ok(_) => Support::Available,
            Err(e) => Support::Missing(e),
        }
    }

    fn set_count(&self, count: u32) -> Result<(), Unavailable> {
        let mtm = Self::main_thread()?;
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        match dock_badge_label(count) {
            // A badge label of `nil` clears; an empty string draws an empty
            // red pill, which looks like a rendering bug.
            Some(text) => tile.setBadgeLabel(Some(&NSString::from_str(&text))),
            None => tile.setBadgeLabel(None),
        }
        Ok(())
    }
}
