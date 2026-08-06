//! `org.freedesktop.Notifications` over the session bus.
//!
//! Blocking zbus, so a caller does not have to own an async runtime. zbus runs
//! its own reactor thread; nothing here spins. The activation listener is a
//! thread parked on a socket read, woken by the kernel when the daemon emits
//! `ActionInvoked`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

use crate::capability::{Support, Unavailable};
use crate::notify::{
    ActivationHandler, DbusNotifyArgs, HintValue, Notification, NotificationHandle, Notifier,
};
use vitrum_proto::SessionId;

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const INTERFACE: &str = "org.freedesktop.Notifications";

/// How many live notification ids to remember for activation routing.
///
/// Bounded because `NotificationClosed` is not reliably delivered for every
/// notification on every desktop, and an unbounded map keyed by a monotonically
/// increasing server id is a slow leak in a process meant to run for weeks.
const ROUTE_CAPACITY: usize = 128;

/// Notification id to session, most recent last.
#[derive(Default)]
struct Routes {
    order: Vec<u32>,
    map: HashMap<u32, SessionId>,
}

impl Routes {
    fn insert(&mut self, id: u32, session: SessionId) {
        if self.map.insert(id, session).is_none() {
            self.order.push(id);
        }
        while self.order.len() > ROUTE_CAPACITY {
            let oldest = self.order.remove(0);
            self.map.remove(&oldest);
        }
    }

    fn get(&self, id: u32) -> Option<SessionId> {
        self.map.get(&id).copied()
    }
}

/// Shared between the notifier and its activation listener thread.
#[derive(Default)]
struct Shared {
    routes: Mutex<Routes>,
    handler: Mutex<Option<ActivationHandler>>,
}

pub struct DbusNotifier {
    conn: Connection,
    shared: Arc<Shared>,
    listener_started: Mutex<bool>,
}

impl DbusNotifier {
    /// Connect and prove the daemon answers.
    ///
    /// `GetServerInformation` rather than a bare connect: owning a bus
    /// connection says nothing about whether anything is listening on
    /// `org.freedesktop.Notifications`, and on a bare X session with no
    /// notification daemon that is exactly the difference between working and
    /// silently dropping every message.
    pub fn connect() -> Result<Self, Unavailable> {
        let conn = Connection::session().map_err(|e| {
            Unavailable::service_missing(format!("no D-Bus session bus: {e}"))
        })?;
        let notifier = Self {
            conn,
            shared: Arc::new(Shared::default()),
            listener_started: Mutex::new(false),
        };
        notifier.server_information()?;
        Ok(notifier)
    }

