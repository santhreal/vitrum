//! Every row of the settings surface, as data.
//!
//! The rows used to be written into the markup, one control at a time, and
//! the only way to check that the catalogue and the surface agreed was to read
//! the surface's own source back and scan it for string literals. A source
//! scan cannot see a row that is built by a loop, cannot see a control that
//! was moved into another file, and goes quiet rather than red when either
//! happens.
//!
//! So the rows are a list here, the surface is built by walking it, and
//! [`tests`] differences the list against [`crate::state::catalog::SETTINGS`]
//! at run time. A setting added to [`Settings`] and to no row fails, a row
//! naming a setting that does not exist fails, and a setting claimed twice
//! fails.
//!
//! # The two escape hatches, and why they are lists rather than judgement
//!
//! Not every setting can be a [`Row`]. Six are edited by a control with its
//! own behaviour: a path field with an Apply button, an import that reads a
//! file and can fail, a chord recorder, a URL that reconnects a socket. Three
//! are not preferences at all; they are what the product remembers about the
//! operator.
//!
//! Both are named in [`BESPOKE`] and [`NOT_A_PREFERENCE`], with the surface
//! that owns each one. That keeps the completeness check total: every
//! catalogued setting is in exactly one of the three lists, so there is no
//! quiet third state a new setting can fall into.

use vitrum_fmt::count;

use crate::state::{
    BLINK_MAX_MS, BLINK_MIN_MS, BackdropFit, CELL_WIDTH_MAX_PCT, CELL_WIDTH_MIN_PCT, CURSOR_SHAPES,
    Density, LINE_HEIGHT_MAX_PCT, LINE_HEIGHT_MIN_PCT, NOTICE_SECONDS_MAX, PRESENT_MODES,
    SCROLLBACK_MAX_LINES, SNOOZE_HOUR_MAX, SPLASH_AFTER_MAX_MS, Settings, SettingsTab,
    TERM_FONT_MAX_PX, TERM_FONT_MIN_PX, ThemePref, WHEEL_LINES_MAX,
};

use super::{
    BLUR_STEPS, CELL_WIDTH_STEPS, DIM_STEPS, FONT_STACKS, LINE_HEIGHT_STEPS, NOTIFY_KINDS,
    OPACITY_STEPS, SCROLLBACK_STEPS, TERM_FONT_STEPS, UI_SCALE_STEPS, WHEEL_STEPS, blur_label,
    notify_enabled, notify_label, notify_path, opacity_note, palette_note,
    set_notify_enabled, term_font_px,
};

/// What kind of widget a row draws, and how it reaches the document.
///
/// Function pointers rather than closures, so the whole table is a `const`
/// and the suite can walk it without building a window. Every setter takes
/// the whole [`Settings`] because several of them clamp a sibling field.
pub(crate) enum Control {
    /// One boolean.
    Switch {
        get: fn(&Settings) -> bool,
        set: fn(&mut Settings, bool),
    },
    /// A menu of `(value, label)` pairs.
    ///
    /// The value is the string the document would hold, which is what makes a
    /// stored value that matches no option detectable rather than silently
    /// rendered as the first entry.
    Choice {
        options: fn() -> Vec<(String, String)>,
        get: fn(&Settings) -> String,
        set: fn(&mut Settings, &str),
    },
}

/// One control, its caption, and the setting it edits.
pub(crate) struct Row {
    /// Dotted path into the persisted document.
    pub path: &'static str,
    pub label: &'static str,
    /// What the setting does, in the surface's own words.
    pub desc: &'static str,
    /// A sentence computed from the document in force, printed after `desc`.
    ///
    /// Four rows need one: the theme row names what the desktop currently
    /// reports, the opacity row names which half of its effect is live, and
    /// the two palette rows name where the colours came from. A caption that
    /// cannot see the document would have to state the general case, which is
    /// how a control ends up telling an operator something that is not true of
    /// their machine.
    pub live_desc: Option<fn(&Settings) -> String>,
    /// Whether the row is drawn at all, given the document in force.
    ///
    /// A blink period with the blink switched off is a control with no
    /// meaning, and a backdrop fit with no backdrop is worse: it implies an
    /// image is being drawn.
    pub visible: Option<fn(&Settings) -> bool>,
    pub control: Control,
}

impl Row {
    /// The caption under the control, with the timing sentence appended.
    ///
    /// One function so no page can print a description and forget the
    /// sentence. [`super::when_note`] reads the catalogue, so a row cannot
    /// claim to be live while the catalogue calls it a restart.
    pub(crate) fn caption(&self, settings: &Settings) -> String {
        let mut out = self.desc.to_string();
        if let Some(f) = self.live_desc {
            let extra = f(settings);
            if !extra.is_empty() {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(&extra);
            }
        }
        out
    }

    /// Is this row drawn against `settings`?
    pub(crate) fn is_visible(&self, settings: &Settings) -> bool {
        self.visible.is_none_or(|f| f(settings))
    }
}

