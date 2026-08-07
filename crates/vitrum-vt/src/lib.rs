//! Ghostty's terminal engine, driving a [`CellGrid`](vitrum_grid::CellGrid).
//!
//! `vitrum-vt` is the parser and screen model that sits between a PTY byte
//! stream and [`vitrum-grid`](vitrum_grid). It has no window, no event loop,
//! and no renderer: bytes go in through [`Vt::feed`], and a grid of cells comes
//! out through [`Vt::sync`].
//!
//! # Why Ghostty and not a parser of our own
//!
//! The client renders terminals with xterm.js inside a webview today, which
//! costs a JavaScript engine per session and looks like a web page pretending
//! to be a terminal. Replacing it needs a VT implementation, and a VT
//! implementation is not a weekend of work: it is DEC modes, scroll regions,
//! reflow on resize, OSC handling, grapheme clustering, and a decade of
//! terminal quirks. `libghostty-vt` is that implementation, extracted from
//! Ghostty and shipped as a C library, so vitrum gets the engine of a terminal
//! people already trust instead of a new one that has to earn trust.
//!
//! It also brings capabilities the webview path never had: OSC 7 working
//! directory and OSC 133 shell integration, semantic selection by word and by
//! command output, and scrollback that reflows when the window resizes.
//!
//! # How the engine is obtained
//!
//! Two features, one choice, because the answer differs per installation:
//!
//! - `vendored` (default) builds libghostty from source, which needs a Zig
//!   toolchain at build time and pins the exact engine commit.
//! - `system` links a libghostty the platform already provides, which needs no
//!   Zig and tracks whatever the system ships.
//!
//! # Cost model
//!
//! - One engine allocation per session, plus the scrollback it is given.
//! - [`Vt::sync`] reads only the rows the terminal reports as changed, and
//!   writes through to the grid, which itself records damage only for cells
//!   whose value differs. An idle terminal produces
//!   [`SyncStats::is_noop`], and the renderer then records no GPU work.
//! - Grapheme clusters are read into a stack buffer. Nothing is allocated per
//!   frame, per row, or per cell.
//!
//! # Example
//!
//! ```
//! use vitrum_grid::{CellGrid, Style};
//! use vitrum_vt::{Vt, VtOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut vt = Vt::new(VtOptions { cols: 20, rows: 3, max_scrollback: 0 })?;
//! let mut grid = CellGrid::new(20, 3, Style::DEFAULT)?;
//!
//! vt.feed(b"\x1b[1;32mgreen\x1b[0m\r\n");
//! let stats = vt.sync(&mut grid)?;
//! assert!(stats.cells_changed > 0);
//! assert_eq!(grid.row_text(0).unwrap().trim_end(), "green");
//!
//! // Nothing arrived since, so the next frame changes nothing.
//! assert!(vt.sync(&mut grid)?.is_noop());
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

pub mod bridge;
pub mod engine;
pub mod events;
pub mod linkage;
pub mod pwd;

#[cfg(test)]
mod tests;

pub use bridge::{CursorShape, CursorState, SyncStats};
pub use engine::{Vt, VtError, VtOptions};
pub use events::Events;
pub use pwd::pwd_path;

// Re-exported so a host can drive scrolling and read colours without taking a
// direct dependency on the engine crate, whose version this crate pins.
pub use libghostty_vt::terminal::ScrollViewport;
pub use libghostty_vt::style::RgbColor;
