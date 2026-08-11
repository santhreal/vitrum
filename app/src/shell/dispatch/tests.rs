//! What a state change is allowed to cost, and what it is allowed to do to
//! an observer that answers it with another change.
//!
//! The class these close: a widget callback that mutates while the window is
//! being told about a mutation. Before the queue, that was a second mutable
//! borrow of one `RefCell` inside a GTK signal handler, which is a panic
//! across an FFI boundary and therefore an abort. Every case below is the
//! same shape as a real panel: a row that notices the session it is showing
//! is gone, a bar that clears a stale flash, a panel that adds another panel.
//!
//! Not covered: a panel that answers every change with a different change
//! forever. That is a live-lock this type cannot detect, and
//! [`a_pass_per_round_is_counted`] is what makes it visible as a pass count
//! rather than as a hang.

use std::cell::RefCell;
use std::rc::Rc;

use super::{Dispatch, Observer};
use crate::Tick;
use crate::state::UiState;
use crate::state::live::ShellSettings;

/// An observer that records what it was told.
struct Spy {
    /// One entry per `state_changed`, holding the sidebar width it saw.
    saw: RefCell<Vec<f64>>,
    /// One entry per `settings_changed`.
    settings: RefCell<usize>,
}

impl Spy {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            saw: RefCell::new(Vec::new()),
            settings: RefCell::new(0),
        })
    }
}

impl Observer for Spy {
    fn state_changed(&self, state: &UiState, _at: Tick) {
        self.saw.borrow_mut().push(state.window.sidebar_width);
    }

    fn settings_changed(&self, _settings: &ShellSettings) {
        *self.settings.borrow_mut() += 1;
    }
}

/// An observer that changes the state the first time it is told about one.
///
/// The panel this models is a row whose session vanished: it is told the
/// state, it discovers the thing it draws is gone, and it says so by writing
/// to the state from inside the fan-out.
struct Answers {
    dispatch: RefCell<Option<Rc<Dispatch>>>,
    answered: std::cell::Cell<bool>,
    seen: RefCell<Vec<f64>>,
}

impl Observer for Answers {
    fn state_changed(&self, state: &UiState, _at: Tick) {
        self.seen.borrow_mut().push(state.window.sidebar_width);
        if self.answered.replace(true) {
            return;
        }
        let d = self.dispatch.borrow().clone().expect("wired");
        d.update(|st| st.window.sidebar_width = 300.0);
    }
}

#[test]
fn an_observer_is_told_the_state_when_it_is_added() {
    let d = Dispatch::new(UiState::default());
    let spy = Spy::new();
    d.watch(spy.clone());
    assert_eq!(spy.saw.borrow().len(), 1, "one push at watch time");
}

#[test]
fn one_update_is_one_fan_out() {
    let d = Dispatch::new(UiState::default());
    let spy = Spy::new();
    d.watch(spy.clone());
    let before = d.passes();
    d.update(|st| st.window.sidebar_width = 240.0);
    assert_eq!(d.passes() - before, 1, "an update costs exactly one pass");
    assert_eq!(spy.saw.borrow().last().copied(), Some(240.0));
}

#[test]
fn every_observer_sees_the_same_value() {
    let d = Dispatch::new(UiState::default());
    let a = Spy::new();
    let b = Spy::new();
    d.watch(a.clone());
    d.watch(b.clone());
    d.update(|st| st.window.sidebar_width = 199.0);
    assert_eq!(a.saw.borrow().last(), b.saw.borrow().last());
}

#[test]
fn an_update_from_inside_a_fan_out_is_applied_and_announced() {
    let d = Rc::new(Dispatch::new(UiState::default()));
    let answers = Rc::new(Answers {
        dispatch: RefCell::new(Some(Rc::clone(&d))),
        answered: std::cell::Cell::new(false),
        seen: RefCell::new(Vec::new()),
    });
    d.watch(answers.clone());
    // The mutation raised during the fan-out reached the state...
    assert_eq!(d.peek(|st| st.window.sidebar_width), 300.0);
    // ...and the observer was told about it rather than left stale.
    assert_eq!(answers.seen.borrow().last().copied(), Some(300.0));
}

#[test]
fn a_pass_per_round_is_counted() {
    let d = Rc::new(Dispatch::new(UiState::default()));
    let answers = Rc::new(Answers {
        dispatch: RefCell::new(Some(Rc::clone(&d))),
        answered: std::cell::Cell::new(false),
        seen: RefCell::new(Vec::new()),
    });
    let before = d.passes();
    d.watch(answers);
    // Being told at watch time is not a pass; it is one observer reading the
    // state it was just handed. The mutation that reading raised is, and it
    // costs exactly one: a second would mean the queue refilled itself.
    assert_eq!(d.passes() - before, 1);
}

#[test]
fn notify_reaches_a_change_made_behind_the_dispatch() {
    // The reducer folds a daemon message into the same state this holds, so
    // the change is already there when `notify` is called.
    let d = Dispatch::new(UiState::default());
    let spy = Spy::new();
    d.watch(spy.clone());
    d.update(|st| st.window.sidebar_width = 111.0);
    d.notify();
    let saw = spy.saw.borrow();
    assert_eq!(saw.len(), 3, "watch, update, notify");
    assert_eq!(saw.last().copied(), Some(111.0));
}

#[test]
fn settings_reach_every_observer() {
    let d = Dispatch::new(UiState::default());
    let a = Spy::new();
    let b = Spy::new();
    d.watch(a.clone());
    d.watch(b.clone());
    d.settings_changed(&crate::state::live::shell_settings());
    assert_eq!(*a.settings.borrow(), 1);
    assert_eq!(*b.settings.borrow(), 1);
}

/// WHY: a transient surface that stays in the fan-out after it is dismissed
/// costs a repaint of a dead widget on every daemon message, for the life of
/// the window. This is the class, not one dialog: it closes for anything
/// added through `watch` and taken away again.
#[test]
fn an_unwatched_observer_hears_nothing_more() {
    let d = Dispatch::new(UiState::default());
    let stays = Spy::new();
    let goes = Spy::new();
    d.watch(stays.clone());
    d.watch(goes.clone());
    let seen_before = goes.saw.borrow().len();

    d.unwatch(&(goes.clone() as Rc<dyn Observer>));
    d.update(|st| st.window.sidebar_width = 321.0);

    assert_eq!(goes.saw.borrow().len(), seen_before, "told after unwatch");
    assert_eq!(stays.saw.borrow().last().copied(), Some(321.0));
}

/// WHY: unwatching by value rather than by pointer would remove a different
/// surface that happens to hold equal fields, which is two dialogs of the
/// same kind open at once losing the wrong one.
#[test]
fn unwatching_removes_only_that_observer() {
    let d = Dispatch::new(UiState::default());
    let first = Spy::new();
    let second = Spy::new();
    d.watch(first.clone());
    d.watch(second.clone());
    d.unwatch(&(first.clone() as Rc<dyn Observer>));
    d.update(|st| st.window.sidebar_width = 77.0);
    assert_eq!(second.saw.borrow().last().copied(), Some(77.0));
}
