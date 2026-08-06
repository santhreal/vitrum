//! StatusNotifierItem over D-Bus, via ksni.
//!
//! No GTK and no libappindicator: ksni speaks `org.kde.StatusNotifierItem` and
//! `com.canonical.dbusmenu` directly over the same zbus this crate already
//! uses. That matters because linking GTK 3 into a wgpu/webview application to
//! draw one 22-pixel icon is a build-time dependency on a toolkit the product
//! does not otherwise use.
//!
//! `org.kde.StatusNotifierWatcher` must be on the bus. KDE and the GNOME
//! AppIndicator extension provide it; a bare GNOME Shell or a plain window
//! manager does not, and that is reported rather than silently swallowed.

use std::sync::Arc;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::StandardItem;
use ksni::{Category, Icon, MenuItem, Status, ToolTip};

use crate::branding::{APP_DISPLAY_NAME, APP_NAME};
use crate::capability::{Support, Unavailable};
use crate::icon::render_tray_icon;
use crate::tray::{
    Tray, TrayCommand, TrayCommandHandler, TrayEntry, tray_icon_size, tray_menu, tray_tooltip,
};

struct SniTray {
    count: u32,
    window_visible: bool,
    handler: TrayCommandHandler,
}

impl SniTray {
    fn dispatch(&self, command: TrayCommand) {
        (self.handler)(command);
    }
}

impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        APP_NAME.to_string()
    }

    fn title(&self) -> String {
        APP_DISPLAY_NAME.to_string()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    /// `NeedsAttention` makes a compliant host emphasise the item, which is the
    /// whole point of the count.
    fn status(&self) -> Status {
        if self.count > 0 { Status::NeedsAttention } else { Status::Active }
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let size = tray_icon_size();
        let image = render_tray_icon(size, self.count);
        vec![Icon {
            width: image.width as i32,
            height: image.height as i32,
            data: image.to_argb_network(),
        }]
    }

    fn attention_icon_pixmap(&self) -> Vec<Icon> {
        self.icon_pixmap()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: String::new(),
            icon_pixmap: Vec::new(),
            title: tray_tooltip(self.count),
            description: String::new(),
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.dispatch(TrayCommand::ToggleWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        tray_menu(self.window_visible, self.count)
            .into_iter()
            .map(|entry| match entry {
                TrayEntry::Separator => MenuItem::Separator,
                TrayEntry::Item(item) => {
                    let command = item.command;
                    StandardItem {
                        label: item.label,
                        enabled: item.enabled,
                        activate: Box::new(move |tray: &mut Self| {
                            if let Some(command) = command {
                                tray.dispatch(command);
                            }
                        }),
                        ..Default::default()
                    }
                    .into()
                }
            })
            .collect()
    }
}

pub struct SniTrayHandle {
    handle: Handle<SniTray>,
}

impl SniTrayHandle {
    pub fn start(handler: TrayCommandHandler) -> Result<Self, Unavailable> {
        let tray = SniTray { count: 0, window_visible: true, handler: Arc::clone(&handler) };
        let handle = tray.spawn().map_err(map_ksni_error)?;
        Ok(Self { handle })
    }

    fn update<F: FnOnce(&mut SniTray)>(&self, f: F) -> Result<(), Unavailable> {
        self.handle.update(f).ok_or_else(|| {
            Unavailable::runtime_error("the tray service has shut down; the icon is gone")
        })
    }
}

/// Separate "nothing implements the tray spec here" from a real failure.
fn map_ksni_error(e: ksni::Error) -> Unavailable {
    let text = e.to_string();
    if text.contains("org.kde.StatusNotifierWatcher") || text.contains("ServiceUnknown") {
        return Unavailable::service_missing(format!(
            "no org.kde.StatusNotifierWatcher on the session bus: {text}. KDE Plasma provides \
             one; GNOME needs the AppIndicator extension; a bare window manager has none."
        ));
    }
    if matches!(e, ksni::Error::WontShow) {
        return Unavailable::service_missing(
            "a StatusNotifierWatcher is registered but reports that no host will display the \
             item, so the tray icon would never appear"
                .to_string(),
        );
    }
    Unavailable::runtime_error(format!("cannot register the tray item: {text}"))
}

impl Tray for SniTrayHandle {
    fn capability(&self) -> Support {
        if self.handle.is_closed() {
            return Support::Missing(Unavailable::service_missing(
                "the tray service has shut down",
            ));
        }
        Support::Available
    }

    fn set_count(&mut self, count: u32) -> Result<(), Unavailable> {
        self.update(move |tray| tray.count = count)
    }

    fn set_window_visible(&mut self, visible: bool) -> Result<(), Unavailable> {
        self.update(move |tray| tray.window_visible = visible)
    }

    fn shutdown(&mut self) {
        let _ = self.handle.shutdown();
    }
}

/// Whether a StatusNotifierWatcher is on the bus, without registering an item.
pub fn probe() -> Support {
    let conn = match zbus::blocking::Connection::session() {
        Ok(c) => c,
        Err(e) => {
            return Support::Missing(Unavailable::service_missing(format!(
                "no D-Bus session bus: {e}"
            )));
        }
    };
    let proxy = match zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    ) {
        Ok(p) => p,
        Err(e) => {
            return Support::Missing(Unavailable::runtime_error(format!(
                "cannot build bus proxy: {e}"
            )));
        }
    };
    match proxy.call::<_, _, bool>("NameHasOwner", &(WATCHER_NAME,)) {
        Ok(true) => Support::Available,
        Ok(false) => Support::Missing(Unavailable::service_missing(format!(
            "nothing owns {WATCHER_NAME} on this session bus, so a tray icon would never be \
             displayed. KDE Plasma provides one; GNOME needs the AppIndicator extension."
        ))),
        Err(e) => Support::Missing(Unavailable::runtime_error(format!(
            "NameHasOwner({WATCHER_NAME}) failed: {e}"
        ))),
    }
}

/// Well-known name a tray host must own for an item to be shown.
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
