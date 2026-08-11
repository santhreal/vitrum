//! The keyboard map: one table, three consumers.
//!
//! [`CHORDS`] is the only place a shortcut is defined. Three things read it and
//! nothing else may hard-code a chord:
//!
//! 1. [`claims`] answers whether the shell already owns a combination, so a
//!    feature that wants a chord of its own cannot quietly take one that is
//!    already bound.
//! 2. [`help_rows`] renders the shortcut overlay from the same table, so every
//!    chord that can fire is listed somewhere a user can find it.
//! 3. [`KeyAction::parse`] turns a stored action string back into the action
//!    Rust performs, which is how an override or a custom binding names a
//!    built-in.
//!
//! The reason for the single table is the acceptance criterion "no shortcut may
//! be undiscoverable". That is not something a comment can guarantee; it is
//! guaranteed by [`tests::every_chord_is_documented`], which fails the build if
//! a chord exists with no overlay row that shows it.
//!
//! The primary modifier is Ctrl on every platform, macOS included. Cmd+Tab is
//! the macOS application switcher and never reaches an application, so binding
//! tab traversal to it would produce a shortcut that is documented and dead.
//! The overlay says this out loud rather than leaving a Mac user guessing.

/// A global chord the shell owns rather than the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    NextTab,
    PrevTab,
    CloseTab,
    /// Zero-based position in the tab strip, from Alt+1 through Alt+9.
    SelectTab(usize),
    ToggleSidebar,
    /// Move focus into the sidebar filter field.
    FocusSearch,
    /// Open the cross-session scrollback search.
    ///
    /// Distinct from [`KeyAction::FocusSearch`], which moves the caret into
    /// the sidebar's local filter over titles, directories and branches. This
    /// one asks the DAEMON to sweep every retained scrollback buffer, which is
    /// a question no client can answer for itself: only the daemon holds every
    /// session's bytes.
    OpenSearch,
    /// Move focus onto a session row so the arrow keys traverse the list.
    FocusSidebar,
    /// Open the new-session dialog.
    NewSession,
    /// Launch one saved preset outright, with no dialog at all.
    ///
    /// Carries [`crate::launch::SavedPreset::id`], which is minted once and
    /// never renumbered, so a rebind or a reorder cannot repoint a chord at a
    /// different command.
    ///
    /// This is what makes a preset's chord a SHORTCUT rather than a dialog
    /// accelerator. The chord used to be matched only by the new-session
    /// dialog's own keydown handler, so firing it meant opening the dialog
    /// first: two keystrokes to reach a thing whose whole purpose was to be
    /// one. Folded into the shared table, it fires from anywhere, is checked
    /// for conflicts against every built-in chord, and appears in the
    /// shortcut overlay beside them.
    LaunchPreset(u64),
    /// Terminate the focused session, not merely its tab.
    CloseSession,
    /// Rename the focused session.
    RenameSession,
    /// Start a second session with the focused one's command and directory.
    DuplicateSession,
    /// Focus the next session whose status wants the operator.
    NextAttention,
    /// Focus the previous session whose status wants the operator.
    PrevAttention,
    /// Move focus one row down the visible session list.
    NextRow,
    /// Move focus one row up the visible session list.
    PrevRow,
    /// Extend the multi-selection one row down.
    ExtendDown,
    /// Extend the multi-selection one row up.
    ExtendUp,
    /// Select every row currently on screen.
    SelectAllRows,
    /// Show or hide the shortcut overlay.
    ToggleShortcuts,
    /// Close whichever transient layer is open.
    Dismiss,
}

impl KeyAction {
    /// The stable string this action is stored and matched as.
    pub fn wire(self) -> String {
        match self {
            KeyAction::NextTab => "next".to_string(),
            KeyAction::PrevTab => "prev".to_string(),
            KeyAction::CloseTab => "close".to_string(),
            KeyAction::SelectTab(i) => format!("tab:{i}"),
            KeyAction::ToggleSidebar => "sidebar".to_string(),
            KeyAction::FocusSearch => "search".to_string(),
            KeyAction::OpenSearch => "openSearch".to_string(),
            KeyAction::FocusSidebar => "focusSidebar".to_string(),
            KeyAction::NewSession => "newSession".to_string(),
            KeyAction::LaunchPreset(id) => format!("preset:{id}"),
            KeyAction::CloseSession => "closeSession".to_string(),
            KeyAction::RenameSession => "rename".to_string(),
            KeyAction::DuplicateSession => "duplicate".to_string(),
            KeyAction::NextAttention => "nextAttention".to_string(),
            KeyAction::PrevAttention => "prevAttention".to_string(),
            KeyAction::NextRow => "nextRow".to_string(),
            KeyAction::PrevRow => "prevRow".to_string(),
            KeyAction::ExtendDown => "extendDown".to_string(),
            KeyAction::ExtendUp => "extendUp".to_string(),
            KeyAction::SelectAllRows => "selectAllRows".to_string(),
            KeyAction::ToggleShortcuts => "shortcuts".to_string(),
            KeyAction::Dismiss => "dismiss".to_string(),
        }
    }

