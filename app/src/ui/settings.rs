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
//!   overrides on that same element. Text scale is the root font size, set
//!   through [`ui_scale_script`]: every geometry and type token in both
//!   stylesheets is declared in `rem`, so one property scales the whole shell
//!   without this module knowing any token's value.
//! - **Sidebar** is `show_branch` / `show_time` / `show_status_word`, read by
//!   `ui/sidebar.rs`, plus the auto-settle window, which is
//!   [`vitrum_model::DispositionPolicy`] and therefore governs every
//!   disposition, section, rollup and traversal decision in the product.
//! - **Workspaces** drives [`crate::state::WorkspaceSet`] directly: create,
//!   rename, delete, reorder, grouping mode, band visibility, folders.
//! - **Terminal** is [`term_options_script`], which drives the live xterm
//!   instance through `window.__vitrum_applyTerm` in `bootstrap.js`. Font,
//!   size, scrollback and the WebGL renderer are all reconfigurable without a
//!   restart, so none of them is a "takes effect next launch" setting.
//! - **Notifications** is [`should_notify`] plus [`notable_transitions`], which
//!   is edge-triggered. Level-triggered notification is the classic defect
//!   here: at twenty agents a predicate re-evaluated on every snapshot
//!   re-notifies about the same blocked session several times a second.
//! - **Keyboard** is [`effective_chords`], folded into the same JSON shape
//!   [`crate::keymap::keymap_json`] produces and pushed through
//!   `window.__vitrum_applyKeymap`. [`crate::ui::shortcuts`] renders from the
//!   same fold, so a rebound chord can never be advertised as its default.
//! - **Advanced** is the daemon URL, which reconnects the socket on the spot,
//!   and a live [`vitrum_os::probe`] report.
//!
//! # What deliberately has no control
//!
//! - **Terminal colour theme.** xterm's palette is read once at mount from the
//!   `--rg-terminal-*` tokens. Re-theming a live terminal is possible, but
//!   there is only one palette to offer, so there is nothing to pick between.
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

use std::sync::{Arc, LazyLock, RwLock};

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use vitrum_model::{SessionView, SidebarStatus};
use vitrum_os::capability::{Support, Unavailable};
use vitrum_os::notify::{Notification, NotificationKind, Notifier};
use vitrum_proto::{SessionId, SessionStatus};

use crate::keymap::{CHORDS, Help, KeyAction, Scope, Shift};
use crate::state::{
    Density, FolderId, Grouping, KeyboardPrefs, Settings, SettingsTab, TermRenderer, TerminalPrefs,
    ThemePref, UiState, WorkspaceId,
};

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

/// The browser default root font size, in CSS pixels.
///
/// Named rather than inlined because it is the one number the scale percentage
/// multiplies, and a bare `16.0` in the middle of a format string says nothing
/// about why it is 16.
pub const ROOT_FONT_PX: f64 = 16.0;

/// Token overrides that make the compact density compact.
///
/// Absolute values, not multiples of whatever `sidebar.css` declares today.
/// Restating the stylesheet's numbers here in order to scale them would put the
/// same constant in two files maintained by two different people, and the copy
/// in this file would go stale silently. So comfortable emits nothing at all
/// and inherits the stylesheet verbatim; compact is this deliberate table.
///
/// Only vertical rhythm is touched. Type steps are left alone because shrinking
/// text is what the scale control is for, and a density switch that also
/// changed the font size would make the two controls fight.
///
/// These are inline custom properties rather than a `.rg-density--compact`
/// block because no stylesheet has such a block, and a switch that waits on
/// somebody else's CSS is a dead switch today. [`Density::class`] is still
/// emitted alongside, for structural rules a custom property cannot express.
/// Every value here is a 4px multiple, and that is not decoration.
///
/// Compact used to leave the grid: `--rg-line-head` was 1.125rem (18px) and
/// `--rg-space-2` and `--rg-content-inset` were 0.375rem (6px). An 18px head
/// line makes the card 12 + 18 + 4 + 20 + 12 = 66, so the whole list ran on a
/// 66px pitch that lands on no grid line anywhere else in the product.
/// Measured on the running binary before the fix: pitch 66, and Comfortable's
/// 76 for comparison.
///
/// `--rg-row-gap` was 0, which made adjacent cards TOUCH: one continuous
/// surface with a corner notch at each end, verified at native resolution.
/// Proximity is the only signal saying where one row ends, so a dense list
/// still needs a gap; it just needs a smaller one. 4px keeps the signal at
/// half of Comfortable's 8.
const COMPACT_TOKENS: &[(&str, &str)] = &[
    ("--rg-card-h", "3.75rem"),
    ("--rg-slim-h", "1.75rem"),
    ("--rg-row-collapsed-h", "1.5rem"),
    ("--rg-row-gap", "0.25rem"),
    ("--rg-line-head", "1rem"),
    ("--rg-space-2", "0.25rem"),
    ("--rg-space-2-5", "0.5rem"),
    ("--rg-space-3", "0.5rem"),
    ("--rg-space-4", "0.75rem"),
    ("--rg-content-inset", "0.25rem"),
    ("--rg-row-inset", "0.5rem"),
];

/// Inline `style` for the application root.
///
/// Custom properties only, so nothing here can trigger layout on its own: the
/// engine recomputes the elements that reference a changed token and no others.
/// Returning a `String` rather than writing to the DOM keeps the whole
/// appearance layer a pure function a test can assert exactly.
///
/// `reduce_motion` zeroes the two duration tokens rather than removing
/// transitions, which is the lever the stylesheet's `prefers-reduced-motion`
/// block already pulls. Every transition in both stylesheets reads its duration
/// from one of those two names, so zeroing them is equivalent to the OS
/// preference and cannot miss a rule added later.
#[must_use]
pub fn root_style(settings: &Settings) -> String {
    let mut style = String::new();
    if settings.density == Density::Compact {
        for (name, value) in COMPACT_TOKENS {
            style.push_str(name);
            style.push(':');
            style.push_str(value);
            style.push(';');
        }
    }
    if settings.reduce_motion {
        style.push_str("--rg-t-fast:0s;--rg-t-base:0s;");
    }
    // Last, so a chosen palette beats anything above it by source order at
    // equal specificity. Nothing above declares a terminal token today; this
    // is so the invariant survives the next block added here.
    style.push_str(&crate::termpalette::css_tokens(settings.terminal.palette));
    style
}

/// The `data-theme` value for the application root.
///
/// [`ThemePref::System`] resolves through [`system_theme`]. Resolving here
/// rather than storing a resolved value means the preference on disk stays
/// "follow the system", which is what the operator actually asked for, instead
/// of freezing whatever the system said the day they set it.
#[must_use]
pub fn theme_attr(settings: &Settings) -> &'static str {
    match settings.theme {
        ThemePref::Light => "light",
        ThemePref::Dark => "dark",
        ThemePref::System => match system_theme() {
            Some(vitrum_os::theme::Theme::Light) => "light",
            // Dark is also the answer when the desktop cannot be asked. The
            // stylesheet's base palette is dark, so this is the branch that
            // changes nothing rather than a guess dressed up as a reading.
            _ => "dark",
        },
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
    *SYSTEM_THEME
        .read()
        .expect("system theme lock is never poisoned")
}

/// Ask the desktop again and update the cache.
#[must_use]
pub fn refresh_system_theme() -> Option<vitrum_os::theme::Theme> {
    let read = read_system_theme();
    *SYSTEM_THEME
        .write()
        .expect("system theme lock is never poisoned") = read;
    read
}

static SYSTEM_THEME: LazyLock<RwLock<Option<vitrum_os::theme::Theme>>> =
    LazyLock::new(|| RwLock::new(read_system_theme()));

fn read_system_theme() -> Option<vitrum_os::theme::Theme> {
    vitrum_os::theme::theme_watcher()
        .ok()
        .and_then(|w| w.current().ok())
}

/// Script that applies the text scale to the document root.
///
/// The root font size is the only lever that scales the shell uniformly: every
/// spacing, geometry and type token in `sidebar.css` and `app.css` is declared
/// in `rem`, so this one property moves all of them and this module never has
/// to know what any of them are worth. Hairlines keep their literal `1px`,
/// which is correct — a scaled border is a blurry border.
///
/// Composes multiplicatively with the HiDPI webview zoom applied underneath by
/// the window layer. This is a user preference on top of that, never a
/// substitute for it, so no device scale appears anywhere in this function.
#[must_use]
pub fn ui_scale_script(percent: u16) -> String {
    let px = ROOT_FONT_PX * f64::from(clamp_scale(percent)) / 100.0;
    format!(
        "document.documentElement.style.fontSize=\"{}px\";",
        trim_num(px)
    )
}

/// Force a stored scale into the range the modal can express.
///
/// A hand-edited settings file with `"textScalePct": 4000` must not produce a
/// window whose first row is taller than the screen, which would put the
/// settings gear permanently out of reach.
#[must_use]
pub fn clamp_scale(percent: u16) -> u16 {
    percent.clamp(
        crate::state::TEXT_SCALE_MIN_PCT,
        crate::state::TEXT_SCALE_MAX_PCT,
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Terminal
// ═══════════════════════════════════════════════════════════════════════════

/// What each renderer costs, in the operator's own terms.
///
/// Shown next to the control rather than buried in a doc comment. WebGL is
/// measurably worse here on both axes that matter, and a toggle that silently
/// makes the application heavier is only marginally better than one that does
/// nothing.
#[must_use]
pub const fn renderer_note(renderer: TermRenderer) -> &'static str {
    match renderer {
        TermRenderer::Dom => {
            "Cheapest at rest, and the default. Measured here: 0% idle CPU and 73 MB/s of \
             throughput, which is more than twenty agents produce between them."
        }
        TermRenderer::Webgl => {
            "Costs a steady 0.244% idle CPU and about 80 MB more memory under WebKitGTK, because \
             the compositor keeps the GL layer awake even when nothing on screen changes. \
             Throughput is 71 MB/s, slightly BELOW the DOM renderer."
        }
    }
}

#[must_use]
pub const fn renderer_label(renderer: TermRenderer) -> &'static str {
    match renderer {
        TermRenderer::Dom => "DOM",
        TermRenderer::Webgl => "WebGL",
    }
}

