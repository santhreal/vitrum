//! The About tab: what is installed, and the update controls.
//!
//! Separate from the rest of the sheet because most of it edits no preference
//! at all: it runs [`crate::update`] and reports what it found. The two
//! preferences it does own are the ones an operator looks for while reading
//! that report, which is why they are here and not on another tab.