    /// Parse a stored action string.
    ///
    /// Unknown strings return `None` rather than a default action: an action
    /// name this build does not recognise must fall through to nothing, never
    /// to "switch tabs", or a settings file from a later build starts silently
    /// stealing keys.
    pub fn parse(s: &str) -> Option<KeyAction> {
        match s {
            "next" => Some(KeyAction::NextTab),
            "prev" => Some(KeyAction::PrevTab),
            "close" => Some(KeyAction::CloseTab),
            "sidebar" => Some(KeyAction::ToggleSidebar),
            "search" => Some(KeyAction::FocusSearch),
            "openSearch" => Some(KeyAction::OpenSearch),
            "focusSidebar" => Some(KeyAction::FocusSidebar),
            "newSession" => Some(KeyAction::NewSession),
            "closeSession" => Some(KeyAction::CloseSession),
            "rename" => Some(KeyAction::RenameSession),
            "duplicate" => Some(KeyAction::DuplicateSession),
            "nextAttention" => Some(KeyAction::NextAttention),
            "prevAttention" => Some(KeyAction::PrevAttention),
            "nextRow" => Some(KeyAction::NextRow),
            "prevRow" => Some(KeyAction::PrevRow),
            "extendDown" => Some(KeyAction::ExtendDown),
            "extendUp" => Some(KeyAction::ExtendUp),
            "selectAllRows" => Some(KeyAction::SelectAllRows),
            "shortcuts" => Some(KeyAction::ToggleShortcuts),
            "dismiss" => Some(KeyAction::Dismiss),
            other => {
                if let Some(n) = other.strip_prefix("preset:") {
                    // No range check: preset ids are minted, not positional,
                    // and the launcher is what decides whether one still
                    // exists. Refusing here would need this file to know the
                    // operator's saved list.
                    return n.parse().ok().map(KeyAction::LaunchPreset);
                }
                let n = other.strip_prefix("tab:")?;
                let i: usize = n.parse().ok()?;
                (i < TAB_SLOTS).then_some(KeyAction::SelectTab(i))
            }
        }
    }
}

/// How many positional tab slots Alt+digit addresses.
pub const TAB_SLOTS: usize = 9;

/// Whether a chord cares about the shift key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shift {
    /// Shift must be up.
    Off,
    /// Shift must be down.
    On,
    /// Either. Used by punctuation whose shifted form is a different `key`
    /// string on one layout and the same one on another.
    Any,
}

/// Where a chord is allowed to fire.
///
/// The terminal is a real program that binds real keys. A shell that claimed
/// every chord everywhere would make Ctrl+K unusable inside readline, so scope
/// is part of the binding, not an afterthought in the event handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Claimed anywhere, including inside the terminal and text fields.
    /// Reserved for modifier combinations no terminal program binds.
    Global,
    /// Skipped while the terminal grid has focus, so the agent receives it.
    NotTerminal,
    /// Skipped while any text entry has focus. The terminal pane receives key
    /// events directly and passes every printable one to the agent, so it is a
    /// text entry as well and this excludes it too.
    NotTextInput,
    /// Only fires while a transient layer (overlay, menu, dialog) is open.
    /// Escape belongs to the agent the rest of the time.
    LayerOnly,
    /// Only fires while focus is on a sidebar row, and never while a layer is
    /// open. Bare arrow keys belong to the agent everywhere else, so list
    /// traversal may only claim them once the operator has actually moved
    /// into the list.
    SessionList,
}

/// Section of the shortcut overlay a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Switching,
    Sessions,
    Sidebar,
    Window,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Switching => "Switching",
            Group::Sessions => "Sessions",
            Group::Sidebar => "Sidebar",
            Group::Window => "Window",
        }
    }
}

/// Every group, in overlay order.
pub const GROUPS: [Group; 4] = [
    Group::Switching,
    Group::Sessions,
    Group::Sidebar,
    Group::Window,
];

/// The overlay section for chords the PANE consumes.
///
/// Separate from [`GROUPS`] because these are not shell bindings and must
/// never become any. [`CHORDS`] is what key dispatch matches, and an entry
/// there is claimed before the focused surface sees the key; a copy chord in
/// that table would fire a shell action AND be copied, or worse, be claimed
/// and never reach the pane at all. So the pane's chords are documented here
/// and bound in the pane, and the two tables are checked against each other
/// rather than merged.
pub const PANE_SECTION_TITLE: &str = "Terminal";

/// One chord the pane handles itself.
///
/// No `KeyAction`, deliberately. There is nothing for the shell to do with
/// one of these, and giving it an action is the mistake this type exists to
/// make impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneChord {
    /// As the overlay prints it, in the same shape [`Chord::rendered`] emits.
    pub keys: &'static str,
    pub what: &'static str,
}

/// Every chord the pane consumes while it has focus.
///
/// These reach the pane because [`CHORDS`] does not contain them: dispatch
/// finds no claim and passes the key on. That is the whole mechanism, and it
/// is why the guard beside the overlay asserts the absence rather than
/// trusting it.
pub const PANE_CHORDS: &[PaneChord] = &[
    PaneChord {
        keys: "Ctrl+Shift+C",
        what: "Copy the selection",
    },
    PaneChord {
        keys: "Ctrl+Shift+V",
        what: "Paste",
    },
    PaneChord {
        keys: "Ctrl+Shift+G",
        what: "Find in this session; Enter and Shift+Enter step, Escape closes",
    },
];

/// The overlay row a chord contributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Help {
    pub group: Group,
    pub what: &'static str,
    /// Replaces the derived key rendering when one row documents several
    /// chords: an alias pair, or the Alt+1 through Alt+9 range.
    pub keys: Option<&'static str>,
}

/// One binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub action: KeyAction,
    /// The key name, lowercased for single characters.
    pub key: &'static str,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: Shift,
    pub scope: Scope,
    /// `None` when another chord's row already shows this one, which is how
    /// aliases stay documented without duplicating a line in the overlay.
    pub help: Option<Help>,
}

impl Chord {
    /// Human rendering of just this chord, e.g. `"Ctrl+Shift+W"`.
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
        s.push_str(key_label(self.key));
        s
    }

    /// What the overlay says this chord does.
    ///
    /// An alias carries no help row of its own, so it borrows the sentence
    /// from the primary chord for the same action rather than reporting
    /// nothing: this is the text a conflict message quotes back at whoever
    /// tried to bind over it, and "already bound" without saying to what is
    /// the same as saying nothing.
    pub fn describes(&self) -> &'static str {
        if let Some(h) = self.help {
            return h.what;
        }
        CHORDS
            .iter()
            .find_map(|c| (c.action == self.action).then_some(c.help?.what))
            .unwrap_or("claimed by the shell")
    }
}

