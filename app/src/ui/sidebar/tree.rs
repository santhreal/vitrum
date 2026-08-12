//! What a panel draws, as data, before any widget exists.
//!
//! # Why the panel is folded before it is built
//!
//! A GTK widget cannot be made without a display, and every rule this sidebar
//! was written to defend is about ORDER, CONTENT or COST rather than about
//! pixels. Folding the panel into a [`Node`] tree first puts all three on a
//! machine with no display: a test asks the fold what the tail line's children
//! are, in what order, and what each of them says, and gets the answer the
//! builder is about to act on rather than a parallel description of it.
//!
//! The builder is an interpreter over this type and holds no rules of its own.
//! That is the whole point: a rule that lives in the builder is a rule only a
//! display can check, and the reason the panel this replaces shipped with a
//! status dot that had four colour modifiers and no box.
//!
//! # An empty element is a real element
//!
//! [`Node::text`] being empty never means "draw nothing". A branch arrives
//! from git, a worktree from the daemon and a time from the model, each after
//! the row is already on screen, and an element that is absent until then
//! reflows the row under a reader who is in the middle of it. The fold emits
//! the element, empty, and adds the sheet's `--empty` modifier so the box is
//! held at the same size it will have when the fact lands.

use vitrum_proto::SessionId;

use crate::agent::AgentMark;
use crate::state::GroupKey;
use vitrum_model::Section;

/// What a press on a widget raises.
///
/// A sidebar-local vocabulary, not a shell one. The shell deliberately has no
/// action enum, because one would have to name every panel's vocabulary and
/// put every panel back in one file. This names only what this panel can ask
/// for, so the fold can say WHICH control does WHAT and a test can read it
/// without a pointer: the product has shipped a "Show" button that could not
/// be clicked and a gear that rendered itself dead, and both are visible here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    /// Focus, extend or toggle one row, depending on the modifiers held.
    Select(SessionId),
    /// Terminate one session.
    Close(SessionId),
    /// Collapse or expand one bucket.
    ToggleProject(GroupKey),
    /// Collapse or expand one band of one bucket.
    ToggleSection(GroupKey, Section),
    /// Show the rest of one bucket's inbox, past the preview cut.
    TogglePreview(GroupKey),
    /// Show the rest of one bucket's finished sessions.
    ToggleSettledTail(GroupKey),
    /// Collapse or expand the panel itself.
    ToggleSidebar,
    /// Reconnect to the daemon.
    Retry,
    /// Move focus to the next session waiting on the operator.
    Jump,
    /// Open the ranked launcher.
    NewSession,
    /// Start the top-ranked launch with no layer at all.
    LaunchNow,
    /// Empty the filter field.
    ClearFilter,
    /// Open the settings surface.
    Settings,
    /// Restart into the staged build.
    Restart,
    /// Stop drawing the focused session's transcript.
    ///
    /// Not a termination: the session has already ended, and this takes its
    /// tab out of the strip. The pane bar's one control, and it carries no
    /// session because the bar has exactly one subject and reading it at the
    /// press is the only way the control can exist at every state without
    /// naming a session that is not there.
    StopViewing,
}

/// What kind of widget a [`Node`] becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A horizontal box.
    Row,
    /// A vertical box.
    Column,
    /// A text label. [`Node::text`] is what it says, and may be empty.
    Label,
    /// The agent identity mark, drawn from its own path data.
    Mark(AgentMark),
    /// A clickable surface, and what a press on it raises.
    Press(Act),
    /// The filter field. [`Node::text`] is the query already in it.
    Field {
        /// Empty below the narrow threshold, where the engine would clip it.
        placeholder: &'static str,
    },
    /// A box whose children after the first are drawn OVER the first and take
    /// no space from it.
    ///
    /// The row's hover detail is one. A `title` attribute, or its GTK
    /// equivalent, is a request for a platform WINDOW anchored to the pointer:
    /// reorder the list under a stationary cursor and it stays where it was,
    /// over rows it no longer describes, painted in the desktop's colours
    /// rather than this product's. An overlay child is a child of the thing it
    /// describes, so the same allocation that moves the row moves it, and it
    /// cannot change the row's height because an overlay child never
    /// contributes to its parent's size request.
    Over,
    /// A place in the tree where a session row goes.
    ///
    /// The row itself is not inlined here. Rows are the only thing in the
    /// panel that survives a repaint, so they are folded separately into
    /// [`super::rows::Rows`] and looked up by id; the seat says where one
    /// belongs in the list and nothing else.
    Seat(SessionId),
    /// A hairline rule with no text.
    Rule,
    /// A one-cell container: every child occupies the same cell, and the cell
    /// is as wide as the widest of them whichever one is showing.
    ///
    /// The row's right-hand slot is one, and that is what fixed the collision
    /// at the panel's 224px floor. The timestamp and the hover actions share
    /// the cell, so the column's width never depends on which of them the
    /// pointer has brought up, and nothing on the right of a row can move.
    Stack,
    /// A dot with no text: a status mark, a connection light.
    Dot,
    /// A vertical scrolling viewport around a single child.
    ///
    /// The list scrolls and the toolbar, the banner and the floor do not, so
    /// the scrolled region is described here rather than being an assumption
    /// the builder makes about the sidebar's second child.
    Scroller,
}

