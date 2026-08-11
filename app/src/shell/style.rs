//! The stylesheet the native widgets are painted by, and the provider that
//! keeps it current.
//!
//! # Why this exists as generated text
//!
//! The appearance of this product was decided once, in a web stylesheet, as a
//! set of custom properties: a colour ramp, a four-pixel spacing grid, a type
//! scale, ten status hues and two motion durations. GTK reads a CSS dialect
//! that has no custom properties for lengths, no `calc`, no `rem` worth
//! relying on and no cascade layers. Translating the sheet by hand would mean
//! writing every one of those numbers out at each use, which is how a design
//! system stops being one.
//!
//! So the tokens live here, in Rust, as the only copy, and the sheet is a
//! template full of `$token$` placeholders that this module substitutes.
//! [`tests::the_template_declares_no_literal_value`] refuses a colour or a
//! length written directly into the template, and
//! [`tests::every_placeholder_resolves`] refuses a placeholder no token
//! declares. Between them there is no way to spell a value that is not a
//! token, and no way for a token to go unresolved.
//!
//! # Why it is regenerated rather than switched
//!
//! Four preferences change what the sheet says: the theme, the density, the
//! text scale and the palette the terminal is painting with. GTK cannot
//! express any of them as a selector, because three of them are numbers.
//! Regenerating is the mechanism that works for all four, and it costs one
//! string build and one parse per change, which happens when a control is
//! operated and never on a frame.
//!
//! # Motion is a separate sheet
//!
//! [`MOTION`] holds every `transition` declaration in the product and nothing
//! else. Reduced motion drops the file instead of overriding it with zeroed
//! durations, so there is no state in which a transition exists with no time
//! to run in, and "does anything loop" is one file's question.

use parking_lot::Mutex;

use crate::state::live::{PanePalette, PaneSettings, ShellSettings};
use crate::state::{Density, ThemePref};

/// The structural sheet: every rule that is not motion.
const TEMPLATE: &str = include_str!("../../assets/shell.css");

/// Every transition the product declares.
const MOTION: &str = include_str!("../../assets/shell-motion.css");

/// The root font size the type and spacing scales are written against.
///
/// Both scales were authored in `rem` against a 16px root. GTK has no root to
/// be relative to, so the numbers are carried here as pixels at that root and
/// the text scale multiplies them. That makes the scale control an exact
/// multiplier on every length rather than a font-size change the fixed
/// pixel values in the old sheet ignored.
pub(crate) const ROOT_PX: f64 = 16.0;

/// `n` root-relative units as pixels, at the text scale in force.
///
/// The escape hatch for the handful of sizes GTK's CSS cannot state. A sheet
/// has no `max-width`, so a width cap is a `GtkScrolledWindow`'s
/// `max_content_width` in pixels, set from Rust. Reading that number through
/// here rather than writing `16.0` again is what keeps one copy of the scale.
pub(crate) fn rem(n: f64) -> f64 {
    n * ROOT_PX * f64::from(current().text_scale_pct) / 100.0
}

// ═══════════════════════════════════════════════════════════════════════════
// What the sheet is generated from
// ═══════════════════════════════════════════════════════════════════════════

/// Which of the two colour ramps to paint.
///
/// Resolved, unlike [`ThemePref`]: by the time a sheet is generated the
/// question "what does the desktop say" has an answer, and carrying the
/// preference this far would mean asking it again inside the generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scheme {
    Dark,
    Light,
}

impl Scheme {
    /// The scheme a preference resolves to right now.
    pub(crate) fn resolve(pref: ThemePref) -> Scheme {
        match pref {
            ThemePref::Light => Scheme::Light,
            ThemePref::Dark => Scheme::Dark,
            // Dark is also the answer when the desktop will not say, because
            // the base ramp is dark and that is the branch which changes
            // nothing rather than a guess dressed as a reading.
            ThemePref::System => match crate::ui::settings::system_theme() {
                Some(vitrum_os::theme::Theme::Light) => Scheme::Light,
                _ => Scheme::Dark,
            },
        }
    }
}

/// Everything a generated sheet depends on.
///
/// Four fields and no borrow. A sheet is built off the settings bus, on
/// whichever thread published, and handed to the main loop as a string.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Look {
    pub(crate) scheme: Scheme,
    pub(crate) density: Density,
    /// UI text scale in percent, already clamped by the settings layer.
    pub(crate) text_scale_pct: u16,
    pub(crate) reduce_motion: bool,
    /// The hues the operator's terminal is painting with, when it is painting
    /// with any. `None` leaves the chrome on the theme's own hues.
    pub(crate) hues: Option<ChromeHues>,
}