/// The shell's own binding for this combination, if it would fire inside a
/// dialog text field.
///
/// A chord in [`CHORDS`] is matched before the focused surface sees the key, so
/// a feature that wants to bind its own chord inside a dialog has to ask this
/// first or it ships a key that never fires.
///
/// Only the scopes that survive a focused text field count as a conflict.
/// [`Scope::NotTextInput`] and [`Scope::SessionList`] are both false the
/// moment focus is in an `input`, and reporting them would refuse bindings
/// that would have worked perfectly.
pub fn claims(key: &str, ctrl: bool, alt: bool, shift: bool) -> Option<Chord> {
    CHORDS
        .iter()
        .find(|c| {
            c.key == key
                && c.ctrl == ctrl
                && c.alt == alt
                && match c.shift {
                    Shift::On => shift,
                    Shift::Off => !shift,
                    Shift::Any => true,
                }
                && matches!(
                    c.scope,
                    Scope::Global | Scope::NotTerminal | Scope::LayerOnly
                )
        })
        .copied()
}

/// The chord one keydown means, with the top digit row unshifted.
///
/// The key name for Ctrl+Shift+1 on a US layout is `!`, not `1`, so a
/// chord stored as `1` never matches the keystroke it is named after: a
/// shortcut a settings panel displays, the overlay explains, and the product
/// never fires. The physical key code is unaffected by Shift or by the
/// layout, so a top-row digit is taken from there. Everything else comes from
/// the key name, because the code for a letter is `KeyK` rather than `k` and
/// because a chord bound to a letter is already layout-dependent in the
/// operator's head.
///
/// Every Rust surface that reads a chord off a key event has to come through
/// here: for a while the launcher had the rule and nothing else did, which is
/// why a preset chord worked inside the dialog and did nothing anywhere else.
#[must_use]
#[cfg(test)]
pub fn chord_from_event(
    key: &str,
    code: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> crate::launch::Chord {
    let digit = code
        .strip_prefix("Digit")
        .filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit()));
    crate::launch::Chord {
        key: digit.unwrap_or(key).to_lowercase(),
        ctrl,
        alt,
        shift,
    }
}

/// Display form of a key name.
fn key_label(key: &str) -> &str {
    match key {
        "arrowdown" => "Down",
        "arrowup" => "Up",
        "arrowleft" => "Left",
        "arrowright" => "Right",
        "escape" => "Esc",
        "pagedown" => "PageDown",
        "pageup" => "PageUp",
        "tab" => "Tab",
        "f1" => "F1",
        "a" => "A",
        "b" => "B",
        "d" => "D",
        "e" => "E",
        "f" => "F",
        "k" => "K",
        "n" => "N",
        "o" => "O",
        "r" => "R",
        "w" => "W",
        "x" => "X",
        other => other,
    }
}

const fn c(
    action: KeyAction,
    key: &'static str,
    ctrl: bool,
    alt: bool,
    shift: Shift,
    scope: Scope,
    help: Option<Help>,
) -> Chord {
    Chord {
        action,
        key,
        ctrl,
        alt,
        shift,
        scope,
        help,
    }
}

const fn help(group: Group, what: &'static str) -> Option<Help> {
    Some(Help {
        group,
        what,
        keys: None,
    })
}

const fn help_as(group: Group, what: &'static str, keys: &'static str) -> Option<Help> {
    Some(Help {
        group,
        what,
        keys: Some(keys),
    })
}

