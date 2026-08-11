//! The native window and the contracts every panel in it is built against.
//!
//! # Why the toolkit allocates and nothing else does
//!
//! The window is a GTK widget tree. The sidebar and the terminal are siblings
//! under a [`gtk::Paned`], so the toolkit decides how wide each one is and
//! hands the pane its rectangle through `size-allocate`. Nothing computes that
//! rectangle, nothing pushes it, and no repaint of any other surface can move
//! it. The terminal moving under the operator while something else rendered
//! was a direct consequence of the pane being positioned by a value derived
//! from the window size and the sidebar width, and a sibling widget cannot
//! have that failure mode.
//!
//! # The four contracts
//!
//! Everything a panel needs is here and nothing else is public.
//!
//! - **Mounting.** A panel is a [`Panel`]: one widget, and an [`Observer`].
//!   [`Shell::mount`] puts it in a [`Slot`].
//! - **State.** [`Observer::state_changed`] is called with the whole
//!   [`UiState`] and one reading of the clock, after every mutation, and once
//!   at mount so a panel never has to paint itself twice to become correct.
//! - **Actions.** A panel raises what it wants through [`Shell::update`] for
//!   anything that changes client state and [`Shell::send`] for anything the
//!   daemon has to hear. There is no action enum: an enum would have to name
//!   every panel's vocabulary, which puts every panel back in one file.
//! - **Dialogs.** [`Shell::present`] puts a [`Dialog`] over the frame in an
//!   overlay, with a scrim that dismisses it. A dialog is never a child of
//!   the pane's allocation, so presenting one cannot resize the terminal.
//!
//! # Writing a panel
//!
//! ```ignore
//! struct Sidebar { root: gtk::Box, shell: Shell }
//!
//! impl Observer for Sidebar {
//!     fn state_changed(&self, state: &UiState, at: Tick) {
//!         // repaint from `state`, using `at` for every relative time
//!     }
//! }
//!
//! impl Panel for Sidebar {
//!     fn root(&self) -> gtk::Widget { self.root.clone().upcast() }
//! }
//!
//! // in a row's click handler:
//! let shell = self.shell.clone();
//! shell.update(move |st| st.open(id, now_ms));
//! ```
//!
//! Two traits and not one because a panel owns a widget and a widget cannot be
//! built without a display, while every rule about ordering and reentrancy has
//! to be checkable without one. [`Observer`] carries the rules;
//! [`crate::shell::dispatch`] tests them.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gtk::prelude::*;
use tokio::sync::mpsc::UnboundedSender;

use crate::state::UiState;
use crate::state::live::{self, ShellSettings};
use crate::wire::ClientEvent;

pub(crate) mod dispatch;
pub(crate) mod frame;
/// Bringing a window up, and everything it owns while it is open.
pub(crate) mod run;
/// The GTK stylesheet, and what reloads it when the operator changes a
/// setting.
pub(crate) mod style;
pub(crate) mod window;

pub(crate) use dispatch::Observer;

/// Where in the frame a panel is mounted.
///
/// Three slots, because the frame has three regions that are not the pane.
/// The pane is not a slot: it is installed by [`crate::pane`] into the
/// container [`Shell::pane_host`] hands out, and no panel may be mounted
/// there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// The strip across the top of the window, above everything.
    Titlebar,
    /// The paned's first half. The operator drags its width.
    Sidebar,
    /// The one-line strip under the terminal.
    ///
    /// Its height must not depend on what it says. A bar that grows when a
    /// string arrives resizes the pane, which resizes the pty, which makes
    /// every agent on screen repaint.
    PaneBar,
}

/// One mounted region of the window.
///
/// A panel owns a widget and is told when the world changed. It is not told
/// what changed: the diff is the panel's own business, because only the panel
/// knows which of its labels the change could reach.
pub(crate) trait Panel: Observer {
    /// The widget to pack into the slot. Called once, at mount.
    fn root(&self) -> gtk::Widget;
}

/// A surface presented over the frame.
///
/// Modal in the sense that matters: it takes the pointer through the scrim
/// and it is dismissed as a unit. It is not a separate window, because a
/// second toplevel is a second thing the window manager places, and placement
/// done anywhere but the toolkit is what this change exists to remove.
pub(crate) trait Dialog {

    /// The widget to centre over the frame.
    fn root(&self) -> gtk::Widget;

    /// The dialog was taken down, by the scrim, by a key, or by a caller.
    fn dismissed(&self) {}
}

