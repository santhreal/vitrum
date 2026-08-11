//! The web document a window is built around: stylesheets, head, and the
//! window configuration that hosts them.
//!
//! The document is markup and stylesheets. It carries no script element and
//! no script source: the terminal is a native GTK widget with a wgpu surface
//! on it, so nothing in the page needs an escape-sequence parser, a renderer,
//! a keydown matcher or a clipboard shim. `app/src/tests/no_javascript.rs`
//! checks the tree for both.

use std::sync::LazyLock;

use super::*;

/// Whether windows in this process are created see-through.
///
/// Read once, at the first window, and frozen for the life of the process.
/// Both halves of it are construction-time: `with_transparent` is passed to
/// the platform when the window is created, and the webview's background
/// colour is handed to WebKit before the first paint. Neither can be revised
/// on a live window, so a value that could change under us would only produce
/// windows that disagree with each other.
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

/// The mark as the window manager wants it.
///
/// `None` only if the rasteriser ever hands back a buffer whose length
/// disagrees with its dimensions, which `tao` rejects. A window with the
/// generic icon is the right answer to that; refusing to open is not.
pub(crate) fn window_icon() -> Option<vitrum_dioxus_desktop::tao::window::Icon> {
    let img = &*MARK_RASTER;
    vitrum_dioxus_desktop::tao::window::Icon::from_rgba(img.rgba.clone(), img.width, img.height)
        .ok()
}

/// The OS window for one slot.
pub(crate) fn window_builder(state: &WindowGeometry, scale: f64, os_scale: f64) -> WindowBuilder {
    let window = WindowBuilder::new()
        .with_title("vitrum")
        .with_window_icon(window_icon())
        .with_inner_size(PhysicalSize::new(state.width, state.height))
        .with_position(PhysicalPosition::new(state.x, state.y))
        .with_min_inner_size(PhysicalSize::new(
            (MIN_WINDOW_CSS.0 * scale * os_scale) as u32,
            (MIN_WINDOW_CSS.1 * scale * os_scale) as u32,
        ))
        .with_maximized(state.maximized)
        // Only when asked. A transparent window needs a compositor, and on a
        // bare window manager without one the alpha channel is not blended
        // with anything: the operator gets whatever was in the framebuffer.
        // An opaque profile must never be exposed to that.
        .with_transparent(*TRANSLUCENT);
    decorate(window)
}

/// Every stylesheet shipped in the document head, in cascade order.
///
/// ONE list, used by `document_head` to build the page and by the CSS guards
/// to check it. That is the point: it held three entries while the head
/// shipped thirteen, so the ten design parts were exempt from the
/// zero-infinite-animation guard, the reduced-motion guard and the
/// transition-duration guard for as long as they existed. Idle CPU is this
/// product's entire competitive claim and the rule protecting it was reading a
/// quarter of the CSS. Now the shipped page and the checked set cannot differ,
/// because they are the same array.
///
/// Every sheet here is ours. Nothing vendored is inlined any more: the
/// terminal's styling is the pane's own, in Rust.
pub(crate) fn stylesheets() -> [(&'static str, &'static str); 17] {
    [
        ("sidebar.css", SIDEBAR_CSS),
        ("settings.css", SETTINGS_CSS),
        ("app.css", APP_CSS),
        ("parts/10-spacing.css", PART_SPACING_CSS),
        ("parts/11-type.css", PART_TYPE_CSS),
        ("parts/12-color.css", PART_COLOR_CSS),
        ("parts/13-empty.css", PART_EMPTY_CSS),
        ("parts/14-chrome.css", PART_CHROME_CSS),
        ("parts/15-rows.css", PART_ROWS_CSS),
        ("parts/16-controls.css", PART_CONTROLS_CSS),
        ("parts/17-motion.css", PART_MOTION_CSS),
        ("parts/18-dialog.css", PART_DIALOG_CSS),
        ("parts/19-settings.css", PART_SETTINGS_CSS),
        ("parts/20-agent-marks.css", PART_AGENT_MARKS_CSS),
        ("parts/21-search.css", PART_SEARCH_CSS),
        ("parts/22-launcher.css", PART_LAUNCHER_CSS),
        // Last on purpose: it softens surfaces the parts above painted
        // opaque, so it has to win on source order at equal specificity.
        ("parts/23-backdrop.css", PART_BACKDROP_CSS),
    ]
}

