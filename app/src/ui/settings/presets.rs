//! Saved commands: the validation rules, and the editor that applies them.
//!
//! One module because the two halves are one contract. Everything here is the
//! only writer of `launch.json`'s preset list, and every rule the new-session
//! dialog is entitled to assume about that list, unique labels, unique ids, a
//! shortcut the matcher can match, is enforced by [`revise`] and [`create`]
//! and by nothing else. Splitting the checks from the form that calls them
//! would put the invariant and its only enforcement in two files.
//!
//! The preference DATA is not here and is not in [`crate::state::Settings`]
//! either. Saved commands are records the operator authored, consumed by
//! [`crate::launch`], and putting them in the settings document would have
//! meant every window's `save_prefs` rewriting them on every unrelated
//! preference change.

use dioxus::prelude::*;

use crate::state::UiState;

/// Longest label the editor will store, in characters.
///
/// Counted in `char`s and not bytes, so a label in a non-Latin script gets the
/// same allowance as one in English. The number is what fits the picker's row
/// in the new-session dialog without eliding; a label that only ever appears
/// as `Claude in vitrum, resu…` is a label that has failed at the one job it
/// has.
pub const PRESET_LABEL_MAX: usize = 40;

/// Which field of a saved command an edit is aimed at.
///
/// The editor commits one field at a time rather than assembling a whole
/// candidate and writing it back. Four independent commits means a typo in the
/// shortcut cannot silently discard the working directory typed a second
/// earlier, which is exactly what a whole-record write does when validation
/// refuses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetField {
    Label,
    /// The program and its arguments as one line, split by
    /// [`crate::launch::split_command`].
    CommandLine,
    /// Default working directory. Empty clears it.
    Cwd,
    /// Chord that starts this command from the new-session dialog. Empty
    /// clears it.
    Shortcut,
    /// Slug of the icon this command draws. Empty clears it back to the one
    /// derived from the command text.
    Icon,
}

/// Why an edit to the saved commands was refused.
///
/// A variant per reason rather than a `String`, because two of them are
/// asserted in tests against exact content and because the panel renders the
/// sentence in one place. Every message names the value that was refused: a
/// form that answers "invalid input" over four fields has said nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetRefusal {
    /// The label was empty or only whitespace.
    NoLabel,
    /// The label was longer than [`PRESET_LABEL_MAX`].
    LabelTooLong(usize),
    /// Another saved command already answers to that label.
    DuplicateLabel(String),
    /// There is no program in the command line: it was empty, whitespace, or
    /// quoting that yields a blank first word.
    NoCommand,
    /// The shortcut is not a chord this build can match.
    BadShortcut(String),
    /// The shortcut is already a shell chord, so the dialog would never see
    /// the keydown. Carries the sentence [`crate::launch::chord_conflict`]
    /// produced, which names the action that owns it.
    ShortcutTaken(String),
    /// Another saved command already answers to that chord. Carries the
    /// canonical chord and the other row's label.
    ShortcutInUse(String, String),
    /// The row was deleted by another window between the render and the edit.
    Vanished,
}

impl std::fmt::Display for PresetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetRefusal::NoLabel => f.write_str(
                "A saved command needs a label. It is the only part of it the picker shows.",
            ),
            PresetRefusal::LabelTooLong(len) => write!(
                f,
                "That label is {len} characters. The picker shows {PRESET_LABEL_MAX}."
            ),
            PresetRefusal::DuplicateLabel(label) => write!(
                f,
                "\u{201c}{label}\u{201d} is already the label of another saved command. Two rows \
                 with one name means the picker offers the same word twice and neither says \
                 which is which."
            ),
            PresetRefusal::NoCommand => f.write_str(
                "A saved command needs a program to run. Anything on PATH works, and so does an \
                 absolute path.",
            ),
            PresetRefusal::BadShortcut(text) => write!(
                f,
                "\u{201c}{text}\u{201d} is not a chord this build can match. Write it as \
                 Ctrl+Shift+K: the modifiers are Ctrl, Alt and Shift, in any case, joined by +."
            ),
            PresetRefusal::ShortcutTaken(why) => write!(
                f,
                "{why} A saved command cannot take a chord the shell already claims: the \
                 keydown is handled before the dialog sees it, so the shortcut would be one \
                 this tab shows and the product never fires."
            ),
            PresetRefusal::ShortcutInUse(chord, label) => write!(
                f,
                "{chord} already starts \u{201c}{label}\u{201d}. Two saved commands on one chord \
                 means the first in the list wins and the other is dead, with nothing on screen \
                 saying so."
            ),
            PresetRefusal::Vanished => f.write_str(
                "That saved command was deleted in another window, so there was nothing to \
                 change. The list above is what is on disk now.",
            ),
        }
    }
}

