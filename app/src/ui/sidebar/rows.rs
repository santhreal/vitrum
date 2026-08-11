//! The row store, and the guarantee that a settled row costs nothing.
//!
//! # What this file is defending
//!
//! The daemon pushes one update per live session per second, and twenty live
//! sessions is the load this product is built for. Exactly one row's contents
//! change on each of those pushes; the other nineteen are identical to what is
//! already on screen. A panel that rebuilt all twenty would spend the frame
//! budget on nineteen widget trees nobody could tell from the ones they
//! replaced, and a GTK widget is more expensive to build than the markup this
//! replaced, not less.
//!
//! So a row is keyed by its session and held across paints, and it is rebuilt
//! only when its [`RowFold`] compares unequal. Everything else about a paint
//! is cheap: folding is a walk over an arrangement the state layer already
//! made, and reseating is a pointer move inside a container.
//!
//! # Why the store is generic over its view
//!
//! Because the guarantee has to be checkable on a machine with no display. A
//! test substitutes a view that counts, drives the SAME [`Rows::sync`] the
//! window drives, and reads the counts back. A parallel test harness that
//! reimplemented the diff would prove something about itself.

use std::collections::HashMap;

use vitrum_proto::SessionId;

use super::fold::RowFold;

/// Anything that can be a row on screen.
pub(crate) trait RowView {
    /// Whatever a row needs besides its fold in order to exist.
    ///
    /// The window's rows need the shell, because a row's controls act on it.
    /// A counting view in a test needs nothing, and says so with `()`. The
    /// context is passed in rather than held, so the store owns no clone of
    /// the shell and cannot keep one alive past the window.
    type Cx;

    /// Build a row for the first time.
    fn build(fold: &RowFold, cx: &Self::Cx) -> Self;

    /// Bring an existing row up to date.
    ///
    /// Called only when the fold changed. A view may assume the previous fold
    /// is stale and nothing else.
    fn apply(&self, fold: &RowFold, cx: &Self::Cx);
}

/// What one [`Rows::sync`] cost.
///
/// Reported rather than logged: the numbers are the contract, and a caller
/// that wants to pack new widgets needs to know which rows are new anyway.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Cost {
    /// Sessions that had no row and now have one, in draw order.
    pub(crate) built: Vec<SessionId>,
    /// Rows whose contents changed, in draw order.
    pub(crate) applied: Vec<SessionId>,
    /// Rows whose session left the list.
    pub(crate) dropped: Vec<SessionId>,
    /// Whether the surviving rows are in a different order than before.
    pub(crate) resorted: bool,
}

impl Cost {
}

/// Every row on screen, in draw order, with the fold each was built from.
pub(crate) struct Rows<V> {
    /// Draw order. The list is short — a preview cut bounds it well under a
    /// hundred — so this is a `Vec` and the map beside it is what makes the
    /// lookup constant.
    order: Vec<SessionId>,
    held: HashMap<SessionId, (RowFold, V)>,
}

impl<V> Default for Rows<V> {
    fn default() -> Self {
        Rows {
            order: Vec::new(),
            held: HashMap::new(),
        }
    }
}

impl<V: RowView> Rows<V> {
    /// Bring the store in line with `next`, and report what that cost.
    ///
    /// A row whose fold is unchanged is not touched at all: not rebuilt, not
    /// applied, and not counted. That is the whole point of the type.
    pub(crate) fn sync(&mut self, next: &[RowFold], cx: &V::Cx) -> Cost {
        let mut cost = Cost::default();
        for fold in next {
            match self.held.get_mut(&fold.id) {
                Some(held) if held.0 == *fold => {}
                Some(held) => {
                    held.0 = fold.clone();
                    held.1.apply(fold, cx);
                    cost.applied.push(fold.id);
                }
                None => {
                    let view = V::build(fold, cx);
                    self.held.insert(fold.id, (fold.clone(), view));
                    cost.built.push(fold.id);
                }
            }
        }
        let wanted: Vec<SessionId> = next.iter().map(|fold| fold.id).collect();
        self.held.retain(|id, _| {
            let keep = wanted.contains(id);
            if !keep {
                cost.dropped.push(*id);
            }
            keep
        });
        cost.dropped.sort_unstable();
        cost.resorted = self.order != wanted;
        self.order = wanted;
        cost
    }

    /// One row's view, or `None` when that session is not on screen.
    pub(crate) fn view(&self, id: SessionId) -> Option<&V> {
        self.held.get(&id).map(|held| &held.1)
    }

}