/// The string `bootstrap.js` matches on.
#[must_use]
pub const fn renderer_wire(renderer: TermRenderer) -> &'static str {
    match renderer {
        TermRenderer::Dom => "dom",
        TermRenderer::Webgl => "webgl",
    }
}

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

/// Smallest terminal font size the modal will set.
///
/// Owned here rather than by the settings struct because the number is a fact
/// about xterm, not about the preference: below this the cell box rounds to
/// zero width, the fit addon divides by it, and the pane goes blank with
/// nothing logged anywhere.
pub const TERM_FONT_MIN_PX: u16 = 8;
/// Largest terminal font size the modal will set.
pub const TERM_FONT_MAX_PX: u16 = 32;

/// Terminal font sizes the modal offers.
pub const TERM_FONT_STEPS: &[u16] = &[9, 10, 11, 12, 13, 14, 16, 18, 20, 24];

/// Script that reconfigures the live terminal.
///
/// Writes the options to `window.__vitrum_termOptions` **and** calls the
/// applier if it exists. Both halves are load-bearing and the order they run in
/// is not knowable: this can be evaluated before `bootstrap.js` has mounted the
/// terminal, in which case `mount()` reads the stashed options when it
/// constructs the `Terminal`, or after, in which case the applier reconfigures
/// it in place. Without the stash, a restart mounts at the default font and
/// visibly reflows to the saved one a moment later.
#[must_use]
pub fn term_options_script(prefs: &TerminalPrefs) -> String {
    let size = prefs.font_size_px.clamp(TERM_FONT_MIN_PX, TERM_FONT_MAX_PX);
    let family = if prefs.font_family.trim().is_empty() {
        "null".to_string()
    } else {
        json_string(&prefs.font_family)
    };
    format!(
        "window.__vitrum_termOptions={{renderer:{renderer},scrollback:{scrollback},\
         fontSize:{size},fontFamily:{family},theme:{theme}}};\
         if(window.__vitrum_applyTerm)window.__vitrum_applyTerm(window.__vitrum_termOptions);",
        renderer = json_string(renderer_wire(prefs.renderer)),
        scrollback = prefs.scrollback_lines,
        theme = crate::termpalette::js_theme(prefs.palette),
    )
}

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
#[must_use]
pub fn notable_transitions(before: &[SessionView], after: &[SessionView]) -> Vec<Notable> {
    let mut out = Vec::new();
    for row in after {
        let Some(was) = before.iter().find(|old| old.id() == row.id()) else {
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
    /// 1. A binding must carry Ctrl or Alt. The bridge matches chords globally,
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
/// mechanical rather than aesthetic: `bootstrap.js` matches chords globally, so
/// the very keypress being captured would also fire whatever action currently
/// owns it. Capturing `Ctrl+W` would close a tab while recording it. Suppressing
/// that would mean a capture-mode flag inside the bridge, which is a second
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
/// Preserves `CHORDS` order, which is the order the bridge matches in, so a
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
/// The bridge matches ONE table, so a preset's chord has to be in it or the
/// chord is not a shortcut at all: it was previously matched only by the
/// new-session dialog's own handler, which meant opening that dialog before
/// the "shortcut" could fire.
///
/// Presets come LAST, after every built-in. `bootstrap.js` takes the first
/// entry that matches, so a preset can never shadow a shipped chord even if
/// the operator saves a conflicting one; the settings surface refuses the
/// conflict up front, and this ordering is what makes that refusal
/// unnecessary to trust.
///
/// A preset with no chord, or one whose chord no longer parses, contributes
/// nothing. It stays launchable from the dialog, which is what a preset is
/// for; only its accelerator is absent.
#[must_use]
pub fn live_chords(prefs: &KeyboardPrefs, presets: &[crate::launch::SavedPreset]) -> Vec<EffectiveChord> {
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

/// The wire form of a shift requirement, as `bootstrap.js` reads it.
///
/// Duplicated from `keymap.rs`, whose own mapping is private and which this
/// module may not edit. The duplication is not left to trust:
/// `no_overrides_reproduces_the_builtin_table_exactly` asserts that the table
/// this module emits with no overrides is byte-identical to
/// [`crate::keymap::keymap_json`], so a divergence in either mapping fails the
/// build rather than silently changing which keys fire.
const fn shift_wire(shift: Shift) -> &'static str {
    match shift {
        Shift::Off => "off",
        Shift::On => "on",
        Shift::Any => "any",
    }
}

/// The wire form of a chord's scope. Duplicated for the reason above, and
/// guarded by the same test.
const fn scope_wire(scope: Scope) -> &'static str {
    match scope {
        Scope::Global => "global",
        Scope::NotTerminal => "notTerminal",
        Scope::NotTextInput => "notTextInput",
        Scope::LayerOnly => "layerOnly",
        Scope::SessionList => "sessionList",
    }
}

/// The chord table in the JSON shape `bootstrap.js` matches against.
///
/// Identical in shape to [`crate::keymap::keymap_json`], because it is the same
/// table with overrides folded in and the bridge must not be able to tell the
/// difference.
#[must_use]
pub fn keymap_json(chords: &[EffectiveChord]) -> String {
    let entries: Vec<serde_json::Value> = chords
        .iter()
        .map(|chord| {
            serde_json::json!({
                "key": chord.key,
                "ctrl": chord.ctrl,
                "alt": chord.alt,
                "shift": shift_wire(chord.shift),
                "scope": scope_wire(chord.scope),
                "action": chord.action.wire(),
            })
        })
        .collect();
    serde_json::to_string(&entries).expect("chord table is plain data")
}

/// Script that replaces the bridge's live chord table.
///
/// Stashes the table on `window.__vitrum_keymap` as well as calling the applier,
/// for the same ordering reason [`term_options_script`] does: this can run
/// before `bootstrap.js` has read that global, in which case the stash is what
/// it reads.
#[must_use]
pub fn keymap_script(prefs: &KeyboardPrefs) -> String {
    // Custom bindings first: bootstrap.js takes the first match, which is the
    // same precedence `dispatch_key` enforces on the Rust side. Without this
    // a chord no built-in owns never reaches Rust at all, because the webview
    // has no reason to intercept it.
    let table = crate::keymap::with_custom_first(
        &keymap_json(&live_chords(prefs, &crate::launch::load_launch_store().presets)),
        &prefs.custom,
    );
    format!(
        "window.__vitrum_keymap={table};\
         if(window.__vitrum_applyKeymap)window.__vitrum_applyKeymap(window.__vitrum_keymap);"
    )
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
// Saved commands
// ═══════════════════════════════════════════════════════════════════════════

/// Longest label the editor will store, in characters.
///
/// Counted in `char`s and not bytes, so a label in a non-Latin script gets the
/// same allowance as one in English. The number is what fits the picker's row
/// in the new-session dialog without eliding; a label that only ever appears
/// as `Claude in vitrum, resu…` is a label that has failed at the one job it
/// has.
pub const PRESET_LABEL_MAX: usize = 40;

/// Which field of a saved command an edit is aimed at.
///
/// The editor commits one field at a time rather than assembling a whole
/// candidate and writing it back. Four independent commits means a typo in the
/// shortcut cannot silently discard the working directory typed a second
/// earlier, which is exactly what a whole-record write does when validation
/// refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetField {
    Label,
    /// The program and its arguments as one line, split by
    /// [`crate::launch::split_command`].
    CommandLine,
    /// Default working directory. Empty clears it.
    Cwd,
    /// Chord that starts this command from the new-session dialog. Empty
    /// clears it.
    Shortcut,
    /// Slug of the icon this command draws. Empty clears it back to the one
    /// derived from the command text.
    Icon,
}

/// Why an edit to the saved commands was refused.
///
/// A variant per reason rather than a `String`, because two of them are
/// asserted in tests against exact content and because the panel renders the
/// sentence in one place. Every message names the value that was refused: a
/// form that answers "invalid input" over four fields has said nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetRefusal {
    /// The label was empty or only whitespace.
    NoLabel,
    /// The label was longer than [`PRESET_LABEL_MAX`].
    LabelTooLong(usize),
    /// Another saved command already answers to that label.
    DuplicateLabel(String),
    /// There is no program in the command line: it was empty, whitespace, or
    /// quoting that yields a blank first word.
    NoCommand,
    /// The shortcut is not a chord this build can match.
    BadShortcut(String),
    /// The shortcut is already a shell chord, so the dialog would never see
    /// the keydown. Carries the sentence [`crate::launch::chord_conflict`]
    /// produced, which names the action that owns it.
    ShortcutTaken(String),
    /// Another saved command already answers to that chord. Carries the
    /// canonical chord and the other row's label.
    ShortcutInUse(String, String),
    /// The row was deleted by another window between the render and the edit.
    Vanished,
}

impl std::fmt::Display for PresetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetRefusal::NoLabel => f.write_str(
                "A saved command needs a label. It is the only part of it the picker shows.",
            ),
            PresetRefusal::LabelTooLong(len) => write!(
                f,
                "That label is {len} characters. The picker shows {PRESET_LABEL_MAX}."
            ),
            PresetRefusal::DuplicateLabel(label) => write!(
                f,
                "\u{201c}{label}\u{201d} is already the label of another saved command. Two rows \
                 with one name means the picker offers the same word twice and neither says \
                 which is which."
            ),
            PresetRefusal::NoCommand => f.write_str(
                "A saved command needs a program to run. Anything on PATH works, and so does an \
                 absolute path.",
            ),
            PresetRefusal::BadShortcut(text) => write!(
                f,
                "\u{201c}{text}\u{201d} is not a chord this build can match. Write it as \
                 Ctrl+Shift+K: the modifiers are Ctrl, Alt and Shift, in any case, joined by +."
            ),
            PresetRefusal::ShortcutTaken(why) => write!(
                f,
                "{why} A saved command cannot take a chord the shell already claims: the \
                 keydown is handled before the dialog sees it, so the shortcut would be one \
                 this tab shows and the product never fires."
            ),
            PresetRefusal::ShortcutInUse(chord, label) => write!(
                f,
                "{chord} already starts \u{201c}{label}\u{201d}. Two saved commands on one chord \
                 means the first in the list wins and the other is dead, with nothing on screen \
                 saying so."
            ),
            PresetRefusal::Vanished => f.write_str(
                "That saved command was deleted in another window, so there was nothing to \
                 change. The list above is what is on disk now.",
            ),
        }
    }
}

