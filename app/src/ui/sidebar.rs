//! Project and session sidebar.
//!
//! Markup only. Every rule the markup expresses lives somewhere else: the
//! layout classes in `app/assets/sidebar.css`, and every decision about what a
//! row IS in `vitrum-model`, reached through [`crate::inbox`]. Nothing in this
//! file decides whether a session is done, urgent, parked or ordered.
//!
//! # Two row shapes, and where the split comes from
//!
//! A row is a **card** when [`SessionView::section`] puts it in
//! [`Section::Active`] and a **slim** row when it puts it in Snoozed or
//! Settled. Nothing else selects the shape: the operator's own parking is what
//! makes the list dense, not the sidebar second-guessing which of their live
//! sessions still matters.
//!
//! A card is three fixed-height lines — project, title, metadata — and a slim
//! row is one. Both end in `__slot`, a single grid cell that holds the status
//! label (card) or the timestamp (slim) stacked with the close button, which
//! is why nothing on the right of a row can collide at any width.
//!
//! # Five contracts worth stating because breaking them is silent
//!
//! - **Every title starts at the same x.** On a card the title owns its own
//!   line and is that line's first child; on a slim row the only thing before
//!   it is the fixed-width project mark. Nothing status-shaped may ever
//!   precede it. The status word varies from five characters to eight, so a
//!   status in front of the title gives twenty rows twenty different title
//!   positions, and that is what makes a list read as a table.
//! - **A row emits an attention rail or the unread dot, never both.** At the
//!   14rem width floor there is not room for two markers plus a slot.
//! - **Recede is the whole design.** A row that wants nothing from the
//!   operator gives up its weight and, if it is still in flight, its opacity;
//!   hover puts both back. Without it every row is equally bright and the
//!   operator has to read all twenty to find the one that needs them.
//! - **A project's rows are split into the model's three bands.** Snoozed and
//!   Done are collapsed by default and always show their count, so nothing is
//!   hidden without a number saying how much.
//! - **Nothing loops.** Hover, selection and disclosure use one-shot
//!   transitions under 150ms. A keyframe animation that repeats is a repaint
//!   per frame forever and costs the most exactly when the most rows are lit.

use std::borrow::Cow;
use std::rc::Rc;

use dioxus::prelude::*;
use vitrum_fmt::{TimeFormat, Timestamp};
use vitrum_model::{AgentKind, Disposition, Section, SessionView, SidebarStatus};
use vitrum_proto::{ProjectId, SessionId, SessionStatus};

use crate::agent::{AgentMark, AgentMarks};
use crate::clock::age;
use crate::inbox::{self, Pill};
use crate::state::{Click, ConnState, GroupKey, UiState, attention_label, attention_modifier};
use crate::ui::dialog;

mod render_count;

/// Disclosure chevron, U+25BE BLACK DOWN-POINTING SMALL TRIANGLE.
///
/// One glyph for both states. `sidebar.css` rotates it -90deg on
/// `.rg-project--collapsed` and `.rg-project__section--collapsed`, so the
/// markup must NOT swap in a right-pointing glyph as well or a collapsed
/// group points up.
const CHEVRON: &str = "\u{25be}";
/// Search glyph, U+2315 TELEPHONE RECORDER.
const SEARCH_ICON: &str = "\u{2315}";
/// Settings glyph, U+2699 GEAR.
const GEAR_ICON: &str = "\u{2699}";

/// Column budget for a path inside a row tooltip.
///
/// A native tooltip does not wrap, so an unbounded absolute path pushes the
/// tooltip wider than the window and clips the lines under it.
const TOOLTIP_PATH_COLUMNS: usize = 64;

/// Column budget for a bucket label that is a filesystem path.
///
/// Only directory and folder buckets hit it; a daemon project's label is
/// already a bare name. An unshortened absolute path in a 14rem column is all
/// prefix and no name, which is the one thing a header must not be.
const GROUP_LABEL_COLUMNS: usize = 28;

/// DOM id of one session row, so the keyboard can scroll it into view.
///
/// Traversal that moves focus to a row thirty rows below the fold, without
/// scrolling, looks exactly like traversal that did nothing.
pub fn row_id(id: SessionId) -> String {
    format!("rg-row-{}", id.0)
}

#[derive(Props, Clone, PartialEq)]
pub struct SidebarProps {
    pub state: Signal<UiState>,
    /// Wall clock at render time, so every row in one paint agrees on "now".
    pub clock: TimeFormat,
    /// This user's home directory, for shortening paths on screen.
    pub home: String,
    /// The daemon this window is pointed at, named in every connection state.
    pub server: &'static str,
    pub on_select: EventHandler<(SessionId, Click)>,
    pub on_close_session: EventHandler<SessionId>,
    /// Collapse or expand one bucket, keyed the way `WindowState` remembers
    /// it: a bucket may be a daemon project, a bare directory, an operator
    /// folder or the unfiled remainder, and only [`GroupKey`] spans all four.
    pub on_toggle_project: EventHandler<GroupKey>,
    pub on_toggle_section: EventHandler<(GroupKey, Section)>,
    pub on_toggle_preview: EventHandler<GroupKey>,
    /// Show or hide the deep end of one bucket's Done shelf.
    ///
    /// Separate from [`SidebarProps::on_toggle_preview`], which is the Active
    /// band's cut, because one operator can legitimately want every live row
    /// on screen and still not want three hundred drained ones.
    ///
    /// REQUIRED, and deliberately not an `Option`: the "Show more" button is
    /// only emitted when there is something behind it, so a handler that
    /// could be absent would put a live-looking control on screen that did
    /// nothing. That is the exact defect [`SidebarProps::on_settings`] used
    /// to have.
    pub on_toggle_settled_tail: EventHandler<GroupKey>,
    pub on_toggle_sidebar: EventHandler<()>,
    pub on_retry: EventHandler<()>,
    /// Move focus to the next session waiting on the operator. Same action as
    /// Ctrl+Shift+Down; the toolbar's count is its pointer-reachable half.
    pub on_jump: EventHandler<()>,
    /// Open the ranked launcher. The caret half of the footer's control, and
    /// the same action as Ctrl+Shift+N.
    pub on_new_session: EventHandler<Option<ProjectId>>,
    /// Start the top-ranked launch immediately, with no layer at all. The
    /// primary half of the footer's control, and the whole reason a new
    /// session is one click rather than two.
    pub on_launch_now: EventHandler<()>,
    pub on_filter: EventHandler<String>,
    pub on_menu: EventHandler<(f64, f64, SessionId)>,
    pub on_resize_start: EventHandler<f64>,
    pub on_resize_nudge: EventHandler<f64>,
    /// Open the settings surface.
    ///
    /// Not optional. It used to be, with the gear rendering `disabled` when
    /// nothing was passed, so that this file and the settings surface could
    /// land in either order during one merge. Both landed; `main.rs` has
    /// passed a handler ever since, so the `None` arm was a control that
    /// rendered itself dead and could never be reached. A landing-order
    /// scaffold that outlives the landing is indistinguishable from a stub.
    pub on_settings: EventHandler<()>,
    /// What the update path has to say, polled by `main.rs`.
    ///
    /// A [`Signal`] rather than a value because the poll that fills it runs
    /// every [`crate::update::STAGED_POLL`] for the life of the window, and a
    /// build staged by `vitrum update` in a terminal must reach the panel
    /// without the operator doing anything.
    pub update_standing: Signal<crate::update::Standing>,
    /// Restart into the staged build.
    ///
    /// Not an `Option`: the affordance is only emitted when there is a build
    /// to restart into, so a handler that could be absent would put a live
    /// control on screen that did nothing, which is the defect
    /// [`SidebarProps::on_settings`] documents above.
    pub on_restart: EventHandler<()>,
}

