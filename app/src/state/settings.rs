//! Application-global preferences, and the pages of the settings modal.
//!
//! Plain data with clamps. Every value here can arrive from a hand-edited
//! `ui.json` rather than from a control, which is why the ranges are enforced
//! by methods on the types themselves and not at the sliders.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vitrum_model::DispositionPolicy;


/// How tall a sidebar row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

/// Which palette to paint, before the OS gets a say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

/// Which xterm.js renderer the terminal uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TermRenderer {
    /// The DOM renderer, and the default.
    ///
    /// Measured, not assumed: under WebKitGTK the WebGL path costs a steady
    /// 0.244% idle CPU and roughly 80 MB more resident, because the compositor
    /// keeps the GL layer awake with nothing on screen changing. It is also
    /// marginally SLOWER on the corpus, 71 MB/s against 73 MB/s, for a
    /// workload that peaks near 0.4 MB/s. Idle cost is the number this product
    /// is sold on, so the default is the one that is idle.
    #[default]
    Dom,
    /// The GPU renderer. Offered because a machine with a cheap compositor may
    /// genuinely prefer it, and disclosed with its idle cost in the settings
    /// row rather than presented as the fast option.
    Webgl,
}

/// How a backdrop image is fitted to the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackdropFit {
    /// Fill the window, cropping the overflow. The default, because a
    /// wallpaper picked for a screen is nearly always meant to fill it.
    #[default]
    Cover,
    /// Fit the whole image, letterboxing the remainder.
    Contain,
    /// Repeat at native size. The one that suits a small texture.
    Tile,
    /// Native size, centred, no repeat.
    Center,
}

impl BackdropFit {
    /// The `background-size` and `background-repeat` pair for this fit.
    #[must_use]
    pub fn css(self) -> (&'static str, &'static str) {
        match self {
            BackdropFit::Cover => ("cover", "no-repeat"),
            BackdropFit::Contain => ("contain", "no-repeat"),
            BackdropFit::Tile => ("auto", "repeat"),
            BackdropFit::Center => ("auto", "no-repeat"),
        }
    }
}

/// Below this the window is too faint to read or to aim at.
///
/// A floor and not a suggestion. Opacity is the one appearance setting that
/// can hide the control that would undo it: at 0 the operator has an invisible
/// window, no settings modal to find, and no reason to suspect the config file.
pub const OPACITY_MIN_PCT: u8 = 20;
/// Fully opaque, and the default.
pub const OPACITY_MAX_PCT: u8 = 100;
/// The widest blur worth offering. Past this the image is a flat colour and
/// the GPU is doing the work of producing one.
pub const BACKDROP_BLUR_MAX_PX: u8 = 64;

/// Window translucency and the backdrop image behind the interface.
///
/// Separate from [`ThemePref`] because they answer different questions. A
/// theme is which palette to paint; this is how much of the desktop shows
/// through it, and an operator who rices their desktop wants the second
/// without giving up the first.
///
/// Chrome and terminal carry their own opacity. The common arrangement is an
/// opaque shell with a translucent grid, so the wallpaper reads behind the
/// text and the sidebar stays legible; one shared number cannot express it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppearancePrefs {
    /// Opacity of the window chrome, percent.
    pub opacity_pct: u8,
    /// Opacity of the terminal grid, percent, independent of the chrome.
    pub terminal_opacity_pct: u8,
    /// Absolute path to a backdrop image. Empty means none.
    pub backdrop: String,
    pub backdrop_fit: BackdropFit,
    /// Gaussian blur over the backdrop, in CSS pixels.
    pub backdrop_blur_px: u8,
    /// A scrim between the backdrop and the interface, percent. This is what
    /// keeps text readable over a bright photograph, so it is offered beside
    /// the image rather than left for the operator to solve with opacity.
    pub backdrop_dim_pct: u8,
}

impl Default for AppearancePrefs {
    fn default() -> Self {
        AppearancePrefs {
            opacity_pct: OPACITY_MAX_PCT,
            terminal_opacity_pct: OPACITY_MAX_PCT,
            backdrop: String::new(),
            backdrop_fit: BackdropFit::default(),
            backdrop_blur_px: 0,
            backdrop_dim_pct: 0,
        }
    }
}

impl AppearancePrefs {
    /// True when anything here needs the window itself to be see-through.
    ///
    /// A backdrop image does NOT: it is painted inside the window, so it needs
    /// no help from the compositor. Only an opacity below 100 does, and the
    /// distinction matters because a transparent window is the part that
    /// depends on the desktop having a compositor at all.
    #[must_use]
    pub fn needs_transparent_window(&self) -> bool {
        self.opacity_pct < OPACITY_MAX_PCT || self.terminal_opacity_pct < OPACITY_MAX_PCT
    }

