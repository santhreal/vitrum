//! The keyboard page: built-in shortcuts, and the operator's own bindings.
//!
//! Two halves of one question, on one page because they compete for the same
//! keys. The top half moves a built-in action to another chord. The bottom half
//! puts an ordered list of the operator's own steps on a chord, which is what
//! makes a terminal binding useful: send `\x03`, then `\e`, then a command and a
//! carriage return, and only when the focused session is actually working.
//!
//! Custom bindings WIN. A chord in the bottom half is taken away from whatever
//! built-in owns it, and the top half says so on the row it affects rather than
//! leaving the operator to discover it by pressing the key. The alternative,
//! honouring the built-in, is a binding this page lists and the product never
//! fires.
//!
//! Everything the operator types is validated as they type it:
//! [`crate::keymap::CustomBinding::validate`] is the same check the dispatcher
//! runs, so a row with no error here cannot be inert at the keyboard, and a row
//! with one says exactly which escape or which chord is wrong.

use dioxus::prelude::*;

use crate::keymap::{
    AttentionKind, BindingError, CustomBinding, KeyAction, LayerKind, MAX_BINDING_DEPTH, Predicate,
    StatusKind, Step,
};
use crate::launch;
use crate::state::UiState;
use crate::ui::settings::{
    BINDABLE_KEYS, Binding, action_label, chord_conflict, clear_override, commit, effective_chords,
    pretty_key, rebindable, set_override,
};

/// Which branch of a conditional a step sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Branch {
    Then,
    Otherwise,
}

/// One descent into a conditional, on the way to the list a step lives in.
///
/// A path of these addresses a nested step list without holding a borrow of the
/// binding, which is what lets an event handler describe an edit it will apply
/// later against whatever the signal holds by then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hop {
    /// Index of the [`Step::When`] to descend into.
    pub at: usize,
    pub branch: Branch,
}

/// The step list this path addresses, or `None` when the path no longer fits.
///
/// `None` rather than a panic: a path is built during a render and applied on a
/// later event, and the list it named can have been edited in between. Dropping
/// the edit is the only safe answer, because applying it to whatever now sits at
/// that index would rewrite a step the operator never pointed at.
pub fn list_at<'a>(steps: &'a mut Vec<Step>, path: &[Hop]) -> Option<&'a mut Vec<Step>> {
    let mut list = steps;
    for hop in path {
        let Step::When {
            then, otherwise, ..
        } = list.get_mut(hop.at)?
        else {
            return None;
        };
        list = match hop.branch {
            Branch::Then => then,
            Branch::Otherwise => otherwise,
        };
    }
    Some(list)
}

/// Every predicate the menu offers, with the sentence it reads as.
///
/// `Unknown` is deliberately absent. It exists so a binding written on a newer
/// build survives a downgrade; offering it would let somebody choose a question
/// that can never be answered, which is a binding that silently does nothing.
pub const PREDICATE_KINDS: &[(&str, &str)] = &[
    ("session-focused", "a session is focused"),
    ("focused-status", "the focused session is"),
    ("focused-unread", "the focused session has unread output"),
    ("layer-open", "this panel is open"),
    ("sidebar-visible", "the sidebar is showing"),
    ("focused-command-contains", "the focused command contains"),
    ("workspace-has-attention", "any session here raises"),
];

/// The name a predicate is chosen by in the menu.
#[must_use]
pub fn predicate_wire(predicate: &Predicate) -> &'static str {
    match predicate {
        Predicate::SessionFocused => "session-focused",
        Predicate::FocusedStatus { .. } => "focused-status",
        Predicate::FocusedUnread => "focused-unread",
        Predicate::LayerOpen { .. } => "layer-open",
        Predicate::SidebarVisible => "sidebar-visible",
        Predicate::FocusedCommandContains { .. } => "focused-command-contains",
        Predicate::WorkspaceHasAttention { .. } => "workspace-has-attention",
        Predicate::Unknown => "unknown",
    }
}

