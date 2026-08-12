//! The whole sidebar, folded from state into a [`Node`] tree.
//!
//! Nothing here touches a toolkit and nothing here decides what a session IS.
//! The bands, the ordering, the dispositions and the times come from
//! `vitrum-model` through [`crate::inbox`]; the visibility of a bucket, a band
//! and a shelf tail comes from [`crate::state::WindowState`]. This assembles
//! them into the elements the panel draws, in the order it draws them, and
//! stops.
//!
//! # Two rules that are structural and not cosmetic
//!
//! **The card is exactly two lines and neither is conditional.** A third line
//! emitted only when a badge existed put rows of three different heights in
//! one band and gave the title 127px, 69.5px and 12.5px of box at the panel's
//! 224px floor, with the close button 33px, 90.5px and 147.5px from the right
//! edge on those same three rows. One list, three row heights, three title
//! widths, three positions for one control.
//!
//! **An element that appears when a fact resolves is emitted before it
//! resolves, empty.** A worktree name is read off the filesystem by the daemon
//! after the session is announced, and a chip that pops in then shoves the
//! rest of the line sideways under a reader who is in the middle of it.
//! [`Node::reserved`] is that rule and the sheet's `--empty` modifiers are its
//! other half.
//!
//! **An element whose VALUE changes holds one width.** The rule above is about
//! a fact arriving; this is about a fact moving. A counter reading `9s` and
//! then `10s`, a pill reading `Ready` and then `Approval`, a slot reading
//! `just now` and then `4m ago` each take a different amount of room for the
//! same element, and the title beside them re-elides at a new point every
//! time. [`Node::wide`] holds those boxes at the widest string they can say,
//! so a row that changes state changes only its state.

use std::rc::Rc;

use vitrum_model::{AgentKind, Clock, Disposition, Section, SessionView};
use vitrum_proto::{Attention, SessionId};

use super::tree::{Act, Kind, Node};
use super::{
    CHEVRON, GEAR_ICON, GROUP_LABEL_COLUMNS, RowFields, RowState, SEARCH_ICON, agent_class,
    completion_shown, contest_title, draws_card, empty_bucket_hint, in_flight, place_label,
    recedes, row_clock, row_tooltip, row_variant, section_class,
};
use crate::agent::AgentMarks;
use crate::clock::age;
use crate::inbox::{self, Pill};
use crate::state::{ConnState, UiState, attention_label, attention_modifier};
use crate::update::Standing;

/// Width of the row's right-hand slot, in characters.
///
/// `just now` is the longest label either occupant of that cell can hold at
/// the second and minute scales where the value actually moves; a calendar
/// date is wider and never changes again once a row is that old. Both the
/// timestamp and the return ticket take it, so the cell is one width whichever
/// of them a row is showing.
pub(super) const SLOT_CHARS: u16 = 8;

/// Width of the pill's live counter, in characters.
///
/// `59s` and `59m` are the widest labels `format_duration_label` produces
/// below one hour, which is the whole range in which the number ticks under a
/// reader. The hour form is wider, arrives once, and then holds its width for
/// the next ten hours.
pub(super) const AUX_CHARS: u16 = 3;

/// One session row, folded.
///
/// Held apart from the panel tree because a row is the only thing in the
/// sidebar that survives a repaint. The tree carries a [`Kind::Seat`] where
/// one goes; [`super::rows::Rows`] holds the row itself, keyed by id, and
/// rebuilds it only when this compares unequal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowFold {
    pub(crate) id: SessionId,
    pub(crate) node: Node,
}

/// Everything the panel draws for one reading of the state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fold {
    /// The panel, as one tree, with a seat where each row goes.
    pub(crate) root: Node,
    /// Every row on screen, in draw order.
    pub(crate) rows: Vec<RowFold>,
    /// How many visible rows are on the attention queue.
    ///
    /// Resolved from the same arrangement the seats came from, so the number
    /// over the list and the list itself can never be counting two different
    /// things.
    pub(crate) attention: usize,
}

impl Fold {
    /// Every session seated in the panel, in draw order.
    #[cfg(test)]
    pub(crate) fn visible_ids(&self) -> Vec<SessionId> {
        self.root.seats()
    }
}

/// What the panel needs that is not in [`UiState`].
///
/// Four values the window settled once and a panel must not answer again: the
/// daemon URL and the home directory come from the shell's identity, the
/// staged-build standing from the update poller, and the launch word from the
/// operator's own profile.
#[derive(Debug, Clone, Default)]
pub(crate) struct Context {
    pub(crate) home: String,
    pub(crate) server: String,
    pub(crate) standing: Standing,
    /// The agent the footer's primary control will start, or `None` when this
    /// operator has launched nothing and the honest label is "New session".
    pub(crate) launch_word: Option<String>,
}

// ───────────────────────────────────────────────────────────────────────────
// The panel
// ───────────────────────────────────────────────────────────────────────────

