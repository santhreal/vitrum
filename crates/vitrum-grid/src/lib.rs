//! GPU terminal cell-grid renderer.
//!
//! `vitrum-grid` draws a fixed-pitch terminal grid with `wgpu` and nothing else.
//! It has no window, no event loop, no PTY, and no VT parser. Its whole input is
//! a [`CellGrid`] that something else fills in.
//!
//! # Why this exists
//!
//! The vitrum client renders terminals with xterm.js inside a webview today. The
//! plan is to move the client to Dioxus Native, which paints through Blitz, and
//! Blitz has no JavaScript engine, so xterm.js cannot come along. This crate is
//! the replacement renderer. Because it is plain `wgpu` and depends on neither
//! Blitz nor Dioxus, the same code also drops into GPUI, Iced, or bare `winit`.
//!
//! The `wgpu` version is pinned to the one Blitz uses, so a Blitz custom widget
//! and this renderer can share one `wgpu::Device` instead of dragging two
//! incompatible copies of `wgpu` into the binary.
//!
//! # Shape of the integration
//!
//! A Blitz custom widget receives a device, paints into a texture, and hands
//! that texture back to the compositor. That maps directly onto
//! [`GridRenderer::render`], which takes a device, a queue, and a
//! `wgpu::TextureView` to draw into. In host terms the widget is attached to a
//! CSS-laid-out node, then painted every time the node needs a frame:
//!
//! ```text
//! let node = doc.query_selector("#terminal")?;      // host: Blitz DOM
//! doc.mutate().set_custom_widget(node, widget);     // host: Blitz DOM
//!
//! // inside the widget's paint, once per frame:
//! let stats = renderer.render(device, queue, &mut grid, &view, (w, h))?;
//! if !stats.gpu_work {
//!     // Nothing changed. No encoder was created and nothing was submitted.
//!     // The host can skip presenting entirely.
//! }
//! ```
//!
//! # Cost model
//!
//! - One allocation for the grid, one for the instance buffer, one texture for
//!   the glyph atlas. Nothing is allocated per frame.
//! - One instanced draw call per frame, whatever the grid size.
//! - Only cells that changed are re-uploaded, and adjacent damage coalesces into
//!   a single `write_buffer`.
//! - A frame with no damage records no GPU command at all. Twenty idle sessions
//!   cost twenty no-ops.
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
//! // Nothing changed since, so this frame costs nothing.
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

#![deny(missing_docs)]

pub mod atlas;
pub mod cell;
pub mod font;
pub mod gpu;
pub mod grid;
pub mod headless;
pub mod renderer;

#[cfg(test)]
mod tests;

pub use atlas::{AtlasEntry, AtlasError, DEFAULT_ATLAS_DIM, GlyphAtlas, GlyphKey};
pub use cell::{Attrs, Cell, CellSlot, CharWidth, Rgba, Style, char_width};
pub use font::{
    CellMetrics, DEFAULT_FAMILIES, FontConfig, FontError, FontStack, FontStyle, MAX_SIZE_PX,
    RasterGlyph,
};
pub use gpu::{AdapterClass, GpuContext, GpuError};
pub use grid::{
    CellGrid, DamageSpan, GridError, MAX_CELLS, MAX_COLS, MAX_ROWS, Region, WriteError,
};
pub use headless::{HeadlessTarget, Image};
pub use renderer::{FrameStats, GridRenderer, RenderError, RendererConfig};
