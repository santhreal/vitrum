//! Saved presets, as chips the operator clicks.
//!
//! A preset is a command the operator already decided is worth keeping. The
//! launcher's ranked list can offer one, but only ever as one row among nine
//! competing with history, `PATH` discovery and whatever is running: the thing
//! you deliberately saved could be pushed below the fold by things you never
//! chose. That is the defect this exists for. A preset the operator saved is
//! shown unconditionally, in the order they saved it, and starting one is a
//! click on the thing itself rather than a search for it.
//!
//! Chips, not a list, and that is the whole distinction from
//! [`crate::ui::recents`]: recents answer "what was I just doing", are ranked
//! by time and read as a column of sentences. Presets are a small fixed set of
//! named buttons, so they wrap across the width instead of consuming nine rows
//! of vertical space above the list that is still the primary surface.
//!
//! Validation happens on the click, never per render, for the reason the
//! launcher documents: [`crate::launch::preset_fault`] is a `stat` and a
//! `PATH` walk, and doing it while drawing would put both on every keystroke
//! of the surface hosting this. A preset that cannot run does not vanish and
//! does not launch; it says which part of it is missing.

/// The band itself, built as GTK widgets and packed into the launcher.
pub mod native;
