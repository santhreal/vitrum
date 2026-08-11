//! The Workspaces tab.
//!
//! The one panel in the sheet that does not edit [`crate::state::Settings`].
//! It drives [`crate::state::WorkspaceSet`] directly: create, rename, delete,
//! reorder, grouping mode, band visibility and folders. That is why it is the
//! only caller of [`crate::state::WorkspaceSet::rename`] that has to report a
//! refusal: a workspace operation can be refused,
//! and "you cannot delete the workspace holding four sessions" has to reach
//! the operator as a sentence rather than as a button that does nothing.

use crate::state::{FolderId, Grouping, WorkspaceId};

use super::sheet::Host;

/// The Workspaces page, as widgets.
///
/// Rebuilt whole on every change rather than diffed. A workspace list is a
/// dozen rows operated by hand, so the cheapest correct thing is to draw it
/// again; a partial update is where a renamed row keeps the old name in a
/// label nobody remembered to touch.
pub(super) fn page(host: &Host) -> gtk::Widget {
    use gtk::prelude::*;

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

    let intro = host.field(
        "Workspaces",
        "A workspace is a separate top-level context, above projects. Every session belongs to \
         exactly one, so a new workspace starts genuinely empty. New sessions land in whichever \
         workspace you are looking at.",
        "",
    );
    root.pack_start(&intro.root, false, false, 0);

    let (rows, intake, viewing, selected, folders) = host.shell().peek(|st| {
        let viewing = st.window.workspace;
        let rows: Vec<(WorkspaceId, String, usize)> = st
            .daemon
            .workspaces
            .iter()
            .map(|w| {
                (
                    w.id,
                    w.display_name().to_string(),
                    st.daemon.workspaces.session_count(w.id),
                )
            })
            .collect();
        let selected = st
            .daemon
            .workspaces
            .get(viewing)
            .map(|w| (w.display_name().to_string(), w.grouping, w.sections));
        let folders: Vec<(FolderId, String)> = st
            .daemon
            .workspaces
            .get(viewing)
            .map(|w| w.folders().iter().map(|f| (f.id, f.name.clone())).collect())
            .unwrap_or_default();
        (
            rows,
            st.daemon.workspaces.intake(),
            viewing,
            selected,
            folders,
        )
    });
    let count = rows.len();

    for (index, (id, name, sessions)) in rows.into_iter().enumerate() {
        let field = host.field("", "", "");
        field.root.style_context().add_class("rg-field--ws");
        if id == viewing {
            field.root.style_context().add_class("rg-field--ws-active");
        }

        let entry = gtk::Entry::new();
        entry.style_context().add_class("rg-field__input");
        entry.set_text(&name);
        entry.set_hexpand(true);
        {
            // On activate and on focus-out, never on every keystroke. A field
            // that commits per character asks the workspace set to accept
            // every prefix of the name being typed, and each refusal replaces
            // the banner the last one wrote.
            let host = host.clone();
            entry.connect_activate(move |entry| {
                let text = entry.text().to_string();
                host.try_edit(move |st| st.daemon.workspaces.rename(id, &text));
            });
        }
        {
            let host = host.clone();
            entry.connect_focus_out_event(move |entry, _| {
                let text = entry.text().to_string();
                host.try_edit(move |st| st.daemon.workspaces.rename(id, &text));
                glib::Propagation::Proceed
            });
        }
        field.control.pack_start(&entry, true, true, 0);

        let switch_to = gtk::Button::with_label(if id == viewing {
            "Viewing"
        } else {
            "Switch to"
        });
        switch_to.style_context().add_class("rg-btn");
        switch_to.set_sensitive(id != viewing);
        {
            let host = host.clone();
            switch_to.connect_clicked(move |_| {
                let now = crate::tick().now_ms;
                host.try_edit(move |st| st.set_workspace(id, now));
            });
        }
        field.control.pack_start(&switch_to, false, false, 0);

        let up = gtk::Button::with_label("\u{2191}");
        up.style_context().add_class("rg-btn");
        up.set_sensitive(index > 0);
        {
            let host = host.clone();
            up.connect_clicked(move |_| {
                host.try_edit(move |st| {
                    st.daemon.workspaces.move_to(id, index.saturating_sub(1))
                });
            });
        }
        field.control.pack_start(&up, false, false, 0);

        let down = gtk::Button::with_label("\u{2193}");
        down.style_context().add_class("rg-btn");
        down.set_sensitive(index + 1 < count);
        {
            let host = host.clone();
            down.connect_clicked(move |_| {
                host.try_edit(move |st| st.daemon.workspaces.move_to(id, index + 1));
            });
        }
        field.control.pack_start(&down, false, false, 0);

        let delete = gtk::Button::with_label("Delete");
        delete.style_context().add_class("rg-btn");
        delete.style_context().add_class("rg-btn--danger");
        {
            let host = host.clone();
            delete.connect_clicked(move |_| {
                let now = crate::tick().now_ms;
                host.try_edit(move |st| st.delete_workspace(id, now));
            });
        }
        field.control.pack_start(&delete, false, false, 0);

        let mut hint = if sessions == 1 {
            "1 session".to_string()
        } else {
            format!("{sessions} sessions")
        };
        if id == intake {
            hint.push_str(" \u{b7} new sessions land here");
        }
        let hint = super::sheet::wrapped(&hint);
        hint.style_context().add_class("rg-field__hint");
        field.root.pack_start(&hint, false, false, 0);
        root.pack_start(&field.root, false, false, 0);
    }

    let create = host.field("New workspace", "", "");
    let name_entry = gtk::Entry::new();
    name_entry.style_context().add_class("rg-field__input");
    name_entry.set_placeholder_text(Some("Name"));
    name_entry.set_hexpand(true);
    name_entry.set_text(&host.draft(NEW_WORKSPACE, ""));
    {
        let host = host.clone();
        name_entry.connect_changed(move |entry| {
            host.set_draft(NEW_WORKSPACE, entry.text().to_string());
        });
    }
    create.control.pack_start(&name_entry, true, true, 0);
    let create_button = gtk::Button::with_label("Create");
    create_button.style_context().add_class("rg-btn");
    create_button.style_context().add_class("rg-btn--primary");
    {
        let host = host.clone();
        let entry = name_entry.clone();
        create_button.connect_clicked(move |_| {
            let name = entry.text().to_string();
            host.clear_draft(NEW_WORKSPACE);
            host.try_edit(move |st| st.create_workspace(&name));
        });
    }
    create.control.pack_start(&create_button, false, false, 0);
    root.pack_start(&create.root, false, false, 0);

    if let Some((name, grouping, sections)) = selected {
        let head = host.field(
            &name,
            "Grouping and band visibility belong to the workspace, not to you: \u{201c}this one \
             is my review queue, show me settled work\u{201d} is a fact about the context and \
             not about the person.",
            "",
        );
        root.pack_start(&head.root, false, false, 0);

        let group = host.field(
            "Group rows by",
            match grouping {
                Grouping::Directory => {
                    "A session under a project root the daemon knows files under that project; \
                     everything else gets a bucket per directory."
                }
                Grouping::Named => {
                    "Folders you create, in your order, plus an Unfiled bucket. Move rows \
                     between folders from the right-click menu."
                }
            },
            "",
        );
        let combo = gtk::ComboBoxText::new();
        combo.style_context().add_class("rg-select");
        combo.append(Some("directory"), Grouping::Directory.label());
        combo.append(Some("named"), Grouping::Named.label());
        combo.set_active_id(Some(match grouping {
            Grouping::Directory => "directory",
            Grouping::Named => "named",
        }));
        {
            let host = host.clone();
            combo.connect_changed(move |combo| {
                let Some(picked) = combo.active_id() else {
                    return;
                };
                host.edit_state(move |st| {
                    if let Some(w) = st.daemon.workspaces.get_mut(viewing) {
                        w.grouping = if picked == "named" {
                            Grouping::Named
                        } else {
                            Grouping::Directory
                        };
                    }
                });
            });
        }
        group.control.pack_start(&combo, false, false, 0);
        root.pack_start(&group.root, false, false, 0);

        for (disposition, label, on) in [
            (vitrum_model::Disposition::Active, "Active", sections.active),
            (vitrum_model::Disposition::Woke, "Woke", sections.woke),
            (
                vitrum_model::Disposition::Snoozed,
                "Snoozed",
                sections.snoozed,
            ),
            (
                vitrum_model::Disposition::Settled,
                "Settled",
                sections.settled,
            ),
        ] {
            let field = host.field(&format!("Show {label}"), "", "");
            let switch = gtk::Switch::new();
            switch.style_context().add_class("rg-switch");
            switch.set_active(on);
            let host = host.clone();
            switch.connect_state_set(move |_, want| {
                host.edit_state(move |st| {
                    if let Some(w) = st.daemon.workspaces.get_mut(viewing) {
                        w.sections.set(disposition, want);
                    }
                });
                glib::Propagation::Proceed
            });
            field.control.pack_start(&switch, false, false, 0);
            root.pack_start(&field.root, false, false, 0);
        }

        // Hidden bands are a footgun: the rows still exist, are still counted
        // in every rollup, and are simply not on screen. Four unlabelled
        // switches do not say how many you have turned off, and "where did
        // that session go" is the question this line exists to answer.
        let hidden = sections.hidden_count();
        if hidden > 0 {
            let field = host.field(
                "",
                "",
                &if hidden == 1 {
                    "1 band is hidden in this workspace. Its sessions still exist and still \
                     count; they are just not drawn."
                        .to_string()
                } else {
                    format!(
                        "{hidden} bands are hidden in this workspace. Their sessions still exist \
                         and still count; they are just not drawn."
                    )
                },
            );
            root.pack_start(&field.root, false, false, 0);
        }

        if grouping == Grouping::Named {
            let head = host.field(
                "Folders",
                "",
                if folders.is_empty() {
                    "No folders yet. Every session shows under Unfiled until you make one."
                } else {
                    ""
                },
            );
            root.pack_start(&head.root, false, false, 0);

            let total = folders.len();
            for (index, (fid, fname)) in folders.into_iter().enumerate() {
                let field = host.field("", "", "");
                field.root.style_context().add_class("rg-field--ws");
                let entry = gtk::Entry::new();
                entry.style_context().add_class("rg-field__input");
                entry.set_text(&fname);
                entry.set_hexpand(true);
                {
                    let host = host.clone();
                    entry.connect_activate(move |entry| {
                        let text = entry.text().to_string();
                        host.try_edit(move |st| {
                            st.daemon.workspaces.rename_folder(viewing, fid, &text)
                        });
                    });
                }
                {
                    let host = host.clone();
                    entry.connect_focus_out_event(move |entry, _| {
                        let text = entry.text().to_string();
                        host.try_edit(move |st| {
                            st.daemon.workspaces.rename_folder(viewing, fid, &text)
                        });
                        glib::Propagation::Proceed
                    });
                }
                field.control.pack_start(&entry, true, true, 0);

                let up = gtk::Button::with_label("\u{2191}");
                up.style_context().add_class("rg-btn");
                up.set_sensitive(index > 0);
                {
                    let host = host.clone();
                    up.connect_clicked(move |_| {
                        host.try_edit(move |st| {
                            st.daemon.workspaces.move_folder(
                                viewing,
                                fid,
                                index.saturating_sub(1),
                            )
                        });
                    });
                }
                field.control.pack_start(&up, false, false, 0);

                let down = gtk::Button::with_label("\u{2193}");
                down.style_context().add_class("rg-btn");
                down.set_sensitive(index + 1 < total);
                {
                    let host = host.clone();
                    down.connect_clicked(move |_| {
                        host.try_edit(move |st| {
                            st.daemon.workspaces.move_folder(viewing, fid, index + 1)
                        });
                    });
                }
                field.control.pack_start(&down, false, false, 0);

                let delete = gtk::Button::with_label("Delete");
                delete.style_context().add_class("rg-btn");
                delete.style_context().add_class("rg-btn--danger");
                {
                    let host = host.clone();
                    delete.connect_clicked(move |_| {
                        host.try_edit(move |st| st.daemon.workspaces.delete_folder(viewing, fid));
                    });
                }
                field.control.pack_start(&delete, false, false, 0);
                root.pack_start(&field.root, false, false, 0);
            }

            let add = host.field("", "", "");
            let entry = gtk::Entry::new();
            entry.style_context().add_class("rg-field__input");
            entry.set_placeholder_text(Some("New folder"));
            entry.set_hexpand(true);
            entry.set_text(&host.draft(NEW_FOLDER, ""));
            {
                let host = host.clone();
                entry.connect_changed(move |entry| {
                    host.set_draft(NEW_FOLDER, entry.text().to_string());
                });
            }
            add.control.pack_start(&entry, true, true, 0);
            let button = gtk::Button::with_label("Add folder");
            button.style_context().add_class("rg-btn");
            {
                let host = host.clone();
                let entry = entry.clone();
                button.connect_clicked(move |_| {
                    let name = entry.text().to_string();
                    host.clear_draft(NEW_FOLDER);
                    host.try_edit(move |st| st.daemon.workspaces.create_folder(viewing, &name));
                });
            }
            add.control.pack_start(&button, false, false, 0);
            root.pack_start(&add.root, false, false, 0);
        }
    }

    root.upcast()
}

/// Draft keys, so a redraw does not empty a name being typed.
const NEW_WORKSPACE: &str = "workspaces.new";
/// Draft key for the folder name field.
const NEW_FOLDER: &str = "workspaces.newFolder";
