//! The transient notice, floated over the frame.
//!
//! # Why it takes no layout space
//!
//! The previous notice was a strip in the layout above the terminal. A strip
//! that appears when there is something to say and vanishes when there is not
//! takes a line away from the pty, which resizes it, which makes every agent
//! in every session repaint its whole screen. One sentence about one session
//! therefore flashed every agent on screen twice: once arriving and once
//! leaving. So this is an overlay child, positioned by the toolkit over the
//! frame and outside every allocation, and the pane cannot observe it at all.
//!
//! # Why it does not flap
//!
//! [`step`] compares what is being shown against what the state now holds and
//! answers [`Step::Hold`] when they are equal. A fan-out happens on every
//! daemon message, so a toast rebuilt on each one would restart its own
//! entrance and its own retirement timer several times a second: a notice that
//! never finishes arriving and never expires. Holding is the whole rule.
//!
//! Speaking once is the state's job and is already done there:
//! `WindowState::history_refused` records the exact window a refusal was about,
//! so the second attempt raises nothing. This module must not add a second
//! memory of what it has said, because two of them disagree the first time a
//! genuinely new notice repeats an old sentence.
//!
//! # Why it is dismissible
//!
//! An error never retires by itself, and neither does a notice on a profile
//! whose configured life is zero, which is how somebody who reads slowly asks
//! to close their own notices. Without a control, both are permanent.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::Tick;
use crate::shell::{Observer, Shell};
use crate::state::{Flash, FlashKind, UiState};

/// What the toast should do about the state it was just handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing to say, and nothing on screen.
    Idle,
    /// Put this on screen and start its life.
    Raise(Flash),
    /// The same notice is already up. Touch nothing.
    Hold,
    /// Take down what is up.
    Retire,
}

/// What to do, from the state and from what is already on screen.
///
/// Pure, because the anti-flap rule is the entire behaviour of this surface
/// and it has to be provable without a toolkit.
pub(crate) fn step(now: Option<&Flash>, showing: Option<&Flash>) -> Step {
    match (now, showing) {
        (None, None) => Step::Idle,
        (None, Some(_)) => Step::Retire,
        (Some(next), Some(current)) if next == current => Step::Hold,
        (Some(next), _) => Step::Raise(next.clone()),
    }
}

/// How long `flash` stays up, or `None` for "until it is dismissed".
///
/// An error reports something the operator has to act on, and a failure that
/// erases itself before it is read is worse than one that stays. A notice is a
/// confirmation, and a confirmation that never leaves is permanent chrome
/// worded like news: "Started an agent." was still on screen twenty-nine
/// minutes after the session started.
pub(crate) fn life(flash: &Flash, configured: Option<u64>) -> Option<u64> {
    match flash.kind {
        FlashKind::Error => None,
        FlashKind::Notice => configured,
    }
}

/// The class that paints one kind of notice.
pub(crate) fn class(kind: FlashKind) -> &'static str {
    match kind {
        FlashKind::Error => "rg-toast--error",
        FlashKind::Notice => "rg-toast--notice",
    }
}

/// The floated notice for one window.
pub(crate) struct Toast {
    shell: Shell,
    root: gtk::Box,
    text: gtk::Label,
    /// Carries out the destructive action the notice is asking about. Present
    /// only while something is armed.
    confirm: gtk::Button,
    /// What is on screen, so a fan-out that changed nothing changes nothing.
    showing: RefCell<Option<Flash>>,
    /// The retirement in flight, cancelled when the notice is replaced. A
    /// timer left running would clear a later notice at the earlier one's
    /// deadline. Shared with the timer itself so a source that has already
    /// fired drops its own handle, and cancelling never names a dead source.
    timer: Rc<RefCell<Option<glib::SourceId>>>,
}