/// Fold the whole panel.
pub(crate) fn panel(st: &UiState, at: crate::Tick, cx: &Context) -> Fold {
    let collapsed = st.window.sidebar_collapsed;
    // Three arrangements and the third is the same mechanism as the second.
    // `--collapsed` is the 48px rail; `--narrow` is the panel dragged below
    // its default, where the toolbar and a row's tail line both run out of
    // columns.
    let narrow = !collapsed && st.window.sidebar_width < crate::state::SIDEBAR_NARROW_PX;
    let mut class = String::from("rg-sidebar");
    if collapsed {
        class.push_str(" rg-sidebar--collapsed");
    } else if narrow {
        class.push_str(" rg-sidebar--narrow");
    }

    // ONE arrangement per paint. Every derivation below is taken from it
    // rather than re-derived: arranging resolves a status and a disposition
    // for every row and sorts three bands, and at thirty sessions on a daemon
    // pushing an update a second, doing that three times per paint is most of
    // what this client would spend its CPU on.
    let groups = st.tree(at.model);
    let attention = st.attention_count_of(&groups, at.model);
    let no_matches = st.window.filter_matched_nothing_in(groups.is_empty());
    // Read once per paint, not once per row: `Settings` is not `Copy`, and
    // twenty rows each reaching into it is twenty reads of the same six bits.
    let fields = RowFields::of(&st.daemon.settings);
    // One buffer for every row's hover detail. Cloning the `String` per row
    // costs an allocation and a copy per row per paint; a refcount bump costs
    // neither.
    let home: Rc<str> = Rc::from(cx.home.as_str());

    let mut rows: Vec<RowFold> = Vec::new();
    let mut body = Node::column("rg-sidebar__body");
    if let Some(empty) = empty_state(st, &groups, no_matches, at.model, cx) {
        body = body.with(empty);
    }
    for group in &groups {
        body = body.with(group_node(st, group, at, fields, &home, &mut rows));
    }
    if !groups.is_empty() {
        body = body.with(floor(st));
    }

    let root = Node::column(&class)
        .with(toolbar(st, collapsed, narrow, attention))
        .maybe(banner(st, &cx.server))
        .with(
            Node::new(Kind::Scroller, "rg-sidebar__scroll")
                .growing()
                .with(body),
        )
        .maybe(restart(st, &cx.standing))
        .with(footer(st, collapsed, cx));
    // The seats the tree carries and the rows folded beside it are one list
    // resolved once; nothing above may add a seat without a row.
    debug_assert_eq!(root.seats().len(), rows.len());

    Fold {
        root,
        rows,
        attention,
    }
}

/// The one row of chrome above the list.
///
/// Where there used to be four bands: a wordmark, a search field, a permanent
/// connection banner and a "Projects" caption over a list of projects. The
/// wordmark is in the titlebar, which is the only place a window should say
/// its own name; the connection state is a dot up there too, because a
/// full-width strip announcing a working socket is a band spent on a
/// non-event.
fn toolbar(st: &UiState, collapsed: bool, narrow: bool, attention: usize) -> Node {
    let bar = Node::row("rg-sidebar__toolbar");
    if collapsed {
        // At 3rem there is no room for a field. The magnifier expands the
        // panel, which is where the field is.
        return bar.with(
            Node::press("rg-sidebar__action", Act::ToggleSidebar)
                .saying(SEARCH_ICON)
                .named("Expand sidebar"),
        );
    }
    let mut search = Node::row("rg-sidebar__search")
        .growing()
        .with(Node::label("rg-sidebar__search-icon", SEARCH_ICON))
        .with(
            Node::new(
                Kind::Field {
                    // One word, and none at all at the panel's narrower
                    // widths. A placeholder is drawn against the field's base
                    // width rather than the width it ends up with, so below
                    // the threshold "Filter" lands in about 40px and paints as
                    // "Filte" with half the field empty after it, which reads
                    // as a rendering fault.
                    placeholder: if narrow { "" } else { "Filter" },
                },
                "rg-sidebar__search-input",
            )
            .saying(st.window.filter.clone())
            .named("Filter sessions")
            .growing(),
        );
    if !narrow {
        search = search.with(Node::label("rg-sidebar__search-kbd", "Ctrl+K"));
    }
    let mut bar = bar.with(search);
    // Zero draws nothing at all: the honest answer to "how many are waiting"
    // when none are is silence, not a grey nought.
    if attention > 0 {
        bar = bar.with(
            Node::press("rg-attn-count", Act::Jump)
                .centred()
                .named("Jump to the next session waiting on you")
                .with(Node::label("rg-attn-count__n", attention.to_string()))
                .with(Node::label("rg-attn-count__word", "waiting")),
        );
    }
    bar
}

