//! Monospace font discovery, cell metrics, and glyph rasterisation.
//!
//! The renderer needs three things from a font: a fixed cell size, a bitmap
//! for every character it draws, and a fallback chain so a CJK codepoint that
//! the primary face lacks still shows the right glyph instead of a blank.
//!
//! Faces are discovered through `fontdb`, which reads the platform font
//! directories on Linux, macOS, and Windows, so nothing here is OS specific.
//! Rasterisation is `fontdue`, which is pure Rust and needs no system text
//! shaper.
//!
//! Bold and italic prefer a real face from the same family. When the family
//! ships only a regular face, the bitmap is emboldened or sheared instead of
//! silently rendering upright, because a terminal that ignores SGR 1 and SGR 3
//! is a terminal that lies about its output.
//!
//! The fallback chain is built by [`fallback_chain`], which reads only the
//! font database: no device, no window, and no parsed face. Faces along it are
//! parsed lazily, the first time a character reaches them, so a stack that
//! never draws a CJK codepoint never pays for the CJK font.

use std::collections::HashMap;

use crate::cell::{Attrs, CharWidth, char_width};

/// Which face variant a cell wants.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
#[repr(u8)]
pub enum FontStyle {
    /// Upright, normal weight.
    #[default]
    Regular = 0,
    /// Upright, bold weight.
    Bold = 1,
    /// Slanted, normal weight.
    Italic = 2,
    /// Slanted, bold weight.
    BoldItalic = 3,
}

impl FontStyle {
    /// Every variant, in slot order.
    pub const ALL: [Self; 4] = [Self::Regular, Self::Bold, Self::Italic, Self::BoldItalic];

    /// The variant selected by a cell's attribute bits.
    #[must_use]
    pub const fn from_attrs(attrs: Attrs) -> Self {
        match (attrs.contains(Attrs::BOLD), attrs.contains(Attrs::ITALIC)) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }

    /// Slot index, `0..4`.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// True for [`FontStyle::Bold`] and [`FontStyle::BoldItalic`].
    #[must_use]
    pub const fn is_bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }

    /// True for [`FontStyle::Italic`] and [`FontStyle::BoldItalic`].
    #[must_use]
    pub const fn is_italic(self) -> bool {
        matches!(self, Self::Italic | Self::BoldItalic)
    }
}

/// Why a font stack could not be built.
#[derive(Clone, PartialEq, Debug)]
pub enum FontError {
    /// No monospace face was found in the system font database. On a headless
    /// box this usually means no fonts are installed at all.
    NoMonospaceFont,
    /// A face the database advertised could not be read back.
    FaceDataUnavailable {
        /// Family name of the face that vanished.
        family: String,
    },
    /// A face was found but is not a font this crate can rasterise.
    Parse {
        /// Family name of the unusable face.
        family: String,
        /// What the parser reported.
        reason: String,
    },
    /// The requested pixel size is not finite, not positive, or absurdly large.
    InvalidSize(f32),
}

impl core::fmt::Display for FontError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoMonospaceFont => f.write_str(
                "no monospace font found in the system font database; install one (for example \
                 DejaVu Sans Mono or Liberation Mono) or pass an explicit family in FontConfig",
            ),
            Self::FaceDataUnavailable { family } => write!(
                f,
                "font database listed family '{family}' but its face data could not be read"
            ),
            Self::Parse { family, reason } => {
                write!(f, "font family '{family}' could not be parsed: {reason}")
            }
            Self::InvalidSize(px) => write!(
                f,
                "font size {px} px is invalid: it must be finite and in 1.0..={MAX_SIZE_PX}"
            ),
        }
    }
}

impl core::error::Error for FontError {}

/// Largest font size the stack accepts. Above this a single glyph would not fit
/// in a reasonably sized glyph atlas.
pub const MAX_SIZE_PX: f32 = 256.0;

/// Slant applied when a family has no real italic face. 0.2 is a shade over 11
/// degrees, the conventional synthetic-oblique angle.
const SYNTHETIC_SLANT: f32 = 0.2;