#[allow(non_snake_case)]
pub fn Sidebar(props: SidebarProps) -> Element {
    let state = props.state;
    // The word the footer's primary control wears, read off the UI thread and
    // only when the session list changes. That is the one moment a launch can
    // have moved the ranking, and the memo itself is a `len()`, so a daemon
    // pushing output twenty times a second costs nothing here. Reading the
    // profile on every paint would put a file read on the hottest surface in
    // the product.
    let launches = use_memo(move || state.read().daemon.sessions.len());
    let top = use_resource(move || {
        let _ = launches();
        async move { dialog::primary_word_now().await }
    });
    let st = props.state.read();
    let model_clock = inbox::model_clock(props.clock);
    let collapsed = st.window.sidebar_collapsed;
    let root_class = if collapsed {
        "rg-sidebar rg-sidebar--collapsed"
    } else {
        "rg-sidebar"
    };
    // ONE arrangement per paint. Every other derivation below is taken from
    // it rather than re-derived: arranging resolves a status and a disposition
    // for every row and sorts three bands, and at thirty sessions on a daemon
    // pushing an update a second, doing that three times per paint is most of
    // what this client would spend its CPU on.
    let groups = st.tree(model_clock);
    let query = st.window.filter.clone();
    // The rule lives in WindowState so the sidebar and the tests that assert
    // "a failed search is not an empty server" cannot drift. It takes the tree
    // we already built rather than building a second one.
    let no_matches = st.window.filter_matched_nothing_in(groups.is_empty());
    let connecting = matches!(st.daemon.conn, ConnState::Connecting);
    let failed = st.daemon.conn.is_retryable();
    let ready = st.server_ready();
    let waiting_on_you = st.attention_count_of(&groups, model_clock);
    // Read once per paint, not once per row: `Settings` is not `Copy`, and
    // twenty rows each reaching into it is twenty reads of the same three bits.
    let fields = RowFields::of(&st.daemon.settings);
    // One buffer for every row's tooltip. Cloning the `String` per row cost an
    // allocation and a copy per row per paint; a refcount bump costs neither.
    let home: Rc<str> = Rc::from(props.home.as_str());
    // THE ONE PLACE THE RESTART AFFORDANCE IS DECIDED.
    //
    // `restart_offer` answers `Some` for a STAGED build and only when the
    // operator has left the affordance switched on. Both halves live in
    // `update.rs` rather than in this expression, because the setting is a
    // contract and not a local `if`: it decides what is DRAWN and nothing
    // else. The check that finds an update, the download that stages it and
    // the swap that applies it on the next start never read it, so an
    // operator who switches the affordance off is still updated — they have
    // asked not to be told, not asked to stop.
    //
    // Resolved to an owned line here and not held as a borrow of the signal,
    // so the read guard is dropped before the markup and the `onclick`
    // closures below capture nothing from it.
    let restart_to: Option<String> = crate::update::restart_offer(
        &props.update_standing.read(),
        st.daemon.settings.show_restart_to_update,
    )
    .map(crate::update::restart_line);
    // Why there is nothing to draw, computed only when there IS nothing to
    // draw. It walks every session, and on every paint that has rows the
    // answer would be thrown away.
    let empty_words: Option<(String, String)> = groups.is_empty().then(|| {
        let policy = st.daemon.policy();
        let ws = st.daemon.workspaces.get(st.window.workspace);
        let mut in_workspace = 0;
        let mut admitted = 0;
        for row in &st.daemon.sessions {
            if st.daemon.workspaces.workspace_of(&row.info) != st.window.workspace {
                continue;
            }
            in_workspace += 1;
            if ws.is_some_and(|w| w.sections.shows(row.disposition(model_clock, policy))) {
                admitted += 1;
            }
        }
        let census = inbox::Census {
            total: st.daemon.sessions.len(),
            in_workspace,
            admitted,
        };
        inbox::Empty::of(census).words(ws.map_or("This workspace", |w| w.name.as_str()))
    });

    // The footer's primary control, resolved once per paint.
    //
    // `word_now` is `None` until the profile read lands and stays `None` when
    // this operator has launched nothing and saved nothing. That is not a
    // loading state to hide: it is the honest one, and the control reads "New
    // session" and opens the list rather than guessing an agent off PATH.
    let word_now: Option<String> = top.read().clone().flatten();
    let can_launch = word_now.is_some();
    let pick_shown = can_launch && !collapsed;
    let go_text = dialog::go_label(word_now.as_deref(), !collapsed);
    let go_aria = match &word_now {
        Some(w) => format!("Start {w}"),
        None => "New session".to_string(),
    };
    // The place is in the tooltip and not in the label, because the project
    // headers directly below this control already say where the operator is.
    let go_hint = {
        let cwd = st
            .window
            .focused
            .and_then(|f| st.session(f))
            .map(|s| s.cwd.clone())
            .or_else(|| st.daemon.projects.first().map(|p| p.root.clone()))
            .unwrap_or_default();
        let place = if cwd.is_empty() {
            String::new()
        } else {
            dialog::place_of(&st.daemon.projects, &cwd, &props.home)
        };
        dialog::go_tip(word_now.as_deref(), &place)
    };

    rsx! {
        aside {
            class: "{root_class}",
            // ALWAYS emit the width, whether the operator chose it or it was
            // derived from the window.
            //
            // This used to be gated on `width_pinned`, leaving the stylesheet's
            // `22vw` to answer for an underived width. That answer is WRONG on
            // this stack: measured on the real WM-managed display, a 1920px
            // window rendered the panel at exactly 224px, which is its 14rem
            // legibility floor, and 224 / 0.22 = 1018.18. Viewport units here
            // resolve against a stale ~1018px initial containing block that
            // never recomputes once the window is really sized, so `22vw` was
            // quietly returning 224 and the floor was doing all the work. The
            // panel shipped at 53% of its designed width, and "the sidebar
            // feels cramped" was that, not a token.
            //
            // `width_pinned` keeps its real job, which is PERSISTENCE: only a
            // width the operator dragged is written back to the profile. That
            // distinction matters and is load-bearing (see the note at
            // main.rs's `fresh_geometry`): writing a computed default into the
            // persisted slot makes the app read its own guess back as if it
            // were a preference, which pinned every window to 282px for a day.
            // Rendering a derived width and SAVING a chosen one are different
            // questions, and conflating them cost both bugs.
            style: format!("--rg-sidebar-width: {}px", st.window.sidebar_width),

            // ONE row of chrome, where there used to be four.
            //
            // The header ("vitrum" again, 44px), the search field, the
            // permanent connection banner and the "Projects" caption with its
            // lone "+" were four stacked bands carrying, between them, one
            // text field and two buttons. They are this. The wordmark is in
            // the titlebar, which is the only place a window should say its
            // own name; the connection state is a dot up there too, because a
            // full-width green strip announcing a working socket is a band
            // spent on a non-event; and "Projects" was a caption over a list
            // of projects.
            div { class: "rg-sidebar__toolbar",
                if collapsed {
                    // At 3rem there is no room for a field. The magnifier
                    // expands the panel, which is where the field is.
                    button {
                        class: "rg-sidebar__action",
                        r#type: "button",
                        title: "Expand sidebar (Ctrl+Shift+B)",
                        "aria-label": "Expand sidebar",
                        onclick: move |_| props.on_toggle_sidebar.call(()),
                        "{SEARCH_ICON}"
                    }
                } else {
                    div { class: "rg-sidebar__search",
                        span { class: "rg-sidebar__search-icon", "{SEARCH_ICON}" }
                        input {
                            class: "rg-sidebar__search-input",
                            id: "rg-filter",
                            r#type: "text",
                            // One word. "Filter sessions" is clipped mid-glyph
                            // by the Ctrl+K keycap at the panel's narrower
                            // widths, and a placeholder ending in a sliced "n"
                            // reads as a rendering fault rather than as an
                            // ellipsis. This fits at every width the panel has.
                            placeholder: "Filter",
                            value: "{query}",
                            oninput: move |e| props.on_filter.call(e.value()),
                            onkeydown: move |e| {
                                if e.key() == Key::Escape {
                                    props.on_filter.call(String::new());
                                    e.prevent_default();
                                }
                            },
                        }
                        // Always emitted: the stylesheet hides the chip itself
                        // once the field has focus, so conditionally rendering
                        // it here would fight the CSS and flicker the field's
                        // width.
                        span { class: "rg-sidebar__search-kbd", "Ctrl+K" }
                    }
                    // The jump key's own affordance, and it says what it means
                    // now. It used to read "3 -> you", which nobody could
                    // decode from the screen. Zero draws nothing at all: the
                    // honest answer to "how many are waiting" when none are is
                    // silence, not a grey nought.
                    if waiting_on_you > 0 {
                        button {
                            class: "rg-attn-count",
                            r#type: "button",
                            title: "Jump to the next session waiting on you (Ctrl+Shift+Down)",
                            onclick: move |_| props.on_jump.call(()),
                            span { class: "rg-attn-count__n", "{waiting_on_you}" }
                            span { class: "rg-attn-count__word", "waiting" }
                        }
                    }
                    // The `+` that used to sit here is gone, and its function
                    // moved DOWN rather than away: the footer's control starts
                    // a session on the first click and names the agent it will
                    // start. It could not stay in this band, because a label
                    // does not fit beside a 116px filter field and a 36px
                    // attention chip at the 224px floor, and a control that
                    // launches on one click cannot be a bare glyph.
                }
            }

            // Only when something is wrong. A socket that is up says so with a
            // dot in the titlebar and takes no vertical space here.
            if connecting || failed {
                div {
                    class: "{st.daemon.conn.banner_class()}",
                    title: "{st.daemon.conn.banner_text(&props.server)}",
                    span { class: "rg-sidebar__status-text", "{st.daemon.conn.banner_text(&props.server)}" }
                    if failed {
                        button {
                            class: "rg-btn-inline",
                            r#type: "button",
                            onclick: move |_| props.on_retry.call(()),
                            "Retry"
                        }
                    }
                }
            }

            div { class: "rg-sidebar__body", id: "rg-sidebar-body",
                if no_matches {
                    div { class: "rg-sidebar__empty rg-sidebar__empty--no-matches",
                        span { class: "rg-empty__title", "Nothing matches \u{201c}{query}\u{201d}" }
                        span { class: "rg-empty__hint",
                            "Titles, commands, directories and branches are all searched."
                        }
                        button {
                            class: "rg-btn",
                            r#type: "button",
                            onclick: move |_| props.on_filter.call(String::new()),
                            "Clear filter"
                        }
                    }
                } else if groups.is_empty() {
                    if connecting {
                        div { class: "rg-sidebar__empty",
                            span { class: "rg-empty__title", "Connecting" }
                            span { class: "rg-empty__hint", "Waiting for {props.server} to answer." }
                        }
                    } else if failed {
                        div { class: "rg-sidebar__empty",
                            span { class: "rg-empty__title", "Not connected" }
                            span { class: "rg-empty__hint",
                                "The session daemon at {props.server} is not answering. Sessions keep running while this window is disconnected."
                            }
                            button {
                                class: "rg-btn rg-btn--primary",
                                r#type: "button",
                                onclick: move |_| props.on_retry.call(()),
                                "Retry"
                            }
                        }
                    } else if let Some((title, hint)) = empty_words.clone() {
                        // THREE empty states, not one. This used to answer
                        // every empty tree with "Projects appear here as soon
                        // as a session runs in one", which is true only when
                        // the daemon really is empty and is a flat lie in the
                        // two cases that matter: a second workspace, which is
                        // blank BECAUSE that is what a workspace is, and a
                        // workspace whose bands the operator switched off in
                        // Settings. Both read as "my sessions are gone", so
                        // two features that work end to end were reported as
                        // missing on the strength of one string. The rule is
                        // in `inbox::Empty` so the words are testable and
                        // cannot drift back into being one sentence.
                        //
                        // The title is empty for the honest first-run state:
                        // the terminal pane carries that one, and two
                        // surfaces shouting the same thing is worse than one.
                        div { class: "rg-sidebar__empty",
                            if !title.is_empty() {
                                span { class: "rg-empty__title", "{title}" }
                            }
                            span { class: "rg-empty__hint", "{hint}" }
                        }
                    }
                }
                for group in groups.iter() {
                    {
                        let key = group.key;
                        // A folder or directory bucket's label is a path, and
                        // an absolute path in a 14rem column is all prefix and
                        // no name. A daemon project's label is already a bare
                        // name and is borrowed rather than cloned: only the
                        // path case has to build a string.
                        let name: Cow<'_, str> = if group.label.starts_with('/') {
                            Cow::Owned(vitrum_fmt::path::shorten_home_relative(
                                &group.label,
                                &props.home,
                                GROUP_LABEL_COLUMNS,
                            ))
                        } else {
                            Cow::Borrowed(group.label.as_str())
                        };
                        let root: &str = group.root.as_deref().unwrap_or_default();
                        // The Unfiled bucket has no name to look for its rows
                        // under, so it cannot be collapsed: doing so would hide
                        // sessions behind a header that does not say what is in
                        // it.
                        let is_collapsed =
                            group.collapsible() && st.window.collapsed.contains(&key);
                        let class = match (group.current, is_collapsed) {
                            (true, true) => "rg-project rg-project--current rg-project--collapsed",
                            (true, false) => "rg-project rg-project--current",
                            (false, true) => "rg-project rg-project--collapsed",
                            (false, false) => "rg-project",
                        };
                        let count = group.len();
                        // The rollup answers "should I open this" and only a
                        // collapsed header has that question. An expanded
                        // group answers it with its own rows.
                        let rollup = is_collapsed.then(|| group.bands.rollup.clone()).flatten();
                        // NEVER empty. A folder or Unfiled bucket has no
                        // filesystem root, so this fell through to `root`,
                        // which is `None` for both, and every folder header
                        // in named grouping shipped a literal `title=""`.
                        let header_title = {
                            let mut text =
                                String::from(if root.is_empty() { name.as_ref() } else { root });
                            if let Some(r) = rollup.as_ref() {
                                text.push('\n');
                                text.push_str(&inbox::rollup_title(r));
                            }
                            text
                        };
                        // The Active caption earns a line only when a shelf
                        // sits under it to be told apart from. On a bucket
                        // with nothing parked and nothing drained it would
                        // print the count the header already shows, one row
                        // above it.
                        let shelved =
                            !group.bands.snoozed.is_empty() || !group.bands.settled.is_empty();
                        let (active_head, active_hint) = inbox::section_head(Section::Active);
                        rsx! {
                            div { class: "{class}", key: "{key:?}",
                                if group.collapsible() {
                                    button {
                                        class: "rg-project__header",
                                        r#type: "button",
                                        title: "{header_title}",
                                        "aria-expanded": if is_collapsed { "false" } else { "true" },
                                        "aria-current": if group.current { "true" } else { "false" },
                                        onclick: move |_| props.on_toggle_project.call(key),
                                        span { class: "rg-project__chevron", "{CHEVRON}" }
                                        span { class: "rg-project__name", "{name}" }
                                        if let Some(rollup) = rollup {
                                            ProjectRollupChips { rollup }
                                        }
                                    }
                                } else {
                                    // The Unfiled bucket, which cannot
                                    // collapse: there is no name to look for
                                    // its rows under, so hiding them behind
                                    // this header would lose them.
                                    //
                                    // No GLYPH, but the chevron's BOX stays.
                                    // Dropping the element moved this header's
                                    // name 20px left of every other header in
                                    // the panel — the 12px slot plus the 8px
                                    // flex gap — which is one of the two
                                    // alignment faults visible on screen. The
                                    // empty span holds the column from the
                                    // same rule the real chevron uses, so the
                                    // two cannot drift; a padding override on
                                    // `--static` would be a second source of
                                    // truth for one measurement.
                                    div { class: "rg-project__header rg-project__header--static",
                                        span { class: "rg-project__chevron" }
                                        span { class: "rg-project__name", "{name}" }
                                    }
                                }
                                div { class: "rg-project__sessions",
                                    if count == 0 {
                                        // An empty bucket is kept on purpose:
                                        // a folder you just made in Settings
                                        // is exactly the one you are about to
                                        // file into, and hiding it makes it
                                        // unreachable. A header over a void
                                        // does not say that, so this does,
                                        // and it names the gesture that fills
                                        // it rather than leaving the operator
                                        // to guess.
                                        div { class: "rg-project__empty",
                                            span { class: "rg-empty__hint", "{empty_bucket_hint(key)}" }
                                        }
                                    } else if group.bands.active.is_empty()
                                        && group.bands.hidden.is_empty()
                                    {
                                        div { class: "rg-project__empty",
                                            span { class: "rg-empty__hint", "Nothing in the inbox here" }
                                        }
                                    }
                                    // The inbox shelf. Always emitted so the
                                    // "show all" affordance sits inside the
                                    // band it expands, and so the row gap
                                    // between bands comes from one rule.
                                    //
                                    // Always OPEN, too, which is why `true` is
                                    // passed literally: `section_open` returns
                                    // true for Active unconditionally, so
                                    // `--collapsed` can never appear here.
                                    div { class: "{section_class(Section::Active, true)}",
                                        if shelved {
                                            // A CAPTION, and deliberately not
                                            // a button. This was a button
                                            // carrying a chevron, an
                                            // `aria-expanded` and an onclick
                                            // into `on_toggle_section`, and
                                            // it could never do anything:
                                            // `WindowState::toggle_section`
                                            // returns early for Active and
                                            // `section_open` hardcodes true
                                            // for it. It announced itself to
                                            // a screen reader as expandable
                                            // and was not. Its real job is to
                                            // mark the boundary between the
                                            // inbox and the shelves under it,
                                            // which is why it appears only
                                            // when there is a shelf to be
                                            // told apart from.
                                            //
                                            // The chevron span is EMPTY and
                                            // still there. The Snoozed and
                                            // Settled captions below carry a
                                            // real one, so without the box
                                            // this caption's label starts
                                            // 20px left of the other two in
                                            // the same bucket.
                                            div {
                                                class: "rg-project__section-head rg-project__section-head--static",
                                                title: "{active_hint}",
                                                span { class: "rg-project__chevron" }
                                                span { class: "rg-project__section-label", "{active_head}" }
                                                span { class: "rg-project__section-rule" }
                                                span { class: "rg-project__section-count", "{group.bands.active.len()}" }
                                            }
                                        }
                                        for s in group.bands.active.iter() {
                                            SessionRow {
                                                key: "{s.id().0}",
                                                row: (*s).clone(),
                                                section: Section::Active,
                                                fields,
                                                active: st.window.focused == Some(s.id()),
                                                picked: st.window.selection.contains(s.id()),
                                                clock: row_clock(props.clock, s),
                                                home: Rc::clone(&home),
                                                contested: st.daemon.collisions.for_session(s.id()),
                                                on_select: props.on_select,
                                                on_close: props.on_close_session,
                                                on_menu: props.on_menu,
                                            }
                                        }
                                        if !group.bands.hidden.is_empty() {
                                            button {
                                                class: "rg-project__more",
                                                r#type: "button",
                                                title: "Show the rest of this project's inbox",
                                                onclick: move |_| props.on_toggle_preview.call(key),
                                                "Show all"
                                                span { class: "rg-project__section-count", "{group.bands.hidden.len()}" }
                                            }
                                        }
                                    }
                                    for section in [Section::Snoozed, Section::Settled] {
                                        {
                                            let rows = group.section(section);
                                            let open = st.section_open(key, section);
                                            let (head, hint) = inbox::section_head(section);
                                            let (shown, deeper) =
                                                band_cut(section, rows.len(), st.window.settled_expanded(key));
                                            rsx! {
                                                if !rows.is_empty() {
                                                    // The head lives inside the
                                                    // wrapper: `--collapsed` hides
                                                    // the rows and never the head,
                                                    // because the head is how they
                                                    // come back.
                                                    div {
                                                        key: "{head}",
                                                        class: "{section_class(section, open)}",
                                                        button {
                                                            class: "rg-project__section-head",
                                                            r#type: "button",
                                                            title: "{hint}",
                                                            "aria-expanded": if open { "true" } else { "false" },
                                                            onclick: move |_| props.on_toggle_section.call((key, section)),
                                                            span { class: "rg-project__chevron", "{CHEVRON}" }
                                                            span { class: "rg-project__section-label", "{head}" }
                                                            span { class: "rg-project__section-rule" }
                                                            span { class: "rg-project__section-count", "{rows.len()}" }
                                                        }
                                                        if open {
                                                            for s in rows.iter().take(shown) {
                                                                SessionRow {
                                                                    key: "{s.id().0}",
                                                                    row: (*s).clone(),
                                                                    section,
                                                                    fields,
                                                                    active: st.window.focused == Some(s.id()),
                                                                    picked: st.window.selection.contains(s.id()),
                                                                    clock: row_clock(props.clock, s),
                                                                    home: Rc::clone(&home),
                                                                    contested: st.daemon.collisions.for_session(s.id()),
                                                                    on_select: props.on_select,
                                                                    on_close: props.on_close_session,
                                                                    on_menu: props.on_menu,
                                                                }
                                                            }
                                                            // Inside the band it
                                                            // expands, like the
                                                            // inbox's own "Show
                                                            // all", and it always
                                                            // carries the number
                                                            // so nothing is hidden
                                                            // without saying how
                                                            // much.
                                                            if deeper > 0 {
                                                                button {
                                                                    class: "rg-project__more",
                                                                    r#type: "button",
                                                                    title: "Show the rest of this project's finished sessions",
                                                                    onclick: move |_| props.on_toggle_settled_tail.call(key),
                                                                    "Show more"
                                                                    span { class: "rg-project__section-count", "{deeper}" }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // WHAT THE BOTTOM OF THE PANEL IS FOR.
                //
                // Measured on a real workspace: three sessions in two
                // projects fill 190px of a 764px scroller, so 75% of the
                // widest column in the window is nothing at all. That is not
                // a rounding error in a layout, it is the panel telling an
                // operator with three agents that they are using a tool built
                // for a list they do not have.
                //
                // Two ways to answer it. Growing the rows to fill is the
                // wrong one: a row's height is a legibility decision taken
                // against the two lines it holds, and stretching it to 200px
                // to absorb a void produces a list whose row height depends
                // on how many sessions you happen to be running. So the
                // region carries something instead, and the only honest thing
                // for it to carry is what goes there: sessions. It is drawn
                // as the place a session is started, and it starts one.
                //
                // It is the LAST child of the scroller and not a floating
                // panel, so it needs no measurement and has no state. With a
                // full list it is `flex: 1 1 auto` against no free space,
                // collapses to nothing and scrolls off the end of a list that
                // is already longer than the panel. With a short list it
                // takes exactly the height the sessions did not.
                //
                // It duplicates the footer's New session, and that is the
                // trade taken deliberately: the footer's control is 32px of
                // chrome an operator learns once, and this is 500px of panel
                // that would otherwise say nothing. The label is hidden by a
                // container query below one line's worth of room, so the
                // duplication only exists when there is space that had no
                // other use.
                if !groups.is_empty() {
                    button {
                        class: "rg-sidebar__floor",
                        r#type: "button",
                        disabled: !ready,
                        title: if ready { "Start a session here (Ctrl+Shift+N)" } else { "Not connected" },
                        "aria-label": "Start a session",
                        onclick: move |_| props.on_new_session.call(None),
                        span { class: "rg-sidebar__floor-label", "Start a session" }
                        span { class: "rg-sidebar__floor-hint", "Ctrl+Shift+N" }
                    }
                }
            }

            // The restart offer, between the list and the footer.
            //
            // Here rather than in the titlebar because the two say different
            // things and only one of them is an interruption. The titlebar's
            // chip means "there is a newer build, spend bandwidth"; this
            // means "the bytes are already on disk and verified, and the next
            // start runs them". The second is free, so it is a line the
            // operator walks past until a moment that suits them, and it sits
            // above the control they use most rather than in the band they
            // read for the window's identity.
            //
            // Emitted only when there is something to restart into, so it is
            // never a dead control, and it never appears in the 48px rail,
            // where a sentence has nowhere to go.
            if let Some(line) = restart_to.clone() {
                button {
                    class: "rg-sidebar__restart",
                    r#type: "button",
                    title: "The update is downloaded and verified. Restarting runs it; sessions keep running in the daemon.",
                    "aria-label": "{crate::update::RESTART_TO_UPDATE}",
                    onclick: move |_| props.on_restart.call(()),
                    span { class: "rg-sidebar__restart-dot" }
                    span { class: "rg-sidebar__restart-line", "{line}" }
                }
            }

            // Bottom-leading, below the scroller: the product's most-used
            // control, beside its two least-used ones.
            //
            // That pairing is deliberate rather than leftover. The primary
            // control has to carry a word, because it launches on the first
            // click and a `+` that starts a process is a mystery button. The
            // toolbar cannot hold a word at any sidebar width the product
            // offers; this band has 120px free at the 224px floor against the
            // 112 the longest agent name needs, and bottom-leading is where
            // every platform source list puts an add action.
            //
            // Settings and the panel's own collapse are the two controls an
            // operator reaches for least often, and both work in the 3rem
            // collapsed state, which is why the collapse lives here rather
            // than in a header that no longer exists.
            div { class: "rg-sidebar__footer",
                div {
                    class: if pick_shown { "rg-newbar" } else { "rg-newbar rg-newbar--solo" },
                    button {
                        class: "rg-newbar__go",
                        r#type: "button",
                        disabled: !ready,
                        title: if ready { go_hint } else { "Not connected".to_string() },
                        "aria-label": "{go_aria}",
                        onclick: move |_| {
                            if can_launch {
                                props.on_launch_now.call(());
                            } else {
                                props.on_new_session.call(None);
                            }
                        },
                        span { class: "rg-newbar__what", "{go_text}" }
                    }
                    // Drawn only when it would do something the primary half
                    // does not. With nothing confident to start, both halves
                    // open the list, and two controls for one action is the
                    // duplication this pass exists to remove.
                    if pick_shown {
                        button {
                            class: "rg-newbar__pick",
                            r#type: "button",
                            disabled: !ready,
                            title: "Choose what to start (Ctrl+Shift+N)",
                            "aria-label": "Choose what to start",
                            onclick: move |_| props.on_new_session.call(None),
                            "{CHEVRON}"
                        }
                    }
                }
                button {
                    class: "rg-sidebar__action",
                    r#type: "button",
                    title: "Settings",
                    "aria-label": "Settings",
                    onclick: move |_| props.on_settings.call(()),
                    "{GEAR_ICON}"
                }
                button {
                    class: "rg-sidebar__action",
                    r#type: "button",
                    title: if collapsed { "Expand sidebar (Ctrl+Shift+B)" } else { "Collapse sidebar (Ctrl+Shift+B)" },
                    "aria-label": if collapsed { "Expand sidebar" } else { "Collapse sidebar" },
                    onclick: move |_| props.on_toggle_sidebar.call(()),
                    if collapsed { "\u{00bb}" } else { "\u{00ab}" }
                }
            }

            // Must stay the last child: the stylesheet positions it absolutely
            // against .rg-sidebar's right edge.
            div {
                class: "rg-sidebar__resizer",
                tabindex: 0,
                role: "separator",
                onmousedown: move |e| props.on_resize_start.call(e.client_coordinates().x),
                onkeydown: move |e| {
                    match e.key() {
                        Key::ArrowLeft => props.on_resize_nudge.call(-16.0),
                        Key::ArrowRight => props.on_resize_nudge.call(16.0),
                        _ => return,
                    }
                    e.prevent_default();
                },
            }
        }
    }
}

/// What an empty bucket says instead of drawing a header over a void.
///
/// Every bucket kind can legitimately be empty and they are empty for
/// different reasons, so one sentence would be wrong for two of them. A
/// folder is the case that matters: `bucket_by_folder` keeps an empty folder
/// deliberately, because a folder you just made is exactly the one you are
/// about to file into and hiding it makes it unreachable — but the sidebar
/// then drew a header, a zero, and nothing, with no hint anywhere on screen
/// that the way to fill it is the row context menu. The operator's only
/// evidence that named grouping works at all was a bucket that looked broken.
fn empty_bucket_hint(key: GroupKey) -> &'static str {
    match key {
        GroupKey::Folder(_) => "Empty. Right-click a session and use Move to folder.",
        GroupKey::Unfiled => "Nothing unfiled.",
        GroupKey::Project(_) | GroupKey::Directory(_) => "No sessions here yet.",
    }
}

/// The coarsest clock that renders this row byte-identically to `clock`.
///
/// # Why a row gets its own clock
///
/// [`SessionRow`] is memoized on its props, and the clock is one of them, so
/// a clock that moves rebuilds the row. A whole-second clock already stops the
/// rebuild inside one second, but it still rebuilds EVERY row on every second
/// boundary, forever. Most of those rows had nothing to say: a row reading
/// `5h ago` repeats that answer 3600 times before it changes, and at the
/// stated load of twenty sessions that is twenty rebuilds a second buying one
/// changed character an hour.
///
/// So the clock handed to a row is floored to the coarsest instant that row
/// cannot tell apart from now. Two things vary with time in a row and each
/// owns its own grid:
///
/// - the LABEL it draws, whose thresholds live in `TimeFormat::relative_floor`
/// - its STATE, whose transitions live in `SessionView::clock_floor_ms`
///
/// The later of the two wins, because that is the one that changes soonest.
/// The result sits inside both intervals, so every string and every
/// disposition is what it would have been at the real clock, and the row is
/// rebuilt exactly when something it shows actually moves.
///
/// A row with a live timer, a countdown, or a pending transition therefore
/// keeps a per-second clock and rebuilds as before. That is the point: this
/// buys precision back only where it was never being spent.
fn row_clock(clock: TimeFormat, row: &SessionView) -> TimeFormat {
    let policy = vitrum_model::DispositionPolicy::default();
    let state = row.clock_floor_ms(inbox::model_clock(clock), policy);
    let label = clock
        .relative_floor(Timestamp::from_millis(row.info.last_activity_ms as i64))
        .as_millis();
    let floor = label.max(state as i64).min(clock.now().as_millis());
    clock.at(Timestamp::from_millis(floor))
}

#[derive(Props, Clone, PartialEq)]
struct ProjectRollupChipsProps {
    rollup: vitrum_model::ProjectRollup,
}

/// The per-state counts a collapsed project header carries.
///
/// The most urgent state leads and the zeroes are dropped. A header that always
/// showed five numbers would spend four of them on nothing, on the row with the
/// least horizontal room in the whole sidebar.
#[allow(non_snake_case)]
fn ProjectRollupChips(props: ProjectRollupChipsProps) -> Element {
    let chips = inbox::rollup_chips(&props.rollup);
    let parked = props.rollup.snoozed;
    let woke = props.rollup.woke;
    rsx! {
        span { class: "rg-rollup",
            for (status, count) in chips {
                span {
                    key: "{status.token()}",
                    class: "rg-rollup__chip {inbox::status_modifier(status)}",
                    title: "{count} {status.label()}",
                    // A DOT, not a glyph. These are up to five chips on the
                    // narrowest row in the panel, and the five status glyphs
                    // spanned 6.2x in ink width, so a run of them read as a
                    // ragged line of unrelated marks rather than as one
                    // scale. One uniform mark per chip, hue from the chip's
                    // own modifier; `status_icon` keeps its single glyph for
                    // the pill, where there is only ever one of it.
                    span { class: "rg-rollup__dot" }
                    "{count}"
                }
            }
            if woke > 0 {
                span {
                    class: "rg-rollup__chip rg-rollup__chip--woke",
                    title: "{woke} came back from a snooze",
                    span { class: "rg-rollup__dot" }
                    "{woke}"
                }
            }
            if parked > 0 {
                span {
                    class: "rg-rollup__chip rg-rollup__chip--snoozed",
                    title: "{parked} parked",
                    span { class: "rg-rollup__dot" }
                    "{parked}"
                }
            }
        }
    }
}

/// The row elements the operator can switch off in Settings.
///
/// One `Copy` value rather than four props, read from
/// [`crate::state::Settings`] once per paint rather than once per row. Three
/// of them do not change the row's SHAPE: `__branch` is still emitted when it
/// is off, empty, because it is the flex spacer that pushes line one's tail
/// right, and `__word` off leaves `__icon`, which is exactly what the
/// collapsed sidebar already draws. `always_slim` is the one that does, and
/// it is the operator asking for every agent on screen at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowFields {
    branch: bool,
    time: bool,
    status_word: bool,
    /// Force every row to the slim shape, whatever band it is in.
    always_slim: bool,
}

impl RowFields {
    fn of(settings: &crate::state::Settings) -> Self {
        RowFields {
            branch: settings.show_branch,
            time: settings.show_time,
            status_word: settings.show_status_word,
            always_slim: settings.always_slim,
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct SessionRowProps {
    row: SessionView,
    /// Which band this row was drawn from, and therefore which shape it takes.
    section: Section,
    /// Which optional row elements this window is drawing.
    fields: RowFields,
    active: bool,
    picked: bool,
    /// The operator's home directory, for the tooltip's path shortening.
    ///
    /// Shared rather than owned. Twenty rows each holding their own copy is
    /// twenty heap allocations and twenty memcpys of the same string on every
    /// paint; one buffer and a refcount bump per row is the same string.
    home: Rc<str>,
    clock: TimeFormat,
    /// Files this session is contesting, and how many other sessions it is
    /// contesting them with. `None` when it is fighting nobody, which is the
    /// overwhelmingly common case and draws no element at all.
    contested: Option<(usize, usize)>,
    on_select: EventHandler<(SessionId, Click)>,
    on_close: EventHandler<SessionId>,
    on_menu: EventHandler<(f64, f64, SessionId)>,
}

/// Class list for one shelf.
///
/// The band modifier is not decoration: `--snoozed` tints its head in the
/// snooze hue and `--settled` drains it, and those are the only two cues that
/// say which shelf a run of rows belongs to once it is scrolled away from its
/// caption.
fn section_class(section: Section, open: bool) -> String {
    let mut class = String::from("rg-project__section ");
    class.push_str(match section {
        Section::Active => "rg-project__section--active",
        Section::Snoozed => "rg-project__section--snoozed",
        Section::Settled => "rg-project__section--settled",
    });
    if !open {
        class.push_str(" rg-project__section--collapsed");
    }
    class
}

/// Which of the two shapes a band's rows take.
///
/// Cards for the inbox, slim rows for the tail. The operator's own parking is
/// what makes the list dense; a sidebar that decided on their behalf which
/// live sessions deserved a card would be guessing at the one thing they have
/// already told it.
///
/// `always_slim` is the operator saying it anyway. At 4K the card yields
/// thirteen rows and the slim row nineteen, so that one switch is the
/// difference between "most of my agents" and "all of them"; it is theirs to
/// throw, not ours to guess.
fn row_variant(section: Section, always_slim: bool) -> &'static str {
    if always_slim {
        return "rg-session--slim";
    }
    match section {
        Section::Active => "rg-session--card",
        Section::Snoozed | Section::Settled => "rg-session--slim",
    }
}

/// Does this row draw the card's markup?
///
/// The SHAPE half of the decision [`row_variant`] makes about the CLASS, and
/// they have to agree. They did not: this was `section == Section::Active`
/// spelled out at the call site while `row_variant` also consulted
/// `always_slim`, so with the operator's "every row slim" switch thrown an
/// Active row wore the slim class and then rendered the card's markup inside
/// it. A box and its contents disagreeing, produced by a control the operator
/// had deliberately used.
///
/// One function, called by both, is the only arrangement in which the two
/// cannot drift again.
fn draws_card(section: Section, always_slim: bool) -> bool {
    !always_slim && section == Section::Active
}

/// How many of a band's rows are drawn, and how many stay behind "Show more".
///
/// Only Done is cut. It was the one band in the sidebar with no bound on it:
/// every session ever finished in a bucket stayed a row, a comparator and a
/// DOM node on every paint once the shelf was open, and a month of work in
/// one project is not a list. [`inbox::SETTLED_TAIL_LIMIT`] answers "what did
/// I just finish", which is the only question the shelf is opened for; older
/// than that is an archive lookup, and the filter is what that is for.
///
/// Snoozed is deliberately NOT cut. A parked row comes back by itself, so
/// that band drains on its own and cannot grow without bound the way a
/// permanent record does. Active is not cut here either — it has its own
/// preview at [`inbox::PREVIEW_LIMIT`], applied in `inbox::build_group`,
/// because that one has to rescue the focused row and this one has nothing to
/// rescue.
fn band_cut(section: Section, rows: usize, expanded: bool) -> (usize, usize) {
    if section != Section::Settled || expanded {
        return (rows, 0);
    }
    let shown = rows.min(inbox::SETTLED_TAIL_LIMIT);
    (shown, rows - shown)
}

/// Everything about a row that changes its class list.
///
/// A struct rather than eight positional booleans: `row_class(true, false,
/// false, true, ...)` is unreadable at the call site and silently wrong when
/// two arguments swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowState {
    section: Section,
    status: SidebarStatus,
    active: bool,
    picked: bool,
    unread: bool,
    woke: bool,
    /// Finished while nobody was looking. Not the same as unread: a working
    /// agent is unread constantly and wants nothing.
    finished_unseen: bool,
    attention: Option<&'static str>,
    /// Every row is slim, whatever band it is in.
    always_slim: bool,
}

/// Does this row give up its prominence?
///
/// The single idea that stops the list reading as a table. Twenty rows at
/// equal weight force the operator to read all twenty; a row that wants
/// nothing yet drops to the muted foreground and normal weight, and hover
/// puts it back, so the quiet majority stays legible without competing.
///
/// A row keeps its weight for exactly four reasons, all of them "a human is
/// implicated": it failed, there is output nobody has read, it came back from
/// a snooze, or it finished unseen. Selection and focus also hold it, because
/// dimming the row the operator is looking at is absurd.
fn recedes(s: RowState) -> bool {
    matches!(
        s.status,
        SidebarStatus::Ready
            | SidebarStatus::Working
            | SidebarStatus::Approval
            | SidebarStatus::Input
    ) && !s.unread
        && !s.woke
        && !s.finished_unseen
        && !s.active
        && !s.picked
}

/// Does the whole row fade, on top of receding?
///
/// Working, approval and input are all "not your problem yet": the agent is
/// mid-turn and the next event will come from it, not from you. Ready is
/// excluded because a finished turn IS your problem even when nothing has
/// flagged it.
fn in_flight(s: RowState) -> bool {
    matches!(
        s.status,
        SidebarStatus::Working | SidebarStatus::Approval | SidebarStatus::Input
    ) && !s.active
        && !s.picked
}

/// Class list for a session row.
///
/// Pulled out of the markup so the exact emitted class names are testable; the
/// stylesheet keys every visual state off this string and a typo in one
/// modifier silently drops that state with no error anywhere.
///
/// `picked` is deliberately not the same thing as `active`: `--active` is the
/// row whose PTY is in the main pane, and `--picked` is membership of a
/// multi-selection. A row can be either, both, or neither.
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
    if let Some(rail) = s.attention {
        class.push(' ');
        class.push_str(rail);
    }
    class
}

/// Should this row draw the unread dot?
///
/// The attention rail and the unread dot share one slot on purpose. At the
/// 14rem width floor the row already carries a status label and a close
/// button, and a second right-hand marker takes the title box below
/// legibility.
fn show_unread_dot(unread: bool, attention: Option<&str>) -> bool {
    unread && attention.is_none()
}

/// Which mouse gesture a click on a row was.
///
/// Shift wins over Ctrl when both are held, matching every file manager:
/// Ctrl+Shift is the additive range, and that is what `RangeAdditive` is.
fn click_kind(ctrl: bool, shift: bool) -> Click {
    match (ctrl, shift) {
        (true, true) => Click::RangeAdditive,
        (false, true) => Click::Range,
        (true, false) => Click::Toggle,
        (false, false) => Click::Plain,
    }
}

/// Everything a row's hover detail says, in one string.
///
/// Assembled here rather than in the markup so the exact text is testable.
///
/// It carries EVERY fact the row used to spread across four separate `title`
/// attributes — the surface, the status pill, the disposition badge and the
/// contest mark. That consolidation is the fix for the black rectangle: see
/// `.rg-session__tip` in `sidebar.css` for why a row inside this panel may
/// not ask the platform for a tooltip at all.
///
/// The state word comes from [`Pill`], not from `status_label`, so this
/// string and the pill 8px above it cannot name one state two ways.
fn row_tooltip(row: &SessionView, home: &str, pill: &Pill) -> String {
    let info = &row.info;
    let mut s = format!(
        "{}\n{}\n{} \u{2022} {}",
        inbox::row_title(info),
        vitrum_fmt::path::shorten_home_relative(&info.cwd, home, TOOLTIP_PATH_COLUMNS),
        AgentKind::of(&info.command).label(),
        pill.word
    );
    // How the state was decided. An inferred status and a probed one look
    // identical apart from this sentence, and it is the one thing the pill's
    // own tooltip carried that nothing else on the row says. It also answers
    // the blocked question for every platform, including the one that cannot
    // probe, so the row does not say it twice in two vocabularies.
    s.push('\n');
    s.push_str(inbox::source_note(pill.source));
    if attention_modifier(&info.attention).is_some() {
        s.push('\n');
        s.push_str(&attention_label(&info.attention));
    }
    s.push_str("\nRight-click for more");
    s
}

/// What a contested row says on hover.
///
/// Names the thing that is actually wrong, in the operator's terms: files,
/// and who else is in them. "Conflict" is avoided on purpose, because that is
/// what version control calls a merge outcome and this is not one: two agents
/// writing one file lose work without anything ever reporting a conflict.
fn contest_title(files: usize, peers: usize) -> String {
    let f = if files == 1 { "file" } else { "files" };
    let p = if peers == 1 { "session" } else { "sessions" };
    format!(
        "{files} {f} also being changed by {peers} other {p}. \
         Whichever writes last wins, silently."
    )
}

/// Status hue for a row's agent mark.
///
/// The mark carries WHO is running; the hue carries WHETHER it is running. One
/// element answers both, which is the whole reason it can afford 16px in a
/// 224px row. Same four tiers as the pill, so a row can never say two things.
fn agent_class(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Starting => "rg-session__agent--starting",
        SessionStatus::Running => "rg-session__agent--running",
        SessionStatus::Exited { code: Some(0) } => "rg-session__agent--exited",
        SessionStatus::Exited { .. } => "rg-session__agent--exited-error",
    }
}

#[derive(Props, Clone, PartialEq)]
struct AgentGlyphProps {
    mark: AgentMark,
    hue: &'static str,
}

/// The agent identity mark on a session row.
///
/// Two subpaths at most and no per-row allocation: the paths are `&'static
/// str` out of the mark table, and the class is one of four constants. This
/// draws once per row per paint, so anything built here is built twenty times
/// a frame.
#[allow(non_snake_case)]
fn AgentGlyph(props: AgentGlyphProps) -> Element {
    rsx! {
        svg {
            class: "rg-session__agent {props.hue}",
            view_box: "0 0 16 16",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.25",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "{props.mark.stroke}" }
            if !props.mark.fill.is_empty() {
                path { d: "{props.mark.fill}", fill: "currentColor", stroke: "none" }
            }
        }
    }
}

#[allow(non_snake_case)]
fn SessionRow(props: SessionRowProps) -> Element {
    render_count::tick();
    let row = &props.row;
    let info = &row.info;
    let id = row.id();
    let model_clock = inbox::model_clock(props.clock);
    // The row does not know the window's policy, and it must not invent one: a
    // row rendering under different auto-settle rules from the group it sits in
    // would show a Snoozed badge inside the Done band. The default is what
    // `UiState` starts with and what the group was built with.
    let policy = vitrum_model::DispositionPolicy::default();

    // One status resolution per row per paint. `Pill::of` already ran it, and
    // `SessionView::status()` would run it a second time for the same answer.
    let pill = Pill::of(row);
    let completion = inbox::completion_badge(row);
    let woke = row.disposition(model_clock, policy) == Disposition::Woke;
    let attention = attention_modifier(&info.attention);
    let card = draws_card(props.section, props.fields.always_slim);
    // Both of these allocate, and each is drawn by exactly one of the two row
    // shapes. A snoozed row is the case that mattered: `disposition_badge`
    // built a class, a countdown and a "Parked until ..." sentence for every
    // slim row on every paint, and the slim markup below never reads it.
    let disposition = card
        .then(|| inbox::disposition_badge(row, model_clock, policy))
        .flatten();
    let parked = (!card)
        .then(|| inbox::parked_label(row, model_clock, policy))
        .flatten();
    // How long the turn that is running right now has been running, or
    // `None`. A different question from the row's timestamp, which is when
    // the agent last SPOKE: an agent silently computing for an hour has a
    // fresh timestamp and is the row worth finding. Absent unless a turn is
    // live, so a list at rest emits no element for it at all.
    let aux = card.then(|| inbox::working_aux(row, model_clock)).flatten();
    let class = row_class(RowState {
        section: props.section,
        always_slim: props.fields.always_slim,
        status: pill.status,
        active: props.active,
        picked: props.picked,
        unread: info.unread,
        woke,
        finished_unseen: row.has_unseen_completion(),
        attention,
    });
    let show_unread = show_unread_dot(info.unread, attention);
    let age = age(props.clock, info.last_activity_ms);
    // Always emitted, empty when the server has not resolved a branch or the
    // operator switched branches off. It is the flexible box on line one, so
    // dropping the element would let the timestamp and the status slide left
    // into the middle of the row on half the rows.
    let branch = if props.fields.branch {
        info.git_branch.as_deref().unwrap_or_default()
    } else {
        ""
    };
    let mut tooltip = row_tooltip(row, &props.home, &pill);
    if let Some((files, peers)) = props.contested {
        tooltip.push('\n');
        tooltip.push_str(&contest_title(files, peers));
    }
    if let Some(badge) = disposition.as_ref() {
        tooltip.push('\n');
        tooltip.push_str(&badge.title);
    }
    if let Some(badge) = completion.as_ref() {
        tooltip.push('\n');
        tooltip.push_str(&badge.title);
    }
    if let Some(ticket) = parked.as_ref() {
        tooltip.push('\n');
        tooltip.push_str(&ticket.title);
    }
    let dom_id = row_id(id);
    // A generated title is the command name, which is the same word on every
    // row a shell runs in: 60 real sessions produced 57 rows reading `bash`.
    // `row_title` appends the session id to those and leaves a name the
    // operator chose exactly as they typed it.
    let title = inbox::row_title(info);
    // Which agent is behind this session. Fixed 16px, so it sits BEFORE the
    // title without making a title's left edge depend on anything variable.
    // `AgentKind::of` never guesses: an unrecognised command draws the unknown
    // mark, not the nearest agent's.
    let agent = AgentKind::of(&info.command);
    let agent_mark = agent.mark();
    let agent_hue = agent_class(&info.status);

    rsx! {
        div {
            class: "{class}",
            id: "{dom_id}",
            tabindex: 0,
            onclick: move |e| {
                let m = e.modifiers();
                props.on_select.call((id, click_kind(m.ctrl() || m.meta(), m.shift())));
            },
            oncontextmenu: move |e| {
                e.prevent_default();
                let p = e.client_coordinates();
                props.on_menu.call((p.x, p.y, id));
            },
            onkeydown: move |e| {
                let k = e.key();
                if k == Key::Enter || k == Key::Character(" ".to_string()) {
                    props.on_select.call((id, click_kind(false, e.modifiers().shift())));
                    e.prevent_default();
                }
            },

            if card {
                // TWO lines, both unconditional, so every card in a band is
                // exactly the same height.
                //
                // There used to be a third, emitted only when a badge existed,
                // and the badges sat beside the title. Measured at the 224px
                // width floor: a plain card gave its title 127px of box, one
                // badge cut it to 69.5px against a 328px string, two badges
                // cut it to 12.5px — one character and an ellipsis, with a
                // chip reading "Done" outranking the name of the session. The
                // close button landed at 33px, 90.5px and 147.5px from the
                // right edge on the same three rows. One list, three row
                // heights, three title widths and three positions for one
                // control, all from the same conditional line.
                //
                // Line one: the agent mark, then the title. The mark is a fixed
                // 16px box on every row, so every title still starts at the same
                // x. The status sits at the far end and never in front, so a
                // title's left edge cannot depend on how long a status word is.
                div { class: "rg-session__line rg-session__line--title",
                    AgentGlyph { mark: agent_mark, hue: agent_hue }
                    span { class: "rg-session__title", "{title}" }
                    if show_unread {
                        span { class: "rg-session__unread" }
                    }
                    span { class: "rg-session__slot",
                        span {
                            class: "{pill.class}",
                            "aria-label": "{pill.word}",
                            // Off leaves the pill's box and its hue, which is
                            // exactly what the collapsed rail already draws:
                            // `.rg-sidebar--collapsed .rg-pill__word` hides
                            // the same element. `aria-label` above still
                            // carries the word, so the state is never lost to
                            // a screen reader, only to the column.
                            if props.fields.status_word {
                                span { class: "rg-pill__word", "{pill.word}" }
                            }
                            if let Some(aux) = aux {
                                span { class: "rg-pill__aux", "{aux}" }
                            }
                        }
                    }
                }
                // Line two: the row's tail. `__branch` is emitted even when
                // it is empty, because it is the flex spacer that pushes
                // everything after it right; drop the element on the rows
                // with no branch and the tail slides into the middle of the
                // row on half the list.
                div { class: "rg-session__line rg-session__line--tail",
                    // The contest leads line two, at the row's left edge.
                    //
                    // Line one is full at the 224px width floor and its budget
                    // is spent on the mark, the title and the status, so a
                    // conditional element there would move a title's left edge
                    // on the few rows that have one.
                    //
                    // It sits BEFORE the branch, which is fixed-width and so
                    // does not disturb the branch's job as the flex spacer for
                    // everything after it. Put after the branch it was pushed
                    // to the far right, where on a row with no branch it
                    // floated alone against the timestamp with the whole left
                    // half of the line empty. The most urgent thing a row can
                    // say belongs under the title, not in the gutter.
                    if let Some((files, _)) = props.contested {
                        span {
                            class: "rg-session__contest",
                            span { class: "rg-session__contest-mark" }
                            span { class: "rg-session__contest-count", "{files}" }
                        }
                    }
                    span { class: "rg-session__branch", "{branch}" }
                    if let Some(badge) = disposition {
                        span { class: "{badge.class}",
                            if let Some(icon) = badge.icon {
                                span { class: "rg-badge__icon", "{icon}" }
                            }
                            "{badge.text}"
                        }
                    }
                    if let Some(badge) = completion {
                        span { class: "{badge.class}",
                            if let Some(icon) = badge.icon {
                                span { class: "rg-badge__icon", "{icon}" }
                            }
                            "{badge.text}"
                        }
                    }
                    span { class: "rg-session__slot",
                        if props.fields.time {
                            span { class: "rg-session__time", "{age}" }
                        }
                        // The hover group, stacked on the timestamp in the
                        // slot's one grid cell. It is a wrapper and not a
                        // bare button because the cell has to cross-fade one
                        // GROUP against the time, and because the row's
                        // lifecycle actions belong beside the close and not
                        // in a second cell that would widen the row.
                        span { class: "rg-session__actions",
                            button {
                                class: "rg-session__close",
                                r#type: "button",
                                "aria-label": "Terminate session",
                                onclick: move |e| {
                                    // Without this the click also lands on the
                                    // row and focuses the session being killed.
                                    e.stop_propagation();
                                    props.on_close.call(id);
                                },
                                "\u{00d7}"
                            }
                        }
                    }
                }
            } else {
                // The slim row. One line, the title at the same left edge as a
                // card's title, so the tail scans as a continuation of the
                // list rather than a new one.
                AgentGlyph { mark: agent_mark, hue: agent_hue }
                span { class: "rg-session__title", "{title}" }
                if let Some(badge) = completion {
                    span { class: "{badge.class}",
                        if let Some(icon) = badge.icon {
                            span { class: "rg-badge__icon", "{icon}" }
                        }
                        "{badge.text}"
                    }
                }
                span { class: "rg-session__slot",
                    if let Some(ticket) = parked {
                        span { class: "{ticket.class}",
                            span { class: "rg-pill__word", "{ticket.text}" }
                        }
                    } else if props.fields.time {
                        span { class: "rg-session__time", "{age}" }
                    }
                    span { class: "rg-session__actions",
                        button {
                            class: "rg-session__close",
                            r#type: "button",
                            "aria-label": "Terminate session",
                            onclick: move |e| {
                                e.stop_propagation();
                                props.on_close.call(id);
                            },
                            "\u{00d7}"
                        }
                    }
                }
            }

            // THE ROW'S HOVER DETAIL, DRAWN BY US.
            //
            // This element replaces four `title` attributes: one on the row
            // surface, one on the status pill, one on each badge, one on the
            // contest mark and one on the close button. `title` is not a
            // request for a tooltip, it is a request for a WINDOW: the engine
            // hands it to the platform, which paints an override-redirect
            // surface above the document that nothing in this stylesheet can
            // reach. Two consequences, both observed on a real session.
            //
            // It is anchored to the POINTER and not to the row. Reorder the
            // list under a stationary cursor — which happens whenever a
            // project is pinned to the top or an agent changes state — and
            // the surface stays exactly where it was, over rows it no longer
            // describes, until the pointer moves and the engine recomputes
            // it. Captured headless it is a black rectangle sitting across
            // three rows for the whole of a reorder.
            //
            // And it is painted by the platform, so its colours are the
            // desktop's tooltip colours and not this product's. On a host
            // with no theme resolved it comes out pure black on a panel this
            // design spent a file getting the value of.
            //
            // A span in the row is neither. It is a child of the thing it
            // describes, so a reorder MOVES it; `:hover` on the row is
            // recomputed by the same layout that moved the row, so no frame
            // can show it detached; and it is painted by us in our own
            // colours. It costs one node and one text node per row, which is
            // the honest price and is why the string is assembled once above
            // rather than per element.
            span { class: "rg-session__tip", role: "tooltip", "{tooltip}" }
        }
    }
}

#[cfg(test)]
mod tests;

/// The session row, RENDERED.
///
/// Every other guard in this file reads `sidebar.rs` as text or calls a pure
/// function. Both pass while the markup that reaches the operator is wrong,
/// and that is not hypothetical: this product shipped a status dot with four
/// colour modifiers and no box, a "Show" button on every notification that
/// could not be clicked, and a whole search result path the client discarded.
/// Each was correct code with one missing link, and a green suite the whole
/// time. A test that builds the component and looks at the HTML is the only
/// kind that can see that class of defect.
#[cfg(test)]
mod rendered_row;

/// The WHOLE sidebar, rendered.
///
/// The panel is the product. It had 1,926 tests and not one of them built it:
/// they asserted CSS substrings, pure-function returns and source text, so a
/// green suite sat beside a screenshot of a project header with no sessions
/// under it. Everything here starts from a `UiState` with real sessions in it
/// and looks at the HTML that would reach the webview.
#[cfg(test)]
mod rendered_sidebar;

/// Does an unchanged row survive a paint?
///
/// The rendered-HTML guards above cannot see this: the HTML is identical
/// whether Dioxus rebuilt a row or skipped it, and the difference between
/// rebuilding one row per update and rebuilding twenty is the VDOM half of
/// the frame budget.
#[cfg(test)]
mod memoization;
