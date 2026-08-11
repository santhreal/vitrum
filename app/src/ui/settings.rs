//! The settings modal, and the pure folds that turn each preference into a
//! real effect.
//!
//! The preference DATA lives in [`crate::state::Settings`], because settings
//! are state and every window must agree about them. This file owns the other
//! half: the seven-tab sheet that edits them, and the mechanism by which each
//! one actually changes the running program.
//!
//! One rule governs the whole file, and it is why several plausible controls
//! are missing: **a control ships only if flipping it changes observable
//! behaviour immediately and survives a restart.** A switch that renders and
//! does nothing is worse than no switch, because it teaches the operator that
//! the settings are decoration.
//!
//! # How each group becomes real
//!
//! - **Appearance** is three CSS mechanisms and no Rust bookkeeping. Theme is
//!   `data-theme` on the app root, which `sidebar.css` already keys its light
//!   palette off. Density is [`root_style`], a short list of `--rg-*` token
//!   the generated sheet. Text scale multiplies every length in it, so one
//!   number scales the whole shell without this module knowing any of them.
//! - **Sidebar** is `show_branch` / `show_place` / `show_time` /
//!   `show_status_word`, read by
//!   `ui/sidebar.rs`, plus the auto-settle window, which is
//!   [`vitrum_model::DispositionPolicy`] and therefore governs every
//!   disposition, section, rollup and traversal decision in the product.
//! - **Workspaces** drives [`crate::state::WorkspaceSet`] directly: create,
//!   rename, delete, reorder, grouping mode, band visibility, folders.
//! - **Terminal** is [`crate::state::TerminalPrefs`], read by the pane when it
//!   paints. Font, size, scrollback and palette are all reconfigurable without
//!   a restart, so none of them is a "takes effect next launch" setting.
//! - **Notifications** is [`should_notify`] plus [`notable_transitions`], which
//!   is edge-triggered. Level-triggered notification is the classic defect
//!   here: at twenty agents a predicate re-evaluated on every snapshot
//!   re-notifies about the same blocked session several times a second.
//! - **Keyboard** is [`effective_chords`], the one table key dispatch matches
//!   against. [`crate::ui::shortcuts`] renders from the same fold, so a
//!   rebound chord can never be advertised as its default.
//! - **Advanced** is the daemon URL, which reconnects the socket on the spot,
//!   and a live [`vitrum_os::probe`] report.
//!
//! # What deliberately has no control
//!
//! - **Per-window preferences.** Everything in `Settings` is app-global on
//!   purpose. Two windows disagreeing about a keybinding is incoherent. The
//!   two things that genuinely vary by context, grouping and band visibility,
//!   live on the workspace instead and appear under the Workspaces tab.
//! - **Live keystroke capture for rebinding.** See [`BINDABLE_KEYS`].
//! - **A row-variant preference.** Asked for, and deliberately not built. There
//!   are exactly two row variants in `sidebar.css`, card and slim, and which
//!   one a row gets is a function of its band — Active is a card, Snoozed and
//!   Settled are slim — not a taste. A three-way picker would have had nothing
//!   to select for its third option, and a two-way one would have fought the
//!   band rule. The honest version of what that control was reaching for is
//!   the Compact density, which shrinks both variants together and is real.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use parking_lot::RwLock;

use serde::{Deserialize, Serialize};

use vitrum_model::{SessionView, SidebarStatus};
use vitrum_os::capability::{Support, Unavailable};
use vitrum_os::notify::{Notification, NotificationKind, Notifier};
use vitrum_proto::{SessionId, SessionStatus};

use crate::keymap::{CHORDS, Help, KeyAction, Scope, Shift};
use crate::state::{
    KeyboardPrefs, TERM_FONT_MAX_PX, TERM_FONT_MIN_PX, TerminalPrefs, ThemePref, UiState,
};
#[cfg(test)]
use crate::state::Settings;

/// The About tab. Edits no preference; it reports what is installed.
mod about;
/// Saved commands: the validation rules and the editor that applies them.
mod presets;
/// The Workspaces tab, the one panel that edits no `Settings` field.
mod workspaces;
/// The declarative row registry the GTK sheet draws from.
mod spec;
/// The GTK settings sheet, presented as a dialog over the frame.
pub(crate) mod sheet;

// ═══════════════════════════════════════════════════════════════════════════
// Appearance
// ═══════════════════════════════════════════════════════════════════════════

/// UI scale steps, in percent of the platform's default root font size.
///
/// Discrete rather than a slider. A slider over a continuous range invites
/// 103%, which is a half-pixel rounding difference on every border in the
/// window and reads as a rendering fault rather than as a choice. The range
/// tops out at 200% because the target panel is 3840x2160 at 162 DPI, where the
/// unscaled shell renders physically half-size.
pub const UI_SCALE_STEPS: &[u16] = &[80, 90, 100, 110, 125, 150, 175, 200];

/// Opacity steps offered, in percent.
///
/// [`crate::state::OPACITY_MAX_PCT`] must be first: it is the default, and a
/// `<select>` whose stored value matches no option silently shows the first
/// one instead, which would make a fully opaque install read as translucent.
/// The floor is [`crate::state::OPACITY_MIN_PCT`] for the reason given there.
/// Coarse below 80 because nobody distinguishes 62% from 65%, and every step
/// is a row in a list the operator has to read.
pub const OPACITY_STEPS: [u8; 10] = [100, 95, 90, 85, 80, 70, 60, 50, 35, 20];

/// Backdrop blur radii offered, in CSS pixels.
pub const BLUR_STEPS: [u8; 7] = [0, 4, 8, 16, 24, 40, 64];

/// Backdrop scrim strengths offered, in percent.
pub const DIM_STEPS: [u8; 8] = [0, 10, 20, 30, 40, 50, 65, 80];

/// The label for one blur step.
#[must_use]
pub fn blur_label(px: u8) -> String {
    if px == 0 {
        "None".to_string()
    } else {
        format!("{px}px")
    }
}