/// Default value of [`FontConfig::families`]: terminal monospace families in
/// preference order, covering Linux, macOS, and Windows.
///
/// This is a default, not a policy. Replace [`FontConfig::families`] with the
/// user's configured font and the list is never consulted.
///
/// It exists because the alternative is worse. `fontdb`'s generic monospace
/// family resolves through a fontconfig alias on Linux and to `Courier New`
/// elsewhere, and when that name is not installed the only thing left is "some
/// face flagged monospaced", chosen in font-directory walk order. That order
/// differs between machines and can differ between runs on one machine, so the
/// same application would render at different cell metrics for no visible
/// reason.
pub const DEFAULT_FAMILIES: &[&str] = &[
    // Linux
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "Ubuntu Mono",
    // Cross-platform developer fonts
    "JetBrains Mono",
    "Fira Mono",
    "Source Code Pro",
    // macOS
    "SF Mono",
    "Menlo",
    "Monaco",
    // Windows
    "Cascadia Mono",
    "Consolas",
    "Courier New",
];

/// How to build a [`FontStack`].
#[derive(Clone, Debug)]
pub struct FontConfig {
    /// Preferred family names, most preferred first. The first one present in
    /// the system database wins.
    ///
    /// Defaults to [`DEFAULT_FAMILIES`]. When none of the listed names is
    /// installed, the database's generic monospace family is tried, and failing
    /// that the monospaced face whose family name sorts first. That last step
    /// is a sort rather than a scan so the choice is the same on every machine
    /// with the same fonts.
    pub families: Vec<String>,
    /// Cell font size in pixels.
    pub size_px: f32,
    /// How many non-primary faces may be parsed while hunting for a fallback
    /// glyph. Each parsed face costs memory, so the chain is bounded.
    pub max_fallback_faces: usize,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            families: DEFAULT_FAMILIES.iter().map(|s| (*s).to_owned()).collect(),
            size_px: 16.0,
            max_fallback_faces: 24,
        }
    }
}

/// Fixed geometry every cell in the grid uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellMetrics {
    /// Cell width in pixels. One column is this wide; a wide character is two.
    pub width: u32,
    /// Cell height in pixels.
    pub height: u32,
    /// Distance from the top of the cell down to the text baseline.
    pub baseline: i32,
    /// Distance from the top of the cell down to the top of the underline rule.
    pub underline_y: u32,
    /// Underline rule thickness in pixels, always at least 1.
    pub underline_thickness: u32,
}

/// A rasterised glyph bitmap positioned relative to its cell's top-left corner.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RasterGlyph {
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// Pixels from the cell's left edge to the bitmap's left edge. Negative
    /// when the glyph overhangs to the left.
    pub left: i32,
    /// Pixels from the cell's top edge to the bitmap's top edge.
    pub top: i32,
    /// Row-major 8-bit coverage, `width * height` bytes.
    pub coverage: Vec<u8>,
}

impl RasterGlyph {
    /// A glyph with no ink, used for spaces and wide-pair tails.
    #[must_use]
    pub const fn blank() -> Self {
        Self {
            width: 0,
            height: 0,
            left: 0,
            top: 0,
            coverage: Vec::new(),
        }
    }

    /// True when this glyph has no pixels to upload.
    #[must_use]
    pub const fn is_blank(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Coverage at `(x, y)` within the bitmap, or 0 outside it.
    #[must_use]
    pub fn coverage_at(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.coverage[(y * self.width + x) as usize]
    }
}

/// Which face a style slot draws with and what has to be faked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct StyleSlot {
    font: usize,
    synth_bold: bool,
    synth_italic: bool,
}

/// One face in the fallback chain, in the order it will be tried.
///
/// The chain is data, not behaviour: [`fallback_chain`] builds it from a
/// `fontdb::Database` alone, so what a given font set resolves to can be
/// asserted without a GPU, a window, or a rasterised pixel.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FallbackEntry {
    /// Database id of the face, so a caller can read its bytes or query it.
    pub id: fontdb::ID,
    /// The face's first family name. This is what a log line names.
    pub family: String,
    /// Whether the face declares itself monospaced. Monospaced faces come
    /// first in the chain, because a proportional face borrowed for one
    /// missing character still has to sit inside a fixed cell.
    pub monospaced: bool,
}

