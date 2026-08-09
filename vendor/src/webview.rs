use crate::file_upload::{DesktopFileData, DesktopFileDragEvent};
use crate::menubar::DioxusMenu;
use crate::PendingDesktopContext;
use crate::{
    app::SharedContext, assets::AssetHandlerRegistry, edits::WryQueue,
    file_upload::NativeFileHover, ipc::UserWindowEvent, protocol, waker::tao_waker, Config,
    DesktopContext, DesktopService,
};
use crate::{document::DesktopDocument, WeakDesktopContext};
use crate::{element::DesktopElement, file_upload::DesktopFormData};
use base64::prelude::BASE64_STANDARD;
use dioxus_core::{consume_context, provide_context, Runtime, ScopeId, VirtualDom};
use dioxus_document::Document;
use dioxus_history::{History, MemoryHistory};
use dioxus_hooks::to_owned;
use dioxus_html::{FileData, FormValue, HtmlEvent, PlatformEventData, SerializedFileData};
use futures_util::{pin_mut, FutureExt};
use std::sync::{atomic::AtomicBool, Arc};
use std::{cell::OnceCell, time::Duration};
use std::{rc::Rc, task::Waker};
use wry::{DragDropEvent, RequestAsyncResponder, WebContext, WebViewBuilder, WebViewId};

// ---------------------------------------------------------------------------
// One WebContext for the whole process.
//
// Upstream builds a fresh `WebContext` per webview. On Linux each one starts
// its own `WebKitNetworkProcess`, and they are pure duplication: same cache,
// same cookie jar, same data directory. Measured on this tree, twenty windows
// spent 177.9 MB on twenty copies of one thing.
//
// Two problems have to be solved together, and they are why upstream does not
// already do this:
//
// 1. wry registers custom protocols PER CONTEXT, so `dioxus://` can only be
//    registered once. A second builder on the same context panics with
//    `DuplicateCustomProtocol`.
// 2. The single handler that survives must still serve each webview its OWN
//    page, because `protocol::module_loader` inlines that webview's edit-queue
//    path and server key into the index.html it returns, and `__events` routes
//    DOM events into one specific VirtualDom.
//
// The fix is to give every webview a known id and route on it.
// `wry::WebViewId` is `&str` and `WebViewBuilder::with_id` sets it, so the id
// is decided BEFORE the webview is built and registered before it can make its
// first request. There is no window in which a request arrives for an unknown
// id, which is what makes this sound rather than a race that usually wins.
// ---------------------------------------------------------------------------

/// Everything the shared `dioxus://` handler needs to serve one webview.
#[derive(Clone)]
struct ProtocolRoute {
    edits: WebviewEdits,
    asset_handlers: AssetHandlerRegistry,
    custom_head: Option<String>,
    custom_index: Option<String>,
    root_name: String,
    headless: bool,
}

thread_local! {
    /// Webview id to its route. Thread-local rather than a `Mutex` because
    /// `WebviewEdits` holds an `Rc<Runtime>` and never leaves the event-loop
    /// thread, which is also the only thread that builds webviews.
    static PROTOCOL_ROUTES: std::cell::RefCell<std::collections::HashMap<String, ProtocolRoute>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Serial number for webview ids. Ids are opaque to wry; they only have to be
/// unique and stable for the life of the webview.
static NEXT_WEBVIEW_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Has the `dioxus://` protocol been registered on the shared context yet?
static PROTOCOL_REGISTERED: AtomicBool = AtomicBool::new(false);

/// One `WebContext` for the process, leaked so it outlives every webview.
///
/// Wry requires the context to outlive the webviews built from it, which
/// upstream achieved by parking a copy in a field on each instance. A
/// `&'static mut` is the stronger guarantee, not a weaker one.
///
/// Handing the same `&mut` out repeatedly is sound HERE because webview
/// construction happens on the tao event-loop thread, one at a time, and wry
/// only borrows the context while building.
fn shared_web_context(data_dir: Option<std::path::PathBuf>) -> &'static mut WebContext {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CONTEXT: AtomicUsize = AtomicUsize::new(0);

    let existing = CONTEXT.load(Ordering::Acquire);
    let ptr = if existing == 0 {
        let dir = data_dir.or_else(|| {
            // On Windows, WebView2 defaults to storing its data next to the
            // executable, which fails on drives where that is not writable.
            if cfg!(windows) {
                let exe = std::env::current_exe().ok()?;
                let name = exe.file_stem()?.to_str()?;
                let local_app_data = std::env::var("LOCALAPPDATA").ok()?;
                Some(std::path::PathBuf::from(local_app_data).join(name))
            } else {
                None
            }
        });
        let fresh = Box::into_raw(Box::new(WebContext::new(dir))) as usize;
        CONTEXT.store(fresh, Ordering::Release);
        fresh
    } else {
        existing
    };
    // SAFETY: the pointer comes from `Box::into_raw` and is never freed, so it
    // is valid for the life of the process. Uniqueness of the `&mut` is the
    // event-loop-thread invariant described above.
    unsafe { &mut *(ptr as *mut WebContext) }
}

