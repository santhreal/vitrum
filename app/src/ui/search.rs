//! Cross-session scrollback search: the client surface for
//! [`vitrum_proto::ClientMsg::Search`].
//!
//! The daemon holds every session's bytes and can sweep all of them at once;
//! a client holds one viewport. "Which of my twenty agents hit an OOM" is one
//! server-side sweep and is impossible anywhere else, which is why this
//! surface exists and why it is not the sidebar's title filter. The filter
//! narrows a list of names. This searches output.
//!
//! # Bytes, not strings, and this is the whole file
//!
//! [`SearchHit::visible`] is `Vec<u8>` deliberately, and the protocol's own
//! doc comment says why: lossy UTF-8 decoding turns one invalid byte into a
//! three-byte `U+FFFD`, which shifts every offset after it, so a highlight
//! drawn at `match_start..match_end` in the DECODED string lands on the wrong
//! substring. It lands wrong only on lines containing a stray byte, which are
//! exactly the lines somebody is searching for when something has gone wrong,
//! so the failure hides in the one case that matters.
//!
//! [`split_hit`] therefore slices the RAW bytes at the daemon's offsets and
//! decodes each of the three pieces separately. Decoding cannot move a
//! boundary it is applied inside. Every other function here takes the same
//! discipline: nothing in this module ever decodes a whole line and then
//! indexes into it.
//!
//! # What is on the surface, and what is not
//!
//! One field, three switches, one honest summary, and the hits. There is no
//! toolbar, no magnifier glyph beside a field whose placeholder already says
//! what it is, and no Search button beside a field that submits on Enter and
//! says so. Each of those was drawn and removed: an element that repeats its
//! neighbour is noise, and this surface quotes terminal output, which needs
//! the room.
//!
//! The switches stay because they are function rather than ornament: they are
//! the three booleans the wire carries, and there is no other way to reach
//! them. They are spelled as words, not as `.*` and `Aa`, because the words
//! fit and a two-character glyph needs a tooltip to mean anything.
//!
//! # Rendering only
//!
//! [`Search`] reads no global state. It takes what the daemon answered, plus
//! the field contents and the three switches, and emits callbacks. Whoever
//! owns the layer owns the query, the options, the in-flight flag and the
//! results; this file owns how they look and nothing else.
//!
//! # Why there is no search-as-you-type
//!
//! Measured in `vitrum-server`'s own header: twenty sessions holding a full
//! ring each is 200 MiB and 84 to 96 ms of daemon CPU per sweep. Firing that
//! on every keystroke would spend the daemon's whole budget on a half-typed
//! word. The field submits on Enter, and a switch only re-runs a sweep if the
//! owner chooses to.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use vitrum_fmt::{bytes, count, text};
use vitrum_proto::{ClientMsg, SearchHit, SessionId};

/// Context lines requested either side of a hit.
///
/// Two. One is not enough to tell a stack frame from its caller, and four
/// turns a twelve-hit answer into a log viewer that no longer fits the sheet:
/// at a 20px line box, four either side is a 180px row and three rows fill
/// the list.
pub const CONTEXT_LINES: u16 = 2;

/// Hit cap for one sweep.
///
/// The daemon rations this per session (`search::fairness_cap`, a quarter of
/// the budget floored at eight), so 500 is 125 per session once more than one
/// session is in scope: enough that a chatty agent cannot crowd out a quiet
/// one, and small enough that the answer is a list a person reads rather than
/// a file they scroll.
pub const MAX_HITS: u32 = 500;

/// How much of the swept pattern the summary quotes back.
///
/// A pattern is operator input with no length limit. Quoting it whole puts an
/// arbitrary string into a one-line summary and wraps the header.
const PATTERN_BUDGET: usize = 48;

/// The three switches [`vitrum_proto::ClientMsg::Search`] actually carries.
///
/// Exactly three, because exactly three exist on the wire. A fourth control
/// here would be a switch that renders and changes nothing.
///
/// Serialisable because these three are PREFERENCES, not query scope. The
/// pattern is one question the operator is asking right now and is rightly
/// forgotten; whether they read output as regex, case-blind or word-bounded
/// is how they work, and an operator who lives in regex would otherwise
/// re-flip the same switch on every launch forever. `settings.rs` states the
/// bar absolutely: a control ships only if flipping it changes behaviour
/// immediately AND survives a restart. Persisting the switches while leaving
/// the query and the hits transient is the only split that satisfies it
/// without saving 500 stale search results into a profile.
///
/// `default` on the container, not just the fields, so a profile written
/// before a fourth switch existed still loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Options {
    /// Treat the pattern as a regular expression rather than a literal.
    pub regex: bool,
    pub case_insensitive: bool,
    pub whole_word: bool,
}