/// Drop `/* ... */` from a stylesheet before it is inlined into the document.
///
/// The comments in this project's CSS are load-bearing FOR THE SOURCE: they
/// carry the measurements and the reasoning behind almost every value. They
/// are worth nothing to the engine, which discards them during parse, and
/// they are 70% of the 410 KB inlined into every webview: 409,530 bytes
/// becomes 122,894.
///
/// Measured over twenty windows: 37.03 MB per WebProcess with them against
/// 35.64 MB without, so 1.4 MB per window and 37.3 MB across the set.
///
/// Safe here because no stylesheet in this tree puts `/*` inside a string;
/// `no_css_string_hides_a_comment_delimiter` keeps it that way, because a
/// naive stripper would eat the rest of the file from such a string onward.
pub(crate) fn strip_css(css: &str) -> String {
    let mut out = String::with_capacity(css.len() / 3);
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            // Unterminated. Keep what came before and stop, rather than
            // shipping the remainder of a file whose structure is unknown.
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Every `<style>` the document head ships, named, in emission order.
///
/// The head is BUILT from this list rather than from a literal run of tags,
/// so a sheet cannot reach the operator without a name here, and the coverage
/// guard has something to enumerate. It used to be a run of literals checked
/// against the number two, and a sheet added later turned a decision into a
/// surprise failure that read as an off-by-one.
pub(crate) fn style_origins() -> Vec<(&'static str, String)> {
    // One pass over every stylesheet, once per call. The engine gets the
    // declarations; the reasoning stays in the source files where it is read.
    //
    // ONE `<style>`, not sixteen. Cascade order is concatenation order, so
    // joining them changes nothing about which rule wins. What it changes is
    // how many stylesheet objects the engine carries: sixteen per document,
    // and a document per window, so twenty windows in one web process held
    // 320 of them where 20 will do. None of these sheets opens with
    // `@charset` or `@import`, which are the two at-rules that must come
    // first in a sheet and are the only reason not to do this.
    //
    // `stylesheets()` still returns them separately, because the guards that
    // check a class is styled need to name the file that styles it.
    let sheets = stylesheets();
    let mut bundle =
        String::with_capacity(sheets.iter().map(|(_, s)| s.len()).sum::<usize>() + 32);
    for (_, sheet) in sheets.iter() {
        bundle.push_str(&strip_css(sheet));
        bundle.push('\n');
    }
    vec![("the design-system bundle", bundle)]
}

/// The inlined stylesheet bundle, built once for the process.
///
/// Building it per window would strip 410 KB of comments again for every
/// window that opens, and the string is immutable for the life of the
/// process.
pub(crate) fn document_head() -> &'static str {
    static HEAD: LazyLock<String> = LazyLock::new(|| {
        // Every sheet, in one place: `style_origins` decides what ships and
        // in what order, and this loop only wraps each one in its element.
        let origins = style_origins();
        let mut head =
            String::with_capacity(origins.iter().map(|(_, css)| css.len() + 16).sum::<usize>());
        for (_, css) in origins.iter() {
            head.push_str("<style>");
            head.push_str(css);
            head.push_str("</style>");
        }
        boot::mark("styles.built");
        head
    });
    &HEAD
}

/// One window's worth of webview configuration.
pub(crate) fn window_config(state: &WindowGeometry, scale: f64, os_scale: f64) -> Config {
    let config = Config::new()
        .with_window(window_builder(state, scale, os_scale))
        .with_custom_head(document_head().to_string())
        // A terminal shell has no use for a File/Edit/View menu, and on Linux
        // the default bar steals vertical space from the grid.
        .with_menu(None)
        .with_close_behaviour(WindowCloseBehaviour::WindowCloses)
        // The webview's own base layer. Opaque by default, and fully clear
        // when the profile asked for translucency: WebKit paints this behind
        // the document, so leaving it at 255 would make every `rgba` surface
        // in the stylesheet blend against a solid colour and change nothing.
        .with_background_color(if *TRANSLUCENT {
            (0, 0, 0, 0)
        } else {
            (6, 6, 8, 255)
        })
        // Two things the window owns from the instant it exists.
        //
        // The pane is installed here rather than when the shell mounts it,
        // which is the difference between a grid that is parsing output while
        // the shell is still being built and one that starts hundreds of
        // milliseconds later having dropped everything that arrived first.
        //
        // The mark goes on the window's own surface for the whole interval
        // before the shell has anything to show. See `splash`.
        .with_on_window(|window, _dom| {
            boot::mark("window.created");
            crate::install_pane(&window);
            crate::splash::install(&window);
        });

    // The scheme is registered exactly once for the process, not once per
    // window, and the second window is the one that proves it: every webview
    // is built from one leaked `WebContext` (`vendor/src/webview.rs`,
    // `shared_web_context`), a custom scheme belongs to the context rather
    // than the webview, and registering the same name twice against it is a
    // hard `DuplicateCustomProtocol` error. Attaching this to every config
    // panicked the moment a second window opened. The vendored fork already
    // guards its own `dioxus` scheme this way.
    //
    // Registering once is not a workaround: `backdrop_protocol` is a free
    // function that resolves the path out of the URL, so it holds nothing
    // window-specific, and the context outlives every window it serves.
    if BACKDROP_SCHEME_REGISTERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return config;
    }
    config.with_custom_protocol("vitrum-backdrop".to_string(), backdrop_protocol)
}