/// Is `label` free, ignoring the row that already owns it?
///
/// Case-insensitive, because the picker is read by a person and `Claude` and
/// `claude` are one name to them. ASCII case folding rather than a full
/// Unicode fold: the comparison has to be identical to the one a test can
/// state, and a locale-dependent fold is not.
fn label_is_free(list: &[crate::launch::SavedPreset], label: &str, except: u64) -> bool {
    !list
        .iter()
        .any(|p| p.id != except && p.label.eq_ignore_ascii_case(label))
}

/// Check and normalise a label, or say why not.
fn accept_label(
    list: &[crate::launch::SavedPreset],
    label: &str,
    except: u64,
) -> Result<String, PresetRefusal> {
    let label = label.trim();
    if label.is_empty() {
        return Err(PresetRefusal::NoLabel);
    }
    let len = label.chars().count();
    if len > PRESET_LABEL_MAX {
        return Err(PresetRefusal::LabelTooLong(len));
    }
    if !label_is_free(list, label, except) {
        return Err(PresetRefusal::DuplicateLabel(label.to_string()));
    }
    Ok(label.to_string())
}

/// Check and split a command line, or say why not.
///
/// [`crate::launch::split_command`] has no failure mode of its own beyond
/// "there was no word here": an unclosed quote takes the rest of the line as
/// one argument rather than erroring, which is the behaviour the dialog's own
/// field has and the two must agree. So the only refusal is an absent
/// program, and it is checked on the SPLIT result and not on the raw line,
/// because `"   "` is a non-empty line whose first word is blank.
fn accept_command(line: &str) -> Result<(String, Vec<String>), PresetRefusal> {
    let Some((command, args)) = crate::launch::split_command(line.trim()) else {
        return Err(PresetRefusal::NoCommand);
    };
    if command.trim().is_empty() {
        return Err(PresetRefusal::NoCommand);
    }
    Ok((command, args))
}

/// Check and canonicalise a shortcut, or say why not. Empty means none.
///
/// Stored in the canonical form [`crate::launch::format_chord`] produces
/// rather than as typed, so the file never holds two spellings of one chord
/// and the matcher never has to fold anything at match time. Canonicalising
/// first is also what makes the duplicate check below exact: `alt+j` and
/// `Alt+J` are one binding and comparing the typed strings would miss it.
fn accept_shortcut(
    list: &[crate::launch::SavedPreset],
    text: &str,
    except: u64,
) -> Result<Option<String>, PresetRefusal> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let Some(chord) = crate::launch::parse_chord(text) else {
        return Err(PresetRefusal::BadShortcut(text.to_string()));
    };
    // A chord the shell already claims never reaches the dialog's keydown, so
    // storing it would produce exactly the thing this tab refuses to ship: a
    // shortcut the settings panel displays and the product never fires.
    if let Some(why) = crate::launch::chord_conflict(&chord) {
        return Err(PresetRefusal::ShortcutTaken(why));
    }
    let canonical = crate::launch::format_chord(&chord);
    // And the same argument one level down: the dialog takes the first preset
    // in list order that matches, so a second preset on one chord is a row
    // that can never be reached by keyboard.
    if let Some(other) = list
        .iter()
        .find(|p| p.id != except && p.shortcut.as_deref() == Some(canonical.as_str()))
    {
        return Err(PresetRefusal::ShortcutInUse(canonical, other.label.clone()));
    }
    Ok(Some(canonical))
}

/// Apply one field edit to the saved command with this id.
///
/// Keyed by id and not by position, and that is not fussiness. The list on
/// disk is shared by every window: a second window that deleted a row leaves
/// this window rendering positions that no longer mean what they meant, and an
/// index-keyed edit would then rewrite the wrong row's label. An id that is
/// gone is [`PresetRefusal::Vanished`], which the panel shows and then
/// refreshes from disk.
///
/// Nothing is written unless the value is accepted, so a refused edit leaves
/// the list byte-identical.
pub fn revise(
    list: &mut [crate::launch::SavedPreset],
    id: u64,
    field: PresetField,
    value: &str,
) -> Result<(), PresetRefusal> {
    let Some(index) = list.iter().position(|p| p.id == id) else {
        return Err(PresetRefusal::Vanished);
    };
    match field {
        PresetField::Label => {
            let label = accept_label(list, value, id)?;
            list[index].label = label;
        }
        PresetField::CommandLine => {
            let (command, args) = accept_command(value)?;
            list[index].command = command;
            list[index].args = args;
        }
        PresetField::Cwd => {
            let cwd = value.trim();
            // Empty clears it rather than storing `Some("")`. An empty string
            // is a directory the picker would try to enter and the daemon
            // would refuse, which is a launch failure standing in for "no
            // opinion".
            list[index].cwd = if cwd.is_empty() {
                None
            } else {
                Some(cwd.to_string())
            };
        }
        PresetField::Shortcut => {
            let shortcut = accept_shortcut(list, value, id)?;
            list[index].shortcut = shortcut;
        }
        PresetField::Icon => {
            // An unknown slug clears rather than refuses. The picker can only
            // emit slugs it owns, so the only way to reach this is a
            // hand-edited profile or one written by a build with an icon this
            // one dropped, and losing the choice beats refusing the save.
            let slug = value.trim();
            list[index].icon = crate::ui::icons::from_slug(slug).map(|i| i.slug.to_string());
        }
    }
    Ok(())
}

/// Append a saved command, returning the id it was given.
///
/// The id comes from [`crate::launch::mint_preset_id`] and is then bumped
/// until it is free. Minting from the label and the command alone is stable,
/// which is what makes it a good id, but stable also means a label that was
/// used, renamed and used again mints the number a live row already holds.
/// Two rows with one id is the picker launching the wrong command, so the
/// collision is resolved here, at the only place that ever creates one.
pub fn create(
    list: &mut Vec<crate::launch::SavedPreset>,
    label: &str,
    command_line: &str,
) -> Result<u64, PresetRefusal> {
    let label = accept_label(list, label, u64::MAX)?;
    let (command, args) = accept_command(command_line)?;
    let mut id = crate::launch::mint_preset_id(&label, &command);
    while list.iter().any(|p| p.id == id) {
        id = id.wrapping_add(1);
    }
    list.push(crate::launch::SavedPreset {
        id,
        label,
        command,
        args,
        cwd: None,
        shortcut: None,
        icon: None,
    });
    Ok(id)
}

/// Drop the saved command with this id. False when it was already gone.
pub fn remove(list: &mut Vec<crate::launch::SavedPreset>, id: u64) -> bool {
    let before = list.len();
    list.retain(|p| p.id != id);
    list.len() != before
}

/// Move a saved command `delta` places, clamped to the ends of the list.
///
/// Returns false when the move would fall off either end, which is what
/// disables the arrow rather than leaving a button that visibly does nothing
/// at the top and bottom of the list.
pub fn move_by(list: &mut [crate::launch::SavedPreset], id: u64, delta: isize) -> bool {
    let Some(from) = list.iter().position(|p| p.id == id) else {
        return false;
    };
    let Some(to) = from.checked_add_signed(delta) else {
        return false;
    };
    if to >= list.len() || delta == 0 {
        return false;
    }
    // A rotation and not a swap: moving a row three places past two others
    // must not reverse the pair it stepped over. With `delta` of one the two
    // are the same operation, and the panel only offers one, but the function
    // is the one place the ordering is decided and it should be right for the
    // argument it takes.
    if to > from {
        list[from..=to].rotate_left(1);
    } else {
        list[to..=from].rotate_right(1);
    }
    true
}

// ═══════════════════════════════════════════════════════════════════════════
// Applying and persisting
// ═══════════════════════════════════════════════════════════════════════════

/// Everything that has to be pushed into the webview, as one script.
///
/// One string and therefore one `eval`, because three separate evals are three
/// IPC round trips and three points at which a failure leaves the settings half
/// applied.
#[must_use]
pub fn live_script(settings: &Settings) -> String {
    let mut script = ui_scale_script(settings.text_scale_pct);
    script.push_str(&term_options_script(&settings.terminal));
    script.push_str(&keymap_script(&settings.keyboard));
    script
}