    /// Clamp every value into the range the interface can survive.
    ///
    /// Applied on load, not just at the controls. The sliders cannot produce
    /// an out-of-range value; a hand-edited `ui.json` is the path that
    /// actually produces an unusable window, and it does not go through them.
    pub fn clamp(&mut self) {
        self.opacity_pct = self.opacity_pct.clamp(OPACITY_MIN_PCT, OPACITY_MAX_PCT);
        self.terminal_opacity_pct = self
            .terminal_opacity_pct
            .clamp(OPACITY_MIN_PCT, OPACITY_MAX_PCT);
        self.backdrop_blur_px = self.backdrop_blur_px.min(BACKDROP_BLUR_MAX_PX);
        self.backdrop_dim_pct = self.backdrop_dim_pct.min(100);
    }
}

/// Terminal preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalPrefs {
    pub renderer: TermRenderer,
    /// Lines of scrollback the terminal keeps in the webview. Server-side
    /// history is unaffected and is where deep scrollback actually lives.
    ///
    /// 1000 matches what `bootstrap.js` mounts with, and is the number the
    /// 174.7 MB resident figure was measured at. The two have to agree or the
    /// first write from the settings modal silently multiplies the terminal's
    /// buffer without the operator touching the control.
    pub scrollback_lines: u32,
    /// CSS font stack, verbatim. Empty means "whatever `--rg-font-mono`
    /// resolves to", which is the one place the default stack is written down;
    /// copying it here would go stale the first time the stylesheet is retuned.
    pub font_family: String,
    pub font_size_px: u16,
    /// Colour palette for the grid.
    ///
    /// Independent of [`Settings::theme`] on purpose. The chrome's light/dark
    /// choice is about the room the operator is sitting in; the grid's palette
    /// is about the colours their prompt and their agent's ANSI output were
    /// tuned for, and those two answers are routinely different.
    pub palette: crate::termpalette::TermPalette,
}

impl Default for TerminalPrefs {
    fn default() -> Self {
        TerminalPrefs {
            renderer: TermRenderer::default(),
            scrollback_lines: 1_000,
            font_family: String::new(),
            font_size_px: 13,
            palette: crate::termpalette::TermPalette::default(),
        }
    }
}

/// Which events raise an OS notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotifyPrefs {
    /// A session's child exited.
    pub finished: bool,
    /// A session is blocked on the operator.
    pub needs_approval: bool,
    /// A session exited non-zero.
    pub failed: bool,
    /// Skip the notification when the session is the one on screen. Watching
    /// something finish and then being told it finished is noise.
    pub skip_focused_session: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        NotifyPrefs {
            finished: false,
            needs_approval: true,
            failed: true,
            skip_focused_session: true,
        }
    }
}

/// Rebound shortcuts.
///
/// Keyed by the action's wire name and valued by a chord string, both plain
/// text, so this file never has to agree with `keymap.rs` about a Rust type.
/// `keymap` parses and validates; an override it cannot parse is ignored and
/// the default binding stands, which is the only behaviour that cannot lock a
/// user out of their own keyboard.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KeyboardPrefs {
    pub overrides: BTreeMap<String, String>,
    /// Bindings the operator wrote: a chord, and an ordered list of steps to
    /// perform when it fires. Consulted before the built-in table, so a
    /// custom binding on a built-in chord shadows it.
    pub custom: crate::keymap::CustomBindings,
}

/// Smallest and largest UI text scale, in percent.
pub const TEXT_SCALE_MIN_PCT: u16 = 80;
/// Largest UI text scale, in percent.
pub const TEXT_SCALE_MAX_PCT: u16 = 200;

