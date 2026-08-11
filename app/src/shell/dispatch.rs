//! One window's state, and the fan-out that tells the window about it.
//!
//! No toolkit here, deliberately. What a change to [`UiState`] costs, in what
//! order the observers hear about it, and what happens when one of them
//! answers by changing the state again are all decided by this file, and all
//! of it is checkable on a machine with no display.
//!
//! # Reentrancy, which is the whole reason this is a type
//!
//! A widget callback is entitled to change the state. So is an observer being
//! told the state changed. The state is behind a `RefCell` that is borrowed
//! for the length of a fan-out, so the second case cannot take a mutable
//! borrow, and the naive answer is a panic in a GTK callback, which is an
//! abort with a stack trace nobody can act on.
//!
//! Instead a mutation raised during a fan-out is queued and applied when the
//! fan-out ends, and the observers are told again. That terminates because
//! each pass applies the queue that existed when it started; an observer that
//! answers every change with another change is a live-lock this cannot fix
//! and must not hide, so the pass count is counted and reported.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::Tick;
use crate::state::UiState;
use crate::state::live::ShellSettings;

/// Anything that wants to hear about this window's state.
///
/// Split out of [`super::Panel`] because a panel also owns a widget, and a
/// widget cannot be built without a display. Every rule about ordering and
/// reentrancy is stated against this trait so the rules can be tested.
pub(crate) trait Observer {
    /// The state changed, or the observer was just added.
    ///
    /// `at` is one reading of the clock shared by every observer in this
    /// fan-out. Two readings inside one fan-out would let two panels disagree
    /// about whether the same instant is "59s ago" or "1m ago".
    fn state_changed(&self, state: &UiState, at: Tick);

    /// The operator changed a setting the frame reads.
    fn settings_changed(&self, _settings: &ShellSettings) {}
}

/// A mutation raised while the state was borrowed for a fan-out.
type Queued = Box<dyn FnOnce(&mut UiState)>;

/// The state, the observers, and the rules connecting them.
pub(crate) struct Dispatch {
    state: RefCell<UiState>,
    observers: RefCell<Vec<Rc<dyn Observer>>>,
    notifying: Cell<bool>,
    pending: RefCell<Vec<Queued>>,
    /// Fan-out passes since this window opened.
    ///
    /// A counter and not a timer: what a change is allowed to cost is a
    /// property of this code, and one change that produces three passes is a
    /// defect a test can see.
    passes: Cell<usize>,
    /// How many [`Dispatch::batch`] calls are open.
    ///
    /// One event from the operator is one repaint. A reducer arm changes the
    /// state four or five times on its way through, and telling every panel
    /// after each one repaints the window five times for one key press. The
    /// batch holds the fan-out until the whole event has been folded in.
    held: Cell<usize>,
    /// Whether anything asked for a fan-out while the batch was held.
    dirty: Cell<bool>,
}

impl Dispatch {
    pub(crate) fn new(state: UiState) -> Self {
        Self {
            state: RefCell::new(state),
            observers: RefCell::new(Vec::new()),
            notifying: Cell::new(false),
            pending: RefCell::new(Vec::new()),
            passes: Cell::new(0),
            held: Cell::new(0),
            dirty: Cell::new(false),
        }
    }

    /// Add `observer` and tell it the state immediately.
    ///
    /// Immediately, so a panel is correct before it is ever seen. A panel
    /// that mounted empty and filled on the next change would show an empty
    /// list for as long as nothing happened, which on a quiet daemon is
    /// forever.
    pub(crate) fn watch(&self, observer: Rc<dyn Observer>) {
        self.observers.borrow_mut().push(Rc::clone(&observer));
        // Under the same guard a fan-out uses. This call holds a shared
        // borrow of the state, and an observer is entitled to answer it by
        // changing the state; without the guard that is a mutable borrow
        // taken while this one is live, which is a panic inside a widget
        // callback and therefore an abort.
        let nested = self.notifying.replace(true);
        {
            let state = self.state.borrow();
            observer.state_changed(&state, crate::tick());
        }
        self.notifying.set(nested);
        if !nested && !self.pending.borrow().is_empty() {
            self.notify();
        }
    }