/// Which switch was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    Regex,
    CaseInsensitive,
    WholeWord,
}

impl Toggle {
    /// Every switch, in the order they render.
    pub const ALL: [Toggle; 3] = [Toggle::Regex, Toggle::CaseInsensitive, Toggle::WholeWord];

    /// The switch's caption.
    ///
    /// A word, not a glyph. `.*`, `Aa` and a boxed `w` are the conventional
    /// icons and every one of them needs a tooltip to mean anything, which is
    /// an affordance that does not exist on a touchpad and does not survive a
    /// screenshot. The three words fit on one line at the sheet's width.
    pub fn label(self) -> &'static str {
        match self {
            Toggle::Regex => "Regex",
            Toggle::CaseInsensitive => "Ignore case",
            Toggle::WholeWord => "Whole word",
        }
    }
}

impl Options {
    /// Whether `which` is on.
    pub fn is_on(self, which: Toggle) -> bool {
        match which {
            Toggle::Regex => self.regex,
            Toggle::CaseInsensitive => self.case_insensitive,
            Toggle::WholeWord => self.whole_word,
        }
    }

    /// The same options with `which` flipped.
    ///
    /// Returned rather than mutated so the owner can hold the options in
    /// whatever it likes and this file stays a pure function of its props.
    pub fn toggled(self, which: Toggle) -> Options {
        let mut next = self;
        match which {
            Toggle::Regex => next.regex = !self.regex,
            Toggle::CaseInsensitive => next.case_insensitive = !self.case_insensitive,
            Toggle::WholeWord => next.whole_word = !self.whole_word,
        }
        next
    }
}

/// One answer from the daemon, as [`vitrum_proto::ServerMsg::SearchResults`]
/// delivered it.
///
/// `pattern` is the pattern the daemon actually swept, which is not
/// necessarily what is in the field now: the operator keeps typing while the
/// sweep runs. The summary quotes THIS one, so the header can never claim a
/// result belongs to a pattern that was never sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub pattern: String,
    pub hits: Vec<SearchHit>,
    pub truncated: bool,
    pub bytes_scanned: u64,
}

/// One line of a hit, cut at the daemon's byte offsets and decoded piecewise.
///
/// Three separate strings rather than one string and a range, because a range
/// into a decoded string is the bug this type exists to make unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
    /// Text before the match.
    pub before: String,
    /// The matched text itself.
    pub matched: String,
    /// Text after the match.
    pub after: String,
    /// The highlight has no glyphs of its own, so it must be drawn as a caret
    /// rather than as a background behind nothing.
    ///
    /// Two real causes: a zero-width regex match (`^`, `x*`), and a match
    /// landing on bytes that [`vitrum_fmt::text::sanitize_line`] drops. Both
    /// would otherwise render a hit with no visible reason for being a hit.
    pub empty_mark: bool,
}

/// Cut `visible` at the daemon's offsets and decode each piece on its own.
///
/// The offsets index the RAW bytes. Slicing first and decoding after is what
/// keeps the highlight on the substring the daemon matched, whatever those
/// bytes turn out to be. Decoding first would insert `U+FFFD` ahead of the
/// offsets and slide the highlight along the line.
///
/// A boundary that falls inside a multi-byte sequence, which a byte-oriented
/// matcher can produce on a line with a stray byte in it, yields a `U+FFFD`
/// at the end of one piece and another at the start of the next. That is the
/// honest rendering: the boundary stays where the daemon put it and the
/// damaged character is shown as damaged on both sides of it.
///
/// Offsets are clamped rather than trusted. They arrive over a socket, and
/// `&visible[start..end]` on a bad pair panics the render.
pub fn split_hit(visible: &[u8], match_start: u32, match_end: u32) -> Split {
    let len = visible.len();
    let start = (match_start as usize).min(len);
    let end = (match_end as usize).clamp(start, len);
    let matched = line_text(&visible[start..end]);
    Split {
        before: line_text(&visible[..start]),
        empty_mark: matched.is_empty(),
        matched,
        after: line_text(&visible[end..]),
    }
}

/// Decode one run of bytes for display on a single line.
///
/// Lossy because a PTY carries whatever the program wrote and there is no
/// other choice at the point where bytes become glyphs. It is safe HERE, and
/// only here, because the slicing already happened: replacement characters
/// can no longer move a boundary.
///
/// Sanitised because the daemon strips escape SEQUENCES, and a bare `\r`,
/// `\t` or `\x07` is not one. A `\r` left in returns the cursor in any
/// terminal this text is pasted into and renders as a hole in the row.
pub fn line_text(raw: &[u8]) -> String {
    text::sanitize_line(&String::from_utf8_lossy(raw)).into_owned()
}

