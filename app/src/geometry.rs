//! Where a window opens, how big it is, and how sharp it looks.
//!
//! Two problems that only look separate: the logical scale the operator sees,
//! and the physical rectangle the OS is asked for. Both are decided here so
//! that a restored window and a fresh one go through the same arithmetic.

use super::*;

/// The logical density this program aims to put in front of the operator.
///
/// Not 96. CSS calls 96 dpi "1x" and these stylesheets are authored in those
/// units, but 96 is a nominal figure from the 1990s and no desktop platform
/// actually targets it. Windows recommends 150% on a 27-inch 4K panel, which
/// leaves the user at 109 logical dpi; a Retina Mac runs a 221 dpi panel at a
/// backing scale of 2, which is 110. Both converge on the same number, so that
/// is the number.
///
/// Targeting 96 instead is not a rounding difference, it is a product
/// decision, and the wrong one. It quantises this machine's 163 dpi panel to
/// 1.75x, which makes a 27-inch 4K display show exactly as many sidebar rows
/// as a 27-inch 1080p one. An operator who bought 4K to watch twenty agents
/// would get sharper text and no more rows, which is not what they paid for.
/// At 110 the same panel lands on 1.5x and the list holds half again as many.
///
/// The bug this exists to fix is still the one it always was: the X session
/// here reports `Xft.dpi: 96` with `GDK_SCALE` unset, so the toolkit hands the
/// webview a scale factor of exactly 1.0 for both a 163 dpi panel and an 82
/// dpi one, and `16px` comes out 2.5 mm tall on the first. Nothing is broken;
/// everything is half-size.
pub(crate) const REFERENCE_DPI: f64 = 110.0;

/// Granularity of the scale offered.
///
/// 25% steps, the ladder Windows and GNOME expose. The measured answer here is
/// 1.702; snapping it to 1.75 costs 2.8% of nominal size and buys glyph stems
/// and one-pixel borders that land on whole device pixels instead of
/// straddling two.
pub(crate) const UI_SCALE_STEP: f64 = 0.25;

/// The UI is never drawn smaller than it was authored.
///
/// The 27-inch 1080p panel beside the 4K one is 82 dpi, and the arithmetically
/// honest scale for it is 0.85. The arithmetically honest answer is the wrong
/// one: text that is already physically comfortable is not a defect, and
/// shrinking it to hit a number would make the low-density monitor worse in
/// order to fix nothing.
pub(crate) const MIN_UI_SCALE: f64 = 1.0;

/// Ceiling on magnification. Past 3x a 1080p window is showing 640 CSS pixels
/// across and the layout, not the type size, is the problem.
pub(crate) const MAX_UI_SCALE: f64 = 3.0;

/// Below this, a reported physical size is not believable.
pub(crate) const MIN_PLAUSIBLE_DPI: f64 = 24.0;
/// Above this, likewise. The densest panel anyone has put a desktop on is a
/// 13-inch 4K laptop at about 340 dpi; 400 leaves room and still rejects the
/// 610 dpi that a television claiming to be 160x90 mm computes to. Projectors,
/// virtual displays and a good number of televisions report 160x90 mm or
/// 1600x900 mm, and either one turns into a scale that makes the app unusable.
/// Out of band means "this panel did not tell me", never "this panel is 8 dpi".
pub(crate) const MAX_PLAUSIBLE_DPI: f64 = 400.0;

/// One monitor, reduced to the five numbers that decide how large to draw.
///
/// A plain value with no handles in it, so every rule below is a pure function
/// and every case in the tests is a literal panel rather than a display nobody
/// has in CI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Density {
    /// Device pixels across the panel.
    pub(crate) width_px: u32,
    /// Device pixels down the panel.
    pub(crate) height_px: u32,
    /// Physical width in millimetres, or 0 when the panel does not say.
    pub(crate) width_mm: u32,
    /// Physical height in millimetres, or 0 when the panel does not say.
    pub(crate) height_mm: u32,
    /// Device pixels per CSS pixel that the toolkit already applies by itself.
    pub(crate) os_scale: f64,
}