/// The rows one page draws, in order.
pub(crate) fn rows(tab: SettingsTab) -> &'static [Row] {
    match tab {
        SettingsTab::Appearance => APPEARANCE,
        SettingsTab::Sidebar => SIDEBAR,
        SettingsTab::Terminal => TERMINAL,
        SettingsTab::Notifications => NOTIFICATIONS,
        SettingsTab::About => ABOUT,
        SettingsTab::Advanced => ADVANCED,
        SettingsTab::Workspaces | SettingsTab::Presets | SettingsTab::Keyboard => &[],
    }
}

/// Every declared row, whatever page it is on.
#[cfg(test)]
pub(crate) fn all_rows() -> impl Iterator<Item = &'static Row> {
    SettingsTab::ALL.into_iter().flat_map(rows)
}

/// Settings edited by a control that is not a [`Row`], and what owns each.
///
/// A pair rather than a bare path, because "which surface edits this" is the
/// question a reader has when they cannot find a row for a setting, and an
/// unexplained exemption list answers it with silence.
#[cfg(test)]
pub(crate) const BESPOKE: &[(&str, &str)] = &[
    (
        "appearance.backdrop",
        "Appearance: a path field with Apply and Clear, because a partial path typed one \
         character at a time would try to read a file per keystroke.",
    ),
    (
        "terminal.followHostTerminal",
        "Terminal: turning it on runs a scan of this machine that can fail, and a failure has \
         to reach the operator as a sentence rather than as a switch that springs back.",
    ),
    (
        "terminal.hostPalette",
        "Terminal: written by the scan, by the named-file import, and by the rescan button. \
         Twenty colours are not a menu.",
    ),
    (
        "keyboard.overrides",
        "Keyboard: the chord recorder, which refuses a chord the live table already matches.",
    ),
    (
        "keyboard.custom",
        "Keyboard: the binding editor, which is a list of ordered steps and conditionals.",
    ),
    (
        "daemonUrl",
        "Advanced: a URL field whose Save button also redials the socket, so committing it and \
         reconnecting cannot come apart.",
    ),
];

/// Settings that persist and are not preferences, with what writes each.
///
/// The catalogue says the same thing in prose. It is repeated as data here so
/// the completeness check can be total: without these three, every run of the
/// check would have to carry an unexplained shortfall.
#[cfg(test)]
pub(crate) const NOT_A_PREFERENCE: &[(&str, &str)] = &[
    (
        "onboarded",
        "Written by the first-run sheet when it is finished with, however it was closed.",
    ),
    (
        "seenVersion",
        "Written by the release-notes sheet and by the end of onboarding.",
    ),
    (
        "ignoredUpdate",
        "Written by the titlebar chip when an available release is dismissed.",
    ),
];

// ═══════════════════════════════════════════════════════════════════════════
// Appearance
// ═══════════════════════════════════════════════════════════════════════════

/// What the desktop says about its own appearance, for the theme row.
fn system_note(_: &Settings) -> String {
    match super::system_theme() {
        Some(vitrum_os::theme::Theme::Dark) => "The desktop currently reports dark.".to_string(),
        Some(vitrum_os::theme::Theme::Light) => "The desktop currently reports light.".to_string(),
        None => "This desktop does not expose an appearance setting, so System paints dark."
            .to_string(),
    }
}

/// Notice and flash lifetimes offered, in seconds.
///
/// Zero is offered first because "stays until I dismiss it" is a position an
/// operator holds, not a degenerate value. Every other step is a whole number
/// of seconds, because the strip counts down in seconds.
const NOTICE_STEPS: &[(u8, &str)] = &[
    (0, "Until dismissed"),
    (3, "3 seconds"),
    (6, "6 seconds \u{2014} the default"),
    (10, "10 seconds"),
    (20, "20 seconds"),
    (45, "45 seconds"),
];

/// Delays offered before the boot mark is drawn, in milliseconds.
///
/// The shipped 120 ms is the shortest delay a start can lose to and still
/// have the mark read as deliberate rather than as a flicker.
const SPLASH_STEPS: &[u16] = &[0, 60, 120, 250, 500, 1_000, 2_000];

fn notice_options() -> Vec<(String, String)> {
    NOTICE_STEPS
        .iter()
        .map(|(secs, label)| (secs.to_string(), (*label).to_string()))
        .collect()
}

fn opacity_options() -> Vec<(String, String)> {
    OPACITY_STEPS
        .iter()
        .map(|p| (p.to_string(), format!("{p}%")))
        .collect()
}

fn has_backdrop(s: &Settings) -> bool {
    !s.appearance.backdrop.is_empty()
}