/// Every chord the shell claims, in match order.
///
/// Match order matters only where two entries could both match one event, and
/// no two here can: every pair differs in `key` or in a modifier that is not
/// [`Shift::Any`].
pub const CHORDS: &[Chord] = &[
    // ---- Tabs -----------------------------------------------------------
    c(
        KeyAction::NextTab,
        "tab",
        true,
        false,
        Shift::Off,
        Scope::Global,
        help_as(Group::Switching, "Next session", "Ctrl+Tab / Ctrl+PageDown"),
    ),
    c(
        KeyAction::NextTab,
        "pagedown",
        true,
        false,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::PrevTab,
        "tab",
        true,
        false,
        Shift::On,
        Scope::Global,
        help_as(
            Group::Switching,
            "Previous session",
            "Ctrl+Shift+Tab / Ctrl+Shift+PageUp",
        ),
    ),
    c(
        KeyAction::PrevTab,
        "pageup",
        true,
        false,
        Shift::On,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::SelectTab(0),
        "1",
        false,
        true,
        Shift::Off,
        Scope::Global,
        help_as(
            Group::Switching,
            "Focus session by position",
            "Alt+1 - Alt+9",
        ),
    ),
    c(
        KeyAction::SelectTab(1),
        "2",
        false,
        true,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::SelectTab(2),
        "3",
        false,
        true,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::SelectTab(3),
        "4",
        false,
        true,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::SelectTab(4),
        "5",
        false,
        true,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::SelectTab(5),
        "6",
        false,
        true,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::SelectTab(6),
        "7",
        false,
        true,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::SelectTab(7),
        "8",
        false,
        true,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::SelectTab(8),
        "9",
        false,
        true,
        Shift::Off,
        Scope::Global,
        None,
    ),
    c(
        KeyAction::CloseTab,
        "w",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Switching, "Stop viewing; the session keeps running"),
    ),
    // ---- Sessions -------------------------------------------------------
    c(
        KeyAction::NewSession,
        "n",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Sessions, "New session"),
    ),
    c(
        KeyAction::RenameSession,
        "r",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Sessions, "Rename the focused session"),
    ),
    c(
        KeyAction::DuplicateSession,
        "d",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(
            Group::Sessions,
            "Duplicate the focused session into a new one",
        ),
    ),
    c(
        KeyAction::CloseSession,
        "x",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Sessions, "Terminate the focused session"),
    ),
    c(
        KeyAction::NextAttention,
        "arrowdown",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Sessions, "Next session that needs you"),
    ),
    c(
        KeyAction::PrevAttention,
        "arrowup",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Sessions, "Previous session that needs you"),
    ),
    // ---- Sidebar --------------------------------------------------------
    c(
        KeyAction::FocusSearch,
        "k",
        true,
        false,
        Shift::Off,
        Scope::NotTerminal,
        help(Group::Sidebar, "Filter sessions"),
    ),
    // Ctrl+Shift+F used to be a second way to focus the sidebar filter, which
    // Ctrl+K already does and is documented for. Repointing it at the daemon's
    // cross-session scrollback sweep costs nothing and buys a Scope::Global
    // chord, so the search opens from inside a terminal pane rather than only
    // when focus happens to be outside one.
    //
    // Plain Ctrl+F was considered and rejected: it is forward-char in readline
    // and emacs, so it would need Scope::NotTerminal and would be dead in the
    // pane where the operator actually is. Ctrl+Shift+F is not a readline
    // binding.
    c(
        KeyAction::OpenSearch,
        "f",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Sessions, "Search every session's scrollback"),
    ),
    c(
        KeyAction::FocusSidebar,
        "e",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Sidebar, "Move focus onto the session list"),
    ),
    c(
        KeyAction::ToggleSidebar,
        "b",
        true,
        false,
        Shift::On,
        Scope::Global,
        help(Group::Sidebar, "Show or hide the sidebar"),
    ),
    c(
        KeyAction::NextRow,
        "arrowdown",
        false,
        false,
        Shift::Off,
        Scope::SessionList,
        help_as(Group::Sidebar, "Move down and up the list", "Down / Up"),
    ),
    c(
        KeyAction::PrevRow,
        "arrowup",
        false,
        false,
        Shift::Off,
        Scope::SessionList,
        None,
    ),
    c(
        KeyAction::ExtendDown,
        "arrowdown",
        false,
        false,
        Shift::On,
        Scope::SessionList,
        help_as(
            Group::Sidebar,
            "Extend the selection",
            "Shift+Down / Shift+Up",
        ),
    ),
    c(
        KeyAction::ExtendUp,
        "arrowup",
        false,
        false,
        Shift::On,
        Scope::SessionList,
        None,
    ),
    c(
        KeyAction::SelectAllRows,
        "a",
        true,
        false,
        Shift::Off,
        Scope::SessionList,
        help(Group::Sidebar, "Select every row on screen"),
    ),
    // ---- Window ---------------------------------------------------------
    c(
        KeyAction::ToggleShortcuts,
        "f1",
        false,
        false,
        Shift::Off,
        Scope::Global,
        help_as(Group::Window, "Show this list", "F1 / ? / Ctrl+/"),
    ),
    c(
        KeyAction::ToggleShortcuts,
        "?",
        false,
        false,
        Shift::Any,
        Scope::NotTextInput,
        None,
    ),
    c(
        KeyAction::ToggleShortcuts,
        "/",
        true,
        false,
        Shift::Any,
        Scope::NotTerminal,
        None,
    ),
    c(
        KeyAction::Dismiss,
        "escape",
        false,
        false,
        Shift::Off,
        Scope::LayerOnly,
        help(Group::Window, "Close the open overlay, menu or dialog"),
    ),
];

/// One line of the shortcut overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpRow {
    pub group: Group,
    pub keys: String,
    pub what: &'static str,
}

/// Every overlay row, in table order.
///
/// TEST-ONLY. The shipped overlay reads
/// [`crate::ui::settings::effective_help_rows`], which applies the operator's
/// key overrides; this reads the raw table and does not. Two live sources for
/// one list is how an overlay ends up documenting a binding the product no
/// longer has, so this stays behind `cfg(test)` where the tests that assert
/// over the DEFAULT table can still reach it.
#[cfg(test)]
pub fn help_rows() -> Vec<HelpRow> {
    CHORDS
        .iter()
        .filter_map(|ch| {
            let h = ch.help?;
            Some(HelpRow {
                group: h.group,
                keys: h.keys.map(str::to_string).unwrap_or_else(|| ch.rendered()),
                what: h.what,
            })
        })
        .collect()
}

