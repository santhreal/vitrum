//! Named colour palettes for the terminal grid.
//!
//! The grid is the one surface in this product whose colours are not a design
//! decision the shell gets to make. An operator arrives with a palette they
//! have read code in for years, and their prompt, their diff colours and their
//! agent's ANSI output are all tuned to it. So this is a real preference and
//! not a theme variant: it is independent of light/dark, and picking one here
//! does not move a single pixel of chrome.
//!
//! # One source of truth
//!
//! A palette is sixteen ANSI slots plus four surface colours, and it has to
//! reach two consumers: the native renderer, which paints the cell matrix
//! from these values directly, and the shell's own stylesheet, which borrows
//! the palette's hues so the chrome agrees with the grid. Writing the numbers
//! twice, once for the chrome and once in a Rust table, is how the two drift
//! and how the pane ends up a different black from the cells inside it.
//!
//! So the table below is the only copy. `shell::style` reads it through the
//! live settings bus, and [`TermPalette::colours`] hands the renderer the
//! same numbers.
//!
//! # Why these palettes
//!
//! Every entry is a palette with a published, stable definition that predates
//! this product, because the value of a named palette is entirely that the
//! operator already knows what it looks like. Inventing one would give them a
//! name that means nothing and colours they would have to audit themselves.
//!
//! [`TermPalette::Inherit`] is the default and is not a palette at all: it
//! emits nothing and lets the app theme's `--rg-terminal-*` tokens through, so
//! a fresh install behaves exactly as it did before this module existed.

use serde::{Deserialize, Serialize};

/// Which palette the terminal grid paints with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TermPalette {
    /// Follow the app theme. The grid uses `--rg-terminal-*` and the
    /// renderer's own ANSI defaults, which is what every build before this
    /// preference did.
    #[default]
    Inherit,
    SolarizedDark,
    SolarizedLight,
    GruvboxDark,
    Nord,
    Dracula,
    TokyoNight,
    OneHalfLight,
}

/// The colours of one palette.
///
/// `ansi` is the standard order: black, red, green, yellow, blue, magenta,
/// cyan, white, then the eight bright variants. The renderer indexes this
/// array by SGR colour number, so the order is load-bearing and is asserted
/// by [`tests::the_ansi_slots_are_in_the_standard_order`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colours {
    pub background: &'static str,
    pub foreground: &'static str,
    pub cursor: &'static str,
    pub selection: &'static str,
    pub ansi: [&'static str; 16],
}

/// Every selectable palette, in the order the settings control lists them.
///
/// [`TermPalette::Inherit`] leads because it is the default and because it is
/// the only entry that is not a palette; grouping it with the named ones would
/// suggest it is a taste rather than an opt-out.
pub const ALL: [TermPalette; 8] = [
    TermPalette::Inherit,
    TermPalette::SolarizedDark,
    TermPalette::SolarizedLight,
    TermPalette::GruvboxDark,
    TermPalette::Nord,
    TermPalette::Dracula,
    TermPalette::TokyoNight,
    TermPalette::OneHalfLight,
];