/// Which end of a label the toolkit sacrifices when the text will not fit.
///
/// A path and a sentence do not degrade the same way, and treating them alike
/// is how a row comes to say `/src/…/hi…`. The sidebar already shortens a path
/// to a column budget before it ever reaches a widget, keeping the first
/// component and the leaf; a widget narrower than that budget then elides a
/// SECOND time, and cutting the tail throws away the leaf the first pass was
/// written to preserve. The remainder is not a shorter answer, it is no
/// answer: `hi…` names nothing and cost the same pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Elide {
    /// Never elide. The label widens or the box does.
    Off,
    /// Sacrifice the end. For prose, where the first words identify it: a
    /// session title, a branch, a hover detail.
    Tail,
    /// Sacrifice the beginning. For a path, where the LAST component
    /// identifies it and the ancestors are context the group header usually
    /// repeats anyway.
    Head,
}

/// One widget, with the classes it wears and the text it says.
///
/// `PartialEq` is the memoization: two folds that compare equal describe the
/// same pixels, so the builder can skip the subtree outright.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) kind: Kind,
    /// Every style class this widget wears, space separated.
    ///
    /// A `String` rather than a list because half of them come from
    /// [`crate::inbox`] already assembled, and because the sheet is keyed off
    /// the exact spelling either way.
    pub(crate) class: String,
    /// What it says. Empty is a value, not an absence: see the module note.
    pub(crate) text: String,
    /// The accessible name, when the visible text is not one.
    ///
    /// A glyph control and an element whose word the operator switched off
    /// both need it. Empty means the text already is the name.
    pub(crate) name: String,
    /// Whether the control accepts a press.
    ///
    /// A control that is drawn and refuses is honest; a control that is drawn
    /// and silently does nothing is the defect a landing-order scaffold leaves
    /// behind. Nothing but a disconnected daemon sets this false.
    pub(crate) enabled: bool,
    /// Whether this child takes the space its siblings do not want.
    ///
    /// The toolkit's answer to a flex spacer, and it is load-bearing rather
    /// than cosmetic: the tail line's branch is the one element that grows, so
    /// everything after it is pushed to the row's right edge whether or not
    /// the branch has a name in it yet. Exactly one child of a box should set
    /// it; two make the space split and the right edge move.
    pub(crate) grow: bool,
    /// Which end of this label is sacrificed when the text outruns its column.
    ///
    /// Only for text that can legitimately be longer than its column: a
    /// title, a path, a branch, a hover detail. A count or a glyph must not
    /// set it, because an elidable label asks for the width of an ellipsis
    /// and will be squeezed to one under pressure.
    pub(crate) elide: Elide,
    /// The width this label holds, in characters, whatever it currently says.
    ///
    /// Zero is natural width. Anything above it is a label whose VALUE changes
    /// while the row is on screen: a status word, an elapsed counter, a
    /// countdown. `--empty` holds a box that has not been filled yet; this
    /// holds a box that will be filled again with something a different length,
    /// which is the same reflow arriving once a second instead of once a
    /// session. The width is reserved for the widest string the element can
    /// ever say, so the title beside it keeps one ellipsis point for the life
    /// of the row.
    pub(crate) chars: u16,
    /// Whether this node sits at its band's centre rather than filling it.
    ///
    /// A box that is shorter than the row it is in fills the row by default,
    /// so its background paints the row's full height and a twenty-pixel
    /// badge comes out as a thirty-two-pixel slab. Set it on the element that
    /// states its own height and means it.
    pub(crate) centred: bool,
    /// Whether this node's CONTENTS sit at the centre of its width.
    ///
    /// Distinct from `centred`, which is the cross axis. GTK3's stylesheet
    /// has no horizontal alignment, so this is the only way to say it, and
    /// only a control that spans a region says it: a full-width press whose
    /// label hugs the left edge reads as a label that happens to be
    /// clickable, which is what the bottom of the session list read as.
    pub(crate) mid: bool,
    pub(crate) children: Vec<Node>,
}

