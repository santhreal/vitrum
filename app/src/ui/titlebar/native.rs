//! The titlebar as native widgets.
//!
//! Every string on this bar is decided by [`super::context`] and
//! [`super::conn`], which are the same two functions the old markup called and
//! which know nothing about a toolkit. This file is the widgets and the
//! signals and nothing else: a rule about what the bar SAYS belongs beside
//! those functions, where it is testable without a display.
//!
//! # Why the drag is handed to the window manager
//!
//! `begin_move_drag` gives the gesture to the window manager, which is what
//! keeps tiling, edge snapping and the shake gesture working. Moving the
//! window from pointer deltas reimplements a worse version of all three and
//! fights the compositor while it does it.
//!
//! # Why the labels are rewritten and the widgets are not
//!
//! The bar hears about every state change this window sees. Rebuilding the
//! widget tree on each one would drop a pointer grab mid-drag and restart any
//! focus the operator had. The widgets are built once, at mount; only text,
//! visibility and style classes change, and only when the value differs from
//! the one already on screen.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use super::{
    Conn, Context, DRAWS_WINDOW_CONTROLS, MACOS_TRAFFIC_LIGHT_INSET, WORDMARK, conn, context,
};
use crate::Tick;
use crate::shell::dispatch::Observer;
use crate::shell::{Panel, Shell};
use crate::state::{Layer, SettingsTab, UiState};
use crate::wire::ClientEvent;

/// What the bar last said, so it is not said again.
///
/// A write into a `GtkLabel` marks the widget for redraw whether or not the
/// text differs, and this bar is told about every state change in the window.
#[derive(Default, PartialEq, Eq)]
struct Said {
    context: Context,
    conn_class: &'static str,
    conn_word: &'static str,
    conn_title: String,
    retryable: bool,
    workspace: String,
    workspace_count: String,
    open: bool,
    names_workspace: bool,
    update: Option<String>,
}

/// The widgets whose content changes after mount.
struct Live {
    primary: gtk::Label,
    secondary: gtk::Label,
    workspace: gtk::Label,
    workspace_count: gtk::Label,
    chevron: gtk::Label,
    link: gtk::Box,
    word: gtk::Label,
    retry: gtk::Button,
    update: gtk::Box,
    update_open: gtk::Button,
}

/// The window's titlebar.
pub(crate) struct TitleBarPanel {
    root: gtk::EventBox,
    live: Live,
    said: RefCell<Said>,
    shell: Shell,
}

