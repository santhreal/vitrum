//! `org.freedesktop.portal.Settings`, the desktop-agnostic appearance source.
//!
//! Chosen over `gsettings org.gnome.desktop.interface color-scheme` because the
//! portal is what KDE, GNOME, Sway and every Flatpak runtime agree on, and
//! because reading the GNOME schema means the setting is invisible on KDE and
//! the code silently reports light to every Plasma user.
//!
//! The subscription is the portal's `SettingChanged` signal. The listener
//! thread is parked in a socket read; it costs nothing until the user flips the
//! switch.

use std::sync::{Arc, Mutex, Weak};

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedValue;

use crate::capability::{Support, Unavailable};
use crate::theme::{
    NO_PREFERENCE_THEME, Theme, ThemeHandler, ThemeWatcher, deduplicate,
    theme_from_portal_color_scheme,
};

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

#[derive(Default)]
struct Shared {
    handler: Mutex<Option<ThemeHandler>>,
}

pub(crate) struct PortalThemeWatcher {
    conn: Connection,
    shared: Arc<Shared>,
    listener_started: Mutex<bool>,
}

impl PortalThemeWatcher {
    pub fn connect() -> Result<Self, Unavailable> {
        let conn = Connection::session()
            .map_err(|e| Unavailable::service_missing(format!("no D-Bus session bus: {e}")))?;
        let watcher = Self {
            conn,
            shared: Arc::new(Shared::default()),
            listener_started: Mutex::new(false),
        };
        // Prove the portal answers before claiming the feature works.
        watcher.read_color_scheme()?;
        Ok(watcher)
    }

    fn proxy(&self) -> Result<Proxy<'_>, Unavailable> {
        Proxy::new(&self.conn, DESTINATION, PATH, INTERFACE)
            .map_err(|e| Unavailable::runtime_error(format!("cannot build portal proxy: {e}")))
    }

    /// The raw `color-scheme` value.
    ///
    /// `ReadOne` is the current method; `Read` is deprecated and, because of a
    /// long-standing xdg-desktop-portal quirk, returns the value wrapped in a
    /// second variant. Both are handled so this works against portals older
    /// than version 2.
    fn read_color_scheme(&self) -> Result<u32, Unavailable> {
        let proxy = self.proxy()?;
        match proxy.call::<_, _, OwnedValue>("ReadOne", &(NAMESPACE, KEY)) {
            Ok(value) => unwrap_u32(&value),
            Err(one_err) => {
                let legacy = proxy
                    .call::<_, _, OwnedValue>("Read", &(NAMESPACE, KEY))
                    .map_err(|e| map_call_error(&one_err, e))?;
                unwrap_u32(&legacy)
            }
        }
    }

    fn start_listener(&self) -> Result<(), Unavailable> {
        let mut started = self
            .listener_started
            .lock()
            .expect("listener flag is never held across a panic");
        if *started {
            return Ok(());
        }
        let conn = Connection::session().map_err(|e| {
            Unavailable::service_missing(format!("no D-Bus session bus for theme changes: {e}"))
        })?;
        let weak = Arc::downgrade(&self.shared);
        std::thread::Builder::new()
            .name("vitrum-theme-watch".to_string())
            .spawn(move || run_listener(conn, weak))
            .map_err(|e| {
                Unavailable::runtime_error(format!("cannot spawn the theme listener: {e}"))
            })?;
        *started = true;
        Ok(())
    }
}

fn run_listener(conn: Connection, shared: Weak<Shared>) {
    let Ok(proxy) = Proxy::new(&conn, DESTINATION, PATH, INTERFACE) else {
        return;
    };
    let Ok(signals) = proxy.receive_signal("SettingChanged") else {
        return;
    };
    for message in signals {
        let Some(shared) = shared.upgrade() else { return };
        let Ok((namespace, key, value)) =
            message.body().deserialize::<(String, String, OwnedValue)>()
        else {
            continue;
        };
        if namespace != NAMESPACE || key != KEY {
            continue;
        }
        let Ok(raw) = unwrap_u32(&value) else { continue };
        let theme = theme_from_portal_color_scheme(raw).unwrap_or(NO_PREFERENCE_THEME);
        let handler =
            shared.handler.lock().expect("handler slot is never held across a panic").clone();
        if let Some(handler) = handler {
            handler(theme);
        }
    }
}

/// Unwrap the value, tolerating one extra layer of variant nesting.
fn unwrap_u32(value: &OwnedValue) -> Result<u32, Unavailable> {
    if let Ok(v) = u32::try_from(value) {
        return Ok(v);
    }
    if let zbus::zvariant::Value::Value(inner) = &**value {
        if let zbus::zvariant::Value::U32(v) = &**inner {
            return Ok(*v);
        }
    }
    Err(Unavailable::runtime_error(format!(
        "portal returned {NAMESPACE}.{KEY} with signature `{}`, expected `u`",
        value.value_signature()
    )))
}

fn map_call_error(read_one: &zbus::Error, read: zbus::Error) -> Unavailable {
    let missing = |e: &zbus::Error| {
        matches!(e, zbus::Error::MethodError(name, _, _)
            if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown"
                || name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner")
    };
    if missing(read_one) || missing(&read) {
        return Unavailable::service_missing(format!(
            "no xdg-desktop-portal on the session bus ({DESTINATION}); install \
             xdg-desktop-portal and a backend such as xdg-desktop-portal-gtk or -kde"
        ));
    }
    Unavailable::runtime_error(format!(
        "portal Settings unreadable: ReadOne failed with {read_one}, Read failed with {read}"
    ))
}

impl ThemeWatcher for PortalThemeWatcher {
    fn capability(&self) -> Support {
        Support::from_result(self.read_color_scheme())
    }

    fn current(&self) -> Result<Theme, Unavailable> {
        Ok(self.preference()?.unwrap_or(NO_PREFERENCE_THEME))
    }

    fn preference(&self) -> Result<Option<Theme>, Unavailable> {
        Ok(theme_from_portal_color_scheme(self.read_color_scheme()?))
    }

    fn subscribe(&self, handler: ThemeHandler) -> Result<(), Unavailable> {
        let handler = deduplicate(self.current().ok(), handler);
        *self.shared.handler.lock().expect("handler slot is never held across a panic") =
            Some(handler);
        self.start_listener()
    }
}