/// Is `label` free, ignoring the row that already owns it?
///
/// Case-insensitive, because the picker is read by a person and `Claude` and
/// `claude` are one name to them. ASCII case folding rather than a full
/// Unicode fold: the comparison has to be identical to the one a test can
/// state, and a locale-dependent fold is not.
fn label_is_free(list: &[crate::launch::SavedPreset], label: &str, except: u64) -> bool {
    !list
        .iter()
        .any(|p| p.id != except && p.label.eq_ignore_ascii_case(label))
}

/// Check and normalise a label, or say why not.
fn accept_label(
    list: &[crate::launch::SavedPreset],
    label: &str,
    except: u64,
) -> Result<String, PresetRefusal> {
    let label = label.trim();
    if label.is_empty() {
        return Err(PresetRefusal::NoLabel);
    }
    let len = label.chars().count();
    if len > PRESET_LABEL_MAX {
        return Err(PresetRefusal::LabelTooLong(len));
    }
    if !label_is_free(list, label, except) {
        return Err(PresetRefusal::DuplicateLabel(label.to_string()));
    }
    Ok(label.to_string())
}

/// Check and split a command line, or say why not.
///
/// [`crate::launch::split_command`] has no failure mode of its own beyond
/// "there was no word here": an unclosed quote takes the rest of the line as
/// one argument rather than erroring, which is the behaviour the dialog's own
/// field has and the two must agree. So the only refusal is an absent
/// program, and it is checked on the SPLIT result and not on the raw line,
/// because `"   "` is a non-empty line whose first word is blank.
fn accept_command(line: &str) -> Result<(String, Vec<String>), PresetRefusal> {
    let Some((command, args)) = crate::launch::split_command(line.trim()) else {
        return Err(PresetRefusal::NoCommand);
    };
    if command.trim().is_empty() {
        return Err(PresetRefusal::NoCommand);
    }
    Ok((command, args))
}

/// Check and canonicalise a shortcut, or say why not. Empty means none.
///
/// Stored in the canonical form [`crate::launch::format_chord`] produces
/// rather than as typed, so the file never holds two spellings of one chord
/// and the matcher never has to fold anything at match time. Canonicalising
/// first is also what makes the duplicate check below exact: `alt+j` and
/// `Alt+J` are one binding and comparing the typed strings would miss it.
fn accept_shortcut(
    list: &[crate::launch::SavedPreset],
    text: &str,
    except: u64,
) -> Result<Option<String>, PresetRefusal> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let Some(chord) = crate::launch::parse_chord(text) else {
        return Err(PresetRefusal::BadShortcut(text.to_string()));
    };
    // A chord the shell already claims never reaches the dialog's keydown, so
    // storing it would produce exactly the thing this tab refuses to ship: a
    // shortcut the settings panel displays and the product never fires.
    if let Some(why) = crate::launch::chord_conflict(&chord) {
        return Err(PresetRefusal::ShortcutTaken(why));
    }
    // And again against the table key dispatch is matching right now, which
    // the check above cannot see: it reads the shipped chords, so an action
    // the operator moved ONTO this chord is invisible to it and the command
    // would be stored, displayed, and then shadowed by the rebinding.
    let candidate = crate::ui::settings::Binding {
        key: chord.key.clone(),
        ctrl: chord.ctrl,
        alt: chord.alt,
        shift: chord.shift,
    };
    let mine = crate::keymap::KeyAction::LaunchPreset(except);
    if let Some(owner) = crate::ui::settings::live_conflict(&candidate, mine) {
        return Err(PresetRefusal::ShortcutTaken(format!(
            "{} is already {}.",
            crate::launch::format_chord(&chord),
            crate::ui::settings::action_label(owner)
        )));
    }
    let canonical = crate::launch::format_chord(&chord);
    // And the same argument one level down: the dialog takes the first preset
    // in list order that matches, so a second preset on one chord is a row
    // that can never be reached by keyboard.
    if let Some(other) = list
        .iter()
        .find(|p| p.id != except && p.shortcut.as_deref() == Some(canonical.as_str()))
    {
        return Err(PresetRefusal::ShortcutInUse(canonical, other.label.clone()));
    }
    Ok(Some(canonical))
}

