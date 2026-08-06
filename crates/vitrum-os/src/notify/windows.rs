//! WinRT toast notifications.
//!
//! `ToastNotificationManager` refuses to create a notifier for an
//! AppUserModelID it cannot resolve to a Start Menu entry. That is not a
//! limitation to work around, it is how Windows attributes a toast to an app,
//! so the install step is documented in [`AUMID_INSTALL_NOTE`] and a missing
//! shortcut is reported as unavailable rather than swallowed.
//!
//! Activation arrives on the `Activated` event carrying the `launch` string we
//! put in the XML, which is a `vitrum://session/<id>` URL, so the click path
//! goes through the same parser as a browser-issued deep link.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastNotification, ToastNotificationManager,
};
use windows::core::{HSTRING, IInspectable, Interface};

use crate::branding::BUNDLE_ID;
use crate::capability::{Support, Unavailable};
use crate::deeplink::{self, DeepLink};
use crate::notify::{ActivationHandler, Notification, NotificationHandle, Notifier};

/// What an installer must do for toasts to work at all.
pub const AUMID_INSTALL_NOTE: &str = "Windows attributes a toast to an AppUserModelID that \
resolves to a Start Menu shortcut. The installer must create \
%APPDATA%\\Microsoft\\Windows\\Start Menu\\Programs\\Vitrum.lnk with its \
System.AppUserModel.ID property set to dev.santhreal.vitrum, pointing at the \
installed executable. Without it CreateToastNotifierWithId fails and no toast \
is ever shown.";

/// Group name for every toast this app raises, so `History` can clear them.
const TOAST_GROUP: &str = "vitrum-sessions";

pub struct ToastNotifier {
    handler: Arc<Mutex<Option<ActivationHandler>>>,
    /// Live toasts keyed by handle. Held because the `Activated` event stops
    /// firing once the `ToastNotification` is released.
    live: Mutex<HashMap<u64, ToastNotification>>,
    next_handle: AtomicU64,
}

impl ToastNotifier {
    /// Prove the AUMID resolves before claiming the feature works.
    pub fn connect() -> Result<Self, Unavailable> {
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(BUNDLE_ID)).map_err(
            |e| {
                Unavailable::service_missing(format!(
                    "CreateToastNotifierWithId({BUNDLE_ID}) failed: {e}. {AUMID_INSTALL_NOTE}"
                ))
            },
        )?;
        Ok(Self {
            handler: Arc::new(Mutex::new(None)),
            live: Mutex::new(HashMap::new()),
            next_handle: AtomicU64::new(0),
        })
    }
}

impl Notifier for ToastNotifier {
    fn capability(&self) -> Support {
        match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(BUNDLE_ID)) {
            Ok(_) => Support::Available,
            Err(e) => Support::Missing(Unavailable::service_missing(format!(
                "CreateToastNotifierWithId({BUNDLE_ID}) failed: {e}. {AUMID_INSTALL_NOTE}"
            ))),
        }
    }

    fn notify(&self, notification: &Notification) -> Result<NotificationHandle, Unavailable> {
        let doc = XmlDocument::new()
            .map_err(|e| Unavailable::runtime_error(format!("XmlDocument::new failed: {e}")))?;
        doc.LoadXml(&HSTRING::from(notification.toast_xml())).map_err(|e| {
            Unavailable::runtime_error(format!("toast XML rejected by the parser: {e}"))
        })?;

        let toast = ToastNotification::CreateToastNotification(&doc).map_err(|e| {
            Unavailable::runtime_error(format!("CreateToastNotification failed: {e}"))
        })?;

        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        let tag = format!("s{}", notification.session.0);
        toast
            .SetTag(&HSTRING::from(&tag))
            .and_then(|()| toast.SetGroup(&HSTRING::from(TOAST_GROUP)))
            .map_err(|e| Unavailable::runtime_error(format!("cannot tag toast: {e}")))?;

        let handler = Arc::clone(&self.handler);
        toast
            .Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
                move |_, args: windows::core::Ref<'_, IInspectable>| {
                    let Some(args) = args.as_ref() else { return Ok(()) };
                    let Ok(args) = args.cast::<ToastActivatedEventArgs>() else {
                        return Ok(());
                    };
                    let Ok(argument) = args.Arguments() else { return Ok(()) };
                    if let Ok(DeepLink::Session(session)) = deeplink::parse(&argument.to_string()) {
                        let handler = handler
                            .lock()
                            .expect("handler slot is never held across a panic")
                            .clone();
                        if let Some(handler) = handler {
                            handler(session);
                        }
                    }
                    Ok(())
                },
            ))
            .map_err(|e| {
                Unavailable::runtime_error(format!("cannot attach toast activation handler: {e}"))
            })?;

        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(BUNDLE_ID))
            .and_then(|notifier| notifier.Show(&toast))
            .map_err(|e| Unavailable::runtime_error(format!("Show failed: {e}")))?;

        self.live
            .lock()
            .expect("live toast map is never held across a panic")
            .insert(handle, toast);
        Ok(NotificationHandle(handle))
    }

    fn close(&self, handle: NotificationHandle) -> Result<(), Unavailable> {
        let toast = self
            .live
            .lock()
            .expect("live toast map is never held across a panic")
            .remove(&handle.0);
        let Some(toast) = toast else {
            return Err(Unavailable::runtime_error(format!(
                "no live toast with handle {}",
                handle.0
            )));
        };
        let tag =
            toast.Tag().map_err(|e| Unavailable::runtime_error(format!("cannot read tag: {e}")))?;
        ToastNotificationManager::History()
            .and_then(|history| {
                history.RemoveGroupedTagWithId(
                    &tag,
                    &HSTRING::from(TOAST_GROUP),
                    &HSTRING::from(BUNDLE_ID),
                )
            })
            .map_err(|e| Unavailable::runtime_error(format!("cannot withdraw toast: {e}")))
    }

    fn set_activation_handler(&self, handler: ActivationHandler) -> Result<(), Unavailable> {
        *self.handler.lock().expect("handler slot is never held across a panic") = Some(handler);
        Ok(())
    }
}