const APPEARANCE: &[Row] = &[
    Row {
        path: "theme",
        label: "Theme",
        desc: "Which palette the interface paints.",
        live_desc: Some(system_note),
        visible: None,
        control: Control::Choice {
            options: || {
                vec![
                    ("system".to_string(), "Follow the system".to_string()),
                    ("dark".to_string(), "Dark".to_string()),
                    ("light".to_string(), "Light".to_string()),
                ]
            },
            get: |s| {
                match s.theme {
                    ThemePref::System => "system",
                    ThemePref::Light => "light",
                    ThemePref::Dark => "dark",
                }
                .to_string()
            },
            set: |s, v| {
                s.theme = match v {
                    "light" => ThemePref::Light,
                    "dark" => ThemePref::Dark,
                    _ => ThemePref::System,
                };
            },
        },
    },
    Row {
        path: "density",
        label: "Density",
        desc: "Row heights and the spacing inside them. Text size is the next control; the two \
               are separate so a dense list can still have readable type.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                vec![
                    ("comfortable".to_string(), "Comfortable".to_string()),
                    ("compact".to_string(), "Compact".to_string()),
                ]
            },
            get: |s| {
                match s.density {
                    Density::Comfortable => "comfortable",
                    Density::Compact => "compact",
                }
                .to_string()
            },
            set: |s, v| {
                s.density = if v == "compact" {
                    Density::Compact
                } else {
                    Density::Comfortable
                };
            },
        },
    },
    Row {
        path: "textScalePct",
        label: "Text scale",
        desc: "Scales the whole shell, not just type: every size in the stylesheet is derived \
               from this. Composes on top of the display's own scaling.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                UI_SCALE_STEPS
                    .iter()
                    .map(|pct| (pct.to_string(), format!("{pct}%")))
                    .collect()
            },
            get: |s| s.text_scale_pct.to_string(),
            set: |s, v| {
                if let Ok(pct) = v.parse::<u16>() {
                    s.set_text_scale(pct);
                }
            },
        },
    },
    Row {
        path: "reduceMotion",
        label: "Reduce motion",
        desc: "Drops every transition. The stylesheet already honours the OS preference; this \
               forces it on regardless.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.reduce_motion,
            set: |s, on| s.reduce_motion = on,
        },
    },
    Row {
        path: "appearance.opacityPct",
        label: "Window opacity",
        desc: "",
        live_desc: Some(|s| opacity_note(&s.appearance).to_string()),
        visible: None,
        control: Control::Choice {
            options: opacity_options,
            get: |s| s.appearance.opacity_pct.to_string(),
            set: |s, v| {
                if let Ok(pct) = v.parse::<u8>() {
                    s.appearance.opacity_pct = pct;
                    s.appearance.clamp();
                }
            },
        },
    },
    Row {
        path: "appearance.terminalOpacityPct",
        label: "Terminal opacity",
        desc: "The grid alone, so the shell can stay solid while the wallpaper reads behind the \
               text. Below 100% the terminal composites every cell instead of filling runs of \
               them, which costs a little more per repaint.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: opacity_options,
            get: |s| s.appearance.terminal_opacity_pct.to_string(),
            set: |s, v| {
                if let Ok(pct) = v.parse::<u8>() {
                    s.appearance.terminal_opacity_pct = pct;
                    s.appearance.clamp();
                }
            },
        },
    },
    Row {
        path: "appearance.backdropFit",
        label: "Backdrop fit",
        desc: "How the image is sized to the window.",
        live_desc: None,
        visible: Some(has_backdrop),
        control: Control::Choice {
            options: || {
                vec![
                    ("cover".to_string(), "Fill the window".to_string()),
                    ("contain".to_string(), "Fit the whole image".to_string()),
                    ("tile".to_string(), "Tile".to_string()),
                    ("center".to_string(), "Centre at native size".to_string()),
                ]
            },
            get: |s| {
                match s.appearance.backdrop_fit {
                    BackdropFit::Cover => "cover",
                    BackdropFit::Contain => "contain",
                    BackdropFit::Tile => "tile",
                    BackdropFit::Center => "center",
                }
                .to_string()
            },
            set: |s, v| {
                s.appearance.backdrop_fit = match v {
                    "contain" => BackdropFit::Contain,
                    "tile" => BackdropFit::Tile,
                    "center" => BackdropFit::Center,
                    _ => BackdropFit::Cover,
                };
            },
        },
    },
    Row {
        path: "appearance.backdropBlurPx",
        label: "Backdrop blur",
        desc: "Blurred once, when the image loads, not per frame. A wide radius on a large \
               photograph is the one setting here that costs memory.",
        live_desc: None,
        visible: Some(has_backdrop),
        control: Control::Choice {
            options: || {
                BLUR_STEPS
                    .iter()
                    .map(|p| (p.to_string(), blur_label(*p)))
                    .collect()
            },
            get: |s| s.appearance.backdrop_blur_px.to_string(),
            set: |s, v| {
                if let Ok(px) = v.parse::<u8>() {
                    s.appearance.backdrop_blur_px = px;
                    s.appearance.clamp();
                }
            },
        },
    },
    Row {
        path: "appearance.backdropDimPct",
        label: "Backdrop dim",
        desc: "A scrim between the image and the interface. This is the control that keeps text \
               readable over a bright photograph.",
        live_desc: None,
        visible: Some(has_backdrop),
        control: Control::Choice {
            options: || {
                DIM_STEPS
                    .iter()
                    .map(|p| (p.to_string(), format!("{p}%")))
                    .collect()
            },
            get: |s| s.appearance.backdrop_dim_pct.to_string(),
            set: |s, v| {
                if let Ok(pct) = v.parse::<u8>() {
                    s.appearance.backdrop_dim_pct = pct;
                    s.appearance.clamp();
                }
            },
        },
    },
    Row {
        path: "notices.flashSeconds",
        label: "Flash messages",
        desc: "How long a one-line result stays up: a rename that failed, a file that could not \
               be written. Dismissing one by hand always works.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: notice_options,
            get: |s| s.notices.flash_seconds.to_string(),
            set: |s, v| {
                if let Ok(secs) = v.parse::<u8>() {
                    s.notices.flash_seconds = secs.min(NOTICE_SECONDS_MAX);
                }
            },
        },
    },
    Row {
        path: "notices.noticeSeconds",
        label: "Notice strips",
        desc: "How long a notice above the pane stays up. Zero keeps it until it is dismissed, \
               and a dismissed notice does not come back for the same session.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: notice_options,
            get: |s| s.notices.notice_seconds.to_string(),
            set: |s, v| {
                if let Ok(secs) = v.parse::<u8>() {
                    s.notices.notice_seconds = secs.min(NOTICE_SECONDS_MAX);
                }
            },
        },
    },
    Row {
        path: "notices.showHistoryNotice",
        label: "Say when history was refused",
        desc: "The daemon answers an attach with what it kept. Off means a pane that got less \
               history than it asked for says nothing and simply starts where it starts.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.notices.show_history_notice,
            set: |s, on| s.notices.show_history_notice = on,
        },
    },
    Row {
        path: "notices.showStartupErrors",
        label: "Show a harness's startup output",
        desc: "What a harness prints before it draws its own interface. Off hides it, including \
               the part that says why it failed to start.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.notices.show_startup_errors,
            set: |s, on| s.notices.show_startup_errors = on,
        },
    },
    Row {
        path: "startup.showSplash",
        label: "Draw the boot mark",
        desc: "The mark on the window before the first frame. A start fast enough never reaches \
               the delay below and draws nothing either way.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.startup.show_splash,
            set: |s, on| s.startup.show_splash = on,
        },
    },
    Row {
        path: "startup.splashAfterMs",
        label: "Boot mark after",
        desc: "Milliseconds of process life before the mark is drawn. Raising it hides the mark \
               on more machines.",
        live_desc: None,
        visible: Some(|s| s.startup.show_splash),
        control: Control::Choice {
            options: || {
                SPLASH_STEPS
                    .iter()
                    .map(|ms| (ms.to_string(), format!("{ms} ms")))
                    .collect()
            },
            get: |s| s.startup.splash_after_ms.to_string(),
            set: |s, v| {
                if let Ok(ms) = v.parse::<u16>() {
                    s.startup.splash_after_ms = ms.min(SPLASH_AFTER_MAX_MS);
                }
            },
        },
    },
];