// ---------------------------------------------------------------------------
// One WebKitWebProcess for the whole application.
//
// Sharing the `WebContext` above collapsed twenty `WebKitNetworkProcess`
// copies into one. It did nothing about the far larger duplication next to it:
// WebKitGTK gives every webview its own `WebKitWebProcess`, and each one
// carries a full engine before our page contributes a byte. Measured on this
// machine with WebKitGTK 2.52.3, twenty independent views holding our
// stylesheets and a live terminal spent 910.4 MB across twenty processes,
// 45.5 MB each.
//
// WebKit's own answer is `webkit_web_view_new_with_related_view`: a view built
// as *related* to another runs inside that view's web process instead of
// spawning one. wry exposes it as `WebViewBuilderExtUnix::with_related_view`,
// so this needs no patch to wry, only somebody to hold a relation target. The
// same twenty pages built as related views measured 270.6 MB in a single
// process, 639.8 MB less for identical content, and the whole application went
// from 1101.0 MB to 399.9 MB at twenty windows.
//
// The cost is shared fate: a web process that dies takes every window with it
// rather than one. That is the trade WebKit's own API exists to make, and it
// is the same bargain every tabbed browser already makes.
//
// **The target has to be alive.** The first version of this kept one handle to
// the first view forever, on the reasoning that a GObject reference keeps it
// valid. That was wrong, and measurably so: close the first window, open
// another, and WebKit declines to reuse the dead view's process and starts a
// second one. Twenty windows came back as 539.6 MB instead of 399.9. Left
// alone it leaks one process per open-and-close generation.
//
// So this keeps every view built and picks the first whose widget still has a
// parent, dropping the rest as it goes. A destroyed webview is unparented by
// GTK, which makes `parent()` the liveness question we actually need answered.
#[cfg(target_os = "linux")]
thread_local! {
    static LIVE_WEBKIT_VIEWS: std::cell::RefCell<Vec<webkit2gtk::WebView>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// A webview that is still attached to a widget tree, for use as the relation
/// target of the next one. Views whose window has closed are dropped here.
#[cfg(target_os = "linux")]
fn relation_target() -> Option<webkit2gtk::WebView> {
    use gtk::prelude::WidgetExt;
    LIVE_WEBKIT_VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        views.retain(|view| view.parent().is_some());
        views.first().cloned()
    })
}

/// Register a freshly built webview as a candidate relation target.
#[cfg(target_os = "linux")]
fn register_webkit_view(view: webkit2gtk::WebView) {
    LIVE_WEBKIT_VIEWS.with(|views| views.borrow_mut().push(view));
}

#[derive(Clone)]
pub(crate) struct WebviewEdits {
    runtime: Rc<Runtime>,
    pub wry_queue: WryQueue,
    desktop_context: Rc<OnceCell<WeakDesktopContext>>,
}

impl WebviewEdits {
    fn new(runtime: Rc<Runtime>, wry_queue: WryQueue) -> Self {
        Self {
            runtime,
            wry_queue,
            desktop_context: Default::default(),
        }
    }

    fn set_desktop_context(&self, context: WeakDesktopContext) {
        _ = self.desktop_context.set(context);
    }

