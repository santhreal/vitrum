//! Creating the toplevel.
//!
//! One function, and everything the window manager is told about a window
//! before it is shown: its size, its position, its floor, its icon, whether it
//! is see-through, and that it draws its own decoration.
//!
//! # Why the geometry is applied before the window is shown
//!
//! A window that is shown and then moved is a window the operator sees in two
//! places. GTK applies `move` and `resize` to an unmapped toplevel as hints
//! the window manager honours at map time, so a restored window comes up where
//! it was rather than jumping there.
//!
//! # Why there is no decoration
//!
//! The titlebar is a panel in the frame, with the session and the window
//! controls in it. Two titlebars is what a decorated toplevel would give.

use gtk::gdk;
use gtk::prelude::*;
use vitrum_os::window_state::WindowState as WindowGeometry;

use crate::geometry::MIN_WINDOW_CSS;

/// Build the toplevel for one window slot.
///
/// `scale` is the operator's UI scale. The floor a window may not be dragged
/// below is a CSS measurement and GTK wants logical pixels, which is the same
/// unit once the UI scale is applied: the toolkit's own scale factor is
/// already divided out of everything GTK is told.
pub(crate) fn create(state: &WindowGeometry, scale: f64) -> gtk::Window {
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("vitrum");
    window.set_app_paintable(true);
    // The frame draws the whole surface, controls included.
    window.set_decorated(false);
    window.set_default_size(state.width as i32, state.height as i32);
    window.move_(state.x, state.y);
    window.set_size_request(
        (MIN_WINDOW_CSS.0 * scale) as i32,
        (MIN_WINDOW_CSS.1 * scale) as i32,
    );
    if state.maximized {
        window.maximize();
    }
    if let Some(icon) = icon() {
        window.set_icon(Some(&icon));
    }
    if crate::chrome::translucent() {
        make_translucent(&window);
    }
    window
}

/// Give the window an RGBA visual so the compositor blends it.
///
/// Only when the profile asked for it. A transparent window needs a
/// compositor, and on a bare window manager without one the alpha channel is
/// not blended with anything: the operator gets whatever was in the
/// framebuffer. An opaque profile must never be exposed to that, and a screen
/// with no RGBA visual is the same case.
fn make_translucent(window: &gtk::Window) {
    let Some(screen) = gdk::Screen::default() else {
        return;
    };
    if !screen.is_composited() {
        tracing::info!("no compositor on this screen; opening the window opaque");
        return;
    }
    let Some(visual) = screen.rgba_visual() else {
        tracing::info!("no rgba visual on this screen; opening the window opaque");
        return;
    };
    window.set_visual(Some(&visual));
}

/// The mark, as a pixbuf the window manager can take.
///
/// `None` only if the rasteriser hands back a buffer whose length disagrees
/// with its dimensions. A window with the generic icon is the right answer to
/// that; refusing to open is not.
fn icon() -> Option<gtk::gdk_pixbuf::Pixbuf> {
    let img = crate::chrome::mark_raster();
    let width = i32::try_from(img.width).ok()?;
    let height = i32::try_from(img.height).ok()?;
    let stride = width.checked_mul(4)?;
    if img.rgba.len() != (stride as usize) * (height as usize) {
        return None;
    }
    Some(gtk::gdk_pixbuf::Pixbuf::from_bytes(
        &glib::Bytes::from(&img.rgba),
        gtk::gdk_pixbuf::Colorspace::Rgb,
        true,
        8,
        width,
        height,
        stride,
    ))
}