impl Density {
    /// Physical dots per inch, or `None` when the panel will not say.
    ///
    /// Both axes are averaged rather than trusting the width, because a panel
    /// with a rotation applied in the EDID reports one axis correctly and the
    /// other transposed, and the mean of the two is closer than either.
    pub(crate) fn dpi(self) -> Option<f64> {
        let axis =
            |px: u32, mm: u32| (px > 0 && mm > 0).then(|| f64::from(px) * 25.4 / f64::from(mm));
        let axes = [
            axis(self.width_px, self.width_mm),
            axis(self.height_px, self.height_mm),
        ];
        let mut total = 0.0;
        let mut n = 0.0;
        for d in axes.into_iter().flatten() {
            total += d;
            n += 1.0;
        }
        if n == 0.0 {
            return None;
        }
        let dpi = total / n;
        (MIN_PLAUSIBLE_DPI..=MAX_PLAUSIBLE_DPI)
            .contains(&dpi)
            .then_some(dpi)
    }

    /// CSS pixels per inch, after whatever the toolkit already scaled by.
    ///
    /// This is the number that actually matters. A Retina Mac is 220 dpi and
    /// reports a scale factor of 2, so a CSS pixel there is already 1/110 of
    /// an inch and magnifying again would draw everything twice life size.
    pub(crate) fn css_dpi(self) -> Option<f64> {
        let scale = if self.os_scale > 0.0 {
            self.os_scale
        } else {
            1.0
        };
        self.dpi().map(|dpi| dpi / scale)
    }

    /// How much this program must magnify to reach reference physical size.
    ///
    /// A panel that will not report its size gets 1.0, which is right on macOS
    /// and Windows (where the platform has already applied the user's scale
    /// and told tao about it) and is the status quo on an X11 session where
    /// nobody configured anything.
    pub(crate) fn ui_scale(self) -> f64 {
        match self.css_dpi() {
            Some(dpi) => quantize_ui_scale(dpi / REFERENCE_DPI),
            None => MIN_UI_SCALE,
        }
    }
}

/// Snap a raw ratio onto the offered ladder and into the offered range.
pub(crate) fn quantize_ui_scale(raw: f64) -> f64 {
    if !raw.is_finite() {
        return MIN_UI_SCALE;
    }
    ((raw / UI_SCALE_STEP).round() * UI_SCALE_STEP).clamp(MIN_UI_SCALE, MAX_UI_SCALE)
}

/// Read a monitor's density through GDK, which is the only interface on X11
/// that knows the panel's physical size.
///
/// tao's own `scale_factor()` is not enough and cannot be: it forwards
/// `gdk_monitor_get_scale_factor`, which on this session is 1 on both a 82 dpi
/// panel and a 163 dpi one because nobody set `GDK_SCALE`. The millimetres
/// come from RandR, which reads them out of the EDID, and they are the only
/// signal that distinguishes the two.
#[cfg(target_os = "linux")]
pub(crate) fn density_of(monitor: &MonitorHandle) -> Density {
    use gtk::gdk::prelude::MonitorExt;
    use vitrum_dioxus_desktop::tao::platform::unix::MonitorHandleExtUnix;

    let gdk = monitor.gdk_monitor();
    // GDK reports geometry in logical pixels and tao reports it in device
    // pixels; the density arithmetic wants device pixels, so undo GDK's
    // division rather than tao's multiplication.
    let os_scale = f64::from(gdk.scale_factor().max(1));
    let geometry = gdk.geometry();
    Density {
        width_px: (f64::from(geometry.width().max(0)) * os_scale) as u32,
        height_px: (f64::from(geometry.height().max(0)) * os_scale) as u32,
        width_mm: gdk.width_mm().max(0) as u32,
        height_mm: gdk.height_mm().max(0) as u32,
        os_scale,
    }
}

/// Everywhere else, the platform already did this properly.
///
/// macOS reports a backing scale factor that is the whole answer, and Windows
/// reports the per-monitor effective DPI the user chose in Settings. tao
/// forwards both. Leaving the millimetres at zero is what makes
/// [`Density::ui_scale`] hand back 1.0 and defer to them.
#[cfg(not(target_os = "linux"))]
pub(crate) fn density_of(monitor: &MonitorHandle) -> Density {
    let size = monitor.size();
    Density {
        width_px: size.width,
        height_px: size.height,
        width_mm: 0,
        height_mm: 0,
        os_scale: monitor.scale_factor(),
    }
}