/// Overlay rows for one section. Test-only, for the reason above.
#[cfg(test)]
pub fn help_rows_for(group: Group) -> Vec<HelpRow> {
    help_rows()
        .into_iter()
        .filter(|r| r.group == group)
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Custom bindings
// ═══════════════════════════════════════════════════════════════════════════

// Everything above is the fixed table. Everything below is the operator's own
// bindings, which are data in the settings file rather than rows in a `const`.
//
// The split is deliberate. `CHORDS` answers "what does this build claim, and is
// every claim documented", which only a compile-time table can guarantee. A
// custom binding answers "what did this operator ask for", which is a string
// somebody typed and therefore has to survive nonsense, a newer build's
// vocabulary, and a hand-edited depth bomb.
//
// This half is a pure function: `CustomBinding::plan` takes the binding and
// `Facts`, a snapshot of the state the predicates ask about, and returns the
// flat ordered `Effect` list. It reads no signal, touches no socket and knows
// nothing about the toolkit, so the whole feature is testable without a window.

/// How deeply a conditional may nest inside a binding.
///
/// The limit exists because the settings file is hand-editable and the planner
/// is recursive: without it, a file nesting `when` a few thousand deep aborts
/// the process on a stack overflow, which is not a failure an operator can
/// diagnose or recover from. Eight is far past any binding a human writes and
/// far short of a stack that matters.
pub const MAX_BINDING_DEPTH: usize = 8;

/// Why a binding cannot be planned.
///
/// Every case is a refusal to act. Literal input especially: a half-decoded
/// escape must not reach a pty, because the bytes that did arrive would run
/// whatever they happen to mean. Nothing is performed unless the whole binding
/// resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingError {
    /// Conditionals nest past [`MAX_BINDING_DEPTH`].
    TooDeep { limit: usize },
    /// An escape sequence this build does not define.
    BadEscape { at: usize, what: String },
    /// A backslash, or a `\x`, at the end of the text with nothing after it.
    UnterminatedEscape { at: usize },
    /// The binding's chord text is not a chord.
    BadChord { chord: String },
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingError::TooDeep { limit } => write!(
                f,
                "conditionals nest deeper than {limit}; flatten the binding"
            ),
            BindingError::BadEscape { at, what } => write!(
                f,
                "{what} at byte {at} is not an escape this build knows; \
                 use \\\\, \\n, \\r, \\t, \\e, \\a, \\b, \\f, \\0 or \\xNN"
            ),
            BindingError::UnterminatedEscape { at } => write!(
                f,
                "the escape starting at byte {at} runs off the end of the text; \
                 finish it, or write \\\\ for a literal backslash"
            ),
            BindingError::BadChord { chord } => write!(
                f,
                "{chord:?} is not a chord; write it as Ctrl+Shift+K, with Ctrl \
                 or Alt and exactly one other key"
            ),
        }
    }
}

impl std::error::Error for BindingError {}

/// Declares a closed set of kebab-case names whose unknown case is a value
/// rather than a parse failure.
///
/// Hand-written serde rather than a derive, because `#[serde(other)]` is only
/// allowed on an internally tagged enum and these appear as plain field values.
/// Without the fallback, one status name added in a later build makes the whole
/// settings file fail to parse, and the operator loses every unrelated setting
/// in it.
macro_rules! wire_kind {
    (
        $(#[$outer:meta])*
        $name:ident { $( $(#[$inner:meta])* $variant:ident => $wire:literal ),+ $(,)? }
    ) => {
        $(#[$outer])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $( $(#[$inner])* $variant, )+
            /// A name this build does not define, read from a newer settings
            /// file. Never matches anything.
            Unknown,
        }

        impl $name {
            /// The name this value serialises as.
            #[must_use]
            pub const fn wire(self) -> &'static str {
                match self {
                    $( $name::$variant => $wire, )+
                    $name::Unknown => "unknown",
                }
            }

            /// Every defined value, in declaration order. `Unknown` is not one.
            #[must_use]
            pub const fn all() -> &'static [$name] {
                &[ $( $name::$variant, )+ ]
            }

            /// Parse a serialised name; anything else is [`Self::Unknown`].
            #[must_use]
            pub fn from_wire(s: &str) -> Self {
                match s {
                    $( $wire => $name::$variant, )+
                    _ => $name::Unknown,
                }
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.wire())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(d)?;
                Ok($name::from_wire(&raw))
            }
        }
    };
}

wire_kind! {
    /// What the sidebar says the focused session is doing.
    ///
    /// The five states of [`vitrum_model::SidebarStatus`], which is what the row
    /// the operator is looking at actually shows. Kept as a separate enum
    /// because this one has an `Unknown` case and the model's must not.
    StatusKind {
        /// Blocked asking the operator to approve an action.
        Approval => "approval",
        /// Blocked asking the operator a question.
        Input => "input",
        /// Computing. Nothing is wanted.
        Working => "working",
        /// Exited nonzero or signalled.
        Failed => "failed",
        /// Stopped, and the next move is the operator's.
        Ready => "ready",
    }
}

impl From<vitrum_model::SidebarStatus> for StatusKind {
    fn from(status: vitrum_model::SidebarStatus) -> Self {
        match status {
            vitrum_model::SidebarStatus::Approval => StatusKind::Approval,
            vitrum_model::SidebarStatus::Input => StatusKind::Input,
            vitrum_model::SidebarStatus::Working => StatusKind::Working,
            vitrum_model::SidebarStatus::Failed => StatusKind::Failed,
            vitrum_model::SidebarStatus::Ready => StatusKind::Ready,
        }
    }
}

wire_kind! {
    /// One transient layer over the shell.
    ///
    /// The variants of [`crate::state::Layer`] other than `None`, which is the
    /// absence of a layer and is spelled `Facts::layer: None` instead.
    LayerKind {
        /// The keyboard reference.
        Shortcuts => "shortcuts",
        /// A right-click menu on a session row.
        Menu => "menu",
        /// The new-session dialog.
        NewSession => "new-session",
        /// The settings modal.
        Settings => "settings",
        /// The rename dialog.
        Rename => "rename",
        /// Cross-session scrollback search.
        Search => "search",
    }
}

wire_kind! {
    /// One reason a session may want the operator.
    ///
    /// The four independent signals in [`vitrum_proto::Attention`]: `bell`,
    /// `failed`, `waiting` and `idle_ms` past the threshold. Independent rather
    /// than ranked, because a binding that asks "is anything in this workspace
    /// failing" must not also fire on a bell.
    AttentionKind {
        /// The child emitted BEL or an OSC 777 notification.
        Bell => "bell",
        /// The child exited nonzero or was signalled.
        Failed => "failed",
        /// The agent declared it is blocked on the operator.
        Waiting => "waiting",
        /// Silent for longer than the idle threshold.
        Idle => "idle",
    }
}

/// What the predicates need to know about the focused session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedSession {
    /// `vitrum_model::SidebarStatus` for this row, as the sidebar resolved it.
    pub status: StatusKind,
    /// [`vitrum_proto::SessionInfo::unread`].
    pub unread: bool,
    /// [`vitrum_proto::SessionInfo::command`], the program, without its args.
    pub command: String,
}

