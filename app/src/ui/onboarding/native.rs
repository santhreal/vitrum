//! The first-launch walkthrough, as a presented surface.
//!
//! Every sentence, every chord and every page on this surface comes from
//! [`super::pages`], which takes the three readings in [`Machine`] and returns
//! data. This file turns that data into widgets and owns one piece of state,
//! which is the page showing.
//!
//! # Why the machine is read late
//!
//! [`crate::launch::detected_agents`] walks `PATH` once per known agent, and a
//! first-run sheet that appears blank while that finishes is a first-run sheet
//! that looks broken. The deck is therefore built from what is known
//! immediately and rebuilt when the walk lands, which can only ever change the
//! first page: the three after it are constant.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use super::{Machine, Page, StepState, finish_label, pages};
use crate::shell::{Dialog, Shell};
use crate::ui::sheet::{self, Sheet};

/// The walkthrough.
pub(crate) struct Onboarding {
    shell: Shell,
    frame: Rc<Sheet>,
    machine: RefCell<Machine>,
    deck: RefCell<Vec<Page>>,
    at: Cell<usize>,
    title: gtk::Label,
    blurb: gtk::Label,
    steps: gtk::Box,
    dots: gtk::Box,
    back: gtk::Button,
    next: gtk::Button,
}

/// Build the sheet and start reading the machine.
pub(crate) fn build(shell: &Shell) -> Rc<Onboarding> {
    let machine = shell.peek(|st| Machine {
        agents: None,
        connected: st.daemon.conn.is_live(),
        any_session: !st.daemon.sessions.is_empty(),
    });

    let panel = sheet::column("rg-sheet__panel");
    let head = sheet::row("rg-sheet__head");
    let title = sheet::label("rg-sheet__title", "");
    title.set_hexpand(true);
    head.pack_start(&title, true, true, 0);
    let skip = gtk::Button::with_label("Skip");
    skip.style_context().add_class("rg-btn-inline");
    head.pack_end(&skip, false, false, 0);
    panel.pack_start(&head, false, false, 0);

    let body = sheet::column("rg-sheet__body");
    let blurb = sheet::label("rg-onboard__intro", "");
    body.pack_start(&blurb, false, false, 0);
    let steps = sheet::column("rg-onboard__steps");
    body.pack_start(&steps, false, false, 0);
    panel.pack_start(&body, true, true, 0);

    let foot = sheet::row("rg-sheet__foot rg-onboard__foot");
    // The position indicator is decorative: the page heading already says
    // where the operator is, and four dots announce nothing a reader needs.
    let dots = sheet::row("rg-onboard__dots");
    dots.set_hexpand(true);
    foot.pack_start(&dots, true, true, 0);
    let back = gtk::Button::with_label("Back");
    back.style_context().add_class("rg-btn");
    foot.pack_end(&back, false, false, 0);
    let next = gtk::Button::new();
    next.style_context().add_class("rg-btn");
    next.style_context().add_class("rg-btn--primary");
    foot.pack_end(&next, false, false, 0);
    panel.pack_start(&foot, false, false, 0);

    let frame = Sheet::new(sheet::ONBOARDING, sheet::DOCUMENT, &panel);
    let me = Rc::new(Onboarding {
        shell: shell.clone(),
        frame,
        deck: RefCell::new(pages(&machine)),
        machine: RefCell::new(machine),
        at: Cell::new(0),
        title,
        blurb,
        steps,
        dots,
        back: back.clone(),
        next: next.clone(),
    });

    // However it closes, it is recorded as seen, and it is recorded in one
    // place: `Dialog::dismissed`. A first-run sheet that comes back because
    // you dismissed it rather than finished it is a sheet that punishes you
    // for not reading it, and two routes out are two chances to forget one.
    skip.connect_clicked({
        let shell = shell.clone();
        move |_| shell.dismiss()
    });
    back.connect_clicked({
        let me = Rc::downgrade(&me);
        move |_| {
            if let Some(me) = me.upgrade() {
                me.at.set(me.at.get().saturating_sub(1));
                me.draw();
            }
        }
    });
    next.connect_clicked({
        let me = Rc::downgrade(&me);
        move |_| {
            let Some(me) = me.upgrade() else { return };
            if me.on_last() {
                me.shell.dismiss();
            } else {
                me.at.set(me.at.get() + 1);
                me.draw();
            }
        }
    });

    me.draw();
    me.read_machine();
    me
}

