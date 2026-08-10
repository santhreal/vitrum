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
//! - **Sidebar** is `show_branch` / `show_place` / `show_time` /
//!   `show_status_word`, read by
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

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, RwLock, Weak};

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use vitrum_model::{SessionView, SidebarStatus};
use vitrum_os::capability::{Support, Unavailable};
use vitrum_os::notify::{Notification, NotificationKind, Notifier};
use vitrum_proto::{SessionId, SessionStatus};

use crate::instance::Mailbox;
use crate::keymap::{CHORDS, Help, KeyAction, Scope, Shift};
use crate::state::{
    BackdropFit, Density, KeyboardPrefs, Settings, SettingsTab, TermRenderer, TerminalPrefs,
    ThemePref, UiState,
};

/// The About tab. Edits no preference; it reports what is installed.
mod about;
/// Saved commands: the validation rules and the editor that applies them.
mod presets;
/// The Workspaces tab, the one panel that edits no `Settings` field.
mod workspaces;

use about::AboutPanel;
use presets::PresetsPanel;
use workspaces::WorkspacesPanel;

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
    style.push_str(&appearance_tokens(&settings.appearance));
    style
}

/// The custom-protocol URL a backdrop image is served from.
///
/// A WebView document served from a custom scheme cannot load a `file://`
/// URL: it is a cross-scheme request and WebKit refuses it. The image has to
/// come back through a scheme the page is allowed to fetch, so the path is
/// carried in the URL and read by the handler in `chrome.rs`.
///
/// Percent-encoded because a wallpaper lives in a directory with spaces more
/// often than not, and an unencoded space makes the URL unparseable rather
/// than merely wrong.
#[must_use]
pub fn backdrop_url(path: &str) -> String {
    // Exactly one slash between the authority and the path. A POSIX path
    // supplies its own and a Windows one (`C:\...`) does not, so the URL grows
    // it here rather than in the handler; two of them reach the handler as
    // `//home/...`, which is a distinct path on POSIX and simply wrong.
    let mut out = String::from("vitrum-backdrop://local");
    if !path.starts_with('/') {
        out.push('/');
    }
    for byte in path.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

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
/// the webview's base colour are settled before the first paint. So the first
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
         README has the one-line rule for each. Without a compositor running, a \
         see-through window has nothing to blend with."
    } else {
        "How much of the desktop shows through, unblurred. The window is created \
         opaque, so the first change here applies to the next window you open; \
         after that it moves live. Blur belongs to your compositor; README has \
         the rule for Hyprland, KWin and picom."
    }
}

