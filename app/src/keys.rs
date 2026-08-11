//! Keyboard actions, resolved against the focused surface.
//!
//! # One table, folded once, matched everywhere
//!
//! A key press is turned into an action HERE, against
//! [`ui::settings::live_chords`]: the shipped table, with the operator's
//! rebindings applied and their saved presets appended. Every surface that
//! can receive a key press comes through [`claim`], so a chord cannot work in
//! one place and be dead in another.
//!
//! That is not a hypothetical. The preset half of the fold used to have no
//! caller outside its own tests, so a shortcut an operator saved against a
//! preset was displayed in Settings, listed in the overlay, checked for
//! conflicts, and fired nothing.
//!
//! # Why the table is cached rather than folded per press
//!
//! The fold walks every shipped chord and every saved preset and allocates a
//! `String` per row. Doing that on each key press would put it on the path
//! between a keystroke and the agent seeing it, which is the one latency in
//! this product an operator feels directly. It is folded when the profile
//! changes instead, which is what [`crate::state::live::subscribe_keyboard`]
//! delivers, and a press reads a shared snapshot.
//!
//! # Why the terminal wins by default
//!
//! The pane is where the operator types. A shell that claimed chords freely
//! would eat Ctrl-A, Ctrl-E and Ctrl-K inside readline, so a chord reaches
//! the shell from inside the pane only when its scope is
//! [`Scope::Global`], and a printable key is never a candidate at all.

use super::*;

use std::sync::{Arc, LazyLock, RwLock};

use crate::keymap::{
    AttentionKind, CustomBinding, Effect, Facts, FocusedSession, LayerKind, Scope, Shift,
};
use crate::launch::Chord;
use crate::state::KeyboardPrefs;
use crate::ui::settings::EffectiveChord;

#[cfg(test)]
mod tests;

/// What a key press turned out to mean.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Claim {
    /// A chord in the live table, built-in or preset.
    Action(KeyAction),
    /// A chord the operator bound to their own action list. Carried as the
    /// chord rather than the binding because the binding is looked up again
    /// against the window's own profile at the moment it runs, and a binding
    /// captured here could have been edited in between.
    Custom(Chord),
}

/// Which surface had the key press.
///
/// Not a boolean pair, because the four cases are not independent and the
/// combinations that do not exist should not be representable: focus is in
/// exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    /// The pane. Everything printable belongs to the agent.
    Terminal,
    /// A text field in a dialog, the sidebar filter, the rename box.
    TextInput,
    /// A row in the session list, where the bare arrows traverse.
    SessionList,
    /// The window frame: no text entry and no list row has focus.
    Shell,
}

/// Whether a chord with this scope may fire from this surface.
///
/// Pure and total, so the whole policy is one function a test can enumerate
/// rather than a condition spread over the event handlers. Adding a
/// [`Scope`] variant makes this fail to compile, which is the point.
pub(crate) fn allows(scope: Scope, focus: Focus, layer_open: bool) -> bool {
    match scope {
        // Reserved for modifier combinations no terminal program binds, so it
        // is claimed wherever focus is.
        Scope::Global => true,
        Scope::NotTerminal => focus != Focus::Terminal,
        // The pane passes every printable key to the agent, so it is a text
        // entry for this purpose as much as an input element is.
        Scope::NotTextInput => !matches!(focus, Focus::Terminal | Focus::TextInput),
        Scope::LayerOnly => layer_open,
        // Never while a layer is open: the arrows belong to whatever the
        // layer put on screen, and the list behind it is not what the
        // operator is looking at.
        Scope::SessionList => focus == Focus::SessionList && !layer_open,
    }
}

/// What this key press means, or `None` to pass it on untouched.
///
/// Custom bindings are consulted FIRST, and that is the whole point of
/// rebinding: an operator who put their own action list on Ctrl+B meant it to
/// happen instead of the sidebar toggling, not as well as.
///
/// Pure in its inputs, including the table, so the resolution is testable
/// without a window, a profile on disk or a display.
pub(crate) fn claim(
    pressed: &Chord,
    prefs: &KeyboardPrefs,
    table: &[EffectiveChord],
    focus: Focus,
    layer_open: bool,
) -> Option<Claim> {
    // A custom binding always carries Ctrl or Alt, which `parse_chord`
    // enforces, so it can be treated as global without eating a printable
    // key. A bare letter is refused at the moment it is bound rather than
    // ignored here, so that the operator is told.
    if prefs.custom.lookup(pressed).is_some() {
        return Some(Claim::Custom(pressed.clone()));
    }
    table
        .iter()
        .find(|chord| matches(chord, pressed) && allows(chord.scope, focus, layer_open))
        .map(|chord| Claim::Action(chord.action))
}

