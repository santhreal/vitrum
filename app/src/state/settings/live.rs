//! Carrying a settings change to the parts of the running program that cannot
//! see the state signal.
//!
//! Most of the interface needs nothing here. The shell reads [`Settings`] out
//! of the state signal, so theme, density, motion, text scale and the sidebar
//! chips repaint on their own the moment a control writes to it.
//!
//! The pane does not. It is a native surface with its own renderer, driven by
//! a widget that is not part of any component tree, and a change to the font
//! or the palette has to reach it as a call. That is what this is: one
//! publish, and two derived snapshots that anything outside the tree can
//! subscribe to.
//!
//! # Why the snapshots are derived and compared
//!
//! Every control in the settings sheet routes through one commit, and one of
//! those controls is a text field. Publishing raw settings would hand the pane
//! a rebuild per character typed into the daemon URL, which is exactly the
//! shape of work that made this product feel slow. So a publish computes the
//! derived snapshot first and fans out only when the snapshot it produced
//! differs from the one already in force. Typing into a field that no
//! subscriber reads costs one derivation and zero notifications.
//!
//! # What is not on the bus
//!
//! The chord table. Key dispatch folds [`KeyboardPrefs`] and the saved presets
//! itself, and the fold lives beside the sheet that edits it. The bus carries
//! the two inputs and a change count; it does not import the fold, because
//! that would put a settings page's code inside the state layer.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use parking_lot::{Mutex, MutexGuard};

use super::hostterm;
use super::{CursorShape, Density, KeyboardPrefs, PresentMode, Settings, StartupPrefs, ThemePref};
use crate::launch::SavedPreset;

/// Twenty colours as the renderer uploads them.
///
/// `[u8; 4]` in `r, g, b, a` order, straight sRGB, not premultiplied: the
/// layout the grid's vertex attribute already expects, so no conversion
/// happens on a paint. Strings are deliberately absent. A hex string that
/// fails to parse at paint time has nowhere to go, so parsing happens once,
/// here, where a failure can fall back to the built-in palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanePalette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the eight
    /// bright variants. SGR order, indexed by colour number.
    pub ansi: [[u8; 4]; 16],
    pub background: [u8; 4],
    pub foreground: [u8; 4],
    pub cursor: [u8; 4],
    pub selection_bg: [u8; 4],
    /// Text drawn inside a selection. No terminal configuration format in use
    /// declares one, so it is the palette's own foreground.
    pub selection_fg: [u8; 4],
}

/// Everything the pane reads out of the settings document.
///
/// A snapshot and not a borrow. The pane runs on the GTK main loop and the
/// settings sheet writes from a component body; handing the pane a reference
/// into the state signal would make every paint a lock.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneSettings {
    /// `None` means follow the application theme, which is what the renderer's
    /// own defaults already do.
    pub palette: Option<PanePalette>,
    /// Font stack, verbatim. Empty means the platform's default monospace.
    pub font_family: String,
    /// Clamped into the range the cell grid can divide a viewport by.
    pub font_size_px: u16,
    pub line_height_pct: u16,
    pub cell_width_pct: u16,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    pub blink_interval_ms: u16,
    pub wheel_lines: u8,
    pub bracketed_paste: bool,
    pub scrollback_lines: u32,
    /// Clamped by the pane to what the adapter reports.
    pub present_mode: PresentMode,
    /// Cell background alpha, as a percentage. Below 100 the grid is drawn
    /// against whatever is behind the window.
    pub opacity_pct: u8,
    pub show_history_notice: bool,
    pub show_startup_errors: bool,
    /// Milliseconds a notice stays up, or `None` to wait for a dismissal.
    pub notice_life_ms: Option<u64>,
}

/// Everything outside the component tree reads out of the settings document.
///
/// The window frame, the tray and the boot surface all live outside Dioxus and
/// all read preferences. They get the same treatment as the pane rather than a
/// second mechanism.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellSettings {
    pub theme: ThemePref,
    pub density: Density,
    pub reduce_motion: bool,
    pub text_scale_pct: u16,
    pub show_branch: bool,
    pub show_place: bool,
    pub show_worktree: bool,
    pub show_time: bool,
    pub show_status_word: bool,
    pub show_status_bar: bool,
    pub always_slim: bool,
    pub show_restart_to_update: bool,
    pub startup: StartupPrefs,
    /// Milliseconds a flash stays up, or `None` to wait for a dismissal.
    pub flash_life_ms: Option<u64>,
    /// Window chrome alpha, as a percentage.
    pub opacity_pct: u8,
}

