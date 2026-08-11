//! Colours, type and cell metrics, as the operator set them.
//!
//! A terminal that ignores the operator's palette is a terminal that looks
//! like somebody else's. Every value here comes from settings and every one of
//! them takes effect while the window is open: changing a colour must not
//! require a restart, because the way an operator picks a colour is by trying
//! several.
//!
//! # Where each value lands
//!
//! Colour is split across two owners and neither can do the other's job. The
//! sixteen ANSI entries, the default foreground and the default background go
//! to the emulator, because a cell's colour is decided when the escape
//! sequence is parsed and the renderer only ever sees the resolved value. The
//! selection and search highlight colours stay here, because no escape
//! sequence produces them: they are the pane's own paint over cells the
//! emulator already coloured.
//!
//! Type goes to the renderer's font stack. The cell size falls out of the
//! font, which is why changing the size changes the grid and therefore
//! resizes the child.
//!
//! # Contrast is not optional
//!
//! An operator can choose a foreground and a background that are the same
//! colour, and a terminal that renders that faithfully is a black rectangle
//! with an invisible cursor and no way to find the settings row that caused
//! it. The selection and highlight colours are therefore derived from the
//! background when the operator has not set them, rather than defaulted to a
//! constant that disappears against half of all themes.

use vitrum_grid::cell::{Rgba, Style};
use vitrum_grid::font::FontConfig;

/// Sixteen ANSI colours, plus the four the pane resolves itself.
///
/// Sparse in the sense that matters: every field has a value, but
/// [`PaneTheme::ansi_overrides`] hands the emulator only the entries that
/// differ from what it already has, so a theme that defines eight colours does
/// not blank the other eight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Palette {
    /// Indices 0 through 15, in the standard order: black, red, green, yellow,
    /// blue, magenta, cyan, white, then the eight bright variants.
    pub ansi: [Rgba; 16],
    /// Default background, used for every cell the child did not colour and
    /// for the pixels no cell covers.
    pub background: Rgba,
    /// Default foreground.
    pub foreground: Rgba,
    /// Cursor colour.
    pub cursor: Rgba,
    /// Background of a selected cell.
    pub selection_bg: Rgba,
    /// Foreground of a selected cell.
    pub selection_fg: Rgba,
    /// Background of a search match that is not the current one.
    pub match_bg: Rgba,
    /// Background of the current search match.
    pub current_match_bg: Rgba,
}

impl Default for Palette {
    /// The colours a pane paints before settings have been read.
    ///
    /// These are seen: a window opens and paints before the settings file has
    /// been parsed, so a default of black on black is a pane that flashes.
    fn default() -> Self {
        const fn c(r: u8, g: u8, b: u8) -> Rgba {
            Rgba::rgb(r, g, b)
        }
        Self {
            ansi: [
                c(0x1c, 0x1e, 0x26),
                c(0xe9, 0x5b, 0x67),
                c(0x8e, 0xc0, 0x7c),
                c(0xe3, 0xb1, 0x4b),
                c(0x61, 0x9c, 0xd4),
                c(0xb2, 0x77, 0xd4),
                c(0x5a, 0xb0, 0xb5),
                c(0xc7, 0xc9, 0xd1),
                c(0x3f, 0x43, 0x50),
                c(0xf2, 0x77, 0x83),
                c(0xa4, 0xd0, 0x94),
                c(0xf0, 0xc6, 0x74),
                c(0x82, 0xb2, 0xe3),
                c(0xc4, 0x94, 0xe3),
                c(0x78, 0xc6, 0xca),
                c(0xe8, 0xea, 0xf0),
            ],
            background: c(0x14, 0x16, 0x1c),
            foreground: c(0xc7, 0xc9, 0xd1),
            cursor: c(0x82, 0xb2, 0xe3),
            selection_bg: c(0x2c, 0x3a, 0x52),
            selection_fg: c(0xe8, 0xea, 0xf0),
            match_bg: c(0x4a, 0x40, 0x1c),
            current_match_bg: c(0x8a, 0x6d, 0x1c),
        }
    }
}