/// Where the glyph for a character comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coverage {
    /// One of the primary family's own faces has it.
    Primary,
    /// The face at this index of [`FontStack::fallback_chain`] has it.
    Fallback(usize),
    /// Nothing in the stack has it, so it draws as a hollow box.
    Missing,
}

/// Lazily parsed fallback face, parallel to [`FontStack::fallback_chain`].
enum FallbackFace {
    Unloaded,
    Failed,
    Loaded(Box<fontdue::Font>),
}

/// A primary monospace face plus its bold/italic variants and a bounded
/// fallback chain for characters none of them cover.
pub struct FontStack {
    fonts: Vec<fontdue::Font>,
    styles: [StyleSlot; 4],
    metrics: CellMetrics,
    size_px: f32,
    family: String,
    db: fontdb::Database,
    /// The fallback chain in try order. `chain[i]` describes the face that
    /// `fallback_faces[i]` parses to, so the order that can be inspected is
    /// the order rasterisation really walks.
    chain: Vec<FallbackEntry>,
    fallback_faces: Vec<FallbackFace>,
    /// `char` to fallback index, or `None` when no face in the chain has it.
    resolved: HashMap<char, Option<usize>>,
}

impl core::fmt::Debug for FontStack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FontStack")
            .field("family", &self.family)
            .field("size_px", &self.size_px)
            .field("metrics", &self.metrics)
            .field("faces", &self.fonts.len())
            .field("fallback_chain", &self.chain.len())
            .finish()
    }
}

impl FontStack {
    /// Discover a monospace family in the system font database and build the
    /// four style slots from it.
    ///
    /// # Errors
    ///
    /// [`FontError::InvalidSize`] for a size outside `1.0..=`[`MAX_SIZE_PX`],
    /// [`FontError::NoMonospaceFont`] when the database has no usable
    /// monospace face, and [`FontError::Parse`] when the chosen face is not
    /// readable as TrueType or OpenType.
    pub fn system(config: &FontConfig) -> Result<Self, FontError> {
        check_size(config.size_px)?;
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Self::from_database(db, config)
    }

    /// Build a stack from an already-populated font database. Useful when the
    /// caller wants to add application-bundled faces before discovery.
    ///
    /// # Errors
    ///
    /// Same conditions as [`FontStack::system`].
    pub fn from_database(db: fontdb::Database, config: &FontConfig) -> Result<Self, FontError> {
        check_size(config.size_px)?;
        let regular_id = pick_regular(&db, &config.families).ok_or(FontError::NoMonospaceFont)?;
        let family = primary_family_name(&db, regular_id);

        let (regular, underline) = load_face(&db, regular_id, &family, config.size_px)?;
        let metrics = cell_metrics(&regular, underline, config.size_px);

        let mut fonts = vec![regular];
        let mut styles = [StyleSlot {
            font: 0,
            synth_bold: false,
            synth_italic: false,
        }; 4];

        for style in [FontStyle::Bold, FontStyle::Italic, FontStyle::BoldItalic] {
            let wanted = fontdb::Query {
                families: &[fontdb::Family::Name(&family)],
                weight: if style.is_bold() {
                    fontdb::Weight::BOLD
                } else {
                    fontdb::Weight::NORMAL
                },
                stretch: fontdb::Stretch::Normal,
                style: if style.is_italic() {
                    fontdb::Style::Italic
                } else {
                    fontdb::Style::Normal
                },
            };
            let matched = db.query(&wanted).filter(|id| *id != regular_id);
            let slot = match matched {
                // A real face exists. Parse failures here are not fatal: fall
                // back to synthesising from the regular face.
                Some(id) => match load_face(&db, id, &family, config.size_px) {
                    Ok((font, _)) => {
                        fonts.push(font);
                        StyleSlot {
                            font: fonts.len() - 1,
                            synth_bold: false,
                            synth_italic: false,
                        }
                    }
                    Err(_) => StyleSlot {
                        font: 0,
                        synth_bold: style.is_bold(),
                        synth_italic: style.is_italic(),
                    },
                },
                None => StyleSlot {
                    font: 0,
                    synth_bold: style.is_bold(),
                    synth_italic: style.is_italic(),
                },
            };
            styles[style.index()] = slot;
        }

        let chain = fallback_chain(&db, regular_id, &family, config.max_fallback_faces);
        let fallback_faces: Vec<FallbackFace> =
            chain.iter().map(|_| FallbackFace::Unloaded).collect();

        Ok(Self {
            fonts,
            styles,
            metrics,
            size_px: config.size_px,
            family,
            db,
            chain,
            fallback_faces,
            resolved: HashMap::new(),
        })
    }

