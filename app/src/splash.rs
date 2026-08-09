//! The mark, drawn on the window itself, before there is a document to draw it.
//!
//! # Why this is not the loading screen in the head
//!
//! There is one in the document too, and it is unreachable in the case that
//! matters. It cannot paint before the webview does, the webview paints late
//! in a launch, and by then the application's own first frame is a few tens of
//! milliseconds away. Filmed on a bare X server, the interval it could ever
//! occupy was 143 ms wide and its timer waits 400. The interval a person
//! actually sees is the one BEFORE the webview exists, and nothing inside the
//! webview can reach it by construction.
//!
//! So the mark is painted by the window, on the surface the window already
//! puts up while the webview is still being built. That surface is drawn
//! within about 100 ms of exec; the document arrives around 900. This is what
//! stands there in between.
//!
//! # Why it draws rather than being a widget
//!
//! A widget in the window's box would take a share of the box and shrink the
//! webview, and taking it away again once content arrived would mean watching
//! the webview for a load signal from the wrong side of the abstraction. A
//! draw handler owns no layout and needs no teardown: the webview covers it as
//! soon as it has something to show, and the handler goes quiet because
//! nothing asks the window to redraw its own background any more.

/// Draw the mark on this window's own surface until something covers it.
///
/// Connected AFTER the default handler, which is what makes it visible: the
/// default handler is where GTK paints the window's CSS background, so a
/// handler that ran first would be painted over by it.
///
/// Silent on failure, in every case. A launch that cannot compose a 128 pixel
/// image is still a launch, and the surface underneath is already the right
/// colour; refusing to open a window over it would be the only worse answer.
#[cfg(target_os = "linux")]
pub(crate) fn install(window: &vitrum_dioxus_desktop::tao::window::Window) {
    use gtk::glib::object::ObjectExt;
    use gtk::glib::value::ToValue;
    use vitrum_dioxus_desktop::tao::platform::unix::WindowExtUnix;

    window
        .gtk_window()
        .connect_local("draw", true, move |values| {
            let widget: gtk::Widget = values.first()?.get().ok()?;
            let cr: gtk::cairo::Context = values.get(1)?.get().ok()?;
            if !retired(&widget) {
                paint(&widget, &cr);
            }
            // `false` is "the drawing is not finished with", which lets every
            // child of the window draw as usual. Returning `true` here would
            // stop the webview being painted at all.
            Some(false.to_value())
        });
}

/// Whether the webview has taken the surface, so the mark is finished.
///
/// This handler runs AFTER the window's default one, which is the only way it
/// can be seen at all: the default handler is where the CSS background and
/// every child are drawn, so a handler that ran first would be painted over.
/// Drawing last is also why it has to stop deliberately. Nothing takes the
/// mark down on its own, and a splash that never retires is not a splash, it
/// is a diamond stamped over the running application.
///
/// The condition is the webview being mapped rather than a timer or a message
/// from the document. A mapped `WebKitWebView` is drawing, whatever it has
/// managed to draw so far, and it is the same widget that will hold the first
/// frame; a timer would be a guess about a machine we are not on, and a
/// message from the document arrives on a path that only exists once the
/// document does.
#[cfg(target_os = "linux")]
fn retired(window: &gtk::Widget) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};

    static RETIRED: AtomicBool = AtomicBool::new(false);
    if RETIRED.load(Ordering::Relaxed) {
        return true;
    }
    if !holds_a_mapped_webview(window) {
        return false;
    }
    RETIRED.store(true, Ordering::Relaxed);
    true
}

/// Depth-first search for a mapped WebKit view under this widget.
///
/// By type name, because the widget arrives through GTK rather than through
/// wry and this crate never names WebKit's Rust types. The tree is a window, a
/// box and its handful of children, so the walk is over before it starts.
#[cfg(target_os = "linux")]
fn holds_a_mapped_webview(widget: &gtk::Widget) -> bool {
    use gtk::glib::object::{Cast, ObjectExt};
    use gtk::prelude::{ContainerExt, WidgetExt};

    if widget.type_().name().contains("WebKitWebView") && widget.is_mapped() {
        return true;
    }
    let Ok(container) = widget.clone().downcast::<gtk::Container>() else {
        return false;
    };
    container.children().iter().any(holds_a_mapped_webview)
}

/// The mark, centred on the widget, in its own colour.
#[cfg(target_os = "linux")]
fn paint(widget: &gtk::Widget, cr: &gtk::cairo::Context) {
    use gtk::prelude::WidgetExt;

    let Some(surface) = mark_surface() else {
        return;
    };
    let x = f64::from(widget.allocated_width() - MARK_PX as i32) / 2.0;
    let y = f64::from(widget.allocated_height() - MARK_PX as i32) / 2.0;
    if cr.set_source_surface(&surface, x, y).is_ok() {
        let _ = cr.paint();
    }
}

/// How large the mark is drawn, in device pixels.
///
/// The same 96 the document's own loading screen uses, so the two never
/// disagree about how big the mark is on a launch slow enough to show both.
#[cfg(target_os = "linux")]
const MARK_PX: u32 = 96;

/// The mark as cairo wants it: premultiplied, native-endian ARGB32.
///
/// Rasterised once and kept as plain bytes rather than as a surface, because a
/// `cairo::ImageSurface` is not shareable and this is read from whichever
/// thread GTK happens to be drawing on. Composing the surface around the bytes
/// costs one copy of 36 KiB, on a path that runs a handful of times per launch
/// and never once the webview has covered it.
#[cfg(target_os = "linux")]
static MARK_ARGB32: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
    let img = vitrum_os::mark::render_mark(MARK_PX, vitrum_os::mark::MARK_COLOUR);
    let mut out = Vec::with_capacity(img.rgba.len());
    for px in img.rgba.chunks_exact(4) {
        let a = u32::from(px[3]);
        let pm = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
        // Cairo reads ARGB32 as a native-endian `u32`, so on a little-endian
        // machine the bytes go down in the order blue, green, red, alpha.
        out.extend_from_slice(&[pm(px[2]), pm(px[1]), pm(px[0]), px[3]]);
    }
    out
});

/// A cairo surface over [`MARK_ARGB32`].
#[cfg(target_os = "linux")]
fn mark_surface() -> Option<gtk::cairo::ImageSurface> {
    let width = MARK_PX as i32;
    gtk::cairo::ImageSurface::create_for_data(
        MARK_ARGB32.clone(),
        gtk::cairo::Format::ARgb32,
        width,
        width,
        width * 4,
    )
    .ok()
}

/// Nothing to draw on: every other platform reaches its first frame through a
/// path this module knows nothing about.
#[cfg(not(target_os = "linux"))]
pub(crate) fn install(_window: &vitrum_dioxus_desktop::tao::window::Window) {}
