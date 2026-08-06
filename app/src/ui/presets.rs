//! Saved presets, as chips the operator clicks.
//!
//! A preset is a command the operator already decided is worth keeping. The
//! launcher's ranked list can offer one, but only ever as one row among nine
//! competing with history, `PATH` discovery and whatever is running: the thing
//! you deliberately saved could be pushed below the fold by things you never
//! chose. That is the defect this exists for. A preset the operator saved is
//! shown unconditionally, in the order they saved it, and starting one is a
//! click on the thing itself rather than a search for it.
//!
//! Chips, not a list, and that is the whole distinction from
//! [`crate::ui::recents`]: recents answer "what was I just doing", are ranked
//! by time and read as a column of sentences. Presets are a small fixed set of
//! named buttons, so they wrap across the width instead of consuming nine rows
//! of vertical space above the list that is still the primary surface.
//!
//! Validation happens on the click, never per render, for the reason the
//! launcher documents: [`launch::preset_fault`] is a `stat` and a `PATH` walk,
//! and doing it while drawing would put both on every keystroke of the surface
//! hosting this. A preset that cannot run does not vanish and does not launch;
//! it says which part of it is missing.

use dioxus::prelude::*;

use crate::launch::{self, Launch, SavedPreset};
use crate::ui::icons::{IconGlyph, resolve};

#[derive(Props, Clone, PartialEq)]
pub struct PresetsProps {
    /// In saved order, exactly as the profile holds them.
    pub presets: Vec<SavedPreset>,
    /// The directory a preset runs in when it pins none. The launcher's `in`
    /// field, resolved.
    pub here: String,
    /// A chip was taken and validated. The caller sends it.
    pub on_launch: EventHandler<Launch>,
}

/// Draw the saved presets, and emit the launch when one is taken.
///
/// Renders nothing at all when there are none. An empty band with a heading
/// would teach that presets exist while giving no way to make one; the run
/// field's own Save control is what teaches that, at the moment there is
/// something worth saving.
#[component]
pub fn Presets(props: PresetsProps) -> Element {
    let mut said = use_signal(|| None::<String>);
    let presets = props.presets.clone();

    if presets.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "rg-presets",
            div { class: "rg-presets__head", "Saved" }
            ul {
                class: "rg-presets__list",
                role: "listbox",
                aria_label: "Saved presets",
                for preset in presets.iter() {
                    {
                        let line = launch::join_command(&preset.command, &preset.args);
                        let icon = *resolve(preset.icon.as_deref(), &line);
                        let label = preset.label.clone();
                        let chord = preset.shortcut.clone();
                        // The tooltip is the exact line that will run, or the
                        // reason it will not. A chip is a short label by
                        // design, so the full truth lives here.
                        let tip = crate::ui::dialog::preset_tip(preset);
                        let taken = preset.clone();
                        let here = props.here.clone();
                        rsx! {
                            li {
                                class: "rg-presets__chip",
                                key: "{preset.id}",
                                role: "option",
                                aria_selected: "false",
                                title: "{tip}",
                                // Off mousedown, so the launcher's query field
                                // does not blur and close the surface before
                                // the click lands.
                                onmousedown: move |e| e.prevent_default(),
                                onclick: move |_| {
                                    match launch::preset_fault(&taken) {
                                        Some(fault) => said.set(Some(fault.sentence())),
                                        None => match launch::preset_launch(&taken, &here) {
                                            Ok(l) => {
                                                said.set(None);
                                                props.on_launch.call(l);
                                            }
                                            Err(why) => said.set(Some(why)),
                                        },
                                    }
                                },
                                IconGlyph { icon, class: "rg-presets__icon" }
                                span { class: "rg-presets__text", "{label}" }
                                if let Some(keys) = chord {
                                    kbd { class: "rg-presets__chord", "{keys}" }
                                }
                            }
                        }
                    }
                }
            }
            if let Some(msg) = said() {
                div { class: "rg-presets__note", "{msg}" }
            }
        }
    }
}

#[cfg(test)]
mod tests;
