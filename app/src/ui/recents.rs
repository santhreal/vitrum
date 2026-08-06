//! The last few things this operator started, as a list they can click.
//!
//! The launcher ranks a whole history by frequency and recency and shows the
//! best nine for whatever is typed. This is the other question: "what was I
//! just doing, and where". It is keyed on the command AND the directory, so
//! the same agent in two checkouts is two rows, and it is stored in the order
//! it will be drawn in, so a surface that renders it does no ranking and takes
//! no clock reading.
//!
//! Markup follows [`crate::ui::dialog`]'s launcher: a `ul` listbox of `li`
//! rows, a glyph slot that is always emitted so every row shares one text
//! column, the command as the row's text, and the place as a chip with the
//! absolute path on its `title`. Taking a row is a click, not a mousedown, and
//! the note line under the list is the only thing this surface is allowed to
//! say.

use dioxus::prelude::*;
use vitrum_proto::ProjectInfo;

use crate::launch::{self, Launch, RecentEntry};
use crate::ui::dialog::place_of;
use crate::ui::icons::{IconGlyph, resolve};

#[derive(Props, Clone, PartialEq)]
pub struct RecentsProps {
    /// Newest first, exactly as [`launch::recents`] hands them back.
    pub entries: Vec<RecentEntry>,
    /// Known projects, so a row's place reads `vitrum/app` rather than as a
    /// 47-character absolute path.
    pub projects: Vec<ProjectInfo>,
    /// This user's home, for shortening a path outside any project.
    pub home: String,
    /// A row was taken and validated. The caller sends it.
    pub on_launch: EventHandler<Launch>,
}

/// List the recent commands, and emit the launch when one is taken.
///
/// Validation happens on the click and nowhere else. [`launch::recent_launch`]
/// is one `stat` and one `PATH` walk; doing it per render would put both on
/// every keystroke of whatever surface hosts this, which is the defect the
/// launcher already fixed once.
///
/// A row whose directory has been deleted since it ran does not vanish and
/// does not launch: it reports the sentence naming the directory. Hiding it
/// would leave the operator wondering where the row went.
#[component]
pub fn Recents(props: RecentsProps) -> Element {
    let mut said = use_signal(|| None::<String>);
    let entries = props.entries.clone();

    if entries.is_empty() {
        return rsx! {
            div { class: "rg-recents",
                div { class: "rg-recents__note", "Nothing started yet." }
            }
        };
    }

    rsx! {
        div { class: "rg-recents",
            ul {
                class: "rg-recents__list",
                role: "listbox",
                aria_label: "Recent commands",
                for (i, entry) in entries.iter().enumerate() {
                    {
                        let line = launch::recent_line(entry);
                        let icon = *resolve(entry.icon.as_deref(), &line);
                        let place = place_of(&props.projects, &entry.cwd, &props.home);
                        let cwd = entry.cwd.clone();
                        let taken = entry.clone();
                        rsx! {
                            li {
                                class: "rg-recents__row",
                                key: "{line}|{cwd}",
                                role: "option",
                                aria_selected: "false",
                                title: "{line} in {cwd}",
                                // Off mousedown, so a surface that owns focus
                                // does not blur and close before the click
                                // lands.
                                onmousedown: move |e| e.prevent_default(),
                                onclick: move |_| {
                                    match launch::recent_launch(&taken) {
                                        Ok(l) => {
                                            said.set(None);
                                            props.on_launch.call(l);
                                        }
                                        Err(why) => said.set(Some(why)),
                                    }
                                },
                                span { class: "rg-recents__key", "{i + 1}" }
                                IconGlyph { icon, class: "rg-recents__icon" }
                                span { class: "rg-recents__text", "{line}" }
                                span { class: "rg-recents__place", title: "{cwd}", "{place}" }
                            }
                        }
                    }
                }
            }
            if let Some(msg) = said() {
                div { class: "rg-recents__note", "{msg}" }
            }
        }
    }
}

/// What the list draws, and in what order.
#[cfg(test)]
mod tests;
