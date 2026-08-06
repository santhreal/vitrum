//! Face discovery, cell metrics, and glyph bitmaps.

use crate::font::{
    DEFAULT_FAMILIES, FontConfig, FontError, FontStack, FontStyle, MAX_SIZE_PX, RasterGlyph,
};
use crate::cell::Attrs;
use crate::tests::support::{TEST_PX, config_at, fonts, fonts_at, system_db};

/// Discovery must produce a face and report which family it picked.
///
/// A renderer that silently fell back to an unnamed face would make every
/// metrics bug impossible to reproduce, because nobody would know which font
/// produced the numbers. The family name is printed by
/// [`FontStack::family`] for exactly that reason.
#[test]
fn system_discovery_names_the_family_it_chose() {
    let stack = fonts();
    assert!(
        !stack.family().is_empty() && stack.family() != "<unknown>",
        "discovery must name a real family, got {:?}",
        stack.family()
    );
    assert_eq!(stack.size_px(), TEST_PX);
}

/// Repeated discovery with the same configuration must pick the same face.
///
/// Cell metrics come from whichever face wins, so an unstable choice means the
/// same application lays its grid out differently between runs. `fontdb`
/// stores faces in font-directory walk order, which is not a stable key, so
/// the fall-through has to sort rather than take the first hit.
#[test]
fn discovery_is_deterministic_across_repeated_calls() {
    let first = fonts();
    for attempt in 0..4 {
        let again = fonts();
        assert_eq!(
            again.family(),
            first.family(),
            "attempt {attempt} chose a different family"
        );
        assert_eq!(again.metrics(), first.metrics());
    }

    // The unconfigured path (no preferred families at all) must be stable too,
    // because that is the branch that reaches the sorted fall-through.
    let bare = || {
        FontStack::from_database(system_db(), &FontConfig {
            families: Vec::new(),
            ..config_at(TEST_PX)
        })
        .expect("a bare configuration must still find a monospace face")
    };
    let baseline = bare();
    for attempt in 0..4 {
        let again = bare();
        assert_eq!(
            again.family(),
            baseline.family(),
            "bare attempt {attempt} chose a different family"
        );
    }
}

/// The default configuration must prefer a family from [`DEFAULT_FAMILIES`]
/// when one is installed.
///
/// Without the list, discovery falls through to "whichever monospaced face
/// sorts first", which on a developer machine picks up whatever obscure Type 1
/// or bitmap face happens to be installed. A terminal must open in a font
/// somebody would choose.
#[test]
fn the_default_configuration_prefers_a_known_terminal_family() {
    let stack = FontStack::from_database(system_db(), &FontConfig::default())
        .expect("the default configuration must find a face");
    let installed: Vec<&&str> = DEFAULT_FAMILIES
        .iter()
        .filter(|name| {
            system_db()
                .query(&fontdb::Query {
                    families: &[fontdb::Family::Name(name)],
                    weight: fontdb::Weight::NORMAL,
                    stretch: fontdb::Stretch::Normal,
                    style: fontdb::Style::Normal,
                })
                .is_some()
        })
        .collect();
    assert!(
        !installed.is_empty(),
        "this machine has none of {DEFAULT_FAMILIES:?} installed, so the preference \
         order cannot be checked; install DejaVu Sans Mono or Liberation Mono"
    );
    assert_eq!(
        stack.family(),
        *installed[0],
        "discovery must take the first installed name in preference order"
    );
}

/// An explicitly requested family must beat the default list.
///
/// This is the user's font setting. If the defaults could override it, a
/// configured font would appear to be ignored.
#[test]
fn an_explicit_family_beats_the_default_list() {
    let default_family = FontStack::from_database(system_db(), &FontConfig::default())
        .expect("the default configuration must find a face")
        .family()
        .to_owned();

    // Pick any installed monospace family that is not the default choice.
    let db = system_db();
    let other = db
        .faces()
        .filter(|f| f.monospaced && f.style == fontdb::Style::Normal)
        .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
        .find(|name| *name != default_family)
        .expect("the test machine must have a second monospace family");

    let stack = FontStack::from_database(db, &FontConfig {
        families: vec![other.clone()],
        ..config_at(TEST_PX)
    })
    .expect("an explicitly named installed family must be usable");
    assert_eq!(stack.family(), other);
    assert_ne!(stack.family(), default_family);
}

