//! The sidebar, as GTK widgets.
//!
//! # Why this file is an interpreter and not a renderer
//!
//! Every decision about what the sidebar contains was already made, in order,
//! by [`super::fold`]. Nothing here reads a session, a band or a setting. It
//! is handed a [`Node`] tree and it makes the widgets that tree describes, and
//! on the next paint it is handed another tree and it patches the widgets it
//! already has into the new shape. That split is what lets every ordering,
//! folding and formatting rule this panel has be asserted on a machine with no
//! display.
//!
//! # Why the tree is patched rather than rebuilt
//!
//! The daemon pushes an update per live session per second. Rebuilding the
//! panel on each of those is the flicker this rewrite exists to remove: a
//! destroyed and recreated widget loses its scroll position, its focus and its
//! hover, and a container that empties and refills paints a blank frame on the
//! way through. So a paint compares the new tree with the previous one and
//! touches only what differs. A subtree whose shape is unchanged has its
//! labels' text set and nothing else; a subtree whose shape changed is rebuilt
//! in place, with any session rows under it lifted out first so they survive.
//!
//! # Why the rows are held apart
//!
//! [`super::rows::Rows`] owns the row widgets, keyed by session, and the tree
//! only carries a [`Kind::Seat`] where each one goes. Reordering the list is
//! then a reparent of existing widgets rather than twenty rebuilds, and a row
//! whose fold did not change is not touched at all.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use vitrum_proto::SessionId;

use super::fold::{self, Context, Fold};
use super::rows::{RowView, Rows};
use super::tree::{Act, Kind, Node};
use crate::shell::{self, Shell};
use crate::state::{Layer, MenuState, SettingsTab, UiState};
use crate::ui::sheet;
use crate::wire::ClientEvent;

