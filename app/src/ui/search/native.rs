//! Cross-session scrollback search, as a presented surface.
//!
//! The byte discipline the rest of this module documents is unchanged and is
//! the reason this file draws three labels per matched line rather than one
//! with markup in it: the offsets the daemon sent index the RAW bytes, and
//! [`super::split_hit`] cuts them there and decodes each piece separately. A
//! single decoded string with a highlight range would put the highlight on the
//! wrong substring for exactly the lines somebody is searching for, which are
//! the ones with a stray byte in them.
//!
//! # Why this surface observes
//!
//! A sweep is 84 to 96 ms of daemon work over 200 MiB, so the answer lands
//! long after the sheet opened. The sheet is therefore in the fan-out and
//! redraws its results when the answer changes, and only then: a redraw on
//! every daemon message would rebuild five hundred rows several times a second
//! under the operator's pointer.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use vitrum_proto::SessionId;

use super::{
    Answer, Toggle, group_by_session, line_text, mark_class, matches_word, opt_class, request,
    split_hit, summary,
};
use crate::Tick;
use crate::shell::{Dialog, Observer, Shell};
use crate::state::UiState;
use crate::ui::sheet::{self, Sheet};
use crate::wire::ClientEvent;

/// The search sheet: a field, three switches, an honest summary, and the hits.
pub(crate) struct SearchSurface {
    shell: Shell,
    frame: Rc<Sheet>,
    field: gtk::Entry,
    switches: Vec<(Toggle, gtk::Button)>,
    summary: gtk::Label,
    results: gtk::Box,
    /// The answer currently drawn, so an unchanged one is not drawn again.
    drawn: RefCell<Option<Answer>>,
    /// This surface as the fan-out holds it, so it can take itself back out.
    handle: RefCell<Weak<Self>>,
}

/// Build the surface and register it with the fan-out.
pub(crate) fn build(shell: &Shell) -> Rc<SearchSurface> {
    let panel = sheet::column("rg-sheet__panel");
    panel.pack_start(&sheet::head(shell, "Search scrollback"), false, false, 0);

    // One element, not a wrapper holding a magnifier and an input. The
    // placeholder says what the field is and how it submits, which is what the
    // glyph and the removed Search button used to do between them.
    let field = gtk::Entry::new();
    field.style_context().add_class("rg-search__input");
    field.set_placeholder_text(Some("Pattern, then Enter"));
    panel.pack_start(&field, false, false, 0);

    let opts = sheet::row("rg-search__opts");
    let mut switches = Vec::new();
    for which in Toggle::ALL {
        let button = gtk::Button::with_label(which.label());
        opts.pack_start(&button, false, false, 0);
        switches.push((which, button));
    }
    panel.pack_start(&opts, false, false, 0);

    let summary = sheet::label("rg-search__summary", "");
    panel.pack_start(&summary, false, false, 0);

    let results = sheet::column("rg-search__results");
    panel.pack_start(&results, true, true, 0);

    let frame = Sheet::new(sheet::SEARCH, sheet::DOCUMENT, &panel);
    let surface = Rc::new(SearchSurface {
        shell: shell.clone(),
        frame,
        field: field.clone(),
        switches: switches.clone(),
        summary,
        results,
        drawn: RefCell::new(None),
        handle: RefCell::new(Weak::new()),
    });
    *surface.handle.borrow_mut() = Rc::downgrade(&surface);

    // The field owns the text while somebody is typing, and the state follows
    // it. Pushing the state back into the field on every fan-out would roll
    // back whatever was typed since the last one; `state_changed` writes the
    // field only when the two genuinely differ, which is a reopen and nothing
    // else.
    field.connect_changed({
        let shell = shell.clone();
        move |entry| {
            let text = entry.text().to_string();
            shell.update(move |st| st.window.search.query = text);
        }
    });
    field.connect_activate({
        let shell = shell.clone();
        move |_| sweep(&shell)
    });
    // The caret goes here as the sheet appears. Done on map rather than by
    // whoever handled the chord, because the widget does not exist yet at that
    // point and a focus issued there lands on whatever had it before, which
    // inside a live pane means the pattern is typed at the child process.
    field.connect_map(|entry| {
        entry.grab_focus();
    });

    for (which, button) in &switches {
        let shell = shell.clone();
        let which = *which;
        button.connect_clicked(move |_| {
            shell.update(move |st| {
                st.window.search.options = st.window.search.options.toggled(which);
            });
        });
    }

    shell.observe(surface.clone() as Rc<dyn Observer>);
    surface
}

/// Send the sweep for whatever is in the field.
///
/// Scoped to the sidebar selection when there is one. One selected row is not
/// a scope: that is where the cursor happens to be, and narrowing to it would
/// make the everyday case silently local.
///
/// [`request`] owns the hit cap and the context width, because the summary
/// quotes the cap back to the operator and a send site that picked its own
/// number would make that sentence name a value never sent. It also refuses an
/// all-whitespace pattern, which would match nearly every line of every ring.
pub(crate) fn sweep(shell: &Shell) {
    let (query, options, scope) = shell.peek(|st| {
        let scope: Vec<SessionId> = if st.window.selection.len() > 1 {
            st.window.selection.iter().collect()
        } else {
            Vec::new()
        };
        (st.window.search.query.clone(), st.window.search.options, scope)
    });
    let Some(msg) = request(&query, options, scope.clone()) else {
        return;
    };
    shell.update(move |st| {
        st.window.search.searching = true;
        st.window.search.scope = scope;
    });
    shell.send(ClientEvent::Msg { msg });
}