impl PaneSettings {
    /// Fold the document down to what a pane paints with.
    #[must_use]
    pub fn derive(settings: &Settings) -> PaneSettings {
        let t = &settings.terminal;
        let alpha = alpha_of(settings.appearance.terminal_opacity_pct);
        PaneSettings {
            palette: pane_palette(settings, alpha),
            font_family: t.font_family.clone(),
            font_size_px: t
                .font_size_px
                .clamp(super::TERM_FONT_MIN_PX, super::TERM_FONT_MAX_PX),
            line_height_pct: t
                .line_height_pct
                .clamp(super::LINE_HEIGHT_MIN_PCT, super::LINE_HEIGHT_MAX_PCT),
            cell_width_pct: t
                .cell_width_pct
                .clamp(super::CELL_WIDTH_MIN_PCT, super::CELL_WIDTH_MAX_PCT),
            cursor_shape: t.cursor_shape,
            cursor_blink: t.cursor_blink,
            blink_interval_ms: t
                .blink_interval_ms
                .clamp(super::BLINK_MIN_MS, super::BLINK_MAX_MS),
            wheel_lines: t.wheel_lines.clamp(1, super::WHEEL_LINES_MAX),
            bracketed_paste: t.bracketed_paste,
            scrollback_lines: t.scrollback_lines.min(super::SCROLLBACK_MAX_LINES),
            present_mode: t.present_mode,
            opacity_pct: settings.appearance.terminal_opacity_pct,
            show_history_notice: settings.notices.show_history_notice,
            show_startup_errors: settings.notices.show_startup_errors,
            notice_life_ms: settings.notices.notice_life_ms(),
        }
    }
}

impl ShellSettings {
    /// Fold the document down to what the window frame reads.
    #[must_use]
    pub fn derive(settings: &Settings) -> ShellSettings {
        ShellSettings {
            theme: settings.theme,
            density: settings.density,
            reduce_motion: settings.reduce_motion,
            text_scale_pct: settings.text_scale_pct,
            show_branch: settings.show_branch,
            show_place: settings.show_place,
            show_worktree: settings.show_worktree,
            show_time: settings.show_time,
            show_status_word: settings.show_status_word,
            show_status_bar: settings.show_status_bar,
            always_slim: settings.always_slim,
            show_restart_to_update: settings.show_restart_to_update,
            startup: settings.startup,
            flash_life_ms: settings.notices.flash_life_ms(),
            opacity_pct: settings.appearance.opacity_pct,
        }
    }
}

/// The palette the grid paints with, host import first.
///
/// Three answers in order. An import that is in force wins, because the
/// operator asked for their own colours and a built-in scheme sitting
/// underneath is not a fallback they want. A named scheme is next. `None` is
/// last and means the renderer keeps its own defaults, which is what a fresh
/// profile has always done.
fn pane_palette(settings: &Settings, alpha: u8) -> Option<PanePalette> {
    let t = &settings.terminal;
    if t.host_palette_in_force() {
        let host = &t.host_palette;
        let mut ansi = [[0u8, 0, 0, 255]; 16];
        for (slot, colour) in ansi.iter_mut().zip(host.ansi.iter()) {
            *slot = hostterm::to_rgba(colour, 255)?;
        }
        let foreground = hostterm::to_rgba(&host.foreground, 255)?;
        return Some(PanePalette {
            ansi,
            background: hostterm::to_rgba(&host.background, alpha)?,
            foreground,
            cursor: hostterm::to_rgba(host.cursor_or_foreground(), 255)?,
            selection_bg: hostterm::to_rgba(host.selection_or_foreground(), 255)?,
            selection_fg: foreground,
        });
    }
    let c = t.palette.colours()?;
    let mut ansi = [[0u8, 0, 0, 255]; 16];
    for (slot, colour) in ansi.iter_mut().zip(c.ansi.iter()) {
        *slot = css_rgba(colour, 255)?;
    }
    let foreground = css_rgba(c.foreground, 255)?;
    Some(PanePalette {
        ansi,
        background: css_rgba(c.background, alpha)?,
        foreground,
        cursor: css_rgba(c.cursor, 255)?,
        selection_bg: css_rgba(c.selection, 255)?,
        selection_fg: foreground,
    })
}

