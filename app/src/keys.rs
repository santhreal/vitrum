//! Keyboard actions, resolved against the focused row.

use super::*;

use crate::keymap::{AttentionKind, CustomBinding, Effect, Facts, FocusedSession, LayerKind};

/// The wire prefix the bridge uses for a chord the operator defined.
///
/// A custom binding needs the webview to report the keystroke, and the webview
/// only reports chords in the keymap table. Rather than teaching that table a
/// second shape, a custom binding appears in it as an ordinary entry whose
/// action is this prefix followed by the chord text, which is self-describing:
/// a table left over from before a reorder still names the binding it meant,
/// where a positional index would fire the wrong one.
pub(crate) const CUSTOM_ACTION_PREFIX: &str = "custom:";

/// Handle one chord the bridge reported.
///
/// The custom bindings are consulted FIRST. That is the whole point of
/// rebinding: an operator who put their own action list on Ctrl+B meant it to
/// happen instead of the sidebar toggling, not as well as.
pub(crate) fn dispatch_key(
    wire: &str,
    bridge: Bridge,
    st: Signal<UiState>,
    attached: Signal<Option<SessionId>>,
    opts: Options,
    pending_terminate: Signal<Vec<SessionId>>,
    pending_open: Signal<Option<PendingLaunch>>,
) {
    if let Some(text) = wire.strip_prefix(CUSTOM_ACTION_PREFIX) {
        let found = launch::parse_chord(text).and_then(|chord| {
            st.peek()
                .daemon
                .settings
                .keyboard
                .custom
                .lookup(&chord)
                .cloned()
        });
        match found {
            Some(binding) => run_binding(
                &binding,
                bridge,
                st,
                attached,
                opts,
                pending_terminate,
                pending_open,
            ),
            None => tracing::warn!("bridge sent custom chord {text:?}, which is not bound"),
        }
        return;
    }

    let Some(action) = KeyAction::parse(wire) else {
        tracing::warn!("bridge sent unknown chord {wire:?}");
        return;
    };

    // The built-in chord fired, so a custom binding on the same chord has to be
    // caught here as well: the operator's list must win even when the webview
    // matched the built-in table entry first.
    if let Some(binding) = shadowing(st, action) {
        run_binding(
            &binding,
            bridge,
            st,
            attached,
            opts,
            pending_terminate,
            pending_open,
        );
        return;
    }

    on_key(
        action,
        bridge,
        st,
        attached,
        opts,
        pending_terminate,
        pending_open,
    );
}

/// The custom binding sitting on this action's live chord, if there is one.
///
/// Resolved through [`ui::settings::effective_chords`] rather than the raw
/// table, so a chord the operator has already rebound is still the chord a
/// custom binding shadows. Cloned, because every effect below re-enters code
/// that takes `st.write()` and a live read guard there panics.
fn shadowing(st: Signal<UiState>, action: KeyAction) -> Option<CustomBinding> {
    let snapshot = st.peek();
    let prefs = &snapshot.daemon.settings.keyboard;
    if prefs.custom.is_empty() {
        return None;
    }
    let chord = ui::settings::effective_chords(prefs)
        .into_iter()
        .find(|chord| chord.action == action)?;
    prefs
        .custom
        .shadowing(&chord.key, chord.ctrl, chord.alt, chord.shift)
        .cloned()
}

/// Plan one custom binding against the live window and perform the result.
///
/// Planned to completion before anything is performed. A binding that cannot be
/// planned does NOTHING and says so: half of a sequence is worse than none of
/// it, because the bytes that did reach the pty run whatever they mean.
fn run_binding(
    binding: &CustomBinding,
    bridge: Bridge,
    mut st: Signal<UiState>,
    attached: Signal<Option<SessionId>>,
    opts: Options,
    pending_terminate: Signal<Vec<SessionId>>,
    pending_open: Signal<Option<PendingLaunch>>,
) {
    let effects = match binding.plan(&facts(st)) {
        Ok(effects) => effects,
        Err(why) => {
            st.write().window.flash = Some(Flash::notice(format!(
                "{} did nothing: {why}",
                binding.title()
            )));
            return;
        }
    };
    for effect in effects {
        match effect {
            Effect::Action(action) => {
                on_key(
                    action,
                    bridge,
                    st,
                    attached,
                    opts,
                    pending_terminate,
                    pending_open,
                );
            }
            Effect::Text(bytes) => send_literal(bridge, st, bytes),
        }
    }
}

/// Write a binding's literal bytes to the focused session's pty.
///
/// The same [`ClientMsg::Input`] frame the terminal grid sends for a keystroke,
/// so a binding's bytes are indistinguishable from typed ones by the time they
/// reach the child. With nothing focused there is no pty to write to, and
/// saying so beats dropping the keystroke silently.
fn send_literal(bridge: Bridge, mut st: Signal<UiState>, data: Vec<u8>) {
    let Some(session) = st.peek().window.focused else {
        st.write().window.flash = Some(Flash::notice("Focus a session before sending text to it."));
        return;
    };
    bridge.msg(&ClientMsg::Input { session, data });
}