    fn proxy(&self) -> Result<Proxy<'_>, Unavailable> {
        Proxy::new(&self.conn, DESTINATION, PATH, INTERFACE)
            .map_err(|e| Unavailable::runtime_error(format!("cannot build notification proxy: {e}")))
    }

    /// `(name, vendor, version, spec_version)` from the running daemon.
    pub fn server_information(&self) -> Result<(String, String, String, String), Unavailable> {
        self.proxy()?
            .call::<_, _, (String, String, String, String)>("GetServerInformation", &())
            .map_err(map_call_error)
    }

    /// What the running daemon says it supports, for example `body-markup`,
    /// `actions`, `persistence`.
    pub fn server_capabilities(&self) -> Result<Vec<String>, Unavailable> {
        self.proxy()?
            .call::<_, _, Vec<String>>("GetCapabilities", &())
            .map_err(map_call_error)
    }

    /// Deliver, replacing an earlier notification in place.
    pub fn notify_replacing(
        &self,
        notification: &Notification,
        replaces: Option<NotificationHandle>,
    ) -> Result<NotificationHandle, Unavailable> {
        // Actions are advertised only when a click can actually be routed:
        // `Show` on a notification nothing is listening for is a button that
        // cannot do anything.
        let args = actions_for_routing(
            notification.dbus_args(replaces.map_or(0, |h| h.0 as u32)),
            self.can_route_clicks(),
        );
        let id = self.send(&args)?;
        self.shared
            .routes
            .lock()
            .expect("route map is never held across a panic")
            .insert(id, notification.session);
        Ok(NotificationHandle(id as u64))
    }

    fn send(&self, args: &DbusNotifyArgs) -> Result<u32, Unavailable> {
        let actions: Vec<&str> = args.actions.iter().map(String::as_str).collect();
        let hints: HashMap<&str, Value<'_>> = args
            .hints
            .iter()
            .map(|(k, v)| (k.as_str(), hint_value(v)))
            .collect();
        self.proxy()?
            .call::<_, _, u32>(
                "Notify",
                &(
                    args.app_name.as_str(),
                    args.replaces_id,
                    args.app_icon.as_str(),
                    args.summary.as_str(),
                    args.body.as_str(),
                    actions,
                    hints,
                    args.expire_timeout,
                ),
            )
            .map_err(map_call_error)
    }

    /// Park a thread on `ActionInvoked` and route clicks back to the handler.
    fn start_listener(&self) -> Result<(), Unavailable> {
        let mut started = self
            .listener_started
            .lock()
            .expect("listener flag is never held across a panic");
        if *started {
            return Ok(());
        }
        // Its own connection: the signal is a broadcast, and a second
        // connection keeps the blocking iterator off the thread that sends.
        let conn = Connection::session().map_err(|e| {
            Unavailable::service_missing(format!("no D-Bus session bus for activations: {e}"))
        })?;
        let weak: Weak<Shared> = Arc::downgrade(&self.shared);
        std::thread::Builder::new()
            .name("vitrum-notify-activations".to_string())
            .spawn(move || run_listener(conn, weak))
            .map_err(|e| {
                Unavailable::runtime_error(format!("cannot spawn activation listener: {e}"))
            })?;
        *started = true;
        Ok(())
    }

    /// Can a click actually be routed right now?
    ///
    /// Both halves are required and neither implies the other in the failure
    /// case: [`Notifier::set_activation_handler`] fills the slot and only then
    /// starts the listener, so a listener that failed to spawn leaves a handler
    /// nothing will ever call.
    fn can_route_clicks(&self) -> bool {
        *self
            .listener_started
            .lock()
            .expect("listener flag is never held across a panic")
            && self
                .shared
                .handler
                .lock()
                .expect("handler slot is never held across a panic")
                .is_some()
    }
}

/// Drop the click actions when nothing can route a click.
///
/// [`Notification::dbus_args`] always offers `default` and `Show`, because the
/// payload is a pure value and cannot know whether anyone is listening. This is
/// where that is decided. A `Notify` call that advertises `Show` while no
/// `ActionInvoked` listener is subscribed puts a button on the user's screen
/// that cannot do anything, and `default` makes the notification body itself
/// falsely clickable, so both go.
///
/// Only the actions are touched. The urgency, the category, the session hint
/// and the expiry are what the notification says rather than what it offers,
/// and a notification with no button is still worth showing.
fn actions_for_routing(mut args: DbusNotifyArgs, routable: bool) -> DbusNotifyArgs {
    if !routable {
        args.actions.clear();
    }
    args
}