/// The snapshot a binding is planned against.
///
/// Deliberately plain owned data and not a borrow of the window: the planner is
/// a pure function, and a `Facts` built in a test has to be indistinguishable
/// from one built from a live window or the tests prove nothing about the
/// product.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facts {
    /// The session streaming in this window, from `WindowState::focused`.
    pub focused: Option<FocusedSession>,
    /// The open layer, from `WindowState::layer`. `None` for `Layer::None`.
    pub layer: Option<LayerKind>,
    /// Whether the sidebar is on screen, the inverse of
    /// `WindowState::sidebar_collapsed`.
    pub sidebar_visible: bool,
    /// Every attention signal raised by any session in the viewed workspace.
    pub workspace_attention: std::collections::BTreeSet<AttentionKind>,
}

/// A test a conditional step asks about the current state.
///
/// A closed enum and not an expression language. An operator's settings file is
/// the wrong place for a parser: every predicate here is one field this build
/// can point at, so the settings UI can offer them as a menu and a binding
/// cannot ask a question the product is unable to answer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Predicate {
    /// Any session is focused in this window.
    SessionFocused,
    /// The focused session's sidebar status is this one. False with no focus.
    FocusedStatus { status: StatusKind },
    /// The focused session has output the operator has not seen.
    FocusedUnread,
    /// This layer is the one currently open.
    LayerOpen { layer: LayerKind },
    /// The sidebar is on screen rather than collapsed.
    SidebarVisible,
    /// The focused session's command contains this substring. Substring rather
    /// than equality because the useful question is "is this a claude session",
    /// and the command is a path whose prefix nobody wants to type.
    FocusedCommandContains { text: String },
    /// Some session in the viewed workspace raises this attention signal.
    WorkspaceHasAttention { attention: AttentionKind },
    /// A predicate this build does not know, read from a newer settings file.
    #[serde(other)]
    Unknown,
}

impl Predicate {
    /// Whether this predicate holds, or `None` when this build cannot answer.
    ///
    /// `None` is not "false". A conditional whose question this build does not
    /// understand runs NEITHER branch, because picking the else branch would be
    /// a guess: the operator who wrote the binding on a newer build meant one
    /// specific side, and running the other one is worse than doing nothing.
    #[must_use]
    pub fn holds(&self, facts: &Facts) -> Option<bool> {
        match self {
            Predicate::SessionFocused => Some(facts.focused.is_some()),
            Predicate::FocusedStatus { status } => (*status != StatusKind::Unknown)
                .then(|| facts.focused.as_ref().is_some_and(|s| s.status == *status)),
            Predicate::FocusedUnread => Some(facts.focused.as_ref().is_some_and(|s| s.unread)),
            Predicate::LayerOpen { layer } => {
                (*layer != LayerKind::Unknown).then(|| facts.layer == Some(*layer))
            }
            Predicate::SidebarVisible => Some(facts.sidebar_visible),
            Predicate::FocusedCommandContains { text } => Some(
                facts
                    .focused
                    .as_ref()
                    .is_some_and(|s| s.command.contains(text.as_str())),
            ),
            Predicate::WorkspaceHasAttention { attention } => (*attention
                != AttentionKind::Unknown)
                .then(|| facts.workspace_attention.contains(attention)),
            Predicate::Unknown => None,
        }
    }
}

/// One thing a binding does, in order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "step", rename_all = "kebab-case")]
pub enum Step {
    /// Perform one built-in action, named by [`KeyAction::wire`].
    ///
    /// The wire string and not a serialised `KeyAction`, because that string is
    /// already the vocabulary the rebinding overrides use, and a
    /// second spelling of the same twenty-four actions is a second thing to
    /// keep in agreement. It is also why a future action degrades for free: an
    /// unrecognised name is dropped by [`CustomBinding::plan`].
    Action { action: String },
    /// Send literal bytes to the focused session.
    ///
    /// Escapes are decoded by [`decode_literal`], so the text is exact about
    /// what reaches the pty: `\e` is one 0x1B byte and `\\e` is two printable
    /// characters.
    Text { text: String },
    /// Run one of two step lists, depending on the state.
    When {
        predicate: Predicate,
        #[serde(default)]
        then: Vec<Step>,
        #[serde(default)]
        otherwise: Vec<Step>,
    },
    /// A step this build does not know, read from a newer settings file. Does
    /// nothing, so the rest of the binding still runs.
    #[serde(other)]
    Unknown,
}

impl Step {
    /// A step performing one built-in action.
    #[must_use]
    pub fn action(action: KeyAction) -> Step {
        Step::Action {
            action: action.wire(),
        }
    }

    /// A step sending literal text, escapes and all.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Step {
        Step::Text { text: text.into() }
    }

    /// A conditional step.
    #[must_use]
    pub fn when(predicate: Predicate, then: Vec<Step>, otherwise: Vec<Step>) -> Step {
        Step::When {
            predicate,
            then,
            otherwise,
        }
    }
}

/// One thing the caller performs. The planner produces these; it never acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Dispatch a built-in action exactly as if its chord had been pressed.
    Action(KeyAction),
    /// Write these exact bytes to the focused session's pty.
    Text(Vec<u8>),
}

