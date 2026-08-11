//! Every setting this product has, as data.
//!
//! One list, three consumers. Every row in the settings sheet declares its
//! path and prints [`Live::note`] under its caption, so a control cannot tell
//! an operator it is live while this table calls it a restart. The manual is
//! checked against the list, so a shipped setting cannot go undocumented. And
//! the persistence suite iterates it, so a setting that nobody wired into the
//! file turns the suite red.
//!
//! # Why the list is checked against the source rather than trusted
//!
//! A hand-kept table of fields goes stale the first time somebody adds a
//! field, and it goes stale silently, which is the same failure as having no
//! table. So [`declared_paths`] parses the settings source itself and the
//! suite asserts the two agree. Adding a field to [`super::Settings`] and
//! stopping there fails; adding a row here for a field that does not exist
//! fails too.
//!
//! # What is deliberately not a row
//!
//! Three fields persist and are not preferences: `onboarded`, `seenVersion`
//! and `ignoredUpdate` are what the product remembers about the operator
//! rather than what the operator told it. They are in the table because they
//! are in the file and the file is what the suite checks; their `applies`
//! column says where they are written from, and no control edits them
//! directly.

#[cfg(test)]
use std::collections::{BTreeMap, BTreeSet};

/// When a change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    /// Before the sheet closes.
    Immediately,
    /// In windows opened after the change. The window's translucency is fixed
    /// when the window is created.
    NewWindow,
    /// The next time the program starts.
    NextLaunch,
}

impl Live {
    /// The sentence the row prints under its description.
    ///
    /// Every row prints one. A setting that applies immediately says so, which
    /// is what makes the absence of the sentence on a restart-only row a
    /// visible defect rather than an assumption the reader has to make.
    #[must_use]
    pub const fn note(self) -> &'static str {
        match self {
            Live::Immediately => "Applies immediately.",
            Live::NewWindow => "Applies to windows opened after this change.",
            Live::NextLaunch => "Applies the next time vitrum starts.",
        }
    }

    /// The word the generated table uses.
    ///
    /// The table is a coherence device rather than a shipped surface: the
    /// manual carries curated per-group tables, and this one exists so the
    /// suite can assert a row per setting against the same data the sheet
    /// reads. Nothing in a running window renders it.
    #[cfg(test)]
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Live::Immediately => "live",
            Live::NewWindow => "new window",
            Live::NextLaunch => "restart",
        }
    }
}

/// One setting, as the file stores it and as the sheet describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// Dotted path into the persisted document, in the names the file uses.
    pub path: &'static str,
    /// The type, and the range where a range is enforced.
    pub kind: &'static str,
    /// The value a fresh profile has, written the way the file writes it.
    pub default: &'static str,
    /// The surface the value changes.
    pub applies: &'static str,
    pub live: Live,
    /// A legal value that is not the default, as a JSON literal.
    ///
    /// The round-trip suite writes this into the document at `path` and
    /// asserts it comes back unchanged. It has to be a value the clamps
    /// accept, because a value that is clamped on load round-trips to the
    /// clamped value and would report a persistence failure that is not one.
    pub alt: &'static str,
    /// What the setting does, in the sheet's own words.
    pub description: &'static str,
}