/// The connection strip, and only when something is wrong.
///
/// A socket that is up says so with a dot in the titlebar and takes no
/// vertical space here.
fn banner(st: &UiState, server: &str) -> Option<Node> {
    let connecting = matches!(st.daemon.conn, ConnState::Connecting);
    let failed = st.daemon.conn.is_retryable();
    if !connecting && !failed {
        return None;
    }
    let text = st.daemon.conn.banner_text(server);
    let mut bar = Node::row(conn_class(&st.daemon.conn))
        .with(Node::new(Kind::Dot, "rg-conn__dot"))
        .with(Node::reserved("rg-conn__word", text));
    if failed {
        bar = bar.with(Node::press("rg-btn-inline", Act::Retry).saying("Retry"));
    }
    Some(bar)
}

/// Which strip the connection wears.
///
/// [`ConnState::banner_class`] names the classes of the web stylesheet, which
/// this window no longer loads, and it dies with the webview. The generated
/// GTK sheet paints `.rg-conn` and its four modifiers, and one name per
/// surface is the rule: the pill states what a session is doing, the strip
/// states whether the daemon answered.
fn conn_class(conn: &ConnState) -> &'static str {
    match conn {
        ConnState::Connecting => "rg-conn rg-conn--connecting",
        ConnState::Live { .. } => "rg-conn rg-conn--ok",
        ConnState::Fixture => "rg-conn rg-conn--fixture",
        _ => "rg-conn rg-conn--failed",
    }
}

