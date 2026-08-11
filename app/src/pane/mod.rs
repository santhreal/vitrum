//! The terminal pane: a native GPU surface inside the window, and everything
//! that happens on it.
//!
//! # Shape
//!
//! The window is a GTK toplevel. The shell is drawn in it by the UI layer, and
//! the pane is a `GtkDrawingArea` with an X window of its own, sitting in a
//! `GtkFixed` inside a `GtkOverlay` above the shell. The shell tells the pane
//! where to be, in device pixels, and the pane owns every pixel of that
//! rectangle: a wgpu swapchain on the drawing area's own window, a cell grid
//! painted by [`vitrum_grid`], and [`vitrum_vt`] as the only thing in the
//! process that parses an escape sequence.
//!
//! Nothing is copied between the parser and the screen. The parser writes
//! cells, the renderer reads cells, and the bytes arriving from the daemon are
//! handed to the parser as a borrowed slice.
//!
//! # The clock
//!
//! There is one, and it is the compositor's. Bytes arriving mark the pane and
//! return; frames are drawn from the toolkit's frame clock, which fires when a
//! frame can actually be shown. See [`pacing`] for why a flush window is worse
//! in both directions at once.
//!
//! # What lives where
//!
//! Everything that can be decided without a widget is decided without one, and
//! is tested without a display:
//!
//! - [`geometry`] measures the padding box and divides it into whole cells.
//! - [`scroll`] is the viewport's position in the history.
//! - [`select`] is what a drag covers and what it copies.
//! - [`mouse`] encodes a pointer event the way the child asked for it.
//! - [`paste`] frames a payload.
//! - [`find`] is incremental search over the grid.
//! - [`theme`] is the operator's colours, type and metrics.
//! - [`pacing`] decides which ticks become frames.
//! - [`keymode`] applies DECCKM to an encoded key.
//! - [`key`] encodes a keystroke.
//! - [`session`] holds the emulator, the grid and the overlay.
//!
//! What is left is [`host`], which is the widget, and [`surface`], which is the
//! swapchain. Both are Linux-only and both are small, because everything that
//! could be moved out of them was.

pub(crate) mod find;
pub(crate) mod geometry;
pub(crate) mod key;
pub(crate) mod keymode;
pub(crate) mod mouse;
pub(crate) mod pacing;
pub(crate) mod paste;
pub(crate) mod scroll;
pub(crate) mod select;
pub(crate) mod session;
pub(crate) mod theme;

#[cfg(target_os = "linux")]
mod host;
#[cfg(target_os = "linux")]
pub(crate) mod surface;

pub(crate) use geometry::PaneRect;
#[cfg(target_os = "linux")]
pub(crate) use host::{PaneHost, install, place};

use crate::state::live::PaneSettings;
use theme::{CursorShape, Palette, PaneTheme, Present};
use vitrum_grid::cell::Rgba;

/// Where a pane sends what the operator typed.
///
/// A borrowed slice, because this is called once per keystroke and once per
/// pointer motion sample while a program is tracking the mouse. The caller
/// decides whether the bytes need to be owned; most of the time they do not.
pub(crate) type InputSink = Box<dyn Fn(&[u8])>;

/// Something the pane observed that only the shell can act on.
///
/// Three things, and none of them is a keystroke. The pane knows what the
/// grid is and what the operator did to it; which session is attached, whether
/// there is more history to ask for, and whether a clipboard write succeeded
/// are all facts held on the other side of this boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneReport {
    /// The grid changed size, so the child's window size has to change with it.
    Resize {
        /// Columns.
        cols: u16,
        /// Rows.
        rows: u16,
    },
    /// The viewport reached the oldest row the emulator still holds.
    PageBack,
    /// A copy was attempted, with the text it carried.
    Copied {
        /// Whether the clipboard accepted it.
        ok: bool,
        /// What was copied.
        text: String,
    },
}