/// Mount the sidebar into the shell's sidebar slot.
pub(crate) fn panel(shell: &Shell) -> Rc<dyn shell::Panel> {
    let panel = SidebarPanel::new(shell);
    // The staged-build poll belongs to whoever draws the offer, and the offer
    // is the sidebar's restart line. Weak, so the timer cannot be the thing
    // keeping a closed window's panel alive.
    let weak = Rc::downgrade(&panel);
    let source = glib::timeout_add_local(crate::update::STAGED_POLL, move || {
        match weak.upgrade() {
            Some(panel) => {
                panel.poll_standing();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        }
    });
    panel.poller.set(Some(source));
    panel
}

/// The sidebar panel: one widget tree, one row store, one context.
struct SidebarPanel {
    shell: Shell,
    tree: RefCell<Rendered>,
    rows: RefCell<Rows<GtkRow>>,
    cx: RefCell<Context>,
    /// The session count the launch word was resolved against.
    ///
    /// Resolving it reads the operator's launch history off disk, and the only
    /// thing that can change the answer is a session starting or ending. Doing
    /// it on that edge rather than every paint is the difference between one
    /// file read per launch and one per daemon push.
    word_at: Cell<Option<usize>>,
    /// The staged-build poll, so it can be cancelled with the panel.
    poller: Cell<Option<glib::SourceId>>,
    /// Set while the filter field is being written to from a fold, so the
    /// change signal that write raises is not read back as the operator
    /// typing.
    settling: Rc<Cell<bool>>,
}

impl SidebarPanel {
    fn new(shell: &Shell) -> Rc<Self> {
        let cx = Context {
            home: shell.ident().home.clone(),
            server: shell.ident().server.clone(),
            standing: standing(),
            launch_word: None,
        };
        let at = crate::tick();
        let fold = shell.peek(|st| fold::panel(st, at, &cx));
        let settling = Rc::new(Cell::new(false));
        let mut rows = Rows::default();
        rows.sync(&fold.rows, shell);
        let tree = build(&fold.root, shell, &settling, &rows, &mut Hovers::default());
        Rc::new(SidebarPanel {
            shell: shell.clone(),
            tree: RefCell::new(tree),
            rows: RefCell::new(rows),
            cx: RefCell::new(cx),
            word_at: Cell::new(None),
            poller: Cell::new(None),
            settling,
        })
    }

    /// Bring the panel in line with one reading of the state.
    fn draw(&self, st: &UiState, at: crate::Tick) {
        let sessions = st.daemon.sessions.len();
        if self.word_at.get() != Some(sessions) {
            self.word_at.set(Some(sessions));
            self.cx.borrow_mut().launch_word = launch_word();
        }
        let fold: Fold = fold::panel(st, at, &self.cx.borrow());
        {
            let mut rows = self.rows.borrow_mut();
            rows.sync(&fold.rows, &self.shell);
        }
        let rows = self.rows.borrow();
        let mut tree = self.tree.borrow_mut();
        let mut hovers = Hovers::default();
        patch(
            &mut tree,
            &fold.root,
            &self.shell,
            &self.settling,
            &rows,
            &mut hovers,
        );
        tree.widget.show_all();
        // Every hover reveal starts hidden. `show_all` above would otherwise
        // put every row's detail on screen at once.
        hovers.rest();
    }

    /// Re-read the staged build, and repaint only when the answer moved.
    fn poll_standing(&self) {
        let next = standing();
        if self.cx.borrow().standing == next {
            return;
        }
        self.cx.borrow_mut().standing = next;
        self.shell.notify();
    }
}

impl Drop for SidebarPanel {
    fn drop(&mut self) {
        if let Some(source) = self.poller.take() {
            source.remove();
        }
    }
}

impl shell::Observer for SidebarPanel {
    fn state_changed(&self, state: &UiState, at: crate::Tick) {
        self.draw(state, at);
    }
}

impl shell::Panel for SidebarPanel {
    fn root(&self) -> gtk::Widget {
        self.tree.borrow().widget.clone()
    }
}

/// What the staged-build poller found, or `Current` when there is no install
/// directory to look in.
fn standing() -> crate::update::Standing {
    match crate::update::install_dir() {
        Ok(dir) => crate::update::standing(&dir, None),
        Err(_) => crate::update::Standing::Current,
    }
}

/// The agent the footer's primary control will start, or `None`.
///
/// `None` is not a loading state to paper over. An operator who has launched
/// nothing has no top-ranked launch, and the control says "New session" and
/// opens the list rather than guessing an agent off PATH.
fn launch_word() -> Option<String> {
    crate::ui::dialog::top_word(&crate::launch::load_launch_store(), crate::launch::now_ms())
}

// ───────────────────────────────────────────────────────────────────────────
// The rendered tree
// ───────────────────────────────────────────────────────────────────────────

/// One widget, the node it was built from, and the widgets under it.
struct Rendered {
    node: Node,
    widget: gtk::Widget,
    /// Where children are packed. `None` for a leaf.
    holder: Option<gtk::Container>,
    /// The action a press raises, in a cell so a row that moves to another
    /// session does not need its handler rewired.
    act: Option<Rc<Cell<Act>>>,
    /// The session whose row occupies this place, for a seat.
    seat: Option<SessionId>,
    kids: Vec<Rendered>,
}

/// A patched widget tree with no session rows in it.
///
/// The pane bar is described by the same [`Node`] vocabulary and wants the
/// same guarantee the sidebar wants: a strip whose contents change without a
/// widget being destroyed, so its height cannot move and the pane's rectangle
/// cannot move with it. One interpreter for both, so the two panels cannot
/// come to disagree about what a node means.
pub(crate) struct Surface {
    tree: RefCell<Rendered>,
    settling: Rc<Cell<bool>>,
    /// Always empty. A surface that seated a session row would be a second
    /// owner of a row the store already owns.
    rows: Rows<GtkRow>,
}

impl Surface {
    /// Build the widgets one node tree describes.
    pub(crate) fn new(node: &Node, shell: &Shell) -> Surface {
        let settling = Rc::new(Cell::new(false));
        let rows = Rows::default();
        let tree = build(node, shell, &settling, &rows, &mut Hovers::default());
        Surface {
            tree: RefCell::new(tree),
            settling,
            rows,
        }
    }

    /// The widget to mount.
    pub(crate) fn root(&self) -> gtk::Widget {
        self.tree.borrow().widget.clone()
    }

    /// Bring the widgets in line with a new tree.
    pub(crate) fn apply(&self, node: &Node, shell: &Shell) {
        let mut hovers = Hovers::default();
        patch(
            &mut self.tree.borrow_mut(),
            node,
            shell,
            &self.settling,
            &self.rows,
            &mut hovers,
        );
        self.root().show_all();
        hovers.rest();
    }
}

/// Everything in one row that appears only under the pointer.
///
/// Collected while building rather than looked up later, because the two
/// mechanisms are different widgets: a slot swaps which of its children shows,
/// and a detail is an overlay that is simply hidden.
#[derive(Default)]
struct Hovers {
    slots: Vec<gtk::Stack>,
    reveals: Vec<gtk::Widget>,
}

impl Hovers {
    /// Show what the pointer reveals.
    fn raise(&self) {
        for slot in &self.slots {
            if let Some(last) = slot.children().last() {
                slot.set_visible_child(last);
            }
        }
        for widget in &self.reveals {
            widget.show();
        }
    }

    /// Put it all back.
    fn rest(&self) {
        for slot in &self.slots {
            if let Some(first) = slot.children().first() {
                slot.set_visible_child(first);
            }
        }
        for widget in &self.reveals {
            widget.hide();
        }
    }
}

/// Whether an existing widget can be patched into `next`, or has to be
/// replaced.
///
/// Kind and seat only. Classes, text, sensitivity and the action behind a
/// press are all applied to a widget that already exists; nothing else about a
/// node can change what widget it needs to be.
fn compatible(old: &Node, next: &Node) -> bool {
    match (&old.kind, &next.kind) {
        (Kind::Seat(a), Kind::Seat(b)) => a == b,
        (Kind::Mark(_), Kind::Mark(_)) => old.kind == next.kind,
        (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
    }
}

/// Build the widget one node describes, and everything under it.
fn build(
    node: &Node,
    shell: &Shell,
    settling: &Rc<Cell<bool>>,
    rows: &Rows<GtkRow>,
    hovers: &mut Hovers,
) -> Rendered {
    let mut act = None;
    let mut seat = None;
    let (widget, holder): (gtk::Widget, Option<gtk::Container>) = match &node.kind {
        Kind::Row => {
            let boxed = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            (boxed.clone().upcast(), Some(boxed.upcast()))
        }
        Kind::Column => {
            let boxed = gtk::Box::new(gtk::Orientation::Vertical, 0);
            (boxed.clone().upcast(), Some(boxed.upcast()))
        }
        Kind::Scroller => {
            let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
            // Never horizontally: a sidebar that scrolls sideways is a sidebar
            // whose labels were allowed to set its width, and the elision on
            // those labels exists so they cannot.
            scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            // The list body is where the keyboard lands when no row can take
            // focus, so the panel offers it up by the name the chord uses.
            shell.register_focus("rg-sidebar-body", &scroll);
            (scroll.clone().upcast(), Some(scroll.upcast()))
        }
        Kind::Label => {
            let label = gtk::Label::new(Some(&node.text));
            label.set_xalign(0.0);
            if node.eliding {
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            }
            reserve_width(&label, node.chars);
            (label.upcast(), None)
        }
        Kind::Dot | Kind::Rule => {
            // A rule and a dot are both a painted box with no content. GTK's
            // separator carries its own theme colour, which is one more thing
            // to override than an empty box that has never had one.
            let boxed = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            (boxed.upcast(), None)
        }
        Kind::Mark(mark) => (
            crate::ui::glyph::mark(mark.stroke, mark.fill, &node.class).upcast(),
            None,
        ),
        Kind::Press(initial) => {
            let button = gtk::Button::new();
            button.set_relief(gtk::ReliefStyle::None);
            // A control is the height its rule states, not the height of the
            // row it happens to sit in. Filling made the footer's buttons 48px
            // tall against a stated 32, so their bottom edge went off the
            // window and their top edge drew a line across the sidebar.
            button.set_valign(gtk::Align::Center);
            let inner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            button.add(&inner);
            let held = Rc::new(Cell::new(*initial));
            act = Some(Rc::clone(&held));
            wire(&button, shell, &held);
            (button.upcast(), Some(inner.upcast()))
        }
        Kind::Field { .. } => {
            let entry = gtk::Entry::new();
            entry.set_text(&node.text);
            // Every keyboard route into the filter asks the shell for this
            // name. Nothing registering it is a chord that silently does
            // nothing, which is what the previous surface shipped.
            shell.register_focus("rg-filter", &entry);
            wire_field(&entry, shell, settling);
            (entry.upcast(), None)
        }
        Kind::Over => {
            let overlay = gtk::Overlay::new();
            (overlay.clone().upcast(), Some(overlay.upcast()))
        }
        Kind::Stack => {
            let stack = gtk::Stack::new();
            // Both children get the same cell and the cell is as wide as the
            // wider of them, so revealing the controls on a row cannot move
            // the time that was there a moment ago.
            stack.set_hhomogeneous(true);
            stack.set_vhomogeneous(true);
            stack.set_transition_type(gtk::StackTransitionType::None);
            hovers.slots.push(stack.clone());
            (stack.clone().upcast(), Some(stack.upcast()))
        }
        Kind::Seat(id) => {
            seat = Some(*id);
            let row = rows
                .view(*id)
                .expect("a seat in the tree without a row beside it");
            (row.root.clone(), None)
        }
    };

    let mut rendered = Rendered {
        node: node.clone(),
        widget,
        holder,
        act,
        seat,
        kids: Vec::new(),
    };
    dress(&rendered, node, settling);
    for (index, child) in node.children.iter().enumerate() {
        let built = build(child, shell, settling, rows, hovers);
        attach(&rendered, &built, index, hovers);
        rendered.kids.push(built);
    }
    rendered
}

/// Put one built child into its parent.
fn attach(parent: &Rendered, child: &Rendered, index: usize, hovers: &mut Hovers) {
    let Some(holder) = &parent.holder else {
        return;
    };
    if let Ok(overlay) = holder.clone().downcast::<gtk::Overlay>() {
        if index == 0 {
            overlay.add(&child.widget);
        } else {
            // An overlay child adds nothing to the parent's size request,
            // which is why the row's detail is one: it can be wider than the
            // row without the row becoming that wide.
            overlay.add_overlay(&child.widget);
            child.widget.set_no_show_all(true);
            child.widget.hide();
            hovers.reveals.push(child.widget.clone());
        }
        return;
    }
    if let Ok(stack) = holder.clone().downcast::<gtk::Stack>() {
        stack.add_named(&child.widget, &index.to_string());
        return;
    }
    if let Ok(boxed) = holder.clone().downcast::<gtk::Box>() {
        boxed.pack_start(&child.widget, child.node.grow, true, 0);
        return;
    }
    holder.add(&child.widget);
}

/// Hold a label at a fixed character width, or release it.
///
/// `set_width_chars` is a MINIMUM in the font's average character width, which
/// is what a reservation wants: the box stops shrinking when the word does,
/// and a word wider than the reservation still fits. Centred, because the
/// reserved elements are a pill's word and the counters beside it, and a short
/// word left-aligned in a box sized for a long one reads as a mistake.
fn reserve_width(label: &gtk::Label, chars: u16) {
    if chars == 0 {
        return;
    }
    label.set_width_chars(i32::from(chars));
    label.set_xalign(0.5);
}

/// Give a widget the classes, text, name and sensitivity its node asks for.
fn dress(rendered: &Rendered, node: &Node, settling: &Rc<Cell<bool>>) {
    let widget = &rendered.widget;
    // A seat's widget belongs to its row and wears the row's own classes. The
    // seat node is a position in the tree and nothing else.
    if rendered.seat.is_some() {
        return;
    }
    sheet::set_classes(&widget.style_context(), &node.class);
    widget.set_sensitive(node.enabled);
    widget.set_valign(if node.centred {
        gtk::Align::Center
    } else {
        gtk::Align::Fill
    });
    match &node.kind {
        Kind::Label => {
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                if label.text() != node.text.as_str() {
                    label.set_text(&node.text);
                }
                reserve_width(label, node.chars);
            }
        }
        Kind::Field { placeholder } => {
            if let Some(entry) = widget.downcast_ref::<gtk::Entry>() {
                let shown = if placeholder.is_empty() {
                    None
                } else {
                    Some(*placeholder)
                };
                entry.set_placeholder_text(shown);
                // The field is written to when something other than typing
                // changed the filter: a chord that clears it, another window's
                // state arriving. Flagged while it happens, because the write
                // raises the same signal a keystroke does and reading it back
                // as one would fight the caret.
                if entry.text() != node.text.as_str() {
                    settling.set(true);
                    entry.set_text(&node.text);
                    settling.set(false);
                }
            }
        }
        Kind::Press(_) => {
            // A control whose whole content is a word carries it as text on
            // the node rather than as a child, because a one-label child is a
            // box, a label and a pack for something a button already does.
            // Only when it has no children: a control with both would need
            // this label to keep a position among them, and none has both.
            if !node.text.is_empty()
                && node.children.is_empty()
                && let Some(holder) = &rendered.holder
            {
                match holder
                    .children()
                    .first()
                    .and_then(|w| w.downcast_ref::<gtk::Label>())
                {
                    Some(label) => label.set_text(&node.text),
                    None => {
                        let label = gtk::Label::new(Some(&node.text));
                        holder.add(&label);
                    }
                }
            }
        }
        _ => {}
    }
    if !node.name.is_empty()
        && let Some(object) = widget.accessible()
    {
        object.set_name(&node.name);
    }
}

/// Bring one rendered subtree in line with `next`.
///
/// Assumes [`compatible`] already said yes for this pair.
fn patch(
    rendered: &mut Rendered,
    next: &Node,
    shell: &Shell,
    settling: &Rc<Cell<bool>>,
    rows: &Rows<GtkRow>,
    hovers: &mut Hovers,
) {
    if let (Some(held), Kind::Press(act)) = (&rendered.act, &next.kind) {
        held.set(*act);
    }
    if rendered.node != *next {
        dress(rendered, next, settling);
    }
    if let Kind::Stack = next.kind
        && let Some(holder) = &rendered.holder
        && let Ok(stack) = holder.clone().downcast::<gtk::Stack>()
    {
        hovers.slots.push(stack);
    }

    let same_shape = rendered.kids.len() == next.children.len()
        && rendered
            .kids
            .iter()
            .zip(&next.children)
            .all(|(old, new)| compatible(&old.node, new));

    if same_shape {
        for (index, (old, new)) in rendered.kids.iter_mut().zip(&next.children).enumerate() {
            let regrown = old.node.grow != new.grow;
            patch(old, new, shell, settling, rows, hovers);
            if regrown
                && let Some(holder) = &rendered.holder
                && let Ok(boxed) = holder.clone().downcast::<gtk::Box>()
            {
                boxed.set_child_packing(&old.widget, new.grow, true, 0, gtk::PackType::Start);
            }
            if let Kind::Over = next.kind
                && index > 0
            {
                hovers.reveals.push(old.widget.clone());
            }
        }
    } else {
        for old in &rendered.kids {
            release(old);
        }
        rendered.kids.clear();
        for (index, child) in next.children.iter().enumerate() {
            let built = build(child, shell, settling, rows, hovers);
            attach(rendered, &built, index, hovers);
            rendered.kids.push(built);
        }
    }
    rendered.node = next.clone();
}

/// Take one subtree off the window.
///
/// Session rows are lifted out first and left alive: the store owns them, they
/// are almost always about to be packed somewhere else in the same paint, and
/// removing their container would otherwise destroy them.
fn release(rendered: &Rendered) {
    unseat(rendered);
    if rendered.seat.is_some() {
        return;
    }
    if let Some(parent) = rendered.widget.parent()
        && let Ok(container) = parent.downcast::<gtk::Container>()
    {
        container.remove(&rendered.widget);
    }
}

/// Detach every session row under `rendered` from whatever holds it.
fn unseat(rendered: &Rendered) {
    if rendered.seat.is_some() {
        if let Some(parent) = rendered.widget.parent()
            && let Ok(container) = parent.downcast::<gtk::Container>()
        {
            container.remove(&rendered.widget);
        }
        return;
    }
    for kid in &rendered.kids {
        unseat(kid);
    }
}

// ───────────────────────────────────────────────────────────────────────────
// One session row
// ───────────────────────────────────────────────────────────────────────────

/// One row on screen: its widget, the tree it came from, and what its pointer
/// reveals.
pub(crate) struct GtkRow {
    root: gtk::Widget,
    tree: RefCell<Rendered>,
    hovers: Rc<RefCell<Hovers>>,
}

impl RowView for GtkRow {
    type Cx = Shell;

    fn build(fold: &super::fold::RowFold, shell: &Shell) -> Self {
        let settling = Rc::new(Cell::new(false));
        let empty = Rows::default();
        let mut collected = Hovers::default();
        let tree = build(&fold.node, shell, &settling, &empty, &mut collected);
        let root = tree.widget.clone();
        let hovers = Rc::new(RefCell::new(collected));
        // The reveal is ours and not the platform's. A GTK tooltip is a
        // separate window that outlives the row it described: the list
        // reorders under it and it goes on floating an opaque rectangle over
        // whatever moved into that place.
        root.add_events(gtk::gdk::EventMask::ENTER_NOTIFY_MASK | gtk::gdk::EventMask::LEAVE_NOTIFY_MASK);
        {
            let hovers = Rc::clone(&hovers);
            root.connect_enter_notify_event(move |_, _| {
                hovers.borrow().raise();
                glib::Propagation::Proceed
            });
        }
        {
            let hovers = Rc::clone(&hovers);
            root.connect_leave_notify_event(move |_, event| {
                // A crossing into a child of the row is still inside the row.
                // Without this the controls the pointer is travelling toward
                // vanish the moment it reaches them.
                if event.detail() == gtk::gdk::NotifyType::Inferior {
                    return glib::Propagation::Proceed;
                }
                hovers.borrow().rest();
                glib::Propagation::Proceed
            });
        }
        // Row traversal moves focus to the row it landed on, so the row that
        // scrolled into view is the one the screen reader and the next key
        // both address. Registering on build and again on apply is what keeps
        // the id pointing at the widget currently standing for that session.
        root.set_can_focus(true);
        shell.register_focus(super::row_id(fold.id), &root);
        hovers.borrow().rest();
        GtkRow {
            root,
            tree: RefCell::new(tree),
            hovers,
        }
    }

    fn apply(&self, fold: &super::fold::RowFold, shell: &Shell) {
        let settling = Rc::new(Cell::new(false));
        let empty = Rows::default();
        let mut collected = Hovers::default();
        patch(
            &mut self.tree.borrow_mut(),
            &fold.node,
            shell,
            &settling,
            &empty,
            &mut collected,
        );
        self.root.show_all();
        collected.rest();
        // A recycled row now stands for a different session, so the id the
        // keyboard uses has to follow the reuse or focus lands on whatever
        // row happened to hold that place before.
        shell.register_focus(super::row_id(fold.id), &self.root);
        *self.hovers.borrow_mut() = collected;
    }
}

// ───────────────────────────────────────────────────────────────────────────
// What a press does
// ───────────────────────────────────────────────────────────────────────────

/// Connect one control to the action its node names.
///
/// Both signals, because they answer different questions. `clicked` is the one
/// a keyboard raises and it carries no modifiers; `button-press-event` carries
/// them and carries the right button, and it is the only way a row can tell a
/// plain click from a click that extends a selection.
fn wire(button: &gtk::Button, shell: &Shell, act: &Rc<Cell<Act>>) {
    {
        let armed = Rc::new(Cell::new(true));
        let guard = Rc::clone(&armed);
        let clicked_shell = shell.clone();
        let clicked_act = Rc::clone(act);
        button.connect_clicked(move |_| {
            // A pointer click raises both signals. The press handler took it
            // already, with the modifiers this one cannot see.
            if !guard.replace(true) {
                return;
            }
            perform(&clicked_shell, clicked_act.get(), crate::state::Click::Plain);
        });
        let shell = shell.clone();
        let pressed = Rc::clone(act);
        button.connect_button_press_event(move |_, event| {
            armed.set(false);
            let state = event.state();
            let held = super::click_kind(
                state.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                    || state.contains(gtk::gdk::ModifierType::META_MASK),
                state.contains(gtk::gdk::ModifierType::SHIFT_MASK),
            );
            if event.button() == 3 {
                if let Act::Select(id) = pressed.get() {
                    menu(&shell, id, event);
                }
                return glib::Propagation::Stop;
            }
            perform(&shell, pressed.get(), held);
            glib::Propagation::Stop
        });
    }
}

/// The filter field: every keystroke is the filter, and Escape empties it.
fn wire_field(entry: &gtk::Entry, shell: &Shell, settling: &Rc<Cell<bool>>) {
    {
        let shell = shell.clone();
        let settling = Rc::clone(settling);
        entry.connect_changed(move |entry| {
            if settling.get() {
                return;
            }
            let text = entry.text().to_string();
            shell.update(move |st| st.window.filter = text);
        });
    }
    let shell = shell.clone();
    entry.connect_key_press_event(move |_, event| {
        if event.keyval() == gtk::gdk::keys::constants::Escape {
            shell.update(|st| st.window.filter.clear());
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
}

/// Open the context menu for one row.
///
/// Set as a layer rather than presented here, so the menu goes up through the
/// same path as every other surface. Two ways to open one popover is two
/// places a chord, a dismissal or a repaint has to be taught about it.
fn menu(shell: &Shell, id: SessionId, event: &gtk::gdk::EventButton) {
    let (x, y) = event.root();
    let anchor = shell
        .window()
        .window()
        .map(|gdk| {
            let (_, ox, oy) = gdk.origin();
            (x as i32 - ox, y as i32 - oy)
        })
        .unwrap_or((x as i32, y as i32));
    let state = MenuState {
        x: f64::from(anchor.0),
        y: f64::from(anchor.1),
        target: id,
    };
    shell.update(move |st| st.window.layer = Layer::Menu(state));
}

/// Do what one control says.
///
/// Every branch is either a mutation of client state or an intent the reducer
/// carries out. Nothing here talks to the daemon, reads a profile off disk or
/// decides what a launch should be: a panel that did any of those would be a
/// second answer to a question that already has one.
fn perform(shell: &Shell, act: Act, click: crate::state::Click) {
    let at = crate::tick();
    match act {
        Act::Select(id) => {
            shell.update(move |st| st.click_row(id, click, at.model));
            // Only a plain click opens. A modifier click is building a set to
            // act on and must not drag the pane along with it.
            if click == crate::state::Click::Plain {
                shell.update(move |st| st.open(id, at.now_ms));
                shell.send(ClientEvent::Reconcile);
            }
        }
        Act::Close(id) => shell.send(ClientEvent::Terminate { targets: vec![id] }),
        // The session has already exited. This drops its tab, which is what
        // moves the attachment, so the reducer has to hear about it. The
        // subject is read here rather than carried, because the bar's control
        // is drawn at every state and there is not always a session to name.
        Act::StopViewing => {
            let Some(id) = shell.peek(|st| st.window.focused) else {
                return;
            };
            shell.update(move |st| st.close_tab(id));
            shell.send(ClientEvent::Reconcile);
        }
        Act::ToggleProject(key) => shell.update(move |st| {
            if !st.window.collapsed.remove(&key) {
                st.window.collapsed.insert(key);
            }
        }),
        Act::ToggleSection(key, section) => shell.update(move |st| st.toggle_section(key, section)),
        Act::TogglePreview(key) => shell.update(move |st| st.toggle_preview(key)),
        Act::ToggleSettledTail(key) => {
            shell.update(move |st| st.window.toggle_settled_tail(key));
        }
        Act::ToggleSidebar => shell.update(|st| {
            st.window.sidebar_collapsed = !st.window.sidebar_collapsed;
        }),
        Act::Retry => shell.send(ClientEvent::Retry),
        // The same action as Ctrl+Shift+Down, through the same path, so the
        // count over the list and the chord cannot come to differ.
        Act::Jump => shell.send(ClientEvent::Key {
            action: crate::keymap::KeyAction::NextAttention,
        }),
        Act::NewSession => shell.update(|st| {
            let seed = crate::state::NewSessionSeed {
                project: None,
                cwd: crate::actions::seed_dir(st, None),
            };
            st.window.layer = Layer::NewSession(seed);
        }),
        Act::LaunchNow => shell.send(ClientEvent::LaunchNow { project: None }),
        Act::ClearFilter => shell.update(|st| st.window.filter.clear()),
        Act::Settings => shell.update(|st| {
            let next = Layer::Settings(SettingsTab::default());
            st.window.layer = if st.window.layer == next {
                Layer::None
            } else {
                next
            };
        }),
        // A new process of the same path, then this window closes. The staged
        // build is applied by that new process before it opens anything, so
        // nothing is swapped from inside the image being replaced. A spawn
        // that fails leaves the window exactly as it was, which is the right
        // failure: closing first would take the window away to install
        // nothing.
        Act::Restart => {
            let Ok(exe) = std::env::current_exe() else {
                return;
            };
            if std::process::Command::new(exe).spawn().is_ok() {
                shell.window().close();
            }
        }
    }
}