/// The state snapshot a binding's predicates ask about.
///
/// Built here rather than inside `keymap`, because this is the only place that
/// holds the window: the planner stays a pure function of this value, which is
/// what lets the binding tests build one by hand and prove something real.
fn facts(st: Signal<UiState>) -> Facts {
    let snapshot = st.peek();
    let focused = snapshot.window.focused.and_then(|id| {
        let row = snapshot.row(id)?;
        Some(FocusedSession {
            status: row.status().into(),
            unread: row.info.unread,
            command: row.info.command.clone(),
        })
    });
    // Onboarding and What's New have no predicate of their own, and reporting
    // them as "no layer" would make `layer-open` answer a question about a
    // window that has a sheet over it. `Unknown` never matches, which is the
    // honest answer.
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
}

/// Perform one keyboard action.
///
/// Split out of the event match so the whole map is one readable block and so
/// every arm ends in the same reconcile. A missing reconcile is the classic
/// bug here: focus moves, the strip repaints, and the terminal keeps streaming
/// the previous session.
pub(crate) fn on_key(
    action: KeyAction,
    bridge: Bridge,
    mut st: Signal<UiState>,
    attached: Signal<Option<SessionId>>,
    opts: Options,
    pending_terminate: Signal<Vec<SessionId>>,
    pending_open: Signal<Option<PendingLaunch>>,
) {
    match action {
        KeyAction::NextTab => st.write().cycle(1),
        KeyAction::PrevTab => st.write().cycle(-1),
        KeyAction::SelectTab(i) => st.write().focus_index(i),
        KeyAction::CloseTab => {
            let focused = st.peek().window.focused;
            if let Some(id) = focused {
                st.write().close_tab(id);
            }
        }
        // Shared with the row's context menu, so the keyboard and the pointer
        // cannot drift apart. `focused` is peeked into a local BEFORE the
        // match: a scrutinee's temporary lives to the end of the match, and an
        // `st.write()` inside an arm while that read guard is still live
        // panics. CloseSession below is written the same way for the same
        // reason.
        KeyAction::DuplicateSession => {
            let focused = st.peek().window.focused;
            match focused {
                Some(id) => duplicate_session(bridge, st, id),
                None => {
                    st.write().window.flash =
                        Some(Flash::notice("Focus a session before duplicating it."))
                }
            }
        }
        KeyAction::ToggleSidebar => {
            let mut w = st.write();
            w.window.sidebar_collapsed = !w.window.sidebar_collapsed;
        }
        KeyAction::FocusSearch => {
            // Expanding first: focusing a field inside a 48px rail that hides
            // the input would put the caret nowhere.
            st.write().window.sidebar_collapsed = false;
            bridge.cmd(BridgeCmd::FocusDom {
                selector: "#rg-filter".to_string(),
            });
        }
        KeyAction::OpenSearch => toggle_layer(st, Layer::Search),
        KeyAction::FocusSidebar => {
            st.write().window.sidebar_collapsed = false;
            let model_clock = tick().model;
            let target = st
                .peek()
                .window
                .focused
                .or_else(|| st.peek().visible_ids(model_clock).first().copied());
            let selector = match target {
                Some(id) => format!("#{}", ui::sidebar::row_id(id)),
                // No rows to land on. Focusing the list container still moves
                // the caret out of the terminal, which is the point.
                None => "#rg-sidebar-body".to_string(),
            };
            bridge.cmd(BridgeCmd::FocusDom { selector });
        }
        KeyAction::NewSession => open_new_session(st, None),
        KeyAction::LaunchPreset(id) => launch_preset(bridge, st, pending_open, id),
        KeyAction::RenameSession => {
            let seed = st
                .peek()
                .window
                .focused
                .and_then(|id| st.peek().session(id).map(|s| (id, s.title.clone())));
            match seed {
                Some((session, title)) if st.peek().server_ready() => {
                    st.write().window.layer = Layer::Rename(RenameSeed { session, title });
                }
                Some(_) => {
                    st.write().window.flash = Some(Flash::notice(
                        "Renaming needs the daemon; this window is not connected.",
                    ));
                }
                None => {
                    st.write().window.flash =
                        Some(Flash::notice("Focus a session before renaming it."));
                }
            }
        }
        KeyAction::CloseSession => {
            let focused = st.peek().window.focused;
            match focused {
                Some(id) => request_terminate(bridge, st, &[id], opts, pending_terminate),
                None => {
                    st.write().window.flash =
                        Some(Flash::notice("No session is focused, so nothing to close."))
                }
            }
        }
        KeyAction::NextAttention => jump_to_attention(bridge, st, Direction::Next),
        KeyAction::PrevAttention => jump_to_attention(bridge, st, Direction::Previous),
        KeyAction::NextRow => step_rows(bridge, st, Direction::Next, false),
        KeyAction::PrevRow => step_rows(bridge, st, Direction::Previous, false),
        KeyAction::ExtendDown => step_rows(bridge, st, Direction::Next, true),
        KeyAction::ExtendUp => step_rows(bridge, st, Direction::Previous, true),
        KeyAction::SelectAllRows => {
            st.write().select_all_visible(tick().model);
        }
        KeyAction::ToggleShortcuts => toggle_layer(st, Layer::Shortcuts),
        KeyAction::Dismiss => dismiss(st),
    }
    reconcile(bridge, st, attached, opts);
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
pub(crate) fn jump_to_attention(bridge: Bridge, mut st: Signal<UiState>, direction: Direction) {
    let tick = tick();
    let target = st.peek().attention_target(tick.model, direction);
    let Some(id) = target else {
        let waiting = st.peek().attention_count(tick.model);
        st.write().window.flash = Some(Flash::notice(if waiting == 0 {
            "No session needs you right now.".to_string()
        } else {
            format!("{waiting} session needs you, and you are already on it.")
        }));
        return;
    };
    {
        let mut w = st.write();
        w.window.sidebar_collapsed = false;
        w.reveal(id, tick.model);
        w.open(id, tick.now_ms);
    }
    bridge.cmd(BridgeCmd::FocusDom {
        selector: format!("#{}", ui::sidebar::row_id(id)),
    });
}

/// Move focus one row through the visible list, optionally extending the
/// selection instead of replacing it.
///
/// Clamps at the ends rather than wrapping. Holding the down arrow at the
/// bottom of a twenty-row list must stop, not spin back to the top: this is
/// list traversal, not a queue, and the queue has its own key.
///
/// Always asks the bridge to scroll the row into view. Focus moving to a row
/// below the fold with no scroll is indistinguishable from the key doing
/// nothing, which is defect 7's other half.
pub(crate) fn step_rows(
    bridge: Bridge,
    mut st: Signal<UiState>,
    direction: Direction,
    extend: bool,
) {
    let model_clock = tick().model;
    let Some(id) = st.peek().step_target(model_clock, direction) else {
        return;
    };
    {
        let mut w = st.write();
        // Selection is instant and never waits on the daemon: stepping through
        // the list must not attach and detach a PTY per keypress. The focused
        // row changes only on a plain step, and even then the terminal follows
        // through the normal reconcile below.
        w.click_row(
            id,
            if extend {
                state::Click::Range
            } else {
                state::Click::Plain
            },
            model_clock,
        );
        if !extend {
            w.window.focused = Some(id);
        }
    }
    bridge.cmd(BridgeCmd::FocusDom {
        selector: format!("#{}", ui::sidebar::row_id(id)),
    });
}
/// Frame-aligned microtask debouncer for keyboard events.
///
/// Prevents main-thread UI layout thrashing during rapid key repeat by coalescing
/// sub-frame repeat events within a ~16ms (60 FPS) frame window into frame-aligned execution batches.
#[derive(Debug, Clone)]
pub struct FrameKeyDebouncer {
    pub frame_budget_ms: u64,
    pub last_processed_ms: u64,
    pub pending_action: Option<KeyAction>,
    pub coalesced_count: u32,
}

impl FrameKeyDebouncer {
    pub const DEFAULT_FRAME_BUDGET_MS: u64 = 16;

    pub fn new(frame_budget_ms: u64) -> Self {
        Self {
            frame_budget_ms,
            last_processed_ms: 0,
            pending_action: None,
            coalesced_count: 0,
        }
    }

    /// Evaluates whether an incoming key action should be processed immediately
    /// or debounced within the current frame window.
    pub fn process(&mut self, action: KeyAction, timestamp_ms: u64) -> Option<DebouncedKeyAction> {
        let elapsed = timestamp_ms.saturating_sub(self.last_processed_ms);
        if elapsed < self.frame_budget_ms && self.pending_action == Some(action) {
            self.coalesced_count += 1;
            None
        } else {
            let previous_coalesced = self.coalesced_count;
            self.last_processed_ms = timestamp_ms;
            self.pending_action = Some(action);
            self.coalesced_count = 1;
            Some(DebouncedKeyAction {
                action,
                coalesced_repeat_count: previous_coalesced.max(1),
            })
        }
    }

    pub fn flush(&mut self) -> Option<DebouncedKeyAction> {
        if let Some(action) = self.pending_action.take() {
            let count = self.coalesced_count;
            self.coalesced_count = 0;
            Some(DebouncedKeyAction {
                action,
                coalesced_repeat_count: count.max(1),
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebouncedKeyAction {
    pub action: KeyAction,
    pub coalesced_repeat_count: u32,
}
