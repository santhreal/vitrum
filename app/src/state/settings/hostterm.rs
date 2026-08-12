//! The colours the operator's own terminal is configured with.
//!
//! The grid paints sixteen ANSI slots plus a background, a foreground, a
//! cursor and a selection. A built-in scheme supplies all twenty, and an
//! operator who has spent years reading code in their own colours does not
//! want any of them. This module reads those colours out of the terminal's
//! configuration and hands them to the grid.
//!
//! # What "the host terminal" resolves to
//!
//! A configuration file, parsed. Four formats cover the terminals that state
//! their colours declaratively:
//!
//! - a sectioned key/value file, which is the form alacritty and foot use;
//! - a flat `key value` file, which is the form kitty uses;
//! - an X resources file, `*color0` through `*color15` plus `*foreground`,
//!   `*background` and `*cursorColor`;
//! - a JSON scheme list, which is the form Windows Terminal uses.
//!
//! The environment picks which candidate is tried first when it names a
//! terminal, and every candidate that exists is tried in turn after that. The
//! first file that yields all twenty colours wins. A file that yields some of
//! them is refused rather than merged, because a palette half from one
//! terminal and half from another is a palette nobody has ever looked at.
//!
//! # What it cannot know
//!
//! - **A terminal that did not launch this process.** Started from a desktop
//!   file or a launcher, this process has no controlling terminal, so the scan
//!   reads whichever configuration file exists rather than the colours of any
//!   window on screen. When more than one is installed the answer is a guess,
//!   and the import records which file it read so the guess is visible.
//! - **Colours set at run time.** A shell that emits OSC 4, 10 or 11 changes
//!   its terminal's palette without touching any file. There is no query path
//!   back: asking would need a controlling terminal in raw mode, and this
//!   process has neither.
//! - **A terminal whose configuration is a program.** Colours computed in Lua,
//!   selected by a theme switcher, or chosen per profile and per window are
//!   not in a file this parser can evaluate.
//! - **Colours 16 through 255.** Those are the standard 6x6x6 cube and the
//!   24-step greyscale ramp. They are the same in every terminal and are not a
//!   preference.
//! - **Anything but colour.** Font, cursor shape, blink and scrollback are
//!   separate settings in this product and are not imported, because a
//!   terminal's font size is chosen for a terminal's window and not for this
//!   one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which kind of file an import came out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostSource {
    /// Nothing has been imported.
    #[default]
    None,
    /// A sectioned key/value file.
    Sectioned,
    /// A flat `key value` file.
    Flat,
    /// An X resources file.
    XResources,
    /// A JSON scheme list.
    Json,
}

impl HostSource {
    /// What the settings row calls it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            HostSource::None => "nothing imported",
            HostSource::Sectioned => "sectioned key/value file",
            HostSource::Flat => "flat key/value file",
            HostSource::XResources => "X resources",
            HostSource::Json => "JSON scheme",
        }
    }
}

/// Twenty colours read out of a terminal's configuration.
///
/// Owned strings, unlike [`crate::termpalette::Colours`], because these are
/// read at run time and cannot be `&'static str`. Every value is normalised to
/// `#rrggbb` on the way in, so a consumer never parses a second syntax.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostPalette {
    pub source: HostSource,
    /// The file the colours came out of, as it was given to the scan. Shown in
    /// the settings row so an operator with several terminals installed can
    /// see which one answered.
    pub origin: String,
    pub background: String,
    pub foreground: String,
    /// Empty falls back to the foreground, which is what a terminal that
    /// declares no cursor colour does.
    pub cursor: String,
    /// Empty falls back to a translucent wash of the foreground.
    pub selection: String,
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the eight
    /// bright variants. SGR order, indexed by colour number.
    pub ansi: Vec<String>,
}