/// One operator-defined binding: a chord, an ordered list of steps, a label.
///
/// Named to keep it distinct from `crate::ui::settings::Binding`, which is the
/// chord half of a rebinding of a BUILT-IN action. This is the other feature:
/// an action list of the operator's own.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CustomBinding {
    /// What the settings list and the shortcut overlay call this binding.
    pub label: String,
    /// Canonical chord text, e.g. `"Ctrl+Shift+G"`.
    ///
    /// Text and not a struct, so that [`crate::launch::parse_chord`] stays the
    /// one parser for a chord that came from a profile rather than from
    /// [`CHORDS`]. A third chord type in this crate would be a third place the
    /// rule "Ctrl or Alt, exactly one other key" could drift.
    pub chord: String,
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl CustomBinding {
    /// The chord this binding fires on, if the text is one.
    #[must_use]
    pub fn parsed_chord(&self) -> Option<crate::launch::Chord> {
        crate::launch::parse_chord(&self.chord)
    }

    /// What to call this binding on screen and in a message.
    ///
    /// Falls back to the chord, because a binding with no name is common while
    /// one is being built and "did nothing" with no subject names nothing the
    /// operator can go and fix.
    #[must_use]
    pub fn title(&self) -> &str {
        let label = self.label.trim();
        if label.is_empty() { &self.chord } else { label }
    }

    /// Whether this binding is one the product can perform, without needing any
    /// state. For the settings UI, which has to refuse a bad binding at the
    /// point somebody types it rather than at the point they press it.
    pub fn validate(&self) -> Result<(), BindingError> {
        if self.parsed_chord().is_none() {
            return Err(BindingError::BadChord {
                chord: self.chord.clone(),
            });
        }
        check_steps(&self.steps, 0)
    }

    /// The flat, ordered effects this binding produces against `facts`.
    ///
    /// Pure. Nothing has happened when this returns; the caller performs the
    /// list. On `Err` the caller performs NOTHING, which is why the list is
    /// built completely before any of it is handed back.
    pub fn plan(&self, facts: &Facts) -> Result<Vec<Effect>, BindingError> {
        plan_steps(&self.steps, facts)
    }
}

/// Plan a bare step list. Same contract as [`CustomBinding::plan`], for a
/// caller that has steps but no chord yet, such as a settings preview.
pub fn plan_steps(steps: &[Step], facts: &Facts) -> Result<Vec<Effect>, BindingError> {
    let mut out = Vec::new();
    push_steps(steps, facts, 0, &mut out)?;
    Ok(out)
}

fn push_steps(
    steps: &[Step],
    facts: &Facts,
    depth: usize,
    out: &mut Vec<Effect>,
) -> Result<(), BindingError> {
    if depth > MAX_BINDING_DEPTH {
        return Err(BindingError::TooDeep {
            limit: MAX_BINDING_DEPTH,
        });
    }
    for step in steps {
        match step {
            Step::Action { action } => {
                // An unrecognised name is dropped rather than refused: it is
                // the one degradation an older build can make safely, because
                // the action it names cannot exist here to be performed wrongly.
                if let Some(action) = KeyAction::parse(action) {
                    out.push(Effect::Action(action));
                }
            }
            Step::Text { text } => out.push(Effect::Text(decode_literal(text)?)),
            Step::When {
                predicate,
                then,
                otherwise,
            } => match predicate.holds(facts) {
                Some(true) => push_steps(then, facts, depth + 1, out)?,
                Some(false) => push_steps(otherwise, facts, depth + 1, out)?,
                None => {}
            },
            Step::Unknown => {}
        }
    }
    Ok(())
}

/// Depth and escape check over both branches, without needing any state.
fn check_steps(steps: &[Step], depth: usize) -> Result<(), BindingError> {
    if depth > MAX_BINDING_DEPTH {
        return Err(BindingError::TooDeep {
            limit: MAX_BINDING_DEPTH,
        });
    }
    for step in steps {
        match step {
            Step::Text { text } => {
                decode_literal(text)?;
            }
            Step::When {
                then, otherwise, ..
            } => {
                check_steps(then, depth + 1)?;
                check_steps(otherwise, depth + 1)?;
            }
            Step::Action { .. } | Step::Unknown => {}
        }
    }
    Ok(())
}

/// The exact bytes a [`Step::Text`] sends.
///
/// A terminal binding is useless without control characters: the point of
/// sending literal input is `\e` to leave insert mode, `\x03` to interrupt, or a
/// command followed by `\r`. Those cannot be typed into a settings field, so
/// the field is escaped text and this is the one decoder for it.
///
/// The escape set is closed, and an escape outside it is an error rather than a
/// passthrough. `\q` silently meaning `q` is how an operator ends up with a
/// binding that types a stray letter at an agent forever without knowing why.
pub fn decode_literal(text: &str) -> Result<Vec<u8>, BindingError> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.char_indices();
    while let Some((at, ch)) = chars.next() {
        if ch != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        let Some((_, esc)) = chars.next() else {
            return Err(BindingError::UnterminatedEscape { at });
        };
        let byte = match esc {
            '\\' => b'\\',
            'n' => b'\n',
            'r' => b'\r',
            't' => b'\t',
            'e' => 0x1b,
            'a' => 0x07,
            'b' => 0x08,
            'f' => 0x0c,
            '0' => 0x00,
            'x' => {
                let Some((_, hi)) = chars.next() else {
                    return Err(BindingError::UnterminatedEscape { at });
                };
                let Some((_, lo)) = chars.next() else {
                    return Err(BindingError::UnterminatedEscape { at });
                };
                let (Some(high), Some(low)) = (hi.to_digit(16), lo.to_digit(16)) else {
                    return Err(BindingError::BadEscape {
                        at,
                        what: format!("\\x{hi}{lo}"),
                    });
                };
                (high * 16 + low) as u8
            }
            other => {
                return Err(BindingError::BadEscape {
                    at,
                    what: format!("\\{other}"),
                });
            }
        };
        out.push(byte);
    }
    Ok(out)
}

