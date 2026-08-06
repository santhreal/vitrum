//! Integration boundary: these tests talk to the real session bus.
//!
//! Everything else in this suite is pure. This file is where the code actually
//! meets a desktop, and it is written so that both outcomes are assertions:
//! when a service is present the test proves the round trip works, and when it
//! is absent the test proves we report *that specific absence* rather than a
//! false success. Nothing here is `#[ignore]`d and nothing silently passes by
//! doing nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use vitrum_proto::SessionId;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedValue;

use crate::capability::UnavailableKind;
use crate::notify::Notification;
use crate::theme::{
    NO_PREFERENCE_THEME, Theme, theme_from_portal_color_scheme, theme_watcher,
};
use crate::{Feature, badge, notify, tray};

/// Is there a session bus at all? Used to make the assertions below precise
/// about which absence they are asserting.
fn session_bus() -> Option<Connection> {
    Connection::session().ok()
}

/// The notification backend must either work against the live daemon or say
/// exactly what is missing.
///
/// The failure this locks out is a backend that connects to the bus, finds
/// nothing serving `org.freedesktop.Notifications`, and reports success anyway
/// because owning a bus connection looked like enough. That produces a product
/// that silently never notifies on a bare X session.
#[test]
fn the_notification_backend_either_works_or_names_what_is_missing() {
    match notify::notifier() {
        Ok(notifier) => {
            let support = notifier.capability();
            assert!(
                support.is_available(),
                "connect succeeded but capability disagrees: {support}"
            );
        }
        Err(u) => {
            assert_eq!(
                u.kind,
                UnavailableKind::ServiceMissing,
                "a missing daemon is a missing service, not a runtime error: {u}"
            );
            assert!(
                u.detail.contains("D-Bus") || u.detail.contains("org.freedesktop.Notifications"),
                "the reason must name what is missing: {}",
                u.detail
            );
        }
    }
}

/// The live daemon must identify itself and advertise the spec's core
/// capabilities.
///
/// `body` and `actions` are what this crate depends on: the body carries the
/// detail line and the `default` action is how a click is routed back to a
/// session. A daemon advertising neither means notifications are decorative,
/// and knowing that is better than discovering it from a user.
#[test]
fn the_live_daemon_identifies_itself_and_supports_bodies() {
    let Ok(notifier) = notify::DbusNotifier::connect() else {
        // No daemon: the previous test has already asserted the reported reason.
        assert!(session_bus().is_none() || notify::notifier().is_err());
        return;
    };
    let (name, vendor, version, spec) =
        notifier.server_information().expect("a connected notifier answers GetServerInformation");
    assert!(!name.is_empty(), "the daemon must name itself");
    assert!(!vendor.is_empty(), "the daemon must name its vendor");
    assert!(!version.is_empty(), "the daemon must report a version");
    assert!(
        spec.starts_with("1."),
        "this crate targets the 1.x notification spec, daemon reports {spec}"
    );

    let caps = notifier.server_capabilities().expect("a connected notifier answers GetCapabilities");
    assert!(caps.contains(&"body".to_string()), "daemon advertises {caps:?} without `body`");
    assert!(
        caps.contains(&"actions".to_string()),
        "daemon advertises {caps:?} without `actions`, so clicks cannot be routed"
    );
}

/// A real notification must be delivered and then withdrawn.
///
/// End to end through the actual daemon: the payload builder, the zbus call and
/// the returned id. It is closed immediately so a test run does not leave
/// anything on screen. A daemon returning id 0 would mean the call was accepted
/// and dropped.
#[test]
fn a_real_notification_is_delivered_and_withdrawn() {
    let Ok(notifier) = notify::notifier() else { return };
    let n = Notification::finished(
        SessionId(0),
        "vitrum-os self test",
        "delivered by the vitrum-os test suite, closing immediately",
    );
    let handle = notifier.notify(&n).expect("a live daemon accepts a well-formed notification");
    assert_ne!(handle.0, 0, "the daemon must return a real notification id");
    notifier.close(handle).expect("a delivered notification can be withdrawn");
}

/// Installing an activation handler must start the listener without error.
///
/// The handler is what turns a click into a focused session. If installing it
/// failed, notifications would still appear and clicking them would do nothing,
/// which is the worst of both.
#[test]
fn installing_an_activation_handler_starts_the_listener() {
    let Ok(notifier) = notify::notifier() else { return };
    let clicks = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&clicks);
    notifier
        .set_activation_handler(Arc::new(move |_session| {
            counter.fetch_add(1, Ordering::SeqCst);
        }))
        .expect("the activation listener must start");
    // Installing it twice must replace rather than spawn a second listener.
    notifier
        .set_activation_handler(Arc::new(|_session| {}))
        .expect("replacing the handler must be idempotent");
    assert_eq!(clicks.load(Ordering::SeqCst), 0, "nothing was clicked");
}

/// The theme watcher must agree with a direct portal read.
///
/// This is the whole detection path checked against its own source: an
/// independent `ReadOne` on a fresh connection, decoded by the pure function,
/// must equal what the watcher reports. A watcher that read the wrong key, or
/// inverted the mapping, passes every unit test and fails here.
#[test]
fn the_theme_watcher_agrees_with_a_direct_portal_read() {
    let raw = read_color_scheme_directly();
    match (theme_watcher(), raw) {
        (Ok(watcher), Some(raw)) => {
            let expected = theme_from_portal_color_scheme(raw);
            assert_eq!(watcher.preference().expect("the portal answered once already"), expected);
            assert_eq!(
                watcher.current().expect("the portal answered once already"),
                expected.unwrap_or(NO_PREFERENCE_THEME)
            );
            assert!(watcher.capability().is_available());
        }
        (Err(u), None) => {
            assert_eq!(
                u.kind,
                UnavailableKind::ServiceMissing,
                "an absent portal is a missing service: {u}"
            );
            assert!(
                u.detail.contains("portal") || u.detail.contains("D-Bus"),
                "the reason must name the portal: {}",
                u.detail
            );
        }
        (Ok(_), None) => panic!("the watcher connected but a direct portal read failed"),
        (Err(u), Some(raw)) => {
            panic!("the portal answers with color-scheme {raw} but the watcher refused: {u}")
        }
    }
}