/// What a window knows about itself that the model does not carry.
///
/// The slot it persists into, the daemon it is talking to, and the operator's
/// home directory. All three are decided once, when the window is created,
/// and every panel that quotes one reads it from here rather than resolving
/// it again per paint.
#[derive(Debug, Clone)]
pub(crate) struct Ident {
    /// This window's slot in the geometry book. Also its pane's key.
    pub(crate) ordinal: usize,
    /// The daemon URL in force, after the command line and the settings
    /// document have both had their say.
    pub(crate) server: String,
    /// The operator's home directory, so a path can be drawn with a `~`
    /// rather than a name that is theirs and nobody else's business.
    pub(crate) home: String,
}

/// One window: its widgets, its state, and the panels mounted in it.
///
/// Cheap to clone. Every clone names the same window, which is what lets a
/// panel keep one and raise an action from a callback thousands of frames
/// later.
#[derive(Clone)]
pub(crate) struct Shell {
    inner: Rc<Inner>,
}

struct Inner {
    /// The frame's widgets.
    frame: frame::Frame,
    /// This window's client state and the fan-out over it.
    dispatch: dispatch::Dispatch,
    /// What is presented over the frame, if anything.
    dialog: std::cell::RefCell<Option<Rc<dyn Dialog>>>,
    /// Where a panel's request for the daemon goes. The same queue the pane's
    /// keystrokes arrive on, so a handler never has to know which.
    events: UnboundedSender<ClientEvent>,
    /// The live settings subscription. Dropping the shell unsubscribes.
    settings: std::cell::RefCell<Option<live::Subscription>>,
    /// The release the updater found, if it found one and the operator has
    /// not dismissed it.
    ///
    /// Held here rather than in [`UiState`] because it is not client state:
    /// it is the answer to a network question this process asked once, it is
    /// never persisted, and a window that never asked has none. The titlebar
    /// reads it, the About page reads it, and the task that resolves it sets
    /// it and calls [`Shell::notify`].
    update_offer: std::cell::RefCell<Option<crate::update::Available>>,
    /// Widgets a panel has offered up for the keyboard to land on, by id.
    ///
    /// A registry rather than a search, because "focus the filter" and "focus
    /// this row" are asked by the keyboard from a place that holds no widget
    /// at all, and walking the tree looking for something that matches a name
    /// is a query this program does not have. Weak, so a row that left the
    /// list takes its entry's target with it rather than being held alive by
    /// the map.
    focusable: RefCell<HashMap<String, glib::WeakRef<gtk::Widget>>>,
    /// What this window knows about itself.
    ident: Ident,
}