/// A predicate of this kind, with a payload the operator can then narrow.
///
/// Seeded with a real value rather than an empty one wherever a choice exists,
/// so switching the menu never leaves a step in a state the planner treats as
/// unanswerable.
#[must_use]
pub fn predicate_of(wire: &str) -> Predicate {
    match wire {
        "session-focused" => Predicate::SessionFocused,
        "focused-status" => Predicate::FocusedStatus {
            status: StatusKind::Approval,
        },
        "focused-unread" => Predicate::FocusedUnread,
        "layer-open" => Predicate::LayerOpen {
            layer: LayerKind::Shortcuts,
        },
        "sidebar-visible" => Predicate::SidebarVisible,
        "focused-command-contains" => Predicate::FocusedCommandContains {
            text: String::new(),
        },
        "workspace-has-attention" => Predicate::WorkspaceHasAttention {
            attention: AttentionKind::Failed,
        },
        _ => Predicate::Unknown,
    }
}

/// What a status reads as in a sentence.
#[must_use]
pub fn status_label(status: StatusKind) -> &'static str {
    match status {
        StatusKind::Approval => "waiting for approval",
        StatusKind::Input => "waiting for input",
        StatusKind::Working => "working",
        StatusKind::Failed => "failed",
        StatusKind::Ready => "ready",
        StatusKind::Unknown => "a state this build does not know",
    }
}

/// What a layer reads as in a sentence.
#[must_use]
pub fn layer_label(layer: LayerKind) -> &'static str {
    match layer {
        LayerKind::Shortcuts => "the shortcut overlay",
        LayerKind::Menu => "a row menu",
        LayerKind::NewSession => "the new-session dialog",
        LayerKind::Settings => "settings",
        LayerKind::Rename => "the rename dialog",
        LayerKind::Search => "scrollback search",
        LayerKind::Unknown => "a panel this build does not know",
    }
}

/// What an attention signal reads as in a sentence.
#[must_use]
pub fn attention_label(attention: AttentionKind) -> &'static str {
    match attention {
        AttentionKind::Bell => "a bell",
        AttentionKind::Failed => "a failure",
        AttentionKind::Waiting => "a block on you",
        AttentionKind::Idle => "going quiet",
        AttentionKind::Unknown => "a signal this build does not know",
    }
}

/// The sentence under a broken row.
#[must_use]
pub fn fault_sentence(why: &BindingError) -> String {
    format!("This binding does nothing until you fix it: {why}")
}

/// Mutate one custom binding, then persist and apply.
///
/// A local twin of the one in `ui::settings`, which is private to that module.
/// Same guarantee for the same reason: every control on this page goes through
/// one function, so "takes effect immediately and survives a restart" is true by
/// construction rather than by each handler remembering both halves.
fn edit_binding(mut state: Signal<UiState>, at: usize, change: impl FnOnce(&mut CustomBinding)) {
    {
        let mut write = state.write();
        let Some(binding) = write.daemon.settings.keyboard.custom.get_mut(at) else {
            return;
        };
        change(binding);
    }
    commit(&state.peek());
}

/// Mutate the step list one path addresses.
fn edit_steps(
    state: Signal<UiState>,
    at: usize,
    path: &[Hop],
    change: impl FnOnce(&mut Vec<Step>),
) {
    edit_binding(state, at, |binding| {
        if let Some(list) = list_at(&mut binding.steps, path) {
            change(list);
        }
    });
}

#[derive(Props, Clone, PartialEq)]
pub struct KeybindsProps {
    pub state: Signal<UiState>,
}

