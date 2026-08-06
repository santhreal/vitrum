//! Test suites for `vitrum-vt`, one concern per module.
//!
//! - [`support`]: the shared fixture, a terminal wired to a grid of the same size.
//! - [`vt_text`]: what a byte stream puts on the screen.
//! - [`vt_style`]: how colour and rendition reach a cell.
//! - [`vt_wide`]: double-width characters and grapheme clusters.
//! - [`vt_damage`]: what does and does not cost work, including the idle path.
//! - [`vt_events`]: replies, bells, title, and working directory.
//! - [`vt_geometry`]: resize, scrollback, and the cursor.
//! - [`vt_linkage`]: the build's linkage record.

mod support;
mod vt_damage;
mod vt_events;
mod vt_geometry;
mod vt_linkage;
mod vt_style;
mod vt_text;
mod vt_wide;
