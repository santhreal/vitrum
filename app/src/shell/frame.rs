//! The widget tree one window is made of.
//!
//! # The tree, and why it is this shape
//!
//! ```text
//! GtkWindow                       the toplevel, undecorated
//! └── GtkBox (vertical)           .rg-root
//!     ├── GtkBox (horizontal)     Slot::Titlebar, unstyled
//!     └── GtkOverlay              dialogs go over this and nothing else
//!         ├── GtkPaned            .rg-paned      the drag the operator has
//!         │   ├── GtkBox          Slot::Sidebar, unstyled
//!         │   └── GtkBox          .rg-content
//!         │       ├── GtkBox      .rg-pane       the terminal's parent
//!         │       └── GtkBox      Slot::PaneBar, unstyled
//!         └── GtkEventBox         .rg-scrim      hidden until a dialog opens
//!             └── GtkBox          .rg-dialog-slot
//! ```
//!
//! The sidebar and the content are the two halves of the paned, so the
//! sidebar's width IS the paned's handle position and nothing derives it. The
//! pane's parent box is inside the content box, under the toolkit's
//! allocation, so a repaint of the titlebar, the sidebar or the bar cannot
//! change where the terminal is: none of them is its parent and none of them
//! can write its position.
//!
//! The scrim is an event box because an event box has a window of its own,
//! which is what stops a click meant for a dialog from reaching the sidebar
//! behind it. It is `no_show_all` so that showing the frame does not show it.
//!
//! # Why the dialog is an overlay child and not a second window
//!
//! A second toplevel is placed by the window manager, and placement done
//! anywhere but the toolkit is the defect this whole tree exists to remove. An
//! overlay child is allocated by GTK over the frame, so it cannot move the
//! pane and cannot be positioned wrongly by anything.

use gtk::prelude::*;

use super::Slot;

/// One window's widgets.
///
/// Every field is a handle GTK reference counts, so a clone of this struct
/// names the same widgets. It is held by the shell and handed out one widget
/// at a time.
#[derive(Clone)]
pub(crate) struct Frame {
    /// The toplevel, so the shell can title it and close it.
    pub(crate) window: gtk::Window,
    /// The strip above everything.
    titlebar: gtk::Box,
    /// The paned's first half.
    sidebar: gtk::Box,
    /// The strip under the terminal.
    panebar: gtk::Box,
    /// The terminal's parent, which is what allocates it.
    pub(crate) pane_host: gtk::Box,
    /// The divider the operator drags.
    pub(crate) paned: gtk::Paned,
    /// Where a dialog is centred.
    dialog_slot: gtk::Box,
    /// What blocks the pointer while a dialog is up.
    scrim: gtk::EventBox,
    /// The overlay a dialog, a scrim and a toast are all children of.
    ///
    /// Held so a transient surface can be attached without a scrim. A notice
    /// is not modal and must not take the pointer, which is the whole
    /// difference between it and a dialog.
    overlay: gtk::Overlay,
    /// The frame's outermost box, which a popover points into.
    root: gtk::Box,
    /// The popover in force, if a surface was presented at a point.
    ///
    /// Behind a reference count so every clone of the frame names the same
    /// one. A second popover left open under the first is a menu the operator
    /// cannot dismiss.
    popover: std::rc::Rc<std::cell::RefCell<Option<gtk::Popover>>>,
}

impl Frame {
    /// Build the tree and put it in `window`.
    ///
    /// `window` must be empty. It is, on every path this program has: the
    /// toplevel is created for this and nothing else adds to it.
    pub(crate) fn build(window: &gtk::Window) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.style_context().add_class("rg-root");

        let titlebar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.pack_start(&titlebar, false, false, 0);

        let overlay = gtk::Overlay::new();
        root.pack_start(&overlay, true, true, 0);

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.style_context().add_class("rg-paned");
        overlay.add(&paned);

        // No style class on a slot. The panel mounted into it wears the
        // class, and a slot carrying the same one means every border, inset
        // and background is drawn twice by two nested boxes.
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // `resize: false` so growing the window grows the terminal and not the
        // list. A sidebar that took half of every new pixel would turn a
        // maximise into a wall of empty column.
        paned.pack1(&sidebar, false, false);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.style_context().add_class("rg-content");
        paned.pack2(&content, true, false);

        let pane_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pane_host.style_context().add_class("rg-pane");
        // A box rather than a `GtkFixed`, because a box sizes its child from
        // its own allocation and a fixed does not. The terminal is that
        // child, packed to expand and fill, so its rectangle is decided by
        // the toolkit walking the widget tree. Nothing computes it and
        // nothing writes it, which is what stops the terminal moving when
        // something else on screen repaints.
        content.pack_start(&pane_host, true, true, 0);

        let panebar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.pack_start(&panebar, false, false, 0);