// ═══════════════════════════════════════════════════════════════════════════
// Sidebar
// ═══════════════════════════════════════════════════════════════════════════

/// Auto-settle windows offered, in milliseconds. `None` disables it.
///
/// The model's own default MUST be one of these. A menu whose stored value
/// matches no option silently displays the first one, so an install sitting at
/// the shipped seven-day window would have read "Never" while quietly settling
/// rows behind the operator.
const SETTLE_STEPS: &[(Option<u64>, &str)] = &[
    (None, "Never \u{2014} I drain the list by hand"),
    (Some(15 * 60_000), "After 15 minutes idle"),
    (Some(60 * 60_000), "After 1 hour idle"),
    (Some(4 * 60 * 60_000), "After 4 hours idle"),
    (Some(24 * 60 * 60_000), "After 24 hours idle"),
    (
        Some(vitrum_model::DispositionPolicy::DEFAULT_AUTO_SETTLE_MS),
        "After 7 days idle \u{2014} the default",
    ),
];

/// The settle menu, as `(value, label)`.
///
/// Public to the module so the suite can assert the shipped default is one of
/// the options without reaching into the table's shape.
pub(crate) fn settle_options() -> Vec<(String, String)> {
    SETTLE_STEPS
        .iter()
        .map(|(ms, label)| {
            (
                ms.map_or_else(|| "off".to_string(), |v| v.to_string()),
                (*label).to_string(),
            )
        })
        .collect()
}

/// Inbox cuts offered, in rows.
const PREVIEW_ROW_STEPS: &[u8] = &[3, 5, 8, 12, 20, 30];

/// Done-shelf cuts offered, in rows.
const SETTLED_ROW_STEPS: &[u8] = &[5, 10, 20, 30, 50];

/// Recents-band lengths offered, in rows.
const RECENT_ROW_STEPS: &[u8] = &[5, 8, 12, 20, 30];

/// Ranked-history sizes offered, in commands.
const HISTORY_STEPS: &[u16] = &[30, 60, 120, 250, 500, 1_000];

/// Tab-strip capacities offered.
const TAB_STEPS: &[u8] = &[4, 6, 8, 12, 16, 24, 32];

/// A menu of whole counts, labelled with `noun`.
fn count_options<T: Copy + Into<u64> + std::fmt::Display>(
    steps: &[T],
    noun: &str,
) -> Vec<(String, String)> {
    steps
        .iter()
        .map(|n| (n.to_string(), count::count_s((*n).into(), noun)))
        .collect()
}

