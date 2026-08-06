//! The system tray icon, and the commands the operator issues from it.
//!
//! The window is not always on screen, and a session that wants an answer is
//! worth knowing about when it is not. The tray is where that lives: an icon
//! that changes when something is pending, a tooltip that says how much, and a
//! menu that can raise the window, start a session, or quit without one.
//!
//! Backends live in [`vitrum_os::tray`]. This is the only thing in the app that
//! calls them. Two rules shape the shape of it.
//!
//! The handle is not `Send`: a macOS `NSStatusItem` is main-thread only and the
//! Windows tray owns a message-only window pumped by its creating thread, so it
//! is created on the thread that owns the event loop and stays there. That is
//! why this is a value the caller holds rather than a process-wide static like
//! [`crate::badge`].
//!
//! Clicks arrive on a thread the app does not own, so they are posted to
//! [`COMMANDS`] rather than acted on. That is the same handoff a second launch
//! and a notification click already use, and it is what makes a foreign thread
//! safe here.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use vitrum_os::tray::{Tray, TrayCommand};

use crate::instance::Mailbox;

/// Menu picks and icon clicks, written by the tray backend's own thread and
/// read by whichever window task gets there first.
pub(crate) static COMMANDS: Mailbox<TrayCommand> = Mailbox::new();

/// Wait for the next thing the operator asked of the tray.
pub(crate) async fn next_command() -> TrayCommand {
    COMMANDS.next().await
}

/// A live tray icon, cheap to clone, valid only on the thread that made it.
#[derive(Clone)]
pub(crate) struct Handle {
    inner: Rc<RefCell<Live>>,
}

struct Live {
    tray: Box<dyn Tray>,
    /// Last count pushed, so an unchanged number rebuilds nothing. On Linux a
    /// push re-emits the whole dbusmenu, and this is called after every server
    /// message.
    count: u32,
    visible: bool,
}

impl Handle {
    /// Report how many sessions want the operator's attention.
    ///
    /// Saturating rather than wrapping: a count past `u32::MAX` must read as
    /// "very many", never as zero, which is the one value meaning "nothing is
    /// pending".
    pub(crate) fn set_attention(&self, count: usize) {
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        let mut live = self.inner.borrow_mut();
        if live.count == count {
            return;
        }
        live.count = count;
        if let Err(why) = live.tray.set_count(count) {
            tracing::warn!("the tray refused a count of {count}: {why}");
        }
    }

    /// Tell the tray whether the window is on screen, so the toggle row says
    /// what clicking it will do.
    pub(crate) fn set_window_visible(&self, visible: bool) {
        let mut live = self.inner.borrow_mut();
        if live.visible == visible {
            return;
        }
        live.visible = visible;
        if let Err(why) = live.tray.set_window_visible(visible) {
            tracing::warn!("the tray refused a visibility of {visible}: {why}");
        }
    }

    /// Take the icon down.
    ///
    /// Worth doing explicitly on quit: a StatusNotifierItem host keeps showing
    /// an item until its bus name goes away, and on Windows the notification
    /// area keeps a dead icon until the user hovers it.
    pub(crate) fn shutdown(&self) {
        self.inner.borrow_mut().tray.shutdown();
    }
}

/// Put the icon in the tray, or say why this desktop has nowhere to put it.
///
/// Must be called from the thread that owns the event loop. `None` is always
/// accompanied by a logged reason: a bare GNOME Shell has no
/// `StatusNotifierWatcher`, and that is a fact about the session worth having
/// in the log rather than a silent absence.
pub(crate) fn install() -> Option<Handle> {
    match vitrum_os::tray::tray(Arc::new(|command| COMMANDS.post(command))) {
        Ok(tray) => {
            Some(Handle { inner: Rc::new(RefCell::new(Live { tray, count: 0, visible: true })) })
        }
        Err(why) => {
            tracing::info!("no tray icon on this session: {why}");
            None
        }
    }
}