/// What the window-opacity control can and cannot do right now.
///
/// Two separate truths, and the control is dishonest if it states only one.
///
/// A window is created see-through or it is not: both the platform flag and
/// the toplevel's RGBA visual are settled before the first paint. So the first
/// move away from a fully opaque profile needs a new window, and every move
/// after that is live.
///
/// And what shows through is the desktop *unblurred*. No application can blur
/// what is behind its own window; the compositor owns that, and on Wayland
/// there is deliberately no protocol to ask. So the frosted look is real and
/// good, but the operator's compositor supplies it, and the honest thing is to
/// name the rule rather than ship a slider that implies we did it.
#[must_use]
pub fn opacity_note(a: &crate::state::AppearancePrefs) -> &'static str {
    if a.needs_transparent_window() {
        "How much of the desktop shows through, unblurred. Blur belongs to your \
         compositor: Hyprland, KWin and picom can all frost this window, and \
         docs/appearance.md has the one-line rule for each. Without a compositor \
         running, a see-through window has nothing to blend with."
    } else {
        "How much of the desktop shows through, unblurred. The window is created \
         opaque, so the first change here applies to the next window you open; \
         after that it moves live. Blur belongs to your compositor; \
         docs/appearance.md has the rule for Hyprland, KWin and picom."
    }
}

/// The desktop's appearance, cached, or `None` when it cannot be read.
///
/// Cached because reading it is a D-Bus round trip through the portal, and this
/// is called on every render of the shell. Refreshable rather than a one-shot
/// `LazyLock<Option<_>>` so the Appearance tab can offer a re-read; a
/// permanently frozen answer would make "System" mean "whatever the system said
/// the first time you launched", which is not what it says on the control.
///
/// No background watcher. Subscribing would mean a thread parked on a D-Bus
/// signal for the life of the process, and the whole product is an argument
/// about what a terminal shell is allowed to do while nothing is happening.
#[must_use]
pub fn system_theme() -> Option<vitrum_os::theme::Theme> {
    *SYSTEM_THEME.read()
}

/// Ask the desktop again and update the cache.
#[must_use]
pub fn refresh_system_theme() -> Option<vitrum_os::theme::Theme> {
    let read = read_system_theme();
    *SYSTEM_THEME.write() = read;
    read
}

static SYSTEM_THEME: LazyLock<RwLock<Option<vitrum_os::theme::Theme>>> =
    LazyLock::new(|| RwLock::new(read_system_theme()));

/// How many portal round trips this process has spent on the desktop's
/// appearance.
///
/// The cost this cache exists to avoid, counted rather than assumed, so a
/// change that quietly turns one read into one per render is a failing test
/// and not a report about a warm laptop.
static SYSTEM_THEME_READS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Portal round trips spent on the desktop's appearance so far.
///
/// Shipped rather than test-only, because the Live apply block is where an
/// operator on a slow desktop portal finds out whether the cost is one round
/// trip or one per render.
#[must_use]
pub fn system_theme_reads() -> usize {
    SYSTEM_THEME_READS.load(std::sync::atomic::Ordering::Relaxed)
}

fn read_system_theme() -> Option<vitrum_os::theme::Theme> {
    SYSTEM_THEME_READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    vitrum_os::theme::theme_watcher()
        .ok()
        .and_then(|w| w.current().ok())
}

/// Re-read the desktop's appearance the moment the operator asks to follow it.
///
/// [`system_theme`] is cached because reading it is a D-Bus round trip and
/// every generated sheet resolves the scheme, so the cache is filled once at
/// the first sheet of the first window and never again. An operator who switches
/// to "Follow the system" an hour later is then painted with the answer the
/// desktop gave at launch. The cache is process-wide and lives outside the
/// component tree, so nothing the sheet renders can reach it. The shell half
/// of [`crate::state::live`] is what carries the change to it.
///
/// Only the move INTO [`ThemePref::System`] refreshes. Following the system
/// already, or leaving it, must not put a portal round trip on the path every
/// settings commit takes.
static SHELL_WATCH: LazyLock<crate::state::live::Subscription> = LazyLock::new(|| {
    let was = RwLock::new(crate::state::live::shell_settings().theme);
    crate::state::live::subscribe_shell(move |now| {
        let before = std::mem::replace(&mut *was.write(), now.theme);
        if now.theme == ThemePref::System && before != ThemePref::System {
            let _ = refresh_system_theme();
        }
    })
});

/// Install the shell subscription, once per process.
///
/// A plain function rather than a hook body, because the subscription belongs
/// to the process and not to a window: two windows installing one each would
/// make one theme change two portal round trips.
pub fn watch_shell() {
    LazyLock::force(&SHELL_WATCH);
}