/// Apply one field edit to the saved command with this id.
///
/// Keyed by id and not by position, and that is not fussiness. The list on
/// disk is shared by every window: a second window that deleted a row leaves
/// this window rendering positions that no longer mean what they meant, and an
/// index-keyed edit would then rewrite the wrong row's label. An id that is
/// gone is [`PresetRefusal::Vanished`], which the panel shows and then
/// refreshes from disk.
///
/// Nothing is written unless the value is accepted, so a refused edit leaves
/// the list byte-identical.
pub fn revise(
    list: &mut [crate::launch::SavedPreset],
    id: u64,
    field: PresetField,
    value: &str,
) -> Result<(), PresetRefusal> {
    let Some(index) = list.iter().position(|p| p.id == id) else {
        return Err(PresetRefusal::Vanished);
    };
    match field {
        PresetField::Label => {
            let label = accept_label(list, value, id)?;
            list[index].label = label;
        }
        PresetField::CommandLine => {
            let (command, args) = accept_command(value)?;
            list[index].command = command;
            list[index].args = args;
        }
        PresetField::Cwd => {
            let cwd = value.trim();
            // Empty clears it rather than storing `Some("")`. An empty string
            // is a directory the picker would try to enter and the daemon
            // would refuse, which is a launch failure standing in for "no
            // opinion".
            list[index].cwd = if cwd.is_empty() {
                None
            } else {
                Some(cwd.to_string())
            };
        }
        PresetField::Shortcut => {
            let shortcut = accept_shortcut(list, value, id)?;
            list[index].shortcut = shortcut;
        }
        PresetField::Icon => {
            // An unknown slug clears rather than refuses. The picker can only
            // emit slugs it owns, so the only way to reach this is a
            // hand-edited profile or one written by a build with an icon this
            // one dropped, and losing the choice beats refusing the save.
            let slug = value.trim();
            list[index].icon = crate::ui::icons::from_slug(slug).map(|i| i.slug.to_string());
        }
    }
    Ok(())
}

/// Append a saved command, returning the id it was given.
///
/// The id comes from [`crate::launch::mint_preset_id`] and is then bumped
/// until it is free. Minting from the label and the command alone is stable,
/// which is what makes it a good id, but stable also means a label that was
/// used, renamed and used again mints the number a live row already holds.
/// Two rows with one id is the picker launching the wrong command, so the
/// collision is resolved here, at the only place that ever creates one.
pub fn create(
    list: &mut Vec<crate::launch::SavedPreset>,
    label: &str,
    command_line: &str,
) -> Result<u64, PresetRefusal> {
    let label = accept_label(list, label, u64::MAX)?;
    let (command, args) = accept_command(command_line)?;
    let mut id = crate::launch::mint_preset_id(&label, &command);
    while list.iter().any(|p| p.id == id) {
        id = id.wrapping_add(1);
    }
    list.push(crate::launch::SavedPreset {
        id,
        label,
        command,
        args,
        cwd: None,
        shortcut: None,
        icon: None,
    });
    Ok(id)
}

/// Drop the saved command with this id. False when it was already gone.
pub fn remove(list: &mut Vec<crate::launch::SavedPreset>, id: u64) -> bool {
    let before = list.len();
    list.retain(|p| p.id != id);
    list.len() != before
}

/// Move a saved command `delta` places, clamped to the ends of the list.
///
/// Returns false when the move would fall off either end, which is what
/// disables the arrow rather than leaving a button that visibly does nothing
/// at the top and bottom of the list.
pub fn move_by(list: &mut [crate::launch::SavedPreset], id: u64, delta: isize) -> bool {
    let Some(from) = list.iter().position(|p| p.id == id) else {
        return false;
    };
    let Some(to) = from.checked_add_signed(delta) else {
        return false;
    };
    if to >= list.len() || delta == 0 {
        return false;
    }
    // A rotation and not a swap: moving a row three places past two others
    // must not reverse the pair it stepped over. With `delta` of one the two
    // are the same operation, and the panel only offers one, but the function
    // is the one place the ordering is decided and it should be right for the
    // argument it takes.
    if to > from {
        list[from..=to].rotate_left(1);
    } else {
        list[to..=from].rotate_right(1);
    }
    true
}


// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Re-read the saved commands, apply one change, write them back.
///
/// Re-reads rather than trusting the signal it is about to overwrite. Every
/// window in the process edits one file, so a copy taken when this panel
/// mounted is stale the moment a second window adds a row, and writing that
/// stale copy back whole would delete the other window's work with nothing on
/// screen saying so.
///
/// The signal is only advanced when the write succeeded. A row that is on the
/// screen but not on the disk is the defect this whole tab exists to avoid, so
/// a failed write leaves the fields showing what is actually stored and puts
/// the reason underneath them.
fn edit_presets(
    mut list: Signal<Vec<crate::launch::SavedPreset>>,
    mut error: Signal<String>,
    change: impl FnOnce(&mut Vec<crate::launch::SavedPreset>) -> Result<(), PresetRefusal>,
) {
    let mut next = crate::launch::presets_saved();
    match change(&mut next) {
        // Nothing was mutated: every operation validates before it writes. The
        // list is still advanced because the re-read above may itself be news,
        // which is the case `Vanished` is reporting.
        Err(why) => {
            error.set(why.to_string());
            list.set(next);
        }
        Ok(()) => match crate::launch::save_presets(&next) {
            Ok(()) => {
                error.set(String::new());
                // A preset's chord lives in the SAME table the built-in
                // chords do, so saving one has to re-announce that table or
                // the shortcut the operator just bound does nothing until the
                // app restarts. Presets are not part of `Settings`, so the
                // commit path that normally announces never runs for them:
                // this is the one place that closes the link.
                crate::state::live::publish_presets(&next);
                list.set(next);
            }
            Err(why) => error.set(format!(
                "The saved commands could not be written: {why}. Nothing on disk changed."
            )),
        },
    }
}

