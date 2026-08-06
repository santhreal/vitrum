//! macOS `NSStatusItem` and Windows `Shell_NotifyIcon`, both through
//! `tray-icon`.
//!
//! One backend for two platforms because the two are genuinely the same shape:
//! an icon owned by the UI thread, a native popup menu, and a click event
//! delivered on the platform's run loop. `tray-icon` is used with default
//! features off, so neither the GTK nor the libxdo path that its Linux support
//! needs is compiled; Linux goes through [`super::linux`] instead.
//!
//! The menu must be rebuilt rather than mutated when the toggle label flips,
//! because a native menu's item text is not a reactive binding on either
//! platform.

use std::sync::Arc;

use tray_icon::menu::{Menu, MenuEvent, MenuItem as NativeMenuItem, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use crate::branding::APP_NAME;
use crate::capability::{Support, Unavailable};
use crate::icon::render_tray_icon;
use crate::tray::{
    Tray, TrayCommand, TrayCommandHandler, TrayEntry, command_for_id, tray_icon_size, tray_menu,
    tray_tooltip,
};

pub struct DesktopTray {
    icon: Option<TrayIcon>,
    handler: TrayCommandHandler,
    count: u32,
    window_visible: bool,
}

impl DesktopTray {
    pub fn start(handler: TrayCommandHandler) -> Result<Self, Unavailable> {
        let mut tray = Self { icon: None, handler: Arc::clone(&handler), count: 0, window_visible: true };

        // One process-wide handler; menu ids are the routing key, which is why
        // they are stable constants rather than generated.
        let routed = Arc::clone(&handler);
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(command) = command_for_id(event.id.as_ref()) {
                routed(command);
            }
        }));

        // A left click on the icon raises the window, which is what every
        // tray-resident app on both platforms does. The menu is the right
        // click.
        let clicked = Arc::clone(&tray.handler);
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                clicked(TrayCommand::ToggleWindow);
            }
        }));

        let menu = tray.build_menu()?;
        let icon = TrayIconBuilder::new()
            .with_id(APP_NAME)
            .with_menu(Box::new(menu))
            .with_tooltip(tray_tooltip(0))
            .with_icon(tray.render_icon()?)
            // The rendered icon is coloured, not a monochrome template, so
            // macOS must not recolour it for the menu bar appearance.
            .with_icon_as_template(false)
            .build()
            .map_err(|e| {
                Unavailable::service_missing(format!(
                    "cannot create the tray icon: {e}. On Windows this needs a running Explorer \
                     shell; on macOS it needs an initialised NSApplication on the main thread."
                ))
            })?;
        tray.icon = Some(icon);
        Ok(tray)
    }

    fn render_icon(&self) -> Result<Icon, Unavailable> {
        let image = render_tray_icon(tray_icon_size(), self.count);
        Icon::from_rgba(image.rgba, image.width, image.height)
            .map_err(|e| Unavailable::runtime_error(format!("cannot build the tray icon: {e}")))
    }

    fn build_menu(&self) -> Result<Menu, Unavailable> {
        let menu = Menu::new();
        for entry in tray_menu(self.window_visible, self.count) {
            let appended = match entry {
                TrayEntry::Separator => menu.append(&PredefinedMenuItem::separator()),
                TrayEntry::Item(item) => menu.append(&NativeMenuItem::with_id(
                    item.id,
                    &item.label,
                    item.enabled,
                    None,
                )),
            };
            appended.map_err(|e| {
                Unavailable::runtime_error(format!("cannot build the tray menu: {e}"))
            })?;
        }
        Ok(menu)
    }

    fn refresh(&mut self) -> Result<(), Unavailable> {
        let icon = self.render_icon()?;
        let menu = self.build_menu()?;
        let tooltip = tray_tooltip(self.count);
        let Some(tray) = self.icon.as_ref() else {
            return Err(Unavailable::runtime_error("the tray icon has been shut down"));
        };
        tray.set_icon(Some(icon))
            .map_err(|e| Unavailable::runtime_error(format!("cannot set the tray icon: {e}")))?;
        tray.set_menu(Some(Box::new(menu)));
        tray.set_tooltip(Some(tooltip)).map_err(|e| {
            Unavailable::runtime_error(format!("cannot set the tray tooltip: {e}"))
        })?;
        tray.set_title(Some(if self.count > 0 {
            self.count.to_string()
        } else {
            String::new()
        }));
        Ok(())
    }
}

impl Tray for DesktopTray {
    fn capability(&self) -> Support {
        match self.icon {
            Some(_) => Support::Available,
            None => Support::Missing(Unavailable::runtime_error(
                "the tray icon has been shut down",
            )),
        }
    }

    fn set_count(&mut self, count: u32) -> Result<(), Unavailable> {
        if self.count == count {
            return Ok(());
        }
        self.count = count;
        self.refresh()
    }

    fn set_window_visible(&mut self, visible: bool) -> Result<(), Unavailable> {
        if self.window_visible == visible {
            return Ok(());
        }
        self.window_visible = visible;
        self.refresh()
    }

    fn shutdown(&mut self) {
        // Dropping the TrayIcon removes it from the status area.
        self.icon = None;
    }
}

/// Whether the platform's status area exists, without creating an icon.
pub fn probe() -> Support {
    #[cfg(target_os = "macos")]
    {
        match objc2::MainThreadMarker::new() {
            Some(_) => Support::Available,
            None => Support::Missing(Unavailable::runtime_error(
                "NSStatusItem is main-thread only; probe the tray from the main thread",
            )),
        }
    }
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        use windows::core::w;
        // SAFETY: a read-only window lookup by class name.
        match unsafe { FindWindowW(w!("Shell_TrayWnd"), None) } {
            Ok(hwnd) if !hwnd.0.is_null() => Support::Available,
            _ => Support::Missing(Unavailable::service_missing(
                "no Shell_TrayWnd window: Explorer is not running, so there is no notification \
                 area to put an icon in",
            )),
        }
    }
}