/// Whether one live chord is the key press that arrived.
fn matches(chord: &EffectiveChord, pressed: &Chord) -> bool {
    chord.key == pressed.key
        && chord.ctrl == pressed.ctrl
        && chord.alt == pressed.alt
        && match chord.shift {
            Shift::On => pressed.shift,
            Shift::Off => !pressed.shift,
            Shift::Any => true,
        }
}

/// The live chord table and the rebindings behind it.
///
/// One lock rather than two, because the two are folded from the same publish
/// and a reader that took them separately could match a table folded from one
/// profile against the custom bindings of another.
struct Live {
    prefs: Arc<KeyboardPrefs>,
    table: Arc<Vec<EffectiveChord>>,
}

/// The table as the profile last published it.
///
/// Seeded with the shipped table so a key press before the first publish
/// resolves to the shipped chord rather than to nothing. A window whose
/// profile has not been restored yet is still a window somebody can press
/// Ctrl+Shift+N in.
static LIVE: LazyLock<RwLock<Live>> = LazyLock::new(|| {
    let prefs = KeyboardPrefs::default();
    let table = ui::settings::live_chords(&prefs, &[]);
    RwLock::new(Live {
        prefs: Arc::new(prefs),
        table: Arc::new(table),
    })
});

/// Refold the table whenever a rebinding or a saved command changes.
///
/// Held for the life of the process on purpose: dropping the subscription
/// would unsubscribe, and there is exactly one table however many windows are
/// open.
static WATCH: LazyLock<crate::state::live::Subscription> = LazyLock::new(|| {
    crate::state::live::subscribe_keyboard(|prefs, presets| {
        let table = ui::settings::live_chords(prefs, presets);
        if let Ok(mut live) = LIVE.write() {
            live.prefs = Arc::new(prefs.clone());
            live.table = Arc::new(table);
        }
    })
});

/// Start folding the live table. Idempotent, and cheap after the first call.
///
/// Called once per window rather than once per process because a window is
/// the thing that can be opened first; the `LazyLock` makes the second call
/// an atomic load.
pub(crate) fn watch_chords() {
    LazyLock::force(&WATCH);
}

/// The rebindings and the folded table, as one consistent pair.
fn live() -> (Arc<KeyboardPrefs>, Arc<Vec<EffectiveChord>>) {
    match LIVE.read() {
        Ok(live) => (Arc::clone(&live.prefs), Arc::clone(&live.table)),
        // A poisoned lock means a fold panicked, which cannot happen from
        // pure data. Falling back to the shipped table keeps the keyboard
        // working rather than taking the window down with it.
        Err(_) => {
            let prefs = KeyboardPrefs::default();
            let table = ui::settings::live_chords(&prefs, &[]);
            (Arc::new(prefs), Arc::new(table))
        }
    }
}

/// Resolve a key press that arrived at `focus`, against the live table.
pub(crate) fn claim_live(pressed: &Chord, focus: Focus, layer_open: bool) -> Option<Claim> {
    let (prefs, table) = live();
    claim(pressed, &prefs, &table, focus, layer_open)
}

/// Whether the shell takes this key press instead of the agent.
///
/// The pane's key handler calls this before encoding, and sends nothing when
/// it answers `true`. That ordering is the contract: a chord the shell claims
/// must not also reach the child, or Ctrl+Shift+N opens a session AND types
/// an escape sequence into the one that was already there.
///
/// `digit` is the top-row digit the physical key carries, when it carries
/// one. The layout's name for Ctrl+Shift+1 is `!` rather than `1`, so a chord
/// bound to a digit would never match the keystroke it is named after; this
/// is the same rule [`crate::keymap::chord_from_event`] applies to the shell's
/// own key events, and `None` is correct for every key that is not a top-row
/// digit.
///
/// The claim is posted to the window rather than performed here. This runs
/// inside a toolkit callback with no access to the window's state, and the
/// pane and the shell are on the same thread, so the post is a queue push and
/// the action runs on the next turn of the loop.
pub(crate) fn claim_in_pane(
    window: WindowId,
    key: pane::key::Key,
    digit: Option<char>,
    mods: pane::key::Mods,
) -> bool {
    let Some(name) = key_name(key, digit) else {
        return false;
    };
    let pressed = Chord {
        key: name,
        ctrl: mods.ctrl,
        alt: mods.alt,
        shift: mods.shift,
    };
    // The pane only has focus when no layer is open: a layer is a dialog over
    // the frame and takes the keyboard with it.
    let Some(claim) = claim_live(&pressed, Focus::Terminal, false) else {
        return false;
    };
    let Some(tx) = crate::window_sender(window) else {
        tracing::debug!("a pane outlived its window and claimed {pressed:?}");
        return false;
    };
    let event = match claim {
        Claim::Action(action) => ClientEvent::Key { action },
        Claim::Custom(chord) => ClientEvent::CustomKey { chord },
    };
    // A send that fails means the window is closing. The keystroke is lost
    // either way, and passing it to a child that is about to be detached is
    // not better.
    tx.send(event).is_ok()
}