/// The keyboard page.
#[component]
pub fn Keybinds(props: KeybindsProps) -> Element {
    let mut state = props.state;
    let prefs = state.read().daemon.settings.keyboard.clone();
    let effective = effective_chords(&prefs);
    let faults = prefs.custom.errors();

    // Which row is open for editing, and the chord being assembled. Local: a
    // half-typed chord is not state another window needs, and writing it to the
    // one global signal re-diffs the sidebar on every modifier click.
    let mut editing = use_signal(|| None::<KeyAction>);
    let mut recording = use_signal(|| None::<usize>);
    let mut draft = use_signal(Binding::default);

    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "Shortcuts" }
            span { class: "rg-field__desc",
                "Ctrl is the primary modifier on every platform, macOS included, because Cmd+Tab \
                 never reaches an application. A chord you claim below, under your own bindings, \
                 is taken away from the action listed here."
            }
        }

        for (action, what) in rebindable() {
            {
                let current = effective.iter().find(|chord| chord.action == action).cloned();
                let taken = current.as_ref().and_then(|chord| {
                    prefs
                        .custom
                        .shadowing(&chord.key, chord.ctrl, chord.alt, chord.shift)
                        .map(|binding| binding.title().to_string())
                });
                let open = editing() == Some(action);
                rsx! {
                    div { class: "rg-field rg-field--chord", key: "{action.wire()}",
                        span { class: "rg-field__label", "{what}" }
                        span { class: "rg-keys__chord",
                            kbd {
                                if let Some(chord) = current.as_ref() { "{chord.rendered()}" } else { "unbound" }
                            }
                        }
                        span { class: "rg-field__control",
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                onclick: {
                                    let seed = current
                                        .as_ref()
                                        .map(crate::ui::settings::EffectiveChord::binding)
                                        .unwrap_or_default();
                                    move |_| {
                                        recording.set(None);
                                        if open {
                                            editing.set(None);
                                        } else {
                                            draft.set(seed.clone());
                                            editing.set(Some(action));
                                        }
                                    }
                                },
                                if open { "Cancel" } else { "Change" }
                            }
                            if current.as_ref().is_some_and(|chord| chord.rebound) {
                                button {
                                    class: "rg-btn",
                                    r#type: "button",
                                    onclick: move |_| {
                                        editing.set(None);
                                        let mut write = state.write();
                                        clear_override(&mut write.daemon.settings.keyboard, action);
                                        drop(write);
                                        commit(&state.peek());
                                    },
                                    "Reset"
                                }
                            }
                        }

                        if let Some(label) = taken {
                            span { class: "rg-field__hint",
                                "Your binding \u{201c}{label}\u{201d} owns this chord, so this action \
                                 does not fire. Move one of them."
                            }
                        }

                        if open {
                            ChordRecorder {
                                draft,
                                conflict: chord_conflict(&effective, &draft(), action),
                                on_save: move |binding: Binding| {
                                    editing.set(None);
                                    let mut write = state.write();
                                    set_override(&mut write.daemon.settings.keyboard, action, &binding);
                                    drop(write);
                                    commit(&state.peek());
                                },
                            }
                        }
                    }
                }
            }
        }

        div { class: "rg-field rg-keybinds__heading",
            span { class: "rg-field__label", "Your own bindings" }
            span { class: "rg-field__desc",
                "An ordered list of steps on one chord. Text is sent to the focused session exactly \
                 as written, with \\e for escape, \\r for return, \\x03 for interrupt and \\\\ for a \
                 literal backslash. A condition runs one of two lists depending on what the window \
                 is doing when you press the key."
            }
            span { class: "rg-field__control",
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    onclick: move |_| {
                        let mut write = state.write();
                        write.daemon.settings.keyboard.custom.push(CustomBinding::default());
                        drop(write);
                        commit(&state.peek());
                    },
                    "Add a binding"
                }
            }
            if !faults.is_empty() {
                span { class: "rg-field__hint",
                    if faults.len() == 1 { "1 of your " } else { "{faults.len()} of your " }
                    if prefs.custom.len() == 1 { "1 binding does nothing until you fix it." } else { "{prefs.custom.len()} bindings do nothing until you fix them." }
                }
            }
        }

        if prefs.custom.is_empty() {
            div { class: "rg-sheet__note",
                "You have no bindings of your own yet. One is a chord, a name, and the steps it runs."
            }
        }

        for (at, binding) in prefs.custom.all().iter().enumerate() {
            {
                let fault = faults.iter().find(|(row, _)| *row == at).map(|(_, why)| why.clone());
                let open = recording() == Some(at);
                let chord_text = binding.chord.clone();
                let label = binding.label.clone();
                let steps = binding.steps.clone();
                rsx! {
                    div { class: "rg-field rg-field--keybind", key: "{at}",
                        span { class: "rg-field__control",
                            input {
                                class: "rg-field__input rg-field__input--prose",
                                aria_label: "Name",
                                placeholder: "Name this binding",
                                initial_value: "{label}",
                                onchange: move |e| {
                                    let text = e.value();
                                    edit_binding(state, at, |binding| binding.label = text);
                                },
                            }
                            span { class: "rg-keys__chord",
                                kbd {
                                    if chord_text.is_empty() { "unbound" } else { "{chord_text}" }
                                }
                            }
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                onclick: {
                                    let seed = launch::parse_chord(&chord_text)
                                        .map(|chord| Binding {
                                            key: chord.key,
                                            ctrl: chord.ctrl,
                                            alt: chord.alt,
                                            shift: chord.shift,
                                        })
                                        .unwrap_or_default();
                                    move |_| {
                                        editing.set(None);
                                        if open {
                                            recording.set(None);
                                        } else {
                                            draft.set(seed.clone());
                                            recording.set(Some(at));
                                        }
                                    }
                                },
                                if open { "Cancel" } else { "Change chord" }
                            }
                            button {
                                class: "rg-btn rg-btn--danger",
                                r#type: "button",
                                onclick: move |_| {
                                    recording.set(None);
                                    let mut write = state.write();
                                    write.daemon.settings.keyboard.custom.remove(at);
                                    drop(write);
                                    commit(&state.peek());
                                },
                                "Delete"
                            }
                        }

                        if let Some(why) = fault {
                            span { class: "rg-field__hint", "{fault_sentence(&why)}" }
                        }

                        if open {
                            ChordRecorder {
                                draft,
                                conflict: None,
                                on_save: move |binding: Binding| {
                                    recording.set(None);
                                    let text = launch::format_chord(&launch::Chord {
                                        key: binding.key,
                                        ctrl: binding.ctrl,
                                        alt: binding.alt,
                                        shift: binding.shift,
                                    });
                                    edit_binding(state, at, |binding| binding.chord = text);
                                },
                            }
                        }

                        StepList { state, at, path: Vec::new(), steps, depth: 0 }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ChordRecorderProps {
    pub draft: Signal<Binding>,
    /// The built-in action this chord would shadow, if any.
    pub conflict: Option<KeyAction>,
    pub on_save: EventHandler<Binding>,
}

/// Record a chord by pressing it, or build it from the menu.
///
/// Both, because pressing is not always possible. `bootstrap.js` matches the
/// live table on `window` in the capture phase, so a chord whose scope survives
/// a focused text field is consumed before this handler runs and can only be
/// assembled from the menu. The note says which case you are in instead of
/// leaving a key that appears not to register.
#[component]
pub fn ChordRecorder(props: ChordRecorderProps) -> Element {
    let mut draft = props.draft;
    let current = draft();
    let rejection = current.rejection();
    let conflict = props.conflict;
    let blocked = rejection.is_some() || conflict.is_some();
    let stolen = crate::keymap::claims(&current.key, current.ctrl, current.alt, current.shift);

    rsx! {
        div { class: "rg-field rg-field--editor",
            span { class: "rg-field__control",
                input {
                    class: "rg-field__input rg-keybinds__record",
                    aria_label: "Press the chord",
                    readonly: true,
                    placeholder: "Click here, then press the keys",
                    value: "{current.rendered()}",
                    onkeydown: move |e: KeyboardEvent| {
                        let m = e.modifiers();
                        if m.meta() {
                            return;
                        }
                        let key = e.key().to_string();
                        // A modifier on its own is half a chord, not a chord.
                        if matches!(key.as_str(), "Control" | "Alt" | "Shift" | "Meta") {
                            return;
                        }
                        e.prevent_default();
                        let code = format!("{:?}", e.code());
                        let chord =
                            crate::keymap::chord_from_event(&key, &code, m.ctrl(), m.alt(), m.shift());
                        draft.set(Binding {
                            key: chord.key,
                            ctrl: chord.ctrl,
                            alt: chord.alt,
                            shift: chord.shift,
                        });
                    },
                }

                for (label, on, which) in [
                    ("Ctrl", current.ctrl, 0u8),
                    ("Alt", current.alt, 1),
                    ("Shift", current.shift, 2),
                ] {
                    button {
                        class: if on { "rg-chip rg-chip--on" } else { "rg-chip" },
                        key: "{label}",
                        r#type: "button",
                        aria_pressed: if on { "true" } else { "false" },
                        onclick: move |_| {
                            let mut next = draft();
                            match which {
                                0 => next.ctrl = !next.ctrl,
                                1 => next.alt = !next.alt,
                                _ => next.shift = !next.shift,
                            }
                            draft.set(next);
                        },
                        "{label}"
                    }
                }

                select {
                    class: "rg-select",
                    aria_label: "Key",
                    onchange: move |e| {
                        let mut next = draft();
                        next.key = e.value();
                        draft.set(next);
                    },
                    for key in BINDABLE_KEYS {
                        option {
                            key: "{key}",
                            value: "{key}",
                            selected: *key == current.key,
                            "{pretty_key(key)}"
                        }
                    }
                }

                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    disabled: blocked,
                    onclick: move |_| props.on_save.call(draft()),
                    "Save {draft().rendered()}"
                }
            }

            if let Some(why) = rejection {
                span { class: "rg-field__hint", "{why}" }
            } else if let Some(other) = conflict {
                span { class: "rg-field__hint",
                    "{draft().rendered()} already does \u{201c}{action_label(other)}\u{201d}. Two \
                     actions on one chord means the first in table order wins and the other is \
                     dead, with nothing on screen saying so."
                }
            } else if let Some(chord) = stolen {
                span { class: "rg-field__hint",
                    "{draft().rendered()} is claimed by the shell for \u{201c}{chord.describes()}\u{201d} \
                     even inside this field, so pressing it here will not record it. Build it from \
                     the menu, and it will still take the chord once you save."
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct StepListProps {
    pub state: Signal<UiState>,
    /// Which custom binding these steps belong to.
    pub at: usize,
    /// Where this list sits inside that binding.
    pub path: Vec<Hop>,
    pub steps: Vec<Step>,
    /// How many conditionals enclose this list. Guards [`MAX_BINDING_DEPTH`].
    pub depth: usize,
}

/// One ordered list of steps, with its own add buttons.
///
/// Recursive: a conditional renders two of these. The depth is carried rather
/// than derived so the add button for a further conditional can be refused at
/// the same limit the planner enforces, which is the difference between "this
/// cannot be built" and "this saves and then refuses to run".
#[component]
pub fn StepList(props: StepListProps) -> Element {
    let state = props.state;
    let at = props.at;
    let path = props.path.clone();
    let depth = props.depth;
    let nestable = depth < MAX_BINDING_DEPTH;

    rsx! {
        div { class: "rg-keybinds__steps",
            for (index, step) in props.steps.iter().enumerate() {
                {
                    let step = step.clone();
                    let path = path.clone();
                    rsx! {
                        div { class: "rg-keybinds__step", key: "{index}",
                            span { class: "rg-field__control",
                                match &step {
                                    Step::Action { action } => rsx! {
                                        span { class: "rg-field__label", "Do" }
                                        select {
                                            class: "rg-select",
                                            aria_label: "Action",
                                            onchange: {
                                                let path = path.clone();
                                                move |e: FormEvent| {
                                                    let wire = e.value();
                                                    edit_steps(state, at, &path, |list| {
                                                        if let Some(slot) = list.get_mut(index) {
                                                            *slot = Step::Action { action: wire };
                                                        }
                                                    });
                                                }
                                            },
                                            for (option_action, what) in rebindable() {
                                                option {
                                                    key: "{option_action.wire()}",
                                                    value: "{option_action.wire()}",
                                                    selected: option_action.wire() == *action,
                                                    "{what}"
                                                }
                                            }
                                        }
                                    },
                                    Step::Text { text } => rsx! {
                                        span { class: "rg-field__label", "Send" }
                                        input {
                                            class: "rg-field__input",
                                            aria_label: "Text",
                                            placeholder: "\\x03",
                                            initial_value: "{text}",
                                            onchange: {
                                                let path = path.clone();
                                                move |e: FormEvent| {
                                                    let text = e.value();
                                                    edit_steps(state, at, &path, |list| {
                                                        if let Some(slot) = list.get_mut(index) {
                                                            *slot = Step::Text { text };
                                                        }
                                                    });
                                                }
                                            },
                                        }
                                    },
                                    Step::When { predicate, then, otherwise } => rsx! {
                                        PredicateEditor {
                                            state,
                                            at,
                                            path: path.clone(),
                                            index,
                                            predicate: predicate.clone(),
                                        }
                                        div { class: "rg-keybinds__branch",
                                            span { class: "rg-field__label", "then" }
                                            StepList {
                                                state,
                                                at,
                                                path: hop(&path, index, Branch::Then),
                                                steps: then.clone(),
                                                depth: depth + 1,
                                            }
                                            span { class: "rg-field__label", "otherwise" }
                                            StepList {
                                                state,
                                                at,
                                                path: hop(&path, index, Branch::Otherwise),
                                                steps: otherwise.clone(),
                                                depth: depth + 1,
                                            }
                                        }
                                    },
                                    Step::Unknown => rsx! {
                                        span { class: "rg-field__hint",
                                            "A step a newer build wrote. It is kept and skipped, so the \
                                             rest of this binding still runs."
                                        }
                                    },
                                }
                                button {
                                    class: "rg-btn rg-btn--danger",
                                    r#type: "button",
                                    onclick: {
                                        let path = path.clone();
                                        move |_| {
                                            edit_steps(state, at, &path, |list| {
                                                if index < list.len() {
                                                    list.remove(index);
                                                }
                                            });
                                        }
                                    },
                                    "Remove"
                                }
                            }
                        }
                    }
                }
            }

            span { class: "rg-field__control",
                for (label, kind) in [("Add text", 0u8), ("Add action", 1), ("Add condition", 2)] {
                    button {
                        class: "rg-btn",
                        key: "{label}",
                        r#type: "button",
                        disabled: kind == 2 && !nestable,
                        onclick: {
                            let path = path.clone();
                            move |_| {
                                let next = match kind {
                                    0 => Step::text(""),
                                    1 => Step::action(KeyAction::NextAttention),
                                    _ => Step::when(
                                        Predicate::SessionFocused,
                                        Vec::new(),
                                        Vec::new(),
                                    ),
                                };
                                edit_steps(state, at, &path, |list| list.push(next));
                            }
                        },
                        "{label}"
                    }
                }
                if !nestable {
                    span { class: "rg-field__hint",
                        "Conditions stop nesting at {MAX_BINDING_DEPTH} deep. Past that a binding is \
                         easier to write as two."
                    }
                }
            }
        }
    }
}

/// The path to one branch of the conditional at `index`.
fn hop(path: &[Hop], index: usize, branch: Branch) -> Vec<Hop> {
    let mut out = Vec::with_capacity(path.len() + 1);
    out.extend_from_slice(path);
    out.push(Hop { at: index, branch });
    out
}

#[derive(Props, Clone, PartialEq)]
pub struct PredicateEditorProps {
    pub state: Signal<UiState>,
    pub at: usize,
    pub path: Vec<Hop>,
    /// Index of the [`Step::When`] this predicate belongs to.
    pub index: usize,
    pub predicate: Predicate,
}

/// The question one conditional asks, and whatever narrows it.
#[component]
pub fn PredicateEditor(props: PredicateEditorProps) -> Element {
    let state = props.state;
    let at = props.at;
    let index = props.index;
    let path = props.path.clone();
    let predicate = props.predicate.clone();
    let wire = predicate_wire(&predicate);

    // Replaces only the predicate. The two branches are the operator's work and
    // changing the question must not throw them away.
    let replace = move |state: Signal<UiState>, path: Vec<Hop>, next: Predicate| {
        edit_steps(state, at, &path, |list| {
            if let Some(Step::When { predicate, .. }) = list.get_mut(index) {
                *predicate = next;
            }
        });
    };

    rsx! {
        span { class: "rg-field__label", "When" }
        select {
            class: "rg-select",
            aria_label: "Condition",
            onchange: {
                let path = path.clone();
                move |e: FormEvent| replace(state, path.clone(), predicate_of(&e.value()))
            },
            for (kind, what) in PREDICATE_KINDS {
                option { key: "{kind}", value: "{kind}", selected: *kind == wire, "{what}" }
            }
            if wire == "unknown" {
                option { value: "unknown", selected: true,
                    "a question a newer build asks"
                }
            }
        }

        match &predicate {
            Predicate::FocusedStatus { status } => rsx! {
                select {
                    class: "rg-select",
                    aria_label: "Status",
                    onchange: {
                        let path = path.clone();
                        move |e: FormEvent| {
                            replace(state, path.clone(), Predicate::FocusedStatus {
                                status: StatusKind::from_wire(&e.value()),
                            });
                        }
                    },
                    for option_status in StatusKind::all() {
                        option {
                            key: "{option_status.wire()}",
                            value: "{option_status.wire()}",
                            selected: option_status == status,
                            "{status_label(*option_status)}"
                        }
                    }
                }
            },
            Predicate::LayerOpen { layer } => rsx! {
                select {
                    class: "rg-select",
                    aria_label: "Panel",
                    onchange: {
                        let path = path.clone();
                        move |e: FormEvent| {
                            replace(state, path.clone(), Predicate::LayerOpen {
                                layer: LayerKind::from_wire(&e.value()),
                            });
                        }
                    },
                    for option_layer in LayerKind::all() {
                        option {
                            key: "{option_layer.wire()}",
                            value: "{option_layer.wire()}",
                            selected: option_layer == layer,
                            "{layer_label(*option_layer)}"
                        }
                    }
                }
            },
            Predicate::WorkspaceHasAttention { attention } => rsx! {
                select {
                    class: "rg-select",
                    aria_label: "Signal",
                    onchange: {
                        let path = path.clone();
                        move |e: FormEvent| {
                            replace(state, path.clone(), Predicate::WorkspaceHasAttention {
                                attention: AttentionKind::from_wire(&e.value()),
                            });
                        }
                    },
                    for option_signal in AttentionKind::all() {
                        option {
                            key: "{option_signal.wire()}",
                            value: "{option_signal.wire()}",
                            selected: option_signal == attention,
                            "{attention_label(*option_signal)}"
                        }
                    }
                }
            },
            Predicate::FocusedCommandContains { text } => rsx! {
                input {
                    class: "rg-field__input rg-field__input--prose",
                    aria_label: "Command contains",
                    placeholder: "claude",
                    initial_value: "{text}",
                    onchange: {
                        let path = path.clone();
                        move |e: FormEvent| {
                            replace(state, path.clone(), Predicate::FocusedCommandContains {
                                text: e.value(),
                            });
                        }
                    },
                }
            },
            _ => rsx! {},
        }
    }
}

#[cfg(test)]
mod tests;