impl HostPalette {
    /// Whether this import has every colour the grid needs.
    ///
    /// The sixteen and the two that have no fallback. Cursor and selection are
    /// derived when absent, so they do not gate an otherwise usable import.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.source != HostSource::None
            && !self.background.is_empty()
            && !self.foreground.is_empty()
            && self.ansi.len() == 16
            && self.ansi.iter().all(|slot| !slot.is_empty())
    }

    /// Drop anything that is not a colour this product can paint.
    ///
    /// A hand-edited profile is the path that reaches here with a 400-entry
    /// `ansi` array or with `background: "; }"`. An import that fails this is
    /// left incomplete, which turns the switch that reads it into a no-op
    /// rather than into a broken grid.
    pub fn clamp(&mut self) {
        self.ansi.truncate(16);
        for slot in &mut self.ansi {
            if parse_colour(slot).is_none() {
                slot.clear();
            }
        }
        for field in [
            &mut self.background,
            &mut self.foreground,
            &mut self.cursor,
            &mut self.selection,
        ] {
            if !field.is_empty() && parse_colour(field).is_none() {
                field.clear();
            }
        }
        if self.origin.len() > ORIGIN_MAX {
            self.origin.truncate(ORIGIN_MAX);
        }
    }

    /// The cursor colour, or the foreground when the file declared none.
    #[must_use]
    pub fn cursor_or_foreground(&self) -> &str {
        if self.cursor.is_empty() {
            &self.foreground
        } else {
            &self.cursor
        }
    }

    /// The selection colour, or the foreground when the file declared none.
    #[must_use]
    pub fn selection_or_foreground(&self) -> &str {
        if self.selection.is_empty() {
            &self.foreground
        } else {
            &self.selection
        }
    }
}

/// Longest origin string kept. A path is a label here, not an address.
const ORIGIN_MAX: usize = 256;

/// Why an import found nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// No candidate file exists on this machine.
    NoCandidate,
    /// Files exist and none of them declares all sixteen ANSI slots plus a
    /// foreground and a background.
    Incomplete {
        /// The files that were read, and what each was missing.
        tried: Vec<(String, String)>,
    },
    /// The named file could not be read.
    Unreadable {
        path: String,
        detail: String,
    },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::NoCandidate => f.write_str(
                "no terminal configuration was found. Pick a built-in palette, or point \
                 the import at a file with Import from a file.",
            ),
            ImportError::Incomplete { tried } => {
                f.write_str("no terminal configuration declares a whole palette.")?;
                for (path, missing) in tried {
                    write!(f, " {path}: {missing}.")?;
                }
                f.write_str(
                    " Declare the missing colours in that file, or pick a built-in palette.",
                )
            }
            ImportError::Unreadable { path, detail } => write!(
                f,
                "{path} could not be read: {detail}. Check the file's permissions, or pick \
                 a built-in palette."
            ),
        }
    }
}

/// One place a palette might be declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub format: HostSource,
}