/// Whether this process has already registered the `vitrum-backdrop` scheme.
static BACKDROP_SCHEME_REGISTERED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Serve the operator's backdrop image to the document.
///
/// The page is served from a custom scheme, and a custom scheme cannot fetch
/// `file://`: WebKit treats it as cross-origin and refuses. So the image comes
/// back through a scheme of ours, with the path percent-encoded into the URL
/// by [`ui::settings::backdrop_url`].
///
/// This reads whatever path the profile names, which is the operator's own
/// file on their own machine, chosen through their own file picker. It is
/// deliberately not restricted to a directory: a wallpaper lives wherever the
/// operator keeps wallpapers. What it does refuse is anything that is not an
/// image, because a stylesheet that can name a path is a stylesheet that can
/// ask for `/etc/shadow`, and answering that with bytes would turn a cosmetic
/// setting into a file-read primitive for anyone who can write `ui.json`.
fn backdrop_protocol(
    _id: vitrum_dioxus_desktop::wry::WebViewId,
    request: vitrum_dioxus_desktop::wry::http::Request<Vec<u8>>,
) -> vitrum_dioxus_desktop::wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    use vitrum_dioxus_desktop::wry::http::Response;

    let deny = |code: u16| {
        Response::builder()
            .status(code)
            .body(std::borrow::Cow::Borrowed(&[][..]))
            .expect("a status-only response is always well formed")
    };

    let Some(path) = backdrop_path(request.uri().path()) else {
        return deny(400);
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return deny(404);
    };
    let Some(mime) = image_mime(&bytes) else {
        return deny(415);
    };
    Response::builder()
        .header("Content-Type", mime)
        .body(std::borrow::Cow::Owned(bytes))
        .expect("a response with one header is always well formed")
}

/// The filesystem path out of a `vitrum-backdrop://` URL path.
///
/// Returns `None` for anything that is not an absolute path, which is what a
/// traversal attempt and a malformed URL both look like from here.
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

/// Open another window in this process.
///
/// Windows are independent views, not mirrors: each gets its own [`UiState`],
/// its own tab strip and its own socket to the daemon, which broadcasts every
/// session change to all of them. What is *not* duplicated is the process, and
/// that is the whole point. A second process is a second WebKit engine, a
/// second network process and a second copy of every mapped page; a second
/// window shares all of it.
pub(crate) fn open_window(from: &DesktopContext, opts: Options, link: Option<DeepLink>) {
    let monitors: Vec<MonitorHandle> = from.available_monitors().collect();
    let rects = monitor_rects(from.primary_monitor().as_ref(), &monitors);
    let ordinal = claim_ordinal();

    // A new window opens on the monitor the window that spawned it is on,
    // which is where the user is looking.
    let host = from.current_monitor().or_else(|| from.primary_monitor());
    let density = host.as_ref().map(density_of);
    let scale = opts
        .ui_scale
        .unwrap_or_else(|| density.map_or(MIN_UI_SCALE, Density::ui_scale));
    let os_scale = density.map_or(1.0, |d| d.os_scale).max(1.0);

    let state = remembered(ordinal)
        .map(|s| window_state::clamp_to_monitors(&s, &rects))
        .unwrap_or_else(|| fresh_geometry(host.as_ref(), scale, ordinal));
    remember(ordinal, state);

    let dom = VirtualDom::new(App)
        .with_root_context(opts)
        .with_root_context(WindowSeed { ordinal, link });

    tracing::info!(
        "opening window {ordinal} at {}x{}+{}+{} scale {scale}; {} now open",
        state.width,
        state.height,
        state.x,
        state.y,
        live_window_count()
    );

    // The returned handle resolves once the window exists. Nothing here needs
    // to drive the new window, so it is dropped: the request is already queued
    // on the shared context and the event loop will build it.
    drop(from.new_window(dom, window_config(&state, scale, os_scale)));
}

#[cfg(test)]
mod tests;