    pub fn handle_event(
        &self,
        request: wry::http::Request<Vec<u8>>,
        responder: wry::RequestAsyncResponder,
    ) {
        let body = self
            .try_handle_event(request)
            .expect("Writing bodies to succeed");
        responder.respond(wry::http::Response::new(body))
    }

    pub fn try_handle_event(
        &self,
        request: wry::http::Request<Vec<u8>>,
    ) -> Result<Vec<u8>, serde_json::Error> {
        use serde::de::Error;

        // todo(jon):
        //
        // I'm a small bit worried about the size of the header being too big on some platforms.
        // It's unlikely we'll hit the 256k limit (from 2010 browsers...) but it's important to think about
        // https://stackoverflow.com/questions/3326210/can-http-headers-be-too-big-for-browsers
        //
        // Also important to remember here that we don't pass a body from the JavaScript side of things
        let data = request
            .headers()
            .get("dioxus-data")
            .ok_or_else(|| Error::custom("dioxus-data header not set"))?;

        let as_utf = std::str::from_utf8(data.as_bytes())
            .map_err(|_| Error::custom("dioxus-data header is not a valid (utf-8) string"))?;

        let data_from_header = base64::Engine::decode(&BASE64_STANDARD, as_utf)
            .map_err(|_| Error::custom("dioxus-data header is not a base64 string"))?;

        let response = match serde_json::from_slice(&data_from_header) {
            Ok(event) => {
                // we need to wait for the mutex lock to let us munge the main thread..
                #[cfg(target_os = "android")]
                let _lock = crate::android_sync_lock::android_runtime_lock();
                self.handle_html_event(event)
            }
            Err(err) => {
                tracing::error!(
                    "Error parsing user_event: {:?}. \n Contents: {:?}, \nraw: {:#?}",
                    err,
                    String::from_utf8(request.body().to_vec()),
                    request
                );
                SynchronousEventResponse::new(false)
            }
        };

        serde_json::to_vec(&response).inspect_err(|err| {
            tracing::error!("failed to serialize SynchronousEventResponse: {err:?}");
        })
    }

    pub fn handle_html_event(&self, event: HtmlEvent) -> SynchronousEventResponse {
        let HtmlEvent {
            element,
            name,
            bubbles,
            data,
        } = event;
        let Some(desktop_context) = self.desktop_context.get() else {
            tracing::error!(
                "Tried to handle event before setting the desktop context on the event handler"
            );
            return Default::default();
        };

        let desktop_context = desktop_context.upgrade().unwrap();

        let query = desktop_context.query.clone();
        let hovered_file = desktop_context.file_hover.clone();

        // check for a mounted event placeholder and replace it with a desktop specific element
        let as_any = match data {
            dioxus_html::EventData::Mounted => {
                let element = DesktopElement::new(element, desktop_context.clone(), query.clone());
                Rc::new(PlatformEventData::new(Box::new(element)))
            }
            dioxus_html::EventData::Form(form) => {
                Rc::new(PlatformEventData::new(Box::new(DesktopFormData {
                    value: form.value,
                    valid: form.valid,
                    values: form
                        .values
                        .into_iter()
                        .map(|obj| {
                            if let Some(text) = obj.text {
                                return (obj.key, FormValue::Text(text));
                            }

                            if let Some(file_data) = obj.file {
                                if file_data.path.capacity() == 0 {
                                    return (obj.key, FormValue::File(None));
                                }

                                return (
                                    obj.key,
                                    FormValue::File(Some(FileData::new(DesktopFileData(
                                        file_data.path,
                                    )))),
                                );
                            };

                            (obj.key, FormValue::Text(String::new()))
                        })
                        .collect(),
                })))
            }
            // Which also includes drops...
            dioxus_html::EventData::Drag(ref drag) => {
                // we want to override this with a native file engine, provided by the most recent drag event
                let full_file_paths = hovered_file.current_paths();

                let xfer_data = drag.data_transfer.clone();
                let new_file_data = xfer_data
                    .files
                    .iter()
                    .map(|f| {
                        let new_path = full_file_paths
                            .iter()
                            .find(|p| p.ends_with(&f.path))
                            .unwrap_or(&f.path);
                        SerializedFileData {
                            path: new_path.clone(),
                            ..f.clone()
                        }
                    })
                    .collect::<Vec<_>>();
                let new_xfer_data = dioxus_html::SerializedDataTransfer {
                    files: new_file_data,
                    ..xfer_data
                };

                Rc::new(PlatformEventData::new(Box::new(DesktopFileDragEvent {
                    mouse: drag.mouse.clone(),
                    data_transfer: new_xfer_data,
                    files: full_file_paths,
                })))
            }
            _ => data.into_any(),
        };

        let event = dioxus_core::Event::new(as_any, bubbles);
        self.runtime.handle_event(&name, event.clone(), element);

        // Get the response from the event
        SynchronousEventResponse::new(!event.default_action_enabled())
    }
}