/// Every hour of the day, as the snooze menus offer them.
///
/// The whole clock rather than a step list, because "wake me at seven" is a
/// time an operator names rather than one they pick from a scale, and a menu
/// that omitted an hour would make the setting unreachable from the sheet
/// while the file still accepted it.
fn hour_options() -> Vec<(String, String)> {
    (0..=SNOOZE_HOUR_MAX)
        .map(|h| (h.to_string(), format!("{h:02}:00")))
        .collect()
}

const SIDEBAR: &[Row] = &[
    Row {
        path: "showBranch",
        label: "Show the git branch",
        desc: "The branch chip on rows whose directory is a checkout.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.show_branch,
            set: |s, on| s.show_branch = on,
        },
    },
    Row {
        path: "showPlace",
        label: "Show the working directory",
        desc: "The part of a session's directory the project header does not already say. A row \
               at the project root shows nothing while its branch is speaking, and shows its \
               directory when there is no branch either. A session an agent moved, or one in a \
               worktree beside the project, always shows where it is.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.show_place,
            set: |s, on| s.show_place = on,
        },
    },
    Row {
        path: "showWorktree",
        label: "Show the worktree",
        desc: "The worktree chip on rows running in a linked worktree of the project. A session \
               in the main working tree shows nothing, because that is the case the project \
               header already covers.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.show_worktree,
            set: |s, on| s.show_worktree = on,
        },
    },
    Row {
        path: "showStatusBar",
        label: "Show the status bar",
        desc: "The strip under the pane: the focused session's directory, its branch and \
               worktree, and what it is waiting on. Off gives the row back to the pane.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.show_status_bar,
            set: |s, on| s.show_status_bar = on,
        },
    },
    Row {
        path: "showTime",
        label: "Show the last-activity time",
        desc: "The relative age at the right of each row.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.show_time,
            set: |s, on| s.show_time = on,
        },
    },
    Row {
        path: "showStatusWord",
        label: "Show the status word",
        desc: "Off leaves the pill's colour, which is what the collapsed sidebar already \
               renders, so a narrow list stays readable. The state stays on the row for a \
               screen reader either way.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.show_status_word,
            set: |s, on| s.show_status_word = on,
        },
    },
    Row {
        path: "alwaysSlim",
        label: "Dense rows",
        desc: "Collapses every row to the slim variant, including the inbox, which normally gets \
               the taller card. Different from Compact density: that shrinks both variants, this \
               removes one of them.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.always_slim,
            set: |s, on| s.always_slim = on,
        },
    },
    Row {
        path: "confirmTerminate",
        label: "Confirm before terminating",
        desc: "Terminating kills the agent's child process. There is no undo.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.confirm_terminate,
            set: |s, on| s.confirm_terminate = on,
        },
    },
    Row {
        path: "policy",
        label: "Settle idle sessions automatically",
        desc: "A settled session drops out of the inbox into the Settled band. This is the only \
               disposition rule with a number in it, and it governs sections, rollups and the \
               attention jump keys as well as the list.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: settle_options,
            get: |s| {
                s.policy
                    .auto_settle_after_ms
                    .map_or_else(|| "off".to_string(), |ms| ms.to_string())
            },
            set: |s, v| s.policy.auto_settle_after_ms = v.parse::<u64>().ok(),
        },
    },
    Row {
        path: "inbox.previewRows",
        label: "Inbox rows before the rest",
        desc: "How many rows a bucket's inbox draws before it offers the remainder behind one \
               affordance. The focused row is drawn whether or not the cut reaches it, so this \
               is a floor on what is shown rather than an exact count.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(PREVIEW_ROW_STEPS, "row"),
            get: |s| s.inbox.preview_rows.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.inbox.preview_rows = n;
                }
            },
        },
    },
    Row {
        path: "inbox.settledRows",
        label: "Done rows before the rest",
        desc: "The same cut for the Settled band. It is separate because the Done shelf is a \
               record rather than a queue, and a long one costs nothing to leave collapsed.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(SETTLED_ROW_STEPS, "row"),
            get: |s| s.inbox.settled_rows.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.inbox.settled_rows = n;
                }
            },
        },
    },
    Row {
        path: "maxTabs",
        label: "Tabs the strip holds",
        desc: "Past this the least recently used tab is evicted. Eviction closes nothing: the \
               child keeps running, the sidebar row stays, and selecting it opens a tab again.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(TAB_STEPS, "tab"),
            get: |s| s.max_tabs.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.max_tabs = n;
                }
            },
        },
    },
    Row {
        path: "launcher.recentRows",
        label: "Recent commands the launcher lists",
        desc: "The recents band at the top of the launcher. Presets are listed in full whatever \
               this says; only the recents are cut.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(RECENT_ROW_STEPS, "row"),
            get: |s| s.launcher.recent_rows.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.launcher.recent_rows = n;
                }
            },
        },
    },
    Row {
        path: "launcher.historyLimit",
        label: "Commands the launcher remembers",
        desc: "What completion draws on. A save trims to this by rank, so lowering it drops what \
               was least worth suggesting rather than what is oldest.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(HISTORY_STEPS, "command"),
            get: |s| s.launcher.history_limit.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.launcher.history_limit = n;
                }
            },
        },
    },
    Row {
        path: "snooze.morningHour",
        label: "Morning snooze wakes at",
        desc: "When \u{201c}tomorrow morning\u{201d} and \u{201c}next week\u{201d} bring a row \
               back. A snoozed row is out of the inbox until then and is not counted in the \
               attention jumps.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: hour_options,
            get: |s| s.snooze.morning_hour.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.snooze.morning_hour = n;
                }
            },
        },
    },
    Row {
        path: "snooze.eveningHour",
        label: "Evening snooze wakes at",
        desc: "When \u{201c}this evening\u{201d} brings a row back. Set behind the current hour \
               it is the next day's evening, because a preset that woke a row immediately would \
               be a snooze that does nothing.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: hour_options,
            get: |s| s.snooze.evening_hour.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.snooze.evening_hour = n;
                }
            },
        },
    },
];