/// The density of the monitor `window` is on right now.
pub(crate) fn window_density(window: &Window) -> Density {
    window
        .current_monitor()
        .or_else(|| window.primary_monitor())
        .as_ref()
        .map(density_of)
        .unwrap_or(Density {
            width_px: 0,
            height_px: 0,
            width_mm: 0,
            height_mm: 0,
            os_scale: window.scale_factor(),
        })
}

/// The scale this window should be drawn at: the override if there is one,
/// otherwise the panel's own answer.
pub(crate) fn window_ui_scale(window: &Window, override_scale: Option<f64>) -> f64 {
    override_scale.unwrap_or_else(|| window_density(window).ui_scale())
}

/// Width of the document, in CSS pixels, once `scale` is applied.
///
/// tao measures the client area in device pixels. The webview divides those by
/// the toolkit's scale factor to get CSS pixels, and page zoom divides them
/// again, so a 3840 px client area at 1.75 zoom is a 2194 CSS pixel document.
/// Anything reasoning about layout has to work in that second number.
pub(crate) fn css_viewport_width(window: &Window, scale: f64) -> f64 {
    let device = f64::from(window.inner_size().width);
    let divisor = window.scale_factor() * scale;
    if divisor > 0.0 {
        device / divisor
    } else {
        device
    }
}

/// Fraction of the document the sidebar takes when nobody has dragged it.
///
/// The fixed pixel default is the defect: 256 px is 20% of a 1280 px window
/// and 6.7% of a 3840 px one, and the second number is a column of truncated
/// titles beside an ocean of terminal. A fraction is the same sidebar on every
/// screen, and 22% is where a 30-character session title stops eliding at the
/// default type size.
pub(crate) const SIDEBAR_FRACTION: f64 = 0.22;

/// Document width the automatic sidebar will not eat into.
///
/// 80 columns plus the tab strip's padding. When the window is too narrow to
/// hold both, the terminal wins: a sidebar beside a 30-column terminal is a
/// file manager, not a terminal shell.
pub(crate) const MIN_CONTENT_CSS_PX: f64 = 360.0;

/// Sidebar width, in CSS pixels, for a window nobody has dragged.
pub(crate) fn default_sidebar_width(css_window_width: f64) -> f64 {
    let room = (css_window_width - MIN_CONTENT_CSS_PX).max(SIDEBAR_MIN_PX);
    (css_window_width * SIDEBAR_FRACTION)
        .min(room)
        .clamp(SIDEBAR_MIN_PX, SIDEBAR_MAX_PX)
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Size of a window on a screen this program has never opened one on, in CSS
/// pixels. Multiplied by the scale before it reaches the window manager, so a
/// first launch on a 163 dpi panel gets a 2240x1400 device-pixel window rather
/// than a postage stamp.
pub(crate) const FRESH_WINDOW_CSS: (f64, f64) = (1280.0, 800.0);

/// Smallest the user may drag a window to, in CSS pixels.
pub(crate) const MIN_WINDOW_CSS: (f64, f64) = (640.0, 400.0);

/// Fraction of a monitor a fresh window will not exceed. A 1280 CSS pixel
/// window at 1.75 is wider than a 1080p screen, and a window larger than its
/// monitor cannot be dragged back onto it.
pub(crate) const FRESH_WINDOW_MONITOR_FRACTION: f64 = 0.9;

/// Share of the monitor a first-run window aims for, before the floor and the
/// cap below are applied.
///
/// Sized like an application rather than like a dialog. The sidebar is a
/// fraction of the WINDOW, so a window that ignores the display makes the
/// panel look starved on exactly the large screens this product is for.
pub(crate) const FRESH_WINDOW_MONITOR_TARGET: f64 = 0.72;

/// Diagonal offset between successive fresh windows, in CSS pixels. Without it
/// the fifth window is exactly on top of the first four and looks like the
/// click did nothing.
pub(crate) const CASCADE_CSS_PX: f64 = 32.0;

/// Cascade steps before starting over at the top left.
pub(crate) const CASCADE_STEPS: usize = 8;

/// What a window needs to know about itself that the process cannot infer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowSeed {
    /// Slot in the remembered-geometry table.
    pub(crate) ordinal: usize,
    /// A session this window was asked to open, carried in by a `vitrum://`
    /// URL that a second launch handed over.
    pub(crate) link: Option<DeepLink>,
}