/// Every setting, in the order the generated table lists them.
pub const SETTINGS: &[Setting] = &[
    // -- Sidebar rows -----------------------------------------------------
    Setting {
        path: "showBranch",
        kind: "bool",
        default: "true",
        applies: "Sidebar rows, status bar",
        live: Live::Immediately,
        alt: "false",
        description: "Show the git branch a session's directory is on.",
    },
    Setting {
        path: "showPlace",
        kind: "bool",
        default: "true",
        applies: "Sidebar rows, status bar",
        live: Live::Immediately,
        alt: "false",
        description: "Show a session's working directory when it is not the project's own \
                      directory.",
    },
    Setting {
        path: "showWorktree",
        kind: "bool",
        default: "true",
        applies: "Sidebar rows, status bar",
        live: Live::Immediately,
        alt: "false",
        description: "Show which linked worktree of a repository a session is in.",
    },
    Setting {
        path: "showTime",
        kind: "bool",
        default: "true",
        applies: "Sidebar rows",
        live: Live::Immediately,
        alt: "false",
        description: "Show how long ago a session last produced output.",
    },
    Setting {
        path: "showStatusWord",
        kind: "bool",
        default: "true",
        applies: "Sidebar rows",
        live: Live::Immediately,
        alt: "false",
        description: "Spell out a session's state beside its status mark. Off leaves the mark.",
    },
    Setting {
        path: "showStatusBar",
        kind: "bool",
        default: "true",
        applies: "Window",
        live: Live::Immediately,
        alt: "false",
        description: "Show the bar under the pane carrying the focused session's directory, \
                      branch, worktree and daemon connection.",
    },
    Setting {
        path: "alwaysSlim",
        kind: "bool",
        default: "false",
        applies: "Sidebar rows",
        live: Live::Immediately,
        alt: "true",
        description: "Draw every sidebar row in the slim variant. Drops the second line of \
                      context from the rows that carry one.",
    },
    Setting {
        path: "showRestartToUpdate",
        kind: "bool",
        default: "true",
        applies: "Sidebar",
        live: Live::Immediately,
        alt: "false",
        description: "Show the band saying a restart will take the staged update. Off hides \
                      the band and nothing else: checking, staging and applying are unchanged.",
    },
    Setting {
        path: "confirmTerminate",
        kind: "bool",
        default: "true",
        applies: "Session actions",
        live: Live::Immediately,
        alt: "false",
        description: "Ask before terminating a session whose child is still running.",
    },
    // -- Lists and limits -------------------------------------------------
    Setting {
        path: "inbox.previewRows",
        kind: "integer, 1-50",
        default: "8",
        applies: "Sidebar bands",
        live: Live::Immediately,
        alt: "20",
        description: "Inbox rows a bucket draws before it offers the rest.",
    },
    Setting {
        path: "inbox.settledRows",
        kind: "integer, 1-100",
        default: "10",
        applies: "Sidebar bands",
        live: Live::Immediately,
        alt: "30",
        description: "Done-shelf rows a bucket draws before it offers the rest.",
    },
    Setting {
        path: "launcher.recentRows",
        kind: "integer, 1-50",
        default: "12",
        applies: "Launcher",
        live: Live::Immediately,
        alt: "5",
        description: "Recent commands the launcher lists.",
    },
    Setting {
        path: "launcher.historyLimit",
        kind: "integer, 10-1000",
        default: "60",
        applies: "Launcher completion",
        live: Live::Immediately,
        alt: "200",
        description: "Commands kept in the ranked history a save trims to.",
    },
    Setting {
        path: "snooze.morningHour",
        kind: "integer, 0-23",
        default: "9",
        applies: "Snooze menu",
        live: Live::Immediately,
        alt: "7",
        description: "Hour of day the morning snooze presets wake at.",
    },
    Setting {
        path: "snooze.eveningHour",
        kind: "integer, 0-23",
        default: "18",
        applies: "Snooze menu",
        live: Live::Immediately,
        alt: "21",
        description: "Hour of day the evening snooze preset wakes at.",
    },
    Setting {
        path: "search.contextLines",
        kind: "integer, 0-64",
        default: "2",
        applies: "Search sheet",
        live: Live::Immediately,
        alt: "5",
        description: "Lines quoted either side of a search hit.",
    },
    Setting {
        path: "search.maxHits",
        kind: "integer, 25-5000",
        default: "500",
        applies: "Search sheet",
        live: Live::Immediately,
        alt: "1000",
        description: "Hits one sweep returns before it reports the answer truncated.",
    },
    Setting {
        path: "maxTabs",
        kind: "integer, 2-32",
        default: "8",
        applies: "Tab strip",
        live: Live::Immediately,
        alt: "16",
        description: "Tabs the strip holds before it evicts the least recently used one. \
                      Eviction closes nothing.",
    },
    // -- Appearance -------------------------------------------------------
    Setting {
        path: "theme",
        kind: "system | light | dark",
        default: "\"system\"",
        applies: "Window",
        live: Live::Immediately,
        alt: "\"dark\"",
        description: "Which palette the interface paints. System follows the desktop and is \
                      re-read when the desktop changes it.",
    },
    Setting {
        path: "density",
        kind: "comfortable | compact",
        default: "\"comfortable\"",
        applies: "Sidebar rows",
        live: Live::Immediately,
        alt: "\"compact\"",
        description: "Vertical rhythm of the sidebar. Compact shrinks the row boxes and \
                      leaves the type sizes alone.",
    },
    Setting {
        path: "textScalePct",
        kind: "integer, 80-200",
        default: "100",
        applies: "Window",
        live: Live::Immediately,
        alt: "150",
        description: "Size of every piece of interface text and every gap between them, as a \
                      percentage. Separate from the terminal font size.",
    },
    Setting {
        path: "reduceMotion",
        kind: "bool",
        default: "false",
        applies: "Window",
        live: Live::Immediately,
        alt: "true",
        description: "Turn off every transition and animation in the interface, whatever the \
                      desktop reports.",
    },
    Setting {
        path: "appearance.opacityPct",
        kind: "integer, 20-100",
        default: "100",
        applies: "Window chrome",
        live: Live::NewWindow,
        alt: "80",
        description: "How much of the desktop shows through the interface. The desktop shows \
                      through unblurred unless your compositor blurs it.",
    },
    Setting {
        path: "appearance.terminalOpacityPct",
        kind: "integer, 20-100",
        default: "100",
        applies: "Terminal pane",
        live: Live::NewWindow,
        alt: "70",
        description: "How much of the desktop shows through the terminal grid, independent of \
                      the chrome.",
    },
    Setting {
        path: "appearance.backdrop",
        kind: "absolute path, empty for none",
        default: "\"\"",
        applies: "Window",
        live: Live::Immediately,
        alt: "\"/src/wall.png\"",
        description: "An image painted behind the interface.",
    },
    Setting {
        path: "appearance.backdropFit",
        kind: "cover | contain | tile | center",
        default: "\"cover\"",
        applies: "Window",
        live: Live::Immediately,
        alt: "\"tile\"",
        description: "How the backdrop image is fitted to the window.",
    },
    Setting {
        path: "appearance.backdropBlurPx",
        kind: "integer, 0-64",
        default: "0",
        applies: "Window",
        live: Live::Immediately,
        alt: "16",
        description: "Blur applied to the backdrop image. Does not blur the desktop, which no \
                      application can do.",
    },
    Setting {
        path: "appearance.backdropDimPct",
        kind: "integer, 0-100",
        default: "0",
        applies: "Window",
        live: Live::Immediately,
        alt: "30",
        description: "A scrim between the backdrop image and the interface, which is what \
                      keeps text readable over a bright photograph.",
    },
    // -- Terminal ---------------------------------------------------------
    Setting {
        path: "terminal.palette",
        kind: "inherit | one of seven named schemes",
        default: "\"inherit\"",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "\"nord\"",
        description: "Colours the grid paints with. Inherit follows the interface theme. \
                      Ignored while the host terminal import is in force.",
    },
    Setting {
        path: "terminal.followHostTerminal",
        kind: "bool",
        default: "false",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "true",
        description: "Paint with the colours read out of your own terminal's configuration \
                      instead of a built-in scheme. Does nothing until an import succeeds.",
    },
    Setting {
        path: "terminal.hostPalette",
        kind: "imported colours",
        default: "{\"source\":\"none\",\"origin\":\"\",\"background\":\"\",\"foreground\":\"\",\
                  \"cursor\":\"\",\"selection\":\"\",\"ansi\":[]}",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "{\"source\":\"flat\",\"origin\":\"/src/kitty.conf\",\"background\":\"#101010\",\
               \"foreground\":\"#d0d0d0\",\"cursor\":\"\",\"selection\":\"\",\"ansi\":[]}",
        description: "The colours the last import found, and the file they came out of. \
                      Written by the import, not typed.",
    },
    Setting {
        path: "terminal.fontFamily",
        kind: "font stack, empty for the platform default",
        default: "\"\"",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "\"Fira Code, monospace\"",
        description: "The face the grid is drawn in. A face that is not installed falls back \
                      to the platform's default monospace.",
    },
    Setting {
        path: "terminal.fontSizePx",
        kind: "integer, 8-32",
        default: "13",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "20",
        description: "Font size the grid is drawn at. Changing it re-measures the cell box \
                      and resizes every session's grid.",
    },
    Setting {
        path: "terminal.lineHeightPct",
        kind: "integer, 80-200",
        default: "100",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "120",
        description: "Height of a cell as a percentage of the font's own line height.",
    },
    Setting {
        path: "terminal.cellWidthPct",
        kind: "integer, 80-140",
        default: "100",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "110",
        description: "Width of a cell as a percentage of the font's own advance width.",
    },
    Setting {
        path: "terminal.cursorShape",
        kind: "block | bar | underline",
        default: "\"block\"",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "\"bar\"",
        description: "Shape of the cursor. A program that asks for a shape over an escape \
                      sequence still gets it; this is the shape before any program asks.",
    },
    Setting {
        path: "terminal.cursorBlink",
        kind: "bool",
        default: "true",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "false",
        description: "Blink the cursor.",
    },
    Setting {
        path: "terminal.blinkIntervalMs",
        kind: "integer, 100-2000",
        default: "530",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "800",
        description: "How long the cursor spends in each of its two states. Read only while \
                      blinking is on.",
    },
    Setting {
        path: "terminal.scrollbackLines",
        kind: "integer, 0-200000",
        default: "1000",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "20000",
        description: "Lines the pane keeps in memory. The daemon keeps the real history and \
                      is unaffected; this is what the wheel moves through without asking it.",
    },
    Setting {
        path: "terminal.wheelLines",
        kind: "integer, 1-25",
        default: "3",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "5",
        description: "Lines one wheel notch scrolls.",
    },
    Setting {
        path: "terminal.bracketedPaste",
        kind: "bool",
        default: "true",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "false",
        description: "Wrap pasted text in the bracketed-paste markers when the running \
                      program asked for them. Off refuses the markers to every program.",
    },
    Setting {
        path: "terminal.presentMode",
        kind: "vsync | adaptive | immediate",
        default: "\"vsync\"",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "\"immediate\"",
        description: "How finished frames reach the screen. Vsync never tears. Adaptive \
                      drops a late frame rather than queueing it. Immediate presents as soon \
                      as a frame is ready and can tear. A mode the graphics adapter does not \
                      offer falls back to vsync. Switching drops the frames already queued, \
                      so the pane is blank for one frame.",
    },
    // -- Notices ----------------------------------------------------------
    Setting {
        path: "notices.flashSeconds",
        kind: "integer, 0-60",
        default: "6",
        applies: "Flash strip",
        live: Live::Immediately,
        alt: "12",
        description: "How long a confirmation stays up. 0 keeps it until it is dismissed.",
    },
    Setting {
        path: "notices.noticeSeconds",
        kind: "integer, 0-60",
        default: "0",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "8",
        description: "How long a pane notice stays up. 0 keeps it until it is dismissed.",
    },
    Setting {
        path: "notices.showHistoryNotice",
        kind: "bool",
        default: "true",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "false",
        description: "Show the strip saying the pane is on history rather than the live tail. \
                      Off removes the strip and leaves the scroll position alone.",
    },
    Setting {
        path: "notices.showStartupErrors",
        kind: "bool",
        default: "true",
        applies: "Terminal pane",
        live: Live::Immediately,
        alt: "false",
        description: "Show what a harness printed while it was starting. Off hides it, \
                      including the reason a harness that failed to start gives.",
    },
    // -- Startup ----------------------------------------------------------
    Setting {
        path: "startup.showSplash",
        kind: "bool",
        default: "true",
        applies: "Boot surface",
        live: Live::NextLaunch,
        alt: "false",
        description: "Draw the mark on the window before the first frame.",
    },
    Setting {
        path: "startup.splashAfterMs",
        kind: "integer, 0-5000",
        default: "120",
        applies: "Boot surface",
        live: Live::NextLaunch,
        alt: "400",
        description: "Milliseconds of startup before the mark is drawn. A start faster than \
                      this draws no mark, because a mark that appears and vanishes inside a \
                      tenth of a second reads as a fault.",
    },
    // -- Notifications ----------------------------------------------------
    Setting {
        path: "notifications.finished",
        kind: "bool",
        default: "false",
        applies: "Desktop notifications",
        live: Live::Immediately,
        alt: "true",
        description: "Raise a notification when a session's child exits.",
    },
    Setting {
        path: "notifications.needsApproval",
        kind: "bool",
        default: "true",
        applies: "Desktop notifications",
        live: Live::Immediately,
        alt: "false",
        description: "Raise a notification when a session blocks on an approval.",
    },
    Setting {
        path: "notifications.failed",
        kind: "bool",
        default: "true",
        applies: "Desktop notifications",
        live: Live::Immediately,
        alt: "false",
        description: "Raise a notification when a session's child exits non-zero.",
    },
    Setting {
        path: "notifications.skipFocusedSession",
        kind: "bool",
        default: "true",
        applies: "Desktop notifications",
        live: Live::Immediately,
        alt: "false",
        description: "Skip the notification when the session is the one on screen.",
    },
    // -- Keyboard ---------------------------------------------------------
    Setting {
        path: "keyboard.overrides",
        kind: "map of action name to chord",
        default: "{}",
        applies: "Key dispatch, shortcut overlay",
        live: Live::Immediately,
        alt: "{\"toggle-sidebar\":\"Ctrl+Alt+J\"}",
        description: "Chords moved off their built-in keys. An entry that does not parse is \
                      ignored and the built-in chord stands.",
    },
    Setting {
        path: "keyboard.custom",
        kind: "list of chord and steps",
        default: "[]",
        applies: "Key dispatch, shortcut overlay",
        live: Live::Immediately,
        alt: "[{\"label\":\"Review\",\"chord\":\"Ctrl+Shift+G\",\"steps\":[]}]",
        description: "Chords you wrote, each running a list of steps. Matched before the \
                      built-in table, so one on a built-in chord shadows it.",
    },
    // -- Advanced ---------------------------------------------------------
    Setting {
        path: "daemonUrl",
        kind: "URL, empty for the command line's",
        default: "\"\"",
        applies: "Daemon connection",
        live: Live::Immediately,
        alt: "\"ws://10.0.0.4:9000\"",
        description: "The daemon to dial. Empty uses whatever --server said. Changing it \
                      reconnects.",
    },
    Setting {
        path: "policy",
        kind: "auto-settle window, in milliseconds",
        default: "{\"autoSettleAfterMs\":604800000}",
        applies: "Sidebar bands",
        live: Live::Immediately,
        alt: "{\"autoSettleAfterMs\":900000}",
        description: "How long an unattended session stays in the inbox before it settles. \
                      Null never settles one.",
    },
    Setting {
        path: "updateChannel",
        kind: "stable | nightly",
        default: "\"stable\"",
        applies: "Update checks",
        live: Live::Immediately,
        alt: "\"nightly\"",
        description: "Which stream of releases this profile follows. Verification does not \
                      vary by channel.",
    },
    Setting {
        path: "connection.reconnectMaxMs",
        kind: "integer, 1000-600000",
        default: "30000",
        applies: "Daemon connection",
        live: Live::Immediately,
        alt: "60000",
        description: "Longest gap between two reconnect attempts.",
    },
    Setting {
        path: "connection.reconnectAttempts",
        kind: "integer, 1-200",
        default: "25",
        applies: "Daemon connection",
        live: Live::Immediately,
        alt: "50",
        description: "Reconnect attempts made before the window offers Retry instead.",
    },
    Setting {
        path: "updateCheckHours",
        kind: "integer, 1-168",
        default: "4",
        applies: "Update checks",
        live: Live::Immediately,
        alt: "24",
        description: "Hours between two quiet update checks while a window is open.",
    },
    // -- Written by the product, not by a control -------------------------
    Setting {
        path: "onboarded",
        kind: "bool",
        default: "false",
        applies: "First-run sheet",
        live: Live::Immediately,
        alt: "true",
        description: "Whether the first-run sheet has been closed. Written by that sheet.",
    },
    Setting {
        path: "seenVersion",
        kind: "version string, empty for never",
        default: "\"\"",
        applies: "Release notes sheet",
        live: Live::Immediately,
        alt: "\"0.4.0\"",
        description: "The version whose release notes were last shown. Written by that sheet.",
    },
    Setting {
        path: "ignoredUpdate",
        kind: "version string, empty for none",
        default: "\"\"",
        applies: "Titlebar update chip",
        live: Live::Immediately,
        alt: "\"0.4.1\"",
        description: "The newest release dismissed from the titlebar. Written by that chip.",
    },
];