// ═══════════════════════════════════════════════════════════════════════════
// Terminal
// ═══════════════════════════════════════════════════════════════════════════

const TERMINAL: &[Row] = &[
    Row {
        path: "terminal.palette",
        label: "Colours",
        desc: "",
        live_desc: Some(|s| palette_note(s.terminal.palette)),
        visible: None,
        control: Control::Choice {
            options: || {
                crate::termpalette::ALL
                    .iter()
                    .map(|p| (p.slug().to_string(), p.label().to_string()))
                    .collect()
            },
            get: |s| s.terminal.palette.slug().to_string(),
            set: |s, v| s.terminal.palette = crate::termpalette::TermPalette::from_slug(v),
        },
    },
    Row {
        path: "terminal.fontFamily",
        label: "Font",
        desc: "Every choice ends in the generic monospace, so a font this machine does not have \
               falls back to another monospace rather than to a proportional face.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                FONT_STACKS
                    .iter()
                    .map(|(label, stack)| ((*stack).to_string(), (*label).to_string()))
                    .collect()
            },
            get: |s| s.terminal.font_family.clone(),
            set: |s, v| s.terminal.font_family = v.to_string(),
        },
    },
    Row {
        path: "terminal.fontSizePx",
        label: "Font size",
        desc: "Independent of the shell's text scale: a large terminal beside a dense sidebar is \
               the normal case.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                TERM_FONT_STEPS
                    .iter()
                    .map(|px| (px.to_string(), format!("{px} px")))
                    .collect()
            },
            get: |s| term_font_px(&s.terminal).to_string(),
            set: |s, v| {
                if let Ok(px) = v.parse::<u16>() {
                    s.terminal.font_size_px = px.clamp(TERM_FONT_MIN_PX, TERM_FONT_MAX_PX);
                }
            },
        },
    },
    Row {
        path: "terminal.scrollbackLines",
        label: "Scrollback",
        desc: "Sets the buffer here and the size of the first request an attach makes: 64 bytes \
               of the daemon's history per line, stopping at 2 MiB, about 32,000 lines' worth. \
               Scroll to the top of a pane to fetch older history, one step at a time, up to \
               8 MiB in one pane.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                SCROLLBACK_STEPS
                    .iter()
                    .map(|(lines, label)| (lines.to_string(), (*label).to_string()))
                    .collect()
            },
            get: |s| s.terminal.scrollback_lines.to_string(),
            set: |s, v| {
                if let Ok(lines) = v.parse::<u32>() {
                    s.terminal.scrollback_lines = lines.min(SCROLLBACK_MAX_LINES);
                }
            },
        },
    },
    Row {
        path: "terminal.lineHeightPct",
        label: "Line height",
        desc: "Percentage of the font's own line height. The cell grid is rebuilt on the next \
               frame, so a session keeps its content and reflows to the new row count.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                LINE_HEIGHT_STEPS
                    .iter()
                    .map(|pct| (pct.to_string(), format!("{pct}%")))
                    .collect()
            },
            get: |s| s.terminal.line_height_pct.to_string(),
            set: |s, v| {
                if let Ok(pct) = v.parse::<u16>() {
                    s.terminal.line_height_pct =
                        pct.clamp(LINE_HEIGHT_MIN_PCT, LINE_HEIGHT_MAX_PCT);
                }
            },
        },
    },
    Row {
        path: "terminal.cellWidthPct",
        label: "Cell width",
        desc: "Percentage of the font's own advance width. Under 100% a wide face fits more \
               columns in the same window and glyphs begin to touch.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                CELL_WIDTH_STEPS
                    .iter()
                    .map(|pct| (pct.to_string(), format!("{pct}%")))
                    .collect()
            },
            get: |s| s.terminal.cell_width_pct.to_string(),
            set: |s, v| {
                if let Ok(pct) = v.parse::<u16>() {
                    s.terminal.cell_width_pct = pct.clamp(CELL_WIDTH_MIN_PCT, CELL_WIDTH_MAX_PCT);
                }
            },
        },
    },
    Row {
        path: "terminal.cursorShape",
        label: "Cursor",
        desc: "The shape drawn where the session's cursor is. A program that sets its own shape \
               through an escape sequence overrides this for as long as it runs.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                CURSOR_SHAPES
                    .iter()
                    .map(|s| (s.slug().to_string(), s.label().to_string()))
                    .collect()
            },
            get: |s| s.terminal.cursor_shape.slug().to_string(),
            set: |s, v| {
                s.terminal.cursor_shape = CURSOR_SHAPES
                    .iter()
                    .copied()
                    .find(|shape| shape.slug() == v)
                    .unwrap_or_default();
            },
        },
    },
    Row {
        path: "terminal.cursorBlink",
        label: "Blink the cursor",
        desc: "Off holds the cursor solid. The blink is driven by the frame clock and wakes the \
               pane only while the window has focus.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.terminal.cursor_blink,
            set: |s, on| s.terminal.cursor_blink = on,
        },
    },
    Row {
        path: "terminal.blinkIntervalMs",
        label: "Blink period",
        desc: "One full cycle. The cursor is visible for half of it.",
        live_desc: None,
        visible: Some(|s| s.terminal.cursor_blink),
        control: Control::Choice {
            options: || {
                super::BLINK_STEPS
                    .iter()
                    .map(|ms| (ms.to_string(), format!("{ms} ms")))
                    .collect()
            },
            get: |s| s.terminal.blink_interval_ms.to_string(),
            set: |s, v| {
                if let Ok(ms) = v.parse::<u16>() {
                    s.terminal.blink_interval_ms = ms.clamp(BLINK_MIN_MS, BLINK_MAX_MS);
                }
            },
        },
    },
    Row {
        path: "terminal.wheelLines",
        label: "Wheel scrolls",
        desc: "Lines per notch. A program that reads mouse events gets the wheel instead, \
               unmodified by this.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                WHEEL_STEPS
                    .iter()
                    .map(|n| (n.to_string(), format!("{n} lines")))
                    .collect()
            },
            get: |s| s.terminal.wheel_lines.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse::<u8>() {
                    s.terminal.wheel_lines = n.clamp(1, WHEEL_LINES_MAX);
                }
            },
        },
    },
    Row {
        path: "terminal.bracketedPaste",
        label: "Bracketed paste",
        desc: "Wrap pasted text in the markers a program asked for, so it can tell a paste from \
               typing. Off refuses the markers even when the program enabled the mode, which is \
               what a program that mishandles them needs.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.terminal.bracketed_paste,
            set: |s, on| s.terminal.bracketed_paste = on,
        },
    },
    Row {
        path: "terminal.presentMode",
        label: "Frame pacing",
        desc: "How a finished frame reaches the display. An adapter that does not offer the \
               chosen mode falls back to the one it does, without changing this.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                PRESENT_MODES
                    .iter()
                    .map(|m| (m.slug().to_string(), m.label().to_string()))
                    .collect()
            },
            get: |s| s.terminal.present_mode.slug().to_string(),
            set: |s, v| {
                s.terminal.present_mode = PRESENT_MODES
                    .iter()
                    .copied()
                    .find(|m| m.slug() == v)
                    .unwrap_or_default();
            },
        },
    },
    Row {
        path: "search.contextLines",
        label: "Search context",
        desc: "Lines quoted either side of a hit in the search sheet. Each line is one row of \
               the hit, so a wide context trades hits on screen for lines around each of them.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(CONTEXT_STEPS, "line"),
            get: |s| s.search.context_lines.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.search.context_lines = n;
                }
            },
        },
    },
    Row {
        path: "search.maxHits",
        label: "Search hit cap",
        desc: "Hits one sweep returns before it reports the answer truncated. The daemon rations \
               this across the sessions in scope, so a chatty session cannot crowd out a quiet \
               one whatever the cap is.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(HIT_STEPS, "hit"),
            get: |s| s.search.max_hits.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.search.max_hits = n;
                }
            },
        },
    },
];