/// Geometry for every window this process has opened, by ordinal.
///
/// Global because a window is created by whichever *existing* window's context
/// noticed the request first, and that window has no business owning the
/// bookkeeping for a sibling it will outlive or be outlived by.
pub(crate) struct WindowBook {
    /// One entry per ordinal ever used. Grows and never shrinks: the rectangle
    /// a closed window occupied is what the next window in that slot, and the
    /// next launch, are restored to.
    pub(crate) slots: Vec<WindowGeometry>,
    /// Ordinals with a window on screen right now.
    pub(crate) live: Vec<usize>,
}

pub(crate) static BOOK: Mutex<WindowBook> = Mutex::new(WindowBook {
    slots: Vec::new(),
    live: Vec::new(),
});

/// Take the book, ignoring poisoning.
///
/// A panicking window handler must not take window placement down with it for
/// the rest of the process: the book is two vectors of plain numbers and there
/// is no state a panic could leave half-applied.
pub(crate) fn book() -> std::sync::MutexGuard<'static, WindowBook> {
    BOOK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Seed the book from disk, before any window exists.
pub(crate) fn seed_book(restored: Vec<WindowGeometry>) {
    book().slots = restored;
}

/// Take the lowest ordinal with no window on it.
///
/// Reusing a closed window's ordinal rather than always appending is what
/// makes "close the second window, open another" put the new one where the old
/// one was instead of cascading forever.
pub(crate) fn claim_ordinal() -> usize {
    let mut book = book();
    let mut ordinal = 0;
    while book.live.contains(&ordinal) {
        ordinal += 1;
    }
    book.live.push(ordinal);
    ordinal
}

/// Give an ordinal back when its window goes away.
pub(crate) fn release_ordinal(ordinal: usize) {
    book().live.retain(|n| *n != ordinal);
}

/// How many windows are on screen.
pub(crate) fn live_window_count() -> usize {
    book().live.len()
}

/// Remembered geometry for an ordinal, from this run or a previous one.
pub(crate) fn remembered(ordinal: usize) -> Option<WindowGeometry> {
    book().slots.get(ordinal).copied()
}

/// Record a window's current rectangle, in memory only.
///
/// Deliberately not a write: this is called from `Moved` and `Resized`, which
/// arrive by the hundred while a window is being dragged, and a filesystem
/// write per pointer sample is how an idle-cost budget dies.
pub(crate) fn remember(ordinal: usize, state: WindowGeometry) {
    let mut book = book();
    if book.slots.len() <= ordinal {
        book.slots.resize(ordinal + 1, WindowGeometry::default());
    }
    book.slots[ordinal] = state;
}

/// Record just the sidebar width, leaving the rectangle alone.
pub(crate) fn remember_sidebar(ordinal: usize, css_px: f64) {
    let mut book = book();
    if book.slots.len() <= ordinal {
        book.slots.resize(ordinal + 1, WindowGeometry::default());
    }
    book.slots[ordinal].sidebar_width = css_px.max(0.0) as u32;
}

/// Where per-window geometry lives.
///
/// `vitrum-os` persists one window's state, and one is no longer how many
/// windows this program has. The file is a list of exactly the type that crate
/// already models, validated on the way back in by the same
/// [`window_state::clamp_to_monitors`] that keeps a single window from being
/// restored onto a monitor somebody has since unplugged.
pub(crate) fn geometry_file() -> Option<PathBuf> {
    match AppPaths::for_current_platform() {
        Ok(paths) => Some(paths.window_state_file()),
        Err(e) => {
            tracing::warn!("window geometry will not persist: {e}");
            None
        }
    }
}