/// The chord name a pane key press is bound under.
///
/// `None` for a key that cannot appear in a binding: the keypad's Enter is
/// the main Enter as far as a chord is concerned, and a character with no
/// lowercase form is passed through unchanged rather than dropped.
fn key_name(key: pane::key::Key, digit: Option<char>) -> Option<String> {
    use pane::key::{Key, Named};

    if let Some(d) = digit {
        return Some(d.to_string());
    }
    Some(match key {
        Key::Char(c) => c.to_lowercase().collect(),
        Key::Named(named) => match named {
            Named::Enter | Named::KeypadEnter => "enter",
            Named::Tab => "tab",
            Named::Backspace => "backspace",
            Named::Escape => "escape",
            Named::Up => "arrowup",
            Named::Down => "arrowdown",
            Named::Right => "arrowright",
            Named::Left => "arrowleft",
            Named::Home => "home",
            Named::End => "end",
            Named::PageUp => "pageup",
            Named::PageDown => "pagedown",
            Named::Insert => "insert",
            Named::Delete => "delete",
            Named::F1 => "f1",
            Named::F2 => "f2",
            Named::F3 => "f3",
            Named::F4 => "f4",
            Named::F5 => "f5",
            Named::F6 => "f6",
            Named::F7 => "f7",
            Named::F8 => "f8",
            Named::F9 => "f9",
            Named::F10 => "f10",
            Named::F11 => "f11",
            Named::F12 => "f12",
        }
        .to_string(),
    })
}

/// The chord one GTK key press means.
///
/// Built from the same two helpers the pane uses, so a chord fires from the
/// shell and from inside the terminal under identical rules. In particular the
/// top-row digit is taken from the unshifted keyval: the layout's name for
/// Ctrl+Shift+1 is `!`, and a chord stored as `1` would never match the
/// keystroke it is named after.
///
/// `None` for a keystroke that names no chord, which is a bare modifier press
/// or a keyval with neither a character nor a name.
pub(crate) fn chord_from_gdk(event: &gtk::gdk::EventKey) -> Option<Chord> {
    let (key, mods) = pane::keyevent::decode_event(event)?;
    Some(Chord {
        key: key_name(key, pane::keyevent::digit_of(event))?,
        ctrl: mods.ctrl,
        alt: mods.alt,
        shift: mods.shift,
    })
}

/// Handle one chord bound to the operator's own action list.
///
/// The binding is looked up again here, against this window's profile, rather
/// than carried from the press: the list can be edited between the two, and
/// running the binding as it was is running a binding that no longer exists.
pub(crate) fn dispatch_custom(cx: &Rc<Ctx>, pressed: &Chord) {
    let found = cx.peek(|st| st.daemon.settings.keyboard.custom.lookup(pressed).cloned());
    match found {
        Some(binding) => run_binding(cx, &binding),
        None => tracing::debug!(
            "{} is no longer bound",
            crate::launch::format_chord(pressed)
        ),
    }
}

/// Plan one custom binding against the live window and perform the result.
///
/// Planned to completion before anything is performed. A binding that cannot be
/// planned does NOTHING and says so: half of a sequence is worse than none of
/// it, because the bytes that did reach the pty run whatever they mean.
fn run_binding(cx: &Rc<Ctx>, binding: &CustomBinding) {
    let effects = match binding.plan(&facts(cx)) {
        Ok(effects) => effects,
        Err(why) => {
            cx.flash(Flash::notice(format!(
                "{} did nothing: {why}",
                binding.title()
            )));
            return;
        }
    };
    for effect in effects {
        match effect {
            Effect::Action(action) => on_key(cx, action),
            Effect::Text(bytes) => send_literal(cx, bytes),
        }
    }
}