pub(crate) struct WebviewInstance {
    pub dom: VirtualDom,
    pub edits: WebviewEdits,
    pub desktop_context: DesktopContext,
    pub waker: Waker,

    // The WebContext is NOT held here: there is one for the process, leaked so
    // it outlives every webview by construction, which is a stronger guarantee
    // than this field gave. See `shared_web_context`.

    // Same with the menu.
    // Currently it's a DioxusMenu because 1) we don't touch it and 2) we support a number of platforms
    // like ios where muda does not give us a menu type. It sucks but alas.
    //
    // This would be a good thing for someone looking to contribute to fix.
    _menu: Option<DioxusMenu>,
}

impl WebviewInstance {
    pub(crate) fn new(
        mut cfg: Config,
        mut dom: VirtualDom,
        shared: Rc<SharedContext>,
    ) -> WebviewInstance {
        let mut window = cfg.window.clone();

        // tao makes small windows for some reason, make them bigger on desktop
        //
        // on mobile, we want them to be `None` so tao makes them the size of the screen. Otherwise we
        // get a window that is not the size of the screen and weird black bars.
        #[cfg(not(any(target_os = "ios", target_os = "android")))]
        {
            if cfg.window.window.inner_size.is_none() {
                window = window.with_inner_size(tao::dpi::LogicalSize::new(800.0, 600.0));
            }
        }

        // We assume that if the icon is None in cfg, then the user just didnt set it
        if cfg.window.window.window_icon.is_none() {
            window = window.with_window_icon(crate::default_icon().ok());
        }

        let window = Arc::new(window.build(&shared.target).unwrap());
        if let Some(on_build) = cfg.on_window.as_mut() {
            on_build(window.clone(), &mut dom);
        }

        // https://developer.apple.com/documentation/appkit/nswindowcollectionbehavior/nswindowcollectionbehaviormanaged
        #[cfg(target_os = "macos")]
        #[allow(deprecated)]
        {
            use cocoa::appkit::NSWindowCollectionBehavior;
            use cocoa::base::id;
            use objc::{msg_send, sel, sel_impl};
            use tao::platform::macos::WindowExtMacOS;

            unsafe {
                let window: id = window.ns_window() as id;
                let _: () = msg_send![window, setCollectionBehavior: NSWindowCollectionBehavior::NSWindowCollectionBehaviorManaged];
            }
        }

        let web_context = shared_web_context(cfg.data_dir.clone());
        let edit_queue = shared.websocket.create_queue();
        let asset_handlers = AssetHandlerRegistry::new();
        let edits = WebviewEdits::new(dom.runtime(), edit_queue.clone());
        let file_hover = NativeFileHover::default();
        let headless = !cfg.window.window.visible;

        // Decide this webview's id and register its route BEFORE the webview
        // exists, so no request can arrive for an id the dispatcher does not
        // know.
        let webview_id = format!(
            "dx{}",
            NEXT_WEBVIEW_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        PROTOCOL_ROUTES.with(|routes| {
            routes.borrow_mut().insert(
                webview_id.clone(),
                ProtocolRoute {
                    edits: edits.clone(),
                    asset_handlers: asset_handlers.clone(),
                    custom_head: cfg.custom_head.clone(),
                    custom_index: cfg.custom_index.clone(),
                    root_name: cfg.root_name.clone(),
                    headless,
                },
            );
        });

        // Registered ONCE on the shared context. A second registration panics
        // with `DuplicateCustomProtocol`, so every later webview relies on this
        // dispatcher and on having put its route in the map above.
        let request_handler = {
            #[cfg(feature = "tokio_runtime")]
            let tokio_rt = tokio::runtime::Handle::current();

            move |id: WebViewId, request, responder: RequestAsyncResponder| {
                #[cfg(feature = "tokio_runtime")]
                let _guard = tokio_rt.enter();

                let route = PROTOCOL_ROUTES.with(|routes| routes.borrow().get(id).cloned());
                let Some(route) = route else {
                    // Only reachable if a webview outlives its route, which
                    // nothing removes. Answering rather than hanging the load.
                    tracing::error!("no dioxus:// route for webview {id:?}");
                    responder.respond(
                        wry::http::Response::builder()
                            .status(wry::http::StatusCode::INTERNAL_SERVER_ERROR)
                            .body(Vec::new())
                            .unwrap(),
                    );
                    return;
                };

                protocol::desktop_handler(
                    request,
                    route.asset_handlers.clone(),
                    responder,
                    &route.edits,
                    route.custom_head.clone(),
                    route.custom_index.clone(),
                    &route.root_name,
                    route.headless,
                )
            }
        };

        let ipc_handler = {
            let window_id = window.id();
            to_owned![shared.proxy];
            move |payload: wry::http::Request<String>| {
                // defer the event to the main thread
                let body = payload.into_body();
                if let Ok(msg) = serde_json::from_str(&body) {
                    _ = proxy.send_event(UserWindowEvent::Ipc { id: window_id, msg });
                }
            }
        };

        let file_drop_handler = {
            to_owned![file_hover];
            let (proxy, window_id) = (shared.proxy.to_owned(), window.id());
            move |evt: DragDropEvent| {
                if cfg!(not(windows)) {
                    // Update the most recent file drop event - when the event comes in from the webview we can use the
                    // most recent event to build a new event with the files in it.
                    file_hover.set(evt);
                } else {
                    // Windows webview blocks HTML-native events when the drop handler is provided.
                    // The problem is that the HTML-native events don't provide the file, so we need this.
                    // Solution: this glue code to mimic drag drop events.
                    file_hover.set(evt.clone());
                    match evt {
                        wry::DragDropEvent::Drop {
                            paths: _,
                            position: _,
                        } => {
                            _ = proxy.send_event(UserWindowEvent::WindowsDragDrop(window_id));
                        }
                        wry::DragDropEvent::Over { position } => {
                            _ = proxy.send_event(UserWindowEvent::WindowsDragOver(
                                window_id, position.0, position.1,
                            ));
                        }
                        wry::DragDropEvent::Leave => {
                            _ = proxy.send_event(UserWindowEvent::WindowsDragLeave(window_id));
                        }
                        _ => {}
                    }
                }

                false
            }
        };

        let navigation_handler = cfg.navigation_handler.take();
        let page_loaded = AtomicBool::new(false);

        let mut webview = WebViewBuilder::new_with_web_context(web_context)
            .with_bounds(wry::Rect {
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                size: wry::dpi::Size::Physical(wry::dpi::PhysicalSize::new(
                    window.inner_size().width,
                    window.inner_size().height,
                )),
            })
            .with_transparent(cfg.window.window.transparent)
            .with_url("dioxus://index.html/")
            .with_ipc_handler(ipc_handler)
            .with_navigation_handler(move |var| {
                // Serve the index and assets.
                if var.starts_with("dioxus://")
                    || var.starts_with("http://dioxus.")
                    || var.starts_with("https://dioxus.")
                {
                    // After the page has loaded once, don't allow any more navigation
                    let page_loaded = page_loaded.swap(true, std::sync::atomic::Ordering::SeqCst);
                    return !page_loaded;
                }

                // External links always open somewhere else. Prevents the webview from navigating
                if var.starts_with("http://")
                    || var.starts_with("https://")
                    || var.starts_with("mailto:")
                {
                    _ = webbrowser::open(&var);
                    return false;
                }

                // By default, external links are allowed. This keeps things like iframes working.
                // However, users can customize this to allow/disallow domains/routes/patterns.
                navigation_handler.as_ref().map(|f| f(&var)).unwrap_or(true)
            })
            .with_id(&webview_id);

        // Once only: the protocol lives on the shared context, and registering
        // it twice is the `DuplicateCustomProtocol` panic. Later webviews are
        // served by the dispatcher the first one installed, which finds them
        // through the id set above.
        if !PROTOCOL_REGISTERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            webview = webview.with_asynchronous_custom_protocol(
                String::from("dioxus"),
                request_handler,
            );
        }

        // Enable https scheme on android, needed for secure context API, like the geolocation API
        #[cfg(target_os = "android")]
        {
            use wry::WebViewBuilderExtAndroid as _;

            webview = webview.with_https_scheme(true);
        };

        // Disable the webview default shortcuts to disable the reload shortcut
        #[cfg(target_os = "windows")]
        {
            use wry::WebViewBuilderExtWindows;
            webview = webview.with_browser_accelerator_keys(false);
        }

        if !cfg.disable_file_drop_handler {
            webview = webview.with_drag_drop_handler(file_drop_handler);
        }

        // Not on Linux: wry hands WebKitGTK a colour it cannot mean.
        //
        // `wry::webkitgtk` builds the colour with
        // `gdk::RGBA::new(red as _, green as _, blue as _, alpha as _)` from
        // the `u8` quadruple. A GDK channel is an `f64` on 0.0..=1.0, so
        // `(6, 6, 8, 255)` arrives as `RGBA(6.0, 6.0, 8.0, 255.0)` and clamps
        // to opaque WHITE. Every background colour that is not black paints
        // white there, which is the opposite of what the setting is for, and
        // the public `WebView::set_background_color` scales the same way and
        // is no escape. The colour is applied below instead, once the view
        // exists, in the units GDK actually reads.
        #[cfg(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        ))]
        if let Some(color) = cfg.background_color {
            webview = webview.with_background_color(color);
        }

        for (name, handler) in cfg.protocols.drain(..) {
            #[cfg(feature = "tokio_runtime")]
            let tokio_rt = tokio::runtime::Handle::current();

            webview = webview.with_custom_protocol(name, move |a, b| {
                #[cfg(feature = "tokio_runtime")]
                let _guard = tokio_rt.enter();
                handler(a, b)
            });
        }

        for (name, handler) in cfg.asynchronous_protocols.drain(..) {
            #[cfg(feature = "tokio_runtime")]
            let tokio_rt = tokio::runtime::Handle::current();

            webview = webview.with_asynchronous_custom_protocol(name, move |a, b, c| {
                #[cfg(feature = "tokio_runtime")]
                let _guard = tokio_rt.enter();
                handler(a, b, c)
            });
        }

        const INITIALIZATION_SCRIPT: &str = r#"
        if (document.addEventListener) {
            document.addEventListener('contextmenu', function(e) {
                e.preventDefault();
            }, false);
        } else {
            document.attachEvent('oncontextmenu', function() {
                window.event.returnValue = false;
            });
        }
        "#;

        if cfg.disable_context_menu {
            // in release mode, we don't want to show the dev tool or reload menus
            webview = webview.with_initialization_script(INITIALIZATION_SCRIPT)
        } else {
            // in debug, we are okay with the reload menu showing and dev tool
            webview = webview.with_devtools(true);
        }

        let menu = if cfg!(not(any(target_os = "android", target_os = "ios"))) {
            let menu_option = cfg.menu.into();
            if let Some(menu) = &menu_option {
                crate::menubar::init_menu_bar(menu, &window);
            }
            menu_option
        } else {
            None
        };

        #[cfg(target_os = "windows")]
        {
            use wry::WebViewBuilderExtWindows;
            if let Some(additional_windows_args) = &cfg.additional_windows_args {
                webview = webview.with_additional_browser_args(additional_windows_args);
            }
        }

        #[cfg(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        ))]
        let webview = if cfg.as_child_window {
            webview.build_as_child(&window)
        } else {
            webview.build(&window)
        };

        #[cfg(not(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "ios",
            target_os = "android"
        )))]
        let webview = {
            use tao::platform::unix::WindowExtUnix;
            use wry::{WebViewBuilderExtUnix, WebViewExtUnix};
            // The window wears the webview's background colour from its very
            // first expose.
            //
            // Two surfaces paint before a document does, and this is the
            // first of them. Between the window being mapped and WebKit
            // taking over, what is on screen is the GTK window's own theme
            // background: white under every default light theme. Filmed on a
            // bare X server at roughly thirty frames a second, a dark
            // application was black for its whole launch and then flashed
            // white on its way to being dark again. A flash is the one part
            // of a launch nobody can miss and no timing instrument can see,
            // because it falls between two events that are both on time.
            //
            // A screen-wide provider rather than a per-widget one, because
            // every window this process opens wants the same answer, and
            // because the colour has to be in place before this window is
            // realised rather than applied to each one afterwards.
            if let Some((r, g, b, a)) = cfg.background_color {
                use gtk::prelude::{CssProviderExt, WidgetExt};
                let css = format!(
                    "window, box {{ background-color: rgba({r},{g},{b},{:.3}); }}",
                    f64::from(a) / 255.0
                );
                let provider = gtk::CssProvider::new();
                if provider.load_from_data(css.as_bytes()).is_ok() {
                    if let Some(screen) = window.gtk_window().screen() {
                        gtk::StyleContext::add_provider_for_screen(
                            &screen,
                            &provider,
                            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                        );
                    }
                }
            }
            let vbox = window.default_vbox().unwrap();
            // Every window relates to a still-live view so they all share one
            // WebKitWebProcess. See `LIVE_WEBKIT_VIEWS`.
            #[cfg(target_os = "linux")]
            let webview = match relation_target() {
                Some(target) => webview.with_related_view(target),
                None => webview,
            };
            let built = webview.build_gtk(vbox);
            #[cfg(target_os = "linux")]
            if let Ok(built) = &built {
                register_webkit_view(built.webview());
                // The second surface, and the one that was actually white.
                //
                // Set here rather than through the builder because wry's
                // conversion cannot express this colour; see the note where
                // `with_background_color` is skipped. Applied before the
                // event loop runs, so the view is never asked to paint while
                // it still holds its default.
                if let Some((r, g, b, a)) = cfg.background_color {
                    use webkit2gtk::WebViewExt as _;
                    built.webview().set_background_color(&gtk::gdk::RGBA::new(
                        f64::from(r) / 255.0,
                        f64::from(g) / 255.0,
                        f64::from(b) / 255.0,
                        f64::from(a) / 255.0,
                    ));
                }
            }
            built
        };
        let webview = webview.unwrap();

        let desktop_context = Rc::from(DesktopService::new(
            webview,
            window,
            shared.clone(),
            asset_handlers,
            file_hover,
            cfg.window_close_behavior,
        ));

        // Provide the desktop context to the virtual dom and edit handler
        edits.set_desktop_context(Rc::downgrade(&desktop_context));
        let provider: Rc<dyn Document> = Rc::new(DesktopDocument::new(desktop_context.clone()));
        let history_provider: Rc<dyn History> = Rc::new(MemoryHistory::default());
        dom.in_scope(ScopeId::ROOT, || {
            provide_context(desktop_context.clone());
            provide_context(provider);
            provide_context(history_provider);
        });

        // Request an initial redraw
        desktop_context.window.request_redraw();

        WebviewInstance {
            dom,
            edits,
            waker: tao_waker(shared.proxy.clone(), desktop_context.window.id()),
            desktop_context,
            _menu: menu,
        }
    }

    pub fn poll_vdom(&mut self) {
        let mut cx = std::task::Context::from_waker(&self.waker);

        // Continuously poll the virtualdom until it's pending
        // Wait for work will return Ready when it has edits to be sent to the webview
        // It will return Pending when it needs to be polled again - nothing is ready
        loop {
            // Check if there is a new edit channel we need to send. On IOS,
            // the websocket will be killed when the device is put into sleep. If we
            // find the socket has been closed, we create a new socket and send it to
            // the webview to continue on
            // https://github.com/DioxusLabs/dioxus/issues/4374
            if self
                .edits
                .wry_queue
                .poll_new_edits_location(&mut cx)
                .is_ready()
            {
                _ = self.desktop_context.webview.evaluate_script(&format!(
                    "window.interpreter.waitForRequest(\"{edits_path}\", \"{expected_key}\");",
                    edits_path = self.edits.wry_queue.edits_path(),
                    expected_key = self.edits.wry_queue.required_server_key()
                ));
            }

            // If we're waiting for a render, wait for it to finish before we continue
            let edits_flushed_poll = self.edits.wry_queue.poll_edits_flushed(&mut cx);
            if edits_flushed_poll.is_pending() {
                return;
            }

            {
                // lock the hack-ed in lock sync wry has some thread-safety issues with event handlers and async tasks
                #[cfg(target_os = "android")]
                let _lock = crate::android_sync_lock::android_runtime_lock();
                let fut = self.dom.wait_for_work();
                pin_mut!(fut);

                match fut.poll_unpin(&mut cx) {
                    std::task::Poll::Ready(_) => {}
                    std::task::Poll::Pending => return,
                }
            }

            // lock the hack-ed in lock sync wry has some thread-safety issues with event handlers
            #[cfg(target_os = "android")]
            let _lock = crate::android_sync_lock::android_runtime_lock();

            self.edits
                .wry_queue
                .with_mutation_state_mut(|f| self.dom.render_immediate(f));
            self.edits.wry_queue.send_edits();
        }
    }

    #[cfg(all(feature = "devtools", debug_assertions))]
    pub fn kick_stylsheets(&self) {
        // run eval in the webview to kick the stylesheets by appending a query string
        // we should do something less clunky than this
        _ = self
            .desktop_context
            .webview
            .evaluate_script("window.interpreter.kickAllStylesheetsOnPage()");
    }

    /// Displays a toast to the developer.
    ///
    /// Unused in this tree; kept so the vendored copy stays a minimal diff
    /// against upstream 0.7.10.
    #[allow(dead_code)]
    pub(crate) fn show_toast(
        &self,
        header_text: &str,
        message: &str,
        level: &str,
        duration: Duration,
        after_reload: bool,
    ) {
        let as_ms = duration.as_millis();

        let js_fn_name = match after_reload {
            true => "scheduleDXToast",
            false => "showDXToast",
        };

        _ = self.desktop_context.webview.evaluate_script(&format!(
            r#"
                if (typeof {js_fn_name} !== "undefined") {{
                    window.{js_fn_name}("{header_text}", "{message}", "{level}", {as_ms});
                }}
                "#,
        ));
    }
}

