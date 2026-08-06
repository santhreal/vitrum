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

use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

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
/// How long a portal read may take before the portal is treated as absent.
///
/// Generous next to the milliseconds a running portal needs, and small next to
/// the 120 second `service_start_timeout` this exists to avoid paying twice.
pub(crate) const PORTAL_CALL_TIMEOUT: Duration = Duration::from_secs(5);

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
        // Ask the bus whether the portal could ever answer before calling it.
        //
        // Without this, a machine with a session bus and no portal installed
        // does not fail fast: D-Bus treats the destination as activatable,
        // tries to start it, and returns TimedOut after
        // `service_start_timeout`, which defaults to 120 seconds. Both `Read`
        // and `ReadOne` are attempted, so probing a machine with no portal cost
        // four minutes and then reported a runtime error rather than a missing
        // service.
        portal_reachable(&watcher.conn)?;
        // Prove the portal answers before claiming the feature works.
        watcher.read_color_scheme()?;
        Ok(watcher)
    }

    /// The raw `color-scheme` value, bounded so a portal that cannot start
    /// costs seconds rather than minutes.
    fn read_color_scheme(&self) -> Result<u32, Unavailable> {
        let conn = self.conn.clone();
        within(PORTAL_CALL_TIMEOUT, move || read_color_scheme_on(&conn))
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

/// Whether the portal name could answer at all: owned now, or startable.
///
/// An activatable name legitimately has no owner until something calls it, so
/// `NameHasOwner` alone would report an installed-but-idle portal as missing.
/// Neither owned nor activatable is the definitive answer, and it costs one
/// round trip instead of two activation timeouts.
fn portal_reachable(conn: &Connection) -> Result<(), Unavailable> {
    let bus = Proxy::new(conn, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")
        .map_err(|e| Unavailable::runtime_error(format!("cannot build the bus proxy: {e}")))?;
    // A bus that cannot answer these is not something to diagnose from here.
    // Fall through and let the real call produce the error, rather than
    // refusing over a failed probe.
    let Ok(owned) = bus.call::<_, _, bool>("NameHasOwner", &(DESTINATION,)) else {
        return Ok(());
    };
    if owned {
        return Ok(());
    }
    let Ok(activatable) = bus.call::<_, _, Vec<String>>("ListActivatableNames", &()) else {
        return Ok(());
    };
    if activatable.iter().any(|name| name == DESTINATION) {
        return Ok(());
    }
    Err(portal_absent())
}

fn portal_absent() -> Unavailable {
    Unavailable::service_missing(format!(
        "no xdg-desktop-portal on the session bus ({DESTINATION}); install \
         xdg-desktop-portal and a backend such as xdg-desktop-portal-gtk or -kde"
    ))
}

/// Run `job` on its own thread and give up waiting after `timeout`.
///
/// A call to an activatable name that never comes up does not fail: the bus
/// waits out `service_start_timeout`, 120 seconds by default, and a portal
/// read makes two calls. `preference` runs when the settings sheet opens, so
/// unbounded that is four minutes of frozen UI on a machine whose portal
/// cannot start.
///
/// A bound rather than the `NoAutoStart` flag, which would cost nothing: an
/// activatable portal legitimately has no owner until something calls it, so
/// refusing to start it would report a working desktop as having no portal.
/// Activation is kept and only the waiting is capped.
///
/// The abandoned thread is not leaked. It owns everything it touches, finishes
/// when the bus finally answers, and drops.
pub(crate) fn within<T: Send + 'static>(
    timeout: Duration,
    job: impl FnOnce() -> Result<T, Unavailable> + Send + 'static,
) -> Result<T, Unavailable> {
    let (tx, rx) = sync_channel(1);
    std::thread::Builder::new()
        .name("vitrum-theme-read".to_string())
        .spawn(move || {
            let _ = tx.send(job());
        })
        .map_err(|e| Unavailable::runtime_error(format!("cannot spawn the theme read: {e}")))?;
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(portal_unresponsive()),
        // The reader panicked. Nothing here can say why, and calling the portal
        // missing would be a guess.
        Err(RecvTimeoutError::Disconnected) => {
            Err(Unavailable::runtime_error("the portal read ended without answering".to_string()))
        }
    }
}

/// The raw `color-scheme` value, on whichever thread is willing to wait.
///
/// `ReadOne` is the current method; `Read` is deprecated and, because of a
/// long-standing xdg-desktop-portal quirk, returns the value wrapped in a
/// second variant. Both are handled so this works against portals older than
/// version 2.
fn read_color_scheme_on(conn: &Connection) -> Result<u32, Unavailable> {
    let proxy = Proxy::new(conn, DESTINATION, PATH, INTERFACE)
        .map_err(|e| Unavailable::runtime_error(format!("cannot build portal proxy: {e}")))?;
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

/// The bus accepted the name but nothing answered in time.
///
/// Reported as missing rather than as a runtime error because the one thing
/// that reliably produces it is a portal that is registered as activatable and
/// cannot start, which is the same situation as not having one.
fn portal_unresponsive() -> Unavailable {
    Unavailable::service_missing(format!(
        "xdg-desktop-portal did not answer within {}s ({DESTINATION}); it is \
         registered on the session bus but not running, so install a backend \
         such as xdg-desktop-portal-gtk or -kde, or start the portal service",
        PORTAL_CALL_TIMEOUT.as_secs()
    ))
}

/// Whether a D-Bus error name and message mean the portal is absent rather
/// than present and failing.
///
/// Split out from [`map_call_error`] because a `zbus::Error::MethodError`
/// carries a `Message` that cannot be constructed in a test, and the decision
/// this makes is the part worth defending.
pub(crate) fn names_an_absent_service(name: &str, detail: Option<&str>) -> bool {
    match name {
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.ServiceStartFailed" => true,
        // The bus accepted the name as activatable, tried to start it, and
        // nothing came up. That is the service being absent, not the service
        // failing a call, and it is what a machine with no portal installed
        // actually reports after `service_start_timeout`. A timeout that does
        // NOT name activation is a portal that exists and hung, which is a
        // runtime error and must stay one.
        "org.freedesktop.DBus.Error.TimedOut" | "org.freedesktop.DBus.Error.NoReply" => {
            detail.is_some_and(|text| text.contains("activate service"))
        }
        other => other.starts_with("org.freedesktop.DBus.Error.Spawn."),
    }
}

fn map_call_error(read_one: &zbus::Error, read: zbus::Error) -> Unavailable {
    let missing = |e: &zbus::Error| {
        matches!(e, zbus::Error::MethodError(name, detail, _)
            if names_an_absent_service(name.as_str(), detail.as_deref()))
    };
    if missing(read_one) || missing(&read) {
        return portal_absent();
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