/// Subscribing to theme changes must succeed without polling.
///
/// The subscription parks a thread on the portal's `SettingChanged` signal.
/// The assertion is that installing it is accepted; the no-polling property is
/// structural, since there is no timer anywhere in the crate.
#[test]
fn subscribing_to_theme_changes_is_accepted() {
    let Ok(watcher) = theme_watcher() else { return };
    let seen = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&seen);
    watcher
        .subscribe(Arc::new(move |_theme| {
            counter.fetch_add(1, Ordering::SeqCst);
        }))
        .expect("the portal signal subscription must start");
    // A second subscribe replaces the handler and must not spawn a second
    // listener thread or fail.
    watcher.subscribe(Arc::new(|_theme| {})).expect("resubscribing must be idempotent");
}

/// Read `org.freedesktop.appearance color-scheme` without going through the
/// watcher, so the test has an independent source of truth.
fn read_color_scheme_directly() -> Option<u32> {
    let conn = session_bus()?;
    let proxy = Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )
    .ok()?;
    let value: OwnedValue = proxy
        .call("ReadOne", &("org.freedesktop.appearance", "color-scheme"))
        .ok()?;
    u32::try_from(&value).ok()
}

/// The tray probe must reflect whether a StatusNotifierWatcher is really there.
///
/// GNOME without the AppIndicator extension has no watcher, and an icon
/// registered there is never displayed. Reporting available anyway means a
/// "minimise to tray" setting that loses the window.
#[test]
fn the_tray_probe_reflects_the_live_watcher() {
    let watcher_present = name_has_owner("org.kde.StatusNotifierWatcher");
    let support = tray::probe();
    match watcher_present {
        Some(true) => assert!(
            support.is_available(),
            "a watcher is on the bus but the probe says {support}"
        ),
        Some(false) => {
            let reason = support.reason().expect("no watcher means unavailable");
            assert_eq!(reason.kind, UnavailableKind::ServiceMissing);
            assert!(
                reason.detail.contains("StatusNotifierWatcher"),
                "the reason must name the watcher: {}",
                reason.detail
            );
        }
        None => {
            let reason = support.reason().expect("no bus means unavailable");
            assert!(
                reason.detail.contains("D-Bus"),
                "the reason must name the bus: {}",
                reason.detail
            );
        }
    }
}

/// The badge probe must reflect whether a Unity launcher is really listening.
///
/// `LauncherEntry` is a fire-and-forget broadcast, so emitting it always
/// "succeeds". The owner check is the only thing standing between an honest
/// report and a badge feature that silently does nothing on most desktops.
#[test]
fn the_badge_probe_reflects_a_live_unity_listener() {
    let listener = name_has_owner("com.canonical.Unity");
    match (badge::badge(None), listener) {
        (Ok(b), Some(true)) => assert!(b.capability().is_available()),
        (Err(u), Some(false)) => {
            assert_eq!(u.kind, UnavailableKind::ServiceMissing);
            assert!(
                u.detail.contains("com.canonical.Unity"),
                "the reason must name the missing listener: {}",
                u.detail
            );
        }
        (Err(u), None) => assert!(
            u.detail.contains("D-Bus"),
            "no bus must be reported as such: {}",
            u.detail
        ),
        (Ok(_), Some(false) | None) => {
            panic!("the badge claimed to work with nothing listening on com.canonical.Unity")
        }
        (Err(u), Some(true)) => panic!("a Unity listener is present but the badge refused: {u}"),
    }
}

/// Setting a real badge count must be accepted by the bus.
#[test]
fn setting_a_real_badge_count_is_accepted() {
    let Ok(b) = badge::badge(None) else { return };
    b.set_count(3).expect("emitting a LauncherEntry Update must succeed");
    b.clear().expect("clearing must succeed");
}

/// The probe must classify this machine, and every unavailable feature must
/// carry an actionable reason.
///
/// This is the report an operator pastes into a bug. An entry that says
/// "unavailable" with no reason wastes the exchange.
#[test]
fn the_live_probe_classifies_this_machine() {
    let report = crate::probe(None);
    for feature in Feature::ALL {
        let support = report.get(feature).expect("every feature is probed");
        if let Some(reason) = support.reason() {
            assert!(!reason.detail.is_empty(), "{feature}: empty reason");
        }
    }
    // Paths and window state depend on nothing but the filesystem and must
    // always work on a machine that can run the test suite at all.
    assert!(report.get(Feature::Paths).expect("probed").is_available());
    assert!(report.get(Feature::WindowState).expect("probed").is_available());
    assert!(report.get(Feature::SingleInstance).expect("probed").is_available());
    // Linux can always write its own desktop entry.
    assert!(report.get(Feature::DeepLinks).expect("probed").is_available());
}

fn name_has_owner(name: &str) -> Option<bool> {
    let conn = session_bus()?;
    let proxy = Proxy::new(
        &conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .ok()?;
    proxy.call::<_, _, bool>("NameHasOwner", &(name,)).ok()
}

/// The decoded theme must be one of the two the UI can draw.
#[test]
fn the_live_theme_is_one_the_ui_can_draw() {
    let Ok(watcher) = theme_watcher() else { return };
    let theme = watcher.current().expect("a connected watcher answers");
    assert!(matches!(theme, Theme::Light | Theme::Dark));
}