/// Watch for `ActionInvoked` with a match rule that does not pin the sender.
///
/// This is deliberately *not* `Proxy::receive_signal`, which filters on the
/// unique name currently owning `org.freedesktop.Notifications`. Measured on
/// GNOME 46: the name is owned by a gjs process (`:1.746`) while the
/// `ActionInvoked` signal is emitted by gnome-shell itself (`:1.738`). A
/// sender-scoped rule therefore never matches and every notification click is
/// silently dropped, which is exactly the failure this crate exists to avoid.
/// The signal carries no authority, only a notification id we minted, so
/// accepting it from any sender costs nothing.
fn run_listener(conn: Connection, shared: Weak<Shared>) {
    let Ok(rule) = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(PATH)
        .and_then(|b| b.interface(INTERFACE))
        .and_then(|b| b.member("ActionInvoked"))
        .map(zbus::match_rule::Builder::build)
    else {
        return;
    };
    let Ok(messages) = zbus::blocking::MessageIterator::for_match_rule(rule, &conn, None) else {
        return;
    };
    for message in messages {
        // The notifier is gone; nobody is left to route to.
        let Some(shared) = shared.upgrade() else { return };
        let Ok(message) = message else { continue };
        let Ok((id, _action)) = message.body().deserialize::<(u32, String)>() else {
            continue;
        };
        let session = shared
            .routes
            .lock()
            .expect("route map is never held across a panic")
            .get(id);
        let Some(session) = session else { continue };
        let handler = shared
            .handler
            .lock()
            .expect("handler slot is never held across a panic")
            .clone();
        if let Some(handler) = handler {
            handler(session);
        }
    }
}

fn hint_value(v: &HintValue) -> Value<'static> {
    match v {
        HintValue::Byte(b) => Value::U8(*b),
        HintValue::Bool(b) => Value::Bool(*b),
        HintValue::Int32(i) => Value::I32(*i),
        HintValue::Str(s) => Value::Str(s.clone().into()),
    }
}

fn map_call_error(e: zbus::Error) -> Unavailable {
    if let zbus::Error::MethodError(name, detail, _) = &e {
        let name = name.as_str();
        if name == "org.freedesktop.DBus.Error.ServiceUnknown"
            || name == "org.freedesktop.DBus.Error.NameHasNoOwner"
        {
            return Unavailable::service_missing(format!(
                "no notification daemon owns {DESTINATION}: {}",
                detail.as_deref().unwrap_or(name)
            ));
        }
        if name == "org.freedesktop.DBus.Error.AccessDenied" {
            return Unavailable::permission_denied(format!(
                "{DESTINATION} refused: {}",
                detail.as_deref().unwrap_or(name)
            ));
        }
    }
    Unavailable::runtime_error(format!("{DESTINATION} call failed: {e}"))
}

impl Notifier for DbusNotifier {
    fn capability(&self) -> Support {
        Support::from_result(self.server_information())
    }

    fn notify(&self, notification: &Notification) -> Result<NotificationHandle, Unavailable> {
        self.notify_replacing(notification, None)
    }

    fn close(&self, handle: NotificationHandle) -> Result<(), Unavailable> {
        self.proxy()?
            .call::<_, _, ()>("CloseNotification", &(handle.0 as u32))
            .map_err(map_call_error)
    }

    fn set_activation_handler(&self, handler: ActivationHandler) -> Result<(), Unavailable> {
        *self
            .shared
            .handler
            .lock()
            .expect("handler slot is never held across a panic") = Some(handler);
        self.start_listener()
    }
}

