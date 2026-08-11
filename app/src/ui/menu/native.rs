//! The context menu, as a presented surface.
//!
//! # Why this no longer computes where it goes
//!
//! The previous menu was positioned by a pure function that flipped and
//! pinned the box against the window so its last two entries, the destructive
//! ones, could not fall off the bottom edge. That arithmetic was correct and
//! it is gone, because the toolkit does the same job with no arithmetic at
//! all: [`crate::shell::Shell::present_at`] puts the menu in a
//! popover anchored at the click, and GTK flips and clamps it inside the
//! toplevel itself.
//!
//! What is kept is the guarantee, not the mechanism. The menu's root is the
//! same scrolled sheet every other transient surface uses, so a menu longer
//! than the room under the pointer scrolls rather than losing its last
//! entries.
//!
//! # Why the entries are buttons and captions are insensitive
//!
//! "Snooze until" and "Move to workspace" caption the entries beneath them and
//! are not actions. They are rendered as insensitive rows rather than dropped,
//! because a list of five wake times with no word saying what they are is five
//! unexplained times.

use std::rc::Rc;

use gtk::prelude::*;

use crate::inbox;
use crate::shell::Shell;
use crate::state::{MenuAction, MenuState};
use crate::ui::sheet::{self, Sheet};
use crate::wire::ClientEvent;

/// The menu for one right-click, or `None` when there is nothing to act on.
///
/// `None` matters: a session that vanished between the right-click and this
/// call has no entries, and presenting an empty box would put a scrim over the
/// window that swallows the next click for nothing.
pub(crate) fn build(shell: &Shell, menu: MenuState) -> Option<Rc<Sheet>> {
    let at = crate::tick();
    let (items, title) = shell.peek(|st| {
        let items = st.menu_items(menu.target, at.model);
        let targets = st.menu_targets(menu.target, at.model);
        // A bulk menu names its count rather than one row's title. A menu
        // headed with one session's name that then closes nineteen is the
        // exact mistake the counted labels exist to prevent.
        let title = if targets.len() > 1 {
            format!("{} sessions selected", targets.len())
        } else {
            st.session(menu.target)
                .map(|s| inbox::row_title(s).into_owned())
                .unwrap_or_default()
        };
        (items, title)
    });
    if items.is_empty() {
        return None;
    }

    let list = sheet::column("rg-menu");
    let head = sheet::label("rg-menu__caption", &title);
    head.set_tooltip_text(Some(&title));
    list.pack_start(&head, false, false, 0);

    for item in &items {
        if item.sep_before {
            let rule = gtk::Separator::new(gtk::Orientation::Horizontal);
            rule.style_context().add_class("rg-menu__sep");
            list.pack_start(&rule, false, false, 0);
        }
        let entry = gtk::Button::new();
        let inside = sheet::row("rg-menu__item");
        let label = sheet::label("rg-menu__label", &item.label);
        label.set_hexpand(true);
        inside.pack_start(&label, true, true, 0);
        if let Some(hint) = &item.hint {
            inside.pack_end(&sheet::label("rg-menu__hint", hint), false, false, 0);
        }
        entry.add(&inside);
        let context = entry.style_context();
        context.add_class("rg-menu__item");
        if item.danger {
            context.add_class("rg-menu__item--danger");
        }
        if item.action.is_caption() {
            context.add_class("rg-menu__item--caption");
        }
        entry.set_sensitive(item.enabled && !item.action.is_caption());

        let shell = shell.clone();
        let action = item.action;
        entry.connect_clicked(move |_| pick(&shell, action, menu));
        list.pack_start(&entry, false, false, 0);
    }

    Some(Sheet::new(sheet::MENU, sheet::LIST, &list))
}