/// Build the titlebar for `shell`.
pub(crate) fn panel(shell: &Shell) -> Rc<dyn Panel> {
    // An event box around the bar, because a `GtkBox` has no window of its
    // own and would never see the press that starts a window drag.
    let root = gtk::EventBox::new();
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.style_context().add_class("rg-titlebar");
    root.add(&bar);
    // On macOS the native traffic lights stay where macOS puts them and this
    // bar reserves their strip. Reimplementing them is how you end up with
    // three buttons that look almost right and ignore Mission Control.
    if !DRAWS_WINDOW_CONTROLS {
        bar.set_margin_start(MACOS_TRAFFIC_LIGHT_INSET as i32);
    }

    let brand = gtk::Label::new(Some(WORDMARK));
    brand.style_context().add_class("rg-titlebar__brand");
    bar.pack_start(&brand, false, false, 0);

    let switcher = gtk::Button::new();
    switcher.style_context().add_class("rg-wsw");
    switcher.set_relief(gtk::ReliefStyle::None);
    let switcher_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let chevron = gtk::Label::new(None);
    chevron.style_context().add_class("rg-wsw__chevron");
    switcher_row.pack_start(&chevron, false, false, 0);
    let workspace = gtk::Label::new(None);
    workspace.style_context().add_class("rg-wsw__name");
    workspace.set_no_show_all(true);
    switcher_row.pack_start(&workspace, false, false, 0);
    let workspace_count = gtk::Label::new(None);
    workspace_count.style_context().add_class("rg-wsw__count");
    workspace_count.set_no_show_all(true);
    switcher_row.pack_start(&workspace_count, false, false, 0);
    switcher.add(&switcher_row);
    bar.pack_start(&switcher, false, false, 0);

    let ctx = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    ctx.style_context().add_class("rg-titlebar__context");
    let primary = gtk::Label::new(None);
    primary.style_context().add_class("rg-titlebar__primary");
    primary.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    primary.set_no_show_all(true);
    ctx.pack_start(&primary, false, false, 0);
    let secondary = gtk::Label::new(None);
    secondary.style_context().add_class("rg-titlebar__secondary");
    secondary.set_ellipsize(gtk::pango::EllipsizeMode::End);
    ctx.pack_start(&secondary, false, false, 0);
    // THE TITLE IS THE BAR'S CENTRE WIDGET, not a child that grew.
    //
    // Packed to expand it took the slack and its labels sat at the left of
    // it, so the window's title started against the sidebar's right edge with
    // thirteen hundred pixels of empty bar after it. A centre widget is
    // measured against the bar rather than against whatever is beside it, so
    // the title is in the middle of the window and stays there when the
    // actions on the right change width.
    bar.set_center_widget(Some(&ctx));

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    actions.style_context().add_class("rg-titlebar__actions");
    // The actions are their stated height, centred in the bar. Left to fill,
    // a pill drawn at the bar's full height loses its rounded ends to the
    // bar's edges.
    actions.set_valign(gtk::Align::Center);

    // A quiet chip, not a modal. About owns Install; this only says a newer
    // release exists. It sits before the connection mark so the corner
    // holding the window controls is not a moving target.
    let update = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    update.style_context().add_class("rg-update");
    update.set_no_show_all(true);
    let update_open = gtk::Button::new();
    update_open.style_context().add_class("rg-update__open");
    update_open.set_relief(gtk::ReliefStyle::None);
    update.pack_start(&update_open, false, false, 0);
    let update_dismiss = gtk::Button::with_label("\u{00d7}");
    update_dismiss.style_context().add_class("rg-update__dismiss");
    update_dismiss.set_relief(gtk::ReliefStyle::None);
    update_dismiss.set_tooltip_text(Some("Dismiss until a later release"));
    update.pack_start(&update_dismiss, false, false, 0);
    actions.pack_start(&update, false, false, 0);

    let link = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    link.style_context().add_class("rg-conn");
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.style_context().add_class("rg-conn__dot");
    // A dot is round only at its own size; filling the pill makes it a bar.
    dot.set_valign(gtk::Align::Center);
    link.pack_start(&dot, false, false, 0);
    let word = gtk::Label::new(None);
    word.style_context().add_class("rg-conn__word");
    word.set_no_show_all(true);
    link.pack_start(&word, false, false, 0);
    let retry = gtk::Button::with_label("Retry");
    retry.style_context().add_class("rg-btn-inline");
    retry.set_relief(gtk::ReliefStyle::None);
    retry.set_no_show_all(true);
    link.pack_start(&retry, false, false, 0);
    actions.pack_start(&link, false, false, 0);

    let shortcuts = gtk::Button::with_label("?");
    shortcuts.style_context().add_class("rg-titlebar__action");
    shortcuts.set_relief(gtk::ReliefStyle::None);
    shortcuts.set_tooltip_text(Some("Keyboard shortcuts (F1)"));
    actions.pack_start(&shortcuts, false, false, 0);

    if DRAWS_WINDOW_CONTROLS {
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        controls.style_context().add_class("rg-window-controls");
        for (glyph, tip) in [
            ("\u{2500}", "Minimise"),
            ("\u{25a1}", "Maximise"),
            ("\u{00d7}", "Close"),
        ] {
            let button = gtk::Button::with_label(glyph);
            let style = button.style_context();
            style.add_class("rg-window-control");
            if tip == "Close" {
                style.add_class("rg-window-control--close");
            }
            button.set_relief(gtk::ReliefStyle::None);
            button.set_tooltip_text(Some(tip));
            controls.pack_start(&button, false, false, 0);
            let window = shell.window();
            match tip {
                "Minimise" => button.connect_clicked(move |_| window.iconify()),
                "Close" => button.connect_clicked(move |_| window.close()),
                _ => button.connect_clicked(move |_| toggle_maximize(&window)),
            };
        }
        // From the edge inwards: `pack_end` fills right to left, so the
        // window controls are packed before the actions to end up outside
        // them.
        bar.pack_end(&controls, false, false, 0);
    }
    bar.pack_end(&actions, false, false, 0);

    {
        let window = shell.window();
        root.connect_button_press_event(move |_, ev| {
            // Primary only. A secondary press here is the window manager's own
            // menu on most desktops, and claiming it would remove it.
            if ev.button() != 1 {
                return glib::Propagation::Proceed;
            }
            if ev.event_type() == gtk::gdk::EventType::DoubleButtonPress {
                toggle_maximize(&window);
            } else {
                let (x, y) = ev.root();
                window.begin_move_drag(1, x as i32, y as i32, ev.time());
            }
            glib::Propagation::Proceed
        });
    }
    {
        let shell = shell.clone();
        switcher.connect_clicked(move |_| {
            shell.update(|st| {
                st.window.workspace_bar_open = !st.window.workspace_bar_open;
            });
            shell.peek(crate::ui::settings::commit);
        });
    }
    {
        let shell = shell.clone();
        shortcuts.connect_clicked(move |_| toggle(&shell, Layer::Shortcuts));
    }
    {
        let shell = shell.clone();
        retry.connect_clicked(move |_| shell.send(ClientEvent::Retry));
    }
    {
        let shell = shell.clone();
        update_open.connect_clicked(move |_| {
            shell.update(|st| st.window.layer = Layer::Settings(SettingsTab::About));
        });
    }
    {
        let shell = shell.clone();
        update_dismiss.connect_clicked(move |_| {
            let Some(offer) = shell.update_offer() else {
                return;
            };
            let version = offer.version.clone();
            shell.update(move |st| st.daemon.settings.ignore_update(&version));
            shell.peek(crate::ui::settings::commit);
            shell.set_update_offer(None);
        });
    }

    root.show_all();
    Rc::new(TitleBarPanel {
        root,
        live: Live {
            primary,
            secondary,
            workspace,
            workspace_count,
            chevron,
            link,
            word,
            retry,
            update,
            update_open,
        },
        said: RefCell::new(Said::default()),
        shell: shell.clone(),
    })
}