/// How the pane presents frames.
///
/// The set a driver offers is a property of the GPU, not of this enum, so a
/// choice made here is a request. What cannot be honoured falls back to
/// [`Present::Vsync`], which every driver has, and says so in the log rather
/// than quietly doing something else.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Present {
    /// Wait for the compositor. No tearing, and the frame rate is the
    /// panel's.
    #[default]
    Vsync,
    /// Present the newest finished frame and discard the rest. No tearing, and
    /// a frame that finishes late is dropped rather than delaying the next
    /// one. This is the lowest latency a composited desktop can offer.
    Newest,
    /// Present immediately. Tears, and exists because measuring the render
    /// path without the compositor's clock in the way is the only way to know
    /// what the path costs.
    Immediate,
}

/// Cursor shapes an operator can choose.
///
/// A child that sets a shape with DECSCUSR overrides this; the setting is the
/// shape a child that says nothing gets.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum CursorShape {
    /// A filled cell.
    #[default]
    Block,
    /// A vertical bar at the left of the cell.
    Bar,
    /// A rule along the bottom of the cell.
    Underline,
}

/// Everything about the pane an operator can change.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct PaneTheme {
    /// Colours.
    pub palette: Palette,
    /// Font families, most preferred first.
    pub families: Vec<String>,
    /// Type size in points, before the display's scale is applied.
    pub size_pt: f32,
    /// Row height as a percentage of the font's own line height.
    pub line_height_pct: u16,
    /// Cell width as a percentage of the font's own advance.
    pub cell_width_pct: u16,
    /// Cursor shape a child has not overridden.
    pub cursor_shape: CursorShape,
    /// Whether that cursor blinks.
    pub cursor_blink: bool,
    /// Milliseconds per blink phase.
    pub blink_interval_ms: u16,
    /// Rows one wheel notch moves.
    pub scroll_lines_per_notch: u16,
    /// Characters that continue a word for a double-click.
    pub word_chars: String,
    /// Pane background opacity, as a percentage.
    pub opacity_pct: u8,
    /// How frames reach the screen.
    pub present: Present,
}

impl Default for PaneTheme {
    fn default() -> Self {
        Self {
            palette: Palette::default(),
            families: vitrum_grid::font::DEFAULT_FAMILIES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            size_pt: 12.0,
            line_height_pct: 100,
            cell_width_pct: 100,
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            blink_interval_ms: 600,
            scroll_lines_per_notch: 3,
            word_chars: super::select::DEFAULT_WORD_CHARS.to_owned(),
            opacity_pct: 100,
            present: Present::Newest,
        }
    }
}

/// Smallest type the pane will render. Below this the glyphs are a texture, not
/// text, and the grid is large enough that a resize storm is expensive.
pub(crate) const MIN_SIZE_PT: f32 = 6.0;
/// Largest. Past this one cell is most of a small window.
pub(crate) const MAX_SIZE_PT: f32 = 96.0;
/// Narrowest and widest a cell may be squeezed, as a percentage.
pub(crate) const MIN_METRIC_PCT: u16 = 50;
/// Widest.
pub(crate) const MAX_METRIC_PCT: u16 = 300;

impl PaneTheme {
    /// Hold every value inside the range the renderer and the emulator accept.
    ///
    /// Clamping rather than refusing. These arrive from a settings file an
    /// operator may have edited by hand, and a pane that refuses to start
    /// because a number is out of range is worse than one that draws at the
    /// nearest size it can.
    pub(crate) fn clamped(mut self) -> Self {
        if !self.size_pt.is_finite() {
            self.size_pt = 12.0;
        }
        self.size_pt = self.size_pt.clamp(MIN_SIZE_PT, MAX_SIZE_PT);
        self.line_height_pct = self.line_height_pct.clamp(MIN_METRIC_PCT, MAX_METRIC_PCT);
        self.cell_width_pct = self.cell_width_pct.clamp(MIN_METRIC_PCT, MAX_METRIC_PCT);
        self.blink_interval_ms = self.blink_interval_ms.max(50);
        self.scroll_lines_per_notch = self.scroll_lines_per_notch.max(1);
        self.opacity_pct = self.opacity_pct.clamp(10, 100);
        if self.families.iter().all(|f| f.trim().is_empty()) {
            self.families = vitrum_grid::font::DEFAULT_FAMILIES
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
        }
        self
    }