/// A percentage as an eight-bit alpha, rounded rather than truncated.
///
/// Truncating makes 100% come out as 254, which is a translucent window on an
/// install that asked for an opaque one and a compositor pass nobody wanted.
fn alpha_of(pct: u8) -> u8 {
    u8::try_from((u16::from(pct.min(100)) * 255 + 50) / 100).unwrap_or(255)
}

/// One colour from the built-in table, which writes `#rrggbb` for solid
/// colours and `rgba(r, g, b, a)` for the selection wash.
fn css_rgba(colour: &str, alpha: u8) -> Option<[u8; 4]> {
    if let Some(rest) = colour
        .trim()
        .strip_prefix("rgba(")
        .and_then(|r| r.strip_suffix(')'))
    {
        let mut parts = rest.split(',').map(str::trim);
        let r = parts.next()?.parse::<u8>().ok()?;
        let g = parts.next()?.parse::<u8>().ok()?;
        let b = parts.next()?.parse::<u8>().ok()?;
        let a = parts.next()?.parse::<f32>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        return Some([r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8]);
    }
    hostterm::to_rgba(colour, alpha)
}

// ═══════════════════════════════════════════════════════════════════════════
// The bus
// ═══════════════════════════════════════════════════════════════════════════

/// A pane listener.
type PaneFn = Arc<dyn Fn(&PaneSettings) + Send + Sync>;
/// A shell listener.
type ShellFn = Arc<dyn Fn(&ShellSettings) + Send + Sync>;
/// A keyboard listener, called with the two inputs the chord fold takes.
type KeyFn = Arc<dyn Fn(&KeyboardPrefs, &[SavedPreset]) + Send + Sync>;

struct Bus {
    pane: Arc<PaneSettings>,
    shell: Arc<ShellSettings>,
    keyboard: Arc<KeyboardPrefs>,
    presets: Arc<Vec<SavedPreset>>,
    pane_listeners: Vec<(u64, PaneFn)>,
    shell_listeners: Vec<(u64, ShellFn)>,
    key_listeners: Vec<(u64, KeyFn)>,
}

impl Bus {
    /// The listeners for one audience, cloned out so they can be called with
    /// the lock released.
    fn pane_calls(&self) -> Vec<PaneFn> {
        self.pane_listeners.iter().map(|(_, f)| Arc::clone(f)).collect()
    }

    fn shell_calls(&self) -> Vec<ShellFn> {
        self.shell_listeners.iter().map(|(_, f)| Arc::clone(f)).collect()
    }

    fn key_calls(&self) -> Vec<KeyFn> {
        self.key_listeners.iter().map(|(_, f)| Arc::clone(f)).collect()
    }
}

static BUS: LazyLock<Mutex<Bus>> = LazyLock::new(|| {
    let settings = Settings::default();
    Mutex::new(Bus {
        pane: Arc::new(PaneSettings::derive(&settings)),
        shell: Arc::new(ShellSettings::derive(&settings)),
        keyboard: Arc::new(settings.keyboard.clone()),
        presets: Arc::new(Vec::new()),
        pane_listeners: Vec::new(),
        shell_listeners: Vec::new(),
        key_listeners: Vec::new(),
    })
});

/// Take the bus.
///
/// A listener that panics must not take every later settings change with it.
/// `parking_lot` does not poison, so a panic in a callback leaves the next
/// caller a plain lock rather than an error path at every call site. The data
/// behind it is four snapshots and three lists, none of which a panic can
/// leave half-written.
fn bus() -> MutexGuard<'static, Bus> {
    BUS.lock()
}

/// Next subscription id. Monotonic so a dropped subscription can never remove
/// a later one that reused its slot.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Publishes, and fan-outs per audience. Counters and not timers: what this
/// module is allowed to spend on a keystroke is a property of the code.
static PUBLISHES: AtomicUsize = AtomicUsize::new(0);
static PANE_FANOUTS: AtomicUsize = AtomicUsize::new(0);
static SHELL_FANOUTS: AtomicUsize = AtomicUsize::new(0);
static KEY_FANOUTS: AtomicUsize = AtomicUsize::new(0);