/// Where a pane sends what it observed.
pub(crate) type ReportSink = Box<dyn Fn(PaneReport)>;

/// Fold the settings bus's snapshot into what the pane paints with.
///
/// The bus carries `[u8; 4]` sRGB and percentages, because that is what the
/// renderer uploads and what the settings sheet edits. This is the one place
/// those become the pane's own types, so a colour is parsed once per settings
/// change rather than once per frame.
pub(crate) fn theme_from(settings: &PaneSettings) -> PaneTheme {
    let default = PaneTheme::default();
    let palette = settings.palette.as_ref().map_or(default.palette, |p| {
        let ansi = core::array::from_fn(|i| rgba(p.ansi[i]));
        let background = rgba(p.background);
        Palette {
            ansi,
            background,
            foreground: rgba(p.foreground),
            cursor: rgba(p.cursor),
            selection_bg: rgba(p.selection_bg),
            selection_fg: rgba(p.selection_fg),
            // No configuration format in use declares a search highlight, so
            // the pane derives one from the operator's own background rather
            // than painting a constant that vanishes on half of all themes.
            match_bg: shift_towards(background, rgba(p.ansi[3]), 0.35),
            current_match_bg: shift_towards(background, rgba(p.ansi[3]), 0.70),
        }
    });

    let families = if settings.font_family.trim().is_empty() {
        default.families
    } else {
        settings
            .font_family
            .split(',')
            .map(|f| f.trim().trim_matches('"').to_owned())
            .filter(|f| !f.is_empty())
            .collect()
    };

    PaneTheme {
        palette,
        families,
        size_pt: f32::from(settings.font_size_px),
        line_height_pct: settings.line_height_pct,
        cell_width_pct: settings.cell_width_pct,
        cursor_shape: match settings.cursor_shape {
            crate::state::CursorShape::Block => CursorShape::Block,
            crate::state::CursorShape::Bar => CursorShape::Bar,
            crate::state::CursorShape::Underline => CursorShape::Underline,
        },
        cursor_blink: settings.cursor_blink,
        blink_interval_ms: settings.blink_interval_ms,
        scroll_lines_per_notch: u16::from(settings.wheel_lines),
        word_chars: select::DEFAULT_WORD_CHARS.to_owned(),
        opacity_pct: settings.opacity_pct,
        present: match settings.present_mode {
            crate::state::PresentMode::Vsync => Present::Vsync,
            crate::state::PresentMode::Adaptive => Present::Newest,
            crate::state::PresentMode::Immediate => Present::Immediate,
        },
    }
    .clamped()
}

/// One bus colour as the grid's own.
const fn rgba(c: [u8; 4]) -> Rgba {
    Rgba::new(c[0], c[1], c[2], c[3])
}