/// Application-global preferences.
///
/// Global, not per workspace: these are statements about how this operator
/// reads a list, and an operator who wants branches hidden wants them hidden
/// everywhere. The two genuinely context-dependent settings — how a workspace
/// buckets its rows and which bands it shows — live on [`super::Workspace`] instead,
/// because "this workspace is my review queue, show me settled work" is a fact
/// about the workspace and not about the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Draw `rg-session__branch` on rows that have a branch.
    pub show_branch: bool,
    /// Draw `rg-session__time`.
    pub show_time: bool,
    /// Draw `rg-pill__word`. Off leaves the icon, which is what the collapsed
    /// sidebar already renders, so the narrow layout is reachable at any width.
    pub show_status_word: bool,
    /// Force every sidebar row to the slim variant.
    ///
    /// Distinct from [`Density::Compact`], which shrinks both variants: this
    /// collapses the card rows to slim ones outright. At twenty agents
    /// "make the list dense" is a real thing to want, and it is a different
    /// want from "make everything smaller". Off by default, so nobody who
    /// never opens the settings sees a different sidebar.
    pub always_slim: bool,
    /// Require a confirmation before terminating a live child.
    pub confirm_terminate: bool,
    /// Force the reduced-motion path regardless of what the OS reports.
    pub reduce_motion: bool,
    pub density: Density,
    pub theme: ThemePref,
    /// UI text scale in percent, clamped to
    /// [`TEXT_SCALE_MIN_PCT`]..=[`TEXT_SCALE_MAX_PCT`] by
    /// [`Settings::set_text_scale`]. Separate from the terminal's own font
    /// size: an operator who wants a big terminal and a dense sidebar is the
    /// normal case, not an exotic one.
    pub text_scale_pct: u16,
    pub terminal: TerminalPrefs,
    pub appearance: AppearancePrefs,
    pub notifications: NotifyPrefs,
    pub keyboard: KeyboardPrefs,
    /// Daemon URL override. Empty means "use whatever the command line said",
    /// which keeps `--server` authoritative for the case it exists for.
    pub daemon_url: String,
    /// Auto-settle tuning. A setting because it is the one disposition rule
    /// with a number in it that an operator has an opinion about.
    pub policy: DispositionPolicy,
    /// Whether the operator has been past the first-run sheet. False on a
    /// fresh profile, and the only thing that opens onboarding.
    pub onboarded: bool,
    /// The version whose changelog was last shown, as it was written. Empty
    /// means never, which is first run and belongs to onboarding rather than
    /// to the release notes. A string and not a `Version` so a profile
    /// written by a build with a different scheme still loads.
    pub seen_version: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_branch: true,
            show_time: true,
            show_status_word: true,
            always_slim: false,
            confirm_terminate: true,
            reduce_motion: false,
            density: Density::default(),
            theme: ThemePref::default(),
            text_scale_pct: 100,
            terminal: TerminalPrefs::default(),
            appearance: AppearancePrefs::default(),
            notifications: NotifyPrefs::default(),
            keyboard: KeyboardPrefs::default(),
            daemon_url: String::new(),
            policy: DispositionPolicy::default(),
            onboarded: false,
            seen_version: String::new(),
        }
    }
}

impl Settings {
    /// Clamp and store a text scale.
    ///
    /// Clamped rather than validated at the control, because a scale read back
    /// from a hand-edited file is the case that actually produces an
    /// unreadable window, and that path does not go through a slider.
    pub fn set_text_scale(&mut self, pct: u16) {
        self.text_scale_pct = pct.clamp(TEXT_SCALE_MIN_PCT, TEXT_SCALE_MAX_PCT);
    }

    /// The daemon URL to dial, given whatever the command line asked for.
    pub fn resolved_daemon_url<'a>(&'a self, cli: &'a str) -> &'a str {
        if self.daemon_url.trim().is_empty() {
            cli
        } else {
            self.daemon_url.trim()
        }
    }

    /// The version whose release notes this profile has already seen, if any.
    ///
    /// Unparseable text reads as "never seen", which shows the notes once more
    /// rather than swallowing them. Showing a sheet twice is a smaller failure
    /// than never showing it.
    pub fn last_seen_version(&self) -> Option<semver::Version> {
        semver::Version::parse(self.seen_version.trim()).ok()
    }

    /// Record that the first-run sheet is done with, whichever control closed
    /// it, and that this version's notes count as read.
    ///
    /// Both at once: an operator who has just been walked through the app does
    /// not then want the release notes for the version they installed a minute
    /// ago.
    pub fn finish_onboarding(&mut self, current: &semver::Version) {
        self.onboarded = true;
        self.seen_version = current.to_string();
    }

    /// Record that the notes for `current` have been read.
    pub fn mark_seen(&mut self, current: &semver::Version) {
        self.seen_version = current.to_string();
    }
}

/// Which page of the settings modal is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SettingsTab {
    /// Theme, density, text scale.
    #[default]
    Appearance,
    /// What a sidebar row shows.
    Sidebar,
    /// The workspace list, and the selected workspace's grouping, bands and
    /// folders. One tab and not three: they are all facets of one object, and
    /// splitting them makes the operator hop between pages to answer one
    /// question.
    Workspaces,
    /// Saved commands: label, command, arguments, default working directory.
    Presets,
    Terminal,
    Notifications,
    Keyboard,
    /// Daemon URL and diagnostics.
    Advanced,
    /// Which version this is, and getting a newer one.
    ///
    /// Last because it is the tab an operator opens twice a year, and first
    /// in importance only when something is wrong, which is when they will
    /// look for it by name rather than by position.
    About,
}

impl SettingsTab {
    pub const ALL: [SettingsTab; 9] = [
        SettingsTab::Appearance,
        SettingsTab::Sidebar,
        SettingsTab::Workspaces,
        SettingsTab::Presets,
        SettingsTab::Terminal,
        SettingsTab::Notifications,
        SettingsTab::Keyboard,
        SettingsTab::Advanced,
        SettingsTab::About,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsTab::Appearance => "Appearance",
            SettingsTab::Sidebar => "Sidebar",
            SettingsTab::Workspaces => "Workspaces",
            SettingsTab::Presets => "Presets",
            SettingsTab::Terminal => "Terminal",
            SettingsTab::Notifications => "Notifications",
            SettingsTab::Keyboard => "Keyboard",
            SettingsTab::Advanced => "Advanced",
            SettingsTab::About => "About",
        }
    }
}