/// Force a stored scale into the range the modal can express.
///
/// A hand-edited settings file with `"textScalePct": 4000` must not produce a
/// window whose first row is taller than the screen, which would put the
/// settings gear permanently out of reach.
#[must_use]
#[cfg(test)]
pub fn clamp_scale(percent: u16) -> u16 {
    percent.clamp(
        crate::state::TEXT_SCALE_MIN_PCT,
        crate::state::TEXT_SCALE_MAX_PCT,
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Terminal
// ═══════════════════════════════════════════════════════════════════════════

/// What the Colours row says under the control.
///
/// The light palettes get a warning rather than a block. Putting a white grid
/// in a dark window is a legitimate choice, but it is also the shape of an
/// accidental pick, and an operator who did not mean it should be told what
/// happened without having to undo it to find out.
#[must_use]
pub fn palette_note(palette: crate::termpalette::TermPalette) -> String {
    use crate::termpalette::TermPalette;
    if palette == TermPalette::Inherit {
        return "The grid follows the app theme and the agent's own ANSI colours are left \
                to the terminal's defaults. Pick a palette to pin all sixteen."
            .to_string();
    }
    let base = format!(
        "{} paints the grid only. The window frame, the sidebar and every dialog stay on \
         the app theme.",
        palette.label()
    );
    if palette.is_light() {
        return format!(
            "{base} This is a light palette, so the grid will be pale whatever the app \
             theme is."
        );
    }
    base
}

/// What the follow-the-host-terminal row says under the switch.
///
/// Three states, and each needs a different sentence. Off says what turning it
/// on would do. On with a usable import names the file the colours came out
/// of, because a machine with several terminals installed has several answers
/// and only one of them is on screen. On without a usable import says the
/// switch is doing nothing right now, which is the state a profile copied
/// between machines lands in.
#[must_use]
pub fn host_palette_note(prefs: &TerminalPrefs) -> String {
    if !prefs.follow_host_terminal {
        return "Reads the sixteen ANSI colours out of the configuration file of the terminal \
                installed on this machine and paints the grid with them. Overrides the palette \
                above while it is on."
            .to_string();
    }
    if prefs.host_palette.is_complete() {
        return format!(
            "Painting with the colours read from {}, a {}. The palette above is ignored while \
             this is on.",
            prefs.host_palette.origin,
            prefs.host_palette.source.label()
        );
    }
    "On, but no complete palette has been imported, so the palette above is still what the \
     grid paints. Turn this off and on again to scan."
        .to_string()
}

/// Scan this machine for a terminal palette.
///
/// The environment and the filesystem, read here rather than in
/// [`crate::state::hostterm`], which takes both as arguments so its own tests
/// can run against fixtures without touching either.
fn import_host_palette()
-> Result<crate::state::hostterm::HostPalette, crate::state::hostterm::ImportError> {
    let env = std::env::vars().collect();
    // Annotated rather than passed by name. `read_to_string` is generic over
    // `AsRef<Path>`, so handing it over directly makes the compiler pick one
    // concrete lifetime, and `import` needs a reader good for any.
    crate::state::hostterm::import(&env, |path: &std::path::Path| {
        std::fs::read_to_string(path)
    })
}

/// Client-side scrollback choices, in lines, with what each one costs.
///
/// The server owns real history; this is only the local viewport buffer that
/// makes the wheel work between repaints. Raising it costs resident memory in
/// this process, which is the number the product competes on, so every step is
/// labelled with that cost rather than presented as free.
pub const SCROLLBACK_STEPS: &[(u32, &str)] = &[
    (1_000, "1,000 lines — the shipped default"),
    (5_000, "5,000 lines"),
    (20_000, "20,000 lines"),
    (100_000, "100,000 lines — tens of MB in this process"),
];

/// Monospace stacks the operator can pick between.
///
/// Every stack ends in the generic `monospace`, so a font that is not installed
/// degrades to the platform's default monospace rather than to a proportional
/// face, which would destroy the character grid. The first entry is empty,
/// meaning "whatever `--rg-font-mono` resolves to", which is how the setting
/// says "no opinion" without this table duplicating the stylesheet's stack.
pub const FONT_STACKS: &[(&str, &str)] = &[
    ("Stylesheet default", ""),
    (
        "JetBrains Mono",
        "\"JetBrains Mono\", ui-monospace, monospace",
    ),
    ("Fira Code", "\"Fira Code\", ui-monospace, monospace"),
    (
        "IBM Plex Mono",
        "\"IBM Plex Mono\", ui-monospace, monospace",
    ),
    (
        "Source Code Pro",
        "\"Source Code Pro\", ui-monospace, monospace",
    ),
    (
        "Cascadia Code",
        "\"Cascadia Code\", ui-monospace, monospace",
    ),
    ("Menlo", "Menlo, ui-monospace, monospace"),
    ("Consolas", "Consolas, ui-monospace, monospace"),
];

/// Terminal font sizes the modal offers.
///
/// A menu of round numbers, not the whole range: the range is
/// [`TERM_FONT_MIN_PX`]..=[`TERM_FONT_MAX_PX`], owned by the settings struct
/// because [`TerminalPrefs::clamp`] enforces it against a hand-edited file
/// that never went through this menu.
pub const TERM_FONT_STEPS: &[u16] = &[9, 10, 11, 12, 13, 14, 16, 18, 20, 24];

/// The size the pane paints at, clamped into the range the modal offers.
///
/// A hand-edited `ui.json` can carry any `u16`, and the modal is not the only
/// way a value reaches the pane. Every reader goes through here so a text
/// editor cannot produce a zero-width cell box, which blanks the pane with
/// nothing logged anywhere.
#[must_use]
pub fn term_font_px(prefs: &TerminalPrefs) -> u16 {
    prefs.font_size_px.clamp(TERM_FONT_MIN_PX, TERM_FONT_MAX_PX)
}

/// Line box heights the modal offers, in percent of the font's own height.
///
/// 100% is the face's metrics untouched. Below that lines collide on a face
/// with tall accents; above it a transcript reads like prose. Both are real
/// preferences and neither is safe to make the default.
pub const LINE_HEIGHT_STEPS: &[u16] = &[90, 100, 110, 120, 130, 140, 160];

/// Cell box widths the modal offers, in percent of the font's own advance.
///
/// The grid is monospaced, so this is not tracking: it is how much of the
/// advance the cell claims. Under 100% a wide face packs more columns into
/// the same window at the cost of glyphs touching.
pub const CELL_WIDTH_STEPS: &[u16] = &[90, 95, 100, 105, 110, 120];

/// Cursor blink periods the modal offers, in milliseconds.
///
/// A full period: the cursor is visible for half of it. 530 ms is the shipped
/// default and is the interval a hardware terminal blinked at, which is why
/// every emulator since has used it.
pub const BLINK_STEPS: &[u16] = &[300, 400, 530, 700, 1_000, 1_400, 2_000];

/// Lines one wheel notch scrolls.
///
/// The range is 1..=[`WHEEL_LINES_MAX`], so there is no "one screen" step:
/// a notch that scrolls a whole screen loses the line you were reading, and
/// paging has its own keys.
pub const WHEEL_STEPS: &[u8] = &[1, 2, 3, 4, 5, 8, 12];

// ═══════════════════════════════════════════════════════════════════════════
// Notifications
// ═══════════════════════════════════════════════════════════════════════════

/// Every notification kind, in the order the tab lists them.
pub const NOTIFY_KINDS: [NotificationKind; 3] = [
    NotificationKind::NeedsApproval,
    NotificationKind::Failed,
    NotificationKind::Finished,
];

/// Should this moment interrupt the operator?
#[must_use]
pub const fn should_notify(
    prefs: &crate::state::NotifyPrefs,
    kind: NotificationKind,
    session_is_focused: bool,
) -> bool {
    if prefs.skip_focused_session && session_is_focused {
        return false;
    }
    notify_enabled(prefs, kind)
}

/// Is the switch for `kind` on, ignoring focus?
#[must_use]
pub const fn notify_enabled(prefs: &crate::state::NotifyPrefs, kind: NotificationKind) -> bool {
    match kind {
        NotificationKind::Finished => prefs.finished,
        NotificationKind::NeedsApproval => prefs.needs_approval,
        NotificationKind::Failed => prefs.failed,
    }
}

/// Turn the switch for `kind` on or off.
pub const fn set_notify_enabled(
    prefs: &mut crate::state::NotifyPrefs,
    kind: NotificationKind,
    on: bool,
) {
    match kind {
        NotificationKind::Finished => prefs.finished = on,
        NotificationKind::NeedsApproval => prefs.needs_approval = on,
        NotificationKind::Failed => prefs.failed = on,
    }
}

/// What the row for `kind` says it is about.
#[must_use]
pub const fn notify_label(kind: NotificationKind) -> (&'static str, &'static str) {
    match kind {
        NotificationKind::NeedsApproval => (
            "Agent needs approval",
            "The agent is blocked on a yes or no. Critical urgency, and the desktop is asked not \
             to auto-dismiss it.",
        ),
        NotificationKind::Failed => (
            "Agent failed",
            "The child exited non-zero or was signalled. Critical urgency.",
        ),
        NotificationKind::Finished => (
            "Agent finished",
            "The agent's turn ended and you have not looked. Off by default: at twenty agents \
             this is the loudest of the three and the least urgent, because a finished agent is \
             not waiting on anything.",
        ),
    }
}

/// One moment worth interrupting the operator about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notable {
    pub session: SessionId,
    pub kind: NotificationKind,
    /// The session's title, unsanitised. [`Notification::new`] cleans it.
    pub title: String,
    /// One line of context, unsanitised.
    pub detail: String,
}

impl Notable {
    /// The `vitrum-os` payload for this moment.
    #[must_use]
    pub fn notification(&self) -> Notification {
        Notification::new(self.kind, self.session, &self.title, &self.detail)
    }
}

/// Moments that just became true, comparing two snapshots of the session list.
///
/// **Edge-triggered, and that is the whole point of the function.** The obvious
/// implementation asks "which sessions are blocked?" on every snapshot and
/// notifies about each. At twenty agents the daemon pushes a snapshot several
/// times a second, so one session blocked on a single approval would raise a
/// notification per snapshot until the operator answered it. Comparing against
/// `before` means each transition fires exactly once.
///
/// A session absent from `before` is never notable, however alarming it looks.
/// The list arrives whole on every reconnect, so treating unseen sessions as
/// fresh transitions would empty a day of failures onto the desktop the moment
/// the socket flaps. A genuinely new session was created by the operator, who
/// is by definition already looking at it.
///
/// At most one notification per session per snapshot, taken in severity order.
/// A child that exits non-zero satisfies both "failed" and "finished", and two
/// notifications about one event is how a product teaches people to mute it.
///
/// Linear in the two lists rather than quadratic. The previous snapshot was
/// scanned from the top for every row of the new one, which at twenty agents
/// on a daemon pushing several snapshots a second is 400 comparisons per push
/// to answer twenty questions. One index over the old rows answers all of them
/// in one pass, and it holds references rather than copies.
#[must_use]
pub fn notable_transitions(before: &[SessionView], after: &[SessionView]) -> Vec<Notable> {
    let was_by_id: HashMap<SessionId, &SessionView> =
        before.iter().map(|row| (row.id(), row)).collect();
    let mut out = Vec::new();
    for row in after {
        let Some(&was) = was_by_id.get(&row.id()) else {
            continue;
        };
        let kind = if failed(row) && !failed(was) {
            NotificationKind::Failed
        } else if row.blocks_on_operator() && !was.blocks_on_operator() {
            NotificationKind::NeedsApproval
        } else if row.has_unseen_completion() && !was.has_unseen_completion() {
            NotificationKind::Finished
        } else {
            continue;
        };
        out.push(Notable {
            session: row.id(),
            kind,
            // A desktop notification reading just "bash" cannot tell the
            // operator which of sixty shells failed, which is the one thing
            // the notification exists to say.
            title: crate::inbox::row_title(&row.info).into_owned(),
            detail: detail_for(kind, row),
        });
    }
    out
}

/// Did the child die badly?
///
/// A clean exit is not a failure however unexpected it was, and conflating the
/// two is what makes a notification stream worthless: the operator stops
/// reading "exited" because it is usually fine.
fn failed(row: &SessionView) -> bool {
    // `code: None` means the child was signalled, which is a death and not a
    // clean exit. Treating the unknown case as success is how a SIGSEGV gets
    // reported to the operator as "finished".
    matches!(row.info.status, SessionStatus::Exited { code } if code != Some(0))
        || row.status() == SidebarStatus::Failed
}

/// The one line of context under the title.
fn detail_for(kind: NotificationKind, row: &SessionView) -> String {
    match kind {
        NotificationKind::Failed => match row.info.status {
            SessionStatus::Exited { code: Some(code) } => {
                format!("{} exited {code}", row.info.command)
            }
            SessionStatus::Exited { code: None } => {
                format!("{} was signalled", row.info.command)
            }
            _ => format!("{} failed", row.info.command),
        },
        NotificationKind::NeedsApproval => row.info.cwd.clone(),
        NotificationKind::Finished => row.info.command.clone(),
    }
}

/// This platform's notification backend, connected once.
///
/// One connection for the life of the process. Connecting per notification
/// would put a D-Bus handshake on the path of an event that is already late by
/// the time anyone sees it, and on GNOME it would also re-register the desktop
/// entry each time.
///
/// The error is kept rather than discarded so the Notifications tab can say why
/// it cannot deliver instead of showing switches that quietly do nothing. That
/// is the whole reason `vitrum_os::notify::notifier` returns a `Result` instead
/// of a silent sink.
///
/// The click route is installed here, with the connection, rather than at
/// startup. This is the only moment that is provably before the first
/// notification exists: every delivery goes through [`notify_now`], which
/// forces this same lock, so a click can never arrive for a notification that
/// was raised before the route was in place. Nothing shipped installed it at
/// all until now, and on Linux the consequence was worse than an unrouted
/// click: the backend only subscribes to `ActionInvoked` when a handler is
/// installed, so every notification rendered a `Show` button with nothing
/// behind it.
static NOTIFIER: LazyLock<Result<Box<dyn Notifier>, Unavailable>> = LazyLock::new(|| {
    let backend = vitrum_os::notify::notifier()?;
    // `crate::activate_session` and nothing else: the backend calls this from
    // its own listener thread, so the handler may touch no signal and no
    // window. It posts to the activation queue, which is the same cross-thread
    // handoff a second launch's deep link already uses.
    if let Err(why) = backend.set_activation_handler(Arc::new(crate::activate_session)) {
        // The notifications themselves still work, and the D-Bus backend
        // answers a missing route by advertising no actions, so what is lost
        // here is the click and not a button that pretends to accept one.
        tracing::warn!("notification clicks cannot open a session: {why}");
    }
    Ok(backend)
});

/// Whether this desktop can deliver notifications, and why not if it cannot.
#[must_use]
pub fn notify_support() -> Support {
    match NOTIFIER.as_ref() {
        Ok(backend) => backend.capability(),
        Err(why) => Support::Missing(why.clone()),
    }
}

/// Deliver one notification, or do nothing if the desktop refused us.
///
/// Fire and forget. A notification that fails to deliver is logged and dropped:
/// there is no retry that would help, and a failure banner about a failure
/// notification is a loop.
pub fn notify_now(notification: &Notification) {
    match NOTIFIER.as_ref() {
        Ok(backend) => {
            if let Err(why) = backend.notify(notification) {
                tracing::warn!("notification not delivered: {why}");
            }
        }
        Err(why) => tracing::debug!("no notification backend: {why}"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Keyboard
// ═══════════════════════════════════════════════════════════════════════════

/// One chord, parsed out of the plain-text form the settings file stores.
///
/// Owned strings rather than the `&'static str` [`crate::keymap::Chord`] uses,
/// because a rebinding is chosen at runtime and cannot be a static.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    /// A DOM `KeyboardEvent.key`, lowercased, exactly as `CHORDS` stores it.
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Binding {
    /// The canonical text form, `ctrl+alt+shift+key`, as stored on disk.
    ///
    /// Modifiers always in that order and always lowercase, so two bindings
    /// that mean the same chord have the same string and a settings file
    /// written by one build reads identically in the next.
    #[must_use]
    pub fn encode(&self) -> String {
        let mut s = String::with_capacity(24);
        if self.ctrl {
            s.push_str("ctrl+");
        }
        if self.alt {
            s.push_str("alt+");
        }
        if self.shift {
            s.push_str("shift+");
        }
        s.push_str(&self.key);
        s
    }

    /// Parse the stored form. `None` for anything unrecognised.
    ///
    /// Returning `None` rather than a partial binding is the only behaviour
    /// that cannot lock a user out of their own keyboard: an override this
    /// build does not understand is ignored and the default chord stands.
    #[must_use]
    pub fn parse(text: &str) -> Option<Binding> {
        let mut binding = Binding::default();
        let mut parts = text.split('+').peekable();
        while let Some(part) = parts.next() {
            let part = part.trim().to_lowercase();
            if parts.peek().is_none() {
                if part.is_empty() {
                    return None;
                }
                binding.key = part;
                return binding.rejection().is_none().then_some(binding);
            }
            match part.as_str() {
                "ctrl" => binding.ctrl = true,
                "alt" => binding.alt = true,
                "shift" => binding.shift = true,
                _ => return None,
            }
        }
        None
    }

    /// Human rendering, e.g. `Ctrl+Shift+W`.
    ///
    /// Deliberately the same shape [`crate::keymap::Chord::rendered`] produces,
    /// so a rebound row and a default row read identically in the overlay.
    #[must_use]
    pub fn rendered(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        s.push_str(&pretty_key(&self.key));
        s
    }

    /// Why this cannot be bound, or `None` when it can.
    ///
    /// Two rules, both about not stealing keys that belong to somebody else:
    ///
    /// 1. A binding must carry Ctrl or Alt. Chords are matched globally,
    ///    so a bare letter would be swallowed before the agent ever saw it,
    ///    which reads as a broken keyboard rather than as a setting.
    /// 2. Escape, Tab and Enter are never rebindable. Escape dismisses every
    ///    layer in the program, so an operator who moves it has no way out of
    ///    the dialog they moved it in.
    #[must_use]
    pub fn rejection(&self) -> Option<&'static str> {
        if self.key.is_empty() {
            return Some("Pick a key.");
        }
        if RESERVED_KEYS.contains(&self.key.as_str()) {
            return Some(
                "Escape, Tab and Enter are reserved. Rebinding Escape would leave no way out of an \
                 open dialog.",
            );
        }
        if !self.ctrl && !self.alt {
            return Some(
                "A shortcut needs Ctrl or Alt. Without one, the shell would swallow the key before \
                 the agent saw it.",
            );
        }
        None
    }
}

/// Keys the operator may never rebind onto.
const RESERVED_KEYS: &[&str] = &["escape", "tab", "enter"];

/// Keys offered in the rebinding menu.
///
/// A fixed menu rather than a live keystroke capture, and the reason is
/// mechanical rather than aesthetic: chords are matched globally, so
/// the very keypress being captured would also fire whatever action currently
/// owns it. Capturing `Ctrl+W` would close a tab while recording it. Suppressing
/// that would mean a capture-mode flag inside key dispatch, which is a second
/// source of truth about whether chords are live. A menu has no such race and
/// is deterministic to test.
pub const BINDABLE_KEYS: &[&str] = &[
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "arrowup",
    "arrowdown",
    "arrowleft",
    "arrowright",
    "pageup",
    "pagedown",
    "home",
    "end",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "/",
    "\\",
    ",",
    ".",
    ";",
    "'",
    "[",
    "]",
    "-",
    "=",
    "`",
];

/// The operator's binding for `action`, if they set a valid one.
#[must_use]
pub fn override_for(prefs: &KeyboardPrefs, action: KeyAction) -> Option<Binding> {
    prefs
        .overrides
        .get(&action.wire())
        .and_then(|text| Binding::parse(text))
}

/// Record one rebinding.
pub fn set_override(prefs: &mut KeyboardPrefs, action: KeyAction, binding: &Binding) {
    prefs.overrides.insert(action.wire(), binding.encode());
}

/// Drop one rebinding, returning the action to its built-in chord.
pub fn clear_override(prefs: &mut KeyboardPrefs, action: KeyAction) {
    prefs.overrides.remove(&action.wire());
}

/// One binding as it will actually behave, defaults and overrides folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveChord {
    pub action: KeyAction,
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: Shift,
    pub scope: Scope,
    pub help: Option<Help>,
    /// True when the operator moved this one off its default.
    pub rebound: bool,
}

impl EffectiveChord {
    /// Human rendering, e.g. `Ctrl+Shift+W`.
    #[must_use]
    pub fn rendered(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift == Shift::On {
            s.push_str("Shift+");
        }
        s.push_str(&pretty_key(&self.key));
        s
    }

    /// This chord as a [`Binding`], for seeding the editor.
    #[must_use]
    pub fn binding(&self) -> Binding {
        Binding {
            key: self.key.clone(),
            ctrl: self.ctrl,
            alt: self.alt,
            shift: self.shift == Shift::On,
        }
    }
}

/// Actions the modal will let the operator rebind, with their descriptions.
///
/// Everything carrying a help row except the nine positional tab slots. Alt+1
/// through Alt+9 are one documented range, not nine independent chords, and
/// offering them individually would let someone move slot 3 alone and leave a
/// hole in a sequence the overlay advertises as contiguous.
#[must_use]
pub fn rebindable() -> Vec<(KeyAction, &'static str)> {
    let mut out: Vec<(KeyAction, &'static str)> = Vec::new();
    for chord in CHORDS {
        let Some(help) = chord.help else { continue };
        if matches!(chord.action, KeyAction::SelectTab(_)) {
            continue;
        }
        if !out.iter().any(|(action, _)| *action == chord.action) {
            out.push((chord.action, help.what));
        }
    }
    out
}

/// Every chord as it will actually behave.
///
/// Preserves `CHORDS` order, which is the order chords are matched in, so a
/// rebinding cannot change which of two candidate chords wins. Aliases — a
/// second chord for the same action, carrying no help row — are rebound
/// alongside their primary. Leaving an alias on its default would mean an
/// action the operator moved is still reachable at its old chord, which is
/// exactly the ghost binding this feature exists to avoid.
#[must_use]
pub fn effective_chords(prefs: &KeyboardPrefs) -> Vec<EffectiveChord> {
    CHORDS
        .iter()
        .map(|chord| match override_for(prefs, chord.action) {
            Some(binding) => EffectiveChord {
                action: chord.action,
                key: binding.key,
                ctrl: binding.ctrl,
                alt: binding.alt,
                shift: if binding.shift { Shift::On } else { Shift::Off },
                scope: chord.scope,
                help: chord.help,
                rebound: true,
            },
            None => EffectiveChord {
                action: chord.action,
                key: chord.key.to_string(),
                ctrl: chord.ctrl,
                alt: chord.alt,
                shift: chord.shift,
                scope: chord.scope,
                help: chord.help,
                rebound: false,
            },
        })
        .collect()
}

/// Every chord that is live, built-ins and saved presets together.
///
/// Key dispatch matches ONE table, so a preset's chord has to be in it or the
/// chord is not a shortcut at all: it was previously matched only by the
/// new-session dialog's own handler, which meant opening that dialog before
/// the "shortcut" could fire.
///
/// Presets come LAST, after every built-in. The first entry that matches
/// wins, so a preset can never shadow a shipped chord even if
/// the operator saves a conflicting one; the settings surface refuses the
/// conflict up front, and this ordering is what makes that refusal
/// unnecessary to trust.
///
/// A preset with no chord, or one whose chord no longer parses, contributes
/// nothing. It stays launchable from the dialog, which is what a preset is
/// for; only its accelerator is absent.
#[must_use]
pub fn live_chords(
    prefs: &KeyboardPrefs,
    presets: &[crate::launch::SavedPreset],
) -> Vec<EffectiveChord> {
    let mut out = effective_chords(prefs);
    out.extend(presets.iter().filter_map(|preset| {
        let text = preset.shortcut.as_deref()?;
        let chord = crate::launch::parse_chord(text)?;
        Some(EffectiveChord {
            action: KeyAction::LaunchPreset(preset.id),
            key: chord.key,
            ctrl: chord.ctrl,
            alt: chord.alt,
            shift: if chord.shift { Shift::On } else { Shift::Off },
            // Global on purpose. A preset launches a session, which is a thing
            // the operator wants from wherever they are, including from inside
            // a running terminal.
            scope: Scope::Global,
            help: None,
            rebound: false,
        })
    }));
    out
}

/// The action `candidate` would collide with, or `None` when it is free.
///
/// Two chords collide when one keydown could match both: same key, same Ctrl
/// and Alt, and shift requirements that overlap. [`Shift::Any`] overlaps
/// everything, which is why this cannot be an equality check on three fields.
///
/// Scope is deliberately ignored. Two chords differing only by scope are still
/// one chord as far as the operator is concerned, and reporting "that is free"
/// and then having it work in three places out of four is worse than refusing.
#[must_use]
pub fn chord_conflict(
    chords: &[EffectiveChord],
    candidate: &Binding,
    for_action: KeyAction,
) -> Option<KeyAction> {
    chords
        .iter()
        .find(|chord| {
            chord.action != for_action
                && chord.key == candidate.key
                && chord.ctrl == candidate.ctrl
                && chord.alt == candidate.alt
                && shift_overlaps(chord.shift, candidate.shift)
        })
        .map(|chord| chord.action)
}

/// The action `candidate` would collide with in the table key dispatch is
/// matching RIGHT NOW, or `None` when it is free.
///
/// One answer for both directions of the same collision, and they used to be
/// two different answers. The Keyboard tab asked [`chord_conflict`] against
/// the built-ins with the operator's rebindings applied and no saved command
/// in the list, so a chord a command already owned was reported free and then
/// fired the command. The presets editor asked [`crate::keymap::claims`],
/// which reads the shipped table with no rebindings at all, so a chord whose
/// action the operator had MOVED AWAY was refused to a command that could
/// have had it.
///
/// The two inputs come off the bus rather than out of a settings signal
/// because the bus is what key dispatch folds its table from. Answering from
/// anything else is answering about a table nobody matches, which is the
/// defect in both directions above. It also means a deleted command frees its
/// chord as soon as the deletion is published, not at the next launch.
#[must_use]
pub fn live_conflict(candidate: &Binding, for_action: KeyAction) -> Option<KeyAction> {
    let prefs = crate::state::live::keyboard_prefs();
    let saved = crate::state::live::presets();
    chord_conflict(&live_chords(&prefs, &saved), candidate, for_action)
}

/// Could these two shift requirements both match one event?
const fn shift_overlaps(existing: Shift, wants_shift: bool) -> bool {
    match existing {
        Shift::Any => true,
        Shift::On => wants_shift,
        Shift::Off => !wants_shift,
    }
}

/// What an action does, for the Keyboard tab and for a conflict message.
#[must_use]
pub fn action_label(action: KeyAction) -> String {
    CHORDS
        .iter()
        .find(|chord| chord.action == action && chord.help.is_some())
        .and_then(|chord| chord.help)
        .map_or_else(|| action.wire(), |help| help.what.to_string())
}

/// Overlay rows for the chords that are actually live.
///
/// The reason this exists rather than [`crate::keymap::help_rows`] being good
/// enough: that function renders `CHORDS` directly, so the moment rebinding
/// shipped it would have advertised the DEFAULT chord for every action the
/// operator had moved. An overlay that names a chord which does nothing is
/// strictly worse than no overlay, because the user stops trusting the one
/// place the product documents itself.
///
/// `keymap::tests::every_chord_is_documented` still guarantees that every
/// chord has a row, which is the half that has not changed. What changed is
/// what the row SAYS, and that is guaranteed here instead, by
/// [`crate::ui::shortcuts`]'s own tests.
#[must_use]
pub fn effective_help_rows(prefs: &KeyboardPrefs) -> Vec<crate::keymap::HelpRow> {
    effective_chords(prefs)
        .into_iter()
        .filter_map(|chord| {
            let help = chord.help?;
            // A literal `Help::keys` documents SEVERAL chords in one row:
            // either an alias pair ("Ctrl+Tab / Ctrl+PageDown") or the
            // positional range ("Alt+1 - Alt+9"). It is correct while the
            // defaults stand and a LIE the moment the action is rebound,
            // because rebinding moves every chord for that action onto the one
            // new binding and the alternatives it lists stop existing. So a
            // rebound row falls back to rendering its actual chord, and only an
            // untouched row keeps the literal.
            //
            // The range never takes this branch: the nine positional slots are
            // excluded from `rebindable`, precisely so one row can keep
            // documenting nine chords.
            let keys = match help.keys {
                Some(literal) if !chord.rebound => literal.to_string(),
                _ => chord.rendered(),
            };
            Some(crate::keymap::HelpRow {
                group: help.group,
                keys,
                what: help.what,
            })
        })
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Persisting
// ═══════════════════════════════════════════════════════════════════════════

/// Apply, then persist.
///
/// Called after every mutation the sheet makes, and it does two things in a
/// fixed order. First it hands the document to [`crate::state::live`], which
/// is what makes a change visible in a pane and in the window frame without a
/// relaunch: the sheet owns no pane and cannot reach one directly. Then it
/// queues the write.
///
/// The write is queued rather than done here. A slider is dragged, not set,
/// and a text field is typed into, so one visible change is tens of
/// mutations; each one used to encode and write the whole profile on the
/// thread that paints. [`crate::state::save_prefs_soon`] collapses a burst
/// into one write on a background thread, and [`flush`] writes what is left
/// when the sheet closes, so the file is never behind the screen.
pub fn commit(state: &UiState) {
    crate::state::live::publish(&state.daemon.settings);
    crate::state::save_prefs_soon(&state.daemon, &state.window);
}

/// Write anything the debounce is still holding.
///
/// Called when the sheet closes and at shutdown. A failure is reported once,
/// here, rather than on each of the mutations that led to it.
pub fn flush() {
    if let Err(why) = crate::state::flush_prefs() {
        tracing::warn!("settings not saved: {why}");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Display form of a DOM key name.
///
/// A superset of the private table in [`crate::keymap`], because that one only
/// covers the keys the built-in chords use and this one has to cover every key
/// the rebinding menu offers.
#[must_use]
pub fn pretty_key(key: &str) -> String {
    match key {
        "arrowdown" => "Down".to_string(),
        "arrowup" => "Up".to_string(),
        "arrowleft" => "Left".to_string(),
        "arrowright" => "Right".to_string(),
        "escape" => "Esc".to_string(),
        "pagedown" => "PageDown".to_string(),
        "pageup" => "PageUp".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "tab" => "Tab".to_string(),
        "enter" => "Enter".to_string(),
        other if other.chars().count() == 1 => other.to_uppercase(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// A float with no trailing zeros, for a CSS length.
fn trim_num(v: f64) -> String {
    let mut s = format!("{v:.4}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    s
}

// ═══════════════════════════════════════════════════════════════════════════
// The sheet
// ═══════════════════════════════════════════════════════════════════════════

// ---------------------------------------------------------------------------
// Reusable rows
// ---------------------------------------------------------------------------

/// When a change to `path` takes effect, in the catalogue's words.
///
/// Read from [`crate::state::catalog`] rather than written into each caption,
/// so a control cannot tell an operator it is live while the generated table
/// calls it a restart. Every row prints one of these: a setting that applies
/// immediately says so, which is what makes the sentence on a restart-only
/// row read as information rather than as an exception the reader has to
/// notice.
///
/// A row that edits no setting passes no path and prints nothing.
/// `every_control_in_the_sheet_is_catalogued` refuses a path that has no row.
#[must_use]
pub(crate) fn when_note(path: &str) -> &'static str {
    if path.is_empty() {
        return "";
    }
    crate::state::catalog::setting(path).map_or("", |s| s.live.note())
}

/// The catalogue path for one notification switch.
///
/// The three switches are rendered from [`NOTIFY_KINDS`] rather than written
/// out, so their paths are matched from the same enum instead of typed into
/// three call sites.
#[must_use]
const fn notify_path(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Finished => "notifications.finished",
        NotificationKind::NeedsApproval => "notifications.needsApproval",
        NotificationKind::Failed => "notifications.failed",
    }
}

/// The extra option a stored value that matches no choice needs, if any.
///
/// A `<select>` whose `value` matches no `<option>` does not error and does
/// not stay blank: the DOM selects the FIRST option, so the control shows a
/// setting that is not the one in effect. Every numeric preference in this
/// sheet can reach that state, because [`crate::state::AppearancePrefs::clamp`]
/// and [`crate::state::Settings::set_text_scale`] clamp to a RANGE while these
/// menus offer a handful of STEPS: a hand-edited `"textScalePct": 137` is
/// accepted whole, renders the shell at 137%, and would have read "80%".
///
/// So the value gets an option of its own rather than being silently
/// swallowed. It is labelled as unoffered, because it is a real state the
/// operator can be in and the honest thing is to name it; picking any other
/// row leaves it, and it cannot be returned to.
#[cfg(test)]
fn stray_option(value: &str, options: &[(String, String)]) -> Option<String> {
    if options.iter().any(|(v, _)| v == value) {
        return None;
    }
    Some(format!("{value} (in effect, not one of the choices)"))
}

// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Sidebar
// ---------------------------------------------------------------------------

/// Auto-settle windows offered, in milliseconds. `None` disables it.
///
/// The model's own default MUST be one of these. A `<select>` whose stored
/// value matches no option silently displays the FIRST one, so an install
/// sitting at the shipped seven-day window would have read "Never — I drain
/// the list by hand" while quietly settling rows behind the operator. The
/// screenshot caught exactly that, and
/// `the_settle_menu_can_express_the_shipped_default` keeps it caught.
#[cfg(test)]
const SETTLE_STEPS: &[(Option<u64>, &str)] = &[
    (None, "Never — I drain the list by hand"),
    (Some(15 * 60_000), "After 15 minutes idle"),
    (Some(60 * 60_000), "After 1 hour idle"),
    (Some(4 * 60 * 60_000), "After 4 hours idle"),
    (Some(24 * 60 * 60_000), "After 24 hours idle"),
    (
        Some(vitrum_model::DispositionPolicy::DEFAULT_AUTO_SETTLE_MS),
        "After 7 days idle — the default",
    ),
];

// ---------------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Advanced
// ---------------------------------------------------------------------------

/// This window's pane pacing, as label and value rows.
///
/// Empty when there is nothing to report, which is a window with no pane open
/// yet. The snapshot is taken once, because it is read under the pane's own
/// borrow and the frame clock wants that cell back.
///
/// The times are main-thread occupancy, from the decision to draw through the
/// end of submit. The pane presents and returns rather than awaiting the
/// queue fence, so this is not the time a frame took to appear, and it is
/// inflated when acquiring the swapchain texture blocks on backpressure. The
/// rows are named for what they measure so nobody reads them as the frame
/// times the performance document publishes.
#[cfg(target_os = "linux")]
pub(crate) fn frame_rows(ordinal: crate::WindowId) -> Vec<(String, String)> {
    let Some(s) = crate::pane::PaneHost::for_window(ordinal).map(|h| h.frame_summary()) else {
        return Vec::new();
    };
    let ms = |d: std::time::Duration| format!("{} ms", trim_num(d.as_secs_f64() * 1000.0));
    vec![
        ("frames drawn".to_string(), s.drawn.to_string()),
        ("ticks skipped".to_string(), s.skipped.to_string()),
        ("ticks idle".to_string(), s.idle.to_string()),
        ("frames timed".to_string(), s.recorded.to_string()),
        ("main thread, median".to_string(), ms(s.p50)),
        ("main thread, 95th".to_string(), ms(s.p95)),
        ("main thread, 99th".to_string(), ms(s.p99)),
        ("main thread, worst".to_string(), ms(s.worst)),
        (
            "owed a frame now".to_string(),
            if s.behind { "yes" } else { "no" }.to_string(),
        ),
    ]
}

/// No pane means no frame clock.
///
/// The pane is a native widget the Linux build installs, so the other targets
/// have no clock to report and the block says so rather than printing zeros
/// that read as a stalled renderer.
#[cfg(not(target_os = "linux"))]
pub(crate) fn frame_rows(_ordinal: crate::WindowId) -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(test)]
mod tests;

/// One test per control: it changes something observable, and it survives the
/// file.
///
/// Both halves matter and neither implies the other. A control that mutates
/// `Settings` but changes no derivation is a switch that does nothing; a
/// control that changes a derivation but is dropped by the serialiser is a
/// switch that does nothing after a restart, which is the same defect
/// arriving late. So every test here asserts a DERIVED value before and after,
/// then pushes the whole document through the exact encode/parse pair
/// `save_prefs` and `load_prefs` use and asserts the derived value again.
///
/// Controls whose observable effect lives in another module are marked and
/// tested for the round trip only; the rendering half is asserted where the
/// markup is. Those are named in the module docs as well, so the gap is
/// visible without reading the tests.
#[cfg(test)]
mod round_trip;

/// Captions in this sheet against what the product actually does.
///
/// Its own module because it is a coherence suite, not a settings-logic one:
/// every test here reads the shipped source of the files that implement the
/// behaviour and asserts that a sentence shown to an operator is true of the
/// code beside it. Source scanning rather than a runtime assertion because
/// neither behaviour has a hook a unit test can reach: one is a wheel event in
/// the terminal pane, the other is a D-Bus click on a live desktop.
#[cfg(test)]
mod sheet_copy_is_true;

/// A saved preset's chord is a SHORTCUT, not a dialog accelerator.
///
/// The distinction is the whole feature. `SavedPreset::shortcut` existed, the
/// new-session dialog matched it, and the settings panel refused conflicts,
/// which made it look finished. But the only matcher was the dialog's own
/// keydown handler, so firing a preset meant opening the dialog first: two
/// keystrokes to reach the thing whose entire purpose was to be one. A
/// shortcut must be able to open a session in a named folder with a named
/// command, and the shipped behaviour did not do that.
///
/// One chord table is matched. These tests are about what is in it.
#[cfg(test)]
mod preset_shortcuts;

/// Translucency and the backdrop image: what a default profile costs, what the
/// controls offer, and what survives a hand-edited `ui.json`.
#[cfg(test)]
mod appearance;