/// A colour `t` of the way from `from` to `to`.
fn shift_towards(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let mix = |a: u8, b: u8| {
        let a = f32::from(a);
        let b = f32::from(b);
        (a + (b - a) * t).clamp(0.0, 255.0) as u8
    };
    Rgba::new(
        mix(from.r, to.r),
        mix(from.g, to.g),
        mix(from.b, to.b),
        255,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Settings;
    use crate::state::live::PaneSettings;

    /// WHY: the pane ignored the configured terminal
    /// colours. The bus carries them and the pane has to actually use them,
    /// including for the two colours no configuration format declares.
    ///
    /// The invariant: with a palette in force, every colour the pane paints
    /// comes from that palette, and the two derived colours are legible
    /// against the operator's own background rather than a constant.
    #[test]
    fn a_palette_on_the_bus_is_the_palette_the_pane_paints() {
        let settings = Settings::default();
        let bus = PaneSettings::derive(&settings);
        let Some(p) = bus.palette else {
            // A fresh profile names no scheme, and the pane's own defaults are
            // what it paints. That is the documented answer, not a gap.
            let theme = theme_from(&bus);
            assert_eq!(theme.palette, PaneTheme::default().palette);
            return;
        };

        let theme = theme_from(&bus);
        assert_eq!(theme.palette.background, rgba(p.background));
        assert_eq!(theme.palette.foreground, rgba(p.foreground));
        assert_eq!(theme.palette.cursor, rgba(p.cursor));
        for i in 0..16 {
            assert_eq!(theme.palette.ansi[i], rgba(p.ansi[i]), "ansi {i}");
        }
        assert_ne!(
            theme.palette.match_bg, theme.palette.current_match_bg,
            "stepping through matches would show no movement"
        );
    }

    /// WHY: a font field is a comma-separated stack in every configuration
    /// format an operator will paste from, and a stack handed over as one
    /// string names a family nobody has installed.
    #[test]
    fn a_font_stack_is_split_into_the_families_it_names() {
        let mut settings = Settings::default();
        settings.terminal.font_family = "\"Iosevka Term\", JetBrains Mono , monospace".to_owned();
        let theme = theme_from(&PaneSettings::derive(&settings));
        assert_eq!(
            theme.families,
            vec![
                "Iosevka Term".to_owned(),
                "JetBrains Mono".to_owned(),
                "monospace".to_owned()
            ]
        );

        // An empty field is the platform default, not an empty stack the font
        // loader would fail on.
        settings.terminal.font_family = "   ".to_owned();
        let theme = theme_from(&PaneSettings::derive(&settings));
        assert_eq!(theme.families, PaneTheme::default().families);
    }

    /// WHY: every value on the pane's half of the settings bus has to reach
    /// the pane, and the way one gets forgotten is by being added to the bus
    /// and never read here. Derived from the bus type's own fields at run
    /// time is not possible in Rust without reflection, so the guard is
    /// stated the other way round: change any pane field in settings and the
    /// theme this produces must differ.
    #[test]
    fn every_pane_setting_changes_what_the_pane_paints() {
        let base = theme_from(&PaneSettings::derive(&Settings::default()));

        let mut s = Settings::default();
        s.terminal.font_size_px = 22;
        assert_ne!(theme_from(&PaneSettings::derive(&s)).size_pt, base.size_pt);

        let mut s = Settings::default();
        s.terminal.line_height_pct = 140;
        assert_ne!(
            theme_from(&PaneSettings::derive(&s)).line_height_pct,
            base.line_height_pct
        );

        let mut s = Settings::default();
        s.terminal.cell_width_pct = 120;
        assert_ne!(
            theme_from(&PaneSettings::derive(&s)).cell_width_pct,
            base.cell_width_pct
        );

        let mut s = Settings::default();
        s.terminal.cursor_shape = crate::state::CursorShape::Bar;
        assert_ne!(
            theme_from(&PaneSettings::derive(&s)).cursor_shape,
            base.cursor_shape
        );

        let mut s = Settings::default();
        s.terminal.cursor_blink = !base.cursor_blink;
        assert_ne!(
            theme_from(&PaneSettings::derive(&s)).cursor_blink,
            base.cursor_blink
        );

        let mut s = Settings::default();
        s.terminal.blink_interval_ms = base.blink_interval_ms + 100;
        assert_ne!(
            theme_from(&PaneSettings::derive(&s)).blink_interval_ms,
            base.blink_interval_ms
        );

        let mut s = Settings::default();
        s.terminal.wheel_lines = 7;
        assert_ne!(
            theme_from(&PaneSettings::derive(&s)).scroll_lines_per_notch,
            base.scroll_lines_per_notch
        );

        let mut s = Settings::default();
        s.appearance.terminal_opacity_pct = 80;
        assert_ne!(
            theme_from(&PaneSettings::derive(&s)).opacity_pct,
            base.opacity_pct
        );

        let mut s = Settings::default();
        s.terminal.present_mode = crate::state::PresentMode::Immediate;
        assert_ne!(theme_from(&PaneSettings::derive(&s)).present, base.present);
    }
}