impl Look {
    /// What the two live snapshots add up to.
    pub(crate) fn from_live(shell: &ShellSettings, pane: &PaneSettings) -> Look {
        let scheme = Scheme::resolve(shell.theme);
        Look {
            scheme,
            density: shell.density,
            text_scale_pct: shell.text_scale_pct,
            reduce_motion: shell.reduce_motion,
            hues: pane.palette.map(|p| ChromeHues::from_palette(&p, scheme)),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The operator's own colours, in the chrome
// ═══════════════════════════════════════════════════════════════════════════

/// The hues the chrome borrows from the terminal.
///
/// Ten status colours and an accent, and deliberately not a surface or a text
/// colour. The chrome's light/dark choice is about the room the operator is
/// sitting in and the palette's is about the code they read; letting the
/// palette decide the window's background is how a light scheme lands inside a
/// dark frame and neither reads. What the palette does decide is every hue
/// that carries meaning, because a status word in a red the operator has never
/// seen is the complaint this closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChromeHues {
    pub(crate) accent: Rgb,
    pub(crate) working: Rgb,
    pub(crate) approval: Rgb,
    pub(crate) input: Rgb,
    pub(crate) failed: Rgb,
    pub(crate) done: Rgb,
    pub(crate) snoozed: Rgb,
    pub(crate) add: Rgb,
    pub(crate) del: Rgb,
    /// What the pane letterboxes to, so the frame around the grid is the
    /// grid's own background and not a near miss of it.
    pub(crate) terminal_bg: Rgb,
    pub(crate) terminal_fg: Rgb,
}

/// One opaque colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rgb(pub(crate) u8, pub(crate) u8, pub(crate) u8);

impl Rgb {
    /// `#rrggbb`.
    fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// The same colour at `a` alpha, as GTK spells it.
    fn rgba(self, a: f64) -> String {
        format!("rgba({}, {}, {}, {a})", self.0, self.1, self.2)
    }

    /// Relative luminance, sRGB, the WCAG definition.
    fn luminance(self) -> f64 {
        fn channel(v: u8) -> f64 {
            let v = f64::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.0) + 0.7152 * channel(self.1) + 0.0722 * channel(self.2)
    }

    /// Contrast ratio against another colour, 1.0 to 21.0.
    fn contrast(self, other: Rgb) -> f64 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// This colour moved toward `toward` by `t`.
    fn mix(self, toward: Rgb, t: f64) -> Rgb {
        let f = |a: u8, b: u8| {
            let v = f64::from(a) + (f64::from(b) - f64::from(a)) * t;
            v.round().clamp(0.0, 255.0) as u8
        };
        Rgb(
            f(self.0, toward.0),
            f(self.1, toward.1),
            f(self.2, toward.2),
        )
    }
}

/// Least contrast a borrowed hue may have against the surface it sits on.
///
/// 3.0 is the WCAG floor for large text and for a graphical object that
/// carries meaning, which is exactly what these are: a status word, a dot, a
/// rail. A palette tuned for a black terminal supplies colours that vanish on
/// a light chrome, and a status nobody can see is worse than a status in the
/// wrong colour.
const MIN_HUE_CONTRAST: f64 = 3.0;

/// Steps taken toward legibility before giving up and using the last one.
///
/// Bounded rather than a loop with a condition, so this terminates on a
/// palette whose colour cannot reach the floor at all: pure mid-grey on a
/// mid-grey surface never gets there, and a bisection that insists would spin.
const HUE_STEPS: u8 = 12;

impl ChromeHues {
    /// Read the chrome's hues out of the palette the grid is painting with.
    ///
    /// The ANSI slots are the standard order, so the meanings map directly:
    /// red is failure, green is done, yellow is waiting on the operator, blue
    /// is the accent, cyan is work in progress, magenta is input. The bright
    /// half is preferred on a dark chrome and the base half on a light one,
    /// which is what those two halves are for.
    pub(crate) fn from_palette(palette: &PanePalette, scheme: Scheme) -> ChromeHues {
        let slot = |base: usize| -> Rgb {
            let i = match scheme {
                Scheme::Dark => base + 8,
                Scheme::Light => base,
            };
            let c = palette.ansi[i];
            Rgb(c[0], c[1], c[2])
        };
        let surface = RAMP[scheme as usize].surface;
        let ink = RAMP[scheme as usize].fg_strong;
        let legible = |hue: Rgb| readable(hue, surface, ink);
        ChromeHues {
            accent: legible(slot(4)),
            working: legible(slot(6)),
            approval: legible(slot(3)),
            input: legible(slot(5)),
            failed: legible(slot(1)),
            done: legible(slot(2)),
            snoozed: legible(slot(4)),
            add: legible(slot(2)),
            del: legible(slot(1)),
            // Not run through the contrast clamp. This one is the grid's own
            // background, and the whole point is that the letterbox matches
            // the cells exactly.
            terminal_bg: Rgb(
                palette.background[0],
                palette.background[1],
                palette.background[2],
            ),
            terminal_fg: Rgb(
                palette.foreground[0],
                palette.foreground[1],
                palette.foreground[2],
            ),
        }
    }
}

/// `hue` if it reads against `surface`, otherwise the nearest step toward
/// `ink` that does.
fn readable(hue: Rgb, surface: Rgb, ink: Rgb) -> Rgb {
    if hue.contrast(surface) >= MIN_HUE_CONTRAST {
        return hue;
    }
    let mut best = hue;
    for step in 1..=HUE_STEPS {
        let candidate = hue.mix(ink, f64::from(step) / f64::from(HUE_STEPS));
        best = candidate;
        if candidate.contrast(surface) >= MIN_HUE_CONTRAST {
            break;
        }
    }
    best
}

// ═══════════════════════════════════════════════════════════════════════════
// The colour ramp
// ═══════════════════════════════════════════════════════════════════════════

/// One theme's colours.
///
/// Every field is an opaque colour. The washes the interface needs on top of
/// them, the row states, the soft status fills, the scroll thumb, are computed
/// from `ink` at a stated alpha rather than written out twice, because the two
/// copies of `rgba(255, 255, 255, 0.09)` in the old sheet had already drifted
/// to two different values by the time this was written.
struct Ramp {
    surface: Rgb,
    surface_raised: Rgb,
    surface_overlay: Rgb,
    surface_sunken: Rgb,
    fg: Rgb,
    fg_strong: Rgb,
    fg_muted: Rgb,
    fg_subtle: Rgb,
    accent: Rgb,
    accent_strong: Rgb,
    accent_text: Rgb,
    /// White on dark, black on light. Every translucent overlay is this
    /// colour at an alpha, which is what makes a row highlight read the same
    /// way on both themes.
    ink: Rgb,
    working: Rgb,
    approval: Rgb,
    input: Rgb,
    failed: Rgb,
    done: Rgb,
    ready: Rgb,
    snoozed: Rgb,
    add: Rgb,
    del: Rgb,
    terminal_bg: Rgb,
    terminal_fg: Rgb,
    /// Alpha the soft status fills and the row states are laid on at.
    wash: f64,
    /// Alpha of a hairline border.
    hairline: f64,
    /// Alpha of a hairline that separates two surfaces rather than edging one.
    hairline_strong: f64,
    /// Alpha of the wash a dialog lays over the window behind it.
    scrim: f64,
}

/// The two ramps, indexed by [`Scheme`].
///
/// The values are the ones the web stylesheet settled on, carried over
/// unchanged. Where the design layer and the sidebar disagreed, the design
/// layer wins: it loaded last and was therefore what shipped.
const RAMP: [Ramp; 2] = [
    // Dark.
    Ramp {
        surface: Rgb(0x13, 0x13, 0x16),
        surface_raised: Rgb(0x1b, 0x1b, 0x1f),
        surface_overlay: Rgb(0x21, 0x21, 0x27),
        surface_sunken: Rgb(0x08, 0x08, 0x0a),
        fg: Rgb(0xf2, 0xf3, 0xf6),
        fg_strong: Rgb(0xf2, 0xf3, 0xf6),
        fg_muted: Rgb(0xb4, 0xb4, 0xb4),
        fg_subtle: Rgb(0x92, 0x92, 0x92),
        accent: Rgb(0x2f, 0x66, 0xef),
        accent_strong: Rgb(0x2a, 0x58, 0xdc),
        accent_text: Rgb(0x6a, 0x95, 0xfc),
        ink: Rgb(0xff, 0xff, 0xff),
        working: Rgb(0x00, 0xbc, 0xff),
        approval: Rgb(0xff, 0xd2, 0x30),
        input: Rgb(0xa3, 0xb3, 0xff),
        failed: Rgb(0xff, 0xa2, 0xa2),
        done: Rgb(0x5e, 0xe9, 0xb5),
        ready: Rgb(0xb4, 0xb4, 0xb4),
        snoozed: Rgb(0x51, 0xa2, 0xff),
        add: Rgb(0x00, 0xd4, 0x92),
        del: Rgb(0xff, 0x64, 0x67),
        terminal_bg: Rgb(0x08, 0x08, 0x0a),
        terminal_fg: Rgb(0xf2, 0xf3, 0xf6),
        wash: 0.15,
        hairline: 0.09,
        hairline_strong: 0.16,
        scrim: 0.55,
    },
    // Light.
    Ramp {
        surface: Rgb(0xf4, 0xf4, 0xf5),
        surface_raised: Rgb(0xfa, 0xfa, 0xfa),
        surface_overlay: Rgb(0xff, 0xff, 0xff),
        surface_sunken: Rgb(0xff, 0xff, 0xff),
        fg: Rgb(0x27, 0x27, 0x2a),
        fg_strong: Rgb(0x27, 0x27, 0x2a),
        fg_muted: Rgb(0x4a, 0x4a, 0x52),
        fg_subtle: Rgb(0x5e, 0x5e, 0x67),
        accent: Rgb(0x2d, 0x62, 0xe6),
        accent_strong: Rgb(0x1f, 0x4b, 0xc4),
        accent_text: Rgb(0x1d, 0x4f, 0xd8),
        ink: Rgb(0x00, 0x00, 0x00),
        working: Rgb(0x00, 0x84, 0xd1),
        approval: Rgb(0xbb, 0x4d, 0x00),
        input: Rgb(0x4f, 0x39, 0xf6),
        failed: Rgb(0xc1, 0x00, 0x07),
        done: Rgb(0x00, 0x7a, 0x55),
        ready: Rgb(0x4a, 0x4a, 0x52),
        snoozed: Rgb(0x15, 0x5d, 0xfc),
        add: Rgb(0x00, 0x99, 0x66),
        del: Rgb(0xe7, 0x00, 0x0b),
        terminal_bg: Rgb(0xff, 0xff, 0xff),
        terminal_fg: Rgb(0x27, 0x27, 0x2a),
        wash: 0.12,
        hairline: 0.10,
        hairline_strong: 0.20,
        scrim: 0.35,
    },
];

// ═══════════════════════════════════════════════════════════════════════════
// The spacing and type scales
// ═══════════════════════════════════════════════════════════════════════════

/// One unit of the spacing grid, in pixels at [`ROOT_PX`].
///
/// Every length in the product is a multiple of this. The old sheet's compact
/// density broke the grid, putting an 18px head line and a 6px inset into a
/// list otherwise built on fours, and the pitch it produced landed on no grid
/// line anywhere else in the interface.
const U: f64 = 4.0;

/// A named length, in pixels at [`ROOT_PX`], before the text scale.
///
/// A slice of pairs rather than a struct with sixty fields, because the
/// template addresses these by name and a struct would need a name-to-field
/// match arm that is the same list written twice.
const LENGTHS: &[(&str, f64)] = &[
    ("u", U),
    ("space-0-5", U),
    ("space-1", U),
    ("space-1-5", 2.0 * U),
    ("space-2", 2.0 * U),
    ("space-2-5", 3.0 * U),
    ("space-3", 3.0 * U),
    ("space-4", 4.0 * U),
    ("space-5", 5.0 * U),
    ("space-6", 6.0 * U),
    ("space-7", 7.0 * U),
    ("space-8", 8.0 * U),
    ("space-10", 10.0 * U),
    ("space-12", 12.0 * U),
    ("space-16", 16.0 * U),
    ("inset", 4.0 * U),
    ("content-inset", 4.0 * U),
    ("row-inset", 4.0 * U),
    ("row-pad-block", 3.0 * U),
    ("control-gap", 2.0 * U),
    ("band-h", 12.0 * U),
    ("control-h", 8.0 * U),
    ("control-h-sm", 7.0 * U),
    ("control-pad-x", 4.0 * U),
    ("control-pad-x-lg", 3.0 * U),
    ("radius-sm", U),
    ("radius-md", 2.0 * U),
    ("radius-lg", 3.0 * U),
    ("radius-full", 999.0),
    ("toolbar-h", 12.0 * U),
    ("footer-h", 12.0 * U),
    ("search-h", 8.0 * U),
    ("card-h", 17.0 * U),
    ("slim-h", 11.0 * U),
    ("row-h", 8.0 * U),
    ("row-h-project", 8.0 * U),
    ("row-collapsed-h", 8.0 * U),
    ("section-h", 5.0 * U),
    ("line-head", 5.0 * U),
    ("row-gap", 2.0 * U),
    ("group-gap", 8.0 * U),
    ("band-gap", 4.0 * U),
    ("glyph-w", 5.0 * U),
    ("icon-sm", 4.0 * U),
    ("chevron-w", 3.0 * U),
    ("close-w", 6.0 * U),
    ("slot-min", 8.0 * U),
    ("dot-size", 2.0 * U),
    ("rail-w", U),
    ("resizer-w", 2.0 * U),
    ("titlebar-h", 10.0 * U),
    ("panebar-h", 7.0 * U),
    ("pane-pad", 2.0 * U),
    ("wsbar-h", 10.0 * U),
    ("wsbar-item-h", 8.0 * U),
    ("wsbar-count-min", 5.0 * U),
    ("set-head-h", 12.0 * U),
    ("set-control-h", 8.0 * U),
    ("set-tab-h", 8.0 * U),
    ("set-rail-w", 44.0 * U),
    ("set-measure", 104.0 * U),
    ("set-sheet-w", 208.0 * U),
    ("set-sheet-h", 152.0 * U),
    ("switch-w", 10.0 * U),
    ("switch-h", 6.0 * U),
    ("switch-pad", U),
    ("switch-knob-size", 4.0 * U),
    ("menu-w", 58.0 * U),
    ("menu-item-h", 7.0 * U),
    ("sidebar-width-min", 56.0 * U),
    ("sidebar-width-max", 112.0 * U),
    ("sidebar-width-collapsed", 12.0 * U),
    ("focus-ring-w", 2.0),
    ("hairline", 1.0),
];

/// The lengths compact density replaces.
///
/// Vertical rhythm only. Type steps are left alone because shrinking text is
/// what the scale control is for, and a density switch that also changed the
/// font size would make the two controls fight.
const COMPACT: &[(&str, f64)] = &[
    ("card-h", 15.0 * U),
    ("slim-h", 7.0 * U),
    ("row-collapsed-h", 6.0 * U),
    ("row-gap", U),
    ("line-head", 4.0 * U),
    ("space-2", U),
    ("space-2-5", 2.0 * U),
    ("space-3", 2.0 * U),
    ("space-4", 3.0 * U),
    ("content-inset", U),
    ("row-inset", 2.0 * U),
];

/// A named font size, in pixels at [`ROOT_PX`], before the text scale.
const TYPE: &[(&str, f64)] = &[
    ("text-glyph", 10.0),
    ("text-10", 11.0),
    ("text-11", 11.0),
    ("text-12", 12.0),
    ("text-13", 13.0),
    ("text-14", 13.0),
    ("text-15", 15.0),
    ("text-16", 15.0),
    ("text-17", 17.0),
    ("text-label", 11.0),
    ("text-xs", 12.0),
    ("text-sm", 13.0),
];

/// The interface face.
///
/// The web stack led with `-apple-system`, `BlinkMacSystemFont` and
/// `system-ui`, none of which name anything to fontconfig. What is left is the
/// families a Linux desktop actually has, in the order the old stack had them.
const FONT_UI: &str = "\"Segoe UI Variable Text\", \"Segoe UI\", Cantarell, Ubuntu, Roboto, \
                       \"Helvetica Neue\", \"Noto Sans\", \"DejaVu Sans\", Arial, sans-serif";

/// The face for a path, a branch, a chord or anything else that is quoted from
/// a machine.
const FONT_MONO: &str = "\"JetBrains Mono\", \"SF Mono\", Menlo, Consolas, \
                         \"Liberation Mono\", \"DejaVu Sans Mono\", monospace";

// ═══════════════════════════════════════════════════════════════════════════
// Motion
// ═══════════════════════════════════════════════════════════════════════════

/// The fast duration: anything that IS the feedback for a press or a hover.
const T_FAST_MS: u32 = 100;
/// The base duration: anything that is smoothing a surface behind information
/// that already changed instantly.
const T_BASE_MS: u32 = 140;
/// The curve everything uses. One curve, so nothing is subtly out of step.
const EASE_STANDARD: &str = "cubic-bezier(0.25, 0.1, 0.25, 1)";

// ═══════════════════════════════════════════════════════════════════════════
// Strips and slots the layout may not lose
// ═══════════════════════════════════════════════════════════════════════════

/// Every strip that shares an axis with the pane, and the length it always
/// occupies on that axis.
///
/// A strip above or beside the terminal that appears when there is something
/// to say and vanishes when there is not takes a line away from the pty, which
/// resizes it, which makes every agent in every session repaint its whole
/// screen. That is the flicker this product is being rejected for, and it is
/// not a widget bug: it is a layout in which a strip is allowed to have no
/// height.
///
/// So each of these is given the same fixed height in every state the sheet
/// can express, and the widget that owns it stays realised and empty rather
/// than hiding. [`tests::a_strip_beside_the_pane_never_changes_size`] reads
/// this list back out of the generated sheet.
#[cfg(test)]
pub(crate) const RESERVED_STRIPS: &[(&str, &str)] = &[
    ("rg-panebar", "panebar-h"),
    ("rg-titlebar", "titlebar-h"),
    ("rg-sidebar__floor", "footer-h"),
];

/// Every element that is empty until a fact resolves, and the length it
/// occupies while it is empty.
///
/// A branch name arrives from git, a time from the daemon, a disposition from
/// the model. Each of them lands after the row is already on screen, and an
/// element that is absent until then makes the row reflow under a reader who
/// is in the middle of it. The sheet gives the empty state the same box as the
/// filled one, so the arrival changes the ink and nothing else.
#[cfg(test)]
pub(crate) const RESERVED_SLOTS: &[(&str, &str)] = &[
    ("rg-session__branch", "line-head"),
    ("rg-session__time", "line-head"),
    ("rg-session__place", "line-head"),
    ("rg-session__worktree", "line-head"),
    ("rg-pill__word", "row-h"),
    ("rg-panebar__value", "panebar-h"),
    ("rg-conn__word", "row-h"),
];

// ═══════════════════════════════════════════════════════════════════════════
// Generation
// ═══════════════════════════════════════════════════════════════════════════

/// A length in the units GTK reads, with no trailing zeros.
///
/// Rounded to whole pixels. GTK renders a fractional length by rounding it
/// anyway, and two adjacent boxes that round in opposite directions are the
/// one-pixel seam that reads as a rendering flaw.
fn px(value: f64) -> String {
    format!("{}px", value.round() as i64)
}

/// Every `$name$` the template may use, and what it resolves to.
fn tokens(look: &Look) -> Vec<(String, String)> {
    let ramp = &RAMP[look.scheme as usize];
    let scale = f64::from(look.text_scale_pct) / 100.0;
    let hues = look.hues;
    let mut out: Vec<(String, String)> = Vec::with_capacity(200);
    let mut put = |name: &str, value: String| out.push((name.to_string(), value));

    // Lengths, compact first so the base value below is skipped for a name
    // the density replaces.
    let compact = look.density == Density::Compact;
    for (name, base) in LENGTHS {
        let value = if compact {
            COMPACT
                .iter()
                .find(|(n, _)| n == name)
                .map_or(*base, |(_, v)| *v)
        } else {
            *base
        };
        // The full-round radius is a shape and not a measurement: scaling it
        // with the text would make a pill on a large scale a rounded
        // rectangle at some sizes and not others.
        let scaled = if *name == "radius-full" {
            value
        } else {
            value * scale
        };
        put(name, px(scaled));
    }
    for (name, base) in TYPE {
        put(name, px(base * scale));
    }

    put("font-ui", FONT_UI.to_string());
    put("font-mono", FONT_MONO.to_string());
    put("weight-normal", "400".to_string());
    put("weight-medium", "500".to_string());
    put("weight-semibold", "600".to_string());

    put("t-fast", format!("{T_FAST_MS}ms"));
    put("t-base", format!("{T_BASE_MS}ms"));
    put("scrim", Rgb(0, 0, 0).rgba(ramp.scrim));
    put("ease-standard", EASE_STANDARD.to_string());

    // A row that is on its way somewhere is dimmed rather than removed, so
    // the list does not reflow while the daemon settles it.
    put("recede-opacity", "0.7".to_string());

    // Surfaces and text.
    put("surface", ramp.surface.hex());
    put("surface-raised", ramp.surface_raised.hex());
    put("surface-overlay", ramp.surface_overlay.hex());
    put("surface-sunken", ramp.surface_sunken.hex());
    put("fg", ramp.fg.hex());
    put("fg-strong", ramp.fg_strong.hex());
    put("fg-muted", ramp.fg_muted.hex());
    put("fg-subtle", ramp.fg_subtle.hex());
    put("fg-dim", ramp.fg_subtle.hex());
    put("fg-faint", ramp.fg_subtle.hex());

    // Washes, all one ink at a stated alpha.
    put("fill-rest", ramp.ink.rgba(0.04));
    put("row-hover", ramp.ink.rgba(0.09));
    put("row-active", ramp.ink.rgba(0.16));
    put("row-selected", ramp.ink.rgba(0.16));
    put("border", ramp.ink.rgba(ramp.hairline));
    put("border-strong", ramp.ink.rgba(ramp.hairline_strong));
    put("scroll-thumb", ramp.ink.rgba(0.14));
    put("scroll-thumb-hover", ramp.ink.rgba(0.24));

    // The accent, and the palette's blue when there is one.
    let accent = hues.map_or(ramp.accent, |h| h.accent);
    put("accent", accent.hex());
    put("accent-strong", hues.map_or(ramp.accent_strong, |h| h.accent).hex());
    put("accent-text", hues.map_or(ramp.accent_text, |h| h.accent).hex());
    put("accent-wash", accent.rgba(ramp.wash));
    put("focus-ring", accent.hex());
    put("focus-halo", accent.rgba(0.26));

    // The ten status hues, each with the soft fill behind it.
    let hue = |name: &str, theme: Rgb, from_palette: Option<Rgb>| -> (Rgb, String, String) {
        let c = from_palette.unwrap_or(theme);
        (c, format!("hue-{name}"), format!("hue-{name}-soft"))
    };
    for (colour, solid, soft) in [
        hue("working", ramp.working, hues.map(|h| h.working)),
        hue("approval", ramp.approval, hues.map(|h| h.approval)),
        hue("input", ramp.input, hues.map(|h| h.input)),
        hue("failed", ramp.failed, hues.map(|h| h.failed)),
        // Woke is approval's colour on purpose: both mean the row wants the
        // operator, and two ambers would be two meanings the eye cannot tell
        // apart anyway.
        hue("woke", ramp.approval, hues.map(|h| h.approval)),
        hue("done", ramp.done, hues.map(|h| h.done)),
        hue("ready", ramp.ready, None),
        hue("snoozed", ramp.snoozed, hues.map(|h| h.snoozed)),
        hue("add", ramp.add, hues.map(|h| h.add)),
        hue("del", ramp.del, hues.map(|h| h.del)),
    ] {
        put(&solid, colour.hex());
        put(&soft, colour.rgba(ramp.wash));
    }

    // The terminal's own surfaces, so the letterbox is the grid's background
    // and not a near miss of it.
    let term_bg = hues.map_or(ramp.terminal_bg, |h| h.terminal_bg);
    let term_fg = hues.map_or(ramp.terminal_fg, |h| h.terminal_fg);
    put("terminal-bg", term_bg.hex());
    put("terminal-fg", term_fg.hex());
    put("terminal-selection", accent.rgba(0.34));

    // The two shadows GTK can draw without a compositor pass per frame.
    put(
        "shadow-1",
        format!("0 1px 2px {}", Rgb(0, 0, 0).rgba(0.26)),
    );
    put(
        "shadow-2",
        format!(
            "0 2px 4px {}, 0 10px 28px {}",
            Rgb(0, 0, 0).rgba(0.28),
            Rgb(0, 0, 0).rgba(0.42)
        ),
    );

    out
}

/// Substitute every `$name$` in `template`.
///
/// Unknown names are collected rather than left in place or panicked on: the
/// caller is a test that wants the whole list, and production has already been
/// through that test.
fn render(template: &str, tokens: &[(String, String)]) -> (String, Vec<String>) {
    let mut out = String::with_capacity(template.len() * 2);
    let mut unknown = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('$') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('$') else {
            // An unpaired marker is a typo, and leaving it in the sheet would
            // make GTK drop the rule it is in and nothing else.
            unknown.push(after.to_string());
            break;
        };
        let name = &after[..close];
        match tokens.iter().find(|(n, _)| n == name) {
            Some((_, value)) => out.push_str(value),
            None => unknown.push(name.to_string()),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    (out, unknown)
}

/// The sheet this look paints.
///
/// Motion is appended rather than overridden, so a reader who asked for none
/// gets a sheet with no `transition` in it at all.
pub(crate) fn stylesheet(look: &Look) -> String {
    let tokens = tokens(look);
    let (mut css, _) = render(TEMPLATE, &tokens);
    if !look.reduce_motion {
        let (motion, _) = render(MOTION, &tokens);
        css.push('\n');
        css.push_str(&motion);
    }
    css
}

// ═══════════════════════════════════════════════════════════════════════════
// The provider
// ═══════════════════════════════════════════════════════════════════════════

/// What the sheet should say right now.
///
/// Global because the two halves arrive on two subscriptions and either can
/// fire first. Holding the last of each is what lets a palette change
/// regenerate a sheet whose theme came from the other publish.
static WANTED: Mutex<Option<Look>> = Mutex::new(None);

/// Recompute [`WANTED`] from whichever half changed, and say whether the
/// answer moved.
///
/// Returning the decision rather than acting on it keeps this callable from a
/// test with no toolkit anywhere near it.
fn wanted(shell: &ShellSettings, pane: &PaneSettings) -> bool {
    let next = Look::from_live(shell, pane);
    let mut held = WANTED.lock();
    if held.as_ref() == Some(&next) {
        return false;
    }
    *held = Some(next);
    true
}

/// The look in force, or the default one before anything has published.
fn current() -> Look {
    WANTED.lock().clone().unwrap_or(Look {
        scheme: Scheme::Dark,
        density: Density::Comfortable,
        text_scale_pct: 100,
        reduce_motion: false,
        hues: None,
    })
}

#[cfg(target_os = "linux")]
mod provider {
    use std::cell::RefCell;

    use gtk::prelude::*;

    use super::{current, stylesheet, wanted, Look};
    use crate::state::live;

    thread_local! {
        /// The one provider, on the thread that owns the main loop.
        ///
        /// A `CssProvider` is not `Send`, and the settings bus publishes from
        /// whichever thread ran the control. So the provider stays here and a
        /// publish only asks the main loop to reread [`super::WANTED`].
        static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    }

    thread_local! {
        /// Subscriptions, held for the life of the process.
        ///
        /// Dropping a [`live::Subscription`] unsubscribes, so these have to
        /// outlive the call that made them or the provider stops hearing about
        /// changes one statement after it starts.
        static HELD: RefCell<Vec<live::Subscription>> = const { RefCell::new(Vec::new()) };
    }

    /// Install the stylesheet on `display` and keep it current.
    ///
    /// Priority is `APPLICATION`, which is above the desktop theme and below
    /// anything a widget sets on itself. That ordering is the whole reason a
    /// widget never needs an inline colour: this sheet already beats the
    /// theme that would otherwise paint a `GtkBox`.
    ///
    /// Called once, before the first window is shown, because a provider added
    /// after a widget is realised repaints it, and the repaint is visible.
    pub(crate) fn install(display: &gdk::Display) {
        let provider = gtk::CssProvider::new();
        let screen = display.default_screen();
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        PROVIDER.with(|slot| *slot.borrow_mut() = Some(provider));

        // Seed from what the bus already holds, so the first paint is
        // correct rather than default-then-corrected.
        wanted(&live::shell_settings(), &live::pane_settings());
        apply(&current());

        let shell = live::subscribe_shell(|shell| {
            if wanted(shell, &live::pane_settings()) {
                schedule();
            }
        });
        let pane = live::subscribe_pane(|pane| {
            if wanted(&live::shell_settings(), pane) {
                schedule();
            }
        });
        HELD.with(|held| held.borrow_mut().extend([shell, pane]));
    }

    /// Ask the main loop to repaint with whatever [`super::WANTED`] now says.
    ///
    /// An idle callback and not a direct call: a publish arrives on the thread
    /// that ran the control, and touching a `CssProvider` from there is a data
    /// race on GTK's own state. The closure carries no data, so the `Send`
    /// bound an idle source needs is satisfied by having nothing to send.
    fn schedule() {
        glib::idle_add_once(|| apply(&current()));
    }

    /// Parse a sheet into the provider.
    ///
    /// A parse error is logged and the old sheet is left in place. The
    /// alternative is a window painted by the desktop theme, which looks like
    /// a different product; keeping the last sheet that parsed keeps the
    /// window readable while the log says what broke.
    fn apply(look: &Look) {
        let css = stylesheet(look);
        PROVIDER.with(|slot| {
            let slot = slot.borrow();
            let Some(provider) = slot.as_ref() else {
                return;
            };
            if let Err(err) = provider.load_from_data(css.as_bytes()) {
                tracing::error!(%err, "the stylesheet did not parse; keeping the last one");
            }
        });
    }
}

#[cfg(target_os = "linux")]
pub(crate) use provider::install;

/// Every class the sheet paints, so a widget module can be checked against it.
///
/// Read by [`tests::the_class_vocabulary_is_complete_and_named`]. The widget
/// modules add these to a `GtkStyleContext` and set no colour or length of
/// their own.
#[cfg(test)]
pub(crate) fn classes() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    let mut rest = TEMPLATE;
    while let Some(at) = rest.find(".rg-") {
        let after = &rest[at + 1..];
        let end = after
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .unwrap_or(after.len());
        // The slice is into a `&'static str`, so the name outlives the call.
        let name: &'static str = &TEMPLATE[TEMPLATE.len() - after.len()..][..end];
        if !out.contains(&name) {
            out.push(name);
        }
        rest = &after[end..];
    }
    out
}

#[cfg(test)]
mod tests;