impl Toast {
    /// Build the toast, float it over `shell`'s frame, and start listening.
    pub(crate) fn install(shell: &Shell) -> Rc<Self> {
        let root = crate::ui::sheet::row("rg-toast");
        // Over the bottom of the frame, sized to its own text. Set here rather
        // than in the sheet because a notice is the one surface that is not
        // centred on the window: it sits where a notice sits on every desktop
        // this product runs on.
        root.set_halign(gtk::Align::Center);
        root.set_valign(gtk::Align::End);
        root.set_no_show_all(true);

        let text = crate::ui::sheet::label("rg-toast__text", "");
        text.set_hexpand(true);
        root.pack_start(&text, true, true, 0);

        let confirm = gtk::Button::with_label("Terminate");
        confirm.style_context().add_class("rg-btn-inline");
        confirm.style_context().add_class("rg-btn-inline--danger");
        confirm.set_no_show_all(true);
        root.pack_end(&confirm, false, false, 0);

        let dismiss = gtk::Button::with_label("Dismiss");
        dismiss.style_context().add_class("rg-btn-inline");
        root.pack_end(&dismiss, false, false, 0);

        let toast = Rc::new(Self {
            shell: shell.clone(),
            root: root.clone(),
            text,
            confirm: confirm.clone(),
            showing: RefCell::new(None),
            timer: Rc::new(RefCell::new(None)),
        });

        dismiss.connect_clicked({
            let shell = shell.clone();
            move |_| {
                // Dismissing the prompt disarms it. A confirmation that
                // survives the sentence explaining it is a trap.
                shell.update(|st| {
                    st.window.flash = None;
                    st.window.armed_terminate.clear();
                });
            }
        });
        confirm.connect_clicked({
            let shell = shell.clone();
            move |_| {
                // The same event the menu entry raises. Sent a second time
                // with the same targets and the same notice on screen, the
                // reducer reads it as the answer to its own prompt and
                // terminates rather than asking again.
                let targets = shell.peek(|st| st.window.armed_terminate.clone());
                shell.send(crate::wire::ClientEvent::Terminate { targets });
            }
        });

        shell.float(&root.clone().upcast());
        shell.observe(toast.clone() as Rc<dyn Observer>);
        toast
    }

    /// Put `flash` on screen and arm its retirement.
    fn raise(&self, flash: &Flash, configured: Option<u64>) {
        self.cancel();
        let context = self.root.style_context();
        context.remove_class(class(FlashKind::Error));
        context.remove_class(class(FlashKind::Notice));
        context.add_class(class(flash.kind));
        self.text.set_text(&flash.text);
        self.root.show();
        self.text.show();
        *self.showing.borrow_mut() = Some(flash.clone());

        let Some(life) = life(flash, configured) else {
            return;
        };
        let shell = self.shell.clone();
        let mine = flash.clone();
        let slot = self.timer.clone();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(life), move || {
            // The source is spent, so drop the handle before anything can try
            // to cancel a source that no longer exists.
            *slot.borrow_mut() = None;
            // Scoped to the exact notice it was raised for, so a later notice
            // keeps its full life.
            if shell.peek(|st| st.window.flash.as_ref() == Some(&mine)) {
                shell.update(|st| st.window.flash = None);
            }
        });
        *self.timer.borrow_mut() = Some(id);
    }

    /// Take the notice down.
    fn retire(&self) {
        self.cancel();
        self.root.hide();
        *self.showing.borrow_mut() = None;
    }

    /// Drop the retirement in flight, if there is one.
    fn cancel(&self) {
        if let Some(id) = self.timer.borrow_mut().take() {
            id.remove();
        }
    }
}

impl Observer for Toast {
    fn state_changed(&self, state: &UiState, _at: Tick) {
        // The confirm control exists only while something is armed. Shown and
        // hidden rather than added and removed: the toast is floated, so
        // neither costs the pane a pixel, and a widget that is built once
        // cannot be built twice by a fan-out.
        if state.window.armed_terminate.is_empty() {
            self.confirm.hide();
        } else {
            self.confirm.show();
        }

        let showing = self.showing.borrow().clone();
        match step(state.window.flash.as_ref(), showing.as_ref()) {
            Step::Idle | Step::Hold => {}
            Step::Retire => self.retire(),
            Step::Raise(flash) => {
                self.raise(&flash, state.daemon.settings.notices.flash_life_ms())
            }
        }
    }
}

#[cfg(test)]
mod tests;