/// A synchronous response to a browser event which may prevent the default browser's action
#[derive(serde::Serialize, Default)]
pub struct SynchronousEventResponse {
    #[serde(rename = "preventDefault")]
    prevent_default: bool,
}

impl SynchronousEventResponse {
    /// Create a new SynchronousEventResponse
    #[allow(unused)]
    pub fn new(prevent_default: bool) -> Self {
        Self { prevent_default }
    }
}

/// A webview that is queued to be created. We can't spawn webviews outside of the main event loop because it may
/// block on windows so we queue them into the shared context and then create them when the main event loop is ready.
pub(crate) struct PendingWebview {
    dom: VirtualDom,
    cfg: Config,
    sender: futures_channel::oneshot::Sender<DesktopContext>,
}

impl PendingWebview {
    pub(crate) fn new(dom: VirtualDom, cfg: Config) -> (Self, PendingDesktopContext) {
        let (sender, receiver) = futures_channel::oneshot::channel();
        let webview = Self { dom, cfg, sender };
        let pending = PendingDesktopContext { receiver };
        (webview, pending)
    }

    pub(crate) fn create_window(self, shared: &Rc<SharedContext>) -> WebviewInstance {
        let window = WebviewInstance::new(self.cfg, self.dom, shared.clone());

        let cx = window
            .dom
            .in_scope(ScopeId::ROOT, consume_context::<Rc<DesktopService>>);
        _ = self.sender.send(cx);

        window
    }
}