/// The bus, held for the body of one test.
///
/// There is one settings document and one preset list per process rather than
/// one per test, so two tests publishing at once each see the other's
/// listeners fire and read each other's presets. Both halves are reset on
/// acquisition rather than on release, so a test that panics while holding
/// this hands the next one a fresh profile instead of whatever it left.
#[cfg(test)]
pub struct BusLease(MutexGuard<'static, ()>);

#[cfg(test)]
impl Drop for BusLease {
    fn drop(&mut self) {
        // Naming the guard is what says the lock is released here, at the end
        // of the test body, and not at the last publish inside it.
        let _ = &self.0;
        HOLDS_BUS.with(|h| h.set(false));
    }
}

#[cfg(test)]
static BUS_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
thread_local! {
    static HOLDS_BUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Take the bus for one test.
///
/// [`publish`] and [`publish_presets`] refuse to run without this, because a
/// helper a test can forget to call fixes one race and leaves the next one to
/// be found the same way: as a red run on a machine nobody is watching.
#[cfg(test)]
#[must_use]
pub fn exclusive() -> BusLease {
    let guard = BUS_TEST_LOCK.lock();
    HOLDS_BUS.with(|h| h.set(true));
    let lease = BusLease(guard);
    publish(&Settings::default());
    publish_presets(&[]);
    lease
}

/// Refuse a publish from a test that did not take the bus.
#[cfg(test)]
fn assert_leased(what: &str) {
    assert!(
        HOLDS_BUS.with(std::cell::Cell::get),
        "{what} without crate::state::live::exclusive(): the bus is \
         process-global, so this would run inside another test's profile and \
         put its listeners on this test's changes"
    );
}

/// Hand a settings document to everything outside the component tree.
///
/// Called by the settings sheet's commit, and once at startup after the
/// profile is restored so a pane created later starts on the right values.
///
/// Each audience is notified only when its own derived snapshot changed.
/// Listeners run outside the lock, so a listener that publishes cannot
/// deadlock and a slow listener cannot block a second publisher.
pub fn publish(settings: &Settings) {
    #[cfg(test)]
    assert_leased("publish");
    PUBLISHES.fetch_add(1, Ordering::Relaxed);
    let pane = PaneSettings::derive(settings);
    let shell = ShellSettings::derive(settings);

    let mut pane_call: Option<(Arc<PaneSettings>, Vec<PaneFn>)> = None;
    let mut shell_call: Option<(Arc<ShellSettings>, Vec<ShellFn>)> = None;
    let mut key_call: Option<(Arc<KeyboardPrefs>, Arc<Vec<SavedPreset>>, Vec<KeyFn>)> = None;
    {
        let mut bus = bus();
        if *bus.pane != pane {
            bus.pane = Arc::new(pane);
            pane_call = Some((Arc::clone(&bus.pane), bus.pane_calls()));
        }
        if *bus.shell != shell {
            bus.shell = Arc::new(shell);
            shell_call = Some((Arc::clone(&bus.shell), bus.shell_calls()));
        }
        if *bus.keyboard != settings.keyboard {
            bus.keyboard = Arc::new(settings.keyboard.clone());
            key_call = Some((
                Arc::clone(&bus.keyboard),
                Arc::clone(&bus.presets),
                bus.key_calls(),
            ));
        }
    }
    deliver(pane_call, shell_call, key_call);
}

/// Hand the saved commands to key dispatch.
///
/// Separate from [`publish`] because presets are not part of the settings
/// document: they are their own file, written by their own editor, and a
/// preset's chord is matched out of the same table the built-in chords are.
/// Without this the shortcut an operator just bound does nothing until the
/// next launch.
pub fn publish_presets(presets: &[SavedPreset]) {
    #[cfg(test)]
    assert_leased("publish_presets");
    PUBLISHES.fetch_add(1, Ordering::Relaxed);
    let mut key_call = None;
    {
        let mut bus = bus();
        if bus.presets.as_slice() != presets {
            bus.presets = Arc::new(presets.to_vec());
            key_call = Some((
                Arc::clone(&bus.keyboard),
                Arc::clone(&bus.presets),
                bus.key_calls(),
            ));
        }
    }
    deliver(None, None, key_call);
}

/// Call the listeners, with the lock released.
fn deliver(
    pane: Option<(Arc<PaneSettings>, Vec<PaneFn>)>,
    shell: Option<(Arc<ShellSettings>, Vec<ShellFn>)>,
    keyboard: Option<(Arc<KeyboardPrefs>, Arc<Vec<SavedPreset>>, Vec<KeyFn>)>,
) {
    if let Some((now, calls)) = pane
        && !calls.is_empty()
    {
        PANE_FANOUTS.fetch_add(1, Ordering::Relaxed);
        for f in calls {
            f(&now);
        }
    }
    if let Some((now, calls)) = shell
        && !calls.is_empty()
    {
        SHELL_FANOUTS.fetch_add(1, Ordering::Relaxed);
        for f in calls {
            f(&now);
        }
    }
    if let Some((prefs, presets, calls)) = keyboard
        && !calls.is_empty()
    {
        KEY_FANOUTS.fetch_add(1, Ordering::Relaxed);
        for f in calls {
            f(&prefs, &presets);
        }
    }
}

/// What a pane created right now should paint with.
#[must_use]
pub fn pane_settings() -> Arc<PaneSettings> {
    Arc::clone(&bus().pane)
}

/// What the window frame should read right now.
#[must_use]
pub fn shell_settings() -> Arc<ShellSettings> {
    Arc::clone(&bus().shell)
}

/// The rebindings in force right now.
#[must_use]
pub fn keyboard_prefs() -> Arc<KeyboardPrefs> {
    Arc::clone(&bus().keyboard)
}

/// The saved commands in force right now.
#[must_use]
pub fn presets() -> Arc<Vec<SavedPreset>> {
    Arc::clone(&bus().presets)
}

/// Which list a subscription belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Audience {
    Pane,
    Shell,
    Keyboard,
}

/// A live subscription. Dropping it unsubscribes.
///
/// Returned rather than an id the caller has to remember to hand back. A pane
/// that is closed while a publish is in flight would otherwise keep a
/// callback holding a widget that no longer exists.
#[must_use = "dropping the subscription unsubscribes immediately"]
pub struct Subscription {
    id: u64,
    audience: Audience,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut bus = bus();
        match self.audience {
            Audience::Pane => bus.pane_listeners.retain(|(id, _)| *id != self.id),
            Audience::Shell => bus.shell_listeners.retain(|(id, _)| *id != self.id),
            Audience::Keyboard => bus.key_listeners.retain(|(id, _)| *id != self.id),
        }
    }
}

