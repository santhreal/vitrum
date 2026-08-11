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

/// Reading the colours out of the host terminal's own configuration.
pub mod hostterm;
/// The bus that carries a settings change to the running pane and shell.
pub mod live;
/// Every setting this product has, as data.
pub mod catalog;

/// What the file format refuses, and what an operator's existing profile
/// survives.
#[cfg(test)]
mod persistence;

/// Smallest terminal font size the product will paint at.
///
/// A fact about the cell grid rather than about the preference: below this the
/// cell box rounds to zero width, the pane has nothing to divide the viewport
/// by, and it goes blank with nothing logged anywhere. Enforced on load by
/// [`TerminalPrefs::clamp`], because a text editor is not a control.
pub const TERM_FONT_MIN_PX: u16 = 8;
/// Largest terminal font size the product will paint at.
pub const TERM_FONT_MAX_PX: u16 = 32;
/// Deepest local scrollback the pane will keep.
///
/// The server owns real history. This is the viewport buffer that makes the
/// wheel work between repaints, and it is resident memory in this process, so
/// it has a ceiling that a hand-edited file cannot raise.
pub const SCROLLBACK_MAX_LINES: u32 = 200_000;

/// Where a session's cell cursor is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CursorShape {
    /// A filled cell.
    #[default]
    Block,
    /// A vertical bar at the left edge of the cell.
    Bar,
    /// A rule along the bottom of the cell.
    Underline,
}

impl CursorShape {
    /// The value persisted and compared in the control.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            CursorShape::Block => "block",
            CursorShape::Bar => "bar",
            CursorShape::Underline => "underline",
        }
    }

    /// What the control calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            CursorShape::Block => "Block",
            CursorShape::Bar => "Bar",
            CursorShape::Underline => "Underline",
        }
    }
}

/// Every cursor shape, in the order the control lists them.
pub const CURSOR_SHAPES: [CursorShape; 3] = [
    CursorShape::Block,
    CursorShape::Bar,
    CursorShape::Underline,
];

/// How the pane's swapchain hands finished frames to the compositor.
///
/// A real choice on this hardware and not a knob for its own sake. Vsync
/// bounds the frame rate at the panel's refresh and never tears. Adaptive
/// keeps the same bound but discards a frame that arrives late instead of
/// blocking the renderer on it, so a burst of output does not queue up
/// latency. Immediate presents the moment a frame is ready, which is the
/// lowest keystroke-to-glyph latency available and the one that can tear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PresentMode {
    /// `Fifo`. Always supported.
    #[default]
    Vsync,
    /// `Mailbox`. Offered only when the adapter reports it.
    Adaptive,
    /// `Immediate`. Offered only when the adapter reports it.
    Immediate,
}

impl PresentMode {
    /// The value persisted and compared in the control.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            PresentMode::Vsync => "vsync",
            PresentMode::Adaptive => "adaptive",
            PresentMode::Immediate => "immediate",
        }
    }

    /// What the control calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            PresentMode::Vsync => "Vsync",
            PresentMode::Adaptive => "Adaptive",
            PresentMode::Immediate => "Immediate",
        }
    }
}

/// Every present mode, in the order the control lists them.
pub const PRESENT_MODES: [PresentMode; 3] = [
    PresentMode::Vsync,
    PresentMode::Adaptive,
    PresentMode::Immediate,
];

/// Narrowest and widest cell box, as a percentage of the font's own advance.
pub const CELL_WIDTH_MIN_PCT: u16 = 80;
/// Widest cell box, as a percentage of the font's own advance.
pub const CELL_WIDTH_MAX_PCT: u16 = 140;
/// Tightest and loosest line box, as a percentage of the font's own height.
pub const LINE_HEIGHT_MIN_PCT: u16 = 80;
/// Loosest line box, as a percentage of the font's own height.
pub const LINE_HEIGHT_MAX_PCT: u16 = 200;
/// Fastest and slowest cursor blink period, in milliseconds.
pub const BLINK_MIN_MS: u16 = 100;
/// Slowest cursor blink period, in milliseconds.
pub const BLINK_MAX_MS: u16 = 2_000;
/// Most lines one wheel notch may scroll.
pub const WHEEL_LINES_MAX: u8 = 25;