/// Maximise, or come back out of it.
fn toggle_maximize(window: &gtk::Window) {
    if window.is_maximized() {
        window.unmaximize();
    } else {
        window.maximize();
    }
}

/// Open `layer`, or close it if it is the one already open.
fn toggle(shell: &Shell, layer: Layer) {
    shell.update(move |st| {
        st.window.layer = if st.window.layer == layer {
            Layer::None
        } else {
            layer
        };
    });
}

impl Panel for TitleBarPanel {
    fn root(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }
}

impl Observer for TitleBarPanel {
    fn state_changed(&self, state: &UiState, at: Tick) {
        let next = read(&self.shell, state, at);
        let mut said = self.said.borrow_mut();
        if *said == next {
            return;
        }

        if said.context != next.context {
            self.live.primary.set_text(&next.context.primary);
            self.live
                .primary
                .set_visible(!next.context.primary.is_empty());
            self.live.secondary.set_text(&next.context.secondary);
        }

        if said.conn_class != next.conn_class {
            let style = self.live.link.style_context();
            if !said.conn_class.is_empty() {
                style.remove_class(said.conn_class);
            }
            style.add_class(next.conn_class);
        }
        if said.conn_word != next.conn_word {
            self.live.word.set_text(next.conn_word);
            self.live.word.set_visible(!next.conn_word.is_empty());
        }
        if said.conn_title != next.conn_title {
            self.live.link.set_tooltip_text(Some(&next.conn_title));
        }
        if said.retryable != next.retryable {
            self.live.retry.set_visible(next.retryable);
        }

        if said.open != next.open {
            self.live
                .chevron
                .set_text(if next.open { "\u{25BE}" } else { "\u{25B8}" });
        }
        // The name and the count only when there is a choice to name. One
        // workspace is not one the operator picked, and printing its name
        // beside the product name states a fact nobody chose and nobody can
        // act on.
        if said.workspace != next.workspace || said.names_workspace != next.names_workspace {
            self.live.workspace.set_text(&next.workspace);
            self.live.workspace.set_visible(next.names_workspace);
        }
        if said.workspace_count != next.workspace_count
            || said.names_workspace != next.names_workspace
        {
            self.live.workspace_count.set_text(&next.workspace_count);
            self.live
                .workspace_count
                .set_visible(next.names_workspace && !next.workspace_count.is_empty());
        }
        if said.update != next.update {
            match &next.update {
                Some(version) => {
                    self.live
                        .update_open
                        .set_label(&format!("Update {version}"));
                    self.live.update_open.set_tooltip_text(Some(&format!(
                        "Open Settings, About, to install vitrum {version}"
                    )));
                    self.live.update.show();
                    self.live.update_open.show();
                }
                None => self.live.update.hide(),
            }
        }

        *said = next;
    }
}

/// Everything the bar says, read out of the state in one pass.
fn read(shell: &Shell, state: &UiState, at: Tick) -> Said {
    let link: Conn = conn(&state.daemon.conn, &shell.ident().server);
    let id = state.window.workspace;
    let name = state
        .daemon
        .workspaces
        .iter()
        .find(|w| w.id == id)
        .map_or_else(|| "Workspace".to_string(), |w| w.display_name().to_string());
    let total = state.daemon.workspaces.len();
    // The workspace id wearing a `ProjectId` hat. The fold does no filtering
    // of its own and nothing downstream reads the label; see
    // `ui::workspaces::chips`, which passes it the same way.
    let rollup = vitrum_model::rollup::rollup_rows(
        vitrum_proto::ProjectId(id.0),
        state.daemon.workspace_rows(id),
        at.model,
        state.daemon.settings.policy,
    );
    Said {
        context: context(state),
        conn_class: link.class,
        conn_word: link.word,
        conn_title: link.title,
        retryable: link.retryable,
        workspace: name,
        workspace_count: crate::ui::workspaces::badge(&rollup)
            .map(|m| m.count.to_string())
            .unwrap_or_default(),
        open: state.window.workspace_bar_open,
        names_workspace: crate::ui::workspaces::names_the_workspace(total),
        update: shell.update_offer().map(|o| o.version.to_string()),
    }
}