/// On-disk shape of [`geometry_file`].
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct PersistedWindows {
    pub(crate) version: u32,
    pub(crate) windows: Vec<WindowGeometry>,
}

/// Read remembered geometry, forced onto the monitors that exist right now.
///
/// A version this build does not understand is discarded rather than coerced.
/// Losing a window layout is a small annoyance; restoring half of a format
/// that meant something else is a window nobody can find.
pub(crate) fn load_geometry(monitors: &[Monitor]) -> Vec<WindowGeometry> {
    let Some(path) = geometry_file() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let parsed: PersistedWindows = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("ignoring unreadable {}: {e}", path.display());
            return Vec::new();
        }
    };
    if parsed.version != window_state::STATE_FORMAT_VERSION {
        tracing::warn!(
            "ignoring {}: format version {}, this build writes {}",
            path.display(),
            parsed.version,
            window_state::STATE_FORMAT_VERSION
        );
        return Vec::new();
    }
    parsed
        .windows
        .iter()
        .map(|state| window_state::clamp_to_monitors(state, monitors))
        .collect()
}

/// Write the per-window UI state: the tab strip, the sidebar collapse, the
/// viewed workspace.
///
/// Separate from [`save_geometry`] because they own different files, and
/// separate from `settings::commit` because that also publishes to the live
/// settings bus, which is wrong at the moment a window is going away.
///
/// This exists because those fields were persisted only by accident. They are
/// in `WindowSnapshot`, which `save_prefs` writes and `restore_window` reads,
/// but not one of the five collapse toggles and not one tab operation ever
/// called `commit`, and the exit hooks wrote only `windows.json`. The state
/// survived a restart when some unrelated control committed afterwards and
/// carried it along, and was lost when nothing did.
pub(crate) fn save_window_state(st: Signal<UiState>) {
    let state = st.peek();
    if let Err(why) = crate::state::save_prefs(&state.daemon, &state.window) {
        tracing::warn!("window state not saved: {why}");
    }
}

/// Write remembered geometry.
///
/// Called when a window loses focus or closes, which are the two moments a
/// user has finished moving it. Never on a pointer sample.
pub(crate) fn save_geometry() {
    let Some(path) = geometry_file() else {
        return;
    };
    let payload = PersistedWindows {
        version: window_state::STATE_FORMAT_VERSION,
        windows: book().slots.clone(),
    };
    let Ok(text) = serde_json::to_string_pretty(&payload) else {
        return;
    };
    // Write-then-rename, the same trade `vitrum_os::window_state::save` makes:
    // a machine that dies mid-write must leave the previous layout intact
    // rather than a truncated file that reads back as corrupt every launch.
    let temp = path.with_extension("json.tmp");
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::warn!("cannot create {}: {e}", parent.display());
        return;
    }
    if let Err(e) = std::fs::write(&temp, text) {
        tracing::warn!("cannot write {}: {e}", temp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&temp, &path) {
        tracing::warn!("cannot replace {}: {e}", path.display());
    }
}

