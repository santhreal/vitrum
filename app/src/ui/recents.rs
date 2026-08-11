//! The last few things this operator started, as a list they can click.
//!
//! The launcher ranks a whole history by frequency and recency and shows the
//! best nine for whatever is typed. This is the other question: "what was I
//! just doing, and where". It is keyed on the command AND the directory, so
//! the same agent in two checkouts is two rows, and it is stored in the order
//! it will be drawn in, so a surface that renders it does no ranking and takes
//! no clock reading.
//!
//! One row is a glyph, the command as its text, and the place as a chip
//! carrying the absolute path as its tooltip. Taking a row is a click, and the
//! note line under the list is the only thing this surface is allowed to say.

/// The list itself, built as GTK widgets and packed into the launcher.
pub mod native;
