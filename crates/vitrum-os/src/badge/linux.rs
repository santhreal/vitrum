//! Unity LauncherEntry, the only badge protocol the Linux desktop has.
//!
//! `com.canonical.Unity.LauncherEntry.Update` is a bare broadcast signal: it
//! has no reply and no acknowledgement, so emitting it tells you nothing about
//! whether a launcher saw it. That makes the owner probe load-bearing. Without
//! it this backend would return `Ok(())` on a bare i3 session and the UI would
//! offer a badge that can never appear.

use zbus::blocking::Connection;

use crate::badge::{
    Badge, UNITY_BUS_NAME, UNITY_INTERFACE, UNITY_OBJECT_PATH, UnityValue, unity_app_uri,
    unity_properties,
};
use crate::capability::{Support, Unavailable};

pub struct UnityBadge {
    conn: Connection,
    app_uri: String,
}

impl UnityBadge {
    pub fn connect() -> Result<Self, Unavailable> {
        let conn = Connection::session()
            .map_err(|e| Unavailable::service_missing(format!("no D-Bus session bus: {e}")))?;
        let badge = Self { conn, app_uri: unity_app_uri() };
        match badge.listener_present() {
            Ok(true) => Ok(badge),
            Ok(false) => Err(Unavailable::service_missing(format!(
                "nothing owns {UNITY_BUS_NAME} on this session bus, so a LauncherEntry badge \
                 would be broadcast to no one. Dash to Dock, Ubuntu Dock, Plank, Latte and the \
                 KDE task manager all provide it; a bare window manager does not."
            ))),
            Err(e) => Err(e),
        }
    }

    /// Ask the bus whether anything owns the Unity name.
    fn listener_present(&self) -> Result<bool, Unavailable> {
        let proxy = zbus::blocking::Proxy::new(
            &self.conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .map_err(|e| Unavailable::runtime_error(format!("cannot build bus proxy: {e}")))?;
        proxy
            .call::<_, _, bool>("NameHasOwner", &(UNITY_BUS_NAME,))
            .map_err(|e| Unavailable::runtime_error(format!("NameHasOwner failed: {e}")))
    }
}

impl Badge for UnityBadge {
    fn capability(&self) -> Support {
        match self.listener_present() {
            Ok(true) => Support::Available,
            Ok(false) => Support::Missing(Unavailable::service_missing(format!(
                "nothing owns {UNITY_BUS_NAME}"
            ))),
            Err(e) => Support::Missing(e),
        }
    }

    fn set_count(&self, count: u32) -> Result<(), Unavailable> {
        let properties: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> =
            unity_properties(count)
                .into_iter()
                .map(|(k, v)| {
                    let value = match v {
                        UnityValue::Int64(i) => zbus::zvariant::Value::I64(i),
                        UnityValue::Bool(b) => zbus::zvariant::Value::Bool(b),
                    };
                    (k, value)
                })
                .collect();

        self.conn
            .emit_signal(
                None::<&str>,
                UNITY_OBJECT_PATH,
                UNITY_INTERFACE,
                "Update",
                &(self.app_uri.as_str(), properties),
            )
            .map_err(|e| {
                Unavailable::runtime_error(format!("cannot emit LauncherEntry Update: {e}"))
            })
    }
}