    /// Stop telling `observer` about anything.
    ///
    /// By pointer identity, because two observers can be equal in every field
    /// and still be two surfaces. Safe during a fan-out: the pass already
    /// holds its own list, so the removal takes effect on the next one.
    ///
    /// Without this a dismissed dialog stays in the fan-out for the life of
    /// the window, and a window whose operator opened the launcher twenty
    /// times repaints twenty dead sheets on every daemon message.
    pub(crate) fn unwatch(&self, observer: &Rc<dyn Observer>) {
        self.observers
            .borrow_mut()
            .retain(|held| !Rc::ptr_eq(held, observer));
    }

    /// Read the state without holding a borrow past the call.
    pub(crate) fn peek<R>(&self, f: impl FnOnce(&UiState) -> R) -> R {
        f(&self.state.borrow())
    }

    /// Change the state and tell every observer.
    pub(crate) fn update(&self, f: impl FnOnce(&mut UiState) + 'static) {
        if self.notifying.get() {
            self.pending.borrow_mut().push(Box::new(f));
            return;
        }
        f(&mut self.state.borrow_mut());
        self.notify();
    }

    /// Change the state from outside a fan-out and read the answer back.
    ///
    /// The reducer is not an observer. It runs from the event pump and from
    /// toolkit callbacks, never from [`Observer::state_changed`], so it is
    /// never inside a fan-out and its mutation always applies immediately.
    /// That is what lets it hand back a value and hold borrowed locals:
    /// [`Dispatch::update`] must box its closure for the queue, so everything
    /// it touches has to be owned and nothing can come back out.
    ///
    /// Calling this from an observer is the one thing that breaks it, and the
    /// state's own borrow is what says so.
    pub(crate) fn edit<R>(&self, f: impl FnOnce(&mut UiState) -> R) -> R {
        debug_assert!(
            !self.notifying.get(),
            "edit ran inside a fan-out; an observer must raise its change through update"
        );
        let out = f(&mut self.state.borrow_mut());
        self.notify();
        out
    }

    /// Fold everything one event causes into the state, then repaint once.
    ///
    /// The reducer arm for a single key press writes the focused session, the
    /// selection, the notice and the attachment. Fanning out after each of
    /// those repaints the whole window four times for one press, and three of
    /// those paints show a window mid-event. Nested batches are counted, so an
    /// arm that calls another arm still produces one repaint.
    pub(crate) fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        self.held.set(self.held.get() + 1);
        let out = f();
        self.held.set(self.held.get() - 1);
        if self.held.get() == 0 && self.dirty.replace(false) {
            self.notify();
        }
        out
    }

    /// Tell every observer to read the state again.
    ///
    /// Called directly when the state changed somewhere this type cannot see,
    /// which is what the reducer folding a daemon message does.
    pub(crate) fn notify(&self) {
        // A batch is open, so the event is not finished being folded in and a
        // panel told now would be told a half-applied state.
        if self.held.get() > 0 {
            self.dirty.set(true);
            return;
        }
        if self.notifying.get() {
            return;
        }
        self.notifying.set(true);
        loop {
            // Anything raised while the state was borrowed is applied BEFORE
            // this pass reads it, so no observer is ever told a value that a
            // queued mutation has already superseded.
            let queued: Vec<Queued> = self.pending.borrow_mut().drain(..).collect();
            if !queued.is_empty() {
                let mut state = self.state.borrow_mut();
                for f in queued {
                    f(&mut state);
                }
            }
            self.passes.set(self.passes.get() + 1);
            let at = crate::tick();
            let observers = self.observers.borrow().clone();
            {
                let state = self.state.borrow();
                for observer in &observers {
                    observer.state_changed(&state, at);
                }
            }
            if self.pending.borrow().is_empty() {
                break;
            }
        }
        self.notifying.set(false);
    }

    /// Hand `settings` to every observer.
    pub(crate) fn settings_changed(&self, settings: &ShellSettings) {
        let observers = self.observers.borrow().clone();
        for observer in &observers {
            observer.settings_changed(settings);
        }
    }

    /// Fan-out passes so far.
    #[cfg(test)]
    pub(crate) fn passes(&self) -> usize {
        self.passes.get()
    }
}

#[cfg(test)]
mod tests;