    /// Build a stack from one caller-supplied font file, using it for all four
    /// style slots with synthetic bold and italic. No system fonts are read and
    /// there is no fallback chain, so this is fully deterministic.
    ///
    /// # Errors
    ///
    /// [`FontError::InvalidSize`] for a bad size and [`FontError::Parse`] when
    /// `data` is not a font this crate can rasterise.
    pub fn from_face_bytes(data: &[u8], index: u32, size_px: f32) -> Result<Self, FontError> {
        check_size(size_px)?;
        let family = String::from("<embedded>");
        let font = parse_font(data, index, &family)?;
        let underline = ttf_parser::Face::parse(data, index)
            .ok()
            .and_then(|f| underline_px(&f, size_px));
        let metrics = cell_metrics(&font, underline, size_px);
        let styles = [
            StyleSlot {
                font: 0,
                synth_bold: false,
                synth_italic: false,
            },
            StyleSlot {
                font: 0,
                synth_bold: true,
                synth_italic: false,
            },
            StyleSlot {
                font: 0,
                synth_bold: false,
                synth_italic: true,
            },
            StyleSlot {
                font: 0,
                synth_bold: true,
                synth_italic: true,
            },
        ];
        Ok(Self {
            fonts: vec![font],
            styles,
            metrics,
            size_px,
            family,
            db: fontdb::Database::new(),
            chain: Vec::new(),
            fallback_faces: Vec::new(),
            resolved: HashMap::new(),
        })
    }

    /// Cell geometry derived from the primary face.
    #[must_use]
    pub const fn metrics(&self) -> CellMetrics {
        self.metrics
    }

    /// Pixel size the stack was built at.
    #[must_use]
    pub const fn size_px(&self) -> f32 {
        self.size_px
    }

    /// Family name of the primary face.
    #[must_use]
    pub fn family(&self) -> &str {
        &self.family
    }

    /// True when `style` draws with a real face rather than a synthesised one.
    #[must_use]
    pub fn has_real_face(&self, style: FontStyle) -> bool {
        let slot = self.styles[style.index()];
        !slot.synth_bold && !slot.synth_italic
    }

    /// The fallback chain, in the order faces are tried.
    ///
    /// Empty for a stack built from caller-supplied bytes, which has no
    /// database to search.
    #[must_use]
    pub fn fallback_chain(&self) -> &[FallbackEntry] {
        &self.chain
    }

    /// Which face in the stack covers `ch`.
    ///
    /// Parses fallback faces lazily, exactly as rasterisation does, and caches
    /// the answer, so asking twice costs one search.
    pub fn coverage(&mut self, ch: char) -> Coverage {
        if self.fonts.iter().any(|font| font.has_glyph(ch)) {
            return Coverage::Primary;
        }
        match self.resolve_fallback(ch) {
            Some(i) => Coverage::Fallback(i),
            None => Coverage::Missing,
        }
    }