/// Hear about every change to what a pane paints with.
///
/// The callback is invoked once immediately with the current value, so a
/// subscriber never has to fetch and then subscribe and handle the change that
/// landed between the two.
pub fn subscribe_pane(f: impl Fn(&PaneSettings) + Send + Sync + 'static) -> Subscription {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let f: PaneFn = Arc::new(f);
    let now = {
        let mut bus = bus();
        bus.pane_listeners.push((id, Arc::clone(&f)));
        Arc::clone(&bus.pane)
    };
    f(&now);
    Subscription {
        id,
        audience: Audience::Pane,
    }
}

/// Hear about every change the window frame reads.
pub fn subscribe_shell(f: impl Fn(&ShellSettings) + Send + Sync + 'static) -> Subscription {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let f: ShellFn = Arc::new(f);
    let now = {
        let mut bus = bus();
        bus.shell_listeners.push((id, Arc::clone(&f)));
        Arc::clone(&bus.shell)
    };
    f(&now);
    Subscription {
        id,
        audience: Audience::Shell,
    }
}

/// Hear about every rebinding and every saved command.
///
/// One audience for both, because there is one chord table and it is folded
/// from both. A subscriber that heard about them separately would rebuild the
/// table twice for one change and, worse, could hold a table folded from a
/// stale half.
pub fn subscribe_keyboard(
    f: impl Fn(&KeyboardPrefs, &[SavedPreset]) + Send + Sync + 'static,
) -> Subscription {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let f: KeyFn = Arc::new(f);
    let (prefs, presets) = {
        let mut bus = bus();
        bus.key_listeners.push((id, Arc::clone(&f)));
        (Arc::clone(&bus.keyboard), Arc::clone(&bus.presets))
    };
    f(&prefs, &presets);
    Subscription {
        id,
        audience: Audience::Keyboard,
    }
}

/// How many documents have been published.
#[must_use]
pub fn publishes() -> usize {
    PUBLISHES.load(Ordering::Relaxed)
}

/// How many publishes reached a pane listener.
#[must_use]
pub fn pane_fanouts() -> usize {
    PANE_FANOUTS.load(Ordering::Relaxed)
}

/// How many publishes reached a shell listener.
#[must_use]
pub fn shell_fanouts() -> usize {
    SHELL_FANOUTS.load(Ordering::Relaxed)
}

/// How many publishes reached a keyboard listener.
#[must_use]
pub fn keyboard_fanouts() -> usize {
    KEY_FANOUTS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests;
