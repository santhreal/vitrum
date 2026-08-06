//! Window geometry that survives a restart without stranding the window.
//!
//! The whole problem is that a saved position is only meaningful relative to
//! the monitor layout that produced it. Undock a laptop, unplug the second
//! screen, or change the primary display's scale, and a faithfully restored
//! `x = 3200` puts the window somewhere the user cannot reach and cannot even
//! see to drag back. So restoring is not "read the file"; it is "read the file,
//! then prove the rectangle still lands on a monitor that exists right now".
//!
//! [`clamp_to_monitors`] is that proof, and it is a pure function of a saved
//! state and the current monitor list, which is why every one of its edge cases
//! is asserted with exact coordinates in the tests instead of eyeballed on a
//! second display nobody has in CI.

use core::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Format version written into the state file.
///
/// Present so a future layout change is detected and reported rather than
/// deserialising into plausible nonsense.
pub const STATE_FORMAT_VERSION: u32 = 1;

/// Smallest window that is still usable: sidebar plus a terminal wide enough
/// for 80 columns at a normal font size.
pub(crate) const MIN_WIDTH: u32 = 720;
/// Smallest usable height: tab strip plus roughly 20 terminal rows.
pub(crate) const MIN_HEIGHT: u32 = 480;
/// Geometry for a first launch, or a restore that could not be validated.
pub(crate) const DEFAULT_WIDTH: u32 = 1280;
/// Geometry for a first launch, or a restore that could not be validated.
pub(crate) const DEFAULT_HEIGHT: u32 = 800;
/// Narrowest sidebar that still shows a session title and its status pill.
pub(crate) const MIN_SIDEBAR_WIDTH: u32 = 180;
/// Widest sidebar. Beyond this the terminal is the smaller pane, which is
/// backwards for a terminal-first product.
pub(crate) const MAX_SIDEBAR_WIDTH: u32 = 560;
/// Terminal width the sidebar is never allowed to eat into.
pub(crate) const MIN_CONTENT_WIDTH: u32 = 360;
/// Sidebar width on a first launch.
pub const DEFAULT_SIDEBAR_WIDTH: u32 = 280;

/// A monitor's logical work area, in the same coordinate space the window
/// manager reports window positions in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monitor {
    /// Left edge. Negative for a monitor placed left of the primary.
    pub x: i32,
    /// Top edge. Negative for a monitor placed above the primary.
    pub y: i32,
    /// Work area width, with panels and docks already excluded.
    pub width: u32,
    /// Work area height, with panels and docks already excluded.
    pub height: u32,
}

impl Monitor {
    /// Take a work area the platform already reported.
    ///
    /// Nothing is validated: a zero-sized rectangle is a real thing to see
    /// while a display is being reconfigured, and rejecting it here would only
    /// move the problem to the caller.
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    fn right(&self) -> i64 {
        self.x as i64 + self.width as i64
    }

    fn bottom(&self) -> i64 {
        self.y as i64 + self.height as i64
    }
}

/// Everything about the window worth remembering between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    /// Restore position, left edge. Clamped onto the monitors that exist at
    /// load time, never trusted as written.
    pub x: i32,
    /// Restore position, top edge. Clamped the same way as `x`.
    pub y: i32,
    /// Restore width. Clamped on load so the window still fits a monitor.
    pub width: u32,
    /// Restore height. Clamped on load alongside `width`.
    pub height: u32,
    /// Whether the window was maximised. The `x`/`y`/`width`/`height` above
    /// remain the *restore* geometry, which is what the window manager needs to
    /// un-maximise back onto a sane rectangle.
    pub maximized: bool,
    /// Sidebar width in logical pixels. Clamped on load so it can never eat
    /// into the terminal's minimum content width, however the window shrank
    /// since it was written.
    pub sidebar_width: u32,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            maximized: false,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
        }
    }
}

impl WindowState {
    fn right(&self) -> i64 {
        self.x as i64 + self.width as i64
    }