/// Write a binding's literal bytes to the focused session's pty.
///
/// The same [`ClientMsg::Input`] frame the terminal grid sends for a keystroke,
/// so a binding's bytes are indistinguishable from typed ones by the time they
/// reach the child. With nothing focused there is no pty to write to, and
/// saying so beats dropping the keystroke silently.
fn send_literal(cx: &Rc<Ctx>, data: Vec<u8>) {
    let Some(session) = cx.peek(|st| st.window.focused) else {
        cx.flash(Flash::notice("Focus a session before sending text to it."));
        return;
    };
    cx.bridge.msg(&ClientMsg::Input { session, data });
}

/// The state snapshot a binding's predicates ask about.
///
/// Built here rather than inside `keymap`, because this is the only place that
/// holds the window: the planner stays a pure function of this value, which is
/// what lets the binding tests build one by hand and prove something real.
fn facts(cx: &Rc<Ctx>) -> Facts {
    cx.peek(|snapshot| {
        let focused = snapshot.window.focused.and_then(|id| {
            let row = snapshot.row(id)?;
            Some(FocusedSession {
                status: row.status().into(),
                unread: row.info.unread,
                command: row.info.command.clone(),
            })
        });
        // Onboarding and What's New have no predicate of their own, and
        // reporting them as "no layer" would make `layer-open` answer a
        // question about a window that has a sheet over it. `Unknown` never
        // matches, which is the honest answer.
        let layer = match &snapshot.window.layer {
            Layer::None => None,
            Layer::Shortcuts => Some(LayerKind::Shortcuts),
            Layer::Menu(_) => Some(LayerKind::Menu),
            Layer::NewSession(_) => Some(LayerKind::NewSession),
            Layer::Settings(_) => Some(LayerKind::Settings),
            Layer::Rename(_) => Some(LayerKind::Rename),
            Layer::Search => Some(LayerKind::Search),
            Layer::Onboarding | Layer::WhatsNew => Some(LayerKind::Unknown),
        };
        let mut workspace_attention = std::collections::BTreeSet::new();
        for row in snapshot.daemon.workspace_rows(snapshot.window.workspace) {
            let raised = &row.info.attention;
            if raised.bell {
                workspace_attention.insert(AttentionKind::Bell);
            }
            if raised.failed {
                workspace_attention.insert(AttentionKind::Failed);
            }
            if raised.waiting == Some(true) {
                workspace_attention.insert(AttentionKind::Waiting);
            }
            if raised.idle_ms >= vitrum_proto::IDLE_ATTENTION_MS {
                workspace_attention.insert(AttentionKind::Idle);
            }
        }
        Facts {
            focused,
            layer,
            sidebar_visible: !snapshot.window.sidebar_collapsed,
            workspace_attention,
        }
    })
}

/// Perform one keyboard action.
///
/// Split out of the event match so the whole map is one readable block and so
/// every arm ends in the same reconcile. A missing reconcile is the classic
/// bug here: focus moves, the strip repaints, and the terminal keeps streaming
/// the previous session.
pub(crate) fn on_key(cx: &Rc<Ctx>, action: KeyAction) {
    match action {
        KeyAction::NextTab => cx.edit(|st| st.cycle(1)),
        KeyAction::PrevTab => cx.edit(|st| st.cycle(-1)),
        KeyAction::SelectTab(i) => cx.edit(|st| st.focus_index(i)),
        KeyAction::CloseTab => {
            if let Some(id) = cx.peek(|st| st.window.focused) {
                cx.edit(|st| st.close_tab(id));
            }
        }
        // Shared with the row's context menu, so the keyboard and the pointer
        // cannot drift apart.
        KeyAction::DuplicateSession => match cx.peek(|st| st.window.focused) {
            Some(id) => duplicate_session(cx, id),
            None => cx.flash(Flash::notice("Focus a session before duplicating it.")),
        },
        KeyAction::ToggleSidebar => {
            cx.edit(|st| st.window.sidebar_collapsed = !st.window.sidebar_collapsed);
        }
        KeyAction::FocusSearch => {
            // Expanding first: focusing a field inside a collapsed rail that
            // hides the input would put the caret nowhere.
            cx.edit(|st| st.window.sidebar_collapsed = false);
            cx.shell.focus("rg-filter");
        }
        KeyAction::OpenSearch => toggle_layer(cx, Layer::Search),
        KeyAction::FocusSidebar => {
            cx.edit(|st| st.window.sidebar_collapsed = false);
            let model_clock = tick().model;
            let target = cx.peek(|st| {
                st.window
                    .focused
                    .or_else(|| st.visible_ids(model_clock).first().copied())
            });
            match target {
                Some(id) => cx.shell.focus(&ui::sidebar::row_id(id)),
                // No rows to land on. Focusing the list itself still moves the
                // caret out of the terminal, which is the point.
                None => cx.shell.focus("rg-sidebar-body"),
            }
        }
        KeyAction::NewSession => open_new_session(cx, None),
        KeyAction::LaunchPreset(id) => launch_preset(cx, id),
        KeyAction::RenameSession => {
            let seed = cx.peek(|st| {
                st.window
                    .focused
                    .and_then(|id| st.session(id).map(|s| (id, s.title.clone())))
            });
            match seed {
                Some((session, title)) if cx.peek(UiState::server_ready) => {
                    cx.edit(|st| st.window.layer = Layer::Rename(RenameSeed { session, title }));
                }
                Some(_) => cx.flash(Flash::notice(
                    "Renaming needs the daemon; this window is not connected.",
                )),
                None => cx.flash(Flash::notice("Focus a session before renaming it.")),
            }
        }
        KeyAction::CloseSession => match cx.peek(|st| st.window.focused) {
            Some(id) => request_terminate(cx, &[id]),
            None => cx.flash(Flash::notice("No session is focused, so nothing to close.")),
        },
        KeyAction::NextAttention => jump_to_attention(cx, Direction::Next),
        KeyAction::PrevAttention => jump_to_attention(cx, Direction::Previous),
        KeyAction::NextRow => step_rows(cx, Direction::Next, false),
        KeyAction::PrevRow => step_rows(cx, Direction::Previous, false),
        KeyAction::ExtendDown => step_rows(cx, Direction::Next, true),
        KeyAction::ExtendUp => step_rows(cx, Direction::Previous, true),
        KeyAction::SelectAllRows => {
            let at = tick().model;
            cx.edit(|st| st.select_all_visible(at));
        }
        KeyAction::ToggleShortcuts => toggle_layer(cx, Layer::Shortcuts),
        KeyAction::Dismiss => dismiss(cx),
    }
    reconcile(cx);
}