/// Where a palette might be declared on this machine, best guess first.
///
/// `env` is the process environment as a map so the order can be asserted
/// without setting variables in a test binary that runs its tests in threads.
///
/// The variable a terminal exports about itself decides the order and nothing
/// else. Every candidate is still tried, because a terminal that exports
/// nothing is common and an operator who runs one terminal and configures
/// another is not.
#[must_use]
pub fn candidates(env: &BTreeMap<String, String>) -> Vec<Candidate> {
    let home = env.get("HOME").map(PathBuf::from);
    let config = env
        .get("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".config")));

    let mut out: Vec<Candidate> = Vec::new();
    if let Some(config) = &config {
        out.push(Candidate {
            path: config.join("alacritty").join("alacritty.toml"),
            format: HostSource::Sectioned,
        });
        out.push(Candidate {
            path: config.join("kitty").join("kitty.conf"),
            format: HostSource::Flat,
        });
        out.push(Candidate {
            path: config.join("foot").join("foot.ini"),
            format: HostSource::Sectioned,
        });
    }
    if let Some(home) = &home {
        out.push(Candidate {
            path: home.join(".Xresources"),
            format: HostSource::XResources,
        });
        out.push(Candidate {
            path: home.join(".Xdefaults"),
            format: HostSource::XResources,
        });
        out.push(Candidate {
            path: home
                .join("AppData")
                .join("Local")
                .join("Packages")
                .join("Microsoft.WindowsTerminal_8wekyb3d8bbwe")
                .join("LocalState")
                .join("settings.json"),
            format: HostSource::Json,
        });
    }

    // The terminal that exported a variable about itself goes first. This is
    // the only signal available: the process has no controlling terminal to
    // ask, so a variable the emulator sets in its children is as close as the
    // scan gets to "the terminal you are looking at".
    let preferred: Option<&str> = if env.contains_key("KITTY_WINDOW_ID") {
        Some("kitty.conf")
    } else if env.contains_key("ALACRITTY_WINDOW_ID")
        || env.contains_key("ALACRITTY_SOCKET")
        || env.get("TERM").is_some_and(|t| t.contains("alacritty"))
    {
        Some("alacritty.toml")
    } else if env.get("TERM").is_some_and(|t| t.contains("foot")) {
        Some("foot.ini")
    } else if env.contains_key("WT_SESSION") {
        Some("settings.json")
    } else {
        None
    };
    if let Some(name) = preferred
        && let Some(at) = out
            .iter()
            .position(|c| c.path.file_name().is_some_and(|f| f == name))
    {
        let first = out.remove(at);
        out.insert(0, first);
    }
    out
}

/// Read the first candidate that declares a whole palette.
///
/// `read` is the file reader, so the scan is exercised end to end in a test
/// without a fixture tree on disk. Production passes
/// [`std::fs::read_to_string`].
pub fn import(
    env: &BTreeMap<String, String>,
    mut read: impl FnMut(&Path) -> std::io::Result<String>,
) -> Result<HostPalette, ImportError> {
    let mut tried: Vec<(String, String)> = Vec::new();
    let mut found_any = false;
    for candidate in candidates(env) {
        let text = match read(&candidate.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(ImportError::Unreadable {
                    path: candidate.path.display().to_string(),
                    detail: e.to_string(),
                });
            }
        };
        found_any = true;
        let origin = candidate.path.display().to_string();
        let mut palette = parse(&text, candidate.format);
        palette.origin = origin.clone();
        palette.clamp();
        if palette.is_complete() {
            return Ok(palette);
        }
        tried.push((origin, missing_of(&palette)));
    }
    if found_any {
        Err(ImportError::Incomplete { tried })
    } else {
        Err(ImportError::NoCandidate)
    }
}

/// [`import`] against this machine: the real environment, the real files.
///
/// The one impure entry point, so the scan has a single owner. Settings calls
/// it when the operator presses Import, and the first run calls it before
/// anything is on screen.
pub fn import_from_machine() -> Result<HostPalette, ImportError> {
    let env = std::env::vars().collect();
    // Annotated rather than passed by name. `read_to_string` is generic over
    // `AsRef<Path>`, so handing it over directly makes the compiler pick one
    // concrete lifetime, and `import` needs a reader good for any.
    import(&env, |path: &Path| std::fs::read_to_string(path))
}