/// Push every live-reconfigurable setting into the webview.
///
/// Theme, density and reduced motion are absent on purpose: those are
/// attributes on the app root, so Dioxus reapplies them as part of the same
/// re-render the settings change already causes, with no bridge involved.
///
/// # Scope: this reaches ONE window
///
/// `document::eval` runs in the calling scope's document, and dioxus-desktop
/// gives every window its own. `Settings` however lives on `DaemonState`,
/// which is shared by every window in the process. So a Terminal or Keyboard
/// change made in window 1 updates the shared model — and therefore every
/// window's markup, immediately, because they all render from it — but the
/// xterm options and the chord table are only pushed into window 1's webview.
/// Windows 2..N keep their old terminal font, scrollback, renderer and
/// keybindings until they are next constructed.
///
/// This is a known gap, not a design. It is stated here rather than hidden
/// because the whole premise of this module is that a control which does not
/// take effect does not ship, and "takes effect in the window you were looking
/// at" is a weaker promise than the one the modal makes. Closing it needs the
/// shell to fan the script out across live windows, which is the window
/// layer's to own: this module has no handle on any document but its own.
pub fn apply_live(settings: &Settings) {
    let _ = document::eval(&live_script(settings));
}

/// Persist to disk, then push into the webview.
///
/// Called after every mutation the modal makes. Settings changes happen at
/// human speed, a handful per session, so writing the file on each one is
/// cheaper than the bookkeeping a debounce would need, and it means the file on
/// disk is never behind what is on screen.
pub fn commit(state: &UiState) {
    if let Err(why) = crate::state::save_prefs(&state.daemon, &state.window) {
        tracing::warn!("settings not saved: {why}");
    }
    apply_live(&state.daemon.settings);
}

/// Mutate settings, then persist and apply, in one place.
///
/// Every control in this file goes through here. Routing them through one
/// function is what makes "takes effect immediately and survives a restart"
/// true by construction rather than by each handler remembering to do both.
fn edit(mut state: Signal<UiState>, change: impl FnOnce(&mut Settings)) {
    change(&mut state.write().daemon.settings);
    commit(&state.peek());
}

/// Mutate anything else on the state, then persist and apply.
fn edit_state(mut state: Signal<UiState>, change: impl FnOnce(&mut UiState)) {
    change(&mut state.write());
    commit(&state.peek());
}