    /// Rasterise `ch` in `style`, positioned relative to the cell's top-left
    /// corner.
    ///
    /// A space or NUL produces [`RasterGlyph::blank`]. A character no face in
    /// the chain covers produces a hollow box, sized to the character's column
    /// count, so missing coverage is visible rather than invisible.
    ///
    /// This allocates one coverage buffer per call. It is called once per new
    /// glyph and the result is cached in the atlas, so it never runs on a
    /// steady-state frame.
    pub fn rasterize(&mut self, ch: char, style: FontStyle) -> RasterGlyph {
        if ch == ' ' || ch == '\0' {
            return RasterGlyph::blank();
        }
        let columns = match char_width(ch) {
            CharWidth::Control | CharWidth::ZeroWidth => return RasterGlyph::blank(),
            CharWidth::Narrow => 1,
            CharWidth::Wide => 2,
        };

        let slot = self.styles[style.index()];
        let (font_kind, synth_bold, synth_italic) = if self.fonts[slot.font].has_glyph(ch) {
            (FontRef::Primary(slot.font), slot.synth_bold, slot.synth_italic)
        } else if slot.font != 0 && self.fonts[0].has_glyph(ch) {
            // The styled face lacks this character but the regular face has it.
            // Draw from the regular face and synthesise the style.
            (FontRef::Primary(0), style.is_bold(), style.is_italic())
        } else {
            match self.resolve_fallback(ch) {
                Some(i) => (FontRef::Fallback(i), style.is_bold(), style.is_italic()),
                None => return self.tofu(columns),
            }
        };

        let font = match font_kind {
            FontRef::Primary(i) => &self.fonts[i],
            FontRef::Fallback(i) => match &self.fallback_faces[i] {
                FallbackFace::Loaded(font) => font.as_ref(),
                // `resolve_fallback` only returns indices it has loaded, so
                // this is unreachable in practice; drawing the box beats
                // panicking if that ever stops being true.
                FallbackFace::Unloaded | FallbackFace::Failed => return self.tofu(columns),
            },
        };

        let (metrics, coverage) = font.rasterize(ch, self.size_px);
        if metrics.width == 0 || metrics.height == 0 {
            // A glyph with an empty outline, such as U+00A0 in most faces.
            return RasterGlyph::blank();
        }

        let mut glyph = RasterGlyph {
            width: metrics.width as u32,
            height: metrics.height as u32,
            left: metrics.xmin,
            top: self.metrics.baseline - (metrics.ymin + metrics.height as i32),
            coverage,
        };
        if synth_bold {
            glyph = embolden(glyph);
        }
        if synth_italic {
            glyph = slant(glyph);
        }
        glyph
    }

    /// Find the first fallback face covering `ch`, parsing faces lazily and
    /// remembering the answer (including "nothing has it").
    fn resolve_fallback(&mut self, ch: char) -> Option<usize> {
        if let Some(hit) = self.resolved.get(&ch) {
            return *hit;
        }
        let mut found = None;
        for i in 0..self.fallback_faces.len() {
            if matches!(self.fallback_faces[i], FallbackFace::Unloaded) {
                let id = self.chain[i].id;
                let parsed = match load_face(&self.db, id, &self.chain[i].family, self.size_px) {
                    Ok((font, _)) => FallbackFace::Loaded(Box::new(font)),
                    Err(_) => FallbackFace::Failed,
                };
                self.fallback_faces[i] = parsed;
            }
            if let FallbackFace::Loaded(font) = &self.fallback_faces[i]
                && font.has_glyph(ch)
            {
                found = Some(i);
                break;
            }
        }
        self.resolved.insert(ch, found);
        found
    }

    /// A hollow box for a character no face covers, `columns` cells wide.
    fn tofu(&self, columns: u32) -> RasterGlyph {
        let width = (self.metrics.width * columns).saturating_sub(2).max(1);
        let height = (self.metrics.baseline.max(2) as u32).min(self.metrics.height).max(2);
        let mut coverage = vec![0u8; (width * height) as usize];
        for x in 0..width {
            coverage[x as usize] = 255;
            coverage[((height - 1) * width + x) as usize] = 255;
        }
        for y in 0..height {
            coverage[(y * width) as usize] = 255;
            coverage[(y * width + width - 1) as usize] = 255;
        }
        RasterGlyph {
            width,
            height,
            left: 1,
            top: self.metrics.baseline - height as i32,
            coverage,
        }
    }
}