/// Perform one entry.
///
/// Every entry acts on [`crate::state::UiState::menu_targets`], which is the
/// whole selection when the right-click landed inside one and the single row
/// otherwise.
///
/// The menu comes down first. An entry that opens another surface, which
/// Rename and New session here both do, would otherwise be dismissed by its
/// own menu closing a moment later.
pub(crate) fn pick(shell: &Shell, action: MenuAction, menu: MenuState) {
    let id = menu.target;
    let at = crate::tick();
    let targets = shell.peek(|st| st.menu_targets(id, at.model));
    shell.dismiss();
    shell.update(|st| st.window.layer = crate::state::Layer::None);

    match action {
        // Captions are rendered insensitive, so these are only reachable if
        // that ever stops being true. Doing nothing is the right answer either
        // way.
        MenuAction::SnoozeHeader
        | MenuAction::MoveToWorkspaceHeader
        | MenuAction::MoveToFolderHeader => {}
        MenuAction::Focus => shell.update(move |st| st.open(id, at.now_ms)),
        MenuAction::CloseTab => {
            shell.update(move |st| st.close_tab(id));
            shell.send(ClientEvent::Reconcile);
        }
        MenuAction::CloseOthers => {
            shell.update(move |st| st.close_other_tabs(id));
            shell.send(ClientEvent::Reconcile);
        }
        MenuAction::Snooze(preset) => snooze(shell, &targets, preset, at),
        MenuAction::Wake => shell.update(move |st| {
            st.wake(&targets, at.now_ms);
        }),
        MenuAction::Settle => shell.update(move |st| {
            let drained = st.settle(&targets, at.now_ms);
            if drained < targets.len() {
                st.window.flash = Some(crate::state::Flash::notice(format!(
                    "Settled {drained} of {}; the rest are still working or blocked on you",
                    targets.len()
                )));
            }
        }),
        MenuAction::Unsettle => shell.update(move |st| st.unsettle(&targets)),
        MenuAction::MarkRead => shell.update(move |st| st.mark_seen(&targets, at.now_ms)),
        MenuAction::MarkUnread => shell.update(move |st| st.mark_unseen(&targets)),
        MenuAction::Rename => {
            // A rename sheet for a session that vanished between the
            // right-click and the pick would send a title for an id the daemon
            // no longer has.
            let title = shell.peek(|st| st.session(id).map(|s| s.title.clone()));
            if let Some(title) = title {
                shell.update(move |st| {
                    st.window.layer = crate::state::Layer::Rename(crate::state::RenameSeed {
                        session: id,
                        title,
                    });
                });
            }
        }
        MenuAction::CopyPath => copy(shell, id, |s| s.cwd.clone()),
        MenuAction::CopyBranch => copy(shell, id, |s| s.git_branch.clone().unwrap_or_default()),
        MenuAction::CopyCommand => copy(shell, id, |s| {
            let mut line = s.command.clone();
            for arg in &s.args {
                line.push(' ');
                line.push_str(arg);
            }
            line
        }),
        MenuAction::NewSessionHere => {
            let project = shell.peek(|st| st.session(id).map(|s| s.project_id));
            let cwd = shell.peek(|st| crate::actions::seed_dir(st, project));
            shell.update(move |st| {
                st.window.layer =
                    crate::state::Layer::NewSession(crate::state::NewSessionSeed { project, cwd });
            });
        }
        MenuAction::Duplicate => shell.send(ClientEvent::Duplicate { session: id }),
        MenuAction::MoveToWorkspace(workspace) => {
            let asked = targets.len();
            shell.update(move |st| {
                let outcome = st.move_to_workspace(&targets, workspace, at.now_ms);
                st.window.flash = Some(moved(outcome, asked, "workspace"));
                // Filing is a deliberate act on a durable arrangement, so it is
                // written now rather than at the next window event.
                save(st);
            });
        }
        MenuAction::MoveToFolder(folder) => {
            let asked = targets.len();
            shell.update(move |st| {
                let outcome = st.move_to_folder(&targets, folder);
                st.window.flash = Some(moved(outcome, asked, "folder"));
                save(st);
            });
        }
        MenuAction::Terminate => shell.send(ClientEvent::Terminate { targets }),
    }
}

/// Park the targets until the preset's wake instant.
fn snooze(
    shell: &Shell,
    targets: &[vitrum_proto::SessionId],
    preset: vitrum_model::SnoozePresetId,
    at: crate::Tick,
) {
    // The preset list is time-dependent: "this evening" disappears once
    // evening is under an hour away. A pick for a preset that is no longer
    // offered says so rather than parking the row until an instant nobody
    // chose.
    let found = shell.peek(|st| {
        st.snooze_presets(at.model)
            .into_iter()
            .find(|p| p.id == preset)
    });
    let Some(found) = found else {
        shell.update(|st| {
            st.window.flash = Some(crate::state::Flash::notice(
                "That snooze time has passed. Open the menu again for current options.",
            ));
        });
        return;
    };
    let targets = targets.to_vec();
    shell.update(move |st| {
        let parked = st.snooze(&targets, found.wake_at_ms, at.now_ms);
        let when = vitrum_model::wake_description(found.wake_at_ms, at.model);
        st.window.flash = Some(crate::state::Flash::notice(if parked == targets.len() {
            format!("Snoozed {parked} until {when}")
        } else {
            format!(
                "Snoozed {parked} of {} until {when}; the rest are blocked on you",
                targets.len()
            )
        }));
    });
}

/// Say what a move actually did.
///
/// A move of five rows that placed three is not a success, and reporting it as
/// one is how somebody discovers two sessions missing an hour later.
pub(crate) fn moved(
    outcome: Result<usize, crate::state::WorkspaceError>,
    asked: usize,
    what: &str,
) -> crate::state::Flash {
    match outcome {
        Ok(moved) if moved == asked => {
            crate::state::Flash::notice(format!("Moved {moved} to another {what}"))
        }
        Ok(moved) => crate::state::Flash::notice(format!(
            "Moved {moved} of {asked} to another {what}; the rest are already there"
        )),
        Err(why) => {
            crate::state::Flash::error(format!("Could not move to that {what}: {why}"))
        }
    }
}

/// Write the arrangement to the profile.
fn save(st: &crate::state::UiState) {
    if let Err(why) = crate::state::save_prefs(&st.daemon, &st.window) {
        tracing::warn!("window state not saved: {why}");
    }
}

/// Put one field of a session on the clipboard.
///
/// Raised rather than written here, because the answer comes back as
/// [`ClientEvent::Copied`]: a write can be refused, and a "Copied" notice for
/// a copy that did not happen is a lie discovered only on the paste.
fn copy(
    shell: &Shell,
    id: vitrum_proto::SessionId,
    field: impl Fn(&vitrum_proto::SessionInfo) -> String,
) {
    let text = shell.peek(|st| st.session(id).map(&field).unwrap_or_default());
    if text.is_empty() {
        return;
    }
    shell.send(ClientEvent::Clipboard { text });
}

/// How much room a menu of `items` entries wants, in rem.
#[cfg(test)]
pub(crate) fn content(items: usize, separators: usize) -> (f64, f64) {
    // The title row, one row per entry, and a rule's worth for each separator.
    (
        sheet::LIST.width,
        2.0 + items as f64 * 2.0 + separators as f64 * 0.5,
    )
}

#[cfg(test)]
mod tests;