/// Why there is nothing to draw, or `None` when there is.
///
/// Four answers and not one. It used to answer every empty tree with
/// "Projects appear here as soon as a session runs in one", which is true only
/// when the daemon really is empty and is a flat lie in the three cases that
/// matter: a failed search, a second workspace, which is blank BECAUSE that is
/// what a workspace is, and a workspace whose bands the operator switched off.
/// All of them read as "my sessions are gone", so two features that work end
/// to end were reported as missing on the strength of one string.
fn empty_state(
    st: &UiState,
    groups: &[crate::state::SidebarGroup<'_>],
    no_matches: bool,
    clock: Clock,
    cx: &Context,
) -> Option<Node> {
    let query = st.window.filter.clone();
    if no_matches {
        return Some(
            Node::column("rg-sidebar__empty rg-sidebar__empty--no-matches")
                .with(Node::label(
                    "rg-empty__title",
                    format!("Nothing matches \u{201c}{query}\u{201d}"),
                ))
                .with(Node::label(
                    "rg-empty__hint",
                    "Titles, commands, directories and branches are all searched.",
                ))
                .with(Node::press("rg-btn", Act::ClearFilter).saying("Clear filter")),
        );
    }
    if !groups.is_empty() {
        return None;
    }
    if matches!(st.daemon.conn, ConnState::Connecting) {
        return Some(
            Node::column("rg-sidebar__empty")
                .with(Node::label("rg-empty__title", "Connecting"))
                .with(Node::label(
                    "rg-empty__hint",
                    format!("Waiting for {} to answer.", cx.server),
                )),
        );
    }
    if st.daemon.conn.is_retryable() {
        return Some(
            Node::column("rg-sidebar__empty")
                .with(Node::label("rg-empty__title", "Not connected"))
                .with(Node::label(
                    "rg-empty__hint",
                    format!(
                        "The session daemon at {} is not answering. Sessions keep running while \
                         this window is disconnected.",
                        cx.server
                    ),
                ))
                .with(Node::press("rg-btn rg-btn--primary", Act::Retry).saying("Retry")),
        );
    }
    let (title, hint) = census_words(st, clock);
    let mut node = Node::column("rg-sidebar__empty");
    // The title is empty for the honest first-run state: the terminal pane
    // carries that one, and two surfaces shouting the same thing is worse than
    // one.
    if !title.is_empty() {
        node = node.with(Node::label("rg-empty__title", title));
    }
    Some(node.with(Node::label("rg-empty__hint", hint)))
}

/// What an empty workspace says, counted against the workspace the tree was
/// built from.
///
/// Counting against `st.window.workspace` while the tree fell back to another
/// one produced the worst sentence in the panel: every session counted as "in
/// another workspace", under a heading naming a workspace that no longer
/// existed.
fn census_words(st: &UiState, clock: Clock) -> (String, String) {
    let policy = st.daemon.policy();
    let here = st.window.drawn_workspace(&st.daemon);
    let ws = st.daemon.workspaces.get(here);
    let mut in_workspace = 0;
    let mut admitted = 0;
    for row in &st.daemon.sessions {
        if st.daemon.workspaces.workspace_of(&row.info) != here {
            continue;
        }
        in_workspace += 1;
        if ws.is_some_and(|w| w.sections.shows(row.disposition(clock, policy))) {
            admitted += 1;
        }
    }
    let census = inbox::Census {
        total: st.daemon.sessions.len(),
        in_workspace,
        admitted,
    };
    inbox::Empty::of(census).words(ws.map_or("This workspace", |w| w.name.as_str()))
}

/// What the bottom of the scroller is for.
///
/// Measured on a real workspace: three sessions in two projects fill 190px of
/// a 764px scroller, so 75% of the widest column in the window is nothing at
/// all. Growing the rows to fill is the wrong answer, because a row's height
/// is a legibility decision taken against the two lines it holds. So the
/// region carries the only honest thing it could carry, which is the place a
/// session is started, and it starts one.
fn floor(st: &UiState) -> Node {
    let ready = st.server_ready();
    Node::press("rg-sidebar__floor", Act::NewSession)
        .refusing(!ready)
        .named(if ready {
            "Start a session"
        } else {
            "Not connected"
        })
        .with(Node::label("rg-sidebar__floor-label", "Start a session"))
        .with(Node::label("rg-sidebar__floor-hint", "Ctrl+Shift+N"))
}

/// The restart offer, between the list and the footer.
///
/// Here rather than in the titlebar because the two say different things and
/// only one of them is an interruption. The titlebar's chip means "there is a
/// newer build, spend bandwidth"; this means "the bytes are already on disk
/// and verified, and the next start runs them".
///
/// THE ONE PLACE THE AFFORDANCE IS DECIDED. The setting decides what is DRAWN
/// and nothing else: the check that finds an update, the download that stages
/// it and the swap that applies it on the next start never read it, so an
/// operator who switches it off is still updated. They have asked not to be
/// told, not asked to stop.
fn restart(st: &UiState, standing: &Standing) -> Option<Node> {
    let version = crate::update::restart_offer(standing, st.daemon.settings.show_restart_to_update)?;
    Some(
        Node::press("rg-sidebar__restart", Act::Restart)
            .named(crate::update::RESTART_TO_UPDATE)
            .with(Node::new(Kind::Dot, "rg-sidebar__restart-dot"))
            .with(Node::label(
                "rg-sidebar__restart-line",
                crate::update::restart_line(version),
            )
            .eliding()),
    )
}

/// The product's most-used control, beside its two least-used ones.
///
/// The primary control has to carry a word, because it launches on the first
/// click and a `+` that starts a process is a mystery button. The toolbar
/// cannot hold a word at any sidebar width the product offers; this band has
/// 120px free at the 224px floor against the 112 the longest agent name needs.
/// Settings and the panel's own collapse are the two controls reached for
/// least often, and both work in the 3rem collapsed state.
fn footer(st: &UiState, collapsed: bool, cx: &Context) -> Node {
    let word = cx.launch_word.as_deref();
    let can_launch = word.is_some();
    // Drawn only when it would do something the primary half does not. With
    // nothing confident to start, both halves open the list, and two controls
    // for one action is duplication.
    let pick = can_launch && !collapsed;
    let ready = st.server_ready();
    let go = if can_launch { Act::LaunchNow } else { Act::NewSession };
    let mut newbar = Node::row(if pick {
        "rg-newbar"
    } else {
        "rg-newbar rg-newbar--solo"
    })
    .with(
        Node::press("rg-newbar__go", go)
            .growing()
            .refusing(!ready)
            .named(match word {
                Some(w) => format!("Start {w}"),
                None => "New session".to_string(),
            })
            .with(Node::label(
                "rg-newbar__what",
                crate::ui::dialog::go_label(word, !collapsed),
            )),
    );
    if pick {
        newbar = newbar.with(
            Node::press("rg-newbar__pick", Act::NewSession)
                .refusing(!ready)
                .saying(CHEVRON)
                .named("Choose what to start"),
        );
    }
    Node::row("rg-sidebar__footer")
        .with(newbar)
        .with(
            Node::press("rg-sidebar__action", Act::Settings)
                .saying(GEAR_ICON)
                .named("Settings"),
        )
        .with(
            Node::press("rg-sidebar__action", Act::ToggleSidebar)
                .saying(if collapsed { "\u{00bb}" } else { "\u{00ab}" })
                .named(if collapsed {
                    "Expand sidebar"
                } else {
                    "Collapse sidebar"
                }),
        )
}

// ───────────────────────────────────────────────────────────────────────────
// One bucket
// ───────────────────────────────────────────────────────────────────────────

/// One bucket: its header, its three bands and the seats in them.
fn group_node(
    st: &UiState,
    group: &crate::state::SidebarGroup<'_>,
    at: crate::Tick,
    fields: RowFields,
    home: &Rc<str>,
    rows: &mut Vec<RowFold>,
) -> Node {
    let key = group.key;
    // A folder or directory bucket's label is a path, and an absolute path in
    // a 14rem column is all prefix and no name. A daemon project's label is
    // already a bare name.
    let name = if group.label.starts_with('/') {
        vitrum_fmt::path::shorten_home_relative(&group.label, home, GROUP_LABEL_COLUMNS)
    } else {
        group.label.clone()
    };
    let root: &str = group.root.as_deref().unwrap_or_default();
    // The same buffer for every row of the bucket.
    let row_root: Rc<str> = Rc::from(root);
    // The Unfiled bucket has no name to look for its rows under, so it cannot
    // be collapsed: doing so would hide sessions behind a header that does not
    // say what is in it.
    let is_collapsed = group.collapsible() && st.window.collapsed.contains(&key);
    let mut class = String::from("rg-project");
    if group.current {
        class.push_str(" rg-project--current");
    }
    if is_collapsed {
        class.push_str(" rg-project--collapsed");
    }
    // The rollup answers "should I open this" and only a collapsed header has
    // that question. An expanded group answers it with its own rows.
    let rollup = is_collapsed.then(|| group.bands.rollup.clone()).flatten();
    // NEVER empty. A folder or Unfiled bucket has no filesystem root, so this
    // fell through to `root`, which is `None` for both, and every folder
    // header in named grouping shipped a literal empty hover detail.
    let tip = {
        let mut text = String::from(if root.is_empty() { name.as_str() } else { root });
        if let Some(r) = rollup.as_ref() {
            text.push('\n');
            text.push_str(&inbox::rollup_title(r));
        }
        text
    };

    let header = if group.collapsible() {
        let mut header = Node::press("rg-project__header", Act::ToggleProject(key))
            .with(Node::label("rg-project__chevron", CHEVRON))
            .with(Node::label("rg-project__name", name.clone()).growing().eliding());
        if let Some(rollup) = rollup {
            header = header.with(rollup_node(&rollup));
        }
        header.with(Node::label("rg-project__tip", tip))
    } else {
        // No GLYPH, but the chevron's BOX stays. Dropping the element moved
        // this header's name 20px left of every other header in the panel,
        // which is one of the two alignment faults that were visible on
        // screen. The empty label holds the column from the same rule the real
        // chevron uses, so the two cannot drift.
        Node::row("rg-project__header rg-project__header--static")
            .with(Node::label("rg-project__chevron", ""))
            .with(Node::label("rg-project__name", name.clone()).growing().eliding())
    };

    let mut sessions = Node::column("rg-project__sessions");
    let count = group.len();
    if count == 0 {
        // An empty bucket is kept on purpose: a folder just made in Settings
        // is exactly the one about to be filed into, and hiding it makes it
        // unreachable. A header over a void does not say that, so this does,
        // and it names the gesture that fills it.
        sessions = sessions.with(
            Node::row("rg-project__empty")
                .with(Node::label("rg-empty__hint", empty_bucket_hint(key))),
        );
    } else if group.bands.active.is_empty() && group.bands.hidden.is_empty() {
        sessions = sessions.with(
            Node::row("rg-project__empty")
                .with(Node::label("rg-empty__hint", "Nothing in the inbox here")),
        );
    }

    // The inbox shelf. Always emitted so the "show all" affordance sits inside
    // the band it expands, and always OPEN: `section_open` returns true for
    // Active unconditionally.
    let shelved = !group.bands.snoozed.is_empty() || !group.bands.settled.is_empty();
    let (head, hint) = inbox::section_head(Section::Active);
    let mut active = Node::column(&section_class(Section::Active, true));
    if shelved {
        // A CAPTION, and deliberately not a button. It shipped as a button
        // carrying a chevron, an expanded state and a handler into
        // `toggle_section`, and it could never do anything:
        // `WindowState::toggle_section` returns early for Active and
        // `section_open` hardcodes true for it. It announced itself as
        // expandable and was not. Its real job is to mark the boundary between
        // the inbox and the shelves under it, which is why it appears only
        // when there is a shelf to be told apart from.
        active = active.with(
            band_head(head, hint, group.bands.active.len())
                .into_static("rg-project__section-head rg-project__section-head--static"),
        );
    }
    for row in &group.bands.active {
        rows.push(fold_row(
            row,
            Section::Active,
            st,
            at,
            fields,
            home,
            &row_root,
        ));
        active = active.with(Node::new(Kind::Seat(row.id()), ""));
    }
    if !group.bands.hidden.is_empty() {
        active = active.with(more(
            Act::TogglePreview(key),
            "Show all",
            group.bands.hidden.len(),
            "Show the rest of this project's inbox",
        ));
    }
    sessions = sessions.with(active);

    for section in [Section::Snoozed, Section::Settled] {
        let band = group.section(section);
        if band.is_empty() {
            continue;
        }
        let open = st.section_open(key, section);
        let (head, hint) = inbox::section_head(section);
        let (shown, deeper) = st.band_cut(key, section, band.len());
        // The head lives inside the wrapper: `--collapsed` hides the rows and
        // never the head, because the head is how they come back.
        let mut shelf = Node::column(&section_class(section, open)).with(
            band_head(head, hint, band.len()).into_press(Act::ToggleSection(key, section)),
        );
        if open {
            for row in band.iter().take(shown) {
                rows.push(fold_row(row, section, st, at, fields, home, &row_root));
                shelf = shelf.with(Node::new(Kind::Seat(row.id()), ""));
            }
            // Inside the band it expands, like the inbox's own "Show all", and
            // it always carries the number so nothing is hidden without saying
            // how much.
            if deeper > 0 {
                shelf = shelf.with(more(
                    Act::ToggleSettledTail(key),
                    "Show more",
                    deeper,
                    "Show the rest of this project's finished sessions",
                ));
            }
        }
        sessions = sessions.with(shelf);
    }

    Node::column(&class).with(header).with(sessions)
}

/// A band's caption, before it is made a button or left inert.
struct Head(Node);

impl Head {
    /// The caption as a control that collapses its band.
    fn into_press(self, act: Act) -> Node {
        let Head(node) = self;
        Node {
            kind: Kind::Press(act),
            ..node
        }
    }

    /// The caption as an inert caption, keeping the chevron's box.
    ///
    /// The Snoozed and Settled captions below carry a real chevron, so without
    /// the box this caption's label starts 20px left of the other two in the
    /// same bucket.
    fn into_static(self, class: &str) -> Node {
        let Head(mut node) = self;
        node.class = class.to_string();
        node.children[0] = Node::label("rg-project__chevron", "");
        node
    }
}

fn band_head(head: &str, hint: &str, count: usize) -> Head {
    Head(
        Node::row("rg-project__section-head")
            .with(Node::label("rg-project__chevron", CHEVRON))
            .with(Node::label("rg-project__section-label", head))
            .with(Node::new(Kind::Rule, "rg-project__section-rule").growing())
            .with(Node::label(
                "rg-project__section-count",
                count.to_string(),
            ))
            .with(Node::label("rg-project__tip", hint)),
    )
}

/// A "show the rest" control, which always carries the number it is holding
/// back.
fn more(act: Act, word: &str, count: usize, hint: &str) -> Node {
    Node::press("rg-project__more", act)
        .saying(word)
        .with(Node::label(
            "rg-project__section-count",
            count.to_string(),
        ))
        .with(Node::label("rg-project__tip", hint))
}

/// The per-state counts a collapsed bucket's header carries.
///
/// The most urgent state leads and the zeroes are dropped. A header that
/// always showed five numbers would spend four of them on nothing, on the row
/// with the least horizontal room in the whole panel.
///
/// A DOT per chip, not a glyph. These are up to five chips on the narrowest
/// row in the panel, and the five status glyphs spanned 6.2x in ink width, so
/// a run of them read as a ragged line of unrelated marks rather than as one
/// scale.
fn rollup_node(rollup: &vitrum_model::ProjectRollup) -> Node {
    let mut node = Node::row("rg-rollup");
    for (status, count) in inbox::rollup_chips(rollup) {
        node = node.with(chip(inbox::status_modifier(status), count));
    }
    if rollup.woke > 0 {
        node = node.with(chip("rg-rollup__chip--woke", rollup.woke));
    }
    if rollup.snoozed > 0 {
        node = node.with(chip("rg-rollup__chip--snoozed", rollup.snoozed));
    }
    node
}

fn chip(modifier: &str, count: usize) -> Node {
    Node::row(&format!("rg-rollup__chip {modifier}"))
        .with(Node::new(Kind::Dot, "rg-rollup__dot"))
        .with(Node::label("", count.to_string()))
}

// ───────────────────────────────────────────────────────────────────────────
// One row
// ───────────────────────────────────────────────────────────────────────────

/// Class list for a session row.
///
/// The stylesheet keys every visual state off this string and a typo in one
/// modifier silently drops that state with no error anywhere.
///
/// `picked` is deliberately not the same thing as `active`: `--active` is the
/// row whose pty is in the pane, and `--picked` is membership of a
/// multi-selection. A row can be either, both, or neither.
///
/// The attention tier is deliberately NOT here, which is the one difference
/// from the markup this replaces. It is a rail ELEMENT with its own
/// modifiers, because the row's own box already carries five states and the
/// rail is the one that has to stay readable while the row is receding.
/// [`RowState::attention`] is therefore ignored, and [`rail_class`] is what
/// reads the tier.
fn row_class(s: RowState) -> String {
    let mut class = String::from("rg-session ");
    class.push_str(row_variant(s.section, s.always_slim));
    if recedes(s) {
        class.push_str(" rg-session--recede");
    }
    if in_flight(s) {
        class.push_str(" rg-session--inflight");
    }
    if s.unread {
        class.push_str(" rg-session--unread");
    }
    if s.woke {
        class.push_str(" rg-session--woke");
    }
    if s.picked {
        class.push_str(" rg-session--picked");
    }
    if s.active {
        class.push_str(" rg-session--active");
    }
    class
}

/// The rail tier a row wears, or `None`.
///
/// [`attention_modifier`] owns the priority ladder and this owns nothing but
/// the translation: the web sheet named the tiers on the ROW and the generated
/// sheet paints them on the rail, which is a different element. Exactly one
/// tier is ever returned, so two rails can never contend.
///
/// A tier with no entry here is a rail with no hue, which is why the table is
/// exhaustive rather than defaulted: see
/// [`super::tests::every_attention_tier_has_a_rail`].
pub(crate) fn rail_class(a: &Attention) -> Option<&'static str> {
    match attention_modifier(a)? {
        "rg-session--attention-failed" => Some("rg-session__rail--failed"),
        "rg-session--attention-waiting" => Some("rg-session__rail--waiting"),
        "rg-session--attention-bell" => Some("rg-session__rail--bell"),
        "rg-session--attention-idle" => Some("rg-session__rail--idle"),
        _ => None,
    }
}