/// The saved-command editor.
///
/// Takes no props, and that is a statement about where the data lives. Saved
/// commands are not in [`Settings`]: they are a list of records the operator
/// authored, they are consumed by [`crate::launch`] rather than by any
/// derivation in this module, and putting them in the settings document would
/// have meant every window's `save_prefs` rewriting them on every unrelated
/// preference change.
///
/// Editing is direct. There is no "edit preset" sub-dialog, because a dialog
/// inside a dialog gives the escape key two meanings that nothing on screen
/// distinguishes, and because a four-field record is smaller than the modal
/// that would frame it.
///
/// # Every field commits on `onchange`, and none on `oninput`
///
/// Measured, not preferred. A text input whose `value` is bound to a signal
/// and whose `oninput` writes that signal re-renders the panel on every
/// keystroke, and the re-render writes `value` back into the DOM node while
/// the operator is still typing into it. Characters are lost. Driving the
/// running binary through xdotool at a 20 ms inter-key delay, the two create
/// fields in this panel took `Missing agent` as `Misn aet` and
/// `no-such-agent-xyz --flag` as `n-uh-agt-xy -flag`, while the row fields
/// beside them, which already committed on `onchange`, took a 16-character
/// path at the same delay with every character intact.
///
/// So nothing in this file reads a half-typed field. `onchange` fires on
/// blur, and the blur that a click on the primary button causes is dispatched
/// before that button's click, which is what makes reading the signal in the
/// click handler correct. The same defect was in the Workspaces panel's two
/// name fields and the Advanced panel's daemon URL, and all three are fixed
/// the same way.
#[component]
pub(super) fn PresetsPanel(state: Signal<UiState>) -> Element {
    let list = use_signal(crate::launch::presets_saved);
    let error = use_signal(String::new);
    let mut new_label = use_signal(String::new);
    let mut new_command = use_signal(String::new);

    let rows = list();
    let count = rows.len();

    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "Saved commands" }
            span { class: "rg-field__desc",
                "A label, a program, and its arguments. Saved commands appear in the \
                 new-session dialog's picker, so the agent you start twenty times a day is one \
                 click rather than a retyped command line. A shortcut starts its command while \
                 that dialog is open; nothing binds these keys anywhere else."
            }
        }

        // Above the list. See the same banner in `WorkspacesPanel`.
        if !error.read().is_empty() {
            div { class: "rg-sheet__error", "{error}" }
        }

        if rows.is_empty() {
            div { class: "rg-preset__empty",
                "None saved yet. The new-session dialog still accepts any command line; a saved \
                 command is for the ones you type often."
            }
        }

        for (index , preset) in rows.iter().cloned().enumerate() {
            {
                let id = preset.id;
                // One PATH walk per row, on a panel that re-renders only when
                // a field is committed. It is the same check the dialog runs
                // before it spawns, run early enough to be useful.
                let fault = crate::launch::preset_fault(&preset);
                let line = crate::launch::join_command(&preset.command, &preset.args);
                let cwd = preset.cwd.clone().unwrap_or_default();
                let shortcut = preset.shortcut.clone().unwrap_or_default();
                rsx! {
                    div { class: "rg-field rg-field--preset", key: "{id}",
                        input {
                            class: "rg-field__input rg-field__input--prose rg-preset__label",
                            r#type: "text",
                            value: "{preset.label}",
                            spellcheck: false,
                            autocomplete: "off",
                            aria_label: "Label",
                            onchange: move |e| {
                                let text = e.value();
                                edit_presets(
                                    list,
                                    error,
                                    |l| revise(l, id, PresetField::Label, &text),
                                );
                            },
                        }
                        span { class: "rg-field__control",
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                disabled: index == 0,
                                aria_label: "Move up",
                                onclick: move |_| {
                                    edit_presets(
                                        list,
                                        error,
                                        |l| {
                                            move_by(l, id, -1).then_some(()).ok_or(PresetRefusal::Vanished)
                                        },
                                    );
                                },
                                "\u{2191}"
                            }
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                disabled: index + 1 >= count,
                                aria_label: "Move down",
                                onclick: move |_| {
                                    edit_presets(
                                        list,
                                        error,
                                        |l| {
                                            move_by(l, id, 1).then_some(()).ok_or(PresetRefusal::Vanished)
                                        },
                                    );
                                },
                                "\u{2193}"
                            }
                            button {
                                class: "rg-btn rg-btn--danger",
                                r#type: "button",
                                onclick: move |_| {
                                    edit_presets(
                                        list,
                                        error,
                                        |l| remove(l, id).then_some(()).ok_or(PresetRefusal::Vanished),
                                    );
                                },
                                "Delete"
                            }
                        }
                        input {
                            class: "rg-field__input rg-preset__cmd",
                            r#type: "text",
                            value: "{line}",
                            spellcheck: false,
                            autocomplete: "off",
                            placeholder: "Command and arguments",
                            aria_label: "Command and arguments",
                            onchange: move |e| {
                                let text = e.value();
                                edit_presets(
                                    list,
                                    error,
                                    |l| revise(l, id, PresetField::CommandLine, &text),
                                );
                            },
                        }
                        input {
                            class: "rg-field__input rg-preset__cwd",
                            r#type: "text",
                            value: "{cwd}",
                            spellcheck: false,
                            autocomplete: "off",
                            placeholder: "Working directory, or the dialog's",
                            aria_label: "Default working directory",
                            onchange: move |e| {
                                let text = e.value();
                                edit_presets(list, error, |l| revise(l, id, PresetField::Cwd, &text));
                            },
                        }
                        input {
                            class: "rg-field__input rg-preset__key",
                            r#type: "text",
                            value: "{shortcut}",
                            spellcheck: false,
                            autocomplete: "off",
                            placeholder: "Shortcut",
                            aria_label: "Shortcut",
                            onchange: move |e| {
                                let text = e.value();
                                edit_presets(
                                    list,
                                    error,
                                    |l| revise(l, id, PresetField::Shortcut, &text),
                                );
                            },
                        }
                        if let Some(fault) = fault {
                            span { class: "rg-field__hint rg-preset__fault", "{fault.sentence()}" }
                        }
                        crate::ui::icons::IconPicker {
                            selected: preset.icon.clone(),
                            command_line: line.clone(),
                            on_pick: move |slug: Option<String>| {
                                let text = slug.unwrap_or_default();
                                edit_presets(
                                    list,
                                    error,
                                    |l| revise(l, id, PresetField::Icon, &text),
                                );
                            },
                        }
                    }
                }
            }
        }

        div { class: "rg-field rg-field--preset-new",
            input {
                class: "rg-field__input rg-field__input--prose rg-preset__label",
                r#type: "text",
                value: "{new_label}",
                spellcheck: false,
                autocomplete: "off",
                placeholder: "Label",
                aria_label: "New saved command label",
                onchange: move |e| new_label.set(e.value()),
            }
            input {
                class: "rg-field__input rg-preset__cmd",
                r#type: "text",
                value: "{new_command}",
                spellcheck: false,
                autocomplete: "off",
                placeholder: "Command and arguments",
                aria_label: "New saved command line",
                onchange: move |e| new_command.set(e.value()),
            }
            span { class: "rg-field__control",
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    onclick: move |_| {
                        let label = new_label.peek().clone();
                        let command = new_command.peek().clone();
                        edit_presets(list, error, |l| create(l, &label, &command).map(|_| ()));
                        if error.peek().is_empty() {
                            new_label.set(String::new());
                            new_command.set(String::new());
                        }
                    },
                    "Save command"
                }
            }
        }
    }
}

/// The saved-command editor, which is the only writer of `launch.json`'s
/// preset list.
///
/// Every test here defends one invariant the new-session dialog is entitled to
/// assume, because it consumes this list and cannot re-validate it: labels are
/// unique and non-empty, ids are unique, a stored shortcut is one the matcher
/// can match, and a stored working directory is either a real string or
/// absent. A refused edit leaves the list byte-identical, so a validation
/// failure can never be a partial write.
#[cfg(test)]
mod saved_commands;