/// Terminal preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TerminalPrefs {
    /// Lines of scrollback the pane keeps locally. Server-side history is
    /// unaffected and is where deep scrollback actually lives.
    pub scrollback_lines: u32,
    /// Font stack, verbatim. Empty means the platform's default monospace
    /// face, which is the one place the default stack is written down;
    /// copying it here would go stale the first time it is retuned.
    pub font_family: String,
    pub font_size_px: u16,
    /// Line box height as a percentage of the font's own line height.
    pub line_height_pct: u16,
    /// Cell box width as a percentage of the font's own advance width.
    pub cell_width_pct: u16,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    /// Cursor blink period, in milliseconds. Read only when
    /// [`TerminalPrefs::cursor_blink`] is on.
    pub blink_interval_ms: u16,
    /// Lines one wheel notch scrolls.
    pub wheel_lines: u8,
    /// Wrap pasted text in the bracketed-paste markers when the program asked
    /// for them. Off refuses the markers regardless, which is what a program
    /// that enables the mode and then mishandles it needs.
    pub bracketed_paste: bool,
    /// How the swapchain presents. Clamped to what the adapter reports.
    pub present_mode: PresentMode,
    /// Colour palette for the grid.
    ///
    /// Independent of [`Settings::theme`] on purpose. The chrome's light/dark
    /// choice is about the room the operator is sitting in; the grid's palette
    /// is about the colours their prompt and their agent's ANSI output were
    /// tuned for, and those two answers are routinely different.
    ///
    /// Ignored while [`TerminalPrefs::follow_host_terminal`] is on and
    /// [`TerminalPrefs::host_palette`] holds an import.
    pub palette: crate::termpalette::TermPalette,
    /// Paint with the colours read out of the host terminal's own
    /// configuration rather than with a built-in scheme.
    pub follow_host_terminal: bool,
    /// The colours the last import found, and where they came from. Persisted
    /// rather than re-detected each launch, so the grid does not change colour
    /// because a config file moved.
    pub host_palette: hostterm::HostPalette,
}

impl Default for TerminalPrefs {
    fn default() -> Self {
        TerminalPrefs {
            scrollback_lines: 1_000,
            font_family: String::new(),
            font_size_px: 13,
            line_height_pct: 100,
            cell_width_pct: 100,
            cursor_shape: CursorShape::default(),
            cursor_blink: true,
            blink_interval_ms: 530,
            wheel_lines: 3,
            bracketed_paste: true,
            present_mode: PresentMode::default(),
            palette: crate::termpalette::TermPalette::default(),
            follow_host_terminal: false,
            host_palette: hostterm::HostPalette::default(),
        }
    }
}

impl TerminalPrefs {
    /// Force every value into the range the pane can paint.
    ///
    /// Applied on load. A zero font size is a zero-width cell box and a blank
    /// pane; a zero blink period is a cursor strobing at the frame rate; a
    /// wheel notch of 200 lines makes the wheel unusable. None of those can
    /// come out of a control, and all of them can come out of a text editor.
    pub fn clamp(&mut self) {
        self.font_size_px = self.font_size_px.clamp(TERM_FONT_MIN_PX, TERM_FONT_MAX_PX);
        self.line_height_pct = self
            .line_height_pct
            .clamp(LINE_HEIGHT_MIN_PCT, LINE_HEIGHT_MAX_PCT);
        self.cell_width_pct = self
            .cell_width_pct
            .clamp(CELL_WIDTH_MIN_PCT, CELL_WIDTH_MAX_PCT);
        self.blink_interval_ms = self.blink_interval_ms.clamp(BLINK_MIN_MS, BLINK_MAX_MS);
        self.wheel_lines = self.wheel_lines.clamp(1, WHEEL_LINES_MAX);
        self.scrollback_lines = self.scrollback_lines.min(SCROLLBACK_MAX_LINES);
        self.host_palette.clamp();
    }