/// Fold one row.
fn fold_row(
    row: &SessionView,
    section: Section,
    st: &UiState,
    at: crate::Tick,
    fields: RowFields,
    home: &Rc<str>,
    root: &Rc<str>,
) -> RowFold {
    let id = row.id();
    let info = &row.info;
    // The row does not know the window's policy and must not invent one: a row
    // rendering under different auto-settle rules from the bucket it sits in
    // would show a Snoozed badge inside the Done band.
    let policy = st.daemon.policy();
    // The coarsest clock this row cannot tell apart from now. A row reading
    // `5h ago` repeats that answer 3600 times before it changes, and rebuilding
    // it on every second boundary buys one changed character an hour.
    let clock = row_clock(at.fmt, row);
    let model = inbox::model_clock(clock);

    // One status resolution per row per paint. `Pill::of` already ran it, and
    // `SessionView::status()` would run it a second time for the same answer.
    let pill = Pill::of(row);
    let completion = completion_shown(pill.status, inbox::completion_badge(row));
    let woke = row.disposition(model, policy) == Disposition::Woke;
    let card = draws_card(section, fields.always_slim);
    let contested = st.daemon.collisions.for_session(id);

    // Both of these allocate and each is drawn by exactly one of the two row
    // shapes. A snoozed row is the case that mattered: `disposition_badge`
    // built a class, a countdown and a "Parked until ..." sentence for every
    // slim row on every paint, and the slim shape never reads it.
    let disposition = card
        .then(|| inbox::disposition_badge(row, model, policy))
        .flatten();
    let parked = (!card)
        .then(|| inbox::parked_label(row, model, policy))
        .flatten();
    // How long the turn running right now has been running, or `None`. A
    // different question from the row's timestamp, which is when the agent
    // last SPOKE: an agent silently computing for an hour has a fresh
    // timestamp and is the row worth finding.
    let aux = card.then(|| inbox::working_aux(row, model)).flatten();

    let class = row_class(RowState {
        section,
        status: pill.status,
        active: st.window.focused == Some(id),
        picked: st.window.selection.contains(id),
        unread: info.unread,
        woke,
        finished_unseen: row.has_unseen_completion(),
        attention: attention_modifier(&row.info.attention),
        always_slim: fields.always_slim,
    });

    // Always emitted, empty when the server has not resolved a branch or the
    // operator switched branches off. It is the flexible box on the tail line,
    // so dropping the element would let the timestamp and the badges slide
    // left into the middle of the row on half the rows.
    let branch = if fields.branch {
        info.git_branch.as_deref().unwrap_or_default()
    } else {
        ""
    };
    // The linked worktree this session is in, or empty for a main working
    // tree. Resolved by the daemon, because only the daemon has the session's
    // filesystem: a linked worktree's `.git` is a FILE pointing into
    // `.git/worktrees/<name>`, and that name is what arrives here. It is git's
    // own name and never a path, so the row cannot leak a machine path the way
    // a directory can.
    let worktree = if fields.worktree {
        crate::ui::terminal::worktree_of(row).unwrap_or_default()
    } else {
        String::new()
    };
    // The working directory, drawn wherever it says something the bucket
    // header above does not, and on a root row where nothing else on the line
    // says it either, where the alternative is a blank line.
    let place = if fields.place {
        place_label(
            &info.cwd,
            root,
            home,
            !branch.is_empty() || !worktree.is_empty(),
        )
    } else {
        String::new()
    };
    let time = if fields.time {
        age(clock, info.last_activity_ms)
    } else {
        String::new()
    };

    let mut tip = row_tooltip(row, home, &pill);
    if let Some((files, peers)) = contested {
        tip.push('\n');
        tip.push_str(&contest_title(files, peers));
    }
    for badge in [disposition.as_ref(), completion.as_ref(), parked.as_ref()]
        .into_iter()
        .flatten()
    {
        tip.push('\n');
        tip.push_str(&badge.title);
    }
    if attention_modifier(&info.attention).is_some() {
        // The rail is a hue and nothing else, so the sentence that says which
        // tier it is has to live in the detail beside it.
        tip.push('\n');
        tip.push_str(&attention_label(&info.attention));
    }

    // A generated title is the command name, which is the same word on every
    // row a shell runs in: 60 real sessions produced 57 rows reading the same
    // four letters. `row_title` appends the session id to those and leaves a
    // name the operator chose exactly as they typed it.
    let title = inbox::row_title(info).into_owned();
    // Which agent is behind this session. Fixed width, so it sits BEFORE the
    // title without making a title's left edge depend on anything variable.
    // `AgentKind::of` never guesses: an unrecognised command draws the unknown
    // mark, not the nearest agent's.
    let agent = AgentKind::of(&info.command);
    let mark = Node::new(
        Kind::Mark(agent.mark()),
        &format!("rg-session__agent {}", agent_class(&info.status)),
    )
    .named(agent.label());

    let lines = if card {
        // TWO lines, both unconditional, so every card in a band is exactly
        // the same height. See the module note for what a third one cost.
        Node::column("")
            .with(
                Node::row("rg-session__line rg-session__line--title")
                    .with(mark)
                    .with(Node::label("rg-session__title", title).growing().eliding())
                    .with(Node::new(Kind::Stack, "rg-session__slot").with(pill_node(
                        &pill,
                        fields.status_word,
                        aux,
                    ))),
            )
            .with(
                Node::row("rg-session__line rg-session__line--tail")
                    // The contest leads the tail line, at the row's left edge.
                    // Line one is full at the 224px floor and its budget is
                    // spent on the mark, the title and the status, so a
                    // conditional element there would move a title's left edge
                    // on the few rows that have one. It sits BEFORE the branch,
                    // which is fixed-width and so does not disturb the branch's
                    // job as the spacer for everything after it: put after the
                    // branch it was pushed to the far right, where on a row
                    // with no branch it floated alone against the timestamp
                    // with the whole left half of the line empty.
                    .maybe(contested.map(|(files, _)| {
                        Node::row("rg-session__contest")
                            .with(Node::new(Kind::Dot, "rg-session__contest-mark"))
                            .with(Node::label(
                                "rg-session__contest-count",
                                files.to_string(),
                            ))
                    }))
                    .with(Node::reserved("rg-session__place", place).eliding())
                    .with(Node::reserved("rg-session__worktree", worktree).eliding())
                    .with(Node::reserved("rg-session__branch", branch).growing().eliding())
                    .maybe(disposition.map(badge_node))
                    .maybe(completion.map(badge_node))
                    .with(
                        Node::new(Kind::Stack, "rg-session__slot")
                            .with(Node::reserved("rg-session__time", time).wide(SLOT_CHARS))
                            .with(close_actions(id)),
                    ),
            )
    } else {
        // The slim row. One line, the title at the same left edge as a card's
        // title, so the tail scans as a continuation of the list rather than a
        // new one.
        Node::column("").with(
            Node::row("rg-session__line")
                .with(mark)
                .with(Node::label("rg-session__title", title).growing().eliding())
                .maybe(completion.map(badge_node))
                .with(
                    Node::new(Kind::Stack, "rg-session__slot")
                        .with(match parked {
                            Some(ticket) => Node::row(&ticket.class)
                                .with(Node::reserved("rg-pill__word", ticket.text).wide(SLOT_CHARS)),
                            None => Node::reserved("rg-session__time", time).wide(SLOT_CHARS),
                        })
                        .with(close_actions(id)),
                ),
        )
    };

    // The rail is present on every row and transparent until a tier lights it.
    // It is the row's leftmost element, so a row that starts wanting the
    // operator gains a colour and not a column.
    let rail = match rail_class(&info.attention) {
        Some(tier) => format!("rg-session__rail {tier}"),
        None => "rg-session__rail".to_string(),
    };

    let node = Node::press(&class, Act::Select(id)).with(
        Node::new(Kind::Over, "")
            .with(Node::row("").with(Node::label(&rail, "")).with(lines))
            // THE ROW'S HOVER DETAIL, DRAWN BY US. See `Kind::Over` for why
            // asking the platform for one strands an opaque rectangle over
            // rows it no longer describes every time the list reorders.
            .with(Node::label("rg-session__tip", tip).eliding()),
    );

    RowFold { id, node }
}