/// Cell metrics must be positive and internally consistent.
///
/// Every coordinate the renderer computes is `column * width` and
/// `row * height`. A zero width collapses the whole grid onto one column; a
/// baseline past the cell bottom pushes every glyph out of its own cell.
#[test]
fn cell_metrics_are_positive_and_self_consistent() {
    let m = fonts().metrics();
    assert!(m.width >= 1, "cell width must be at least one pixel");
    assert!(m.height >= 1, "cell height must be at least one pixel");
    assert!(
        m.baseline > 0 && m.baseline <= m.height as i32,
        "baseline {} must sit inside a {}px cell",
        m.baseline,
        m.height
    );
    assert!(m.underline_thickness >= 1);
    assert!(
        m.underline_y + m.underline_thickness <= m.height,
        "underline rule {}..{} must fit inside a {}px cell",
        m.underline_y,
        m.underline_y + m.underline_thickness,
        m.height
    );
    assert!(
        m.underline_y >= m.baseline as u32,
        "the underline must sit at or below the baseline, not through the text"
    );
}

/// Cell size must scale with the requested pixel size.
///
/// Font size is a user setting. If metrics were computed from a fixed size, the
/// grid would keep its geometry while the glyphs changed size and text would
/// either overlap or float in oversized cells.
#[test]
fn cell_metrics_scale_with_the_requested_size() {
    let small = fonts_at(10.0).metrics();
    let large = fonts_at(30.0).metrics();
    assert!(
        large.width > small.width,
        "30px cells ({}) must be wider than 10px cells ({})",
        large.width,
        small.width
    );
    assert!(
        large.height > small.height,
        "30px cells ({}) must be taller than 10px cells ({})",
        large.height,
        small.height
    );
    assert!(large.baseline > small.baseline);
}

/// A monospace face must give every ASCII glyph the same advance.
///
/// This is what makes the grid a grid. A proportional face slipped into
/// discovery would still render, but every column after the first would be
/// wrong, and the bug would look like a layout problem rather than a font
/// selection one.
#[test]
fn the_discovered_face_is_actually_monospaced() {
    let mut stack = fonts();
    let width = stack.metrics().width;
    // Rasterised ink can be narrower than the advance; what must hold is that
    // no ASCII glyph is wider than one cell.
    for ch in ('!'..='~').chain(['a', 'W', '0', '@']) {
        let glyph = stack.rasterize(ch, FontStyle::Regular);
        assert!(
            glyph.width <= width + 2,
            "{ch:?} rasterised {}px wide in a {width}px cell; the face is not monospaced",
            glyph.width
        );
    }
}

/// Spaces and NUL must produce no bitmap at all.
///
/// A screen is mostly spaces. Giving each one an atlas entry would fill the
/// atlas with identical empty rectangles and force a reset, which triggers a
/// full-grid re-upload. Blank cells must cost nothing.
#[test]
fn blanks_rasterize_to_nothing() {
    let mut stack = fonts();
    for ch in [' ', '\0'] {
        for style in FontStyle::ALL {
            let glyph = stack.rasterize(ch, style);
            assert!(glyph.is_blank(), "{ch:?} in {style:?} must have no ink");
            assert_eq!(glyph.width, 0);
            assert_eq!(glyph.height, 0);
            assert!(glyph.coverage.is_empty());
            assert_eq!(glyph, RasterGlyph::blank());
        }
    }
}