/// Hits from one session, in the order the daemon returned them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group<'a> {
    pub session: SessionId,
    /// The session's title, or `Session N` when the caller supplied none.
    pub label: String,
    pub hits: Vec<&'a SearchHit>,
}

/// Bucket hits by session, first-appearance order preserved.
///
/// The daemon sweeps sessions in ascending id and its global cap consumes
/// that order, which is what makes a truncated answer the FIRST n hits.
/// Re-sorting here would throw that property away, so this groups without
/// reordering.
pub fn group_by_session<'a>(
    hits: &'a [SearchHit],
    titles: &[(SessionId, String)],
) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> = Vec::new();
    for hit in hits {
        match groups.iter_mut().find(|g| g.session == hit.session) {
            Some(group) => group.hits.push(hit),
            None => groups.push(Group {
                session: hit.session,
                label: session_label(hit.session, titles),
                hits: vec![hit],
            }),
        }
    }
    groups
}

/// A session's heading.
///
/// Falls back to the id rather than to an empty heading: a group with no name
/// reads as a rendering fault, and the id is at least something the operator
/// can match against the sidebar.
fn session_label(session: SessionId, titles: &[(SessionId, String)]) -> String {
    titles
        .iter()
        .find(|(id, _)| *id == session)
        .map(|(_, title)| title.trim())
        .filter(|title| !title.is_empty())
        .map_or_else(
            || format!("Session {}", session.0),
            |title| text::truncate_end(title, 40),
        )
}

/// The line above the results, and the class that paints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The full class string, modifier included.
    pub class: &'static str,
    pub text: String,
}

/// What to say about the last answer.
///
/// Four states, and the distinctions are the point:
///
/// - **Not searched yet** and **no matches** are different facts. Rendering
///   both as an empty list tells an operator their agents never printed the
///   word they are looking for when in truth nothing was ever asked.
/// - **Truncated** never implies completeness. It says these are the first n
///   and that there are more, because the daemon stopped at its cap, and an
///   operator who reads "12 matches" and acts on it having been shown 12 of
///   400 has been actively misled.
/// - `bytes_scanned` is reported as swept, not as total, because that is what
///   it is: a sweep that stopped early scanned less than the rings hold.
///
/// The idle line names Enter. That is where the submit button's function
/// went when the button was removed for repeating the field beside it.
///
/// `scope` is how many sessions the sweep was restricted to, zero for all of
/// them. Every sentence states it, because "3 matches in 2 sessions" is the
/// same words for a sweep of twenty and a sweep of the two you selected, and
/// only one of those means your other eighteen agents are clean.
pub fn summary(answer: Option<&Answer>, searching: bool, scope: usize) -> Summary {
    let where_ = scope_phrase(scope);
    if searching {
        return Summary {
            class: "rg-search__summary rg-search__summary--busy",
            text: format!("Sweeping {where_}."),
        };
    }
    let Some(answer) = answer else {
        return Summary {
            class: "rg-search__summary rg-search__summary--idle",
            text: format!("Not searched yet. Type a pattern and press Enter to sweep {where_}."),
        };
    };

    let pattern = text::truncate_middle(&answer.pattern, PATTERN_BUDGET);
    let swept = bytes::binary(answer.bytes_scanned);
    if answer.hits.is_empty() {
        return Summary {
            class: "rg-search__summary rg-search__summary--none",
            text: format!("No matches for \u{201c}{pattern}\u{201d} in {where_}. Swept {swept}."),
        };
    }

    let matches = matches_word(answer.hits.len() as u64);
    let sessions = count::count_s(distinct_sessions(&answer.hits) as u64, "session");
    if answer.truncated {
        return Summary {
            class: "rg-search__summary rg-search__summary--truncated",
            text: format!(
                "First {matches} in {sessions} of {where_}, then the sweep hit its cap of \
                 {MAX_HITS}. There are more. Swept {swept} so far."
            ),
        };
    }
    Summary {
        class: "rg-search__summary rg-search__summary--ok",
        text: format!("{matches} in {sessions} of {where_}. Swept {swept}."),
    }
}

/// What the sweep covered, as a noun phrase.
fn scope_phrase(scope: usize) -> String {
    if scope == 0 {
        "every session's scrollback".to_string()
    } else {
        format!(
            "the {} you selected",
            count::count_s(scope as u64, "session")
        )
    }
}