thread_local! {
    /// Every shell on this thread, for the settings bus to reach.
    ///
    /// The bus wants a `Send + Sync` callback and a shell is neither: it is
    /// full of widgets that belong to the thread that made them. So the
    /// callback hops to the main loop and finds its shells here.
    static SHELLS: std::cell::RefCell<Vec<Shell>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl Shell {
    /// Build the frame inside `window` and take ownership of `state`.
    ///
    /// The widget tree exists when this returns, so the pane can be installed
    /// and the panels mounted before the window is shown. A window shown
    /// first would present an empty frame and then fill it, which is one
    /// visible reflow on every launch.
    pub(crate) fn new(
        window: &gtk::Window,
        state: UiState,
        events: UnboundedSender<ClientEvent>,
        ident: Ident,
    ) -> Self {
        let shell = Self {
            inner: Rc::new(Inner {
                frame: frame::Frame::build(window),
                dispatch: dispatch::Dispatch::new(state),
                dialog: std::cell::RefCell::new(None),
                events,
                settings: std::cell::RefCell::new(None),
                update_offer: std::cell::RefCell::new(None),
                focusable: RefCell::new(HashMap::new()),
                ident,
            }),
        };
        shell.inner.frame.on_scrim_click({
            let shell = shell.clone();
            move || shell.dismiss()
        });
        SHELLS.with(|v| v.borrow_mut().push(shell.clone()));
        shell.watch_settings();
        shell
    }

    /// The toplevel this shell is the content of.
    pub(crate) fn window(&self) -> gtk::Window {
        self.inner.frame.window.clone()
    }

    /// The release the updater found, or `None`.
    pub(crate) fn update_offer(&self) -> Option<crate::update::Available> {
        self.inner.update_offer.borrow().clone()
    }

    /// Record what the updater found and tell every panel.
    ///
    /// The borrow is released before the fan-out, because a panel is entitled
    /// to read the offer while it is being told about it.
    pub(crate) fn set_update_offer(&self, offer: Option<crate::update::Available>) {
        *self.inner.update_offer.borrow_mut() = offer;
        self.notify();
    }

    /// Which window slot this is, and the two strings its panels quote.
    ///
    /// Handed out rather than re-derived per panel: the daemon URL comes off
    /// the command line and the settings document, and the home directory is
    /// one syscall a paint must not repeat. A panel that read either itself
    /// would be a second answer to a question the window already settled.
    pub(crate) fn ident(&self) -> &Ident {
        &self.inner.ident
    }

    /// The divider between the sidebar and the terminal.
    ///
    /// Handed out so the window can restore and remember the width the
    /// operator dragged. Nothing else reads it, and in particular the pane
    /// does not: the pane learns its size from its own allocation.
    pub(crate) fn paned(&self) -> gtk::Paned {
        self.inner.frame.paned.clone()
    }

    /// The container the terminal's own widget goes in.
    ///
    /// A box inside the paned's second half, packed to expand and fill. It is
    /// handed out rather than filled here because the pane owns its surface,
    /// its input method and its frame clock, and none of that is the frame's
    /// business. What the frame guarantees is the allocation: this container
    /// is sized by the toolkit and by nothing else, and the terminal it holds
    /// is sized from it.
    pub(crate) fn pane_host(&self) -> gtk::Box {
        self.inner.frame.pane_host.clone()
    }

    /// Put `panel` in `slot`.
    ///
    /// A slot holds one panel. Mounting a second one replaces the widget,
    /// because a slot is a region of the window rather than a list.
    pub(crate) fn mount(&self, slot: Slot, panel: Rc<dyn Panel>) {
        let host = self.inner.frame.slot(slot);
        for child in host.children() {
            host.remove(&child);
        }
        let root = panel.root();
        host.pack_start(&root, true, true, 0);
        root.show_all();
        self.observe(panel as Rc<dyn Observer>);
    }

    /// Tell `observer` about the state without giving it a slot.
    ///
    /// For a surface that is not part of the frame's layout: a notice floated
    /// over the window, or a dialog that has to repaint while it is open
    /// because the answer it is showing arrives seconds after it did.
    ///
    /// Told immediately, on the same terms as a mounted panel.
    pub(crate) fn observe(&self, observer: Rc<dyn Observer>) {
        self.inner.dispatch.watch(Rc::clone(&observer));
        observer.settings_changed(&live::shell_settings());
    }

    /// Stop telling `observer` anything.
    ///
    /// A transient surface MUST do this when it goes away. An observer that
    /// is only dropped from the caller's side stays in the fan-out, so a
    /// window whose operator opened the launcher twenty times would repaint
    /// twenty dead sheets on every daemon message.
    pub(crate) fn unobserve(&self, observer: &Rc<dyn Observer>) {
        self.inner.dispatch.unwatch(observer);
    }

    /// Offer `widget` up to [`Shell::focus`] under `id`.
    ///
    /// Re-registering an id replaces the target, which is what keeps a list
    /// whose rows come and go from accumulating entries for rows that are
    /// gone.
    pub(crate) fn register_focus(&self, id: impl Into<String>, widget: &impl IsA<gtk::Widget>) {
        self.inner
            .focusable
            .borrow_mut()
            .insert(id.into(), widget.as_ref().downgrade());
    }

    /// Put keyboard focus on the widget registered under `id`.
    ///
    /// A widget that has gone since it registered cannot take focus, and its
    /// entry is dropped when that is discovered rather than kept to fail
    /// again. A key bound to a surface that is not on screen has nowhere to
    /// put focus, so nothing happening is the right answer.
    pub(crate) fn focus(&self, id: &str) {
        let held = self.inner.focusable.borrow().get(id).and_then(|w| w.upgrade());
        match held {
            Some(widget) => {
                widget.grab_focus();
            }
            None => {
                self.inner.focusable.borrow_mut().remove(id);
                tracing::debug!("nothing registered as {id:?}, so focus stays where it is");
            }
        }
    }

    /// Read the state without holding a borrow past the call.
    ///
    /// A closure rather than a guard, so a caller cannot keep the borrow
    /// alive across a call into GTK that emits a signal that mutates.
    pub(crate) fn peek<R>(&self, f: impl FnOnce(&UiState) -> R) -> R {
        self.inner.dispatch.peek(f)
    }

    /// Change the state and tell every panel.
    ///
    /// Called from inside a fan-out, the mutation is queued and applied when
    /// the fan-out ends. Callers do not have to know which case they are in.
    pub(crate) fn update(&self, f: impl FnOnce(&mut UiState) + 'static) {
        self.inner.dispatch.update(f);
    }

    /// Change the state from outside a fan-out and read the answer back.
    ///
    /// For the reducer, which runs from the event pump rather than from a
    /// panel being told about a change. See [`dispatch::Dispatch::edit`].
    pub(crate) fn edit<R>(&self, f: impl FnOnce(&mut UiState) -> R) -> R {
        self.inner.dispatch.edit(f)
    }

    /// Fold everything one event causes into the state, then repaint once.
    ///
    /// Wrapped around a whole reducer pass. See [`dispatch::Dispatch::batch`].
    pub(crate) fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        self.inner.dispatch.batch(f)
    }

    /// Tell every panel to read the state again.
    ///
    /// For a change that happened somewhere the shell cannot see, such as the
    /// reducer folding a daemon message into the same state.
    pub(crate) fn notify(&self) {
        self.inner.dispatch.notify();
    }

    /// Hand an event to the window's reducer.
    ///
    /// The same queue the pane's keystrokes arrive on. A closed queue is a
    /// window that is going away, and a dropped event is the right answer.
    pub(crate) fn send(&self, event: ClientEvent) {
        let _ = self.inner.events.send(event);
    }

    /// Present `dialog` over the frame.
    ///
    /// Replaces whatever was presented, which is what a surface opening from
    /// inside another one has to do: two scrims stack into a window nobody
    /// can dismiss.
    pub(crate) fn present(&self, dialog: Rc<dyn Dialog>) {
        self.take_presented();
        self.inner.frame.show_dialog(&dialog.root());
        *self.inner.dialog.borrow_mut() = Some(dialog);
        self.dim_pane(true);
    }

    /// Dim the pane behind a sheet, or undim it.
    ///
    /// The scrim dims every widget it covers except one. The pane draws into a
    /// native child window of the toplevel, and a translucent widget over a
    /// native window is a second window whose background paints opaque: the
    /// scrim turned the terminal into a black rectangle instead of dimming it.
    /// So the pane is told, and dims itself in its own renderer.
    ///
    /// Paired with the scrim and not with the layer, because the scrim is what
    /// dims: a popover menu has none and must not dim anything.
    fn dim_pane(&self, dimmed: bool) {
        if let Some(pane) = crate::pane::PaneHost::for_window(self.inner.ident.ordinal) {
            pane.set_dimmed(dimmed);
        }
    }

    /// Present `dialog` at a point in the frame.
    ///
    /// For the one surface that has a position, which is the context menu:
    /// the click point is where the operator asked for it and a menu that
    /// ignores it is not a context menu. GTK draws the popover inside the
    /// toplevel and clamps it there, so the point is handed over rather than
    /// turned into a rectangle by this program.
    ///
    /// An operator dismisses a menu by clicking away from it more often than
    /// by choosing anything in it, so the popover's own close is wired to the
    /// same path a caller's [`Shell::dismiss`] takes.
    pub(crate) fn present_at(&self, dialog: Rc<dyn Dialog>, x: i32, y: i32) {
        self.take_presented();
        let popover = self.inner.frame.show_popover(&dialog.root(), x, y);
        *self.inner.dialog.borrow_mut() = Some(dialog);
        let shell = self.clone();
        popover.connect_closed(move |_| shell.dismiss());
    }

    /// Attach `widget` over the frame without a scrim.
    ///
    /// For a notice, which is not modal and must not take the pointer. It is
    /// an overlay child, so it takes no layout space and cannot move the
    /// terminal: a transient sentence that resizes the pane is the loudest
    /// source of the flashing this product was reported for.
    pub(crate) fn float(&self, widget: &gtk::Widget) {
        self.inner.frame.float(widget);
    }

    /// Take down whatever is presented.
    pub(crate) fn dismiss(&self) {
        if let Some(open) = self.take_presented() {
            open.dismissed();
        }
    }

    /// Clear both presentations and hand back what was open.
    ///
    /// Both, unconditionally, because a caller does not know which kind the
    /// open surface used and a popover left up under a sheet is a menu the
    /// operator cannot reach to dismiss.
    fn take_presented(&self) -> Option<Rc<dyn Dialog>> {
        let open = self.inner.dialog.borrow_mut().take();
        self.inner.frame.clear_dialog();
        self.inner.frame.clear_popover();
        self.dim_pane(false);
        open
    }

    /// Fan a settings change out to the panels, on the main loop.
    fn watch_settings(&self) {
        let sub = live::subscribe_shell(|settings| {
            let settings: Arc<ShellSettings> = Arc::new(settings.clone());
            glib::idle_add_once(move || {
                let shells = SHELLS.with(|v| v.borrow().clone());
                for shell in shells {
                    shell.inner.dispatch.settings_changed(&settings);
                }
            });
        });
        *self.inner.settings.borrow_mut() = Some(sub);
    }

}
