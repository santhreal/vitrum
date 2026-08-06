//! What the terminal asks the host to do while it parses bytes.
//!
//! A VT stream is not only screen content. It also contains queries the program
//! expects an answer to, and state changes (title, working directory) that
//! belong to the window rather than the grid. libghostty reports these through
//! callbacks it invokes during a write, so this module owns the sink those
//! callbacks write into.
//!
//! # Why this is not a queue
//!
//! An event queue has to answer "what happens when the host stops draining it",
//! and every answer is bad: growing without bound leaks, dropping loses a reply
//! a program is blocked on. So there is no queue. Each kind of event is folded
//! into the smallest state that preserves its meaning:
//!
//! - PTY replies accumulate into one byte buffer, because they are a byte
//!   stream and concatenating them is exactly what the PTY expects.
//! - Bells are a count, because ten bells and one bell differ only in number.
//! - Title and working directory are last-wins, because an older value is not
//!   information, it is a stale value.
//!
//! Memory is therefore bounded by the pending reply bytes alone, and nothing
//! is ever silently discarded.

use std::cell::{Cell, RefCell};

/// The sink every terminal callback writes into.
///
/// Shared as an [`Rc`](std::rc::Rc) between the callbacks and the engine, which
/// is why every method takes `&self` and mutates through interior mutability.
#[derive(Debug, Default)]
pub struct Events {
    pty_write: RefCell<Vec<u8>>,
    bells: Cell<usize>,
    title: RefCell<Option<String>>,
    pwd: RefCell<Option<String>>,
}

impl Events {
    /// An empty sink.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record bytes the terminal wants written back to the PTY.
    pub fn push_pty_write(&self, data: &[u8]) {
        self.pty_write.borrow_mut().extend_from_slice(data);
    }

    /// Record one bell.
    pub fn push_bell(&self) {
        self.bells.set(self.bells.get().saturating_add(1));
    }

    /// Record the current window title.
    pub fn set_title(&self, title: &str) {
        *self.title.borrow_mut() = Some(title.to_owned());
    }

    /// Record the current working directory.
    pub fn set_pwd(&self, pwd: &str) {
        *self.pwd.borrow_mut() = Some(pwd.to_owned());
    }

    /// True when the terminal owes the PTY a reply.
    #[must_use]
    pub fn has_pty_write(&self) -> bool {
        !self.pty_write.borrow().is_empty()
    }

    /// Move every pending reply byte onto `out`, leaving the sink empty.
    ///
    /// Appends rather than assigns so a host can batch one write across several
    /// sessions without an intermediate buffer.
    pub fn drain_pty_write(&self, out: &mut Vec<u8>) {
        let mut pending = self.pty_write.borrow_mut();
        out.extend_from_slice(&pending);
        pending.clear();
    }

    /// Take the bell count accumulated since the last call.
    pub fn take_bells(&self) -> usize {
        self.bells.replace(0)
    }

    /// Take the title if it changed since the last call.
    pub fn take_title(&self) -> Option<String> {
        self.title.borrow_mut().take()
    }

    /// Take the working directory if it changed since the last call.
    pub fn take_pwd(&self) -> Option<String> {
        self.pwd.borrow_mut().take()
    }
}