impl Node {
    /// A container with no text.
    pub(crate) fn new(kind: Kind, class: &str) -> Self {
        Node {
            kind,
            class: class.to_string(),
            text: String::new(),
            name: String::new(),
            children: Vec::new(),
            enabled: true,
            grow: false,
            elide: Elide::Off,
            chars: 0,
            centred: false,
            mid: false,
        }
    }

    /// A horizontal box.
    pub(crate) fn row(class: &str) -> Self {
        Node::new(Kind::Row, class)
    }

    /// A vertical box.
    pub(crate) fn column(class: &str) -> Self {
        Node::new(Kind::Column, class)
    }

    /// A label saying `text`.
    pub(crate) fn label(class: &str, text: impl Into<String>) -> Self {
        Node {
            kind: Kind::Label,
            class: class.to_string(),
            text: text.into(),
            name: String::new(),
            enabled: true,
            grow: false,
            elide: Elide::Off,
            chars: 0,
            centred: false,
            mid: false,
            children: Vec::new(),
        }
    }

    /// A label that is present whether or not its fact has resolved.
    ///
    /// Empty text keeps the element and adds the sheet's `--empty` modifier,
    /// which holds the box at its filled size in transparent ink. The
    /// alternative, dropping the element, is what shoves the rest of a row
    /// sideways the moment the daemon answers.
    pub(crate) fn reserved(class: &'static str, text: impl Into<String>) -> Self {
        let text = text.into();
        let class = if text.is_empty() {
            format!("{class} {class}--empty")
        } else {
            class.to_string()
        };
        Node {
            kind: Kind::Label,
            class,
            text,
            name: String::new(),
            enabled: true,
            grow: false,
            elide: Elide::Off,
            chars: 0,
            centred: false,
            mid: false,
            children: Vec::new(),
        }
    }

    /// Hold this label at `chars` characters whatever it says.
    ///
    /// See [`Node::chars`]. Applied to an element whose value changes under a
    /// reader, never to one that is merely long: a title that wants more room
    /// elides, and an element that reserves room it does not need is width
    /// stolen from the title on a 224px row.
    pub(crate) fn wide(mut self, chars: u16) -> Self {
        self.chars = chars;
        self
    }

    /// Sit at the centre of the band rather than filling it.
    pub(crate) fn centred(mut self) -> Self {
        self.centred = true;
        self
    }

    /// Put this node's contents at the centre of its width.
    pub(crate) fn mid(mut self) -> Self {
        self.mid = true;
        self
    }

    /// A control that raises `act` when it is pressed.
    pub(crate) fn press(class: &str, act: Act) -> Self {
        Node::new(Kind::Press(act), class)
    }

    /// Give this node an accessible name that its text does not carry.
    pub(crate) fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Draw this control but refuse a press on it.
    pub(crate) fn refusing(mut self, refuse: bool) -> Self {
        self.enabled = !refuse;
        self
    }

    /// Take the space this node's siblings do not want.
    pub(crate) fn growing(mut self) -> Self {
        self.grow = true;
        self
    }

    /// Elide the END rather than widen when the text outruns the column.
    pub(crate) fn eliding(mut self) -> Self {
        self.elide = Elide::Tail;
        self
    }

    /// Elide the BEGINNING, keeping the leaf. For a path and nothing else.
    pub(crate) fn eliding_leaf(mut self) -> Self {
        self.elide = Elide::Head;
        self
    }

    /// Give this node some text.
    pub(crate) fn saying(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// Append one child.
    pub(crate) fn with(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }

    /// Append one child when `child` is `Some`.
    pub(crate) fn maybe(self, child: Option<Node>) -> Self {
        match child {
            Some(child) => self.with(child),
            None => self,
        }
    }

    /// Every node in this subtree, parents before children, in draw order.
    pub(crate) fn walk(&self) -> Vec<&Node> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect<'a>(&'a self, out: &mut Vec<&'a Node>) {
        out.push(self);
        for child in &self.children {
            child.collect(out);
        }
    }

    /// Every session seated in this subtree, in draw order.
    pub(crate) fn seats(&self) -> Vec<SessionId> {
        self.walk()
            .into_iter()
            .filter_map(|node| match node.kind {
                Kind::Seat(id) => Some(id),
                _ => None,
            })
            .collect()
    }

}