enum FontRef {
    Primary(usize),
    Fallback(usize),
}

fn check_size(size_px: f32) -> Result<(), FontError> {
    if size_px.is_finite() && (1.0..=MAX_SIZE_PX).contains(&size_px) {
        Ok(())
    } else {
        Err(FontError::InvalidSize(size_px))
    }
}

/// Pick the regular face: explicit families first, then the database's generic
/// monospace family, then the monospaced face whose family name sorts first.
///
/// The final step sorts rather than scanning because `fontdb` stores faces in
/// font-directory walk order, which is not stable between machines. Cell
/// metrics come from whichever face wins, so an unstable choice means unstable
/// geometry.
fn pick_regular(db: &fontdb::Database, families: &[String]) -> Option<fontdb::ID> {
    for name in families {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        if let Some(id) = db.query(&query) {
            return Some(id);
        }
    }
    let generic = fontdb::Query {
        families: &[fontdb::Family::Monospace],
        weight: fontdb::Weight::NORMAL,
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    };
    if let Some(id) = db.query(&generic) {
        return Some(id);
    }
    let sort_key = |face: &fontdb::FaceInfo| {
        (
            face.style != fontdb::Style::Normal,
            face.weight != fontdb::Weight::NORMAL,
            face.families
                .first()
                .map_or_else(String::new, |(name, _)| name.clone()),
            face.post_script_name.clone(),
        )
    };
    db.faces()
        .filter(|f| f.monospaced)
        .min_by_key(|f| sort_key(f))
        .map(|f| f.id)
}

fn primary_family_name(db: &fontdb::Database, id: fontdb::ID) -> String {
    db.face(id)
        .and_then(|f| f.families.first().map(|(name, _)| name.clone()))
        .unwrap_or_else(|| String::from("<unknown>"))
}

/// Parse one face into a fontdue font plus its underline metrics in pixels.
///
/// Both parsers read the same borrowed bytes in one `with_face_data` call, so
/// the face is memory-mapped once rather than twice.
fn load_face(
    db: &fontdb::Database,
    id: fontdb::ID,
    family: &str,
    size_px: f32,
) -> Result<(fontdue::Font, Option<(u32, u32)>), FontError> {
    let loaded = db.with_face_data(id, |data, face_index| {
        let font = parse_font(data, face_index, family)?;
        let underline = ttf_parser::Face::parse(data, face_index)
            .ok()
            .and_then(|face| underline_px(&face, size_px));
        Ok((font, underline))
    });
    loaded.unwrap_or_else(|| {
        Err(FontError::FaceDataUnavailable {
            family: family.to_owned(),
        })
    })
}

fn parse_font(data: &[u8], index: u32, family: &str) -> Result<fontdue::Font, FontError> {
    let settings = fontdue::FontSettings {
        collection_index: index,
        scale: 64.0,
        load_substitutions: false,
    };
    fontdue::Font::from_bytes(data, settings).map_err(|reason| FontError::Parse {
        family: family.to_owned(),
        reason: reason.to_owned(),
    })
}

/// Underline top offset below the baseline and thickness, both in pixels.
fn underline_px(face: &ttf_parser::Face<'_>, size_px: f32) -> Option<(u32, u32)> {
    let metrics = face.underline_metrics()?;
    let upem = f32::from(face.units_per_em());
    if upem <= 0.0 {
        return None;
    }
    let scale = size_px / upem;
    // `position` is the top of the stroke relative to the baseline, negative
    // below it. Cell coordinates grow downward, so flip the sign.
    let below = (-f32::from(metrics.position) * scale).round().max(1.0) as u32;
    let thickness = (f32::from(metrics.thickness) * scale).round().max(1.0) as u32;
    Some((below, thickness))
}