/// Read one named file, whatever format it turns out to be.
///
/// The operator's escape hatch for a terminal the scan does not know: the
/// format is decided by the file's own shape rather than by its name, because
/// a palette exported to `colors.conf` is still a flat key/value file.
pub fn import_file(
    path: &Path,
    read: impl FnOnce(&Path) -> std::io::Result<String>,
) -> Result<HostPalette, ImportError> {
    let text = read(path).map_err(|e| ImportError::Unreadable {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;
    let origin = path.display().to_string();
    let mut best: Option<HostPalette> = None;
    for format in [
        HostSource::Json,
        HostSource::Sectioned,
        HostSource::Flat,
        HostSource::XResources,
    ] {
        let mut palette = parse(&text, format);
        palette.origin = origin.clone();
        palette.clamp();
        if palette.is_complete() {
            return Ok(palette);
        }
        if best.as_ref().is_none_or(|b| filled(&palette) > filled(b)) {
            best = Some(palette);
        }
    }
    Err(ImportError::Incomplete {
        tried: vec![(
            origin,
            best.as_ref().map_or_else(
                || "no colours".to_string(),
                |p| missing_of(p),
            ),
        )],
    })
}

/// How many of the twenty slots an import filled.
fn filled(p: &HostPalette) -> usize {
    p.ansi.iter().filter(|s| !s.is_empty()).count()
        + usize::from(!p.background.is_empty())
        + usize::from(!p.foreground.is_empty())
}

/// What an incomplete import is short of, as a sentence fragment.
fn missing_of(p: &HostPalette) -> String {
    let mut parts: Vec<String> = Vec::new();
    if p.background.is_empty() {
        parts.push("no background".to_string());
    }
    if p.foreground.is_empty() {
        parts.push("no foreground".to_string());
    }
    let slots: Vec<usize> = (0..16)
        .filter(|n| p.ansi.get(*n).is_none_or(|s| s.is_empty()))
        .collect();
    match slots.len() {
        0 => {}
        16 => parts.push("no ANSI colours".to_string()),
        _ => parts.push(format!(
            "no colour {}",
            slots
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
    if parts.is_empty() {
        "complete".to_string()
    } else {
        parts.join(", ")
    }
}

/// Parse one file in one format. Never fails: an unparseable file yields an
/// empty palette, which [`HostPalette::is_complete`] refuses by name.
#[must_use]
pub fn parse(text: &str, format: HostSource) -> HostPalette {
    let mut palette = HostPalette {
        source: format,
        ..HostPalette::default()
    };
    palette.ansi = vec![String::new(); 16];
    match format {
        HostSource::None => {}
        HostSource::Sectioned => parse_sectioned(text, &mut palette),
        HostSource::Flat => parse_flat(text, &mut palette),
        HostSource::XResources => parse_x_resources(text, &mut palette),
        HostSource::Json => parse_json(text, &mut palette),
    }
    if filled(&palette) == 0 {
        palette.source = HostSource::None;
    }
    palette
}

/// The eight base colour names, in SGR order.
const BASE_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// One value out of a sectioned file, where the `#` is optional.
///
/// foot writes `background=1e1e1e`, because in its file `#` starts a comment
/// and cannot also start a colour. A reader that insisted on the prefix would
/// answer "no palette" on a file that plainly has one.
///
/// Separate from [`parse_colour`], which stays strict: that function also
/// validates a palette read back from this product's own profile, where a
/// value is always written `#rrggbb` and a bare run of digits is a
/// hand-edited field that must be refused rather than guessed at.
fn sectioned_colour(value: &str) -> Option<String> {
    parse_colour(value).or_else(|| {
        let bare = value.trim().trim_matches(['"', '\'']).trim();
        let widthed = matches!(bare.len(), 3 | 6 | 8 | 12)
            && bare.chars().all(|c| c.is_ascii_hexdigit());
        widthed.then(|| parse_colour(&format!("#{bare}")))?
    })
}

/// A sectioned key/value file: `[section.subsection]` then `key = value`.
///
/// Two dialects share this shape and neither needs its own parser. One names
/// its sections `colors.primary`, `colors.normal`, `colors.bright` and
/// `colors.cursor` and its keys by colour name; the other uses one `colors`
/// section and names its keys `regular0` through `bright7`. Both are handled
/// by looking at the section and the key together.
fn parse_sectioned(text: &str, out: &mut HostPalette) {
    let mut section = String::new();
    for raw in text.lines() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let Some(colour) = sectioned_colour(value.trim()) else {
            continue;
        };
        match (section.as_str(), key.as_str()) {
            ("colors.primary" | "colors" | "color", "background") => out.background = colour,
            ("colors.primary" | "colors" | "color", "foreground") => out.foreground = colour,
            ("colors.cursor", "cursor") | ("colors" | "color", "cursor") => out.cursor = colour,
            ("colors.selection", "background") | ("colors" | "color", "selection-background") => {
                out.selection = colour;
            }
            ("colors.normal", name) => {
                if let Some(at) = BASE_NAMES.iter().position(|n| *n == name) {
                    out.ansi[at] = colour;
                }
            }
            ("colors.bright", name) => {
                if let Some(at) = BASE_NAMES.iter().position(|n| *n == name) {
                    out.ansi[8 + at] = colour;
                }
            }
            ("colors" | "color", key) => {
                if let Some(at) = indexed_slot(key) {
                    out.ansi[at] = colour;
                }
            }
            _ => {}
        }
    }
}

/// `regular3`, `bright3`, `color3` or a bare `3` as a slot number.
fn indexed_slot(key: &str) -> Option<usize> {
    for (prefix, base) in [("regular", 0usize), ("bright", 8), ("color", 0), ("", 0)] {
        if let Some(rest) = key.strip_prefix(prefix)
            && !rest.is_empty()
            && let Ok(n) = rest.parse::<usize>()
        {
            let limit = if prefix == "bright" { 8 } else { 16 };
            if n < limit {
                return Some(base + n);
            }
            return None;
        }
    }
    None
}

/// A flat `key value` file, one declaration per line, no sections.
fn parse_flat(text: &str, out: &mut HostPalette) {
    for raw in text.lines() {
        let line = strip_comment(raw);
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else { continue };
        let Some(value) = parts.next() else { continue };
        let Some(colour) = parse_colour(value) else {
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "background" => out.background = colour,
            "foreground" => out.foreground = colour,
            "cursor" => out.cursor = colour,
            "selection_background" | "selection-background" => out.selection = colour,
            key => {
                if let Some(at) = indexed_slot(key) {
                    out.ansi[at] = colour;
                }
            }
        }
    }
}

/// An X resources file: `<anything>color0: #rrggbb`, one per line.
///
/// The class prefix is ignored. A file that declares the same resource for
/// several classes declares the same colour for all of them in practice, and a
/// reader that insisted on one class would answer "no palette" on a file that
/// plainly has one.
fn parse_x_resources(text: &str, out: &mut HostPalette) {
    for raw in text.lines() {
        let line = strip_comment_x(raw);
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let Some(colour) = parse_colour(value.trim()) else {
            continue;
        };
        let name = key
            .rsplit(['.', '*'])
            .next()
            .unwrap_or(key)
            .trim()
            .to_ascii_lowercase();
        match name.as_str() {
            "background" => out.background = colour,
            "foreground" => out.foreground = colour,
            "cursorcolor" => out.cursor = colour,
            "highlightcolor" | "selectionbackground" => out.selection = colour,
            name => {
                if let Some(rest) = name.strip_prefix("color")
                    && let Ok(n) = rest.parse::<usize>()
                    && n < 16
                {
                    out.ansi[n] = colour;
                }
            }
        }
    }
}

/// A JSON scheme list: `{"schemes":[{"name":..,"background":..,"black":..}]}`.
///
/// The scheme named by `defaultProfile`'s profile is not resolved. A settings
/// file with one scheme is the common case and is read; a file with several
/// takes the first, and the origin string says which file it came from so an
/// operator who has more than one can see what happened.
fn parse_json(text: &str, out: &mut HostPalette) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let scheme = value
        .get("schemes")
        .and_then(|s| s.as_array())
        .and_then(|a| a.first())
        .unwrap_or(&value);
    let get = |key: &str| {
        scheme
            .get(key)
            .and_then(serde_json::Value::as_str)
            .and_then(parse_colour)
    };
    if let Some(c) = get("background") {
        out.background = c;
    }
    if let Some(c) = get("foreground") {
        out.foreground = c;
    }
    if let Some(c) = get("cursorColor") {
        out.cursor = c;
    }
    if let Some(c) = get("selectionBackground") {
        out.selection = c;
    }
    for (at, name) in BASE_NAMES.iter().enumerate() {
        let mut bright = String::from("bright");
        bright.push_str(name);
        // The key is `brightBlack`, so the first letter of the base name is
        // uppercased rather than the whole word.
        let bright = bright
            .char_indices()
            .map(|(i, c)| if i == 6 { c.to_ascii_uppercase() } else { c })
            .collect::<String>();
        if let Some(c) = get(name) {
            out.ansi[at] = c;
        }
        if let Some(c) = get(&bright) {
            out.ansi[8 + at] = c;
        }
    }
}

/// Everything before a `#` comment, trimmed.
///
/// `#` is both the comment character in these formats and the first
/// character of every colour in them, so position alone cannot tell the two
/// apart: `color0 #000000 # the darkest one` has one of each, and both are
/// preceded by whitespace. What separates them is what follows. A `#` that
/// introduces a run of hex digits of a width a colour is written in, ending
/// at whitespace or a quote, is a colour; anything else opens a comment.
///
/// A `#` inside a quoted value is a colour too, and the quote suspends the
/// rule entirely.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quoted = false;
    for (at, b) in bytes.iter().enumerate() {
        match b {
            b'"' | b'\'' => quoted = !quoted,
            b'#' if !quoted && !opens_colour(&line[at..]) => return line[..at].trim(),
            _ => {}
        }
    }
    line.trim()
}

/// Whether `rest`, which starts at a `#`, is the start of a hex colour.
///
/// The widths are the ones terminals write: `#rgb`, `#rgba`, `#rrggbb`,
/// `#rrggbbaa` and `#rrrrggggbbbb`. A run of some other length is a comment
/// that happens to begin with hex letters, which `# beef stew` is.
fn opens_colour(rest: &str) -> bool {
    let digits = rest[1..]
        .bytes()
        .take_while(u8::is_ascii_hexdigit)
        .count();
    if !matches!(digits, 3 | 4 | 6 | 8 | 12) {
        return false;
    }
    match rest.as_bytes().get(1 + digits) {
        None => true,
        Some(b) => b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b',' | b';'),
    }
}

/// Everything before a `!` comment, trimmed. X resources comment with `!`, and
/// `#` is the start of every colour in the file.
fn strip_comment_x(line: &str) -> &str {
    match line.find('!') {
        Some(at) => line[..at].trim(),
        None => line.trim(),
    }
}

/// One colour in any of the syntaxes these files use, as `#rrggbb`.
///
/// Accepted: `#rgb`, `#rrggbb`, `#rrrrggggbbbb`, `0xrrggbb`, and
/// `rgb:rr/gg/bb` with one to four hex digits per channel. Quotes and
/// surrounding whitespace are stripped first, because two of the four formats
/// quote their values and one does not.
///
/// Alpha is dropped rather than refused. A terminal that declares an eight
/// digit colour is declaring a translucent background, and this product owns
/// its own opacity setting; honouring both would multiply them.
#[must_use]
pub fn parse_colour(raw: &str) -> Option<String> {
    let text = raw.trim().trim_matches(['"', '\'']).trim();
    if let Some(rest) = text.strip_prefix("rgb:") {
        let mut channels = [0u8; 3];
        let mut seen = 0;
        for (at, part) in rest.split('/').enumerate() {
            if at >= 3 || part.is_empty() || part.len() > 4 {
                return None;
            }
            channels[at] = scale_channel(part)?;
            seen += 1;
        }
        if seen != 3 {
            return None;
        }
        return Some(format!(
            "#{:02x}{:02x}{:02x}",
            channels[0], channels[1], channels[2]
        ));
    }
    let hex = text
        .strip_prefix('#')
        .or_else(|| text.strip_prefix("0x"))
        .or_else(|| text.strip_prefix("0X"))?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let per = match hex.len() {
        3 => 1,
        6 => 2,
        8 => 2,
        12 => 4,
        16 => 4,
        _ => return None,
    };
    let mut channels = [0u8; 3];
    for (at, slot) in channels.iter_mut().enumerate() {
        *slot = scale_channel(&hex[at * per..(at + 1) * per])?;
    }
    Some(format!(
        "#{:02x}{:02x}{:02x}",
        channels[0], channels[1], channels[2]
    ))
}

/// One hex channel of any width, scaled to eight bits.
///
/// `f` is 255 and not 15: a one-digit channel names a fraction of full scale,
/// so it repeats rather than zero-extends. Zero-extending turns `#fff` into a
/// dark grey, which is the classic version of this bug.
fn scale_channel(hex: &str) -> Option<u8> {
    let value = u32::from_str_radix(hex, 16).ok()?;
    let max = match hex.len() {
        1 => 0xf_u32,
        2 => 0xff,
        3 => 0xfff,
        4 => 0xffff,
        _ => return None,
    };
    Some(u8::try_from(value * 255 / max).unwrap_or(255))
}

/// One colour as the four bytes the renderer uploads.
///
/// `None` for anything [`parse_colour`] refuses, which is the same rule the
/// import applied on the way in, so a stored palette that passed
/// [`HostPalette::clamp`] never reaches the renderer as `None`.
#[must_use]
pub fn to_rgba(colour: &str, alpha: u8) -> Option<[u8; 4]> {
    let hex = parse_colour(colour)?;
    let n = u32::from_str_radix(&hex[1..], 16).ok()?;
    Some([
        u8::try_from((n >> 16) & 0xff).ok()?,
        u8::try_from((n >> 8) & 0xff).ok()?,
        u8::try_from(n & 0xff).ok()?,
        alpha,
    ])
}

#[cfg(test)]
mod tests;