/// Run a mutation that can be refused, surfacing the refusal instead of
/// swallowing it.
///
/// Every workspace and folder operation goes through here. `WorkspaceError`
/// exists so "you cannot delete the workspace holding four sessions" reaches
/// the operator as a sentence; a handler that dropped the `Result` would leave
/// a Delete button that visibly does nothing, which is the worst of the three
/// possible behaviours.
///
/// The success arm clears the message as well as committing, so a refusal
/// followed by a valid action does not leave a stale complaint on screen.
fn try_edit<T, E: core::fmt::Display>(
    mut state: Signal<UiState>,
    mut error: Signal<String>,
    change: impl FnOnce(&mut UiState) -> Result<T, E>,
) {
    let outcome = change(&mut state.write());
    match outcome {
        Ok(_) => {
            error.set(String::new());
            commit(&state.peek());
        }
        Err(why) => error.set(why.to_string()),
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

/// A JSON string literal, safe to paste into a script.
///
/// Goes through `serde_json` rather than hand-rolled quoting because a font
/// stack contains double quotes (`"JetBrains Mono", monospace`), and getting
/// that wrong turns a settings change into a syntax error in the webview, which
/// surfaces as "the terminal stopped responding" rather than as anything
/// resembling its cause.
fn json_string(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
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

#[derive(Props, Clone, PartialEq)]
pub struct SettingsSheetProps {
    pub state: Signal<UiState>,
    /// Which page is showing. Held in [`crate::state::Layer`] rather than in a
    /// local signal so reopening the modal returns to the tab you left.
    pub tab: SettingsTab,
    pub on_tab: EventHandler<SettingsTab>,
    /// Dial a different daemon. Owned by the shell, which holds the bridge.
    pub on_reconnect: EventHandler<String>,
    pub on_dismiss: EventHandler<()>,
}

#[component]
pub fn SettingsSheet(props: SettingsSheetProps) -> Element {
    let tab = props.tab;
    rsx! {
        div {
            class: "rg-layer rg-layer--dim",
            onclick: move |_| props.on_dismiss.call(()),
            div {
                class: "rg-sheet rg-sheet--settings",
                role: "dialog",
                aria_label: "Settings",
                onclick: move |e| e.stop_propagation(),

                div { class: "rg-sheet__head",
                    span { class: "rg-sheet__title", "Settings" }
                    button {
                        class: "rg-btn-inline",
                        r#type: "button",
                        onclick: move |_| props.on_dismiss.call(()),
                        "Close"
                    }
                }

                div { class: "rg-sheet__body",
                    div { class: "rg-sheet__tabs", role: "tablist",
                        for entry in SettingsTab::ALL {
                            button {
                                class: if entry == tab { "rg-sheet__tab rg-sheet__tab--active" } else { "rg-sheet__tab" },
                                key: "{entry.label()}",
                                r#type: "button",
                                role: "tab",
                                aria_selected: if entry == tab { "true" } else { "false" },
                                onclick: move |_| props.on_tab.call(entry),
                                "{entry.label()}"
                            }
                        }
                    }

                    div { class: "rg-sheet__panel", role: "tabpanel",
                        match tab {
                            SettingsTab::Appearance => rsx! { AppearancePanel { state: props.state } },
                            SettingsTab::Sidebar => rsx! { SidebarPanel { state: props.state } },
                            SettingsTab::Workspaces => rsx! { WorkspacesPanel { state: props.state } },
                            SettingsTab::Presets => rsx! { PresetsPanel { state: props.state } },
                            SettingsTab::Terminal => rsx! { TerminalPanel { state: props.state } },
                            SettingsTab::Notifications => rsx! { NotificationsPanel { state: props.state } },
                            SettingsTab::Keyboard => rsx! {
                                crate::ui::keybinds::Keybinds { state: props.state }
                            },
                            SettingsTab::Advanced => rsx! {
                                AdvancedPanel {
                                    state: props.state,
                                    on_reconnect: props.on_reconnect,
                                }
                            },
                            SettingsTab::About => rsx! { AboutPanel { state: props.state } },
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Reusable rows
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
struct SwitchProps {
    label: String,
    desc: String,
    on: bool,
    onchange: EventHandler<bool>,
}

/// A labelled on/off row.
#[component]
fn SwitchRow(props: SwitchProps) -> Element {
    let on = props.on;
    rsx! {
        div { class: "rg-field rg-field--switch",
            span { class: "rg-field__label", "{props.label}" }
            span { class: "rg-field__control",
                button {
                    class: if on { "rg-switch rg-switch--on" } else { "rg-switch" },
                    r#type: "button",
                    role: "switch",
                    aria_checked: if on { "true" } else { "false" },
                    aria_label: "{props.label}",
                    onclick: move |_| props.onchange.call(!on),
                    span { class: "rg-switch__knob" }
                }
            }
            if !props.desc.is_empty() {
                span { class: "rg-field__desc", "{props.desc}" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct SelectProps {
    label: String,
    desc: String,
    /// Currently selected value.
    value: String,
    /// `(value, label)` pairs in menu order.
    options: Vec<(String, String)>,
    onpick: EventHandler<String>,
}

/// A labelled menu row.
///
/// A native `<select>` rather than a custom popup: it is one element, it is
/// keyboard accessible without any code here, and it cannot render off the edge
/// of the sheet the way a hand-rolled menu at the bottom of a scrolling panel
/// can.
#[component]
fn SelectRow(props: SelectProps) -> Element {
    let current = props.value.clone();
    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "{props.label}" }
            span { class: "rg-field__control",
                select {
                    class: "rg-select",
                    aria_label: "{props.label}",
                    onchange: move |e| props.onpick.call(e.value()),
                    for (value, label) in props.options.iter() {
                        option {
                            key: "{value}",
                            value: "{value}",
                            selected: *value == current,
                            "{label}"
                        }
                    }
                }
            }
            if !props.desc.is_empty() {
                span { class: "rg-field__desc", "{props.desc}" }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct PanelProps {
    state: Signal<UiState>,
}

// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

#[component]
fn AppearancePanel(props: PanelProps) -> Element {
    let state = props.state;
    let settings = state.read().daemon.settings.clone();
    let mut detected = use_signal(system_theme);

    let system_note = match detected() {
        Some(vitrum_os::theme::Theme::Dark) => "The desktop currently reports dark.".to_string(),
        Some(vitrum_os::theme::Theme::Light) => "The desktop currently reports light.".to_string(),
        None => {
            "This desktop does not expose an appearance setting, so System paints dark.".to_string()
        }
    };

    rsx! {
        SelectRow {
            label: "Theme",
            desc: system_note,
            value: match settings.theme {
                ThemePref::System => "system",
                ThemePref::Light => "light",
                ThemePref::Dark => "dark",
            }.to_string(),
            options: vec![
                ("system".to_string(), "Follow the system".to_string()),
                ("dark".to_string(), "Dark".to_string()),
                ("light".to_string(), "Light".to_string()),
            ],
            onpick: move |v: String| {
                edit(state, |s| {
                    s.theme = match v.as_str() {
                        "light" => ThemePref::Light,
                        "dark" => ThemePref::Dark,
                        _ => ThemePref::System,
                    };
                });
            },
        }

        if settings.theme == ThemePref::System {
            div { class: "rg-field",
                span { class: "rg-field__control",
                    button {
                        class: "rg-btn",
                        r#type: "button",
                        onclick: move |_| { detected.set(refresh_system_theme()); },
                        "Re-read the system theme"
                    }
                }
                span { class: "rg-field__desc",
                    "Read on demand rather than watched. A background watcher would park a thread \
                     on a D-Bus signal for the life of the process, and idle cost is the point of \
                     this product."
                }
            }
        }

        SelectRow {
            label: "Density",
            desc: "Row heights and the spacing inside them. Text size is the next control; \
                   the two are separate so a dense list can still have readable type."
                .to_string(),
            value: match settings.density {
                Density::Comfortable => "comfortable",
                Density::Compact => "compact",
            }.to_string(),
            options: vec![
                ("comfortable".to_string(), "Comfortable".to_string()),
                ("compact".to_string(), "Compact".to_string()),
            ],
            onpick: move |v: String| {
                edit(state, |s| {
                    s.density = if v == "compact" { Density::Compact } else { Density::Comfortable };
                });
            },
        }

        SelectRow {
            label: "Text scale",
            desc: "Scales the whole shell, not just type: every size in both stylesheets is \
                   declared in rem. Composes on top of the display's own scaling."
                .to_string(),
            value: settings.text_scale_pct.to_string(),
            options: UI_SCALE_STEPS
                .iter()
                .map(|pct| (pct.to_string(), format!("{pct}%")))
                .collect(),
            onpick: move |v: String| {
                if let Ok(pct) = v.parse::<u16>() {
                    edit(state, |s| s.set_text_scale(pct));
                }
            },
        }

        SwitchRow {
            label: "Reduce motion".to_string(),
            desc: "Zeroes both transition durations. The stylesheets already honour the OS \
                   preference; this forces it on regardless."
                .to_string(),
            on: settings.reduce_motion,
            onchange: move |on| edit(state, |s| s.reduce_motion = on),
        }
    }
}

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

#[component]
fn SidebarPanel(props: PanelProps) -> Element {
    let state = props.state;
    let settings = state.read().daemon.settings.clone();

    rsx! {
        SwitchRow {
            label: "Show the git branch".to_string(),
            desc: "The branch chip on rows whose directory is a checkout.".to_string(),
            on: settings.show_branch,
            onchange: move |on| edit(state, |s| s.show_branch = on),
        }
        SwitchRow {
            label: "Show the last-activity time".to_string(),
            desc: "The relative age at the right of each row.".to_string(),
            on: settings.show_time,
            onchange: move |on| edit(state, |s| s.show_time = on),
        }
        SwitchRow {
            label: "Show the status word".to_string(),
            desc: "Off leaves the status icon, which is what the collapsed sidebar already \
                   renders, so a narrow list stays readable."
                .to_string(),
            on: settings.show_status_word,
            onchange: move |on| edit(state, |s| s.show_status_word = on),
        }
        SwitchRow {
            label: "Dense rows".to_string(),
            desc: "Collapses every row to the slim variant, including the inbox, which normally \
                   gets the taller card. Different from Compact density: that shrinks both \
                   variants, this removes one of them."
                .to_string(),
            on: settings.always_slim,
            onchange: move |on| edit(state, |s| s.always_slim = on),
        }
        SwitchRow {
            label: "Confirm before terminating".to_string(),
            desc: "Terminating kills the agent's child process. There is no undo.".to_string(),
            on: settings.confirm_terminate,
            onchange: move |on| edit(state, |s| s.confirm_terminate = on),
        }
        SelectRow {
            label: "Settle idle sessions automatically",
            desc: "A settled session drops out of the inbox into the Settled band. This is the \
                   only disposition rule with a number in it, and it governs sections, rollups \
                   and the attention jump keys as well as the list."
                .to_string(),
            value: settings
                .policy
                .auto_settle_after_ms
                .map_or_else(|| "off".to_string(), |ms| ms.to_string()),
            options: SETTLE_STEPS
                .iter()
                .map(|(ms, label)| {
                    (
                        ms.map_or_else(|| "off".to_string(), |v| v.to_string()),
                        (*label).to_string(),
                    )
                })
                .collect(),
            onpick: move |v: String| {
                let ms = v.parse::<u64>().ok();
                edit(state, |s| s.policy.auto_settle_after_ms = ms);
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Workspaces
// ---------------------------------------------------------------------------

#[component]
fn WorkspacesPanel(props: PanelProps) -> Element {
    let state = props.state;
    let snapshot = state.read();
    let workspaces: Vec<(WorkspaceId, String, usize)> = snapshot
        .daemon
        .workspaces
        .iter()
        .map(|w| {
            (
                w.id,
                w.display_name().to_string(),
                snapshot.daemon.workspaces.session_count(w.id),
            )
        })
        .collect();
    let count = workspaces.len();
    let intake = snapshot.daemon.workspaces.intake();
    let viewing = snapshot.window.workspace;
    let selected = snapshot
        .daemon
        .workspaces
        .get(viewing)
        .map(|w| (w.display_name().to_string(), w.grouping, w.sections));
    let folders: Vec<(FolderId, String)> = snapshot
        .daemon
        .workspaces
        .get(viewing)
        .map(|w| w.folders().iter().map(|f| (f.id, f.name.clone())).collect())
        .unwrap_or_default();
    drop(snapshot);

    let error = use_signal(String::new);
    let mut new_name = use_signal(String::new);
    let mut new_folder = use_signal(String::new);

    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "Workspaces" }
            span { class: "rg-field__desc",
                "A workspace is a separate top-level context, above projects. Every session \
                 belongs to exactly one, so a new workspace starts genuinely empty. New sessions \
                 land in whichever workspace you are looking at."
            }
        }

        // Above the list, not below it. A refusal rendered after a long list
        // is off the bottom of the scroller, and a message nobody can see
        // without scrolling is the same as no message: the control looks like
        // it silently did nothing. Measured in the running binary, where a
        // refused shortcut put its sentence three scroll notches below the
        // fold.
        if !error.read().is_empty() {
            div { class: "rg-sheet__error", "{error}" }
        }

        for (index , (id , name , sessions)) in workspaces.iter().cloned().enumerate() {
            div {
                class: if id == viewing { "rg-field rg-field--ws rg-field--ws-active" } else { "rg-field rg-field--ws" },
                key: "{id.0}",

                input {
                    class: "rg-field__input rg-field__input--prose",
                    r#type: "text",
                    value: "{name}",
                    spellcheck: false,
                    autocomplete: "off",
                    aria_label: "Workspace name",
                    onchange: move |e| {
                        let text = e.value();
                        try_edit(state, error, |st| st.daemon.workspaces.rename(id, &text));
                    },
                }

                span { class: "rg-field__hint",
                    if sessions == 1 { "1 session" } else { "{sessions} sessions" }
                    if id == intake { " · new sessions land here" }
                }

                span { class: "rg-field__control",
                    button {
                        class: "rg-btn",
                        r#type: "button",
                        disabled: id == viewing,
                        onclick: move |_| {
                            let now = crate::tick().now_ms;
                            try_edit(state, error, |st| st.set_workspace(id, now));
                        },
                        if id == viewing { "Viewing" } else { "Switch to" }
                    }
                    button {
                        class: "rg-btn",
                        r#type: "button",
                        disabled: index == 0,
                        aria_label: "Move up",
                        onclick: move |_| {
                            try_edit(
                                state,
                                error,
                                |st| st.daemon.workspaces.move_to(id, index.saturating_sub(1)),
                            );
                        },
                        "\u{2191}"
                    }
                    button {
                        class: "rg-btn",
                        r#type: "button",
                        disabled: index + 1 >= count,
                        aria_label: "Move down",
                        onclick: move |_| {
                            try_edit(state, error, |st| st.daemon.workspaces.move_to(id, index + 1));
                        },
                        "\u{2193}"
                    }
                    button {
                        class: "rg-btn rg-btn--danger",
                        r#type: "button",
                        onclick: move |_| {
                            let now = crate::tick().now_ms;
                            try_edit(state, error, |st| st.delete_workspace(id, now));
                        },
                        "Delete"
                    }
                }
            }
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "New workspace" }
            span { class: "rg-field__control",
                input {
                    class: "rg-field__input rg-field__input--prose",
                    r#type: "text",
                    placeholder: "Name",
                    value: "{new_name}",
                    spellcheck: false,
                    autocomplete: "off",
                    aria_label: "New workspace name",
                    // `onchange`, never `oninput`. See `PresetsPanel`.
                    onchange: move |e| new_name.set(e.value()),
                }
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    onclick: move |_| {
                        let name = new_name.peek().clone();
                        let before = state.peek().daemon.workspaces.len();
                        try_edit(state, error, |st| st.create_workspace(&name));
                        if state.peek().daemon.workspaces.len() > before {
                            new_name.set(String::new());
                        }
                    },
                    "Create"
                }
            }
        }

        if let Some((name, grouping, sections)) = selected {
            div { class: "rg-field",
                span { class: "rg-field__label", "{name}" }
                span { class: "rg-field__desc",
                    "Grouping and band visibility belong to the workspace, not to you: \
                     \u{201c}this one is my review queue, show me settled work\u{201d} is a fact \
                     about the context and not about the person."
                }
            }

            SelectRow {
                label: "Group rows by",
                desc: match grouping {
                    Grouping::Directory => "A session under a project root the daemon knows files under that project; everything else gets a bucket per directory."
                        .to_string(),
                    Grouping::Named => "Folders you create, in your order, plus an Unfiled bucket. Move rows between folders from the right-click menu."
                        .to_string(),
                },
                value: match grouping {
                    Grouping::Directory => "directory",
                    Grouping::Named => "named",
                }
                    .to_string(),
                options: vec![
                    ("directory".to_string(), Grouping::Directory.label().to_string()),
                    ("named".to_string(), Grouping::Named.label().to_string()),
                ],
                onpick: move |v: String| {
                    edit_state(
                        state,
                        |st| {
                            if let Some(w) = st.daemon.workspaces.get_mut(viewing) {
                                w.grouping = if v == "named" {
                                    Grouping::Named
                                } else {
                                    Grouping::Directory
                                };
                            }
                        },
                    );
                },
            }

            for (disposition , label , on) in [
                (vitrum_model::Disposition::Active, "Active", sections.active),
                (vitrum_model::Disposition::Woke, "Woke", sections.woke),
                (vitrum_model::Disposition::Snoozed, "Snoozed", sections.snoozed),
                (vitrum_model::Disposition::Settled, "Settled", sections.settled),
            ] {
                SwitchRow {
                    key: "{label}",
                    // `format!`, not `"Show {label}".to_string()`: rsx
                    // interpolates text nodes and attribute values, not a
                    // string literal being passed to `.to_string()`, so the
                    // latter ships the four literal characters `{lab` … to the
                    // screen. It did, and the screenshot caught it.
                    label: format!("Show {label}"),
                    desc: String::new(),
                    on,
                    onchange: move |want| {
                        edit_state(
                            state,
                            |st| {
                                if let Some(w) = st.daemon.workspaces.get_mut(viewing) {
                                    w.sections.set(disposition, want);
                                }
                            },
                        );
                    },
                }
            }

            if sections.hidden_count() > 0 {
                div { class: "rg-field",
                    span { class: "rg-field__hint",
                        // Hidden bands are a footgun: the rows still exist, are
                        // still counted in every rollup, and are simply not on
                        // screen. Four unlabelled switches do not say how many
                        // you have turned off, and "where did that session go"
                        // is the question this line exists to answer.
                        if sections.hidden_count() == 1 {
                            "1 band is hidden in this workspace. Its sessions still exist and still count; they are just not drawn."
                        } else {
                            "{sections.hidden_count()} bands are hidden in this workspace. Their sessions still exist and still count; they are just not drawn."
                        }
                    }
                }
            }

            if grouping == Grouping::Named {
                div { class: "rg-field",
                    span { class: "rg-field__label", "Folders" }
                    if folders.is_empty() {
                        span { class: "rg-field__hint",
                            "No folders yet. Every session shows under Unfiled until you make one."
                        }
                    }
                }

                for (index , (fid , fname)) in folders.iter().cloned().enumerate() {
                    div { class: "rg-field rg-field--ws", key: "{fid.0}",
                        input {
                            class: "rg-field__input rg-field__input--prose",
                            r#type: "text",
                            value: "{fname}",
                            spellcheck: false,
                            autocomplete: "off",
                            aria_label: "Folder name",
                            onchange: move |e| {
                                let text = e.value();
                                try_edit(
                                    state,
                                    error,
                                    |st| st.daemon.workspaces.rename_folder(viewing, fid, &text),
                                );
                            },
                        }
                        span { class: "rg-field__control",
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                disabled: index == 0,
                                aria_label: "Move folder up",
                                onclick: move |_| {
                                    try_edit(
                                        state,
                                        error,
                                        |st| {
                                            st.daemon
                                                .workspaces
                                                .move_folder(viewing, fid, index.saturating_sub(1))
                                        },
                                    );
                                },
                                "\u{2191}"
                            }
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                disabled: index + 1 >= folders.len(),
                                aria_label: "Move folder down",
                                onclick: move |_| {
                                    try_edit(
                                        state,
                                        error,
                                        |st| st.daemon.workspaces.move_folder(viewing, fid, index + 1),
                                    );
                                },
                                "\u{2193}"
                            }
                            button {
                                class: "rg-btn rg-btn--danger",
                                r#type: "button",
                                onclick: move |_| {
                                    try_edit(
                                        state,
                                        error,
                                        |st| st.daemon.workspaces.delete_folder(viewing, fid),
                                    );
                                },
                                "Delete"
                            }
                        }
                    }
                }

                div { class: "rg-field",
                    span { class: "rg-field__control",
                        input {
                            class: "rg-field__input rg-field__input--prose",
                            r#type: "text",
                            placeholder: "New folder",
                            value: "{new_folder}",
                            spellcheck: false,
                            autocomplete: "off",
                            aria_label: "New folder name",
                            onchange: move |e| new_folder.set(e.value()),
                        }
                        button {
                            class: "rg-btn",
                            r#type: "button",
                            onclick: move |_| {
                                let name = new_folder.peek().clone();
                                try_edit(
                                    state,
                                    error,
                                    |st| st.daemon.workspaces.create_folder(viewing, &name),
                                );
                                if error.peek().is_empty() {
                                    new_folder.set(String::new());
                                }
                            },
                            "Add folder"
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Re-read the saved commands, apply one change, write them back.
///
/// Re-reads rather than trusting the signal it is about to overwrite. Every
/// window in the process edits one file, so a copy taken when this panel
/// mounted is stale the moment a second window adds a row, and writing that
/// stale copy back whole would delete the other window's work with nothing on
/// screen saying so.
///
/// The signal is only advanced when the write succeeded. A row that is on the
/// screen but not on the disk is the defect this whole tab exists to avoid, so
/// a failed write leaves the fields showing what is actually stored and puts
/// the reason underneath them.
fn edit_presets(
    mut list: Signal<Vec<crate::launch::SavedPreset>>,
    mut error: Signal<String>,
    state: Signal<UiState>,
    change: impl FnOnce(&mut Vec<crate::launch::SavedPreset>) -> Result<(), PresetRefusal>,
) {
    let mut next = crate::launch::presets_saved();
    match change(&mut next) {
        // Nothing was mutated: every operation validates before it writes. The
        // list is still advanced because the re-read above may itself be news,
        // which is the case `Vanished` is reporting.
        Err(why) => {
            error.set(why.to_string());
            list.set(next);
        }
        Ok(()) => match crate::launch::save_presets(&next) {
            Ok(()) => {
                error.set(String::new());
                list.set(next);
                // A preset's chord lives in the SAME table the built-in
                // chords do, so saving one has to re-push that table or the
                // shortcut the operator just bound does nothing until the app
                // restarts. Presets are not part of `Settings`, so the commit
                // path that normally does this never runs for them: this is
                // the one place that closes the link.
                apply_live(&state.peek().daemon.settings);
            }
            Err(why) => error.set(format!(
                "The saved commands could not be written: {why}. Nothing on disk changed."
            )),
        },
    }
}

/// The saved-command editor.
///
/// Takes no props, and that is a statement about where the data lives. Saved
/// commands are not in [`Settings`]: they are a list of records the operator
/// authored, they are consumed by [`crate::launch`] rather than by any
/// derivation in this module, and putting them in the settings document would
/// have meant every window's `save_prefs` rewriting them on every unrelated
/// preference change.
///
/// Editing is direct. There is no "edit preset" sub-dialog, because a dialog
/// inside a dialog gives the escape key two meanings that nothing on screen
/// distinguishes, and because a four-field record is smaller than the modal
/// that would frame it.
///
/// # Every field commits on `onchange`, and none on `oninput`
///
/// Measured, not preferred. A text input whose `value` is bound to a signal
/// and whose `oninput` writes that signal re-renders the panel on every
/// keystroke, and the re-render writes `value` back into the DOM node while
/// the operator is still typing into it. Characters are lost. Driving the
/// running binary through xdotool at a 20 ms inter-key delay, the two create
/// fields in this panel took `Missing agent` as `Misn aet` and
/// `no-such-agent-xyz --flag` as `n-uh-agt-xy -flag`, while the row fields
/// beside them, which already committed on `onchange`, took a 16-character
/// path at the same delay with every character intact.
///
/// So nothing in this file reads a half-typed field. `onchange` fires on
/// blur, and the blur that a click on the primary button causes is dispatched
/// before that button's click, which is what makes reading the signal in the
/// click handler correct. The same defect was in the Workspaces panel's two
/// name fields and the Advanced panel's daemon URL, and all three are fixed
/// the same way.
#[component]
fn PresetsPanel(state: Signal<UiState>) -> Element {
    let list = use_signal(crate::launch::presets_saved);
    let error = use_signal(String::new);
    let mut new_label = use_signal(String::new);
    let mut new_command = use_signal(String::new);

    let rows = list();
    let count = rows.len();

    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "Saved commands" }
            span { class: "rg-field__desc",
                "A label, a program, and its arguments. Saved commands appear in the \
                 new-session dialog's picker, so the agent you start twenty times a day is one \
                 click rather than a retyped command line. A shortcut starts its command while \
                 that dialog is open; nothing binds these keys anywhere else."
            }
        }

        // Above the list. See the same banner in `WorkspacesPanel`.
        if !error.read().is_empty() {
            div { class: "rg-sheet__error", "{error}" }
        }

        if rows.is_empty() {
            div { class: "rg-preset__empty",
                "None saved yet. The new-session dialog still accepts any command line; a saved \
                 command is for the ones you type often."
            }
        }

        for (index , preset) in rows.iter().cloned().enumerate() {
            {
                let id = preset.id;
                // One PATH walk per row, on a panel that re-renders only when
                // a field is committed. It is the same check the dialog runs
                // before it spawns, run early enough to be useful.
                let fault = crate::launch::preset_fault(&preset);
                let line = crate::launch::join_command(&preset.command, &preset.args);
                let cwd = preset.cwd.clone().unwrap_or_default();
                let shortcut = preset.shortcut.clone().unwrap_or_default();
                rsx! {
                    div { class: "rg-field rg-field--preset", key: "{id}",
                        input {
                            class: "rg-field__input rg-field__input--prose rg-preset__label",
                            r#type: "text",
                            value: "{preset.label}",
                            spellcheck: false,
                            autocomplete: "off",
                            aria_label: "Label",
                            onchange: move |e| {
                                let text = e.value();
                                edit_presets(
                                    list,
                                    error,
                                    state,
                                    |l| revise(l, id, PresetField::Label, &text),
                                );
                            },
                        }
                        span { class: "rg-field__control",
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                disabled: index == 0,
                                aria_label: "Move up",
                                onclick: move |_| {
                                    edit_presets(
                                        list,
                                        error,
                                        state,
                                        |l| {
                                            move_by(l, id, -1).then_some(()).ok_or(PresetRefusal::Vanished)
                                        },
                                    );
                                },
                                "\u{2191}"
                            }
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                disabled: index + 1 >= count,
                                aria_label: "Move down",
                                onclick: move |_| {
                                    edit_presets(
                                        list,
                                        error,
                                        state,
                                        |l| {
                                            move_by(l, id, 1).then_some(()).ok_or(PresetRefusal::Vanished)
                                        },
                                    );
                                },
                                "\u{2193}"
                            }
                            button {
                                class: "rg-btn rg-btn--danger",
                                r#type: "button",
                                onclick: move |_| {
                                    edit_presets(
                                        list,
                                        error,
                                        state,
                                        |l| remove(l, id).then_some(()).ok_or(PresetRefusal::Vanished),
                                    );
                                },
                                "Delete"
                            }
                        }
                        input {
                            class: "rg-field__input rg-preset__cmd",
                            r#type: "text",
                            value: "{line}",
                            spellcheck: false,
                            autocomplete: "off",
                            placeholder: "Command and arguments",
                            aria_label: "Command and arguments",
                            onchange: move |e| {
                                let text = e.value();
                                edit_presets(
                                    list,
                                    error,
                                    state,
                                    |l| revise(l, id, PresetField::CommandLine, &text),
                                );
                            },
                        }
                        input {
                            class: "rg-field__input rg-preset__cwd",
                            r#type: "text",
                            value: "{cwd}",
                            spellcheck: false,
                            autocomplete: "off",
                            placeholder: "Working directory, or the dialog's",
                            aria_label: "Default working directory",
                            onchange: move |e| {
                                let text = e.value();
                                edit_presets(list, error, state, |l| revise(l, id, PresetField::Cwd, &text));
                            },
                        }
                        input {
                            class: "rg-field__input rg-preset__key",
                            r#type: "text",
                            value: "{shortcut}",
                            spellcheck: false,
                            autocomplete: "off",
                            placeholder: "Shortcut",
                            aria_label: "Shortcut",
                            onchange: move |e| {
                                let text = e.value();
                                edit_presets(
                                    list,
                                    error,
                                    state,
                                    |l| revise(l, id, PresetField::Shortcut, &text),
                                );
                            },
                        }
                        if let Some(fault) = fault {
                            span { class: "rg-field__hint rg-preset__fault", "{fault.sentence()}" }
                        }
                        crate::ui::icons::IconPicker {
                            selected: preset.icon.clone(),
                            command_line: line.clone(),
                            on_pick: move |slug: Option<String>| {
                                let text = slug.unwrap_or_default();
                                edit_presets(
                                    list,
                                    error,
                                    state,
                                    |l| revise(l, id, PresetField::Icon, &text),
                                );
                            },
                        }
                    }
                }
            }
        }

        div { class: "rg-field rg-field--preset-new",
            input {
                class: "rg-field__input rg-field__input--prose rg-preset__label",
                r#type: "text",
                value: "{new_label}",
                spellcheck: false,
                autocomplete: "off",
                placeholder: "Label",
                aria_label: "New saved command label",
                onchange: move |e| new_label.set(e.value()),
            }
            input {
                class: "rg-field__input rg-preset__cmd",
                r#type: "text",
                value: "{new_command}",
                spellcheck: false,
                autocomplete: "off",
                placeholder: "Command and arguments",
                aria_label: "New saved command line",
                onchange: move |e| new_command.set(e.value()),
            }
            span { class: "rg-field__control",
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    onclick: move |_| {
                        let label = new_label.peek().clone();
                        let command = new_command.peek().clone();
                        edit_presets(list, error, state, |l| create(l, &label, &command).map(|_| ()));
                        if error.peek().is_empty() {
                            new_label.set(String::new());
                            new_command.set(String::new());
                        }
                    },
                    "Save command"
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------------

#[component]
fn TerminalPanel(props: PanelProps) -> Element {
    let state = props.state;
    let prefs = state.read().daemon.settings.terminal.clone();

    rsx! {
        SelectRow {
            label: "Colours",
            desc: palette_note(prefs.palette).to_string(),
            value: prefs.palette.slug().to_string(),
            options: crate::termpalette::ALL
                .iter()
                .map(|p| (p.slug().to_string(), p.label().to_string()))
                .collect(),
            onpick: move |v: String| {
                let picked = crate::termpalette::TermPalette::from_slug(&v);
                edit(state, |s| s.terminal.palette = picked);
            },
        }

        SelectRow {
            label: "Renderer",
            desc: renderer_note(prefs.renderer).to_string(),
            value: renderer_wire(prefs.renderer).to_string(),
            options: vec![
                ("dom".to_string(), format!("{} (default)", renderer_label(TermRenderer::Dom))),
                ("webgl".to_string(), renderer_label(TermRenderer::Webgl).to_string()),
            ],
            onpick: move |v: String| {
                edit(state, |s| {
                    s.terminal.renderer = if v == "webgl" { TermRenderer::Webgl } else { TermRenderer::Dom };
                });
            },
        }

        SelectRow {
            label: "Font",
            desc: "Every choice ends in the generic monospace, so a font this machine does not \
                   have falls back to another monospace rather than to a proportional face."
                .to_string(),
            value: prefs.font_family.clone(),
            options: FONT_STACKS
                .iter()
                .map(|(label, stack)| ((*stack).to_string(), (*label).to_string()))
                .collect(),
            onpick: move |v: String| edit(state, |s| s.terminal.font_family = v),
        }

        SelectRow {
            label: "Font size",
            desc: "Independent of the shell's text scale: a large terminal beside a dense \
                   sidebar is the normal case."
                .to_string(),
            value: prefs.font_size_px.to_string(),
            options: TERM_FONT_STEPS
                .iter()
                .map(|px| (px.to_string(), format!("{px} px")))
                .collect(),
            onpick: move |v: String| {
                if let Ok(px) = v.parse::<u16>() {
                    edit(state, |s| {
                        s.terminal.font_size_px = px.clamp(TERM_FONT_MIN_PX, TERM_FONT_MAX_PX);
                    });
                }
            },
        }

        SelectRow {
            label: "Scrollback",
            // One number governing two things, because they are the same
            // number: the xterm buffer, and how many bytes of the daemon's
            // history an attach asks for via `wire::backfill_max_bytes`. It
            // used to govern only the buffer. The backfill was a hard-coded
            // 64 KiB, so "100,000 lines" grew the buffer a hundredfold and
            // fetched nothing extra, while this caption said raising it was
            // how you see further back.
            //
            // Paging is real now, so this is no longer the ONLY way to see
            // further back and the caption no longer says it is. It sizes the
            // first request; scrolling to the top grows the window one step at
            // a time up to `wire::PAGE_CEILING_BYTES`.
            desc: "Sets the buffer here and the size of the first request an attach \
                   makes: 64 bytes of the daemon's history per line, stopping at 2 MiB, \
                   about 32,000 lines' worth. Scroll to the top of a pane to fetch \
                   older history, one step at a time, up to 8 MiB in one pane."
                .to_string(),
            value: prefs.scrollback_lines.to_string(),
            options: SCROLLBACK_STEPS
                .iter()
                .map(|(lines, label)| (lines.to_string(), (*label).to_string()))
                .collect(),
            onpick: move |v: String| {
                if let Ok(lines) = v.parse::<u32>() {
                    edit(state, |s| s.terminal.scrollback_lines = lines);
                }
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

#[component]
fn NotificationsPanel(props: PanelProps) -> Element {
    let state = props.state;
    let prefs = state.read().daemon.settings.notifications;
    // Connecting is a D-Bus handshake. Once per mount of this panel, never per
    // render, and never while the modal is on another tab.
    let support = use_hook(notify_support);

    rsx! {
        if let Some(why) = support.reason() {
            div { class: "rg-sheet__warn",
                "This desktop cannot deliver notifications: {why}. The switches below still \
                 record your preference, but nothing will be shown until the service is \
                 available."
            }
        }

        for kind in NOTIFY_KINDS {
            {
                let (label, desc) = notify_label(kind);
                rsx! {
                    SwitchRow {
                        key: "{kind}",
                        label: label.to_string(),
                        desc: desc.to_string(),
                        on: notify_enabled(&prefs, kind),
                        onchange: move |on| {
                            edit(state, |s| set_notify_enabled(&mut s.notifications, kind, on));
                        },
                    }
                }
            }
        }

        SwitchRow {
            label: "Stay quiet about the session on screen".to_string(),
            desc: "Watching an agent finish and then being told it finished is noise.".to_string(),
            on: prefs.skip_focused_session,
            onchange: move |on| edit(state, |s| s.notifications.skip_focused_session = on),
        }

        div { class: "rg-sheet__note",
            "Clicking a notification opens the session it is about in a new window, through the \
             same vitrum://session/<id> handoff a link from a browser takes. The window you are \
             in is left where it is."
        }
    }
}

// ---------------------------------------------------------------------------
// Advanced
// ---------------------------------------------------------------------------

#[derive(Props, Clone, PartialEq)]
struct AdvancedProps {
    state: Signal<UiState>,
    on_reconnect: EventHandler<String>,
}

#[component]
fn AdvancedPanel(props: AdvancedProps) -> Element {
    let state = props.state;
    let settings = state.read().daemon.settings.clone();
    let mut url = use_signal(|| settings.daemon_url.clone());

    // Eight probes, several of them a service handshake. Once per mount of this
    // panel, which only happens when the operator asks for it.
    let report = use_hook(|| vitrum_os::probe(None));
    let path = crate::state::ui_state_path();

    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "Daemon" }
            span { class: "rg-field__control",
                input {
                    class: "rg-field__input",
                    r#type: "text",
                    spellcheck: false,
                    autocomplete: "off",
                    placeholder: "ws://127.0.0.1:7737",
                    value: "{url}",
                    aria_label: "Daemon URL",
                    onchange: move |e| url.set(e.value()),
                }
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    onclick: move |_| {
                        let next = url.peek().trim().to_string();
                        edit(state, |s| s.daemon_url.clone_from(&next));
                        let dial = {
                            let read = state.peek();
                            read.daemon.settings.resolved_daemon_url("").to_string()
                        };
                        if !dial.is_empty() {
                            props.on_reconnect.call(dial);
                        }
                    },
                    "Save and reconnect"
                }
            }
            span { class: "rg-field__hint",
                "Empty means whatever --server said on the command line, which keeps the flag \
                 authoritative for the case it exists for. Saving reconnects immediately."
            }
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "Platform integration" }
            span { class: "rg-field__desc",
                "Probed live, just now. Anything unavailable says why rather than failing \
                 silently later."
            }
            div { class: "rg-keys",
                for (feature, support) in report.iter() {
                    div { class: "rg-keys__row", key: "{feature}",
                        span { class: "rg-keys__chord", "{feature}" }
                        span { class: "rg-keys__what", "{support}" }
                    }
                }
            }
        }

        // The daemon already knows why its watcher is partial and says so in
        // finished sentences. Until now the client stored them and rendered
        // nothing, so on macOS and Windows the contested-files marker simply
        // never appeared and no screen said why.
        {
            let collisions = state.read().daemon.collisions.clone();
            rsx! {
                div { class: "rg-field",
                    span { class: "rg-field__label", "Contested files" }
                    span { class: "rg-field__desc", "{collisions.summary()}" }
                    if !collisions.reasons().is_empty() {
                        div { class: "rg-keys",
                            for (i , reason) in collisions.reasons().iter().enumerate() {
                                div { class: "rg-keys__row", key: "{i}",
                                    span { class: "rg-keys__what", "{reason}" }
                                }
                            }
                        }
                    }
                }
            }
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "Settings file" }
            span { class: "rg-field__hint",
                match &path {
                    Ok(p) => p.display().to_string(),
                    Err(why) => format!("no config directory on this platform: {why}"),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// About
// ---------------------------------------------------------------------------

/// What the update control is doing right now.
///
/// One value rather than a set of booleans, because the states are mutually
/// exclusive and a pair of flags is how a control ends up saying "checking"
/// and "up to date" at once.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateUi {
    /// Nothing asked for yet.
    Idle,
    /// A check or an install is in flight; the string is the current step.
    Busy(String),
    /// The answer to the last check.
    Answer(crate::update::Status),
    /// The update finished and the new binaries are on disk.
    Installed(String),
    /// The last attempt failed, and why.
    Failed(String),
}

#[component]
fn AboutPanel(state: Signal<UiState>) -> Element {
    let mut ui = use_signal(|| UpdateUi::Idle);
    let current = crate::update::current_version();

    // The daemon is a separate process that outlives every window, so its
    // version is not this binary's version and after an update it will not be.
    // Read from the Welcome frame rather than assumed.
    let daemon_version = match &state.read().daemon.conn {
        crate::state::ConnState::Live { server_version } => Some(server_version.clone()),
        _ => None,
    };
    let daemon_is_stale = daemon_version
        .as_deref()
        .is_some_and(|v| v != current.to_string());

    // Held so the Install button knows what it is installing without asking
    // the network a second time and risking a different answer than the one
    // the operator is looking at.
    let mut ready = use_signal(|| None::<crate::update::Available>);

    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "Version" }
            span { class: "rg-field__hint", "vitrum {current} ({crate::update::TARGET})" }
            span { class: "rg-field__hint",
                match &daemon_version {
                    Some(v) if daemon_is_stale => format!(
                        "The daemon holding your sessions is still running {v}. Restarting it \
                         picks up {current} and ends every session it is holding."
                    ),
                    Some(v) => format!("Daemon {v}, running your sessions."),
                    None => "Not connected to a daemon.".to_string(),
                }
            }
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "Updates" }
            span { class: "rg-field__control",
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    disabled: matches!(ui(), UpdateUi::Busy(_)),
                    onclick: move |_| {
                        ui.set(UpdateUi::Busy("checking".to_string()));
                        ready.set(None);
                        spawn(async move {
                            // The check is a blocking HTTP round trip. On the
                            // UI thread it would freeze every window in this
                            // process, since they share one event loop.
                            let got = crate::off_thread(crate::update::check).await;
                            match got {
                                Ok(status) => {
                                    if let crate::update::Status::Ready(a) = &status {
                                        ready.set(Some(a.clone()));
                                    }
                                    ui.set(UpdateUi::Answer(status));
                                }
                                Err(e) => ui.set(UpdateUi::Failed(format!("{e:#}"))),
                            }
                        });
                    },
                    "Check for updates"
                }
                if let Some(available) = ready() {
                    button {
                        class: "rg-btn rg-btn--primary",
                        r#type: "button",
                        disabled: matches!(ui(), UpdateUi::Busy(_)),
                        onclick: move |_| {
                            let available = available.clone();
                            ui.set(UpdateUi::Busy("starting".to_string()));
                            spawn(async move {
                                let done = crate::off_thread(move || {
                                    let dir = crate::update::install_dir()?;
                                    if !crate::update::writable(&dir) {
                                        anyhow::bail!(
                                            "cannot write to {}. This copy was installed by \
                                             something else; update it the same way.",
                                            dir.display()
                                        );
                                    }
                                    // Progress is discarded on this path on
                                    // purpose: a signal cannot be written from
                                    // the worker thread, and the steps take a
                                    // few seconds in total. The button says
                                    // what is happening; a per-step readout
                                    // that flickers past is not worth a
                                    // channel.
                                    crate::update::install(&available, &dir, &mut |_| {})?;
                                    Ok::<_, anyhow::Error>(available.version.to_string())
                                })
                                .await;
                                match done {
                                    Ok(v) => ui.set(UpdateUi::Installed(v)),
                                    Err(e) => ui.set(UpdateUi::Failed(format!("{e:#}"))),
                                }
                            });
                        },
                        "Install {available.version}"
                    }
                }
            }
            span { class: "rg-field__hint",
                match ui() {
                    UpdateUi::Idle => format!(
                        "Checks the latest release of {}, never the branch. \
                         The download's checksum must match the one published beside it.",
                        crate::update::REPO
                    ),
                    UpdateUi::Busy(step) => step,
                    UpdateUi::Answer(crate::update::Status::UpToDate { version }) =>
                        format!("vitrum {version} is the newest release."),
                    UpdateUi::Answer(crate::update::Status::NoReleases) => format!(
                        "No releases published for {} yet.", crate::update::REPO
                    ),
                    UpdateUi::Answer(crate::update::Status::NoAssetForPlatform { version, target }) =>
                        format!(
                            "vitrum {version} is available but published no build for {target}. \
                             Build it from source."
                        ),
                    UpdateUi::Answer(crate::update::Status::Ready(a)) =>
                        format!("vitrum {} is available. You have {current}.", a.version),
                    UpdateUi::Installed(v) =>
                        format!("Updated to {v}. {}", crate::update::AFTER_INSTALL),
                    UpdateUi::Failed(why) => why,
                }
            }
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "From a terminal" }
            span { class: "rg-field__hint",
                "vitrum update --check   reports what is available and installs nothing. \
                 vitrum update           installs it. Same code as the button above."
            }
        }
    }
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

/// The saved-command editor, which is the only writer of `launch.json`'s
/// preset list.
///
/// Every test here defends one invariant the new-session dialog is entitled to
/// assume, because it consumes this list and cannot re-validate it: labels are
/// unique and non-empty, ids are unique, a stored shortcut is one the matcher
/// can match, and a stored working directory is either a real string or
/// absent. A refused edit leaves the list byte-identical, so a validation
/// failure can never be a partial write.
#[cfg(test)]
mod saved_commands;

/// Captions in this sheet against what the product actually does.
///
/// Its own module because it is a coherence suite, not a settings-logic one:
/// every test here reads the shipped source of the files that implement the
/// behaviour and asserts that a sentence shown to an operator is true of the
/// code beside it. Source scanning rather than a runtime assertion because
/// neither behaviour has a hook a unit test can reach: one is a wheel event in
/// a webview, the other is a D-Bus click on a live desktop.
#[cfg(test)]
mod sheet_copy_is_true;

/// A saved preset's chord is a SHORTCUT, not a dialog accelerator.
///
/// The distinction is the whole feature. `SavedPreset::shortcut` existed, the
/// new-session dialog matched it, and the settings panel refused conflicts,
/// which made it look finished. But the only matcher was the dialog's own
/// keydown handler, so firing a preset meant opening the dialog first: two
/// keystrokes to reach the thing whose entire purpose was to be one. The
/// design requires shortcuts that do complex things like open a session
/// in a named folder with a named command, and the shipped behaviour did not.
///
/// `bootstrap.js` matches exactly ONE table. These tests are about what is in
/// it.
#[cfg(test)]
mod preset_shortcuts;