fn cell_metrics(
    font: &fontdue::Font,
    underline: Option<(u32, u32)>,
    size_px: f32,
) -> CellMetrics {
    let line = font.horizontal_line_metrics(size_px);
    let (ascent, descent, gap) = match line {
        Some(m) => (m.ascent, m.descent, m.line_gap),
        // A face with no horizontal line metrics is malformed but rasterisable.
        // These proportions are the OpenType defaults for a 1000-upem face.
        None => (size_px * 0.8, -size_px * 0.2, 0.0),
    };
    let height = (ascent - descent + gap).round().max(1.0) as u32;
    let baseline = ascent.round().max(0.0) as i32;

    let width = ['M', '0', 'W', 'x']
        .into_iter()
        .map(|probe| font.metrics(probe, size_px).advance_width)
        .find(|adv| *adv > 0.0)
        .unwrap_or(size_px * 0.6)
        .round()
        .max(1.0) as u32;

    let (below_baseline, thickness) = underline.unwrap_or_else(|| {
        // No post table: put the rule a quarter of the descender below the
        // baseline and scale the thickness with the em size.
        let below = ((-descent) * 0.25).round().max(1.0) as u32;
        let thickness = (size_px / 14.0).round().max(1.0) as u32;
        (below, thickness)
    });
    let thickness = thickness.clamp(1, height);
    let underline_y = (baseline.max(0) as u32)
        .saturating_add(below_baseline)
        .min(height - thickness);

    CellMetrics {
        width,
        height,
        baseline,
        underline_y,
        underline_thickness: thickness,
    }
}

/// Faces worth trying when the primary family lacks a glyph: other monospaced
/// faces first, then everything else, capped at `limit`.
///
/// Pure. The chain is a function of the database, the primary face, and the
/// limit, and nothing here parses a face or touches a device, so a caller can
/// assert the whole chain on a machine with no display.
#[must_use]
pub fn fallback_chain(
    db: &fontdb::Database,
    regular_id: fontdb::ID,
    family: &str,
    limit: usize,
) -> Vec<FallbackEntry> {
    let mut mono = Vec::new();
    let mut rest = Vec::new();
    for face in db.faces() {
        if face.id == regular_id || face.style != fontdb::Style::Normal {
            continue;
        }
        if face.families.iter().any(|(name, _)| name == family) {
            continue;
        }
        let entry = FallbackEntry {
            id: face.id,
            family: face
                .families
                .first()
                .map_or_else(|| String::from("<unknown>"), |(name, _)| name.clone()),
            monospaced: face.monospaced,
        };
        let bucket = if face.monospaced { &mut mono } else { &mut rest };
        bucket.push(entry);
    }
    mono.into_iter().chain(rest).take(limit).collect()
}

/// Double-strike embolden: OR the bitmap with itself shifted one pixel right.
fn embolden(glyph: RasterGlyph) -> RasterGlyph {
    let (w, h) = (glyph.width, glyph.height);
    let out_w = w + 1;
    let mut coverage = vec![0u8; (out_w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let v = glyph.coverage[(y * w + x) as usize];
            let a = &mut coverage[(y * out_w + x) as usize];
            *a = (*a).max(v);
            let b = &mut coverage[(y * out_w + x + 1) as usize];
            *b = (*b).max(v);
        }
    }
    RasterGlyph {
        width: out_w,
        height: h,
        left: glyph.left,
        top: glyph.top,
        coverage,
    }
}

/// Shear the bitmap so the top leans right, pivoting on the bottom row.
fn slant(glyph: RasterGlyph) -> RasterGlyph {
    let (w, h) = (glyph.width, glyph.height);
    let extra = (SYNTHETIC_SLANT * (h.saturating_sub(1)) as f32).ceil() as u32;
    if extra == 0 {
        return glyph;
    }
    let out_w = w + extra;
    let mut coverage = vec![0u8; (out_w * h) as usize];
    for y in 0..h {
        let shift = (SYNTHETIC_SLANT * (h - 1 - y) as f32).round() as u32;
        for x in 0..w {
            coverage[(y * out_w + x + shift) as usize] = glyph.coverage[(y * w + x) as usize];
        }
    }
    RasterGlyph {
        width: out_w,
        height: h,
        left: glyph.left,
        top: glyph.top,
        coverage,
    }
}