/// `count_s` would pluralise `match` as `matchs`. English does not.
///
/// A named wrapper rather than an inlined call, so the two sites cannot drift
/// and so the wrong plural has a test naming it.
fn matches_word(n: u64) -> String {
    count::count(n, "match", "matches")
}

/// How many distinct sessions the hits came from.
fn distinct_sessions(hits: &[SearchHit]) -> usize {
    let mut seen: Vec<SessionId> = Vec::new();
    for hit in hits {
        if !seen.contains(&hit.session) {
            seen.push(hit.session);
        }
    }
    seen.len()
}

/// The wire request for the current field, or `None` when there is nothing to
/// sweep.
///
/// Built here rather than at the send site so the caps live beside the UI
/// that reports them: [`summary`] names `MAX_HITS` when it says the sweep was
/// truncated, and a send site that chose its own cap would make that sentence
/// a lie.
///
/// An all-whitespace pattern returns `None`. A literal sweep for `" "` would
/// match nearly every line of every ring, which is 200 MiB of scanning to
/// produce an answer nobody asked for.
pub fn request(query: &str, options: Options, sessions: Vec<SessionId>) -> Option<ClientMsg> {
    let pattern = query.trim();
    if pattern.is_empty() {
        return None;
    }
    Some(ClientMsg::Search {
        sessions,
        pattern: pattern.to_string(),
        regex: options.regex,
        case_insensitive: options.case_insensitive,
        whole_word: options.whole_word,
        context_lines: CONTEXT_LINES,
        max_hits: MAX_HITS,
    })
}

/// The switch's exact class string.
///
/// Pulled out of the markup so the emitted names are testable: the stylesheet
/// keys the on-state off `--on`, and a typo would render a switch that never
/// looks pressed, with no error anywhere.
pub fn opt_class(on: bool) -> &'static str {
    if on {
        "rg-search__opt rg-search__opt--on"
    } else {
        "rg-search__opt"
    }
}

