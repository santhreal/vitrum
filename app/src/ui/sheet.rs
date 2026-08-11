//! How a transient surface is sized, and the type every one of them is
//! presented as.
//!
//! # The defect this file answers
//!
//! An approval option list cut off at the bottom of the window is the worst
//! case of a surface that asked for more room than the window had: the rows
//! that fell off the edge are exactly the ones the operator has to answer, and
//! nothing on screen says they exist.
//!
//! A toolkit has two ways to resolve "the content wants more than there is".
//! It can clip, or it can scroll. Clipping is the defect. So the root of every
//! transient surface in this product is a [`gtk::ScrolledWindow`] whose
//! MINIMUM size is a scrollbar and whose NATURAL size is the content capped by
//! [`Bounds`]. A widget whose minimum fits any window can always be allocated
//! inside that window, and the overflow becomes a scroll position rather than
//! a row nobody can reach.
//!
//! # Why the rule is also written as a function
//!
//! [`allocated`] is `gtk_widget_adjust_size_allocation` for a centred child,
//! followed by `gtk_box_size_allocate` distributing to one non-expanding
//! child. It computes no position, and nothing at runtime consults it: the
//! widget tree is what makes it true. It exists because `gtk_init` needs a
//! display, every test in this program runs without one, and "the surface fits
//! the window" would otherwise be a claim nobody can check.
//!
//! # Why the caps are in rem
//!
//! The type and spacing scales are authored against a 16px root and multiplied
//! by the operator's text scale, so a cap in raw pixels would be a box that
//! stopped matching its own contents the moment the scale moved.
//! [`crate::shell::style::rem`] is the one conversion.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::shell::{Dialog, Shell};

/// The widest and tallest a surface may ask to be, in rem.
///
/// A cap, not a size. A surface shorter than its cap is drawn at its own
/// height; one taller scrolls inside itself. Two numbers rather than a single
/// size class because the two axes fail differently: an over-wide sheet is
/// unreadable prose, an over-tall one is a sliced option list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bounds {
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// A surface that is one column of controls: a rename field, a confirmation.
pub(crate) const NARROW: Bounds = Bounds {
    width: 26.0,
    height: 30.0,
};

/// A surface that is a list the operator picks from.
pub(crate) const LIST: Bounds = Bounds {
    width: 40.0,
    height: 34.0,
};

/// A surface that is a document: reference tables, release notes, transcript
/// quotes. Wider because the content is two columns of text, and taller
/// because there is no useful place to break it.
pub(crate) const DOCUMENT: Bounds = Bounds {
    width: 52.0,
    height: 40.0,
};

/// Every transient surface this module presents, and the box it may not
/// exceed.
///
/// One table rather than a constant beside each surface, so that "does every
/// surface fit" is one question with one answer. A surface missing from here
/// is a surface [`tests::every_layer_names_a_registered_surface`] refuses.
#[cfg(test)]
pub(crate) const SURFACES: &[(&str, Bounds)] = &[
    (SHORTCUTS, DOCUMENT),
    (SEARCH, DOCUMENT),
    (LAUNCHER, LIST),
    (RENAME, NARROW),
    (MENU, LIST),
    (ONBOARDING, DOCUMENT),
    (WHATSNEW, DOCUMENT),
    (UPDATE, NARROW),
];

/// [`Dialog::id`] for the keyboard reference.
pub(crate) const SHORTCUTS: &str = "shortcuts";
/// [`Dialog::id`] for cross-session scrollback search.
pub(crate) const SEARCH: &str = "search";
/// [`Dialog::id`] for the launcher.
pub(crate) const LAUNCHER: &str = "launcher";
/// [`Dialog::id`] for the rename field.
pub(crate) const RENAME: &str = "rename";
/// [`Dialog::id`] for a context menu.
pub(crate) const MENU: &str = "menu";
/// [`Dialog::id`] for the first-run sheet.
pub(crate) const ONBOARDING: &str = "onboarding";
/// [`Dialog::id`] for the post-update release notes.
pub(crate) const WHATSNEW: &str = "whatsnew";
/// [`Dialog::id`] for the restart-to-update prompt.
#[cfg(test)]
pub(crate) const UPDATE: &str = "update";

