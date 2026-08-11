//! The keyboard reference overlay.
//!
//! Rendered from [`crate::ui::settings::effective_help_rows`], which is
//! `keymap::CHORDS` folded with the operator's rebindings. That fold, and not
//! the raw table, is also what key dispatch matches a key press against, so
//! the overlay and the live keyboard cannot disagree.
//!
//! # Why this stopped reading `CHORDS` directly
//!
//! It used to, and `keymap::tests::every_chord_is_documented` guaranteed that
//! every chord had a row somewhere a user could find. That guarantee is intact
//! and still worth having. What it never covered, because rebinding did not
//! exist when it was written, is whether the row shows the chord that FIRES.
//! Rendering `CHORDS` after rebinding shipped would have advertised the default
//! for every action the operator had moved: a documented chord that does
//! nothing, and a live chord documented nowhere. That is strictly worse than no
//! overlay, because the overlay is the one place the product explains itself
//! and a user who catches it lying once stops reading it.
//!
//! So the invariant this file now defends is the stronger one — **the
//! documented chord is the effective chord** — and it is asserted here, in
//! [`tests`], rather than in `keymap.rs`, because `keymap.rs` cannot see the
//! overrides.
//!
//! Reachable three ways, all of them listed inside it: `F1` anywhere, `?`
//! outside a text field, `Ctrl+/` outside the terminal, plus the `?` button at
//! the end of the tab strip for anyone who never presses a function key.

use dioxus::prelude::*;

use crate::keymap::GROUPS;
use crate::state::UiState;
use crate::ui::settings::effective_help_rows;

/// Why the primary modifier is Ctrl on macOS too.
///
/// Cmd+Tab is the macOS application switcher and is intercepted by the window
/// server before any application sees it, so a Cmd-based tab traversal would be
/// documented and dead. Saying so beats letting a Mac user conclude the
/// shortcuts are broken.
const MODIFIER_NOTE: &str = "vitrum uses Ctrl on every platform, macOS included, so Cmd+Tab stays the system application switcher.";

/// Shown when anything has been rebound, so a screenshot of this overlay is
/// never mistaken for the shipped defaults.
const REBOUND_NOTE: &str = "Some shortcuts have been changed in Settings \u{203a} Keyboard. The chords below are the ones that fire.";

#[derive(Props, Clone, PartialEq)]
pub struct ShortcutsProps {
    pub state: Signal<UiState>,
    pub on_dismiss: EventHandler<()>,
}