    /// Whether the host import is what the grid will actually paint with.
    ///
    /// Both halves matter. The switch alone is not enough: an operator can
    /// turn it on before any import has succeeded, and painting with an empty
    /// palette would blank the pane.
    #[must_use]
    pub fn host_palette_in_force(&self) -> bool {
        self.follow_host_terminal && self.host_palette.is_complete()
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

/// Transient strips: the flash line above the pane, and the notice the pane
/// raises when it cannot show what was asked for.
///
/// Its own group because the two operator complaints here are opposite. A
/// notice that will not dismiss is a defect; a notice that vanishes before it
/// is read is also a defect, and which one an operator hits depends on how
/// fast they read. So both durations are numbers, and zero means the strip
/// stays until it is dismissed rather than meaning the strip is off. Turning
/// a strip off is a separate switch, because those are separate decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NoticePrefs {
    /// Seconds a confirmation flash stays up. 0 keeps it until dismissed.
    pub flash_seconds: u8,
    /// Seconds a scrollback notice stays up. 0 keeps it until dismissed.
    pub notice_seconds: u8,
    /// Show the strip that says the pane is showing history rather than the
    /// live tail. Off leaves the scroll position alone and only removes the
    /// strip.
    pub show_history_notice: bool,
    /// Show a harness's startup diagnostics in the pane.
    ///
    /// On. A harness that fails to start prints why, and hiding that leaves a
    /// blank pane and no explanation. Off is for an operator whose harness
    /// prints a banner they have read a thousand times.
    pub show_startup_errors: bool,
}

impl Default for NoticePrefs {
    fn default() -> Self {
        NoticePrefs {
            flash_seconds: 6,
            notice_seconds: 0,
            show_history_notice: true,
            show_startup_errors: true,
        }
    }
}

impl NoticePrefs {
    /// How long a flash lives, or `None` when it waits for a dismissal.
    #[must_use]
    pub const fn flash_life_ms(&self) -> Option<u64> {
        match self.flash_seconds {
            0 => None,
            n => Some(n as u64 * 1_000),
        }
    }

    /// How long a notice lives, or `None` when it waits for a dismissal.
    #[must_use]
    pub const fn notice_life_ms(&self) -> Option<u64> {
        match self.notice_seconds {
            0 => None,
            n => Some(n as u64 * 1_000),
        }
    }

    /// Force both durations into the range the strip can express.
    pub fn clamp(&mut self) {
        self.flash_seconds = self.flash_seconds.min(NOTICE_SECONDS_MAX);
        self.notice_seconds = self.notice_seconds.min(NOTICE_SECONDS_MAX);
    }
}

/// Longest a transient strip may be pinned for, in seconds.
///
/// A minute. Past that the operator wanted a strip that waits for a
/// dismissal, which is what 0 already means.
pub const NOTICE_SECONDS_MAX: u8 = 60;

/// The boot surface: the mark drawn on the window before the first frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StartupPrefs {
    /// Draw the mark at all.
    pub show_splash: bool,
    /// Milliseconds of process life before the mark is drawn.
    ///
    /// A start that beats this shows no mark, which is the point: a mark that
    /// appears and disappears inside a tenth of a second reads as a rendering
    /// fault rather than as progress. Raising it hides the mark on more
    /// machines; lowering it shows it on more.
    pub splash_after_ms: u16,
}

impl Default for StartupPrefs {
    fn default() -> Self {
        StartupPrefs {
            show_splash: true,
            splash_after_ms: 120,
        }
    }
}

impl StartupPrefs {
    /// Force the delay into the range the boot surface can express.
    pub fn clamp(&mut self) {
        self.splash_after_ms = self.splash_after_ms.min(SPLASH_AFTER_MAX_MS);
    }
}