/// The monitor list in the form [`window_state::clamp_to_monitors`] wants,
/// primary first because that function breaks ties by position in the slice
/// and "the second screen is gone" has to land on the primary.
pub(crate) fn monitor_rects(
    primary: Option<&MonitorHandle>,
    all: &[MonitorHandle],
) -> Vec<Monitor> {
    let rect = |m: &MonitorHandle| {
        let p = m.position();
        let s = m.size();
        Monitor::new(p.x, p.y, s.width, s.height)
    };
    let mut out: Vec<Monitor> = primary.map(rect).into_iter().collect();
    for m in all {
        let r = rect(m);
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

/// Read a live window's rectangle.
///
/// The sidebar width comes from the book rather than from the window, because
/// it is a document measurement the window cannot see. Keeping it out of here
/// is what lets `Moved` write a rectangle without clobbering a width the user
/// dragged half a second earlier.
pub(crate) fn measure(window: &Window, ordinal: usize) -> WindowGeometry {
    let size = window.inner_size();
    let position = window
        .outer_position()
        .unwrap_or(PhysicalPosition::new(0, 0));
    WindowGeometry {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: window.is_maximized(),
        sidebar_width: remembered(ordinal)
            .map(|s| s.sidebar_width)
            .unwrap_or(window_state::DEFAULT_SIDEBAR_WIDTH),
    }
}

/// Geometry for a window that has never been placed.
///
/// Every number here is in device pixels, which is what tao reports and what
/// the window manager consumes. Persisting device pixels rather than logical
/// ones is deliberate: the two agree on this machine and diverge the moment
/// somebody sets `GDK_SCALE`, and a rectangle saved in one unit and restored
/// in the other is a window at a quarter size with no clue why.
pub(crate) fn fresh_geometry(
    monitor: Option<&MonitorHandle>,
    scale: f64,
    ordinal: usize,
) -> WindowGeometry {
    let density = monitor.map(density_of);
    let os_scale = density.map_or(1.0, |d| d.os_scale).max(1.0);
    let (mx, my, mw, mh) = match monitor {
        Some(m) => {
            let p = m.position();
            let s = m.size();
            (
                p.x as f64,
                p.y as f64,
                f64::from(s.width),
                f64::from(s.height),
            )
        }
        None => (
            0.0,
            0.0,
            FRESH_WINDOW_CSS.0 * scale * os_scale,
            FRESH_WINDOW_CSS.1 * scale * os_scale,
        ),
    };

    // A FRACTION of the monitor, floored at the fixed default.
    //
    // This was a flat `FRESH_WINDOW_CSS * scale`, so every first launch opened
    // 1280 CSS px wide whatever it was launched on. On a 3840px panel that is
    // a third of the screen, and since the sidebar is 22% of the WINDOW, the
    // panel came out at 281px next to an ocean of desktop. That is the whole
    // of "the sidebar feels cramped": the panel was correctly proportioned to
    // a window that was itself far too small for the display.
    //
    // 72% is the same instinct a macOS app has on first run: clearly a window
    // rather than a full-screen takeover, but sized to the machine it is on.
    // The old fixed size survives as the FLOOR, so a small laptop panel is
    // never worse off than before, and the 90% cap still keeps the frame
    // reachable.
    let fresh_w = FRESH_WINDOW_CSS.0 * scale * os_scale;
    let fresh_h = FRESH_WINDOW_CSS.1 * scale * os_scale;
    let width = (mw * FRESH_WINDOW_MONITOR_TARGET)
        .max(fresh_w)
        .min(mw * FRESH_WINDOW_MONITOR_FRACTION);
    let height = (mh * FRESH_WINDOW_MONITOR_TARGET)
        .max(fresh_h)
        .min(mh * FRESH_WINDOW_MONITOR_FRACTION);
    let step = (ordinal % CASCADE_STEPS) as f64 * CASCADE_CSS_PX * scale * os_scale;
    // Centre, then cascade, then push back on screen: eight steps down and
    // right from the middle of a small monitor walks the last window off the
    // bottom corner, and a window whose title bar is off screen cannot be
    // dragged back.
    let x = (mx + ((mw - width) / 2.0).max(0.0) + step).min(mx + (mw - width).max(0.0));
    let y = (my + ((mh - height) / 2.0).max(0.0) + step).min(my + (mh - height).max(0.0));

    WindowGeometry {
        x: x.round() as i32,
        y: y.round() as i32,
        width: width.round() as u32,
        height: height.round() as u32,
        maximized: false,
        // ZERO means "the operator has never said". It is not a width.
        //
        // This used to write `default_sidebar_width(...)` here, and that one
        // line was the whole reason the sidebar was stuck at 282px on a 4K
        // panel. `sidebar_pinned` asks `remembered(ordinal).sidebar_width > 0`
        // to decide whether a width is a PREFERENCE, so writing a computed
        // default into the slot on first launch made the app read its own
        // guess back as a user choice from the second launch onward — and the
        // guess was 22% of whatever window happened to open first, typically a
        // 1280px one, inherited by a 2560 CSS px window forever. The
        // fraction-of-the-document rule could never apply again.
        //
        // With zero, the only writer of a non-zero value is
        // `remember_sidebar`, which is called from exactly two places: the
        // drag handler and the keyboard nudge. So the flag now means what it
        // says.
        sidebar_width: 0,
    }
}
