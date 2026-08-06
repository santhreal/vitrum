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

/// Pre-computed 256-color foreground ANSI sequences.
static FG_256_LUT: [&str; 256] = [
    "\x1b[38;5;0m", "\x1b[38;5;1m", "\x1b[38;5;2m", "\x1b[38;5;3m", "\x1b[38;5;4m", "\x1b[38;5;5m", "\x1b[38;5;6m", "\x1b[38;5;7m",
    "\x1b[38;5;8m", "\x1b[38;5;9m", "\x1b[38;5;10m", "\x1b[38;5;11m", "\x1b[38;5;12m", "\x1b[38;5;13m", "\x1b[38;5;14m", "\x1b[38;5;15m",
    "\x1b[38;5;16m", "\x1b[38;5;17m", "\x1b[38;5;18m", "\x1b[38;5;19m", "\x1b[38;5;20m", "\x1b[38;5;21m", "\x1b[38;5;22m", "\x1b[38;5;23m",
    "\x1b[38;5;24m", "\x1b[38;5;25m", "\x1b[38;5;26m", "\x1b[38;5;27m", "\x1b[38;5;28m", "\x1b[38;5;29m", "\x1b[38;5;30m", "\x1b[38;5;31m",
    "\x1b[38;5;32m", "\x1b[38;5;33m", "\x1b[38;5;34m", "\x1b[38;5;35m", "\x1b[38;5;36m", "\x1b[38;5;37m", "\x1b[38;5;38m", "\x1b[38;5;39m",
    "\x1b[38;5;40m", "\x1b[38;5;41m", "\x1b[38;5;42m", "\x1b[38;5;43m", "\x1b[38;5;44m", "\x1b[38;5;45m", "\x1b[38;5;46m", "\x1b[38;5;47m",
    "\x1b[38;5;48m", "\x1b[38;5;49m", "\x1b[38;5;50m", "\x1b[38;5;51m", "\x1b[38;5;52m", "\x1b[38;5;53m", "\x1b[38;5;54m", "\x1b[38;5;55m",
    "\x1b[38;5;56m", "\x1b[38;5;57m", "\x1b[38;5;58m", "\x1b[38;5;59m", "\x1b[38;5;60m", "\x1b[38;5;61m", "\x1b[38;5;62m", "\x1b[38;5;63m",
    "\x1b[38;5;64m", "\x1b[38;5;65m", "\x1b[38;5;66m", "\x1b[38;5;67m", "\x1b[38;5;68m", "\x1b[38;5;69m", "\x1b[38;5;70m", "\x1b[38;5;71m",
    "\x1b[38;5;72m", "\x1b[38;5;73m", "\x1b[38;5;74m", "\x1b[38;5;75m", "\x1b[38;5;76m", "\x1b[38;5;77m", "\x1b[38;5;78m", "\x1b[38;5;79m",
    "\x1b[38;5;80m", "\x1b[38;5;81m", "\x1b[38;5;82m", "\x1b[38;5;83m", "\x1b[38;5;84m", "\x1b[38;5;85m", "\x1b[38;5;86m", "\x1b[38;5;87m",
    "\x1b[38;5;88m", "\x1b[38;5;89m", "\x1b[38;5;90m", "\x1b[38;5;91m", "\x1b[38;5;92m", "\x1b[38;5;93m", "\x1b[38;5;94m", "\x1b[38;5;95m",
    "\x1b[38;5;96m", "\x1b[38;5;97m", "\x1b[38;5;98m", "\x1b[38;5;99m", "\x1b[38;5;100m", "\x1b[38;5;101m", "\x1b[38;5;102m", "\x1b[38;5;103m",
    "\x1b[38;5;104m", "\x1b[38;5;105m", "\x1b[38;5;106m", "\x1b[38;5;107m", "\x1b[38;5;108m", "\x1b[38;5;109m", "\x1b[38;5;110m", "\x1b[38;5;111m",
    "\x1b[38;5;112m", "\x1b[38;5;113m", "\x1b[38;5;114m", "\x1b[38;5;115m", "\x1b[38;5;116m", "\x1b[38;5;117m", "\x1b[38;5;118m", "\x1b[38;5;119m",
    "\x1b[38;5;120m", "\x1b[38;5;121m", "\x1b[38;5;122m", "\x1b[38;5;123m", "\x1b[38;5;124m", "\x1b[38;5;125m", "\x1b[38;5;126m", "\x1b[38;5;127m",
    "\x1b[38;5;128m", "\x1b[38;5;129m", "\x1b[38;5;130m", "\x1b[38;5;131m", "\x1b[38;5;132m", "\x1b[38;5;133m", "\x1b[38;5;134m", "\x1b[38;5;135m",
    "\x1b[38;5;136m", "\x1b[38;5;137m", "\x1b[38;5;138m", "\x1b[38;5;139m", "\x1b[38;5;140m", "\x1b[38;5;141m", "\x1b[38;5;142m", "\x1b[38;5;143m",
    "\x1b[38;5;144m", "\x1b[38;5;145m", "\x1b[38;5;146m", "\x1b[38;5;147m", "\x1b[38;5;148m", "\x1b[38;5;149m", "\x1b[38;5;150m", "\x1b[38;5;151m",
    "\x1b[38;5;152m", "\x1b[38;5;153m", "\x1b[38;5;154m", "\x1b[38;5;155m", "\x1b[38;5;156m", "\x1b[38;5;157m", "\x1b[38;5;158m", "\x1b[38;5;159m",
    "\x1b[38;5;160m", "\x1b[38;5;161m", "\x1b[38;5;162m", "\x1b[38;5;163m", "\x1b[38;5;164m", "\x1b[38;5;165m", "\x1b[38;5;166m", "\x1b[38;5;167m",
    "\x1b[38;5;168m", "\x1b[38;5;169m", "\x1b[38;5;170m", "\x1b[38;5;171m", "\x1b[38;5;172m", "\x1b[38;5;173m", "\x1b[38;5;174m", "\x1b[38;5;175m",
    "\x1b[38;5;176m", "\x1b[38;5;177m", "\x1b[38;5;178m", "\x1b[38;5;179m", "\x1b[38;5;180m", "\x1b[38;5;181m", "\x1b[38;5;182m", "\x1b[38;5;183m",
    "\x1b[38;5;184m", "\x1b[38;5;185m", "\x1b[38;5;186m", "\x1b[38;5;187m", "\x1b[38;5;188m", "\x1b[38;5;189m", "\x1b[38;5;190m", "\x1b[38;5;191m",
    "\x1b[38;5;192m", "\x1b[38;5;193m", "\x1b[38;5;194m", "\x1b[38;5;195m", "\x1b[38;5;196m", "\x1b[38;5;197m", "\x1b[38;5;198m", "\x1b[38;5;199m",
    "\x1b[38;5;200m", "\x1b[38;5;201m", "\x1b[38;5;202m", "\x1b[38;5;203m", "\x1b[38;5;204m", "\x1b[38;5;205m", "\x1b[38;5;206m", "\x1b[38;5;207m",
    "\x1b[38;5;208m", "\x1b[38;5;209m", "\x1b[38;5;210m", "\x1b[38;5;211m", "\x1b[38;5;212m", "\x1b[38;5;213m", "\x1b[38;5;214m", "\x1b[38;5;215m",
    "\x1b[38;5;216m", "\x1b[38;5;217m", "\x1b[38;5;218m", "\x1b[38;5;219m", "\x1b[38;5;220m", "\x1b[38;5;221m", "\x1b[38;5;222m", "\x1b[38;5;223m",
    "\x1b[38;5;224m", "\x1b[38;5;225m", "\x1b[38;5;226m", "\x1b[38;5;227m", "\x1b[38;5;228m", "\x1b[38;5;229m", "\x1b[38;5;230m", "\x1b[38;5;231m",
    "\x1b[38;5;232m", "\x1b[38;5;233m", "\x1b[38;5;234m", "\x1b[38;5;235m", "\x1b[38;5;236m", "\x1b[38;5;237m", "\x1b[38;5;238m", "\x1b[38;5;239m",
    "\x1b[38;5;240m", "\x1b[38;5;241m", "\x1b[38;5;242m", "\x1b[38;5;243m", "\x1b[38;5;244m", "\x1b[38;5;245m", "\x1b[38;5;246m", "\x1b[38;5;247m",
    "\x1b[38;5;248m", "\x1b[38;5;249m", "\x1b[38;5;250m", "\x1b[38;5;251m", "\x1b[38;5;252m", "\x1b[38;5;253m", "\x1b[38;5;254m", "\x1b[38;5;255m",
];