/// The smallest allocation a window manager can hand a client.
///
/// Not a hypothetical. A workspace switch, an unmap and a compositor restart
/// all hand a client a one-pixel allocation for one frame before the real one
/// arrives, and a surface that clips rather than scrolls is permanently
/// truncated by the frame it was built in.
#[cfg(test)]
pub(crate) const SMALLEST: (i32, i32) = (1, 1);

/// What one axis of a centred surface is actually allocated.
///
/// `GTK_ALIGN_CENTER` shrinks a child to its natural size when there is room
/// and leaves it at the allocation when there is not, and a box distributing
/// to one non-expanding child raises it from its minimum toward its natural
/// size with whatever space is left. Both reduce to the same statement, which
/// is the whole reason the root is a scrolled window: what lands on screen is
/// never larger than the window.
#[cfg(test)]
pub(crate) fn allocated(window: i32, natural: i32) -> i32 {
    natural.min(window.max(0))
}

/// Does this surface have to scroll to be reachable at this window size?
///
/// Asked so a test can prove the content is still reachable rather than merely
/// prove the box is small. A surface that fits and one that has been silently
/// truncated are both "no larger than the window".
#[cfg(test)]
pub(crate) fn scrolls(window: i32, natural: i32) -> bool {
    natural > allocated(window, natural)
}

/// The cap in device pixels at the operator's current text scale.
fn cap(rem: f64) -> i32 {
    crate::shell::style::rem(rem).round() as i32
}

/// The natural size a surface presents to the toolkit, given how much room
/// its content wants.
///
/// `content` is in rem, so a caller states its content in the same units the
/// cap is written in and never in pixels. The cap is a maximum and not a
/// size: a two-line confirmation is a two-line box.
#[cfg(test)]
pub(crate) fn natural(bounds: Bounds, content: (f64, f64)) -> (i32, i32) {
    (
        cap(bounds.width.min(content.0)),
        cap(bounds.height.min(content.1)),
    )
}

/// One presented surface: an id, a widget, and what to do when it goes away.
///
/// Every surface in this module is one of these. Sharing the type is what
/// makes the fit rule structural rather than something each surface has to
/// remember: there is one place a transient root is built, and it builds a
/// scrolled window every time.
pub(crate) struct Sheet {
    root: gtk::ScrolledWindow,
    on_dismiss: RefCell<Option<Box<dyn Fn()>>>,
}

impl Sheet {
    /// Wrap `body` in a root that fits any window.
    ///
    /// `body` is packed whole, head and controls included, so a window too
    /// short to show the title still scrolls to it. Scrolling the body while
    /// pinning the head would be the nicer sheet and the worse guarantee: the
    /// head would then have a minimum height the window has to find first.
    pub(crate) fn new(
        id: &'static str,
        bounds: Bounds,
        body: &impl IsA<gtk::Widget>,
    ) -> Rc<Self> {
        let root = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        root.style_context().add_class("rg-sheet");
        // The id is the widget's name, which is what a GTK inspector and a
        // name selector in the sheet both look for.
        root.set_widget_name(id);
        // Automatic on both axes. A scrollbar that is always present steals a
        // column from a surface that does not need one, and one that is never
        // present is the clipping this file exists to remove.
        root.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        // The natural size is the content's, up to the cap. Without this the
        // scrolled window's natural size is its minimum and every sheet opens
        // as a scrollbar with a sliver of content in it.
        root.set_propagate_natural_width(true);
        root.set_propagate_natural_height(true);
        root.set_max_content_width(cap(bounds.width));
        root.set_max_content_height(cap(bounds.height));
        // Deliberately no `min_content_*`. A minimum content size is a floor
        // the window has to satisfy before anything can be laid out, which is
        // exactly the property that turns a small window into a clipped
        // surface.
        root.add(body);
        Rc::new(Self {
            root,
            on_dismiss: RefCell::new(None),
        })
    }