impl SearchSurface {
    /// Draw `answer`'s hits, headed by session.
    fn draw(&self, state: &UiState, answer: Option<&Answer>) {
        for child in self.results.children() {
            self.results.remove(&child);
        }
        let Some(answer) = answer else { return };
        let titles: Vec<(SessionId, String)> = state
            .daemon
            .sessions
            .iter()
            .map(|row| (row.id(), row.info.title.clone()))
            .collect();
        for group in group_by_session(&answer.hits, &titles) {
            let block = sheet::column("rg-search__group");
            let head = sheet::row("rg-search__group-head");
            head.pack_start(
                &sheet::label("rg-search__group-name", &group.label),
                false,
                false,
                0,
            );
            head.pack_end(
                &sheet::label(
                    "rg-search__group-count",
                    &matches_word(group.hits.len() as u64),
                ),
                false,
                false,
                0,
            );
            block.pack_start(&head, false, false, 0);
            for hit in group.hits {
                block.pack_start(&self.hit(hit), false, false, 0);
            }
            self.results.pack_start(&block, false, false, 0);
        }
        self.results.show_all();
    }

    /// One matched line with its context, as one keyboard stop.
    fn hit(&self, hit: &vitrum_proto::SearchHit) -> gtk::Button {
        let lines = sheet::column("rg-search__hit");
        for raw in &hit.before {
            lines.pack_start(&sheet::label("rg-search__ctx", &line_text(raw)), false, false, 0);
        }

        let split = split_hit(&hit.visible, hit.match_start, hit.match_end);
        let line = sheet::row("rg-search__line");
        line.pack_start(&sheet::label("rg-search__pre", &split.before), false, false, 0);
        line.pack_start(
            &sheet::label(mark_class(split.empty_mark), &split.matched),
            false,
            false,
            0,
        );
        line.pack_start(&sheet::label("rg-search__post", &split.after), false, false, 0);
        lines.pack_start(&line, false, false, 0);

        for raw in &hit.after {
            lines.pack_start(&sheet::label("rg-search__ctx", &line_text(raw)), false, false, 0);
        }

        let button = gtk::Button::new();
        button.add(&lines);
        button.set_tooltip_text(Some(&format!(
            "Jump to this line (byte {} of this session's output)",
            vitrum_fmt::count::grouped(hit.line_seq)
        )));

        let shell = self.shell.clone();
        let session = hit.session;
        let line_seq = hit.line_seq;
        button.connect_clicked(move |_| {
            // The intent is recorded BEFORE the focus moves: opening clears the
            // history anchor, and the request is anchored on the hit rather
            // than on the head of the buffer. Without it the surface promised
            // "jump to this line" and left the operator wherever the usual
            // head-anchored paint stopped, which for an hour-old hit is
            // nowhere near it.
            let now = crate::tick().now_ms;
            shell.update(move |st| {
                st.open(session, now);
                st.window.history_intent = crate::state::HistoryIntent::Jump(line_seq);
                st.window.layer = crate::state::Layer::None;
            });
            shell.dismiss();
        });
        button
    }
}

impl Dialog for SearchSurface {

    fn root(&self) -> gtk::Widget {
        self.frame.root()
    }

    fn dismissed(&self) {
        // Out of the fan-out. Every search sheet ever opened staying in it
        // would make a window slower the longer it had been used.
        if let Some(me) = self.handle.borrow().upgrade() {
            self.shell.unobserve(&(me as Rc<dyn Observer>));
        }
    }
}

impl Observer for SearchSurface {
    fn state_changed(&self, state: &UiState, _at: Tick) {
        let search = &state.window.search;

        // Only on a genuine difference. Writing the field on every fan-out
        // would roll back the characters typed since the last one.
        if self.field.text() != search.query.as_str() {
            self.field.set_text(&search.query);
        }

        for (which, button) in &self.switches {
            sheet::set_classes(
                &button.style_context(),
                opt_class(search.options.is_on(*which)),
            );
        }

        let summary = summary(search.answer.as_ref(), search.searching, search.scope.len());
        self.summary.set_text(&summary.text);
        sheet::set_classes(&self.summary.style_context(), summary.class);

        // Five hundred rows are not rebuilt because a session's title changed.
        if needs_redraw(self.drawn.borrow().as_ref(), search.answer.as_ref()) {
            self.draw(state, search.answer.as_ref());
            *self.drawn.borrow_mut() = search.answer.clone();
        }
    }
}

/// Is the answer on screen out of date?
///
/// Split out and pure because it is the whole flicker rule for this surface.
/// The state fans out on every daemon message and this sheet holds up to five
/// hundred rows; rebuilding them because a session title changed is a list
/// that flashes under the operator's pointer several times a second.
pub(crate) fn needs_redraw(drawn: Option<&Answer>, now: Option<&Answer>) -> bool {
    drawn != now
}

/// How much room the results want, in rem, for `answer`.
///
/// Every hit is its matched line plus the configured context either side
/// ([`super::context_lines`]), and every session that answered adds a
/// heading. Counted rather than guessed, because the answer is capped
/// ([`super::max_hits`]) and that cap is far taller than any window.
#[cfg(test)]
pub(crate) fn content(answer: Option<&Answer>) -> (f64, f64) {
    let hits = answer.map_or(0, |a| a.hits.len());
    let sessions = answer.map_or(0, |a| {
        let mut ids: Vec<SessionId> = a.hits.iter().map(|hit| hit.session).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    });
    let rows = hits * (1 + 2 * super::context_lines() as usize) + sessions;
    // Head, field, switches and summary.
    (sheet::DOCUMENT.width, 10.0 + rows as f64 * 1.5)
}

#[cfg(test)]
mod tests;