// ═══════════════════════════════════════════════════════════════════════════
// Notifications
// ═══════════════════════════════════════════════════════════════════════════

const NOTIFICATIONS: &[Row] = &[
    Row {
        path: notify_path(NOTIFY_KINDS[0]),
        label: notify_label(NOTIFY_KINDS[0]).0,
        desc: notify_label(NOTIFY_KINDS[0]).1,
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| notify_enabled(&s.notifications, NOTIFY_KINDS[0]),
            set: |s, on| set_notify_enabled(&mut s.notifications, NOTIFY_KINDS[0], on),
        },
    },
    Row {
        path: notify_path(NOTIFY_KINDS[1]),
        label: notify_label(NOTIFY_KINDS[1]).0,
        desc: notify_label(NOTIFY_KINDS[1]).1,
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| notify_enabled(&s.notifications, NOTIFY_KINDS[1]),
            set: |s, on| set_notify_enabled(&mut s.notifications, NOTIFY_KINDS[1], on),
        },
    },
    Row {
        path: notify_path(NOTIFY_KINDS[2]),
        label: notify_label(NOTIFY_KINDS[2]).0,
        desc: notify_label(NOTIFY_KINDS[2]).1,
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| notify_enabled(&s.notifications, NOTIFY_KINDS[2]),
            set: |s, on| set_notify_enabled(&mut s.notifications, NOTIFY_KINDS[2], on),
        },
    },
    Row {
        path: "notifications.skipFocusedSession",
        label: "Stay quiet about the session on screen",
        desc: "Watching an agent finish and then being told it finished is noise.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.notifications.skip_focused_session,
            set: |s, on| s.notifications.skip_focused_session = on,
        },
    },
];