/// Tests for the one decision this backend makes that is not a D-Bus call.
///
/// Here rather than in `crate::tests` because the function under test is
/// private to this module: `src/tests/` descends from the crate root, not from
/// `notify`, so it cannot see it. Everything else in this file needs a live
/// session bus and is asserted in `crate::tests::live_linux`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::NotificationKind;

    /// Locks out: a notification advertising an action this build cannot
    /// service.
    ///
    /// `dbus_args` offers `default` and `Show` unconditionally, and for a long
    /// time nothing in the shipping application ever called
    /// `set_activation_handler`, so [`DbusNotifier::start_listener`] was never
    /// reached and the `ActionInvoked` match rule was never subscribed to. Every
    /// Linux notification therefore rendered a `Show` button that could not do
    /// anything, and a body that looked clickable and was not.
    ///
    /// Every kind, and no hand-kept field list, because both are shapes a guard
    /// falls behind. The first version used only `finished`, and a kind is what
    /// fixes `expire_timeout`, urgency and category, so a strip conditioned on
    /// any of those hid from it. Hunted for and confirmed: `if !routable &&
    /// args.expire_timeout != 0` passed the single-kind version while leaving the
    /// button on every approval, which is critical, never expires, and is the one
    /// notification where a dead button costs the most.
    ///
    /// The `match` on `kind` is the enforcement point: adding a variant to
    /// [`NotificationKind`] makes it non-exhaustive and this file stops
    /// compiling, so a new kind cannot arrive untested. And "only the actions
    /// change" is asserted by comparing whole structs rather than by listing
    /// seven fields, so a field added to [`DbusNotifyArgs`] is covered the day it
    /// appears instead of the day someone remembers this test.
    #[test]
    fn an_unroutable_notification_advertises_no_actions() {
        for kind in [
            NotificationKind::Finished,
            NotificationKind::NeedsApproval,
            NotificationKind::Failed,
        ] {
            // Literals, never values read back out of the code under test: an
            // expectation derived from the same call is an identity, not a check.
            let (detail, summary, body, expire) = match kind {
                NotificationKind::Finished => ("exit 0", "build finished", "exit 0", -1),
                NotificationKind::NeedsApproval => {
                    ("run rm -rf?", "build needs approval", "run rm -rf?", 0)
                }
                NotificationKind::Failed => ("signalled", "build failed", "signalled", 0),
            };
            let n = Notification::new(kind, SessionId(7), "build", detail);
            let full = n.dbus_args(0);

            assert_eq!(full.summary, summary, "{kind} summary");
            assert_eq!(full.body, body, "{kind} body");
            assert_eq!(full.expire_timeout, expire, "{kind} expiry");
            assert_eq!(
                full.actions,
                vec!["default".to_string(), "Show".to_string()],
                "{kind} no longer advertises the two keys this guard is about"
            );

            let mut want = full.clone();
            want.actions.clear();
            assert_eq!(
                actions_for_routing(n.dbus_args(0), false),
                want,
                "stripping a {kind} nothing can route must clear the actions and \
                 change nothing else"
            );
            assert_eq!(
                actions_for_routing(n.dbus_args(0), true),
                full,
                "a routable {kind} must be handed to the daemon untouched"
            );
        }
    }

    /// Locks out: the strip above existing and never being reached.
    ///
    /// The test above proves [`actions_for_routing`] is correct in isolation,
    /// and a pure function proven correct in isolation is exactly the shape
    /// this crate already shipped once: `set_activation_handler` worked
    /// perfectly and nothing in `app/` ever called it, which is the whole
    /// reason the `Show` button was dead. So this asserts the WIRING. There is
    /// one place that builds the `Notify` arguments, and it must build them
    /// through the strip, with the live routability rather than a constant.
    #[test]
    fn the_only_send_site_builds_its_arguments_through_the_strip() {
        let src = include_str!("linux.rs");
        // Only the shipped half: below the test module these same needles
        // appear as assertion data, which would prove nothing.
        let shipped = src
            .split_once("#[cfg(test)]")
            .map_or(src, |(before, _)| before);

        let built = shipped.matches("dbus_args(").count();
        assert_eq!(
            built, 1,
            "the Notify arguments are built in {built} places; every one of them \
             has to strip the actions, or one path advertises a button nothing \
             can service"
        );

        let strip = shipped
            .find("= actions_for_routing(")
            .expect("the send site no longer strips the actions of an unroutable notification");
        let at = shipped
            .find("dbus_args(")
            .expect("nothing builds the Notify arguments any more");
        assert!(
            at > strip && at - strip < 120,
            "the Notify arguments are built outside the strip, so a `Show` \
             button ships again whenever no click can be routed"
        );
        assert!(
            shipped[strip..].starts_with("= actions_for_routing(")
                && shipped[strip..(strip + 200).min(shipped.len())]
                    .contains("self.can_route_clicks()"),
            "the strip is passed a constant rather than the live routing state, \
             so it can no longer answer the question it exists for"
        );
    }
}
