//! `NSApp.effectiveAppearance`, with the distributed notification for changes.
//!
//! `AppleInterfaceThemeChangedNotification` on the *distributed* notification
//! centre is what the system broadcasts when the user flips appearance, and it
//! is the reason this does not need a KVO observer on `effectiveAppearance` or
//! a timer. The block-based observer API is used rather than a custom delegate
//! class because there is no state to carry beyond the handler.
//!
//! Reading the appearance is main-thread only, like everything in AppKit, and a
//! call from a worker is reported rather than silently answered with a guess.

use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSString};

use crate::capability::{Support, Unavailable};
use crate::theme::{Theme, ThemeHandler, ThemeWatcher, deduplicate, theme_from_ns_appearance_name};

const CHANGE_NOTIFICATION: &str = "AppleInterfaceThemeChangedNotification";

pub(crate) struct AppKitThemeWatcher {
    handler: Arc<Mutex<Option<ThemeHandler>>>,
    /// Token returned by `addObserverForName:`; the observer is removed when
    /// this is released, so it is held for the watcher's lifetime.
    observer: Mutex<Option<Retained<AnyObject>>>,
}

// SAFETY: the observer token is an Objective-C object with an atomic reference
// count, and both fields are behind a `Mutex`.
unsafe impl Send for AppKitThemeWatcher {}
unsafe impl Sync for AppKitThemeWatcher {}

impl AppKitThemeWatcher {
    pub fn connect() -> Result<Self, Unavailable> {
        Ok(Self { handler: Arc::new(Mutex::new(None)), observer: Mutex::new(None) })
    }

    fn read(mtm: MainThreadMarker) -> Theme {
        let name = NSApplication::sharedApplication(mtm).effectiveAppearance().name();
        theme_from_ns_appearance_name(&name.to_string())
    }

    fn main_thread() -> Result<MainThreadMarker, Unavailable> {
        MainThreadMarker::new().ok_or_else(|| {
            Unavailable::runtime_error(
                "NSApp.effectiveAppearance is main-thread only; read the theme from the main \
                 thread",
            )
        })
    }
}

impl ThemeWatcher for AppKitThemeWatcher {
    fn capability(&self) -> Support {
        match Self::main_thread() {
            Ok(_) => Support::Available,
            Err(e) => Support::Missing(e),
        }
    }

    fn current(&self) -> Result<Theme, Unavailable> {
        Ok(Self::read(Self::main_thread()?))
    }

    fn preference(&self) -> Result<Option<Theme>, Unavailable> {
        // macOS always has a concrete appearance; there is no "no preference".
        self.current().map(Some)
    }

    fn subscribe(&self, handler: ThemeHandler) -> Result<(), Unavailable> {
        let handler = deduplicate(self.current().ok(), handler);
        *self.handler.lock().expect("handler slot is never held across a panic") = Some(handler);

        let mut slot = self.observer.lock().expect("observer is never held across a panic");
        if slot.is_some() {
            return Ok(());
        }

        let handler_slot = Arc::clone(&self.handler);
        let block = RcBlock::new(move |_notification: core::ptr::NonNull<NSNotification>| {
            // The distributed centre delivers on the main run loop, so a main
            // thread marker is available here.
            let Some(mtm) = MainThreadMarker::new() else { return };
            let theme = Self::read(mtm);
            let handler = handler_slot
                .lock()
                .expect("handler slot is never held across a panic")
                .clone();
            if let Some(handler) = handler {
                handler(theme);
            }
        });

        let center = NSDistributedNotificationCenter::defaultCenter();
        let name = NSString::from_str(CHANGE_NOTIFICATION);
        // SAFETY: the block outlives the observer because `RcBlock` is retained
        // by the notification centre for the observer's lifetime, and the token
        // is stored below.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(&name), None, None, &block)
        };
        // SAFETY: `addObserverForName:` returns an opaque retained observer
        // object; it is only ever stored and released.
        *slot = Some(unsafe { Retained::cast_unchecked::<AnyObject>(token) });
        Ok(())
    }
}