// ═══════════════════════════════════════════════════════════════════════════
// About
// ═══════════════════════════════════════════════════════════════════════════

/// Every release stream, in the order the menu lists them.
const CHANNELS: [crate::update::Channel; 2] = [
    crate::update::Channel::Stable,
    crate::update::Channel::Nightly,
];

const ABOUT: &[Row] = &[
    Row {
        path: "updateChannel",
        label: "Release stream",
        desc: "Stable takes published releases. Nightly also takes the moving nightly build, and \
               still takes a stable release when one of them is newer. Neither installs a \
               version older than the one running.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || {
                CHANNELS
                    .iter()
                    .map(|c| (c.as_str().to_string(), channel_label(*c).to_string()))
                    .collect()
            },
            get: |s| s.update_channel.as_str().to_string(),
            set: |s, v| {
                s.update_channel = CHANNELS
                    .iter()
                    .copied()
                    .find(|c| c.as_str() == v)
                    .unwrap_or_default();
            },
        },
    },
    Row {
        path: "showRestartToUpdate",
        label: "Offer the restart in the sidebar",
        desc: "A staged update is applied on the next start whatever this says. Off hides the \
               band that offers to restart now, and hides nothing else: checking, staging and \
               applying are unaffected, and so is this page.",
        live_desc: None,
        visible: None,
        control: Control::Switch {
            get: |s| s.show_restart_to_update,
            set: |s, on| s.show_restart_to_update = on,
        },
    },
    Row {
        path: "updateCheckHours",
        label: "Check for updates every",
        desc: "How often a window that stays open asks about a new release. A launch checks as \
               well, so this is the interval for a long-lived window and nothing else about the \
               check varies with it.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(UPDATE_HOURS_STEPS, "hour"),
            get: |s| s.update_check_hours.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.update_check_hours = n;
                }
            },
        },
    },
];

// ═══════════════════════════════════════════════════════════════════════════
// Advanced
// ═══════════════════════════════════════════════════════════════════════════

/// Search context budgets offered, in lines either side of a hit.
const CONTEXT_STEPS: &[u16] = &[0, 1, 2, 3, 5, 8, 16];

/// Search hit caps offered.
const HIT_STEPS: &[u32] = &[100, 250, 500, 1_000, 2_500, 5_000];

/// Update-check intervals offered, in hours.
const UPDATE_HOURS_STEPS: &[u8] = &[1, 4, 12, 24, 72, 168];

/// Reconnect ceilings offered, in milliseconds.
const RECONNECT_MAX_STEPS: &[u32] = &[5_000, 10_000, 30_000, 60_000, 300_000, 600_000];

/// Reconnect attempt counts offered.
const RECONNECT_ATTEMPT_STEPS: &[u32] = &[5, 10, 25, 50, 100, 200];

/// The reconnect-ceiling menu, labelled in seconds.
///
/// Seconds rather than milliseconds because the stored unit is what the file
/// holds, not what a person waits in. The value written is still the
/// millisecond count the document takes.
fn reconnect_max_options() -> Vec<(String, String)> {
    RECONNECT_MAX_STEPS
        .iter()
        .map(|ms| (ms.to_string(), count::count_s(u64::from(ms / 1_000), "second")))
        .collect()
}

const ADVANCED: &[Row] = &[
    Row {
        path: "connection.reconnectMaxMs",
        label: "Longest gap between reconnect attempts",
        desc: "The schedule doubles from a quarter of a second and then holds here. A short \
               ceiling reaches a daemon that came back sooner, at the cost of dialling a socket \
               nothing is listening on more often.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: reconnect_max_options,
            get: |s| s.connection.reconnect_max_ms.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.connection.reconnect_max_ms = n;
                }
            },
        },
    },
    Row {
        path: "connection.reconnectAttempts",
        label: "Reconnect attempts before giving up",
        desc: "After the last one the window says the daemon is gone and offers Retry. Nothing \
               is lost either way: sessions live in the daemon, and a reconnect finds them.",
        live_desc: None,
        visible: None,
        control: Control::Choice {
            options: || count_options(RECONNECT_ATTEMPT_STEPS, "attempt"),
            get: |s| s.connection.reconnect_attempts.to_string(),
            set: |s, v| {
                if let Ok(n) = v.parse() {
                    s.connection.reconnect_attempts = n;
                }
            },
        },
    },
];

/// What the menu calls a release stream.
///
/// Distinct from [`crate::update::Channel::as_str`], which is the word the
/// setting is stored and logged as. A stored word is not a menu entry: this
/// one says what picking it does.
const fn channel_label(channel: crate::update::Channel) -> &'static str {
    match channel {
        crate::update::Channel::Stable => "Stable releases",
        crate::update::Channel::Nightly => "Nightly builds and stable releases",
    }
}

#[cfg(test)]
mod tests;
