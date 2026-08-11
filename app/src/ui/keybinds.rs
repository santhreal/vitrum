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

use crate::keymap::{
    AttentionKind, BindingError, CustomBinding, KeyAction, LayerKind, MAX_BINDING_DEPTH, Predicate,
    StatusKind, Step,
};
use crate::launch;
use crate::ui::settings::sheet::Host;
use crate::ui::settings::{
    BINDABLE_KEYS, Binding, EffectiveChord, action_label, clear_override, effective_chords,
    live_conflict, pretty_key, rebindable, set_override,
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
/// Every control on this page goes through one function, so "takes effect
/// immediately and survives a restart" is true by construction rather than by
/// each handler remembering both halves.
fn edit_binding(host: &Host, at: usize, change: impl FnOnce(&mut CustomBinding) + 'static) {
    host.edit(move |settings| {
        if let Some(binding) = settings.keyboard.custom.get_mut(at) {
            change(binding);
        }
    });
}

/// Mutate the step list one path addresses.
fn edit_steps(
    host: &Host,
    at: usize,
    path: &[Hop],
    change: impl FnOnce(&mut Vec<Step>) + 'static,
) {
    let path = path.to_vec();
    edit_binding(host, at, move |binding| {
        if let Some(list) = list_at(&mut binding.steps, &path) {
            change(list);
        }
    });
}

/// Draft key holding the wire name of the built-in action open for rebinding.
const EDITING: &str = "keyboard.editing";
/// Draft key holding the index of the custom binding whose chord is open.
const RECORDING: &str = "keyboard.recording";
/// Draft key holding the chord being assembled, in [`Binding::encode`] form.
const DRAFT: &str = "keyboard.draft";

/// The chord a GDK key press names, when it is one a binding can hold.
///
/// `None` for a modifier pressed alone, for a Super combination the desktop
/// owns, and for anything outside [`BINDABLE_KEYS`]. Returning nothing rather
/// than a near miss is what lets the field say "build it from the menu"
/// instead of storing a chord the matcher would never see.
fn recorded(event: &gdk::EventKey) -> Option<Binding> {
    let state = event.state();
    if state.contains(gdk::ModifierType::SUPER_MASK)
        || state.contains(gdk::ModifierType::META_MASK)
    {
        return None;
    }
    // The unshifted level, because a chord is named for the key and not for
    // the character shift produces on it. Without this, Ctrl+Shift+1 records
    // as `!`, which is not a name any table holds.
    let name = event.keyval().to_lower().name()?;
    let key = match name.as_str() {
        "Up" => "arrowup".to_string(),
        "Down" => "arrowdown".to_string(),
        "Left" => "arrowleft".to_string(),
        "Right" => "arrowright".to_string(),
        "Page_Up" => "pageup".to_string(),
        "Page_Down" => "pagedown".to_string(),
        "slash" => "/".to_string(),
        "backslash" => "\\".to_string(),
        "comma" => ",".to_string(),
        "period" => ".".to_string(),
        "semicolon" => ";".to_string(),
        "apostrophe" => "'".to_string(),
        "bracketleft" => "[".to_string(),
        "bracketright" => "]".to_string(),
        "minus" => "-".to_string(),
        "equal" => "=".to_string(),
        "grave" => "`".to_string(),
        other => other.to_lowercase(),
    };
    if !BINDABLE_KEYS.contains(&key.as_str()) {
        return None;
    }
    Some(Binding {
        key,
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        alt: state.contains(gdk::ModifierType::MOD1_MASK),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
    })
}

/// The Keyboard page, as widgets.
///
/// Which row is open and what chord is being assembled live in the sheet's
/// draft map rather than in a widget, because the page is rebuilt whole after
/// every edit and a half-assembled chord that vanished on the first modifier
/// click would be unusable.
pub(crate) fn page(host: &Host) -> gtk::Widget {
    use gtk::prelude::*;

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let prefs = host.settings().keyboard.clone();
    let effective = effective_chords(&prefs);
    let faults = prefs.custom.errors();
    let editing = KeyAction::parse(&host.draft(EDITING, ""));
    let recording: Option<usize> = host.draft(RECORDING, "").parse().ok();
    let draft = Binding::parse(&host.draft(DRAFT, "")).unwrap_or_default();

    let intro = host.field(
        "Shortcuts",
        "Ctrl is the primary modifier on every platform, macOS included, because Cmd+Tab never \
         reaches an application. A chord you claim below, under your own bindings, is taken away \
         from the action listed here.",
        "",
    );
    root.pack_start(&intro.root, false, false, 0);

    for (action, what) in rebindable() {
        let current = effective
            .iter()
            .find(|chord| chord.action == action)
            .cloned();
        let taken = current.as_ref().and_then(|chord| {
            prefs
                .custom
                .shadowing(&chord.key, chord.ctrl, chord.alt, chord.shift)
                .map(|binding| binding.title().to_string())
        });
        let open = editing == Some(action);

        let field = host.field(what, "", "");

        let chord_label = gtk::Label::new(Some(
            &current
                .as_ref()
                .map_or_else(|| "unbound".to_string(), EffectiveChord::rendered),
        ));
        chord_label.style_context().add_class("rg-keys__chord");
        field.control.pack_start(&chord_label, false, false, 0);

        let change = gtk::Button::with_label(if open { "Cancel" } else { "Change" });
        change.style_context().add_class("rg-btn");
        {
            let host = host.clone();
            let seed = current
                .as_ref()
                .map(EffectiveChord::binding)
                .unwrap_or_default();
            change.connect_clicked(move |_| {
                host.clear_draft(RECORDING);
                if open {
                    host.clear_draft(EDITING);
                } else {
                    host.set_draft(DRAFT, seed.encode());
                    host.set_draft(EDITING, action.wire());
                }
                host.refresh();
            });
        }
        field.control.pack_start(&change, false, false, 0);

        if current.as_ref().is_some_and(|chord| chord.rebound) {
            let reset = gtk::Button::with_label("Reset");
            reset.style_context().add_class("rg-btn");
            let host = host.clone();
            reset.connect_clicked(move |_| {
                host.clear_draft(EDITING);
                host.edit(move |settings| clear_override(&mut settings.keyboard, action));
            });
            field.control.pack_start(&reset, false, false, 0);
        }

        if let Some(label) = taken {
            let hint = super::settings::sheet::wrapped(&format!(
                "Your binding \u{201c}{label}\u{201d} owns this chord, so this action does not \
                 fire. Move one of them."
            ));
            hint.style_context().add_class("rg-field__hint");
            field.root.pack_start(&hint, false, false, 0);
        }

        if open {
            let host_for_save = host.clone();
            let recorder = chord_recorder(
                host,
                &draft,
                live_conflict(&draft, action),
                move |binding| {
                    host_for_save.clear_draft(EDITING);
                    host_for_save.edit(move |settings| {
                        set_override(&mut settings.keyboard, action, &binding);
                    });
                },
            );
            field.root.pack_start(&recorder, false, false, 0);
        }

        root.pack_start(&field.root, false, false, 0);
    }

    let heading = host.field(
        "Your own bindings",
        "An ordered list of steps on one chord. Text is sent to the focused session exactly as \
         written, with \\e for escape, \\r for return, \\x03 for interrupt and \\\\ for a literal \
         backslash. A condition runs one of two lists depending on what the window is doing when \
         you press the key.",
        "",
    );
    heading
        .root
        .style_context()
        .add_class("rg-keybinds__heading");
    let add = gtk::Button::with_label("Add a binding");
    add.style_context().add_class("rg-btn");
    add.style_context().add_class("rg-btn--primary");
    {
        let host = host.clone();
        add.connect_clicked(move |_| {
            host.edit(|settings| settings.keyboard.custom.push(CustomBinding::default()));
        });
    }
    heading.control.pack_start(&add, false, false, 0);
    if !faults.is_empty() {
        let hint = super::settings::sheet::wrapped(&format!(
            "{} of your {} until you fix {}.",
            if faults.len() == 1 {
                "1".to_string()
            } else {
                faults.len().to_string()
            },
            if prefs.custom.len() == 1 {
                "1 binding does nothing".to_string()
            } else {
                format!("{} bindings do nothing", prefs.custom.len())
            },
            if faults.len() == 1 { "it" } else { "them" },
        ));
        hint.style_context().add_class("rg-field__hint");
        heading.root.pack_start(&hint, false, false, 0);
    }
    root.pack_start(&heading.root, false, false, 0);

    if prefs.custom.is_empty() {
        let note = super::settings::sheet::wrapped(
            "You have no bindings of your own yet. One is a chord, a name, and the steps it runs.",
        );
        note.style_context().add_class("rg-sheet__note");
        root.pack_start(&note, false, false, 0);
    }

    for (at, binding) in prefs.custom.all().iter().enumerate() {
        let fault = faults
            .iter()
            .find(|(row, _)| *row == at)
            .map(|(_, why)| why.clone());
        let open = recording == Some(at);
        let chord_text = binding.chord.clone();

        let field = host.field("", "", "");
        field.root.style_context().add_class("rg-field--keybind");

        let name = gtk::Entry::new();
        name.style_context().add_class("rg-field__input");
        name.set_placeholder_text(Some("Name this binding"));
        name.set_text(&binding.label);
        name.set_hexpand(true);
        {
            let host = host.clone();
            name.connect_activate(move |entry| {
                let text = entry.text().to_string();
                edit_binding(&host, at, |binding| binding.label = text);
            });
        }
        {
            let host = host.clone();
            name.connect_focus_out_event(move |entry, _| {
                let text = entry.text().to_string();
                edit_binding(&host, at, |binding| binding.label = text);
                glib::Propagation::Proceed
            });
        }
        field.control.pack_start(&name, true, true, 0);

        let shown = gtk::Label::new(Some(if chord_text.is_empty() {
            "unbound"
        } else {
            &chord_text
        }));
        shown.style_context().add_class("rg-keys__chord");
        field.control.pack_start(&shown, false, false, 0);

        let change = gtk::Button::with_label(if open { "Cancel" } else { "Change chord" });
        change.style_context().add_class("rg-btn");
        {
            let host = host.clone();
            let seed = launch::parse_chord(&chord_text)
                .map(|chord| Binding {
                    key: chord.key,
                    ctrl: chord.ctrl,
                    alt: chord.alt,
                    shift: chord.shift,
                })
                .unwrap_or_default();
            change.connect_clicked(move |_| {
                host.clear_draft(EDITING);
                if open {
                    host.clear_draft(RECORDING);
                } else {
                    host.set_draft(DRAFT, seed.encode());
                    host.set_draft(RECORDING, at.to_string());
                }
                host.refresh();
            });
        }
        field.control.pack_start(&change, false, false, 0);

        let delete = gtk::Button::with_label("Delete");
        delete.style_context().add_class("rg-btn");
        delete.style_context().add_class("rg-btn--danger");
        {
            let host = host.clone();
            delete.connect_clicked(move |_| {
                host.clear_draft(RECORDING);
                host.edit(move |settings| {
                    if at < settings.keyboard.custom.len() {
                        settings.keyboard.custom.remove(at);
                    }
                });
            });
        }
        field.control.pack_start(&delete, false, false, 0);

        if let Some(why) = fault {
            let hint = super::settings::sheet::wrapped(&fault_sentence(&why));
            hint.style_context().add_class("rg-field__hint");
            field.root.pack_start(&hint, false, false, 0);
        }

        if open {
            let host_for_save = host.clone();
            let recorder = chord_recorder(host, &draft, None, move |binding| {
                host_for_save.clear_draft(RECORDING);
                let text = launch::format_chord(&launch::Chord {
                    key: binding.key,
                    ctrl: binding.ctrl,
                    alt: binding.alt,
                    shift: binding.shift,
                });
                edit_binding(&host_for_save, at, |binding| binding.chord = text);
            });
            field.root.pack_start(&recorder, false, false, 0);
        }

        field.root.pack_start(
            &step_list(host, at, &[], &binding.steps, 0),
            false,
            false,
            0,
        );
        root.pack_start(&field.root, false, false, 0);
    }

    root.upcast()
}

/// Record a chord by pressing it, or build it from the menu.
///
/// Both, because pressing is not always possible. The shell matches the live
/// table on the window, so a chord whose scope survives a focused text field
/// is consumed before this field sees it and can only be assembled from the
/// menu. The note says which case you are in instead of leaving a key that
/// appears not to register.
fn chord_recorder(
    host: &Host,
    current: &Binding,
    conflict: Option<KeyAction>,
    save: impl Fn(Binding) + 'static,
) -> gtk::Widget {
    use gtk::prelude::*;

    let rejection = current.rejection();
    let blocked = rejection.is_some() || conflict.is_some();
    let stolen = crate::keymap::claims(&current.key, current.ctrl, current.alt, current.shift);

    let field = host.field("", "", "");
    field.root.style_context().add_class("rg-field--editor");

    let press = gtk::Entry::new();
    press.style_context().add_class("rg-field__input");
    press.style_context().add_class("rg-keybinds__record");
    press.set_placeholder_text(Some("Click here, then press the keys"));
    press.set_text(&current.rendered());
    press.set_editable(false);
    press.set_hexpand(true);
    {
        let host = host.clone();
        press.connect_key_press_event(move |_, event| {
            let Some(binding) = recorded(event) else {
                return glib::Propagation::Proceed;
            };
            host.set_draft(DRAFT, binding.encode());
            host.refresh();
            glib::Propagation::Stop
        });
    }
    field.control.pack_start(&press, true, true, 0);

    for (label, on, which) in [
        ("Ctrl", current.ctrl, 0u8),
        ("Alt", current.alt, 1),
        ("Shift", current.shift, 2),
    ] {
        let chip = gtk::ToggleButton::with_label(label);
        chip.style_context().add_class("rg-chip");
        chip.set_active(on);
        let host = host.clone();
        let seed = current.clone();
        chip.connect_toggled(move |chip| {
            let mut next = seed.clone();
            match which {
                0 => next.ctrl = chip.is_active(),
                1 => next.alt = chip.is_active(),
                _ => next.shift = chip.is_active(),
            }
            host.set_draft(DRAFT, next.encode());
            host.refresh();
        });
        field.control.pack_start(&chip, false, false, 0);
    }

    let keys = gtk::ComboBoxText::new();
    keys.style_context().add_class("rg-select");
    for key in BINDABLE_KEYS {
        keys.append(Some(key), &pretty_key(key));
    }
    keys.set_active_id(Some(current.key.as_str()));
    {
        let host = host.clone();
        let seed = current.clone();
        keys.connect_changed(move |combo| {
            let Some(picked) = combo.active_id() else {
                return;
            };
            let mut next = seed.clone();
            next.key = picked.to_string();
            host.set_draft(DRAFT, next.encode());
            host.refresh();
        });
    }
    field.control.pack_start(&keys, false, false, 0);

    let commit_button = gtk::Button::with_label(&format!("Save {}", current.rendered()));
    commit_button.style_context().add_class("rg-btn");
    commit_button.style_context().add_class("rg-btn--primary");
    commit_button.set_sensitive(!blocked);
    {
        let host = host.clone();
        let binding = current.clone();
        commit_button.connect_clicked(move |_| {
            host.clear_draft(DRAFT);
            save(binding.clone());
        });
    }
    field.control.pack_start(&commit_button, false, false, 0);

    let note = if let Some(why) = rejection {
        why.to_string()
    } else if let Some(other) = conflict {
        format!(
            "{} already does \u{201c}{}\u{201d}. Two actions on one chord means the first in \
             table order wins and the other is dead, with nothing on screen saying so.",
            current.rendered(),
            action_label(other)
        )
    } else if let Some(chord) = stolen {
        format!(
            "{} is claimed by the shell for \u{201c}{}\u{201d} even inside this field, so \
             pressing it here will not record it. Build it from the menu, and it will still take \
             the chord once you save.",
            current.rendered(),
            chord.describes()
        )
    } else {
        String::new()
    };
    if !note.is_empty() {
        let hint = super::settings::sheet::wrapped(&note);
        hint.style_context().add_class("rg-field__hint");
        field.root.pack_start(&hint, false, false, 0);
    }

    field.root.upcast()
}

/// One ordered list of steps, with its own add buttons.
///
/// Recursive: a conditional builds two of these. The depth is carried rather
/// than derived so the add button for a further conditional can be refused at
/// the same limit the planner enforces, which is the difference between "this
/// cannot be built" and "this saves and then refuses to run".
fn step_list(host: &Host, at: usize, path: &[Hop], steps: &[Step], depth: usize) -> gtk::Widget {
    use gtk::prelude::*;

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.style_context().add_class("rg-keybinds__steps");
    let nestable = depth < MAX_BINDING_DEPTH;

    for (index, step) in steps.iter().enumerate() {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
        row.style_context().add_class("rg-keybinds__step");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        line.style_context().add_class("rg-field__control");

        match step {
            Step::Action { action } => {
                let label = gtk::Label::new(Some("Do"));
                label.style_context().add_class("rg-field__label");
                line.pack_start(&label, false, false, 0);
                let combo = gtk::ComboBoxText::new();
                combo.style_context().add_class("rg-select");
                for (option_action, what) in rebindable() {
                    combo.append(Some(&option_action.wire()), what);
                }
                combo.set_active_id(Some(action.as_str()));
                let host = host.clone();
                let path = path.to_vec();
                combo.connect_changed(move |combo| {
                    let Some(wire) = combo.active_id() else {
                        return;
                    };
                    let wire = wire.to_string();
                    edit_steps(&host, at, &path, move |list| {
                        if let Some(slot) = list.get_mut(index) {
                            *slot = Step::Action { action: wire };
                        }
                    });
                });
                line.pack_start(&combo, false, false, 0);
            }
            Step::Text { text } => {
                let label = gtk::Label::new(Some("Send"));
                label.style_context().add_class("rg-field__label");
                line.pack_start(&label, false, false, 0);
                let entry = gtk::Entry::new();
                entry.style_context().add_class("rg-field__input");
                entry.set_placeholder_text(Some("\\x03"));
                entry.set_text(text);
                entry.set_hexpand(true);
                let commit = {
                    let host = host.clone();
                    let path = path.to_vec();
                    move |entry: &gtk::Entry| {
                        let text = entry.text().to_string();
                        edit_steps(&host, at, &path, move |list| {
                            if let Some(slot) = list.get_mut(index) {
                                *slot = Step::Text { text };
                            }
                        });
                    }
                };
                {
                    let commit = commit.clone();
                    entry.connect_activate(move |entry| commit(entry));
                }
                entry.connect_focus_out_event(move |entry, _| {
                    commit(entry);
                    glib::Propagation::Proceed
                });
                line.pack_start(&entry, true, true, 0);
            }
            Step::When {
                predicate,
                then,
                otherwise,
            } => {
                line.pack_start(
                    &predicate_editor(host, at, path, index, predicate),
                    false,
                    false,
                    0,
                );
                let branches = gtk::Box::new(gtk::Orientation::Vertical, 0);
                branches.style_context().add_class("rg-keybinds__branch");
                for (word, branch, list) in [
                    ("then", Branch::Then, then),
                    ("otherwise", Branch::Otherwise, otherwise),
                ] {
                    let label = gtk::Label::new(Some(word));
                    label.style_context().add_class("rg-field__label");
                    branches.pack_start(&label, false, false, 0);
                    branches.pack_start(
                        &step_list(host, at, &hop(path, index, branch), list, depth + 1),
                        false,
                        false,
                        0,
                    );
                }
                row.pack_start(&branches, false, false, 0);
            }
            Step::Unknown => {
                let hint = super::settings::sheet::wrapped(
                    "A step a newer build wrote. It is kept and skipped, so the rest of this \
                     binding still runs.",
                );
                hint.style_context().add_class("rg-field__hint");
                line.pack_start(&hint, false, false, 0);
            }
        }

        let remove = gtk::Button::with_label("Remove");
        remove.style_context().add_class("rg-btn");
        remove.style_context().add_class("rg-btn--danger");
        {
            let host = host.clone();
            let path = path.to_vec();
            remove.connect_clicked(move |_| {
                edit_steps(&host, at, &path, move |list| {
                    if index < list.len() {
                        list.remove(index);
                    }
                });
            });
        }
        line.pack_start(&remove, false, false, 0);
        row.pack_start(&line, false, false, 0);
        // The branches box is packed before the line for a conditional, so
        // reorder it back under the question it belongs to.
        row.reorder_child(&line, 0);
        root.pack_start(&row, false, false, 0);
    }

    let adders = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    adders.style_context().add_class("rg-field__control");
    for (label, kind) in [("Add text", 0u8), ("Add action", 1), ("Add condition", 2)] {
        let button = gtk::Button::with_label(label);
        button.style_context().add_class("rg-btn");
        button.set_sensitive(kind != 2 || nestable);
        let host = host.clone();
        let path = path.to_vec();
        button.connect_clicked(move |_| {
            let next = match kind {
                0 => Step::text(""),
                1 => Step::action(KeyAction::NextAttention),
                _ => Step::when(Predicate::SessionFocused, Vec::new(), Vec::new()),
            };
            edit_steps(&host, at, &path, |list| list.push(next));
        });
        adders.pack_start(&button, false, false, 0);
    }
    root.pack_start(&adders, false, false, 0);

    if !nestable {
        let hint = super::settings::sheet::wrapped(&format!(
            "Conditions stop nesting at {MAX_BINDING_DEPTH} deep. Past that a binding is easier \
             to write as two."
        ));
        hint.style_context().add_class("rg-field__hint");
        root.pack_start(&hint, false, false, 0);
    }

    root.upcast()
}

/// The question one conditional asks, and whatever narrows it.
fn predicate_editor(
    host: &Host,
    at: usize,
    path: &[Hop],
    index: usize,
    predicate: &Predicate,
) -> gtk::Widget {
    use gtk::prelude::*;

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let wire = predicate_wire(predicate);

    let label = gtk::Label::new(Some("When"));
    label.style_context().add_class("rg-field__label");
    root.pack_start(&label, false, false, 0);

    // Replaces only the predicate. The two branches are the operator's work
    // and changing the question must not throw them away.
    let replace = {
        let host = host.clone();
        let path = path.to_vec();
        move |next: Predicate| {
            edit_steps(&host, at, &path, move |list| {
                if let Some(Step::When { predicate, .. }) = list.get_mut(index) {
                    *predicate = next;
                }
            });
        }
    };

    let kinds = gtk::ComboBoxText::new();
    kinds.style_context().add_class("rg-select");
    for (kind, what) in PREDICATE_KINDS {
        kinds.append(Some(kind), what);
    }
    if wire == "unknown" {
        kinds.append(Some("unknown"), "a question a newer build asks");
    }
    kinds.set_active_id(Some(wire));
    {
        let replace = replace.clone();
        kinds.connect_changed(move |combo| {
            let Some(picked) = combo.active_id() else {
                return;
            };
            replace(predicate_of(&picked));
        });
    }
    root.pack_start(&kinds, false, false, 0);

    let narrow = gtk::ComboBoxText::new();
    narrow.style_context().add_class("rg-select");
    match predicate {
        Predicate::FocusedStatus { status } => {
            for option_status in StatusKind::all() {
                narrow.append(Some(&option_status.wire()), status_label(*option_status));
            }
            narrow.set_active_id(Some(&status.wire()));
            let replace = replace.clone();
            narrow.connect_changed(move |combo| {
                let Some(picked) = combo.active_id() else {
                    return;
                };
                replace(Predicate::FocusedStatus {
                    status: StatusKind::from_wire(&picked),
                });
            });
            root.pack_start(&narrow, false, false, 0);
        }
        Predicate::LayerOpen { layer } => {
            for option_layer in LayerKind::all() {
                narrow.append(Some(&option_layer.wire()), layer_label(*option_layer));
            }
            narrow.set_active_id(Some(&layer.wire()));
            let replace = replace.clone();
            narrow.connect_changed(move |combo| {
                let Some(picked) = combo.active_id() else {
                    return;
                };
                replace(Predicate::LayerOpen {
                    layer: LayerKind::from_wire(&picked),
                });
            });
            root.pack_start(&narrow, false, false, 0);
        }
        Predicate::WorkspaceHasAttention { attention } => {
            for option_signal in AttentionKind::all() {
                narrow.append(Some(&option_signal.wire()), attention_label(*option_signal));
            }
            narrow.set_active_id(Some(&attention.wire()));
            let replace = replace.clone();
            narrow.connect_changed(move |combo| {
                let Some(picked) = combo.active_id() else {
                    return;
                };
                replace(Predicate::WorkspaceHasAttention {
                    attention: AttentionKind::from_wire(&picked),
                });
            });
            root.pack_start(&narrow, false, false, 0);
        }
        Predicate::FocusedCommandContains { text } => {
            let entry = gtk::Entry::new();
            entry.style_context().add_class("rg-field__input");
            entry.set_placeholder_text(Some("claude"));
            entry.set_text(text);
            entry.set_hexpand(true);
            let commit = move |entry: &gtk::Entry| {
                replace(Predicate::FocusedCommandContains {
                    text: entry.text().to_string(),
                });
            };
            {
                let commit = commit.clone();
                entry.connect_activate(move |entry| commit(entry));
            }
            entry.connect_focus_out_event(move |entry, _| {
                commit(entry);
                glib::Propagation::Proceed
            });
            root.pack_start(&entry, true, true, 0);
        }
        _ => {}
    }

    root.upcast()
}

/// The path to one branch of the conditional at `index`.
fn hop(path: &[Hop], index: usize, branch: Branch) -> Vec<Hop> {
    let mut out = Vec::with_capacity(path.len() + 1);
    out.extend_from_slice(path);
    out.push(Hop { at: index, branch });
    out
}

#[cfg(test)]
mod tests;