/// A rasterised glyph's coverage buffer must be exactly `width * height` bytes.
///
/// The atlas uploads this buffer with `bytes_per_row = width` and
/// `rows_per_image = height`. A buffer of any other length is either a wgpu
/// validation error or a read past the end of the allocation.
#[test]
fn coverage_buffer_length_matches_the_declared_size() {
    let mut stack = fonts();
    for ch in ['A', 'g', 'W', '#', '漢', '\u{2588}'] {
        for style in FontStyle::ALL {
            let glyph = stack.rasterize(ch, style);
            assert_eq!(
                glyph.coverage.len(),
                (glyph.width * glyph.height) as usize,
                "{ch:?} in {style:?}: {}x{} declared but {} coverage bytes",
                glyph.width,
                glyph.height,
                glyph.coverage.len()
            );
        }
    }
}

/// A letter with a solid stem must produce real ink, not an empty box.
///
/// If the rasteriser silently returned zeros, every pixel test would still pass
/// on the background and the terminal would render blank. Asserting that some
/// pixel reaches full coverage proves the outline was actually filled.
#[test]
fn a_solid_letter_rasterizes_to_real_coverage() {
    let mut stack = fonts();
    let glyph = stack.rasterize('W', FontStyle::Regular);
    assert!(!glyph.is_blank(), "'W' must have a bitmap");

    let max = glyph.coverage.iter().copied().max().unwrap_or(0);
    assert!(
        max >= 200,
        "'W' peaked at coverage {max}; the outline was not filled"
    );
    let inked = glyph.coverage.iter().filter(|c| **c > 0).count();
    assert!(
        inked >= 10,
        "'W' produced only {inked} inked pixels out of {}",
        glyph.coverage.len()
    );
}

/// `coverage_at` must read the bitmap row-major and return 0 outside it.
///
/// The pixel tests build their CPU reference through this accessor. A
/// transposed read would make the reference agree with a transposed renderer
/// and both bugs would cancel out invisibly.
#[test]
fn coverage_at_reads_row_major_and_clamps_outside() {
    let glyph = RasterGlyph {
        width: 3,
        height: 2,
        left: 0,
        top: 0,
        coverage: vec![10, 20, 30, 40, 50, 60],
    };
    assert_eq!(glyph.coverage_at(0, 0), 10);
    assert_eq!(glyph.coverage_at(2, 0), 30);
    assert_eq!(glyph.coverage_at(0, 1), 40);
    assert_eq!(glyph.coverage_at(2, 1), 60);
    assert_eq!(glyph.coverage_at(3, 0), 0, "past the right edge");
    assert_eq!(glyph.coverage_at(0, 2), 0, "past the bottom edge");
    assert_eq!(glyph.coverage_at(u32::MAX, u32::MAX), 0);
}

/// Bold must produce a different bitmap from regular, real face or synthesised.
///
/// SGR 1 is the most common attribute an agent emits. If bold resolved to the
/// same bitmap, the renderer would still upload a second atlas entry and draw
/// it, so the cost would be paid with none of the benefit and nobody would see
/// a difference on screen.
#[test]
fn bold_differs_from_regular() {
    let mut stack = fonts();
    let regular = stack.rasterize('E', FontStyle::Regular);
    let bold = stack.rasterize('E', FontStyle::Bold);

    assert!(!regular.is_blank() && !bold.is_blank());
    assert_ne!(
        (regular.width, regular.height, &regular.coverage),
        (bold.width, bold.height, &bold.coverage),
        "bold 'E' rasterised identically to regular 'E' (real face: {})",
        stack.has_real_face(FontStyle::Bold)
    );

    let ink = |g: &RasterGlyph| g.coverage.iter().map(|c| u32::from(*c)).sum::<u32>();
    assert!(
        ink(&bold) > ink(&regular),
        "bold must lay down more ink: bold {} vs regular {}",
        ink(&bold),
        ink(&regular)
    );
}

/// Italic must produce a different bitmap from regular.
///
/// The counterpart for SGR 3. When a family has no italic face the bitmap is
/// sheared; a stack that quietly dropped the shear would render italics
/// upright and the terminal would misreport the agent's output.
#[test]
fn italic_differs_from_regular() {
    let mut stack = fonts();
    let regular = stack.rasterize('l', FontStyle::Regular);
    let italic = stack.rasterize('l', FontStyle::Italic);

    assert!(!regular.is_blank() && !italic.is_blank());
    assert_ne!(
        (regular.width, &regular.coverage),
        (italic.width, &italic.coverage),
        "italic 'l' rasterised identically to regular 'l' (real face: {})",
        stack.has_real_face(FontStyle::Italic)
    );
}

