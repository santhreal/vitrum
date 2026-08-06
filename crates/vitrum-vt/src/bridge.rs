//! Projecting a libghostty render snapshot onto a [`CellGrid`].
//!
//! The two sides model a cell differently, and every difference is resolved
//! here so the engine stays a control flow and the renderer stays untouched:
//!
//! - libghostty resolves colour lazily: a cell reports `None` when it uses the
//!   terminal default, and the default itself lives on the snapshot. The grid
//!   stores absolute colour per cell, so the default is substituted here.
//! - libghostty tracks eleven rendition attributes. The grid draws four. The
//!   rest are folded or dropped by [`attrs_of`], which documents each choice.
//! - libghostty gives a cell a full grapheme cluster. A grid cell is 16 bytes
//!   and holds one `char`, so clusters are flattened to their base codepoint
//!   and counted, never silently discarded.

use libghostty_vt::render::Colors;
use libghostty_vt::screen::CellWide;
use libghostty_vt::style::{RgbColor, Style as VtStyle, Underline};
use vitrum_grid::cell::{Attrs, Cell, CellSlot, Rgba};

/// Longest grapheme cluster read out of a cell in one go.
///
/// A cluster longer than this is truncated at the base codepoint, which is
/// already what the grid stores. The buffer exists only to give libghostty
/// somewhere to write, so it is sized for the realistic case (a base plus a
/// handful of combining marks) and never heap-allocated.
pub const GRAPHEME_BUF: usize = 8;

/// Convert a libghostty colour to the renderer's, which is the same three
/// channels plus a fully opaque alpha.
#[must_use]
pub const fn to_rgba(color: RgbColor) -> Rgba {
    Rgba::rgb(color.r, color.g, color.b)
}

/// Fold a libghostty style into the four rendition bits the renderer draws.
///
/// The attributes that do not survive, and why:
///
/// - `faint`, `blink`, `invisible`, `strikethrough`, `overline`: the shader
///   draws none of them, and inventing an approximation (dimming for faint,
///   say) would be a rendering decision made in a translation layer.
/// - underline *style*: every variant collapses to one rule, because the
///   fragment shader draws one rule. Curly and dotted underlines are visible
///   as underlines rather than as nothing.
#[must_use]
pub fn attrs_of(style: &VtStyle) -> Attrs {
    let mut attrs = Attrs::NONE;
    if style.bold {
        attrs = attrs.with(Attrs::BOLD);
    }
    if style.italic {
        attrs = attrs.with(Attrs::ITALIC);
    }
    if style.underline != Underline::None {
        attrs = attrs.with(Attrs::UNDERLINE);
    }
    if style.inverse {
        attrs = attrs.with(Attrs::REVERSE);
    }
    attrs
}

/// Which grid slot a cell occupies, given its libghostty wide property.
///
/// `SpacerHead` is the padding cell at the end of a soft-wrapped line that a
/// wide character could not fit into. It is a real blank, not half of a pair,
/// so it maps to [`CellSlot::Single`].
#[must_use]
pub const fn slot_of(wide: CellWide) -> CellSlot {
    match wide {
        CellWide::Wide => CellSlot::WideHead,
        CellWide::SpacerTail => CellSlot::WideTail,
        CellWide::Narrow | CellWide::SpacerHead => CellSlot::Single,
    }
}

/// Build the grid cell for one screen cell.
///
/// `ch` is the base codepoint already read from the cluster, `None` when the
/// cell holds no text. `fg`/`bg` are the cell's own colours, `None` meaning
/// "the terminal default", which `colors` supplies.
#[must_use]
pub fn cell_of(
    ch: Option<char>,
    fg: Option<RgbColor>,
    bg: Option<RgbColor>,
    style: &VtStyle,
    wide: CellWide,
    colors: &Colors,
) -> Cell {
    let slot = slot_of(wide);
    Cell {
        // A wide character's tail draws nothing, and the grid spells that as
        // the null character rather than a space so the renderer can skip the
        // atlas lookup entirely.
        ch: match slot {
            CellSlot::WideTail => '\0',
            CellSlot::Single | CellSlot::WideHead => ch.unwrap_or(' '),
        },
        fg: to_rgba(fg.unwrap_or(colors.foreground)),
        bg: to_rgba(bg.unwrap_or(colors.background)),
        attrs: attrs_of(style),
        slot,
    }
}

/// Where the cursor is and how it should be drawn.
///
/// Reported separately from the grid because the grid has no cursor: it is a
/// screen of cells, and the cursor is a renderer overlay that must not damage
/// the cell underneath it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CursorState {
    /// Column, in cells, within the viewport.
    pub col: u16,
    /// Row, in cells, within the viewport.
    pub row: u16,
    /// Whether the program has hidden the cursor (DEC mode 25).
    pub visible: bool,
    /// Whether the cursor sits on the tail half of a wide character, in which
    /// case a block cursor should cover both columns.
    pub at_wide_tail: bool,
    /// Colour to draw it in, absolute.
    pub color: Rgba,
    /// Shape requested by the program.
    pub shape: CursorShape,
}

/// The cursor shapes DECSCUSR can select.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorShape {
    /// A filled rectangle over the cell.
    #[default]
    Block,
    /// An outlined rectangle, conventionally drawn when the window is unfocused.
    HollowBlock,
    /// A vertical bar at the left edge of the cell.
    Bar,
    /// A rule along the bottom of the cell.
    Underline,
}

/// What one [`sync`](crate::Vt::sync) actually did.
///
/// Every field is a count rather than a flag so a caller can assert on the work
/// performed, which is how the idle path is proven to cost nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SyncStats {
    /// Rows examined and written through to the grid.
    pub rows_synced: u16,
    /// Rows the terminal reported as unchanged, and so skipped entirely.
    pub rows_skipped: u16,
    /// Cells whose value actually differed from what the grid already held.
    /// This is the number that becomes GPU upload work.
    pub cells_changed: usize,
    /// Cells whose grapheme cluster held more than one codepoint and was
    /// flattened to its base. Non-zero means the screen is showing an
    /// approximation of what the program sent.
    pub graphemes_flattened: usize,
    /// True when the grid was resized to follow the terminal during this sync.
    pub resized: bool,
}

impl SyncStats {
    /// True when the grid ended up identical to what it already held, so the
    /// renderer has nothing to upload and no frame to present.
    #[must_use]
    pub const fn is_noop(&self) -> bool {
        self.cells_changed == 0 && !self.resized
    }
}
