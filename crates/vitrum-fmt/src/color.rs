//! Fast lookup-table ANSI color and attribute encoder.
//!
//! Provides zero-allocation ANSI escape sequence formatting for terminal output.
//! Uses pre-computed static lookup tables for 16-color palettes and text attributes,
//! with fast lookup and formatting for 8-bit (256-color) and 24-bit RGB truecolor sequences.

use std::fmt::Write as _;

/// Pre-computed 16-color foreground ANSI sequences.
static FG_16_LUT: [&str; 16] = [
    "\x1b[30m", "\x1b[31m", "\x1b[32m", "\x1b[33m",
    "\x1b[34m", "\x1b[35m", "\x1b[36m", "\x1b[37m",
    "\x1b[90m", "\x1b[91m", "\x1b[92m", "\x1b[93m",
    "\x1b[94m", "\x1b[95m", "\x1b[96m", "\x1b[97m",
];

/// Pre-computed 16-color background ANSI sequences.
static BG_16_LUT: [&str; 16] = [
    "\x1b[40m", "\x1b[41m", "\x1b[42m", "\x1b[43m",
    "\x1b[44m", "\x1b[45m", "\x1b[46m", "\x1b[47m",
    "\x1b[100m", "\x1b[101m", "\x1b[102m", "\x1b[103m",
    "\x1b[104m", "\x1b[105m", "\x1b[106m", "\x1b[107m",
];

/// Reset sequence.
pub const RESET: &str = "\x1b[0m";

/// Text attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Attribute {
    /// Bold text (`ESC [ 1 m`)
    Bold = 0x01,
    /// Dim/faint text (`ESC [ 2 m`)
    Dim = 0x02,
    /// Italic text (`ESC [ 3 m`)
    Italic = 0x04,
    /// Underline text (`ESC [ 4 m`)
    Underline = 0x08,
    /// Slow blink (`ESC [ 5 m`)
    Blink = 0x10,
    /// Reverse video / invert (`ESC [ 7 m`)
    Reverse = 0x20,
    /// Hidden / conceal (`ESC [ 8 m`)
    Hidden = 0x40,
    /// Strikethrough / crossed-out (`ESC [ 9 m`)
    Strikethrough = 0x80,
}

static ATTR_LUT: [(Attribute, &str); 8] = [
    (Attribute::Bold, "\x1b[1m"),
    (Attribute::Dim, "\x1b[2m"),
    (Attribute::Italic, "\x1b[3m"),
    (Attribute::Underline, "\x1b[4m"),
    (Attribute::Blink, "\x1b[5m"),
    (Attribute::Reverse, "\x1b[7m"),
    (Attribute::Hidden, "\x1b[8m"),
    (Attribute::Strikethrough, "\x1b[9m"),
];

/// ANSI color choices: 16-color palette, 256-color 8-bit palette, or 24-bit RGB truecolor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnsiColor {
    /// 16 standard/bright ANSI color index (0..=15)
    Basic(u8),
    /// 256-color palette index (0..=255)
    Fixed(u8),
    /// 24-bit RGB truecolor (r, g, b)
    Rgb(u8, u8, u8),
}

impl AnsiColor {
    /// Write the foreground ANSI escape sequence into `out`.
    pub fn write_fg<W: std::fmt::Write>(&self, out: &mut W) -> std::fmt::Result {
        match *self {
            AnsiColor::Basic(idx) => {
                let code = FG_16_LUT[(idx & 0x0F) as usize];
                out.write_str(code)
            }
            AnsiColor::Fixed(idx) => {
                write!(out, "\x1b[38;5;{idx}m")
            }
            AnsiColor::Rgb(r, g, b) => {
                write!(out, "\x1b[38;2;{r};{g};{b}m")
            }
        }
    }

    /// Write the background ANSI escape sequence into `out`.
    pub fn write_bg<W: std::fmt::Write>(&self, out: &mut W) -> std::fmt::Result {
        match *self {
            AnsiColor::Basic(idx) => {
                let code = BG_16_LUT[(idx & 0x0F) as usize];
                out.write_str(code)
            }
            AnsiColor::Fixed(idx) => {
                write!(out, "\x1b[48;5;{idx}m")
            }
            AnsiColor::Rgb(r, g, b) => {
                write!(out, "\x1b[48;2;{r};{g};{b}m")
            }
        }
    }
}

/// A composite ANSI style containing foreground color, background color, and attribute bitflags.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    /// Optional foreground color.
    pub fg: Option<AnsiColor>,
    /// Optional background color.
    pub bg: Option<AnsiColor>,
    /// Bitmask of enabled [`Attribute`] flags.
    pub attrs: u8,
}

impl Style {
    /// Create a plain style with no colors or attributes.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            attrs: 0,
        }
    }

    /// Set foreground color.
    #[must_use]
    pub const fn fg(mut self, color: AnsiColor) -> Self {
        self.fg = Some(color);
        self
    }

    /// Set background color.
    #[must_use]
    pub const fn bg(mut self, color: AnsiColor) -> Self {
        self.bg = Some(color);
        self
    }

    /// Add an attribute flag.
    #[must_use]
    pub const fn attr(mut self, attr: Attribute) -> Self {
        self.attrs |= attr as u8;
        self
    }

    /// Render opening escape sequences for this style into `out`.
    pub fn write_start<W: std::fmt::Write>(&self, out: &mut W) -> std::fmt::Result {
        for (attr, seq) in &ATTR_LUT {
            if (self.attrs & (*attr as u8)) != 0 {
                out.write_str(seq)?;
            }
        }
        if let Some(fg) = self.fg {
            fg.write_fg(out)?;
        }
        if let Some(bg) = self.bg {
            bg.write_bg(out)?;
        }
        Ok(())
    }

    /// Render resetting escape sequence into `out`.
    pub fn write_reset<W: std::fmt::Write>(&self, out: &mut W) -> std::fmt::Result {
        out.write_str(RESET)
    }

    /// Format `text` wrapped with this style's ANSI codes into `out`.
    pub fn render<W: std::fmt::Write>(&self, text: &str, out: &mut W) -> std::fmt::Result {
        self.write_start(out)?;
        out.write_str(text)?;
        self.write_reset(out)
    }

    /// Format `text` into a styled string.
    #[must_use]
    pub fn paint(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() + 32);
        let _ = self.render(text, &mut out);
        out
    }
}