/// Custom properties for translucency and the backdrop image.
///
/// Emitted as tokens rather than as a stylesheet block because the values are
/// per-profile and the stylesheet is a compile-time constant. Every rule that
/// consumes them lives in `parts/23-backdrop.css`, so the shape of the effect
/// is still in CSS and only the numbers come from here.
///
/// Nothing is emitted for a default profile. An operator who has never opened
/// the Appearance tab gets the same document they got before this existed,
/// which keeps the opaque path free of `rgba` compositing it does not need.
///
/// # Three of these tokens switch layers on rather than parameterise them
///
/// `--rg-backdrop-layer`, `--rg-scrim-layer` and `--rg-surface-blur` are the
/// existence of an effect, not its value, and they are here because the
/// claim in the paragraph above was not true of the compositor.
///
/// `.rg-app::before` and `::after` are `content: var(--rg-*-layer)`, which is
/// `none` unless this function says otherwise, so a default profile generates
/// neither pseudo-element. It used to generate both unconditionally: two
/// fixed, viewport-sized layers on every install, one of them carrying
/// `filter: blur(0px)`, which is not a no-op. Any `filter` other than `none`
/// makes an element a containing block, a stacking context and its own
/// composited layer, so the opaque path was paying for a wallpaper it did not
/// have.
///
/// `--rg-surface-blur` is the same argument for `backdrop-filter` on sheets,
/// menus and tooltips. That blur exists to keep text legible where the
/// DESKTOP shows through, and its own comment said so, but nothing gated it:
/// every menu and every tooltip in a fully opaque window forced a readback
/// and a Gaussian blur of everything behind it, to composite it against an
/// opaque surface.
#[must_use]
pub fn appearance_tokens(a: &crate::state::AppearancePrefs) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if a.opacity_pct < crate::state::OPACITY_MAX_PCT {
        let _ = write!(out, "--rg-opacity:{};", f64::from(a.opacity_pct) / 100.0);
        // Only where the desktop genuinely shows through.
        out.push_str("--rg-surface-blur:blur(var(--rg-space-3));");
    }
    if a.terminal_opacity_pct < crate::state::OPACITY_MAX_PCT {
        let _ = write!(
            out,
            "--rg-terminal-opacity:{};",
            f64::from(a.terminal_opacity_pct) / 100.0
        );
    }
    let backdrop = a.backdrop.trim();
    if !backdrop.is_empty() {
        let (size, repeat) = a.backdrop_fit.css();
        let _ = write!(
            out,
            "--rg-backdrop-layer:\"\";--rg-backdrop:url(\"{}\");\
             --rg-backdrop-size:{size};--rg-backdrop-repeat:{repeat};",
            backdrop_url(backdrop)
        );
        if a.backdrop_blur_px > 0 {
            let _ = write!(out, "--rg-backdrop-blur:{}px;", a.backdrop_blur_px);
        }
        if a.backdrop_dim_pct > 0 {
            // The scrim is a second full-viewport layer and earns one only
            // when it would actually paint something.
            let _ = write!(
                out,
                "--rg-scrim-layer:\"\";--rg-backdrop-dim:{};",
                f64::from(a.backdrop_dim_pct) / 100.0
            );
        }
    }
    out
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
pub fn term_options_script(prefs: &TerminalPrefs, opacity_pct: u8) -> String {
    let size = prefs.font_size_px.clamp(TERM_FONT_MIN_PX, TERM_FONT_MAX_PX);
    let family = if prefs.font_family.trim().is_empty() {
        "null".to_string()
    } else {
        json_string(&prefs.font_family)
    };
    // xterm refuses to composite a non-opaque cell background unless it is
    // told to up front, and the flag costs real work: it forces the renderer
    // to blend every cell instead of filling the run. So it is set only when
    // the operator asked for a see-through grid, and an opaque profile keeps
    // exactly the renderer it had.
    //
    // The cell background is cleared in `bootstrap.js` and not here, because
    // the `Inherit` palette sends no theme at all: the colours are read from
    // CSS on the other side, and that is the only place both cases meet. The
    // tint the operator actually sees comes from `.rg-terminal` in
    // `parts/23-backdrop.css`, so exactly one layer applies the alpha.
    let translucent = opacity_pct < crate::state::OPACITY_MAX_PCT;
    format!(
        "window.__vitrum_termOptions={{renderer:{renderer},scrollback:{scrollback},\
         fontSize:{size},fontFamily:{family},theme:{theme},allowTransparency:{translucent}}};\
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
        &keymap_json(&live_chords(
            prefs,
            &crate::launch::load_launch_store().presets,
        )),
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
    script.push_str(&term_options_script(
        &settings.terminal,
        settings.appearance.terminal_opacity_pct,
    ));
    script.push_str(&keymap_script(&settings.keyboard));
    script
}

/// Every live window's inbox for [`live_script`] output.
///
/// Held by `Weak`, so a closing window needs no deregistration call and the
/// bus needs no window identity: the inbox dies with the component tree that
/// owned it, and the next broadcast drops the dangling entry. An explicit
/// unsubscribe would have to be driven from the window layer's close path, and
/// one missed call there is a queue that grows for the life of the process
/// holding scripts nobody will ever drain.
static LISTENERS: Mutex<Vec<Weak<Mailbox<String>>>> = Mutex::new(Vec::new());

/// Take the listener list, ignoring poisoning.
///
/// A panicking window must not cost every other window its settings for the
/// rest of the process. The list is a vector of weak pointers and there is no
/// state a panic could leave half-applied.
fn listeners() -> std::sync::MutexGuard<'static, Vec<Weak<Mailbox<String>>>> {
    LISTENERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Register an inbox for one window and hand it back.
fn subscribe() -> Arc<Mailbox<String>> {
    let inbox = Arc::new(Mailbox::new());
    listeners().push(Arc::downgrade(&inbox));
    inbox
}

/// Post `script` to every window that still has an inbox, forgetting the rest.
fn broadcast(script: &str) {
    let live = {
        let mut listeners = listeners();
        let mut live = Vec::with_capacity(listeners.len());
        listeners.retain(|inbox| match inbox.upgrade() {
            Some(inbox) => {
                live.push(inbox);
                true
            }
            None => false,
        });
        live
    };
    // Posted outside the lock: `Mailbox::post` wakes its wakers, and a waker
    // that resumed its task inline would re-enter this function on a lock this
    // thread already holds.
    for inbox in live {
        inbox.post(script.to_owned());
    }
}

/// Subscribe this window to live settings pushes, for as long as it exists.
///
/// One call per window, at the top of its root component. The inbox lives in a
/// hook rather than inside the future, so the subscription's lifetime is the
/// component tree's rather than the task's.
pub fn use_live_settings() {
    let inbox = use_hook(subscribe);
    use_future(move || {
        let inbox = inbox.clone();
        async move {
            loop {
                let script = inbox.next().await;
                let _ = document::eval(&script);
            }
        }
    });
}

/// Push every live-reconfigurable setting into the window that is mounting.
///
/// The self-directed half of [`apply_live`]. A window that has just been
/// constructed has to catch up to settings changed before it existed, and
/// broadcasting to do it would re-evaluate the same script in every sibling
/// for no reason.
pub fn apply_here(settings: &Settings) {
    let _ = document::eval(&live_script(settings));
}

/// Push every live-reconfigurable setting into EVERY live window.
///
/// Theme, density and reduced motion are absent on purpose: those are
/// attributes on the app root, so Dioxus reapplies them as part of the same
/// re-render the settings change already causes, with no bridge involved.
///
/// # Why this is a broadcast
///
/// `document::eval` runs in the calling scope's document, and dioxus-desktop
/// gives every window its own, while `Settings` lives on `DaemonState`, which
/// every window in the process shares. Evaluating directly here updated every
/// window's markup — they all render from the shared model — but pushed the
/// xterm options and the chord table into one webview only. Windows 2..N kept
/// their old terminal font, scrollback, renderer and keybindings until they
/// were next constructed, which made those four controls silently
/// window-local while every other control in the sheet was global.
///
/// So this posts to a per-window inbox instead, and each window runs the
/// script in its own document. The window that made the change is not a
/// special case: it receives its own broadcast through the same subscription,
/// one executor tick later.
pub fn apply_live(settings: &Settings) {
    broadcast(&live_script(settings));
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
    /// Quiet update check shared with the titlebar chip.
    pub update_offer: Signal<Option<crate::update::Available>>,
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
                            SettingsTab::About => rsx! {
                                AboutPanel {
                                    state: props.state,
                                    offer: props.update_offer,
                                }
                            },
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
fn stray_option(value: &str, options: &[(String, String)]) -> Option<String> {
    if options.iter().any(|(v, _)| v == value) {
        return None;
    }
    Some(format!("{value} (in effect, not one of the choices)"))
}

/// A labelled menu row.
///
/// A native `<select>` rather than a custom popup: it is one element, it is
/// keyboard accessible without any code here, and it cannot render off the edge
/// of the sheet the way a hand-rolled menu at the bottom of a scrolling panel
/// can.
#[component]
fn SelectRow(props: SelectProps) -> Element {
    let onpick = props.onpick;
    let stray = stray_option(&props.value, &props.options);
    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "{props.label}" }
            span { class: "rg-field__control",
                select {
                    class: "rg-select",
                    aria_label: "{props.label}",
                    onchange: move |e| onpick.call(e.value()),
                    if let Some(label) = stray {
                        option {
                            key: "{props.value}",
                            value: "{props.value}",
                            selected: true,
                            "{label}"
                        }
                    }
                    for (value, label) in props.options.iter() {
                        option {
                            key: "{value}",
                            value: "{value}",
                            selected: *value == props.value,
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
    // A memo, not a read, and the difference is one whole class of work.
    //
    // Reading `UiState` in a panel body subscribes the panel to it, and
    // `UiState` carries the session list, so a daemon streaming output twenty
    // times a second re-ran this body twenty times a second: a clone of the
    // whole `Settings` document each time, plus the thirty-odd option strings
    // the menus below build, to draw a tab whose contents had not changed.
    // A memo only marks the panel dirty when `Settings` itself differs.
    let settings = use_memo(move || state.read().daemon.settings.clone());
    let settings = settings.read();
    let mut detected = use_signal(system_theme);
    let mut backdrop = use_signal(|| settings.appearance.backdrop.clone());

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

        SelectRow {
            label: "Window opacity",
            desc: opacity_note(&settings.appearance).to_string(),
            value: settings.appearance.opacity_pct.to_string(),
            options: OPACITY_STEPS.iter().map(|p| (p.to_string(), format!("{p}%"))).collect(),
            onpick: move |v: String| {
                if let Ok(pct) = v.parse::<u8>() {
                    edit(state, |s| { s.appearance.opacity_pct = pct; s.appearance.clamp(); });
                }
            },
        }

        SelectRow {
            label: "Terminal opacity",
            desc: "The grid alone, so the shell can stay solid while the wallpaper reads \
                   behind the text. Below 100% the terminal composites every cell instead \
                   of filling runs of them, which costs a little more per repaint."
                .to_string(),
            value: settings.appearance.terminal_opacity_pct.to_string(),
            options: OPACITY_STEPS.iter().map(|p| (p.to_string(), format!("{p}%"))).collect(),
            onpick: move |v: String| {
                if let Ok(pct) = v.parse::<u8>() {
                    edit(state, |s| {
                        s.appearance.terminal_opacity_pct = pct;
                        s.appearance.clamp();
                    });
                }
            },
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "Backdrop" }
            span { class: "rg-field__control",
                input {
                    class: "rg-field__input rg-field__input--prose",
                    r#type: "text",
                    spellcheck: false,
                    autocomplete: "off",
                    placeholder: "/home/you/wallpaper.png",
                    value: "{backdrop}",
                    aria_label: "Backdrop image path",
                    onchange: move |e| backdrop.set(e.value()),
                }
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    onclick: move |_| {
                        let next = backdrop.peek().trim().to_string();
                        edit(state, |s| s.appearance.backdrop.clone_from(&next));
                    },
                    "Apply"
                }
                if !settings.appearance.backdrop.is_empty() {
                    button {
                        class: "rg-btn",
                        r#type: "button",
                        onclick: move |_| {
                            backdrop.set(String::new());
                            edit(state, |s| s.appearance.backdrop.clear());
                        },
                        "Clear"
                    }
                }
            }
            span { class: "rg-field__desc",
                "An absolute path to a PNG, JPEG, GIF or WEBP. Read by signature and not by \
                 extension, so a file that is not an image is refused rather than drawn. SVG \
                 is refused too: it is a scripted document, and this one would render inside \
                 the application page."
            }
        }

        if !settings.appearance.backdrop.is_empty() {
            SelectRow {
                label: "Backdrop fit",
                desc: "How the image is sized to the window.".to_string(),
                value: match settings.appearance.backdrop_fit {
                    BackdropFit::Cover => "cover",
                    BackdropFit::Contain => "contain",
                    BackdropFit::Tile => "tile",
                    BackdropFit::Center => "center",
                }.to_string(),
                options: vec![
                    ("cover".to_string(), "Fill the window".to_string()),
                    ("contain".to_string(), "Fit the whole image".to_string()),
                    ("tile".to_string(), "Tile".to_string()),
                    ("center".to_string(), "Centre at native size".to_string()),
                ],
                onpick: move |v: String| {
                    edit(state, |s| {
                        s.appearance.backdrop_fit = match v.as_str() {
                            "contain" => BackdropFit::Contain,
                            "tile" => BackdropFit::Tile,
                            "center" => BackdropFit::Center,
                            _ => BackdropFit::Cover,
                        };
                    });
                },
            }

            SelectRow {
                label: "Backdrop blur",
                desc: "Blurred once, when the image loads, not per frame. A wide radius on a \
                       large photograph is the one setting here that costs memory."
                    .to_string(),
                value: settings.appearance.backdrop_blur_px.to_string(),
                options: BLUR_STEPS.iter().map(|p| (p.to_string(), blur_label(*p))).collect(),
                onpick: move |v: String| {
                    if let Ok(px) = v.parse::<u8>() {
                        edit(state, |s| {
                            s.appearance.backdrop_blur_px = px;
                            s.appearance.clamp();
                        });
                    }
                },
            }

            SelectRow {
                label: "Backdrop dim",
                desc: "A scrim between the image and the interface. This is the control that \
                       keeps text readable over a bright photograph."
                    .to_string(),
                value: settings.appearance.backdrop_dim_pct.to_string(),
                options: DIM_STEPS.iter().map(|p| (p.to_string(), format!("{p}%"))).collect(),
                onpick: move |v: String| {
                    if let Ok(pct) = v.parse::<u8>() {
                        edit(state, |s| {
                            s.appearance.backdrop_dim_pct = pct;
                            s.appearance.clamp();
                        });
                    }
                },
            }
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
    // Memoized for the reason `AppearancePanel` gives: a preference tab must
    // not repaint because an agent printed a line.
    let settings = use_memo(move || state.read().daemon.settings.clone());
    let settings = settings.read();

    rsx! {
        SwitchRow {
            label: "Show the git branch".to_string(),
            desc: "The branch chip on rows whose directory is a checkout.".to_string(),
            on: settings.show_branch,
            onchange: move |on| edit(state, |s| s.show_branch = on),
        }
        SwitchRow {
            label: "Show the working directory".to_string(),
            desc: "The part of a session's directory the project header does not already \
                   say. Rows sitting at the project root show nothing. A session an agent \
                   moved, or one in a worktree beside the project, shows where it is."
                .to_string(),
            on: settings.show_place,
            onchange: move |on| edit(state, |s| s.show_place = on),
        }
        SwitchRow {
            label: "Show the last-activity time".to_string(),
            desc: "The relative age at the right of each row.".to_string(),
            on: settings.show_time,
            onchange: move |on| edit(state, |s| s.show_time = on),
        }
        SwitchRow {
            label: "Show the status word".to_string(),
            desc: "Off leaves the pill's colour, which is what the collapsed sidebar already \
                   renders, so a narrow list stays readable. The state stays on the row for \
                   a screen reader either way."
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
// Terminal
// ---------------------------------------------------------------------------

#[component]
fn TerminalPanel(props: PanelProps) -> Element {
    let state = props.state;
    let prefs = use_memo(move || state.read().daemon.settings.terminal.clone());
    let prefs = prefs.read();

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
    let prefs = use_memo(move || state.read().daemon.settings.notifications);
    let prefs = prefs();
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
    let settings = use_memo(move || state.read().daemon.settings.clone());
    let settings = settings.read();
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
/// a webview, the other is a D-Bus click on a live desktop.
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
/// `bootstrap.js` matches exactly ONE table. These tests are about what is in
/// it.
#[cfg(test)]
mod preset_shortcuts;

/// Translucency and the backdrop image: what a default profile costs, what the
/// controls offer, and what survives a hand-edited `ui.json`.
#[cfg(test)]
mod appearance;