    fn bottom(&self) -> i64 {
        self.y as i64 + self.height as i64
    }

    /// Area shared with a monitor, in square logical pixels.
    ///
    /// `i64` throughout: two `i32` coordinates near the limits multiply out of
    /// `i32` range, and a corrupt state file is exactly where those show up.
    fn overlap_area(&self, m: &Monitor) -> i64 {
        let w = (self.right().min(m.right()) - (self.x as i64).max(m.x as i64)).max(0);
        let h = (self.bottom().min(m.bottom()) - (self.y as i64).max(m.y as i64)).max(0);
        w * h
    }
}

/// Force a saved state onto the current monitor layout.
///
/// Rules, in order:
///
/// 1. **No monitors at all.** Nothing can be validated, so the geometry is
///    discarded for the default rectangle at the origin. `maximized` and the
///    sidebar width survive because neither depends on a monitor. This happens
///    on a headless start and when a compositor reports zero outputs during
///    a hotplug.
/// 2. **Target monitor** is the one sharing the most area with the saved
///    rectangle; ties go to the earlier monitor, which callers pass primary
///    first. A rectangle that touches nothing lands on the primary, which is
///    the "the second screen is gone" case this function exists for.
/// 3. **Size** is clamped into `[MIN_*, monitor size]`. The monitor wins if it
///    is smaller than the minimum, because a window larger than its screen
///    cannot be moved onto it.
/// 4. **Position** is clamped so the whole rectangle sits inside the target
///    monitor. Not merely "the title bar is visible": a window half off the
///    right edge of the only screen is still a window the user has to fix.
/// 5. **Sidebar width** is clamped so the terminal keeps `MIN_CONTENT_WIDTH`.
pub fn clamp_to_monitors(state: &WindowState, monitors: &[Monitor]) -> WindowState {
    let Some(target) = pick_monitor(state, monitors) else {
        return WindowState {
            x: 0,
            y: 0,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            maximized: state.maximized,
            sidebar_width: clamp_sidebar(state.sidebar_width, DEFAULT_WIDTH),
        };
    };

    let width = state.width.max(MIN_WIDTH).min(target.width);
    let height = state.height.max(MIN_HEIGHT).min(target.height);

    let max_x = target.right() - width as i64;
    let max_y = target.bottom() - height as i64;
    let x = (state.x as i64).clamp(target.x as i64, max_x) as i32;
    let y = (state.y as i64).clamp(target.y as i64, max_y) as i32;

    WindowState {
        x,
        y,
        width,
        height,
        maximized: state.maximized,
        sidebar_width: clamp_sidebar(state.sidebar_width, width),
    }
}

fn pick_monitor<'a>(state: &WindowState, monitors: &'a [Monitor]) -> Option<&'a Monitor> {
    let mut best: Option<(&Monitor, i64)> = None;
    for m in monitors {
        let area = state.overlap_area(m);
        match best {
            Some((_, best_area)) if area <= best_area => {}
            _ => best = Some((m, area)),
        }
    }
    match best {
        Some((m, area)) if area > 0 => Some(m),
        // Overlaps nothing: the monitor it was saved on is gone.
        Some(_) => monitors.first(),
        None => None,
    }
}

/// Clamp the sidebar against a window width, keeping the terminal usable.
///
/// When the window is too narrow to hold both minimums the terminal wins and
/// the sidebar takes whatever is left, possibly zero. Preferring the sidebar
/// would leave a terminal too narrow to render a prompt.
pub(crate) fn clamp_sidebar(sidebar_width: u32, window_width: u32) -> u32 {
    let hi = window_width.saturating_sub(MIN_CONTENT_WIDTH).min(MAX_SIDEBAR_WIDTH);
    let lo = MIN_SIDEBAR_WIDTH.min(hi);
    sidebar_width.clamp(lo, hi)
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    version: u32,
    #[serde(flatten)]
    state: WindowState,
}