static BG_256_LUT: [&str; 256] = [
    "\x1b[48;5;0m", "\x1b[48;5;1m", "\x1b[48;5;2m", "\x1b[48;5;3m", "\x1b[48;5;4m", "\x1b[48;5;5m", "\x1b[48;5;6m", "\x1b[48;5;7m",
    "\x1b[48;5;8m", "\x1b[48;5;9m", "\x1b[48;5;10m", "\x1b[48;5;11m", "\x1b[48;5;12m", "\x1b[48;5;13m", "\x1b[48;5;14m", "\x1b[48;5;15m",
    "\x1b[48;5;16m", "\x1b[48;5;17m", "\x1b[48;5;18m", "\x1b[48;5;19m", "\x1b[48;5;20m", "\x1b[48;5;21m", "\x1b[48;5;22m", "\x1b[48;5;23m",
    "\x1b[48;5;24m", "\x1b[48;5;25m", "\x1b[48;5;26m", "\x1b[48;5;27m", "\x1b[48;5;28m", "\x1b[48;5;29m", "\x1b[48;5;30m", "\x1b[48;5;31m",
    "\x1b[48;5;32m", "\x1b[48;5;33m", "\x1b[48;5;34m", "\x1b[48;5;35m", "\x1b[48;5;36m", "\x1b[48;5;37m", "\x1b[48;5;38m", "\x1b[48;5;39m",
    "\x1b[48;5;40m", "\x1b[48;5;41m", "\x1b[48;5;42m", "\x1b[48;5;43m", "\x1b[48;5;44m", "\x1b[48;5;45m", "\x1b[48;5;46m", "\x1b[48;5;47m",
    "\x1b[48;5;48m", "\x1b[48;5;49m", "\x1b[48;5;50m", "\x1b[48;5;51m", "\x1b[48;5;52m", "\x1b[48;5;53m", "\x1b[48;5;54m", "\x1b[48;5;55m",
    "\x1b[48;5;56m", "\x1b[48;5;57m", "\x1b[48;5;58m", "\x1b[48;5;59m", "\x1b[48;5;60m", "\x1b[48;5;61m", "\x1b[48;5;62m", "\x1b[48;5;63m",
    "\x1b[48;5;64m", "\x1b[48;5;65m", "\x1b[48;5;66m", "\x1b[48;5;67m", "\x1b[48;5;68m", "\x1b[48;5;69m", "\x1b[48;5;70m", "\x1b[48;5;71m",
    "\x1b[48;5;72m", "\x1b[48;5;73m", "\x1b[48;5;74m", "\x1b[48;5;75m", "\x1b[48;5;76m", "\x1b[48;5;77m", "\x1b[48;5;78m", "\x1b[48;5;79m",
    "\x1b[48;5;80m", "\x1b[48;5;81m", "\x1b[48;5;82m", "\x1b[48;5;83m", "\x1b[48;5;84m", "\x1b[48;5;85m", "\x1b[48;5;86m", "\x1b[48;5;87m",
    "\x1b[48;5;88m", "\x1b[48;5;89m", "\x1b[48;5;90m", "\x1b[48;5;91m", "\x1b[48;5;92m", "\x1b[48;5;93m", "\x1b[48;5;94m", "\x1b[48;5;95m",
    "\x1b[48;5;96m", "\x1b[48;5;97m", "\x1b[48;5;98m", "\x1b[48;5;99m", "\x1b[48;5;100m", "\x1b[48;5;101m", "\x1b[48;5;102m", "\x1b[48;5;103m",
    "\x1b[48;5;104m", "\x1b[48;5;105m", "\x1b[48;5;106m", "\x1b[48;5;107m", "\x1b[48;5;108m", "\x1b[48;5;109m", "\x1b[48;5;110m", "\x1b[48;5;111m",
    "\x1b[48;5;112m", "\x1b[48;5;113m", "\x1b[48;5;114m", "\x1b[48;5;115m", "\x1b[48;5;116m", "\x1b[48;5;117m", "\x1b[48;5;118m", "\x1b[48;5;119m",
    "\x1b[48;5;120m", "\x1b[48;5;121m", "\x1b[48;5;122m", "\x1b[48;5;123m", "\x1b[48;5;124m", "\x1b[48;5;125m", "\x1b[48;5;126m", "\x1b[48;5;127m",
    "\x1b[48;5;128m", "\x1b[48;5;129m", "\x1b[48;5;130m", "\x1b[48;5;131m", "\x1b[48;5;132m", "\x1b[48;5;133m", "\x1b[48;5;134m", "\x1b[48;5;135m",
    "\x1b[48;5;136m", "\x1b[48;5;137m", "\x1b[48;5;138m", "\x1b[48;5;139m", "\x1b[48;5;140m", "\x1b[48;5;141m", "\x1b[48;5;142m", "\x1b[48;5;143m",
    "\x1b[48;5;144m", "\x1b[48;5;145m", "\x1b[48;5;146m", "\x1b[48;5;147m", "\x1b[48;5;148m", "\x1b[48;5;149m", "\x1b[48;5;150m", "\x1b[48;5;151m",
    "\x1b[48;5;152m", "\x1b[48;5;153m", "\x1b[48;5;154m", "\x1b[48;5;155m", "\x1b[48;5;156m", "\x1b[48;5;157m", "\x1b[48;5;158m", "\x1b[48;5;159m",
    "\x1b[48;5;160m", "\x1b[48;5;161m", "\x1b[48;5;162m", "\x1b[48;5;163m", "\x1b[48;5;164m", "\x1b[48;5;165m", "\x1b[48;5;166m", "\x1b[48;5;167m",
    "\x1b[48;5;168m", "\x1b[48;5;169m", "\x1b[48;5;170m", "\x1b[48;5;171m", "\x1b[48;5;172m", "\x1b[48;5;173m", "\x1b[48;5;174m", "\x1b[48;5;175m",
    "\x1b[48;5;176m", "\x1b[48;5;177m", "\x1b[48;5;178m", "\x1b[48;5;179m", "\x1b[48;5;180m", "\x1b[48;5;181m", "\x1b[48;5;182m", "\x1b[48;5;183m",
    "\x1b[48;5;184m", "\x1b[48;5;185m", "\x1b[48;5;186m", "\x1b[48;5;187m", "\x1b[48;5;188m", "\x1b[48;5;189m", "\x1b[48;5;190m", "\x1b[48;5;191m",
    "\x1b[48;5;192m", "\x1b[48;5;193m", "\x1b[48;5;194m", "\x1b[48;5;195m", "\x1b[48;5;196m", "\x1b[48;5;197m", "\x1b[48;5;198m", "\x1b[48;5;199m",
    "\x1b[48;5;200m", "\x1b[48;5;201m", "\x1b[48;5;202m", "\x1b[48;5;203m", "\x1b[48;5;204m", "\x1b[48;5;205m", "\x1b[48;5;206m", "\x1b[48;5;207m",
    "\x1b[48;5;208m", "\x1b[48;5;209m", "\x1b[48;5;210m", "\x1b[48;5;211m", "\x1b[48;5;212m", "\x1b[48;5;213m", "\x1b[48;5;214m", "\x1b[48;5;215m",
    "\x1b[48;5;216m", "\x1b[48;5;217m", "\x1b[48;5;218m", "\x1b[48;5;219m", "\x1b[48;5;220m", "\x1b[48;5;221m", "\x1b[48;5;222m", "\x1b[48;5;223m",
    "\x1b[48;5;224m", "\x1b[48;5;225m", "\x1b[48;5;226m", "\x1b[48;5;227m", "\x1b[48;5;228m", "\x1b[48;5;229m", "\x1b[48;5;230m", "\x1b[48;5;231m",
    "\x1b[48;5;232m", "\x1b[48;5;233m", "\x1b[48;5;234m", "\x1b[48;5;235m", "\x1b[48;5;236m", "\x1b[48;5;237m", "\x1b[48;5;238m", "\x1b[48;5;239m",
    "\x1b[48;5;240m", "\x1b[48;5;241m", "\x1b[48;5;242m", "\x1b[48;5;243m", "\x1b[48;5;244m", "\x1b[48;5;245m", "\x1b[48;5;246m", "\x1b[48;5;247m",
    "\x1b[48;5;248m", "\x1b[48;5;249m", "\x1b[48;5;250m", "\x1b[48;5;251m", "\x1b[48;5;252m", "\x1b[48;5;253m", "\x1b[48;5;254m", "\x1b[48;5;255m",
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
                out.write_str(FG_256_LUT[idx as usize])
            }
            AnsiColor::Rgb(r, g, b) => {
                write_rgb_fg(r, g, b, out)
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
                out.write_str(BG_256_LUT[idx as usize])
            }
            AnsiColor::Rgb(r, g, b) => {
                write_rgb_bg(r, g, b, out)
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
#[inline]
fn write_rgb_fg<W: std::fmt::Write>(r: u8, g: u8, b: u8, out: &mut W) -> std::fmt::Result {
    out.write_str("\x1b[38;2;")?;
    write_u8_ascii(r, out)?;
    out.write_char(';')?;
    write_u8_ascii(g, out)?;
    out.write_char(';')?;
    write_u8_ascii(b, out)?;
    out.write_char('m')
}

#[inline]
fn write_rgb_bg<W: std::fmt::Write>(r: u8, g: u8, b: u8, out: &mut W) -> std::fmt::Result {
    out.write_str("\x1b[48;2;")?;
    write_u8_ascii(r, out)?;
    out.write_char(';')?;
    write_u8_ascii(g, out)?;
    out.write_char(';')?;
    write_u8_ascii(b, out)?;
    out.write_char('m')
}

#[inline]
fn write_u8_ascii<W: std::fmt::Write>(mut n: u8, out: &mut W) -> std::fmt::Result {
    if n >= 100 {
        let hundred = n / 100;
        n %= 100;
        out.write_char((b'0' + hundred) as char)?;
        let ten = n / 10;
        let one = n % 10;
        out.write_char((b'0' + ten) as char)?;
        out.write_char((b'0' + one) as char)
    } else if n >= 10 {
        let ten = n / 10;
        let one = n % 10;
        out.write_char((b'0' + ten) as char)?;
        out.write_char((b'0' + one) as char)
    } else {
        out.write_char((b'0' + n) as char)
    }
}