/// Longest the boot surface may stay blank before the mark is drawn.
///
/// Five seconds. Past that an operator has a window that looks broken, and
/// "never draw it" is already a separate switch.
pub const SPLASH_AFTER_MAX_MS: u16 = 5_000;

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
    /// Draw `rg-session__place`: the session's working directory, on the rows
    /// where it is not the project's own directory.
    ///
    /// On by default. It costs nothing on the common row, which sits at the
    /// project root and draws nothing, and it is the only thing that says a
    /// row is in a worktree or that an agent moved itself somewhere else.
    pub show_place: bool,
    /// Draw `rg-session__time`.
    pub show_time: bool,
    /// Draw `rg-pill__word`. Off leaves the icon, which is what the collapsed
    /// sidebar already renders, so the narrow layout is reachable at any width.
    pub show_status_word: bool,
    /// Draw the worktree label: which linked worktree of a repository a
    /// session is in, on the rows and in the status bar where it is not the
    /// repository's main working tree.
    ///
    /// On. A session in a worktree and a session in the main tree share a
    /// branch name often enough that the branch chip alone identifies
    /// neither, and the product ran for three releases with no way to tell
    /// them apart.
    pub show_worktree: bool,
    /// Draw the status bar under the pane.
    ///
    /// The bar carries the focused session's working directory, its branch,
    /// its worktree and the daemon connection. Each of those is separately
    /// hideable; this switch removes the row itself, which is the one an
    /// operator who wants the whole window to be grid reaches for.
    pub show_status_bar: bool,
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
    /// Flash and notice lifetimes, and which strips are drawn at all.
    pub notices: NoticePrefs,
    /// The boot surface.
    pub startup: StartupPrefs,
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
    /// Newest release the operator dismissed from the titlebar chip, as a
    /// version string. Empty means nothing has been dismissed. Matching the
    /// quiet check's answer against this is what keeps a "not now" from
    /// coming back every launch, without hiding a later release.
    pub ignored_update: String,
    /// Draw the sidebar's restart-to-update affordance.
    ///
    /// Cosmetic, and cosmetic only. Off hides the band saying a restart will
    /// take the staged build, and hides nothing else: the client keeps
    /// checking, keeps staging a verified update and keeps applying it on the
    /// next start. An operator who does not want a badge has not asked to be
    /// left on an old build, and `vitrum update` and the About tab are
    /// unaffected. [`crate::update::restart_offer`] is the only reader.
    pub show_restart_to_update: bool,
    /// Which stream of releases this profile follows.
    ///
    /// Stable resolves the latest published release. Nightly also resolves the
    /// moving `nightly` prerelease, and still takes a stable release when one
    /// is newer than the nightly. Verification does not vary by channel.
    pub update_channel: crate::update::Channel,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_branch: true,
            show_place: true,
            show_time: true,
            show_status_word: true,
            show_worktree: true,
            show_status_bar: true,
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
            notices: NoticePrefs::default(),
            startup: StartupPrefs::default(),
            daemon_url: String::new(),
            policy: DispositionPolicy::default(),
            onboarded: false,
            seen_version: String::new(),
            ignored_update: String::new(),
            show_restart_to_update: true,
            update_channel: crate::update::Channel::default(),
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

    /// Force every value in the document into the range the product can
    /// render.
    ///
    /// One entry point, called by the load path. Each group owns its own
    /// ranges; what this adds is that no group can be forgotten, because
    /// forgetting one is invisible until an operator hand-edits exactly that
    /// group and the window comes up unusable.
    pub fn clamp(&mut self) {
        self.text_scale_pct = self
            .text_scale_pct
            .clamp(TEXT_SCALE_MIN_PCT, TEXT_SCALE_MAX_PCT);
        self.appearance.clamp();
        self.terminal.clamp();
        self.notices.clamp();
        self.startup.clamp();
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

    /// Record that the operator dismissed an available update of this version.
    ///
    /// The quiet titlebar check compares this string against a ready release
    /// via [`crate::update::chrome_offer`]. Keeping the rule in one place is
    /// why there is no separate `is_ignored` predicate on `Settings`.
    pub fn ignore_update(&mut self, version: &semver::Version) {
        self.ignored_update = version.to_string();
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
