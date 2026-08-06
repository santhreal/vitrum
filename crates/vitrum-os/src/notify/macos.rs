//! `UNUserNotificationCenter`, the only notification API macOS 11+ supports for
//! an app (`NSUserNotification` was removed).
//!
//! Two hard platform facts drive the shape here, and both are reported rather
//! than hidden:
//!
//! - **UserNotifications requires a bundle.** `currentNotificationCenter` on an
//!   unbundled binary raises an Objective-C exception, which in Rust is an
//!   abort. So the bundle identifier is checked first and a missing one is
//!   reported as unavailable. `cargo run` of a bare binary is exactly that
//!   case; a `.app` built by the packaging step is not.
//! - **Delivery needs authorisation.** The first request pops the system
//!   prompt. A denial is a permanent `PermissionDenied`, not a retryable error.

use std::sync::{Arc, Mutex};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_foundation::{NSBundle, NSDictionary, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationInterruptionLevel, UNNotificationPresentationOptions, UNNotificationRequest,
    UNNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use crate::capability::{Support, Unavailable};
use crate::deeplink::{self, DeepLink};
use crate::notify::{
    ActivationHandler, MacInterruptionLevel, Notification, NotificationHandle, Notifier,
};

/// Slot the delegate reads the current handler out of.
type HandlerSlot = Arc<Mutex<Option<ActivationHandler>>>;

struct DelegateIvars {
    handler: HandlerSlot,
}

define_class!(
    // Any thread: UserNotifications delivers responses on an arbitrary queue,
    // and pinning the delegate to the main thread would mean it could not be
    // constructed from a worker.
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    #[name = "VitrumNotificationDelegate"]
    #[ivars = DelegateIvars]
    struct NotificationDelegate;

    unsafe impl NSObjectProtocol for NotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NotificationDelegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive_response(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &DynBlock<dyn Fn()>,
        ) {
            if let Some(session) = session_from_response(response) {
                let handler = self
                    .ivars()
                    .handler
                    .lock()
                    .expect("handler slot is never held across a panic")
                    .clone();
                if let Some(handler) = handler {
                    handler(session);
                }
            }
            // Not calling this leaks the response and eventually stops
            // delivery, so it runs on every path including the parse failure.
            completion_handler.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            // Without this macOS suppresses banners while the app is
            // foreground, which silently loses every notification raised by a
            // background session while the user is looking at another tab.
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::Sound,));
        }
    }
);

/// Pull the session out of a click, through the same URL parser everything
/// else uses.
fn session_from_response(response: &UNNotificationResponse) -> Option<vitrum_proto::SessionId> {
    let info = response.notification().request().content().userInfo();
    let key = NSString::from_str("url");
    let value = info.objectForKey(&key)?;
    let url = value.downcast::<NSString>().ok()?;
    match deeplink::parse(&url.to_string()) {
        Ok(DeepLink::Session(id)) => Some(id),
        _ => None,
    }
}

pub struct MacNotifier {
    center: Retained<UNUserNotificationCenter>,
    handler: HandlerSlot,
    /// Held for the lifetime of the notifier: `setDelegate:` is a weak
    /// property, so dropping this would silently stop activations.
    delegate: Mutex<Option<Retained<NotificationDelegate>>>,
    /// Identifiers of delivered notifications, in handle order.
    delivered: Mutex<Vec<String>>,
}

// SAFETY: `UNUserNotificationCenter` and the delegate are Objective-C objects
// with atomic reference counts, and every mutable field is behind a `Mutex`.
unsafe impl Send for MacNotifier {}
unsafe impl Sync for MacNotifier {}