/// All four style slots must be distinguishable from each other.
///
/// Bold-italic is the case that gets forgotten: a slot table with a copy-paste
/// error maps it onto plain bold and every emphasised comment in a diff renders
/// wrong.
#[test]
fn all_four_style_slots_produce_distinct_bitmaps() {
    let mut stack = fonts();
    let glyphs: Vec<RasterGlyph> = FontStyle::ALL
        .iter()
        .map(|s| stack.rasterize('R', *s))
        .collect();

    for (i, a) in glyphs.iter().enumerate() {
        assert!(!a.is_blank(), "{:?} produced no bitmap", FontStyle::ALL[i]);
        for (j, b) in glyphs.iter().enumerate().skip(i + 1) {
            assert_ne!(
                (a.width, a.height, &a.coverage),
                (b.width, b.height, &b.coverage),
                "{:?} and {:?} rasterised identically",
                FontStyle::ALL[i],
                FontStyle::ALL[j]
            );
        }
    }
}

/// The style a cell's attributes select must be the style that gets rasterised.
///
/// This is the join between the attribute bits and the font slots. Testing the
/// mapping and the rasteriser separately would miss a renderer that looked up
/// the right slot and then drew from the wrong one.
#[test]
fn attributes_select_the_matching_rasterized_face() {
    let mut stack = fonts();
    for (attrs, style) in [
        (Attrs::NONE, FontStyle::Regular),
        (Attrs::BOLD, FontStyle::Bold),
        (Attrs::ITALIC, FontStyle::Italic),
        (Attrs::BOLD | Attrs::ITALIC, FontStyle::BoldItalic),
        (Attrs::BOLD | Attrs::UNDERLINE, FontStyle::Bold),
        (Attrs::ITALIC | Attrs::REVERSE, FontStyle::Italic),
    ] {
        assert_eq!(FontStyle::from_attrs(attrs), style);
        let by_attr = stack.rasterize('S', FontStyle::from_attrs(attrs));
        let by_style = stack.rasterize('S', style);
        assert_eq!(by_attr, by_style, "{attrs:?} must rasterise as {style:?}");
    }
}

/// A wide character must rasterise wider than one cell, glyph or fallback box.
///
/// The renderer gives a wide head a two-column quad. If the bitmap were only
/// one cell wide, CJK text would render in the left half of its box with a gap
/// on the right, which looks like a spacing bug rather than a font one.
#[test]
fn a_wide_character_rasterizes_wider_than_one_cell() {
    let mut stack = fonts();
    let cell_w = stack.metrics().width;
    for ch in ['漢', 'あ', '한'] {
        let glyph = stack.rasterize(ch, FontStyle::Regular);
        assert!(!glyph.is_blank(), "{ch:?} must produce a bitmap or a box");
        assert!(
            glyph.width > cell_w,
            "{ch:?} rasterised {}px wide, which fits in one {cell_w}px column",
            glyph.width
        );
        assert!(
            glyph.width <= cell_w * 2 + 2,
            "{ch:?} rasterised {}px wide, past its two-column box",
            glyph.width
        );
    }
}