/// Outcome of reading the state file.
///
/// Five variants rather than `Option`, because "the file is not there" and "the
/// file is there and I could not read it" call for different UI: the first is a
/// first launch, the second is a bug or a permissions problem the operator
/// should see. Collapsing them into a silent default is how a product loses a
/// user's layout every launch without ever saying why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateLoad {
    /// No file yet. First launch.
    Missing,
    /// The file exists and the filesystem refused it.
    Unreadable {
        /// The filesystem error, verbatim.
        detail: String,
    },
    /// The file exists and is not valid state.
    Corrupt {
        /// What the parser objected to.
        detail: String,
    },
    /// Written by a newer build.
    UnsupportedVersion {
        /// The version the file declares, higher than this build understands.
        found: u32,
    },
    /// Read and parsed. Not yet clamped.
    Loaded(WindowState),
}

impl StateLoad {
    /// The state to actually use, clamped onto the live monitor layout.
    ///
    /// Every non-`Loaded` outcome yields the default geometry, still clamped,
    /// so a default that does not fit a small screen is corrected too.
    pub fn resolve(&self, monitors: &[Monitor]) -> WindowState {
        let base = match self {
            Self::Loaded(state) => *state,
            _ => WindowState::default(),
        };
        clamp_to_monitors(&base, monitors)
    }

    /// A message worth logging, or `None` when nothing went wrong.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Missing | Self::Loaded(_) => None,
            Self::Unreadable { detail } => Some(format!("window state unreadable: {detail}")),
            Self::Corrupt { detail } => Some(format!("window state corrupt: {detail}")),
            Self::UnsupportedVersion { found } => Some(format!(
                "window state is format version {found}, this build understands {STATE_FORMAT_VERSION}"
            )),
        }
    }
}

impl fmt::Display for StateLoad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("missing"),
            Self::Loaded(_) => f.write_str("loaded"),
            _ => f.write_str(&self.problem().unwrap_or_default()),
        }
    }
}

/// Read the state file. Never panics, never silently defaults.
pub fn load(path: &Path) -> StateLoad {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return StateLoad::Missing,
        Err(e) => return StateLoad::Unreadable { detail: e.to_string() },
    };
    parse(&text)
}

/// Parse state file contents.
pub fn parse(text: &str) -> StateLoad {
    // Read the version before the body so a future format change reports the
    // version rather than a field-level parse error about the change itself.
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(map)) => match map.get("version").and_then(|v| v.as_u64()) {
            Some(v) if v != STATE_FORMAT_VERSION as u64 => {
                return StateLoad::UnsupportedVersion { found: v.min(u32::MAX as u64) as u32 };
            }
            Some(_) => {}
            None => {
                return StateLoad::Corrupt { detail: "missing `version` field".to_string() };
            }
        },
        Ok(other) => {
            return StateLoad::Corrupt {
                detail: format!("expected a JSON object, found {}", json_kind(&other)),
            };
        }
        Err(e) => return StateLoad::Corrupt { detail: e.to_string() },
    }

    match serde_json::from_str::<Persisted>(text) {
        Ok(p) => StateLoad::Loaded(p.state),
        Err(e) => StateLoad::Corrupt { detail: e.to_string() },
    }
}

fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Serialise state exactly as [`save`] writes it.
pub fn encode(state: &WindowState) -> String {
    serde_json::to_string_pretty(&Persisted { version: STATE_FORMAT_VERSION, state: *state })
        .expect("WindowState is a fixed struct of primitives and cannot fail to serialise")
}

/// Write the state file atomically.
///
/// Write-then-rename because the alternative loses the user's layout whenever
/// the machine dies mid-write, and a truncated JSON file reads back as
/// [`StateLoad::Corrupt`] on every subsequent launch until someone deletes it.
pub fn save(path: &Path, state: &WindowState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, encode(state))?;
    std::fs::rename(&tmp, path)
}