/// Every binding the operator defined, in the order the settings list shows.
///
/// A newtype rather than a bare `Vec`, because the two rules that make the
/// feature survivable belong to the collection and not to one binding: a
/// structurally broken entry in a hand-edited file is dropped instead of
/// failing the whole file, and a binding that cannot be planned is reported by
/// index instead of removed.
///
/// # Precedence against `keyboard.overrides`
///
/// A custom binding WINS over both the built-in table and any rebinding in
/// `crate::state::KeyboardPrefs::overrides`. The two features answer different
/// questions: an override says "put this built-in action on that chord", and a
/// custom binding says "on that chord, do my list instead". If both name one
/// chord, honouring the override would make the custom binding a shortcut the
/// settings panel lists and the product never fires, which is the exact defect
/// this half exists to avoid. The dispatcher therefore resolves the chord an
/// event means through the override table and only then asks [`Self::lookup`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CustomBindings {
    list: Vec<CustomBinding>,
}

impl From<Vec<CustomBinding>> for CustomBindings {
    fn from(list: Vec<CustomBinding>) -> Self {
        CustomBindings { list }
    }
}

impl CustomBindings {
    /// Every binding, in settings order.
    #[must_use]
    pub fn all(&self) -> &[CustomBinding] {
        &self.list
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Append a binding. The settings panel's "Add" button.
    pub fn push(&mut self, binding: CustomBinding) {
        self.list.push(binding);
    }

    /// Drop the binding at this position. False when there is none.
    pub fn remove(&mut self, at: usize) -> bool {
        if at >= self.list.len() {
            return false;
        }
        self.list.remove(at);
        true
    }

    /// The binding at this position, for editing in place.
    pub fn get_mut(&mut self, at: usize) -> Option<&mut CustomBinding> {
        self.list.get_mut(at)
    }

    /// The binding this chord fires, if any.
    ///
    /// First match rather than only match. A hand-edited file can bind one
    /// chord twice, and firing the earlier one beats firing neither; the panel
    /// reports the duplicate rather than the dispatcher refusing to act.
    ///
    /// A binding whose text is not a chord never matches, so a typo in one row
    /// cannot capture keys meant for another.
    #[must_use]
    pub fn lookup(&self, chord: &crate::launch::Chord) -> Option<&CustomBinding> {
        self.list
            .iter()
            .find(|binding| binding.parsed_chord().as_ref() == Some(chord))
    }

    /// The binding that takes over this built-in chord, if any.
    ///
    /// Takes the chord's parts rather than a [`Chord`] so the caller can pass a
    /// chord the operator has already rebound, which is a runtime value and not
    /// a row in [`CHORDS`].
    ///
    /// [`Shift::Any`] is one table entry that fires with shift either way, so
    /// both spellings match it. Requiring the unshifted one would leave a
    /// punctuation chord quietly impossible to shadow.
    #[must_use]
    pub fn shadowing(
        &self,
        key: &str,
        ctrl: bool,
        alt: bool,
        shift: Shift,
    ) -> Option<&CustomBinding> {
        self.list.iter().find(|binding| {
            binding.parsed_chord().is_some_and(|chord| {
                chord.key == key
                    && chord.ctrl == ctrl
                    && chord.alt == alt
                    && match shift {
                        Shift::On => chord.shift,
                        Shift::Off => !chord.shift,
                        Shift::Any => true,
                    }
            })
        })
    }

    /// Every binding this build cannot perform, by position.
    ///
    /// Per binding and not one verdict for the set: a bad escape in row three
    /// is row three's problem, and refusing the whole list would take away the
    /// bindings that do work along with the ability to see why one does not.
    #[must_use]
    pub fn errors(&self) -> Vec<(usize, BindingError)> {
        self.list
            .iter()
            .enumerate()
            .filter_map(|(at, binding)| binding.validate().err().map(|why| (at, why)))
            .collect()
    }
}

impl serde::Serialize for CustomBindings {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.list.serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for CustomBindings {
    /// Reads a JSON array, dropping entries that are not bindings at all.
    ///
    /// Element by element through `serde_json::Value` rather than straight into
    /// `Vec<CustomBinding>`, because a single entry whose `steps` is a string
    /// would otherwise fail the whole array, and the array lives inside the one
    /// settings file: the operator would lose every unrelated preference to one
    /// mistyped row. A dropped entry is reported once and the rest still load.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = <Vec<serde_json::Value> as serde::Deserialize>::deserialize(d)?;
        let mut list = Vec::with_capacity(raw.len());
        for (at, value) in raw.into_iter().enumerate() {
            match serde_json::from_value::<CustomBinding>(value) {
                Ok(binding) => list.push(binding),
                Err(why) => {
                    tracing::warn!("custom binding {at} is not a binding, dropped: {why}");
                }
            }
        }
        Ok(CustomBindings { list })
    }
}

#[cfg(test)]
mod custom_binding_tests;

#[cfg(test)]
mod binding_tests;

#[cfg(test)]
mod tests;

/// The chord a keydown means, for chords bound to a DIGIT.
///
/// The key name for Ctrl+Shift+1 on a US layout is `!`, not `1`. A
/// binding stored as `1` therefore never matches the keystroke it is named
/// after: a shortcut the settings panel displays, the overlay explains, and the
/// product never fires. Digits are the most natural thing to bind a saved
/// command to, so this took out precisely the bindings an operator makes first.
///
/// The rule lives in two places because two matchers exist: [`chord_from_event`]
/// normalises every key event the shell dispatches, and `ui/dialog.rs::chord_of`
/// matches the launcher's own. For a while only the launcher had it, which is
/// why a preset chord worked inside the dialog and did nothing anywhere else.
/// These tests pin the rule so the two cannot drift.
#[cfg(test)]
mod digit_chords;
