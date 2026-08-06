//! The web document a window is built around: stylesheets, head, and the
//! window configuration that hosts them.

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
    state::load_prefs()
        .0
        .settings
        .appearance
        .needs_transparent_window()
});

/// The OS window for one slot.
pub(crate) fn window_builder(state: &WindowGeometry, scale: f64, os_scale: f64) -> WindowBuilder {
    let window = WindowBuilder::new()
        .with_title("vitrum")
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
/// `xterm.css` is not here: it is vendored, and our motion rules are not its
/// to obey.
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

/// The inlined stylesheet and script bundle, built once for the process.
///
/// `OnceLock` rather than `LazyLock` because the renderer name in it comes off
/// the command line, which does not exist at declaration time. Building it per
/// window would copy the vendored sources for every window that opens, and the
/// string is immutable for the life of the process.
pub(crate) fn document_head(opts: Options) -> &'static str {
    static HEAD: OnceLock<String> = OnceLock::new();
    HEAD.get_or_init(|| {
        // One pass over every stylesheet, once per process. The engine gets the
        // declarations; the reasoning stays in the source files where it is
        // read. `xterm.css` is vendored and left alone.
        // ONE `<style>`, not sixteen.
        //
        // Cascade order is concatenation order, so joining them changes
        // nothing about which rule wins. What it changes is how many
        // stylesheet objects the engine carries: sixteen per document, and a
        // document per window, so twenty windows in one web process held 320
        // of them where 20 will do. None of these sheets opens with `@charset`
        // or `@import`, which are the two at-rules that must come first in a
        // sheet and are the only reason not to do this.
        //
        // `stylesheets()` still returns them separately, because the guards
        // that check a class is styled need to name the file that styles it.
        let css: String = {
            let sheets = stylesheets();
            let mut out =
                String::with_capacity(sheets.iter().map(|(_, s)| s.len()).sum::<usize>() + 32);
            out.push_str("<style>");
            for (_, sheet) in sheets.iter() {
                out.push_str(&strip_css(sheet));
                out.push('\n');
            }
            out.push_str("</style>");
            out
        };
        format!(
            "<style>{XTERM_CSS}</style>\
             {css}\
             <script>window.__vitrum_renderer={:?};window.__vitrum_keymap={};</script>\
             <script type=\"text/plain\" id=\"rg-vendor-xterm\">{XTERM_JS}</script>\
             {webgl}\
             <script type=\"text/plain\" id=\"rg-vendor-fit\">{ADDON_FIT_JS}</script>",
            opts.renderer.as_str(),
            // The table the OPERATOR has, not the compile-time default: their
            // rebindings folded in and their saved presets appended. The head
            // is the only copy that is guaranteed to be in place before the
            // first keydown, so shipping defaults here means every rebound
            // chord and every preset shortcut is dead until the mount-time
            // push lands, and silently dead forever if it does not.
            ui::settings::keymap_json(&ui::settings::live_chords(
                &state::load_prefs().0.settings.keyboard,
                &launch::load_launch_store().presets,
            )),
            // `type="text/plain"` so the engine STORES these and does not
            // parse them. `bootstrap.js::loadVendor` evaluates them on the
            // first command that needs a terminal, then clears the element's
            // text so the source is released too.
            //
            // Measured, twenty windows: 41.28 MB per WebProcess with these
            // parsed at startup against 36.30 MB with them absent entirely,
            // so 5.0 MB per window, 105.5 MB across the set. That is the cost
            // of COMPILING 390 KB of JavaScript in every window, and most
            // windows in a twenty-window session never focus a session at all.
            //
            // The WebGL addon is a further 100 KB and always ships, because
            // the Terminal settings row offers WebGL and used to be unable to
            // deliver it: the script was emitted only for `--renderer webgl`,
            // so an operator who picked WebGL in the row got an error flash
            // and a silent revert to DOM, on that launch and on every launch
            // after it, with nothing anywhere mentioning a flag.
            //
            // Shipping it costs 100 KB of DOM text per window and nothing
            // else. It is NOT in `loadVendor`'s list, so the 100 KB is never
            // COMPILED unless the operator actually selects WebGL, which is
            // where the 5.0 MB per window above was spent. Text is cheap;
            // parsing is not.
            webgl = format!(
                "<script type=\"text/plain\" id=\"rg-vendor-webgl\">{ADDON_WEBGL_JS}</script>"
            )
        )
    })
    .as_str()
}

/// One window's worth of webview configuration.
pub(crate) fn window_config(
    opts: Options,
    state: &WindowGeometry,
    scale: f64,
    os_scale: f64,
) -> Config {
    Config::new()
        .with_window(window_builder(state, scale, os_scale))
        .with_custom_head(document_head(opts).to_string())
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
        .with_custom_protocol("vitrum-backdrop".to_string(), backdrop_protocol)
}

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
    drop(from.new_window(dom, window_config(opts, &state, scale, os_scale)));
}

#[cfg(test)]
mod tests;