/// The highlight's exact class string. See [`Split::empty_mark`].
pub fn mark_class(empty: bool) -> &'static str {
    if empty {
        "rg-search__mark rg-search__mark--empty"
    } else {
        "rg-search__mark"
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct SearchProps {
    /// What is in the field. Owned by the caller so reopening the layer can
    /// restore the last query instead of clearing it.
    pub query: String,
    pub options: Options,
    /// The last answer, or `None` when nothing has been swept. The two render
    /// differently; see [`summary`].
    pub answer: Option<Answer>,
    /// True between the send and the answer. A 200 MiB sweep is 84 to 96 ms
    /// of daemon CPU, which is long enough for "No matches" to flash on
    /// screen and be read as the answer.
    pub searching: bool,
    /// How many sessions the sweep is restricted to, zero for all of them.
    /// Comes from the sidebar selection at the moment the sweep was sent.
    pub scope: usize,
    /// Session titles for the group headings. A session missing here is
    /// headed by its id rather than left blank.
    pub titles: Vec<(SessionId, String)>,
    pub on_query: EventHandler<String>,
    pub on_toggle: EventHandler<Toggle>,
    /// Run the sweep. Fired by Enter in the field.
    pub on_submit: EventHandler<()>,
    /// Take me to this line: the session, and the byte offset of the matched
    /// line's first byte in that session's cumulative output.
    pub on_activate: EventHandler<(SessionId, u64)>,
    pub on_dismiss: EventHandler<()>,
}

#[component]
pub fn Search(props: SearchProps) -> Element {
    let summary = summary(props.answer.as_ref(), props.searching, props.scope);
    let groups = match props.answer.as_ref() {
        Some(answer) => group_by_session(&answer.hits, &props.titles),
        None => Vec::new(),
    };

    rsx! {
        div {
            class: "rg-layer rg-layer--dim rg-search-layer",
            onclick: move |_| props.on_dismiss.call(()),
            div {
                class: "rg-search",
                role: "dialog",
                aria_label: "Search scrollback",
                onclick: move |e| e.stop_propagation(),

                div { class: "rg-search__head",
                    span { class: "rg-search__title", "Search scrollback" }
                    button {
                        class: "rg-search__close",
                        r#type: "button",
                        onclick: move |_| props.on_dismiss.call(()),
                        "Close"
                    }
                }

                // One element, not a wrapper holding a magnifier and an
                // input. The placeholder says what the field is and how it
                // submits, which is what the glyph and the button used to do
                // between them.
                input {
                    class: "rg-search__input",
                    id: "rg-search-input",
                    r#type: "text",
                    placeholder: "Pattern, then Enter",
                    value: "{props.query}",
                    // The caret goes here as the layer opens. It has to be done
                    // on mount rather than by the shell when it handles the
                    // chord: the element does not exist yet at that point, so a
                    // focus command issued there finds nothing and the operator
                    // types into whatever had focus before. Inside a live pane
                    // that means the pattern is fed to the child process.
                    onmounted: move |e| {
                        spawn(async move {
                            let _ = e.set_focus(true).await;
                        });
                    },
                    // Escape is deliberately not handled. The bridge matches
                    // the shell's chord table on `window` in the capture
                    // phase and claims it for Dismiss before any handler here
                    // could run.
                    oninput: move |e| props.on_query.call(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter {
                            props.on_submit.call(());
                            e.prevent_default();
                        }
                    },
                }

                div { class: "rg-search__opts",
                    for which in Toggle::ALL {
                        button {
                            key: "{which:?}",
                            class: "{opt_class(props.options.is_on(which))}",
                            r#type: "button",
                            aria_pressed: "{props.options.is_on(which)}",
                            onclick: move |_| props.on_toggle.call(which),
                            "{which.label()}"
                        }
                    }
                }

                div { class: "{summary.class}", "{summary.text}" }

                if !groups.is_empty() {
                    div { class: "rg-search__results",
                        for group in groups.iter() {
                            div { class: "rg-search__group", key: "{group.session.0}",
                                div { class: "rg-search__group-head",
                                    span { class: "rg-search__group-name", "{group.label}" }
                                    span { class: "rg-search__group-count",
                                        "{matches_word(group.hits.len() as u64)}"
                                    }
                                }
                                for (index , hit) in group.hits.iter().enumerate() {
                                    Hit {
                                        key: "{hit.line_seq}:{index}",
                                        session: hit.session,
                                        line_seq: hit.line_seq,
                                        before: hit.before.iter().map(|l| line_text(l)).collect(),
                                        split: split_hit(
                                            &hit.visible,
                                            hit.match_start,
                                            hit.match_end,
                                        ),
                                        after: hit.after.iter().map(|l| line_text(l)).collect(),
                                        on_activate: props.on_activate,
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

#[derive(Props, Clone, PartialEq)]
struct HitProps {
    session: SessionId,
    line_seq: u64,
    /// The context lines, already decoded.
    ///
    /// Decoded by the caller rather than here, and that is the difference
    /// between this row costing its text and costing its text plus a copy of
    /// the raw bytes. The props used to be the whole [`SearchHit`], so every
    /// render of the layer deep-copied `visible` plus every context line of
    /// every hit, up to [`MAX_HITS`] of them, and then decoded them anyway.
    /// Nothing in the markup ever wanted the bytes.
    before: Vec<String>,
    split: Split,
    after: Vec<String>,
    on_activate: EventHandler<(SessionId, u64)>,
}

/// One matched line with its context.
///
/// A `button` holding only phrasing content, so the whole row is one keyboard
/// stop and one pointer target rather than a div wearing `role="button"` and
/// a hand-written key handler.
#[component]
fn Hit(props: HitProps) -> Element {
    let split = &props.split;
    let session = props.session;
    let line_seq = props.line_seq;
    let on_activate = props.on_activate;

    rsx! {
        button {
            class: "rg-search__hit",
            r#type: "button",
            title: "Jump to this line (byte {count::grouped(line_seq)} of this session's output)",
            onclick: move |_| on_activate.call((session, line_seq)),

            for (index , line) in props.before.iter().enumerate() {
                span { class: "rg-search__ctx", key: "b{index}", "{line}" }
            }
            span { class: "rg-search__line",
                span { class: "rg-search__pre", "{split.before}" }
                span { class: "{mark_class(split.empty_mark)}", "{split.matched}" }
                span { class: "rg-search__post", "{split.after}" }
            }
            for (index , line) in props.after.iter().enumerate() {
                span { class: "rg-search__ctx", key: "a{index}", "{line}" }
            }
        }
    }
}

#[cfg(test)]
mod tests;

/// Render smoke tests.
///
/// Everything in `search.rs`'s own test module exercises pure functions and
/// stylesheet text. Nothing there has ever built the markup, so a panic in the
/// RSX, a bad `key`, or a highlight that is computed correctly and then
/// dropped on the floor by the markup would all pass 25 green tests.
///
/// These render the real component through `dioxus-ssr` and assert on the HTML
/// that comes out.
#[cfg(test)]
mod render;