    /// The font stack this theme asks for at `scale` device pixels per logical
    /// pixel.
    ///
    /// Points to pixels through the display's scale, not through a constant.
    /// A pane that converts at 96 dpi draws 12 pt type at 16 px on a panel
    /// where the rest of the window is drawing it at 28, which is the whole
    /// reason this product measures density rather than trusting the toolkit.
    pub(crate) fn font_config(&self, scale: f64) -> FontConfig {
        let px = f64::from(self.size_pt) * scale;
        FontConfig {
            families: self.families.clone(),
            size_px: px.clamp(1.0, f64::from(vitrum_grid::font::MAX_SIZE_PX)) as f32,
            ..FontConfig::default()
        }
    }

    /// The grid's default style: the colours a cell the child never wrote has.
    pub(crate) fn default_style(&self) -> Style {
        Style {
            fg: self.palette.foreground,
            bg: self.background_with_opacity(),
            ..Style::DEFAULT
        }
    }

    /// The background with the operator's opacity applied.
    ///
    /// Alpha rather than a blend against a guess at what is behind the window:
    /// the compositor knows what is behind and the pane does not, and blending
    /// here would make a transparent pane opaque over anything the compositor
    /// later moved.
    pub(crate) fn background_with_opacity(&self) -> Rgba {
        let bg = self.palette.background;
        let a = (u32::from(self.opacity_pct) * 255 / 100).min(255) as u8;
        Rgba::new(bg.r, bg.g, bg.b, a)
    }

    /// Palette entries to push into the emulator, as index and colour.
    ///
    /// All sixteen. The emulator preserves a per-index override a program set
    /// with OSC 4, so pushing an entry the program has already claimed does
    /// not take the program's colour away.
    pub(crate) fn ansi_overrides(&self) -> [(u8, Rgba); 16] {
        core::array::from_fn(|i| (i as u8, self.palette.ansi[i]))
    }

    /// The colours a selected cell is painted in.
    ///
    /// Falls back to a derived pair when the operator's choice would be
    /// invisible. A selection the operator cannot see is a selection they
    /// cannot tell they made.
    pub(crate) fn selection_colours(&self) -> (Rgba, Rgba) {
        let (bg, fg) = (self.palette.selection_bg, self.palette.selection_fg);
        if contrast(bg, fg) >= MIN_CONTRAST {
            (bg, fg)
        } else {
            (bg, readable_on(bg))
        }
    }

    /// The background a search match is painted on. `current` picks the one the
    /// viewport is sitting on, which has to be distinguishable from the rest or
    /// stepping through matches shows no movement.
    pub(crate) fn match_colours(&self, current: bool) -> (Rgba, Rgba) {
        let bg = if current {
            self.palette.current_match_bg
        } else {
            self.palette.match_bg
        };
        (bg, readable_on(bg))
    }
}

/// Least ratio between two colours' relative luminance that counts as legible.
///
/// 3.0 rather than the 4.5 a body-text guideline asks for. Terminal type is
/// monospaced and dense and the ratio is being applied to a highlight over
/// arbitrary content, not to a headline; 4.5 would reject palettes that are
/// perfectly readable and replace the operator's choice with the pane's.
const MIN_CONTRAST: f32 = 3.0;