/// The catalogued paths.
///
/// A set for the suite to difference against [`declared_paths`]. The running
/// program looks settings up one at a time through [`setting`].
#[cfg(test)]
#[must_use]
pub fn catalogued_paths() -> BTreeSet<String> {
    SETTINGS.iter().map(|s| s.path.to_string()).collect()
}

/// The row for one path.
#[must_use]
pub fn setting(path: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.path == path)
}

/// The table, as a markdown document body.
///
/// Generated rather than written, so a setting cannot be documented with a
/// default that is not its default. Read by the suite, which is what checks
/// the manual against it; no window renders it.
#[cfg(test)]
#[must_use]
pub fn markdown_table() -> String {
    let mut out = String::from(
        "| Setting | Type | Default | Applies to | When |\n\
         | --- | --- | --- | --- | --- |\n",
    );
    for s in SETTINGS {
        out.push_str(&format!(
            "| `{}` | {} | `{}` | {} | {} |\n",
            s.path,
            s.kind,
            s.default,
            s.applies,
            s.live.word()
        ));
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// What the source actually declares
// ═══════════════════════════════════════════════════════════════════════════

/// The settings source, as shipped.
///
/// `include_str!` and not a path read at run time: the test binary must see
/// the source it was compiled from, and a path relative to the working
/// directory is a different file the moment anything runs from elsewhere.
///
/// Test-only, and that is the point of it. Parsing the source is how the
/// suite proves the list above is complete; a running window has the real
/// struct and needs no parser.
#[cfg(test)]
const SOURCE: &str = include_str!("../settings.rs");

/// Every persisted leaf the source declares, as a dotted path in the names the
/// file uses.
///
/// Walks from `Settings` and recurses into any field whose type is another
/// struct declared in the same source. A type from anywhere else is a leaf: it
/// is persisted whole, its own module owns its shape, and a row here describes
/// the whole of it.
#[cfg(test)]
#[must_use]
pub fn declared_paths() -> BTreeSet<String> {
    let structs = declared_structs();
    let mut out = BTreeSet::new();
    walk("Settings", "", &structs, &mut out, 0);
    out
}

#[cfg(test)]
fn walk(
    name: &str,
    prefix: &str,
    structs: &BTreeMap<String, Vec<(String, String)>>,
    out: &mut BTreeSet<String>,
    depth: usize,
) {
    // A struct that contains itself would otherwise not terminate. Nothing in
    // the file does, and a guard is cheaper than trusting that it stays true.
    assert!(depth < 8, "settings nest more than eight deep at {prefix}");
    let Some(fields) = structs.get(name) else {
        return;
    };
    for (field, ty) in fields {
        let path = if prefix.is_empty() {
            camel(field)
        } else {
            format!("{prefix}.{}", camel(field))
        };
        if structs.contains_key(ty) {
            walk(ty, &path, structs, out, depth + 1);
        } else {
            out.insert(path);
        }
    }
}

/// Every `pub struct` in the settings source, and its public fields.
///
/// Only the structs that carry `#[serde(default)]` on the container, which is
/// the marker this file already uses for "a group persisted whole". A helper
/// struct without it is not part of the document.
#[cfg(test)]
fn declared_structs() -> BTreeMap<String, Vec<(String, String)>> {
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut lines = SOURCE.lines().peekable();
    let mut persisted = false;
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[serde(") && trimmed.contains("default") {
            persisted = true;
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("pub struct ") else {
            if !trimmed.starts_with("#[") && !trimmed.starts_with("///") && !trimmed.is_empty() {
                persisted = false;
            }
            continue;
        };
        let Some(name) = rest.strip_suffix(" {") else {
            persisted = false;
            continue;
        };
        if !persisted {
            continue;
        }
        persisted = false;
        let mut fields: Vec<(String, String)> = Vec::new();
        for body in lines.by_ref() {
            let body = body.trim();
            if body == "}" {
                break;
            }
            let Some(decl) = body.strip_prefix("pub ") else {
                continue;
            };
            let Some((field, ty)) = decl.split_once(':') else {
                continue;
            };
            if field.contains(' ') || field.contains('(') {
                continue;
            }
            let ty = ty
                .trim()
                .trim_end_matches(',')
                .rsplit("::")
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            fields.push((field.trim().to_string(), ty));
        }
        out.insert(name.to_string(), fields);
    }
    out
}

/// A snake_case field name as the file writes it.
#[cfg(test)]
fn camel(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut upper = false;
    for c in field.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests;
