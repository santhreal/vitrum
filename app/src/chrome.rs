//! What every window in this process shares: whether it is see-through, the
//! mark it wears, and the backdrop image the operator chose.
//!
//! There is no document here any more. The window is a GTK toplevel, its
//! styling is a `GtkCssProvider` installed on the display by `shell::style`,
//! and the terminal is a drawing area with a wgpu surface on it.

use std::sync::LazyLock;

use super::*;

/// Whether windows in this process are created see-through.
///
/// Read once, at the first window, and frozen for the life of the process.
/// It is construction-time: an RGBA visual has to be selected before the
/// toplevel is realised, so a value that could change under us would only
/// produce windows that disagree with each other.
///
/// The Appearance tab says as much rather than implying a slider does more
/// than it can: opacity moves live within a window that was created
/// see-through, and the first move from a fully opaque profile needs a new
/// window.
static TRANSLUCENT: LazyLock<bool> = LazyLock::new(|| {
    state::startup_prefs()
        .0
        .settings
        .appearance
        .needs_transparent_window()
});

/// Size of the raster handed to the window manager as this window's icon.
///
/// One raster, because that is all `tao` accepts: it sets a single
/// `_NET_WM_ICON` on X11 and one `HICON` pair on Windows, and both scale what
/// they are given. 128 is the size a HiDPI alt-tab and a Windows jump list ask
/// for, and it downsamples to a legible 16 pixel taskbar entry; handing over a
/// 16 pixel raster instead leaves the alt-tab switcher upscaling eight times.
const WINDOW_ICON_SIZE: u32 = 128;

/// The mark, rasterised once for every window this process opens.
///
/// Not a file, and not a resource: the geometry is compiled in and drawn by
/// [`vitrum_os::mark`], so a window that opens before anything is installed
/// still carries the mark. Without this the window shows whatever generic
/// placeholder the desktop keeps for a program it cannot identify.
///
/// The RASTER is cached rather than the `Icon`, because the raster is plain
/// bytes that cross a thread and an `Icon` is a toolkit type that makes no
/// such promise. That is what lets the prewarm thread pay for it while the
/// main thread is bringing up the toolkit, and it is also what stops the
/// twentieth window redrawing a mark identical to the first nineteen.
static MARK_RASTER: LazyLock<vitrum_os::icon::IconImage> = LazyLock::new(|| {
    MARK_RASTERISATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    vitrum_os::mark::render_mark(WINDOW_ICON_SIZE, vitrum_os::mark::MARK_COLOUR)
});

/// How many times the mark has been drawn for a window icon.
static MARK_RASTERISATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// The number [`MARK_RASTERISATIONS`] is holding.
pub(crate) fn mark_rasterisations() -> usize {
    MARK_RASTERISATIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Draw the mark now, so the thread that needs it does not have to.
pub(crate) fn warm_window_icon() {
    LazyLock::force(&MARK_RASTER);
}

/// Whether windows in this process are created see-through.
pub(crate) fn translucent() -> bool {
    *TRANSLUCENT
}

/// The mark, rasterised once for the process.
///
/// Handed out as the raster rather than as a toolkit icon because the window
/// takes a pixbuf, the tray takes something else, and the bytes are the one
/// form both agree on.
pub(crate) fn mark_raster() -> &'static vitrum_os::icon::IconImage {
    &MARK_RASTER
}

/// The filesystem path out of a backdrop URL path.
///
/// Returns `None` for anything that is not an absolute path, which is what a
/// traversal attempt and a malformed URL both look like from here.
#[cfg(test)]
pub(crate) fn backdrop_path(uri_path: &str) -> Option<std::path::PathBuf> {
    let decoded = percent_decode(uri_path)?;

    // Every URL path carries a leading slash. On Unix that slash is the root
    // and has to stay. On Windows the path under it is `C:\...`, and the slash
    // makes it rooted-but-driveless, which `is_absolute` rejects: leaving it on
    // would refuse every backdrop on Windows and look like the feature simply
    // does not work there.
    #[cfg(windows)]
    let decoded = decoded.strip_prefix('/').unwrap_or(&decoded).to_string();

    let path = std::path::PathBuf::from(&decoded);
    if !path.is_absolute() {
        return None;
    }

    // Components rather than a split on '/': on Windows a traversal is spelled
    // `..\`, which a slash-split never sees. `..` cannot widen what this serves,
    // because the answer is gated on the bytes being an image either way, but a
    // path that needs normalising is a path nobody chose from a picker.
    if path
        .components()
        .any(|c| c == std::path::Component::ParentDir)
    {
        return None;
    }
    Some(path)
}

/// Percent-decode a URL path into a string, or `None` if it is malformed.
#[cfg(test)]
fn percent_decode(s: &str) -> Option<String> {
    let raw = s.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = raw.get(i + 1..i + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(text, 16).ok()?);
            i += 3;
        } else {
            out.push(raw[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The MIME type for `bytes`, by signature, or `None` if it is not an image.
///
/// By content and never by file extension. The extension is attacker-supplied
/// in exactly the case this guards, and the point is to answer only with
/// things that really are images.
#[cfg(test)]
pub(crate) fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    const GIF87: &[u8] = b"GIF87a";
    const GIF89: &[u8] = b"GIF89a";
    if bytes.starts_with(PNG) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(GIF87) || bytes.starts_with(GIF89) {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // SVG is text and has no signature. Refused rather than sniffed: it is a
    // document with scripting and external references, and this one is
    // rendered inside the privileged application page.
    None
}

#[cfg(test)]
mod tests;