/// Focus the next or previous session that wants the operator.
///
/// This is the answer to twenty agents. The inbox order is deliberately static
/// so rows never move under the cursor; the cost of that is that the one
/// session waiting on an approval could be anywhere in the list, and this key
/// is what pays it. [`vitrum_model::traversal::adjacent_matching`] guarantees
/// the jump never returns the row it started on, so pressing it with a single
/// blocked row reports "nowhere else to go" rather than pretending to move.
///
/// Reveals and scrolls, because moving focus to a row thirty rows below the
/// fold with no scroll is indistinguishable from the shortcut doing nothing.
/// An empty queue says so rather than silently no-opping, which is the same
/// failure in a different disguise.
pub(crate) fn jump_to_attention(cx: &Rc<Ctx>, direction: Direction) {
    let tick = tick();
    let target = cx.peek(|st| st.attention_target(tick.model, direction));
    let Some(id) = target else {
        let waiting = cx.peek(|st| st.attention_count(tick.model));
        cx.flash(Flash::notice(if waiting == 0 {
            "No session needs you right now.".to_string()
        } else {
            format!("{waiting} session needs you, and you are already on it.")
        }));
        return;
    };
    cx.edit(|st| {
        st.window.sidebar_collapsed = false;
        st.reveal(id, tick.model);
        st.open(id, tick.now_ms);
    });
    cx.shell.focus(&ui::sidebar::row_id(id));
}

/// Move focus one row through the visible list, optionally extending the
/// selection instead of replacing it.
///
/// Clamps at the ends rather than wrapping. Holding the down arrow at the
/// bottom of a twenty-row list must stop, not spin back to the top: this is
/// list traversal, not a queue, and the queue has its own key.
///
/// Always asks the shell to move focus to the row, because focus landing on a
/// row below the fold is what scrolls it into view. A traversal with no scroll
/// is indistinguishable from the key doing nothing.
pub(crate) fn step_rows(cx: &Rc<Ctx>, direction: Direction, extend: bool) {
    let model_clock = tick().model;
    let Some(id) = cx.peek(|st| st.step_target(model_clock, direction)) else {
        return;
    };
    cx.edit(|st| {
        // Selection is instant and never waits on the daemon: stepping through
        // the list must not attach and detach a PTY per keypress. The focused
        // row changes only on a plain step, and even then the terminal follows
        // through the normal reconcile below.
        st.click_row(
            id,
            if extend {
                state::Click::Range
            } else {
                state::Click::Plain
            },
            model_clock,
        );
        if !extend {
            st.window.focused = Some(id);
        }
    });
    cx.shell.focus(&ui::sidebar::row_id(id));
}
