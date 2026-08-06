//! The Workspaces tab.
//!
//! The one panel in the sheet that does not edit [`crate::state::Settings`].
//! It drives [`crate::state::WorkspaceSet`] directly: create, rename, delete,
//! reorder, grouping mode, band visibility and folders. That is why it is the
//! only caller of [`super::try_edit`]: a workspace operation can be refused,
//! and "you cannot delete the workspace holding four sessions" has to reach
//! the operator as a sentence rather than as a button that does nothing.

use dioxus::prelude::*;

use crate::state::{FolderId, Grouping, WorkspaceId};

use super::{PanelProps, SelectRow, SwitchRow, edit_state, try_edit};

#[component]
pub(super) fn WorkspacesPanel(props: PanelProps) -> Element {
    let state = props.state;
    let snapshot = state.read();
    let workspaces: Vec<(WorkspaceId, String, usize)> = snapshot
        .daemon
        .workspaces
        .iter()
        .map(|w| {
            (
                w.id,
                w.display_name().to_string(),
                snapshot.daemon.workspaces.session_count(w.id),
            )
        })
        .collect();
    let count = workspaces.len();
    let intake = snapshot.daemon.workspaces.intake();
    let viewing = snapshot.window.workspace;
    let selected = snapshot
        .daemon
        .workspaces
        .get(viewing)
        .map(|w| (w.display_name().to_string(), w.grouping, w.sections));
    let folders: Vec<(FolderId, String)> = snapshot
        .daemon
        .workspaces
        .get(viewing)
        .map(|w| w.folders().iter().map(|f| (f.id, f.name.clone())).collect())
        .unwrap_or_default();
    drop(snapshot);

    let error = use_signal(String::new);
    let mut new_name = use_signal(String::new);
    let mut new_folder = use_signal(String::new);

    rsx! {
        div { class: "rg-field",
            span { class: "rg-field__label", "Workspaces" }
            span { class: "rg-field__desc",
                "A workspace is a separate top-level context, above projects. Every session \
                 belongs to exactly one, so a new workspace starts genuinely empty. New sessions \
                 land in whichever workspace you are looking at."
            }
        }

        // Above the list, not below it. A refusal rendered after a long list
        // is off the bottom of the scroller, and a message nobody can see
        // without scrolling is the same as no message: the control looks like
        // it silently did nothing. Measured in the running binary, where a
        // refused shortcut put its sentence three scroll notches below the
        // fold.
        if !error.read().is_empty() {
            div { class: "rg-sheet__error", "{error}" }
        }

        for (index , (id , name , sessions)) in workspaces.iter().cloned().enumerate() {
            div {
                class: if id == viewing { "rg-field rg-field--ws rg-field--ws-active" } else { "rg-field rg-field--ws" },
                key: "{id.0}",

                input {
                    class: "rg-field__input rg-field__input--prose",
                    r#type: "text",
                    value: "{name}",
                    spellcheck: false,
                    autocomplete: "off",
                    aria_label: "Workspace name",
                    onchange: move |e| {
                        let text = e.value();
                        try_edit(state, error, |st| st.daemon.workspaces.rename(id, &text));
                    },
                }

                span { class: "rg-field__hint",
                    if sessions == 1 { "1 session" } else { "{sessions} sessions" }
                    if id == intake { " · new sessions land here" }
                }

                span { class: "rg-field__control",
                    button {
                        class: "rg-btn",
                        r#type: "button",
                        disabled: id == viewing,
                        onclick: move |_| {
                            let now = crate::tick().now_ms;
                            try_edit(state, error, |st| st.set_workspace(id, now));
                        },
                        if id == viewing { "Viewing" } else { "Switch to" }
                    }
                    button {
                        class: "rg-btn",
                        r#type: "button",
                        disabled: index == 0,
                        aria_label: "Move up",
                        onclick: move |_| {
                            try_edit(
                                state,
                                error,
                                |st| st.daemon.workspaces.move_to(id, index.saturating_sub(1)),
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
                            try_edit(state, error, |st| st.daemon.workspaces.move_to(id, index + 1));
                        },
                        "\u{2193}"
                    }
                    button {
                        class: "rg-btn rg-btn--danger",
                        r#type: "button",
                        onclick: move |_| {
                            let now = crate::tick().now_ms;
                            try_edit(state, error, |st| st.delete_workspace(id, now));
                        },
                        "Delete"
                    }
                }
            }
        }

        div { class: "rg-field",
            span { class: "rg-field__label", "New workspace" }
            span { class: "rg-field__control",
                input {
                    class: "rg-field__input rg-field__input--prose",
                    r#type: "text",
                    placeholder: "Name",
                    value: "{new_name}",
                    spellcheck: false,
                    autocomplete: "off",
                    aria_label: "New workspace name",
                    // `onchange`, never `oninput`. See `PresetsPanel`.
                    onchange: move |e| new_name.set(e.value()),
                }
                button {
                    class: "rg-btn rg-btn--primary",
                    r#type: "button",
                    onclick: move |_| {
                        let name = new_name.peek().clone();
                        let before = state.peek().daemon.workspaces.len();
                        try_edit(state, error, |st| st.create_workspace(&name));
                        if state.peek().daemon.workspaces.len() > before {
                            new_name.set(String::new());
                        }
                    },
                    "Create"
                }
            }
        }

        if let Some((name, grouping, sections)) = selected {
            div { class: "rg-field",
                span { class: "rg-field__label", "{name}" }
                span { class: "rg-field__desc",
                    "Grouping and band visibility belong to the workspace, not to you: \
                     \u{201c}this one is my review queue, show me settled work\u{201d} is a fact \
                     about the context and not about the person."
                }
            }

            SelectRow {
                label: "Group rows by",
                desc: match grouping {
                    Grouping::Directory => "A session under a project root the daemon knows files under that project; everything else gets a bucket per directory."
                        .to_string(),
                    Grouping::Named => "Folders you create, in your order, plus an Unfiled bucket. Move rows between folders from the right-click menu."
                        .to_string(),
                },
                value: match grouping {
                    Grouping::Directory => "directory",
                    Grouping::Named => "named",
                }
                    .to_string(),
                options: vec![
                    ("directory".to_string(), Grouping::Directory.label().to_string()),
                    ("named".to_string(), Grouping::Named.label().to_string()),
                ],
                onpick: move |v: String| {
                    edit_state(
                        state,
                        |st| {
                            if let Some(w) = st.daemon.workspaces.get_mut(viewing) {
                                w.grouping = if v == "named" {
                                    Grouping::Named
                                } else {
                                    Grouping::Directory
                                };
                            }
                        },
                    );
                },
            }

            for (disposition , label , on) in [
                (vitrum_model::Disposition::Active, "Active", sections.active),
                (vitrum_model::Disposition::Woke, "Woke", sections.woke),
                (vitrum_model::Disposition::Snoozed, "Snoozed", sections.snoozed),
                (vitrum_model::Disposition::Settled, "Settled", sections.settled),
            ] {
                SwitchRow {
                    key: "{label}",
                    // `format!`, not `"Show {label}".to_string()`: rsx
                    // interpolates text nodes and attribute values, not a
                    // string literal being passed to `.to_string()`, so the
                    // latter ships the four literal characters `{lab` … to the
                    // screen. It did, and the screenshot caught it.
                    label: format!("Show {label}"),
                    desc: String::new(),
                    on,
                    onchange: move |want| {
                        edit_state(
                            state,
                            |st| {
                                if let Some(w) = st.daemon.workspaces.get_mut(viewing) {
                                    w.sections.set(disposition, want);
                                }
                            },
                        );
                    },
                }
            }

            if sections.hidden_count() > 0 {
                div { class: "rg-field",
                    span { class: "rg-field__hint",
                        // Hidden bands are a footgun: the rows still exist, are
                        // still counted in every rollup, and are simply not on
                        // screen. Four unlabelled switches do not say how many
                        // you have turned off, and "where did that session go"
                        // is the question this line exists to answer.
                        if sections.hidden_count() == 1 {
                            "1 band is hidden in this workspace. Its sessions still exist and still count; they are just not drawn."
                        } else {
                            "{sections.hidden_count()} bands are hidden in this workspace. Their sessions still exist and still count; they are just not drawn."
                        }
                    }
                }
            }

            if grouping == Grouping::Named {
                div { class: "rg-field",
                    span { class: "rg-field__label", "Folders" }
                    if folders.is_empty() {
                        span { class: "rg-field__hint",
                            "No folders yet. Every session shows under Unfiled until you make one."
                        }
                    }
                }

                for (index , (fid , fname)) in folders.iter().cloned().enumerate() {
                    div { class: "rg-field rg-field--ws", key: "{fid.0}",
                        input {
                            class: "rg-field__input rg-field__input--prose",
                            r#type: "text",
                            value: "{fname}",
                            spellcheck: false,
                            autocomplete: "off",
                            aria_label: "Folder name",
                            onchange: move |e| {
                                let text = e.value();
                                try_edit(
                                    state,
                                    error,
                                    |st| st.daemon.workspaces.rename_folder(viewing, fid, &text),
                                );
                            },
                        }
                        span { class: "rg-field__control",
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                disabled: index == 0,
                                aria_label: "Move folder up",
                                onclick: move |_| {
                                    try_edit(
                                        state,
                                        error,
                                        |st| {
                                            st.daemon
                                                .workspaces
                                                .move_folder(viewing, fid, index.saturating_sub(1))
                                        },
                                    );
                                },
                                "\u{2191}"
                            }
                            button {
                                class: "rg-btn",
                                r#type: "button",
                                disabled: index + 1 >= folders.len(),
                                aria_label: "Move folder down",
                                onclick: move |_| {
                                    try_edit(
                                        state,
                                        error,
                                        |st| st.daemon.workspaces.move_folder(viewing, fid, index + 1),
                                    );
                                },
                                "\u{2193}"
                            }
                            button {
                                class: "rg-btn rg-btn--danger",
                                r#type: "button",
                                onclick: move |_| {
                                    try_edit(
                                        state,
                                        error,
                                        |st| st.daemon.workspaces.delete_folder(viewing, fid),
                                    );
                                },
                                "Delete"
                            }
                        }
                    }
                }

                div { class: "rg-field",
                    span { class: "rg-field__control",
                        input {
                            class: "rg-field__input rg-field__input--prose",
                            r#type: "text",
                            placeholder: "New folder",
                            value: "{new_folder}",
                            spellcheck: false,
                            autocomplete: "off",
                            aria_label: "New folder name",
                            onchange: move |e| new_folder.set(e.value()),
                        }
                        button {
                            class: "rg-btn",
                            r#type: "button",
                            onclick: move |_| {
                                let name = new_folder.peek().clone();
                                try_edit(
                                    state,
                                    error,
                                    |st| st.daemon.workspaces.create_folder(viewing, &name),
                                );
                                if error.peek().is_empty() {
                                    new_folder.set(String::new());
                                }
                            },
                            "Add folder"
                        }
                    }
                }
            }
        }
    }
}