    /// Call `f` when this surface is taken down, however it was taken down.
    ///
    /// One hook rather than one per route out. The scrim, the close control
    /// and a chord all end the same way, and a surface that has to remember
    /// its own teardown three times forgets it once.
    pub(crate) fn on_dismiss(&self, f: impl Fn() + 'static) {
        *self.on_dismiss.borrow_mut() = Some(Box::new(f));
    }
}

impl Dialog for Sheet {

    fn root(&self) -> gtk::Widget {
        self.root.clone().upcast()
    }

    fn dismissed(&self) {
        if let Some(f) = self.on_dismiss.borrow().as_ref() {
            f();
        }
    }
}

/// A vertical column of controls, with `class` on it.
pub(crate) fn column(class: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    column.style_context().add_class(class);
    column
}

/// A horizontal strip of controls, with `class` on it.
pub(crate) fn row(class: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.style_context().add_class(class);
    row
}

/// A label carrying `class`, wrapping rather than widening.
///
/// Wrapping is not decoration. A label that does not wrap has a natural width
/// equal to its longest line, and a sentence quoted from a machine has no
/// bound on that, so one long path would push a sheet's natural width past its
/// cap and put a horizontal scrollbar under every surface that quotes one.
pub(crate) fn label(class: &str, text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.style_context().add_class(class);
    label.set_xalign(0.0);
    label.set_line_wrap(true);
    label.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    label
}

/// The head of a sheet: what it is, and the way out of it.
///
/// The close control is here rather than left to the scrim because a scrim
/// dismiss is discoverable only by trying it, and a surface whose only exit is
/// a chord is a surface somebody is stuck in.
pub(crate) fn head(shell: &Shell, title: &str) -> gtk::Box {
    let head = row("rg-sheet__head");
    let name = label("rg-sheet__title", title);
    name.set_hexpand(true);
    head.pack_start(&name, true, true, 0);

    let close = gtk::Button::with_label("Close");
    close.style_context().add_class("rg-btn-inline");
    let shell = shell.clone();
    close.connect_clicked(move |_| shell.dismiss());
    head.pack_end(&close, false, false, 0);
    head
}

/// Make `spec` the entire class list on `context`.
///
/// Several of the pure functions in this module's neighbours return a whole
/// class string with its modifier in it, because that string is what a test
/// asserts on and splitting it at the call site would put the modifier
/// spelling back in the markup. GTK adds one class at a time, so the list is
/// replaced wholesale: the widgets this is used on carry no class that did not
/// come from a spec.
pub(crate) fn set_classes(context: &gtk::StyleContext, spec: &str) {
    for existing in context.list_classes() {
        context.remove_class(&existing);
    }
    for class in spec.split_whitespace() {
        context.add_class(class);
    }
}

/// Assert that a surface wanting `content` rem is never sliced by the window.
///
/// The one assertion behind every surface's own fit test. Written once because
/// the property is one property: a surface is allocated no more than the
/// window, and whatever did not fit is reachable by scrolling rather than
/// gone. A per-surface copy of this would be five chances to assert only the
/// first half, which is the half that also passes on the defect.
#[cfg(test)]
pub(crate) fn assert_fits(id: &str, bounds: Bounds, content: (f64, f64)) {
    let natural = natural(bounds, content);
    let windows = [
        (0, 0),
        SMALLEST,
        (natural.0 - 1, natural.1 - 1),
        (natural.0 / 2, natural.1 / 2),
    ];
    for (w, h) in windows {
        assert!(
            allocated(w, natural.0) <= w.max(0) && allocated(h, natural.1) <= h.max(0),
            "{id} leaves a {w}x{h} window: allocated {}x{} for a natural {}x{}",
            allocated(w, natural.0),
            allocated(h, natural.1),
            natural.0,
            natural.1
        );
        assert!(
            scrolls(w, natural.0) || scrolls(h, natural.1),
            "{id} is smaller than a {w}x{h} window on both axes, so this proves nothing"
        );
    }
}

#[cfg(test)]
mod tests;