#[component]
pub fn Shortcuts(props: ShortcutsProps) -> Element {
    let (rows, rebound) = {
        let read = props.state.read();
        let prefs = &read.daemon.settings.keyboard;
        (effective_help_rows(prefs), !prefs.overrides.is_empty())
    };

    rsx! {
        div {
            class: "rg-layer rg-layer--dim",
            onclick: move |_| props.on_dismiss.call(()),
            div {
                class: "rg-sheet rg-sheet--shortcuts",
                role: "dialog",
                aria_label: "Keyboard shortcuts",
                onclick: move |e| e.stop_propagation(),

                div { class: "rg-sheet__head",
                    span { class: "rg-sheet__title", "Keyboard" }
                    button {
                        class: "rg-btn-inline",
                        r#type: "button",
                        onclick: move |_| props.on_dismiss.call(()),
                        "Close"
                    }
                }

                div { class: "rg-keys",
                    for group in GROUPS {
                        div { class: "rg-keys__group", key: "{group.title()}",
                            div { class: "rg-keys__heading", "{group.title()}" }
                            for row in rows.iter().filter(|row| row.group == group) {
                                div { class: "rg-keys__row", key: "{row.keys}",
                                    // One chip per alternative, not one chip
                                    // holding a slash. `kbd` is `white-space:
                                    // nowrap`, so a combined
                                    // "Ctrl+Shift+Tab / Ctrl+Shift+PageUp"
                                    // could neither wrap nor shrink: it
                                    // overflowed its fixed column and drew on
                                    // top of the description beside it. Split,
                                    // the surrounding spaces give the line a
                                    // legal break and each chord stays intact.
                                    span { class: "rg-keys__chord",
                                        for (i, chord) in row.keys.split(" / ").enumerate() {
                                            span { key: "{chord}",
                                                if i > 0 {
                                                    " / "
                                                }
                                                kbd { "{chord}" }
                                            }
                                        }
                                    }
                                    span { class: "rg-keys__what", "{row.what}" }
                                }
                            }
                        }
                    }
                    // The pane's own chords, in their own section, after every
                    // shell binding. They are not in `rows` and must not be:
                    // `rows` is folded from the table dispatch matches, and an
                    // entry there would be claimed by the shell before the
                    // pane ever saw the key.
                    div { class: "rg-keys__group", key: "{crate::keymap::PANE_SECTION_TITLE}",
                        div { class: "rg-keys__heading", "{crate::keymap::PANE_SECTION_TITLE}" }
                        for row in crate::keymap::PANE_CHORDS {
                            div { class: "rg-keys__row", key: "{row.keys}",
                                span { class: "rg-keys__chord", kbd { "{row.keys}" } }
                                span { class: "rg-keys__what", "{row.what}" }
                            }
                        }
                    }
                }

                if rebound {
                    div { class: "rg-sheet__note", "{REBOUND_NOTE}" }
                }
                div { class: "rg-sheet__note", "{MODIFIER_NOTE}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Group, HelpRow, KeyAction};
    use crate::state::KeyboardPrefs;
    use crate::ui::settings::{Binding, set_override};

    fn defaults() -> Vec<HelpRow> {
        effective_help_rows(&KeyboardPrefs::default())
    }

    /// With nothing rebound the overlay must be exactly what `keymap.rs`
    /// documents. Any drift changes what a user who never opened the settings
    /// is told about their own keyboard.
    #[test]
    fn an_untouched_install_shows_the_documented_defaults() {
        assert_eq!(defaults(), crate::keymap::help_rows());
    }

    /// **The invariant the Terminal section exists for.** A chord the pane
    /// consumes must not be claimed by the shell.
    ///
    /// The pane receives Ctrl+Shift+C, V and G only because dispatch finds no
    /// match for them and passes them on. Adding any of the three to `CHORDS`
    /// would take them away silently: the shell would claim the key, the pane
    /// would never see it, and the overlay would still be documenting a copy
    /// shortcut that no longer copies. Nothing about a new entry in that table
    /// announces which pane behaviour it just broke, so the absence is
    /// asserted here rather than trusted.
    ///
    /// Rebinding is included on purpose. An operator is free to move a shell
    /// action onto Ctrl+Shift+C, and doing so genuinely does take copy away;
    /// that is their decision and the settings surface reports the conflict.
    /// What this forbids is the SHIPPED table doing it, where nobody chose it.
    #[test]
    fn the_shell_claims_none_of_the_chords_the_pane_documents() {
        let table = crate::ui::settings::live_chords(&KeyboardPrefs::default(), &[]);
        for pane in crate::keymap::PANE_CHORDS {
            let claimed = table.iter().find(|chord| {
                chord.rendered() == pane.keys
                    && crate::keys::allows(chord.scope, crate::keys::Focus::Terminal, false)
            });
            assert!(
                claimed.is_none(),
                "{} is documented as a terminal chord and the shell claims it \
                 for {:?}, so the pane never receives it",
                pane.keys,
                claimed.map(|c| c.action)
            );
        }
    }

    /// **The invariant this file exists for.** A rebound action must be
    /// advertised at its NEW chord. Showing the default would document a chord
    /// that does nothing and leave the live one documented nowhere, which is
    /// the failure mode that made rendering `CHORDS` directly unacceptable once
    /// rebinding shipped.
    #[test]
    fn a_rebound_action_is_documented_at_the_chord_that_fires() {
        let mut prefs = KeyboardPrefs::default();
        set_override(
            &mut prefs,
            KeyAction::ToggleSidebar,
            &Binding {
                key: "j".to_string(),
                ctrl: true,
                alt: true,
                shift: false,
            },
        );
        let rows = effective_help_rows(&prefs);
        let row = rows
            .iter()
            .find(|row| row.what == sidebar_row_text())
            .expect("toggling the sidebar is still documented");
        assert_eq!(
            row.keys, "Ctrl+Alt+J",
            "the overlay is advertising a chord that no longer fires"
        );
        assert!(
            !rows.iter().any(|row| row.keys == "Ctrl+B"),
            "the old chord is still advertised as live: {rows:?}"
        );
    }

    /// The chord in the overlay must be a chord that is genuinely in the table
    /// key dispatch matches on, for every row, rebound or not. This is the
    /// end-to-end statement; the test above is its single interesting case.
    ///
    /// Compared against the effective chord LIST rather than against rendered
    /// text, because the table stores raw key names (`arrowdown`) while the
    /// overlay prints display names (`Down`). Reverse-mapping the display name
    /// back to a key would be a third copy of that table and would have made
    /// this test assert something subtly weaker than it claims.
    ///
    /// A row documenting several chords at once is split and EVERY alternative
    /// checked, rather than skipped. Skipping is what let the alias rows drift:
    /// "Ctrl+Tab / Ctrl+PageDown" is one row for two chords, and a rebinding
    /// that moved the action would have left both advertised and neither live.
    #[test]
    fn every_documented_chord_appears_in_the_table_dispatch_matches() {
        let mut prefs = KeyboardPrefs::default();
        set_override(
            &mut prefs,
            KeyAction::NewSession,
            &Binding {
                key: "9".to_string(),
                ctrl: true,
                alt: true,
                shift: true,
            },
        );
        let live = crate::ui::settings::live_chords(&prefs, &[]);
        for row in effective_help_rows(&prefs) {
            // The positional slots are one row for nine chords written as a
            // range. They are excluded from rebinding for exactly that reason,
            // so the literal cannot go stale and there is nothing to check.
            if row.keys.contains(" - ") {
                continue;
            }
            for alternative in row.keys.split(" / ") {
                assert!(
                    live.iter().any(|chord| chord.rendered() == alternative),
                    "the overlay documents {alternative} (in row \"{}\") but no chord in the live \
                     table fires on it",
                    row.keys
                );
            }
        }

        // And the rebound one specifically reached both halves.
        assert!(
            effective_help_rows(&prefs)
                .iter()
                .any(|row| row.keys == "Ctrl+Alt+Shift+9")
        );
        assert!(
            live.iter().any(|chord| chord.action == KeyAction::NewSession
                && chord.key == "9"
                && chord.ctrl
                && chord.alt),
            "the rebound chord never reached the table dispatch matches"
        );
    }

    /// **Regression, found by the test above.** Rebinding an action whose help
    /// row lists ALIASES must drop the alias list. "Ctrl+Tab / Ctrl+PageDown"
    /// is one row for two chords; rebinding moves both onto the new binding, so
    /// leaving the literal in place would advertise two chords that no longer
    /// fire and hide the one that does — two undiscoverable bindings from one
    /// stale string.
    #[test]
    fn rebinding_an_alias_row_drops_the_alias_list() {
        let defaults = effective_help_rows(&KeyboardPrefs::default());
        assert!(
            defaults
                .iter()
                .any(|row| row.keys == "Ctrl+Tab / Ctrl+PageDown"),
            "the alias row this test is about no longer exists: {defaults:?}"
        );

        let mut prefs = KeyboardPrefs::default();
        set_override(
            &mut prefs,
            KeyAction::NextTab,
            &Binding {
                key: "]".to_string(),
                ctrl: true,
                alt: false,
                shift: false,
            },
        );
        let rows = effective_help_rows(&prefs);
        assert!(
            !rows.iter().any(|row| row.keys.contains("Ctrl+Tab")),
            "the overlay still advertises the old alias pair: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.keys == "Ctrl+]"),
            "the overlay does not advertise the chord that now fires: {rows:?}"
        );
    }

    /// Every section the overlay renders must have rows. An empty section
    /// paints a heading over blank space, which reads as a bug in the overlay
    /// rather than as an empty category.
    #[test]
    fn every_rendered_section_has_rows() {
        let rows = defaults();
        for group in GROUPS {
            assert!(
                rows.iter().any(|row| row.group == group),
                "the {} section would render as a heading over nothing",
                group.title()
            );
        }
    }

    /// The overlay must document the way it is itself opened and closed. A
    /// help sheet you cannot find, or cannot get out of, is worse than none.
    #[test]
    fn the_overlay_documents_its_own_keys() {
        let rows = defaults();
        assert!(rows.iter().any(|row| row.keys.contains("F1")));
        assert!(rows.iter().any(|row| row.keys.contains("Esc")));
    }

    /// Rows must be unique. Two identical chord strings mean one of them is a
    /// stale duplicate and the user has no way to tell which is live. This
    /// matters more with rebinding than it did without: a rebinding that
    /// collides would show up here as two rows carrying the same chord.
    #[test]
    fn no_two_rows_show_the_same_chord() {
        let rows = defaults();
        let mut keys: Vec<&str> = rows.iter().map(|row| row.keys.as_str()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "a duplicate chord in {rows:?}");
    }

    /// Every row must say what the key does in plain words. A blank
    /// description column is a row that documents nothing.
    #[test]
    fn every_row_describes_its_action() {
        for row in defaults() {
            assert!(
                !row.what.trim().is_empty(),
                "{} has no description",
                row.keys
            );
            assert!(
                !row.keys.trim().is_empty(),
                "\"{}\" is documented with no chord",
                row.what
            );
        }
    }

    /// A rebound install must say so. Without the note, a screenshot of this
    /// overlay would be read as the shipped defaults by anyone who did not do
    /// the rebinding themselves.
    #[test]
    fn the_overlay_admits_when_it_is_not_showing_defaults() {
        assert!(REBOUND_NOTE.contains("Settings"), "{REBOUND_NOTE}");
        let mut prefs = KeyboardPrefs::default();
        assert!(prefs.overrides.is_empty());
        set_override(
            &mut prefs,
            KeyAction::CloseTab,
            &Binding {
                key: "y".to_string(),
                ctrl: true,
                alt: false,
                shift: false,
            },
        );
        assert!(
            !prefs.overrides.is_empty(),
            "the note's trigger condition never becomes true"
        );
    }

    /// The macOS note must name the reason. "Use Ctrl" without saying why
    /// reads as an oversight; naming Cmd+Tab makes it a decision.
    #[test]
    fn the_modifier_note_explains_itself() {
        assert!(MODIFIER_NOTE.contains("Cmd+Tab"));
        assert!(MODIFIER_NOTE.contains("Ctrl"));
    }

    /// The text the rebinding test looks for, taken from the table rather than
    /// typed twice, so rewording the description does not silently turn that
    /// test into a no-op.
    fn sidebar_row_text() -> &'static str {
        crate::keymap::CHORDS
            .iter()
            .find(|chord| chord.action == KeyAction::ToggleSidebar && chord.help.is_some())
            .and_then(|chord| chord.help)
            .map(|help| help.what)
            .expect("toggling the sidebar is a documented chord")
    }

    /// Sections are rendered in one fixed order, so the overlay does not
    /// reshuffle between openings.
    #[test]
    fn the_section_order_is_fixed() {
        assert_eq!(GROUPS.len(), 4);
        assert_eq!(GROUPS[0], Group::Switching);
    }
}