/// A codepoint no installed face covers must render a visible box, not nothing.
///
/// Invisible missing glyphs are the worst possible failure: the output looks
/// like the agent printed nothing. A hollow box tells the operator the terminal
/// received a character it cannot draw.
#[test]
fn an_uncovered_codepoint_renders_a_visible_box() {
    // Plane 16 Private Use. U+F8FF (the BMP PUA slot) is not usable here: a
    // system with a Mac-oriented font set really does map it, which would make
    // this test pass or fail depending on which fonts are installed.
    const UNCOVERED: char = '\u{10fffd}';

    let mut stack = fonts();
    let glyph = stack.rasterize(UNCOVERED, FontStyle::Regular);
    assert!(
        !glyph.is_blank(),
        "an uncovered codepoint must not vanish; it rendered nothing"
    );
    let corners = [
        glyph.coverage_at(0, 0),
        glyph.coverage_at(glyph.width - 1, 0),
        glyph.coverage_at(0, glyph.height - 1),
        glyph.coverage_at(glyph.width - 1, glyph.height - 1),
    ];
    assert_eq!(
        corners,
        [255, 255, 255, 255],
        "the fallback box must have solid corners"
    );
    assert!(
        glyph.width > 2 && glyph.height > 2,
        "the fallback box must be big enough to be visible, got {}x{}",
        glyph.width,
        glyph.height
    );
    assert_eq!(
        glyph.coverage_at(glyph.width / 2, glyph.height / 2),
        0,
        "the fallback box must be hollow, not a filled rectangle"
    );

    // The same must hold with no fallback chain at all, which is the path an
    // application shipping a single bundled font takes.
    let bytes = crate::tests::support::primary_face_bytes();
    let mut solo = FontStack::from_face_bytes(&bytes, 0, TEST_PX).unwrap();
    let solo_glyph = solo.rasterize(UNCOVERED, FontStyle::Regular);
    assert_eq!(
        solo_glyph.coverage_at(0, 0),
        255,
        "a stack with no fallbacks must still draw the box"
    );
}

/// Rasterising the same character twice must give byte-identical results.
///
/// The atlas caches the first result forever. If rasterisation were not
/// deterministic, a glyph would look different after an atlas reset than it did
/// before, and the difference would appear only under memory pressure.
#[test]
fn rasterization_is_deterministic() {
    let mut a = fonts();
    let mut b = fonts();
    for ch in ['A', 'g', '@', '漢', '\u{f8ff}'] {
        for style in FontStyle::ALL {
            assert_eq!(
                a.rasterize(ch, style),
                b.rasterize(ch, style),
                "{ch:?} in {style:?} rasterised differently across two stacks"
            );
            let repeat = a.rasterize(ch, style);
            assert_eq!(
                repeat,
                b.rasterize(ch, style),
                "{ch:?} in {style:?} rasterised differently on a repeat call"
            );
        }
    }
}

/// A glyph's placement must put its bitmap inside the cell for normal text.
///
/// `top` is measured down from the cell's top edge and drives the shader's
/// clipping test. A sign error there hides every glyph, because the whole
/// bitmap lands outside the cell rectangle and gets clipped away.
#[test]
fn glyph_placement_puts_ascii_ink_inside_the_cell() {
    let mut stack = fonts();
    let m = stack.metrics();
    for ch in ['A', 'x', 'g', '|', '_'] {
        let glyph = stack.rasterize(ch, FontStyle::Regular);
        assert!(!glyph.is_blank());
        assert!(
            glyph.top >= 0,
            "{ch:?} sits {}px above the cell top and would be clipped away",
            -glyph.top
        );
        assert!(
            glyph.top + glyph.height as i32 <= m.height as i32,
            "{ch:?} extends to y={} in a {}px cell",
            glyph.top + glyph.height as i32,
            m.height
        );
        assert!(
            glyph.left >= -1 && glyph.left < m.width as i32,
            "{ch:?} starts at x={} in a {}px cell",
            glyph.left,
            m.width
        );
    }
}

/// Invalid font sizes must be refused with the offending value.
///
/// A size of zero divides by zero in the metrics scale and a NaN propagates
/// into every vertex position, blanking the screen with no error anywhere.
/// Rejecting at construction keeps the failure attached to its cause.
#[test]
fn invalid_font_sizes_are_refused() {
    for bad in [0.0f32, -1.0, f32::NAN, f32::INFINITY, MAX_SIZE_PX + 1.0, 0.5] {
        let err = FontStack::system(&FontConfig {
            families: Vec::new(),
            size_px: bad,
            max_fallback_faces: 4,
        })
        .expect_err("size {bad} must be refused");
        match err {
            FontError::InvalidSize(got) => {
                assert!(
                    got.to_bits() == bad.to_bits(),
                    "error must carry the rejected size: got {got}, sent {bad}"
                );
            }
            other => panic!("expected InvalidSize for {bad}, got {other:?}"),
        }
    }
    assert!(
        FontStack::system(&FontConfig {
            families: Vec::new(),
            size_px: MAX_SIZE_PX,
            max_fallback_faces: 0,
        })
        .is_ok(),
        "the documented maximum size must be accepted"
    );
}

