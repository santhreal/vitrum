//! Test suites for `vitrum-grid`, one concern per module.
//!
//! - [`cell_semantics`]: colour, attribute, and cell invariants that everything
//!   else is built on.
//! - [`grid_geometry`]: construction, bounds, fills, scrolling, and resize.
//! - [`grid_wide_chars`]: double-width characters and the pair repair rules.
//! - [`grid_damage`]: what does and does not mark a cell dirty.
//! - [`font_raster`]: face discovery, cell metrics, and glyph bitmaps.
//! - [`atlas_packing`]: shelf placement, reset, and exhaustion.
//! - [`instance_layout`]: the byte layout the vertex shader reads.
//! - [`render_pixels`]: headless renders with exact pixel assertions.
//! - [`render_cursor`]: the caret shapes, composited over the cell beneath.
//! - [`render_cost`]: upload counts, the zero-work idle path, and frame timing.

mod atlas_packing;
mod cell_semantics;
mod font_raster;
mod grid_damage;
mod grid_geometry;
mod grid_wide_chars;
mod instance_layout;
mod render_cost;
mod render_cursor;
mod render_pixels;
mod support;