/// The status pill: its hue, its word and its live counter.
///
/// The word off leaves the pill's box and its hue, which is exactly what the
/// collapsed rail already draws. The accessible name still carries the word,
/// so the state is never lost to a screen reader, only to the column.
fn pill_node(pill: &Pill, word: bool, aux: Option<String>) -> Node {
    Node::row(&pill.class)
        .centred()
        .named(pill.word)
        .with(if word {
            Node::reserved("rg-pill__word", pill.word).wide(inbox::state_word_chars())
        } else {
            // No reservation with the word off: the box is empty on purpose,
            // and holding eight transparent characters there would spend the
            // width the narrow layout exists to give back.
            Node::reserved("rg-pill__word", "")
        })
        .maybe(aux.map(|aux| Node::label("rg-pill__aux", aux).wide(AUX_CHARS)))
}

fn badge_node(badge: inbox::Badge) -> Node {
    let mut node = Node::row(&badge.class);
    if let Some(icon) = badge.icon {
        node = node.with(Node::label("rg-badge__icon", icon));
    }
    node.with(Node::label("", badge.text))
}

/// The hover group, stacked on the timestamp in the slot's one cell.
///
/// A wrapper and not a bare button because the cell cross-fades one GROUP
/// against the time, and because the row's lifecycle actions belong beside the
/// close rather than in a second cell that would widen the row.
fn close_actions(id: SessionId) -> Node {
    Node::row("rg-session__actions").with(
        Node::press("rg-session__close", Act::Close(id))
            .saying("\u{00d7}")
            .named("Terminate session"),
    )
}