/// Relative luminance, as the sRGB definition gives it.
fn luminance(c: Rgba) -> f32 {
    fn channel(v: u8) -> f32 {
        let v = f32::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// Contrast ratio between two colours, from 1.0 to 21.0.
fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Black or white, whichever can be read on `bg`.
fn readable_on(bg: Rgba) -> Rgba {
    if contrast(bg, Rgba::BLACK) >= contrast(bg, Rgba::WHITE) {
        Rgba::BLACK
    } else {
        Rgba::WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WHY: these numbers arrive from a file an operator can edit by hand, and
    /// every one of them reaches an API that refuses or panics on a bad value.
    /// A zero font size divides into the pane's box; a zero blink interval is
    /// a timer that never sleeps; a zero wheel setting freezes the wheel.
    ///
    /// The class: every numeric field is clamped, and the assertion is over
    /// the extremes of the type rather than over a value somebody chose.
    #[test]
    fn every_number_an_operator_can_edit_is_held_inside_a_usable_range() {
        let hostile = PaneTheme {
            size_pt: 0.0,
            line_height_pct: 0,
            cell_width_pct: 0,
            blink_interval_ms: 0,
            scroll_lines_per_notch: 0,
            opacity_pct: 0,
            families: vec![String::new(), "   ".into()],
            ..PaneTheme::default()
        }
        .clamped();

        assert_eq!(hostile.size_pt, MIN_SIZE_PT);
        assert_eq!(hostile.line_height_pct, MIN_METRIC_PCT);
        assert_eq!(hostile.cell_width_pct, MIN_METRIC_PCT);
        assert!(hostile.blink_interval_ms >= 50);
        assert_eq!(hostile.scroll_lines_per_notch, 1);
        assert_eq!(hostile.opacity_pct, 10);
        assert!(
            !hostile.families.is_empty() && !hostile.families[0].trim().is_empty(),
            "an empty family list must fall back to a font that exists"
        );

        let huge = PaneTheme {
            size_pt: 100_000.0,
            line_height_pct: u16::MAX,
            cell_width_pct: u16::MAX,
            opacity_pct: 200,
            ..PaneTheme::default()
        }
        .clamped();
        assert_eq!(huge.size_pt, MAX_SIZE_PT);
        assert_eq!(huge.line_height_pct, MAX_METRIC_PCT);
        assert_eq!(huge.cell_width_pct, MAX_METRIC_PCT);
        assert_eq!(huge.opacity_pct, 100);

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let t = PaneTheme {
                size_pt: bad,
                ..PaneTheme::default()
            }
            .clamped();
            assert!(t.size_pt.is_finite(), "{bad} survived clamping");
            assert!((MIN_SIZE_PT..=MAX_SIZE_PT).contains(&t.size_pt));
        }
    }

    /// WHY: the pane's type was the wrong
    /// physical size on a dense panel. Points are a physical unit and pixels
    /// are not, so the conversion has to go through the display's measured
    /// scale. A pane that converts at a fixed ratio draws 12 pt at 16 px on a
    /// panel where the rest of the window draws it at 28.
    #[test]
    fn type_size_follows_the_displays_scale_rather_than_a_constant() {
        let theme = PaneTheme {
            size_pt: 12.0,
            ..PaneTheme::default()
        };
        let one = theme.font_config(1.0).size_px;
        let one_and_a_half = theme.font_config(1.5).size_px;
        let two = theme.font_config(2.0).size_px;

        assert!(one_and_a_half > one && two > one_and_a_half);
        assert!(
            (one_and_a_half / one - 1.5).abs() < 0.01,
            "1.5x did not scale by 1.5"
        );
        assert!((two / one - 2.0).abs() < 0.01);

        // A scale nobody should ever report still produces a font the
        // rasteriser accepts, because the alternative is a pane that fails to
        // start on a display that lied about its size.
        for scale in [0.0, 0.001, 1_000.0, f64::MAX] {
            let px = theme.font_config(scale).size_px;
            assert!(px >= 1.0, "{scale} gave {px}px");
            assert!(px <= vitrum_grid::font::MAX_SIZE_PX, "{scale} gave {px}px");
        }
    }

    /// WHY: an operator can pick a selection colour that is the same as their
    /// text colour, and a pane that renders it faithfully makes selected text
    /// vanish. Vanishing text looks like the selection failed.
    ///
    /// The invariant: whatever the operator chose, what is painted is legible.
    /// Asserted over a sweep of backgrounds rather than one, because the
    /// failing case is the one nobody tried.
    #[test]
    fn a_selection_is_legible_whatever_colours_the_operator_chose() {
        for level in (0u8..=255).step_by(5) {
            let same = Rgba::rgb(level, level, level);
            let theme = PaneTheme {
                palette: Palette {
                    selection_bg: same,
                    selection_fg: same,
                    ..Palette::default()
                },
                ..PaneTheme::default()
            };
            let (bg, fg) = theme.selection_colours();
            assert_eq!(bg, same, "the operator's background must be kept");
            assert!(
                contrast(bg, fg) >= MIN_CONTRAST,
                "level {level} gave a contrast of {}",
                contrast(bg, fg)
            );
        }

        // A choice that is already legible is left exactly alone.
        let theme = PaneTheme::default();
        let (bg, fg) = theme.selection_colours();
        assert_eq!(bg, theme.palette.selection_bg);
        assert_eq!(fg, theme.palette.selection_fg);
    }

    /// WHY: stepping through search matches shows no movement if the current
    /// match looks like every other one, and a match whose text cannot be read
    /// hides the thing the operator searched for.
    #[test]
    fn the_current_search_match_is_distinguishable_and_readable() {
        let theme = PaneTheme::default();
        let (bg, fg) = theme.match_colours(false);
        let (cur_bg, cur_fg) = theme.match_colours(true);

        assert_ne!(bg, cur_bg, "the current match looks like every other one");
        assert!(contrast(bg, fg) >= MIN_CONTRAST);
        assert!(contrast(cur_bg, cur_fg) >= MIN_CONTRAST);
    }

    /// WHY: opacity is alpha on the background colour, not a blend against a
    /// guessed backdrop. Blending here makes a transparent pane opaque over
    /// whatever the compositor moves behind the window afterwards.
    #[test]
    fn opacity_becomes_alpha_and_leaves_the_colour_alone() {
        let theme = PaneTheme::default();
        let opaque = theme.background_with_opacity();
        assert_eq!(opaque.a, 255);
        assert_eq!(
            (opaque.r, opaque.g, opaque.b),
            (
                theme.palette.background.r,
                theme.palette.background.g,
                theme.palette.background.b
            )
        );

        for pct in [10u8, 25, 50, 75, 100] {
            let t = PaneTheme {
                opacity_pct: pct,
                ..PaneTheme::default()
            };
            let c = t.background_with_opacity();
            assert_eq!(
                (c.r, c.g, c.b),
                (
                    t.palette.background.r,
                    t.palette.background.g,
                    t.palette.background.b
                ),
                "{pct}% changed the colour"
            );
            assert_eq!(c.a, (u32::from(pct) * 255 / 100) as u8, "{pct}%");
        }
    }

    /// WHY: a window opens and paints before the settings file has been
    /// parsed. A default of black on black is a pane that flashes, which is on
    /// the operator's list.
    #[test]
    fn the_colours_a_pane_paints_before_settings_are_read_are_legible() {
        let theme = PaneTheme::default();
        assert!(
            contrast(theme.palette.background, theme.palette.foreground) >= MIN_CONTRAST,
            "the default text is not readable on the default background"
        );
        assert!(
            contrast(theme.palette.background, theme.palette.cursor) >= 1.5,
            "the default cursor is invisible on the default background"
        );
        // Every ANSI colour has to be distinguishable from the background, or
        // an agent that paints in it is painting in nothing.
        for (i, c) in theme.palette.ansi.iter().enumerate() {
            if i == 0 {
                // Index 0 is the theme's own black and is expected to be close
                // to the background; that is what makes it black.
                continue;
            }
            assert!(
                contrast(theme.palette.background, *c) >= 1.5,
                "ANSI {i} is invisible on the default background"
            );
        }
    }

    /// WHY: all sixteen entries are pushed, in index order, or an agent's
    /// colour lands on the wrong index and red output prints green.
    #[test]
    fn palette_overrides_are_indexed_in_the_standard_order() {
        let theme = PaneTheme::default();
        let overrides = theme.ansi_overrides();
        assert_eq!(overrides.len(), 16);
        for (i, (index, colour)) in overrides.iter().enumerate() {
            assert_eq!(usize::from(*index), i);
            assert_eq!(*colour, theme.palette.ansi[i]);
        }
    }

    /// WHY: the grid's default style is what fills every cell nobody wrote,
    /// including on the very first frame. It has to be the operator's
    /// background or the pane opens in the wrong colour and settles into the
    /// right one, which is a flash.
    #[test]
    fn the_grids_default_style_is_the_operators_own_colours() {
        let mut theme = PaneTheme::default();
        theme.palette.background = Rgba::rgb(9, 9, 9);
        theme.palette.foreground = Rgba::rgb(200, 200, 200);
        let style = theme.default_style();
        assert_eq!(style.fg, Rgba::rgb(200, 200, 200));
        assert_eq!(
            (style.bg.r, style.bg.g, style.bg.b),
            (9, 9, 9),
            "the default cell is not the operator's background"
        );
    }
}