/// A nonexistent requested family must fall through to discovery, not fail.
///
/// Users configure font names that are not installed all the time. Refusing to
/// start is the wrong answer; picking a monospace face and carrying on is the
/// right one, and the chosen family is reported so the mismatch is visible.
#[test]
fn an_unknown_requested_family_falls_through_to_discovery() {
    let stack = FontStack::from_database(
        {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            db
        },
        &FontConfig {
            families: vec![String::from("No Such Font 9000")],
            size_px: TEST_PX,
            max_fallback_faces: 4,
        },
    )
    .expect("an unknown family must not prevent discovery");
    assert_ne!(stack.family(), "No Such Font 9000");
    assert!(stack.metrics().width >= 1);
}

/// An empty font database must produce the dedicated no-font error.
///
/// On a stripped container there may be no fonts at all. That must say so
/// plainly rather than surfacing as a parse failure or a panic three layers
/// down in the rasteriser.
#[test]
fn an_empty_font_database_reports_no_monospace_font() {
    let err = FontStack::from_database(fontdb::Database::new(), &FontConfig::default())
        .expect_err("an empty database cannot yield a face");
    assert_eq!(err, FontError::NoMonospaceFont);
    let message = err.to_string();
    assert!(
        message.contains("no monospace font"),
        "the message must name the problem: {message}"
    );
}

/// Garbage bytes must be reported as a parse failure, not panic.
///
/// An application shipping its own font can ship a truncated one. The error
/// carries the family label so a log line identifies which face failed.
#[test]
fn malformed_font_bytes_are_reported_as_a_parse_error() {
    let err = FontStack::from_face_bytes(b"not a font at all", 0, TEST_PX)
        .expect_err("garbage bytes must not parse");
    match err {
        FontError::Parse { family, reason } => {
            assert_eq!(family, "<embedded>");
            assert!(!reason.is_empty(), "the parser must say what went wrong");
        }
        other => panic!("expected Parse, got {other:?}"),
    }
}

/// A caller-supplied face must drive all four slots with synthetic styling.
///
/// An application that ships one font file still has to render bold and italic.
/// Falling back to "regular for everything" would make emphasis invisible.
#[test]
fn a_caller_supplied_face_synthesizes_all_four_styles() {
    let db = {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    };
    let system = FontStack::from_database(db.clone(), &FontConfig::default())
        .expect("discovery must succeed");
    let family = system.family().to_owned();
    let id = db
        .query(&fontdb::Query {
            families: &[fontdb::Family::Name(&family)],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        })
        .expect("the discovered family must still resolve");
    let bytes = db
        .with_face_data(id, |data, _| data.to_vec())
        .expect("face data must be readable");

    let mut stack = FontStack::from_face_bytes(&bytes, 0, TEST_PX)
        .expect("a face the system uses must parse");
    assert_eq!(stack.family(), "<embedded>");
    assert!(!stack.has_real_face(FontStyle::Bold));
    assert!(!stack.has_real_face(FontStyle::Italic));
    assert!(stack.has_real_face(FontStyle::Regular));

    let regular = stack.rasterize('n', FontStyle::Regular);
    let bold = stack.rasterize('n', FontStyle::Bold);
    let italic = stack.rasterize('n', FontStyle::Italic);
    assert_eq!(bold.width, regular.width + 1, "double-strike adds one column");
    assert!(
        italic.width > regular.width,
        "the shear must widen the bitmap: {} vs {}",
        italic.width,
        regular.width
    );
    assert_eq!(italic.height, regular.height, "shearing must not change height");
}
