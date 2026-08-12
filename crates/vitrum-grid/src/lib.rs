//! GPU terminal cell-grid renderer.
//!
//! `vitrum-grid` paints a fixed-pitch terminal grid with `wgpu` and nothing
//! else. It has no window, no event loop, no PTY, and no VT parser. Its whole
//! input is a [`CellGrid`] that something else fills in.
//!
//! # What this renders
//!
//! A live session. The daemon reads a PTY, [`vitrum_vt`] parses the bytes and
//! syncs the result into a [`CellGrid`], and this crate turns that grid into
//! pixels. The same path serves a replayed recording, because a replay produces
//! the same grid type from the same parser. There is one parser, one grid, and
//! one renderer between the PTY and the panel.
//!
//! The renderer is the frame budget of the product. Everything in it is
//! arranged so that a frame costs what changed:
//!
//! - [`CellGrid`] compares every write against the value already stored and
//!   records damage only on a difference, as one inclusive-exclusive column
//!   span per row.
//! - [`GridRenderer::render`] uploads exactly those spans, coalescing spans
//!   that are adjacent in flat cell order into one `write_buffer`.
//! - A frame with no damage creates no command encoder, writes no buffer, and
//!   submits nothing. Twenty idle sessions cost twenty no-ops.
//!
//! What is not yet damage-scoped is rasterisation. The draw is one instanced
//! call over `cols * rows` instances with `LoadOp::Clear` on the whole
//! attachment, so a one-cell change still rasterises the panel. Closing that is
//! a change to the draw range, the load op, and the scissor rect; see
//! [`GridRenderer::render`] and [`FrameStats::instances_drawn`], which is the
//! number a damage-scoped draw would shrink.
//!
//! # Integration
//!
//! [`GridRenderer::render`] takes a device, a queue, the grid, a
//! `wgpu::TextureView`, and the viewport in pixels. Any host that can hand over
//! those five things can host this renderer: a `winit` surface, a GPUI or Iced
//! view, or a custom widget in a CSS-laid-out host. Nothing here depends on a
//! particular host, and the `wgpu` version is pinned so a host and this
//! renderer share one `wgpu::Device` rather than linking two incompatible
//! copies of `wgpu`.
//!
//! ```text
//! // once per frame, inside the host's paint:
//! let stats = renderer.render(device, queue, &mut grid, &view, (w, h))?;
//! if !stats.gpu_work {
//!     // Nothing changed. No encoder was created and nothing was submitted,
//!     // so the host can skip presenting entirely.
//! }
//! ```
//!
//! # Cost model
//!
//! - One allocation for the grid, one for the instance buffer, one texture for
//!   the glyph atlas. Nothing is allocated per frame.
//! - One instanced draw call per frame, whatever the grid size.
//! - Only changed cells are re-uploaded.
//! - Glyphs are rasterised once and cached in the atlas as 8-bit coverage.
//!
//! # Example
//!
//! ```
//! use vitrum_grid::{CellGrid, GpuContext, GridRenderer, HeadlessTarget, RendererConfig, Style};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let gpu = GpuContext::headless()?;
//! let config = RendererConfig {
//!     format: HeadlessTarget::FORMAT,
//!     ..RendererConfig::default()
//! };
//! let mut renderer = GridRenderer::new(gpu.device(), &config)?;
//!
//! let (cw, ch) = renderer.cell_size();
//! let target = HeadlessTarget::new(gpu.device(), cw * 20, ch * 3);
//!
//! let mut grid = CellGrid::new(20, 3, Style::DEFAULT)?;
//! grid.write_str(0, 0, "hello", Style::DEFAULT)?;
//!
//! let drawn = renderer.render(
//!     gpu.device(),
//!     gpu.queue(),
//!     &mut grid,
//!     target.view(),
//!     (target.width(), target.height()),
//! )?;
//! assert!(drawn.gpu_work);
//!
//! // Nothing changed since, so this frame records no GPU command at all.
//! let idle = renderer.render(
//!     gpu.device(),
//!     gpu.queue(),
//!     &mut grid,
//!     target.view(),
//!     (target.width(), target.height()),
//! )?;
//! assert!(!idle.gpu_work);
//! # Ok(())
//! # }
//! ```
//!
//! # Module map
//!
//! - [`mod@cell`]: colour, attributes, the 16-byte cell, and the caret.
//! - [`mod@grid`]: the bounded grid and its per-row damage spans.
//! - [`mod@font`]: face discovery, cell metrics, and glyph rasterisation.
//! - [`mod@atlas`]: the coverage texture and the shelf packer that fills it.
//! - [`mod@renderer`]: the pipeline, the instance layout, and the frame.
//! - [`mod@gpu`]: device acquisition for callers that do not already have one.
//! - [`mod@headless`]: an offscreen target with pixel readback.
//! - [`mod@probe`]: off-by-default attribution of a frame to its phases.

#![deny(missing_docs)]

pub mod atlas;
pub mod cell;
pub mod font;
pub mod gpu;
pub mod grid;
pub mod headless;
pub mod probe;
pub mod renderer;

#[cfg(test)]
mod tests;

pub use atlas::{AtlasEntry, AtlasError, DEFAULT_ATLAS_DIM, GlyphAtlas, GlyphKey};
pub use cell::{
    Attrs, Cell, CellSlot, CharWidth, Cursor, CursorShape, Rgba, Style, char_width,
};
pub use font::{
    CellMetrics, Coverage, DEFAULT_FAMILIES, FallbackEntry, FontConfig, FontError, FontStack,
    FontStyle, MAX_SIZE_PX, RasterGlyph, fallback_chain, prewarm_font_stack,
};
pub use gpu::{AdapterClass, GpuContext, GpuError};
pub use grid::{
    CellGrid, DamageSpan, GridError, MAX_CELLS, MAX_COLS, MAX_ROWS, Region, WriteError,
};
pub use headless::{HeadlessTarget, Image};
pub use renderer::{FrameStats, GridRenderer, RenderError, RendererConfig, origin_px};