impl TermPalette {
    /// The value persisted in settings and compared in the settings control.
    ///
    /// Kept in step with the `serde` representation by
    /// [`tests::the_wire_slug_is_what_serde_writes`], because a control that
    /// round-trips through a different string than the file does is a
    /// preference that silently resets on restart.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            TermPalette::Inherit => "inherit",
            TermPalette::SolarizedDark => "solarized-dark",
            TermPalette::SolarizedLight => "solarized-light",
            TermPalette::GruvboxDark => "gruvbox-dark",
            TermPalette::Nord => "nord",
            TermPalette::Dracula => "dracula",
            TermPalette::TokyoNight => "tokyo-night",
            TermPalette::OneHalfLight => "one-half-light",
        }
    }

    /// Parse a slug back. Unknown input is [`TermPalette::Inherit`], which is
    /// the safe answer: a settings file written by a newer build names a
    /// palette this one does not have, and following the app theme is a
    /// readable terminal rather than a broken one.
    #[must_use]
    pub fn from_slug(slug: &str) -> TermPalette {
        ALL.into_iter()
            .find(|p| p.slug() == slug)
            .unwrap_or(TermPalette::Inherit)
    }

    /// What the settings control calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            TermPalette::Inherit => "Follow the app theme",
            TermPalette::SolarizedDark => "Solarized Dark",
            TermPalette::SolarizedLight => "Solarized Light",
            TermPalette::GruvboxDark => "Gruvbox Dark",
            TermPalette::Nord => "Nord",
            TermPalette::Dracula => "Dracula",
            TermPalette::TokyoNight => "Tokyo Night",
            TermPalette::OneHalfLight => "One Half Light",
        }
    }

    /// Whether the palette is a light one, so the control can say so.
    ///
    /// An operator running the app in dark mode who picks Solarized Light gets
    /// a white grid in a dark frame. That is allowed and is sometimes the
    /// point, but it should not be a surprise.
    #[must_use]
    pub const fn is_light(self) -> bool {
        matches!(
            self,
            TermPalette::SolarizedLight | TermPalette::OneHalfLight
        )
    }

    /// The colours, or `None` for [`TermPalette::Inherit`].
    #[must_use]
    pub const fn colours(self) -> Option<Colours> {
        Some(match self {
            TermPalette::Inherit => return None,

            // Ethan Schoonover, 2011. The base tones are the sixteen-colour
            // definition, not the eight-colour reduction.
            TermPalette::SolarizedDark => Colours {
                background: "#002b36",
                foreground: "#839496",
                cursor: "#93a1a1",
                selection: "rgba(88, 110, 117, 0.45)",
                ansi: [
                    "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198",
                    "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4",
                    "#93a1a1", "#fdf6e3",
                ],
            },
            TermPalette::SolarizedLight => Colours {
                background: "#fdf6e3",
                foreground: "#657b83",
                cursor: "#586e75",
                selection: "rgba(147, 161, 161, 0.45)",
                ansi: [
                    "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198",
                    "#eee8d5", "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4",
                    "#93a1a1", "#fdf6e3",
                ],
            },

            // Pavel Pertsev's gruvbox, dark medium.
            TermPalette::GruvboxDark => Colours {
                background: "#282828",
                foreground: "#ebdbb2",
                cursor: "#ebdbb2",
                selection: "rgba(168, 153, 132, 0.35)",
                ansi: [
                    "#282828", "#cc241d", "#98971a", "#d79921", "#458588", "#b16286", "#689d6a",
                    "#a89984", "#928374", "#fb4934", "#b8bb26", "#fabd2f", "#83a598", "#d3869b",
                    "#8ec07c", "#ebdbb2",
                ],
            },

            // Arctic Ice Studio's Nord, polar night through aurora.
            TermPalette::Nord => Colours {
                background: "#2e3440",
                foreground: "#d8dee9",
                cursor: "#d8dee9",
                selection: "rgba(76, 86, 106, 0.6)",
                ansi: [
                    "#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0",
                    "#e5e9f0", "#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead",
                    "#8fbcbb", "#eceff4",
                ],
            },

            // Zeno Rocha's Dracula.
            TermPalette::Dracula => Colours {
                background: "#282a36",
                foreground: "#f8f8f2",
                cursor: "#f8f8f2",
                selection: "rgba(68, 71, 90, 0.7)",
                ansi: [
                    "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd",
                    "#f8f8f2", "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df",
                    "#a4ffff", "#ffffff",
                ],
            },

            // enkia's Tokyo Night, the storm-adjacent default.
            TermPalette::TokyoNight => Colours {
                background: "#1a1b26",
                foreground: "#c0caf5",
                cursor: "#c0caf5",
                selection: "rgba(40, 52, 87, 0.8)",
                ansi: [
                    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff",
                    "#a9b1d6", "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7",
                    "#7dcfff", "#c0caf5",
                ],
            },

            // Son A. Pham's One Half, light.
            TermPalette::OneHalfLight => Colours {
                background: "#fafafa",
                foreground: "#383a42",
                cursor: "#383a42",
                selection: "rgba(189, 195, 199, 0.5)",
                ansi: [
                    "#383a42", "#e45649", "#50a14f", "#c18401", "#0184bc", "#a626a4", "#0997b3",
                    "#fafafa", "#4f525d", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd",
                    "#56b6c2", "#ffffff",
                ],
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Position of one palette, as an EXHAUSTIVE match.
    ///
    /// Same guard shape as `agent.rs`: `ALL` is hand-kept because Rust cannot
    /// enumerate variants, so the list has to be enforced rather than trusted.
    /// A ninth palette breaks this match, which stops the crate compiling.
    fn index(palette: TermPalette) -> usize {
        match palette {
            TermPalette::Inherit => 0,
            TermPalette::SolarizedDark => 1,
            TermPalette::SolarizedLight => 2,
            TermPalette::GruvboxDark => 3,
            TermPalette::Nord => 4,
            TermPalette::Dracula => 5,
            TermPalette::TokyoNight => 6,
            TermPalette::OneHalfLight => 7,
        }
    }

    #[test]
    fn all_names_every_palette_exactly_once() {
        let mut seen = [false; 8];
        for p in ALL {
            let at = index(p);
            assert!(!seen[at], "{} is listed twice", p.slug());
            seen[at] = true;
        }
        assert!(seen.iter().all(|hit| *hit), "ALL is missing a palette");
    }

    /// The control's value strings and the file's are the same strings.
    ///
    /// THE BUG this stops: the control writes `"tokyo-night"`, serde writes
    /// `"tokyoNight"`, and the select renders with nothing selected on every
    /// restart while the palette itself applies fine. That is a preference
    /// that looks unset and is not, which is worse than one that does not
    /// persist at all.
    #[test]
    fn the_wire_slug_is_what_serde_writes() {
        for p in ALL {
            let json = serde_json::to_string(&p).expect("a fieldless enum serialises");
            assert_eq!(
                json,
                format!("\"{}\"", p.slug()),
                "{:?} serialises as {json} but its slug is {}",
                p,
                p.slug()
            );
            assert_eq!(TermPalette::from_slug(p.slug()), p);
        }
    }

    /// A slug from a newer build must land on the app theme, not panic and not
    /// pick an arbitrary palette.
    #[test]
    fn an_unknown_slug_falls_back_to_the_app_theme() {
        assert_eq!(TermPalette::from_slug("catppuccin"), TermPalette::Inherit);
        assert_eq!(TermPalette::from_slug(""), TermPalette::Inherit);
    }

    /// Inherit must contribute nothing.
    ///
    /// This is the entire opt-out mechanism. If Inherit ever answered with a
    /// literal dark colour, light mode would ship a black terminal again,
    /// which is a bug this codebase has already had once.
    #[test]
    fn following_the_app_theme_overrides_nothing() {
        assert_eq!(TermPalette::Inherit.colours(), None);
    }

    /// Every named palette must define all twenty colours, non-empty.
    #[test]
    fn every_named_palette_is_complete() {
        for p in ALL.into_iter().filter(|p| *p != TermPalette::Inherit) {
            let c = p
                .colours()
                .unwrap_or_else(|| panic!("{} has no colours", p.slug()));
            for (n, slot) in c.ansi.iter().enumerate() {
                assert!(!slot.is_empty(), "{} ansi slot {n} is empty", p.slug());
            }
            for (name, value) in [
                ("background", c.background),
                ("foreground", c.foreground),
                ("cursor", c.cursor),
                ("selection", c.selection),
            ] {
                assert!(!value.is_empty(), "{} has no {name}", p.slug());
            }
        }
    }

    /// Foreground and background must not be the same colour.
    ///
    /// A transcription slip in a twenty-entry hex table is invisible on
    /// review and produces an unreadable terminal, so the one relationship
    /// that must hold is checked rather than eyeballed.
    #[test]
    fn no_palette_paints_text_in_the_background_colour() {
        for p in ALL.into_iter().filter(|p| *p != TermPalette::Inherit) {
            let c = p.colours().expect("named palettes have colours");
            assert_ne!(
                c.foreground.to_ascii_lowercase(),
                c.background.to_ascii_lowercase(),
                "{} is invisible",
                p.slug()
            );
        }
    }

    /// A light palette must actually be light, and a dark one dark.
    ///
    /// `is_light` drives a caption that warns the operator they are about to
    /// put a white grid in a dark window. A wrong flag makes that caption a
    /// lie, and the flag is hand-set, so it is derived from the background
    /// here instead of trusted.
    #[test]
    fn the_light_flag_agrees_with_the_background() {
        for p in ALL.into_iter().filter(|p| *p != TermPalette::Inherit) {
            let bg = p.colours().expect("named palettes have colours").background;
            assert_eq!(
                p.is_light(),
                luma(bg) > 127,
                "{} has luma {} but is_light() says {}",
                p.slug(),
                luma(bg),
                p.is_light()
            );
        }
    }

    /// Rec. 601 luma of an `#rrggbb` string, integer.
    ///
    /// Precision is irrelevant here: every colour these tests compare is far
    /// from the middle of the scale.
    fn luma(hex: &str) -> u32 {
        let n = u32::from_str_radix(hex.trim_start_matches('#'), 16)
            .expect("palette colours are #rrggbb");
        let (r, g, b) = ((n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff);
        (299 * r + 587 * g + 114 * b) / 1000
    }

    /// The sixteen slots must sit in SGR order.
    ///
    /// The renderer indexes `ansi` by colour number and reports nothing when
    /// the row is rotated: every glyph gets a plausible colour and the whole
    /// palette is wrong. A rotation is invisible on review of a twenty-entry
    /// hex table, so the relationships that hold for every named palette are
    /// checked, plus the endpoints of one table by value.
    ///
    /// Slot 8 is deliberately not required to be brighter than slot 0. In
    /// Solarized's sixteen-colour definition slot 8 is base03, which is the
    /// darkest tone in the table and darker than slot 0's base02. "Bright
    /// black is brighter" is a convention of the other palettes, not a
    /// property of the standard, and asserting it would pin a defect into
    /// Solarized instead of catching one.
    #[test]
    fn the_ansi_slots_are_in_the_standard_order() {
        for p in ALL.into_iter().filter(|p| *p != TermPalette::Inherit) {
            let c = p.colours().expect("named palettes have colours");
            assert!(
                luma(c.ansi[0]) < luma(c.ansi[7]),
                "{}: slot 0 must be black and slot 7 white",
                p.slug()
            );
            assert!(
                luma(c.ansi[7]) < luma(c.ansi[15]),
                "{}: slot 15 must be a brighter white than slot 7",
                p.slug()
            );
            let brightest = (0..16).max_by_key(|&i| luma(c.ansi[i])).expect("sixteen slots");
            assert_eq!(
                brightest,
                15,
                "{}: slot 15 must be the brightest of the sixteen, not slot {brightest}",
                p.slug()
            );
        }
        let nord = TermPalette::Nord.colours().expect("Nord has colours");
        assert_eq!(nord.ansi[0], "#3b4252", "Nord slot 0 moved");
        assert_eq!(nord.ansi[15], "#eceff4", "Nord slot 15 moved");
    }
}