impl MacNotifier {
    pub fn connect() -> Result<Self, Unavailable> {
        let bundle = NSBundle::mainBundle();
        if bundle.bundleIdentifier().is_none() {
            return Err(Unavailable::not_implemented(
                "UserNotifications requires a bundled application; this binary has no \
                 CFBundleIdentifier, so `UNUserNotificationCenter.currentNotificationCenter` \
                 would raise. Package as Vitrum.app and run from the bundle.",
            ));
        }
        let notifier = Self {
            center: UNUserNotificationCenter::currentNotificationCenter(),
            handler: Arc::new(Mutex::new(None)),
            delegate: Mutex::new(None),
            delivered: Mutex::new(Vec::new()),
        };
        notifier.request_authorization();
        Ok(notifier)
    }

    /// Ask once. The result arrives asynchronously; a denial surfaces on the
    /// next `notify` as the system dropping the request, and
    /// `getNotificationSettings` is the way to read it back.
    fn request_authorization(&self) {
        let block = RcBlock::new(|_granted: objc2::runtime::Bool, _error: *mut NSError| {});
        self.center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &block,
        );
    }
}

impl Notifier for MacNotifier {
    fn capability(&self) -> Support {
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            return Support::Missing(Unavailable::not_implemented(
                "no CFBundleIdentifier: UserNotifications is unavailable to an unbundled binary",
            ));
        }
        Support::Available
    }

    fn notify(&self, notification: &Notification) -> Result<NotificationHandle, Unavailable> {
        let plan = notification.mac_plan();

        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&plan.title));
        content.setBody(&NSString::from_str(&plan.body));
        content.setThreadIdentifier(&NSString::from_str(&plan.thread_identifier));
        content.setInterruptionLevel(match plan.interruption_level {
            MacInterruptionLevel::Active => UNNotificationInterruptionLevel::Active,
            MacInterruptionLevel::TimeSensitive => UNNotificationInterruptionLevel::TimeSensitive,
        });

        let keys: Vec<Retained<NSString>> =
            plan.user_info.iter().map(|(k, _)| NSString::from_str(k)).collect();
        let values: Vec<Retained<NSString>> =
            plan.user_info.iter().map(|(_, v)| NSString::from_str(v)).collect();
        let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let value_refs: Vec<&NSString> = values.iter().map(|v| &**v).collect();
        let info = NSDictionary::from_slices(&key_refs, &value_refs);
        // SAFETY: the dictionary is `NSString -> NSString`, which is a valid
        // property-list userInfo payload.
        unsafe {
            let untyped: Retained<NSDictionary> = Retained::cast_unchecked(info);
            content.setUserInfo(&untyped);
        }

        let identifier = NSString::from_str(&plan.identifier);
        let request =
            UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
        self.center.addNotificationRequest_withCompletionHandler(&request, None);

        let mut delivered =
            self.delivered.lock().expect("delivered list is never held across a panic");
        delivered.push(plan.identifier);
        Ok(NotificationHandle(delivered.len() as u64 - 1))
    }

    fn close(&self, handle: NotificationHandle) -> Result<(), Unavailable> {
        let delivered =
            self.delivered.lock().expect("delivered list is never held across a panic");
        let Some(identifier) = delivered.get(handle.0 as usize) else {
            return Err(Unavailable::runtime_error(format!(
                "no delivered notification with handle {}",
                handle.0
            )));
        };
        let identifier = NSString::from_str(identifier);
        let array = objc2_foundation::NSArray::from_slice(&[&*identifier]);
        self.center.removeDeliveredNotificationsWithIdentifiers(&array);
        Ok(())
    }

    fn set_activation_handler(&self, handler: ActivationHandler) -> Result<(), Unavailable> {
        *self.handler.lock().expect("handler slot is never held across a panic") = Some(handler);
        let mut slot = self.delegate.lock().expect("delegate is never held across a panic");
        if slot.is_none() {
            let this = NotificationDelegate::alloc()
                .set_ivars(DelegateIvars { handler: Arc::clone(&self.handler) });
            // SAFETY: `NSObject`'s designated initialiser on a freshly
            // allocated instance of our own subclass.
            let delegate: Retained<NotificationDelegate> = unsafe { msg_send![super(this), init] };
            self.center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            *slot = Some(delegate);
        }
        Ok(())
    }
}