        // Input only, and deliberately.
        //
        // An event box with a window of its own is a native X window, and a
        // native window does not composite with the one beside it: the pane's
        // swapchain window is a sibling, so a translucent fill painted here
        // reached the server as an opaque rectangle and the terminal went
        // black whenever a sheet opened. Without a window the box still takes
        // the clicks that dismiss the sheet, and the dimming moves to a
        // windowless child that paints into the toplevel's own surface: over
        // the chrome that is a translucent wash, and over the pane it is
        // simply behind the swapchain and invisible. The pane dims itself,
        // through `PaneHost::set_dimmed`.
        let scrim = gtk::EventBox::new();
        scrim.set_visible_window(false);
        scrim.set_no_show_all(true);
        let wash = gtk::Box::new(gtk::Orientation::Vertical, 0);
        wash.style_context().add_class("rg-scrim");
        scrim.add(&wash);
        let dialog_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        dialog_slot.style_context().add_class("rg-dialog-slot");
        dialog_slot.set_halign(gtk::Align::Center);
        dialog_slot.set_valign(gtk::Align::Center);
        wash.pack_start(&dialog_slot, true, true, 0);
        // Shown now, while its parent is not: `show_dialog` shows the scrim
        // and the slot, and `no_show_all` on the scrim means a later
        // `show_all` never reaches this one.
        wash.show();
        overlay.add_overlay(&scrim);
        overlay.set_overlay_pass_through(&scrim, false);

        window.add(&root);

        // The frame exists on screen. This is the mark the startup claim is
        // measured against on the far side of window creation, and it is
        // taken here rather than after the first paint because a realized
        // frame is what replaced a mounted document.
        root.connect_realize(|_| crate::boot::mark("frame.realized"));

        Self {
            window: window.clone(),
            titlebar,
            sidebar,
            panebar,
            pane_host,
            paned,
            dialog_slot,
            scrim,
            overlay,
            root,
            popover: std::rc::Rc::new(std::cell::RefCell::new(None)),
        }
    }

    /// The box a panel is packed into.
    pub(crate) fn slot(&self, slot: Slot) -> gtk::Box {
        match slot {
            Slot::Titlebar => self.titlebar.clone(),
            Slot::Sidebar => self.sidebar.clone(),
            Slot::PaneBar => self.panebar.clone(),
        }
    }

    /// Present `widget` at a point in the frame, in a popover.
    ///
    /// The one surface that has a position is a context menu, and the point
    /// is the click. GTK3 draws a popover inside the toplevel and flips and
    /// clamps it against the window itself, so the surface stays on screen
    /// near the pointer without this program computing a rectangle. That is
    /// the same rule as everything else in the frame: the toolkit places it.
    ///
    /// Returns the popover so the caller can hear it close, because an
    /// operator dismisses a menu by clicking away from it far more often than
    /// by choosing anything in it.
    pub(crate) fn show_popover(&self, widget: &gtk::Widget, x: i32, y: i32) -> gtk::Popover {
        self.clear_popover();
        let popover = gtk::Popover::new(Some(&self.root));
        popover.set_pointing_to(&gtk::gdk::Rectangle::new(x, y, 1, 1));
        popover.set_position(gtk::PositionType::Bottom);
        popover.add(widget);
        widget.show_all();
        popover.popup();
        *self.popover.borrow_mut() = Some(popover.clone());
        popover
    }

    /// Take the popover down, if one is up.
    ///
    /// The slot is emptied BEFORE the popdown. Popping down emits `closed`,
    /// whose handler dismisses, which comes back here; finding the slot
    /// already empty is what ends that rather than a second borrow of a cell
    /// this call is holding.
    pub(crate) fn clear_popover(&self) {
        let open = self.popover.borrow_mut().take();
        if let Some(open) = open {
            open.popdown();
            // Destroyed rather than kept: a popover is created per menu and
            // one left parented to the frame is a widget per right-click for
            // the life of the window.
            unsafe { open.destroy() };
        }
    }

    /// Put `widget` on the scrim and show both.
    pub(crate) fn show_dialog(&self, widget: &gtk::Widget) {
        self.dialog_slot.pack_start(widget, false, false, 0);
        self.scrim.show();
        self.dialog_slot.show();
        widget.show_all();
    }

    /// Take the scrim down and empty it.
    pub(crate) fn clear_dialog(&self) {
        for child in self.dialog_slot.children() {
            self.dialog_slot.remove(&child);
        }
        self.scrim.hide();
    }

    /// Attach `widget` over the frame without a scrim.
    ///
    /// Pass-through, so a notice can never eat a click meant for the sidebar
    /// or the terminal underneath it. The caller owns the alignment and the
    /// style class; what the frame owns is that the widget is an overlay
    /// child and therefore takes no layout space from the pane.
    pub(crate) fn float(&self, widget: &gtk::Widget) {
        self.overlay.add_overlay(widget);
        self.overlay.set_overlay_pass_through(widget, true);
        widget.show_all();
    }

    /// Call `f` when the operator clicks the scrim outside the dialog.
    ///
    /// Outside, by allocation. A click on the dialog itself that no control
    /// claimed propagates to the event box, and dismissing on that would make
    /// a sheet close when the operator pressed one of its own labels.
    pub(crate) fn on_scrim_click(&self, f: impl Fn() + 'static) {
        let slot = self.dialog_slot.clone();
        self.scrim.connect_button_press_event(move |_, ev| {
            let (x, y) = ev.position();
            let a = slot.allocation();
            let inside = x >= f64::from(a.x())
                && x < f64::from(a.x() + a.width())
                && y >= f64::from(a.y())
                && y < f64::from(a.y() + a.height());
            if inside {
                return glib::Propagation::Proceed;
            }
            f();
            glib::Propagation::Stop
        });
    }
}