impl Onboarding {
    /// Is the last page showing?
    fn on_last(&self) -> bool {
        self.at.get() >= self.deck.borrow().len().saturating_sub(1)
    }

    /// Walk `PATH` on a thread and rebuild the deck when it lands.
    fn read_machine(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let found = crate::ui::dialog::off_thread(crate::launch::detected_agents).await;
            let Some(me) = me.upgrade() else { return };
            me.machine.borrow_mut().agents = Some(found);
            *me.deck.borrow_mut() = pages(&me.machine.borrow());
            me.draw();
        });
    }

    /// Put the current page on screen.
    fn draw(&self) {
        let deck = self.deck.borrow();
        let last = deck.len().saturating_sub(1);
        let index = self.at.get().min(last);
        self.at.set(index);
        let page = &deck[index];

        self.title.set_text(&page.title);
        self.blurb.set_text(&page.blurb);

        for child in self.steps.children() {
            self.steps.remove(&child);
        }
        for step in &page.rows {
            let row = sheet::column(step_class(step.state));
            row.pack_start(
                &sheet::label("rg-onboard__step-title", &step.title),
                false,
                false,
                0,
            );
            row.pack_start(
                &sheet::label("rg-onboard__step-body", &step.body),
                false,
                false,
                0,
            );
            self.steps.pack_start(&row, false, false, 0);
        }
        self.steps.show_all();

        for child in self.dots.children() {
            self.dots.remove(&child);
        }
        for n in 0..deck.len() {
            let dot = sheet::row(if n == index {
                "rg-onboard__dot rg-onboard__dot--on"
            } else {
                "rg-onboard__dot"
            });
            self.dots.pack_start(&dot, false, false, 0);
        }
        self.dots.show_all();

        self.back.set_visible(index > 0);
        self.next.set_label(if index == last {
            finish_label(&self.machine.borrow())
        } else {
            "Next"
        });
    }
}

impl Dialog for Onboarding {

    fn root(&self) -> gtk::Widget {
        self.frame.root()
    }

    /// Record that this operator has seen the walkthrough.
    ///
    /// Finished and skipped persist identically, which is the rule the module
    /// above states, so this is one route rather than a branch on how the
    /// sheet was closed.
    fn dismissed(&self) {
        self.shell.update(|st| {
            st.daemon
                .settings
                .finish_onboarding(&crate::update::current_version());
            st.window.layer = crate::state::Layer::None;
        });
        self.shell.peek(crate::ui::settings::commit);
    }
}

/// The class one step row carries, by how the step stands.
///
/// A function rather than three literals at the call site, so the modifier
/// spelling is in one place and a test can read it.
pub(crate) fn step_class(state: StepState) -> &'static str {
    match state {
        StepState::Done => "rg-onboard__step rg-onboard__step--done",
        StepState::Todo => "rg-onboard__step rg-onboard__step--todo",
        StepState::Info => "rg-onboard__step rg-onboard__step--info",
    }
}

/// How much room the tallest page of `machine`'s walkthrough wants, in rem.
///
/// The tallest page, not the current one. The sheet is built once and the
/// pages are swapped inside it, so a size taken from page one would slice page
/// three the moment the operator pressed Next.
#[cfg(test)]
pub(crate) fn content(machine: &Machine) -> (f64, f64) {
    let tallest = pages(machine)
        .iter()
        .map(|page| page.rows.len())
        .max()
        .unwrap_or(0);
    (0.0, HEAD_REM + BLURB_REM + tallest as f64 * STEP_REM + FOOT_REM)
}

/// The sheet's head, in rem.
#[cfg(test)]
const HEAD_REM: f64 = 2.5;

/// The sentence under the title, in rem.
#[cfg(test)]
const BLURB_REM: f64 = 3.0;

/// One step row: a title line and a body that wraps, in rem.
#[cfg(test)]
const STEP_REM: f64 = 4.5;

/// The dots and the two controls, in rem.
#[cfg(test)]
const FOOT_REM: f64 = 2.5;

#[cfg(test)]
mod tests;
