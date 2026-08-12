//! What the stylesheet is not allowed to do.
//!
//! Three of these guard a defect the product is being rejected for, and each
//! is written against the property rather than against the reproduction: a
//! looping animation anywhere, a strip beside the pane whose height depends on
//! what it has to say, and an element that is absent until a fact resolves.
//! The remaining ones keep the design system a system: no value that is not a
//! token, no token that does not resolve, and one repaint per preference.

use super::*;
use crate::state::Settings;

/// The two colours an unconfigured grid paints, for the looks these tests
/// build by hand.
///
/// Read from the pane's own default rather than written out, so a change to
/// what the terminal clears to cannot leave these fixtures describing a
/// colour nothing paints.
const TEST_TERMINAL: (Rgb, Rgb) = {
    let p = crate::pane::theme::Palette::DEFAULT;
    (
        Rgb(p.background.r, p.background.g, p.background.b),
        Rgb(p.foreground.r, p.foreground.g, p.foreground.b),
    )
};

/// Every look the generator has to be correct for.
///
/// The product of the four preferences, not a hand-picked sample, so a rule
/// that only holds on the default theme at the default scale fails here.
fn every_look() -> Vec<Look> {
    let mut out = Vec::new();
    for scheme in [Scheme::Dark, Scheme::Light] {
        for density in [Density::Comfortable, Density::Compact] {
            for pct in [
                crate::state::TEXT_SCALE_MIN_PCT,
                100,
                crate::state::TEXT_SCALE_MAX_PCT,
            ] {
                for reduce_motion in [false, true] {
                    for hues in [None, Some(ChromeHues::from_palette(&fixture(), scheme))] {
                        out.push(Look {
                            scheme,
                            density,
                            text_scale_pct: pct,
                            reduce_motion,
                            hues,
                            terminal: TEST_TERMINAL,
                        });
                    }
                }
            }
        }
    }
    out
}

/// A palette with all sixteen slots distinct, so a test that reads one back
/// can tell which slot it came from.
fn fixture() -> PanePalette {
    let mut ansi = [[0u8, 0, 0, 255]; 16];
    for (i, slot) in ansi.iter_mut().enumerate() {
        let n = u8::try_from(i).expect("sixteen slots");
        *slot = [n * 16, 255 - n * 16, 128, 255];
    }
    PanePalette {
        ansi,
        background: [1, 2, 3, 255],
        foreground: [250, 251, 252, 255],
        cursor: [4, 5, 6, 255],
        selection_bg: [7, 8, 9, 255],
        selection_fg: [10, 11, 12, 255],
    }
}

/// Strip `/* ... */` so a comment may talk about a colour or a duration
/// without being read as one.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The declarations of every rule whose selector list names exactly `class`.
///
/// Exactly, so `.rg-foo` does not collect `.rg-foobar`'s block: the character
/// after the name has to be one that cannot continue a CSS identifier, which
/// is the same escape that made a substring check useless in the sheet this
/// replaces.
fn blocks<'a>(css: &'a str, class: &str) -> Vec<&'a str> {
    let needle = format!(".{class}");
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(found) = css[at..].find(&needle) {
        let start = at + found;
        at = start + needle.len();
        let next = css[at..].chars().next();
        if next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        let Some(open) = css[at..].find('{') else {
            continue;
        };
        // A `,` before the `{` means the name is one of several selectors and
        // the block still applies to it, which is what we want. A `{` that
        // comes after another `}` would mean we ran past the end of the rule.
        if css[at..at + open].contains('}') {
            continue;
        }
        let body_start = at + open + 1;
        let Some(close) = css[body_start..].find('}') else {
            continue;
        };
        out.push(&css[body_start..body_start + close]);
    }
    out
}

/// One declaration's value out of a block, or `None`.
///
/// Anything before the last brace is dropped first, so the `:` of a
/// `.rg-btn:hover` selector is not mistaken for the one separating a
/// declaration. Without that a rule whose selector list carries a pseudo-class
/// reads as having no declarations at all, which is a guard that passes on a
/// sheet it never looked at.
fn value<'a>(block: &'a str, property: &str) -> Option<&'a str> {
    block.split(';').find_map(|decl| {
        let decl = decl.rsplit(['{', '}']).next()?;
        let (name, value) = decl.split_once(':')?;
        (name.trim() == property).then(|| value.trim())
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// The template is the only place a value may be spelled, and it may not
// spell one
// ═══════════════════════════════════════════════════════════════════════════

/// No colour and no length is written into a template.
///
/// This is what keeps the design system a system. A `12px` typed into a rule
/// is a number outside the scale, invisible to the text-scale multiplier and
/// to the density switch, and the web sheet this replaces had accumulated two
/// hundred of them across fourteen files. Every value here goes through
/// [`super::tokens`] or it does not exist.
///
/// Zero is allowed, unitless: `padding: 0` and `border: none` are the absence
/// of a value rather than a value.
#[test]
fn the_template_declares_no_literal_value() {
    for (name, template) in [("shell.css", TEMPLATE), ("shell-motion.css", MOTION)] {
        // Placeholders out first: a token's NAME may legitimately contain
        // `px`-like text and its VALUE is generated, not written here.
        let mut outside = String::new();
        let mut rest = strip_comments(template);
        let mut cursor = rest.as_str();
        while let Some(open) = cursor.find('$') {
            outside.push_str(&cursor[..open]);
            let after = &cursor[open + 1..];
            let close = after
                .find('$')
                .unwrap_or_else(|| panic!("{name} has an unpaired $"));
            cursor = &after[close + 1..];
        }
        outside.push_str(cursor);
        rest = outside;

        for banned in ["#", "rgb(", "hsl("] {
            assert!(
                !rest.contains(banned),
                "{name} writes {banned:?} directly instead of using a token"
            );
        }
        // Every remaining digit run must be a lone zero.
        //
        // A substring scan for "px" cannot do this job: "empty" and "opts"
        // both contain "pt", and a guard that fires on a class name teaches
        // its author to rename the class. What a literal length always has is
        // a digit, and after the placeholders are gone the only digit the
        // sheet legitimately needs is the zero in `padding: 0`.
        let mut chars = rest.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            if !c.is_ascii_digit() {
                continue;
            }
            let mut end = at + 1;
            while chars.peek().is_some_and(|(_, c)| c.is_ascii_digit()) {
                end = chars.next().expect("peeked").0 + 1;
            }
            let run = &rest[at..end];
            let next = rest[end..].chars().next();
            assert!(
                run == "0" && !next.is_some_and(|c| c.is_ascii_alphabetic() || c == '%'),
                "{name} writes the literal {run:?} instead of using a token"
            );
        }
    }
}

/// Every `$name$` a template uses is a token the generator declares.
///
/// An unresolved placeholder is not a visible failure: GTK drops the one
/// declaration it cannot parse and paints the rest, so the element loses a
/// colour and looks like a layout bug.
#[test]
fn every_placeholder_resolves() {
    for look in every_look() {
        let tokens = tokens(&look);
        for (name, template) in [("shell.css", TEMPLATE), ("shell-motion.css", MOTION)] {
            let (out, unknown) = render(template, &tokens);
            assert!(
                unknown.is_empty(),
                "{name} uses placeholders the generator does not declare: {unknown:?}"
            );
            assert!(
                !out.contains('$'),
                "{name} still has a placeholder in it after substitution"
            );
        }
    }
}

/// The substituter itself, or the guard above passes on a sheet it never read.
#[test]
fn render_substitutes_and_reports_what_it_cannot() {
    let tokens = vec![("a".to_string(), "1px".to_string())];
    let (out, unknown) = render("x: $a$;", &tokens);
    assert_eq!(out, "x: 1px;");
    assert!(unknown.is_empty());

    let (out, unknown) = render("x: $b$;", &tokens);
    assert_eq!(unknown, vec!["b".to_string()]);
    assert_eq!(out, "x: ;", "the unknown name is dropped, not left in");

    let (_, unknown) = render("x: $a", &tokens);
    assert_eq!(unknown, vec!["a".to_string()]);

    assert_eq!(render("no markers", &tokens).0, "no markers");
}

/// Every length the generated sheet contains is on the four-pixel grid, or is
/// the hairline, or is the full-round radius.
///
/// The grid is the reason the interface looks deliberate. Compact density
/// broke it once, putting an 18px head line and a 6px inset into a list
/// otherwise built on fours, and the row pitch it produced landed on no grid
/// line anywhere else in the product.
///
/// Only at 100%: the text scale is a multiplier and 80% of a four is not a
/// four. What the scale must not do is round two lengths that were equal into
/// two that are not, which is a separate assertion below.
#[test]
fn every_length_is_on_the_grid() {
    for (name, value) in LENGTHS.iter().chain(COMPACT) {
        let n = *value as i64;
        assert!(
            n % 4 == 0 || n == 1 || n == 2 || n == 999,
            "{name} is {n}px, off the four-pixel grid, and is neither the \
             hairline, the focus ring nor the full-round radius"
        );
    }
}

/// Every pixel value in a generated sheet is one a token produced.
///
/// The complement of the template guard: that one refuses a literal written
/// into the source, this one refuses one that arrives any other way.
#[test]
fn every_pixel_value_in_the_sheet_came_from_a_token() {
    for look in every_look() {
        // Every pixel run that appears anywhere in a token value, not only a
        // token that IS one. A shadow is one token carrying four offsets, and
        // the offsets are as much part of the system as the token is.
        let mut allowed: Vec<String> = Vec::new();
        for (_, value) in tokens(&look) {
            for (at, _) in value.match_indices("px") {
                let start = value[..at]
                    .rfind(|c: char| !c.is_ascii_digit())
                    .map_or(0, |i| i + 1);
                allowed.push(value[start..at + 2].to_string());
            }
        }
        let css = strip_comments(&stylesheet(&look));
        let mut seen = 0usize;
        for (at, _) in css.match_indices("px") {
            let head = &css[..at];
            let start = head
                .rfind(|c: char| !c.is_ascii_digit())
                .map_or(0, |i| i + 1);
            let length = &css[start..at + 2];
            assert!(
                allowed.contains(&length.to_string()),
                "{length} is in the sheet and no token produced it"
            );
            seen += 1;
        }
        assert!(seen > 200, "only {seen} lengths read; the scan broke");
    }
}

/// Every font size the generated sheet contains comes from the type scale.
#[test]
fn every_font_size_comes_from_the_type_scale() {
    let scale: Vec<i64> = TYPE.iter().map(|(_, px)| *px as i64).collect();
    let css = stylesheet(&Look {
        scheme: Scheme::Dark,
        density: Density::Comfortable,
        text_scale_pct: 100,
        reduce_motion: false,
        hues: None,
        terminal: TEST_TERMINAL,
    });
    let mut seen = 0usize;
    for block in css.split('}') {
        let Some(size) = value(block, "font-size") else {
            continue;
        };
        let n: i64 = size
            .trim_end_matches("px")
            .parse()
            .unwrap_or_else(|_| panic!("{size} is not a pixel size"));
        assert!(scale.contains(&n), "{n}px is not a step of the type scale");
        seen += 1;
    }
    assert!(seen > 40, "only {seen} font sizes read; the scan broke");
}

// ═══════════════════════════════════════════════════════════════════════════
// Nothing loops
// ═══════════════════════════════════════════════════════════════════════════

/// No look produces a sheet that animates on a loop.
///
/// A looping animation repaints the window at the display's refresh rate for
/// as long as it is on screen, forever on an idle window, and its cost grows
/// with the number of lit rows. That is the specific defect this client exists
/// to avoid, so it is banned outright rather than capped: there is no
/// `animation` property in either template at all, which is a stronger rule
/// than pinning an iteration count and one nobody can weaken by editing a
/// number.
#[test]
fn nothing_in_the_sheet_can_loop() {
    for (name, template) in [("shell.css", TEMPLATE), ("shell-motion.css", MOTION)] {
        let code = strip_comments(template);
        assert!(
            !code.contains("animation"),
            "{name} declares an animation; the product has none, and a \
             one-shot added here is one edit away from a looping one"
        );
        assert!(!code.contains("infinite"), "{name} declares a loop");
    }
    for look in every_look() {
        let css = strip_comments(&stylesheet(&look));
        assert!(!css.contains("animation"), "a generated sheet animates");
        assert!(!css.contains("infinite"), "a generated sheet loops");
    }
}

/// Longest transition any rule may declare.
///
/// 200ms, for exactly one case: the status pill's colour change. The word and
/// the glyph swap instantly, so the fade is not carrying the information, it
/// is settling the surface behind it, and at that job 200ms reads as settling
/// where 90ms reads as a flicker.
const MAX_TRANSITION_MS: f64 = 200.0;

/// Properties a transition must never name, because animating one relayouts
/// the window on every frame it runs, and the window contains a pty whose size
/// is a function of that layout.
const BANNED_PROPERTIES: [&str; 12] = [
    "width",
    "height",
    "min-width",
    "min-height",
    "top",
    "left",
    "right",
    "bottom",
    "margin",
    "padding",
    "border-width",
    "font-size",
];

/// Every transition, as (declaration, milliseconds).
fn transitions(css: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for block in strip_comments(css).split('}') {
        let Some(value) = value(block, "transition") else {
            continue;
        };
        for part in value.split(',') {
            let ms = part.split_whitespace().find_map(|token| {
                if let Some(n) = token.strip_suffix("ms") {
                    return n.parse::<f64>().ok();
                }
                token.strip_suffix('s')?.parse::<f64>().ok().map(|v| v * 1000.0)
            });
            if let Some(ms) = ms {
                out.push((part.trim().to_string(), ms));
            }
        }
    }
    out
}

/// The duration parser, or the cap below passes on a sheet full of one-second
/// transitions.
#[test]
fn the_duration_parser_reads_what_it_claims_to() {
    let got = transitions(".a { transition: color 90ms linear, opacity 0.12s ease 300ms; }");
    assert_eq!(
        got.iter().map(|(_, ms)| *ms).collect::<Vec<_>>(),
        vec![90.0, 120.0]
    );
    assert!(transitions(".a { color: red; }").is_empty());
}

/// A transition is brief, and it never names a property that relayouts.
#[test]
fn transitions_are_brief_and_never_geometric() {
    for look in every_look() {
        if look.reduce_motion {
            continue;
        }
        let css = stylesheet(&look);
        let found = transitions(&css);
        assert!(!found.is_empty(), "no transitions at all in a live sheet");
        for (decl, ms) in found {
            for banned in BANNED_PROPERTIES {
                assert!(
                    !decl.split_whitespace().any(|tok| tok == banned),
                    "a transition names {banned}, which relayouts the window \
                     and therefore the pty, on every frame it runs: {decl:?}"
                );
            }
            assert!(
                ms > 0.0 && ms <= MAX_TRANSITION_MS,
                "a {ms}ms transition is outside 0..={MAX_TRANSITION_MS}: {decl:?}"
            );
        }
    }
}

/// Reduced motion removes the transitions rather than zeroing them.
///
/// Zeroing is what the web sheet did, and it left every transition declared
/// with no time to run in: a state a reader of the sheet cannot distinguish
/// from a bug, and one that comes back the moment a duration token is added
/// and left out of the override block.
#[test]
fn reduced_motion_leaves_no_transition_behind() {
    for look in every_look() {
        let css = strip_comments(&stylesheet(&look));
        if look.reduce_motion {
            assert!(
                !css.contains("transition"),
                "a reduced-motion sheet still declares a transition"
            );
        } else {
            assert!(css.contains("transition"), "a live sheet declares none");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Nothing beside the pane changes size
// ═══════════════════════════════════════════════════════════════════════════

/// A strip that shares an axis with the pane occupies the same space in every
/// state.
///
/// A bar under the terminal that appears when it has something to say takes a
/// line away from the pty. The pty resizes, and every agent in every session
/// repaints its whole screen. That is the flickering, and it is a layout
/// property rather than a widget bug: the fix is that the strip is never
/// allowed to have a different height, so there is nothing for a widget to
/// get wrong.
#[test]
fn a_strip_beside_the_pane_never_changes_size() {
    for look in every_look() {
        let css = strip_comments(&stylesheet(&look));
        for (class, token) in RESERVED_STRIPS {
            let found = blocks(&css, class);
            assert!(!found.is_empty(), ".{class} has no rule at all");
            let expected = tokens(&look)
                .into_iter()
                .find(|(n, _)| n == token)
                .map(|(_, v)| v)
                .unwrap_or_else(|| panic!("{token} is not a token"));
            let heights: Vec<&str> = found.iter().filter_map(|b| value(b, "min-height")).collect();
            assert_eq!(
                heights.len(),
                1,
                ".{class} states its height {} times; one state can therefore \
                 disagree with another and the pty resizes when it does",
                heights.len()
            );
            assert_eq!(
                heights[0], expected,
                ".{class} is {} tall and the token says {expected}",
                heights[0]
            );
            for block in &found {
                for property in ["height", "max-height"] {
                    assert_eq!(
                        value(block, property),
                        None,
                        ".{class} sets {property}, which can differ from the \
                         reserved height"
                    );
                }
            }
        }
    }
}

/// An element that fills in when a fact resolves keeps its box while it is
/// empty.
///
/// A branch arrives from git, a time from the daemon, a disposition from the
/// model, and each of them lands after the row is already on screen. An
/// element that is absent until then makes the row reflow under a reader who
/// is in the middle of it. So the empty variant carries the same box as the
/// filled one and differs only in ink.
#[test]
fn a_late_fact_keeps_its_box_while_it_is_empty() {
    for look in every_look() {
        let css = strip_comments(&stylesheet(&look));
        for (class, token) in RESERVED_SLOTS {
            let expected = tokens(&look)
                .into_iter()
                .find(|(n, _)| n == token)
                .map(|(_, v)| v)
                .unwrap_or_else(|| panic!("{token} is not a token"));
            let empty_class = format!("{class}--empty");
            // Read every rule that names the class and take the height from
            // the ones that state it, rather than from whichever rule happens
            // to come first: a class can also appear in a rule that sets
            // something else entirely, such as the gap between two facts on a
            // line, and reading that one reports a missing box that is
            // declared three rules further down. Every rule that does state a
            // height must state the same one, so a second rule cannot
            // contradict the first depending on which the toolkit applies
            // last.
            for (which, name) in [("filled", *class), ("empty", empty_class.as_str())] {
                let all = blocks(&css, name);
                assert!(
                    !all.is_empty(),
                    ".{name} has no rule, so the element is absent until the \
                     fact resolves and the row reflows when it does"
                );
                let stated: Vec<&str> = all.iter().filter_map(|b| value(b, "min-height")).collect();
                assert!(
                    !stated.is_empty(),
                    ".{name} states no height in any of its {} rules, so the \
                     {which} state does not reserve {expected}",
                    all.len()
                );
                for height in stated {
                    assert_eq!(
                        height, expected,
                        ".{class} in its {which} state reserves {height}, not \
                         {expected}"
                    );
                }
            }
            let empty = blocks(&css, &empty_class);
            // The empty state may only take the ink away. Anything else it
            // changed would be a box that differs from the filled one.
            let allowed = ["min-height", "color"];
            for decl in empty[0].split(';') {
                let Some((property, _)) = decl.split_once(':') else {
                    continue;
                };
                let property = property.trim();
                assert!(
                    allowed.contains(&property),
                    ".{class}--empty sets {property}; an empty state may only \
                     drop the ink, never change the box"
                );
            }
            assert_eq!(
                value(empty[0], "color"),
                Some("transparent"),
                ".{class}--empty paints ink into a box that has no content"
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Every preference repaints
// ═══════════════════════════════════════════════════════════════════════════

/// Switching the theme changes what is painted.
#[test]
fn switching_the_theme_repaints() {
    let look = |scheme| Look {
        scheme,
        density: Density::Comfortable,
        text_scale_pct: 100,
        reduce_motion: false,
        hues: None,
        terminal: TEST_TERMINAL,
    };
    let dark = stylesheet(&look(Scheme::Dark));
    let light = stylesheet(&look(Scheme::Light));
    assert_ne!(dark, light);
    assert!(dark.contains(&RAMP[Scheme::Dark as usize].surface.hex()));
    assert!(light.contains(&RAMP[Scheme::Light as usize].surface.hex()));
    assert!(
        !light.contains(&RAMP[Scheme::Dark as usize].surface.hex()),
        "the light sheet still carries the dark surface"
    );
}

/// Switching the density changes the vertical rhythm and nothing else.
#[test]
fn switching_the_density_repaints_the_rhythm_only() {
    let look = |density| Look {
        scheme: Scheme::Dark,
        density,
        text_scale_pct: 100,
        reduce_motion: false,
        hues: None,
        terminal: TEST_TERMINAL,
    };
    let comfortable = tokens(&look(Density::Comfortable));
    let compact = tokens(&look(Density::Compact));
    let of = |set: &[(String, String)], name: &str| {
        set.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .expect("a token")
    };
    assert_ne!(of(&comfortable, "card-h"), of(&compact, "card-h"));
    assert_ne!(of(&comfortable, "row-gap"), of(&compact, "row-gap"));
    // Type is untouched: a density switch that also changed the font size
    // would make the two controls fight.
    for (name, _) in TYPE {
        assert_eq!(of(&comfortable, name), of(&compact, name), "{name} moved");
    }
    assert_ne!(
        stylesheet(&look(Density::Comfortable)),
        stylesheet(&look(Density::Compact))
    );
}

/// Switching the text scale multiplies every length and every font size.
///
/// Every length, which the web sheet did not manage: it scaled the root font
/// size, so the sidebar's `rem` type moved and the fixed pixel values in the
/// window frame did not.
#[test]
fn switching_the_text_scale_repaints_every_length() {
    let look = |pct| Look {
        scheme: Scheme::Dark,
        density: Density::Comfortable,
        text_scale_pct: pct,
        reduce_motion: false,
        hues: None,
        terminal: TEST_TERMINAL,
    };
    let of = |pct: u16, name: &str| -> f64 {
        tokens(&look(pct))
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.trim_end_matches("px").parse::<f64>().expect("a length"))
            .expect("a token")
    };
    for (name, base) in LENGTHS.iter().chain(TYPE) {
        if *name == "radius-full" {
            // A shape, not a measurement.
            assert_eq!(of(200, name), of(100, name));
            continue;
        }
        assert_eq!(
            of(200, name),
            (base * 2.0).round(),
            "{name} did not double at 200%"
        );
    }
    assert_ne!(stylesheet(&look(100)), stylesheet(&look(200)));
}

/// Switching the terminal palette repaints the chrome's hues.
///
/// "The palette ignores my terminal" is the defect. A palette used to reach
/// the cell matrix and nothing else, so an operator who imported their own
/// colours got them inside the grid and the product's blue everywhere a status
/// was stated.
#[test]
fn switching_the_terminal_palette_repaints_the_chrome() {
    let base = Look {
        scheme: Scheme::Dark,
        density: Density::Comfortable,
        text_scale_pct: 100,
        reduce_motion: false,
        hues: None,
        terminal: TEST_TERMINAL,
    };
    let borrowed = Look {
        hues: Some(ChromeHues::from_palette(&fixture(), Scheme::Dark)),
        // The fixture's own background and foreground: with a palette in
        // force the grid clears to them, so the chrome around it must say so.
        terminal: (Rgb(1, 2, 3), Rgb(250, 251, 252)),
        ..base.clone()
    };
    assert_ne!(stylesheet(&base), stylesheet(&borrowed));

    // The letterbox around the grid is the grid's own background, exactly.
    let css = stylesheet(&borrowed);
    let pane = blocks(&css, "rg-pane");
    assert_eq!(
        value(pane[0], "background-color"),
        Some(Rgb(1, 2, 3).hex().as_str()),
        "the pane letterboxes to something other than the grid's background"
    );

    // And the built-in hue is gone from the sheet rather than merely joined.
    assert!(
        !css.contains(&RAMP[Scheme::Dark as usize].failed.hex()),
        "the theme's red is still in the sheet beside the palette's"
    );
}

/// Every built-in palette, on both schemes, yields hues a reader can see.
///
/// A palette tuned for a black terminal supplies colours that vanish on a
/// light chrome. A status nobody can see is worse than a status in the wrong
/// colour, so a borrowed hue is moved toward the text colour until it reads.
#[test]
fn a_borrowed_hue_is_always_legible() {
    for palette in crate::termpalette::ALL {
        let Some(colours) = palette.colours() else {
            continue;
        };
        let mut ansi = [[0u8, 0, 0, 255]; 16];
        for (slot, hex) in ansi.iter_mut().zip(colours.ansi) {
            let rgb = u32::from_str_radix(hex.trim_start_matches('#'), 16).expect("a hex colour");
            *slot = [
                u8::try_from(rgb >> 16 & 0xff).expect("a byte"),
                u8::try_from(rgb >> 8 & 0xff).expect("a byte"),
                u8::try_from(rgb & 0xff).expect("a byte"),
                255,
            ];
        }
        let pane = PanePalette {
            ansi,
            background: [0, 0, 0, 255],
            foreground: [255, 255, 255, 255],
            cursor: [255, 255, 255, 255],
            selection_bg: [0, 0, 0, 255],
            selection_fg: [255, 255, 255, 255],
        };
        for scheme in [Scheme::Dark, Scheme::Light] {
            let hues = ChromeHues::from_palette(&pane, scheme);
            let surface = RAMP[scheme as usize].surface;
            for (what, hue) in [
                ("accent", hues.accent),
                ("working", hues.working),
                ("approval", hues.approval),
                ("input", hues.input),
                ("failed", hues.failed),
                ("done", hues.done),
                ("snoozed", hues.snoozed),
                ("add", hues.add),
                ("del", hues.del),
            ] {
                assert!(
                    hue.contrast(surface) >= MIN_HUE_CONTRAST,
                    "{} on {scheme:?} gives an unreadable {what}: {:.2}:1",
                    palette.slug(),
                    hue.contrast(surface)
                );
            }
        }
    }
}

/// The legibility clamp terminates on a colour that can never reach the floor.
///
/// Mid-grey ink on a mid-grey surface has nowhere to go. A bisection that
/// insisted on the floor would spin, and a stalled repaint is a worse failure
/// than a hue that is merely as good as it can be.
#[test]
fn the_legibility_clamp_terminates_on_a_hopeless_colour() {
    let grey = Rgb(0x80, 0x80, 0x80);
    let out = readable(grey, grey, grey);
    assert_eq!(out, grey, "no step is available, so the input comes back");
}

/// The contrast arithmetic, against values with a published answer.
#[test]
fn contrast_matches_the_published_definition() {
    let white = Rgb(255, 255, 255);
    let black = Rgb(0, 0, 0);
    assert!((white.contrast(black) - 21.0).abs() < 0.01);
    assert!((white.contrast(white) - 1.0).abs() < 0.001);
    assert_eq!(white.mix(black, 1.0), black);
    assert_eq!(white.mix(black, 0.0), white);
    assert_eq!(white.mix(black, 0.5), Rgb(128, 128, 128));
}

// ═══════════════════════════════════════════════════════════════════════════
// The bus
// ═══════════════════════════════════════════════════════════════════════════

/// A publish that changes nothing the sheet reads does not repaint.
///
/// Every control in the settings sheet routes through one commit and one of
/// them is a text field, so a publish arrives per character typed into the
/// daemon URL. Regenerating and reparsing a stylesheet on each of those is the
/// shape of work that made this product feel slow.
#[test]
fn a_publish_that_changes_nothing_does_not_repaint() {
    let _lease = crate::state::live::exclusive();
    let mut settings = Settings::default();
    let shell = ShellSettings::derive(&settings);
    let pane = PaneSettings::derive(&settings);

    assert!(wanted(&shell, &pane), "the first look is always a change");
    assert!(!wanted(&shell, &pane), "the same look asked for a repaint");

    settings.terminal.font_family = "Iosevka".to_string();
    let unread = ShellSettings::derive(&settings);
    assert!(
        !wanted(&unread, &PaneSettings::derive(&settings)),
        "a setting the sheet does not read asked for a repaint"
    );

    settings.density = Density::Compact;
    assert!(
        wanted(
            &ShellSettings::derive(&settings),
            &PaneSettings::derive(&settings)
        ),
        "the density changed and the sheet did not"
    );

    settings.terminal.palette = crate::termpalette::TermPalette::Nord;
    assert!(
        wanted(
            &ShellSettings::derive(&settings),
            &PaneSettings::derive(&settings)
        ),
        "the palette changed and the sheet did not"
    );

    settings.text_scale_pct = 150;
    assert!(
        wanted(
            &ShellSettings::derive(&settings),
            &PaneSettings::derive(&settings)
        ),
        "the text scale changed and the sheet did not"
    );

    settings.reduce_motion = true;
    assert!(
        wanted(
            &ShellSettings::derive(&settings),
            &PaneSettings::derive(&settings)
        ),
        "reduced motion was asked for and the sheet did not change"
    );
}

/// A look folded off the live snapshots reads every preference the sheet
/// depends on, and no other.
#[test]
fn a_look_is_folded_from_the_live_snapshots() {
    let mut settings = Settings::default();
    settings.density = Density::Compact;
    settings.text_scale_pct = 125;
    settings.reduce_motion = true;
    settings.theme = ThemePref::Light;
    settings.terminal.palette = crate::termpalette::TermPalette::Dracula;

    let look = Look::from_live(
        &ShellSettings::derive(&settings),
        &PaneSettings::derive(&settings),
    );
    assert_eq!(look.scheme, Scheme::Light);
    assert_eq!(look.density, Density::Compact);
    assert_eq!(look.text_scale_pct, 125);
    assert!(look.reduce_motion);
    assert!(look.hues.is_some(), "a chosen palette did not reach the chrome");

    settings.terminal.palette = crate::termpalette::TermPalette::Inherit;
    let inherited = Look::from_live(
        &ShellSettings::derive(&settings),
        &PaneSettings::derive(&settings),
    );
    assert!(
        inherited.hues.is_none(),
        "following the app theme still borrowed hues from somewhere"
    );
}

/// The host terminal's own colours reach the chrome, not just the grid.
#[test]
fn the_host_terminals_colours_reach_the_chrome() {
    let mut settings = Settings::default();
    settings.terminal.follow_host_terminal = true;
    let host = &mut settings.terminal.host_palette;
    host.source = crate::state::hostterm::HostSource::Flat;
    host.background = "#101010".to_string();
    host.foreground = "#eeeeee".to_string();
    host.cursor = "#eeeeee".to_string();
    host.selection = "#333333".to_string();
    host.ansi = (0..16)
        .map(|i: u32| format!("#{:02x}00{:02x}", i * 16, 255 - i * 16))
        .collect();
    host.origin = "/src/project/terminal.conf".to_string();
    assert!(
        settings.terminal.host_palette_in_force(),
        "the fixture import is not complete, so the rest of this proves nothing"
    );

    let look = Look::from_live(
        &ShellSettings::derive(&settings),
        &PaneSettings::derive(&settings),
    );
    let hues = look.hues.expect("the import did not reach the chrome");
    assert_eq!(
        look.terminal.0,
        Rgb(0x10, 0x10, 0x10),
        "the pane letterboxes to something other than the imported background"
    );
    assert!(
        stylesheet(&look).contains(&hues.failed.hex()),
        "the imported red is not in the sheet"
    );
}

/// The letterbox token is the colour the grid clears to, for every palette.
///
/// WHY: the chrome used to carry its own pair of terminal colours, so an
/// unconfigured install put `#08080a` in the sheet while the GPU cleared to
/// `#14161c`. That is a seam around the terminal on the default install, and
/// it was invisible to a test that only checked an imported palette, because
/// the imported case was the one branch that read the real colours.
///
/// The variant space is [`crate::termpalette::ALL`] read at run time, so a
/// palette added later is covered without anyone remembering to add it here.
#[test]
fn the_letterbox_is_the_colour_the_grid_clears_to_for_every_palette() {
    for palette in crate::termpalette::ALL {
        let mut settings = Settings::default();
        settings.terminal.palette = palette;
        let pane = PaneSettings::derive(&settings);
        let painted = crate::pane::theme_from(&pane).palette.background;
        let want = Rgb(painted.r, painted.g, painted.b);

        let look = Look::from_live(&ShellSettings::derive(&settings), &pane);
        assert_eq!(
            look.terminal.0,
            want,
            "{} letterboxes to something the grid does not paint",
            palette.slug()
        );
        assert!(
            stylesheet(&look).contains(&want.hex()),
            "{} does not put the grid's own background in the sheet",
            palette.slug()
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// The vocabulary
// ═══════════════════════════════════════════════════════════════════════════

/// Every class the widget modules may set has a rule, and every rule in the
/// sheet is reachable by a class rather than by a GTK node name alone.
///
/// A class with no rule renders with no padding, no colour and no box, which
/// reads as a layout bug rather than as a missing rule. This is the list the
/// widget modules are written against.
#[test]
fn the_class_vocabulary_is_complete_and_named() {
    let named = classes();
    assert!(
        named.len() > 150,
        "only {} classes in the sheet; the surface shrank or the scan broke",
        named.len()
    );
    for class in &named {
        assert!(
            class.starts_with("rg-"),
            "{class} is not in the product's namespace"
        );
        assert!(
            !class.ends_with('-'),
            "{class} is a truncated name, so the scan is cutting them short"
        );
    }
    // The frame's own nine, which every other surface hangs off.
    for class in [
        "rg-root",
        "rg-titlebar",
        "rg-paned",
        "rg-sidebar",
        "rg-content",
        "rg-pane",
        "rg-panebar",
        "rg-scrim",
        "rg-dialog-slot",
    ] {
        assert!(named.contains(&class), "the frame's .{class} has no rule");
    }
    // And the two reserved lists, which are the guards' own subjects.
    for (class, _) in RESERVED_STRIPS.iter().chain(RESERVED_SLOTS) {
        assert!(named.contains(class), ".{class} is reserved but unpainted");
    }
}

/// The generated sheet carries no unresolved marker and no empty declaration.
///
/// An empty value is what a missing token leaves behind, and GTK answers it by
/// dropping the declaration silently.
#[test]
fn the_generated_sheet_has_no_empty_declaration() {
    for look in every_look() {
        let css = strip_comments(&stylesheet(&look));
        for block in css.split('}') {
            for decl in block.split(';') {
                let Some((property, value)) = decl.rsplit('{').next().and_then(|d| d.split_once(':'))
                else {
                    continue;
                };
                assert!(
                    !value.trim().is_empty(),
                    "{} has no value",
                    property.trim()
                );
            }
        }
    }
}

/// GTK itself parses every sheet the generator can produce.
///
/// The guards above read the sheet as text, which cannot tell a property GTK
/// does not implement from one it does. This hands the real parser the real
/// bytes.
///
/// Through the C entry point rather than the Rust wrapper, and that is the
/// whole reason this test can exist. `gtk::CssProvider::new` asserts that
/// `gtk::init` has run, which opens a display, and there is no display on a
/// build machine. The parser itself is pure string work on a plain `GObject`
/// and needs neither. A parse check that only runs where someone is sitting
/// at a screen is a parse check that never runs.
#[cfg(target_os = "linux")]
#[test]
fn gtk_parses_every_sheet_this_can_produce() {
    for look in every_look() {
        let css = stylesheet(&look);
        // SAFETY: `gtk_css_provider_new` returns an owned floating-free
        // `GObject` and `gtk_css_provider_load_from_data` reads `len` bytes
        // from the pointer without retaining it. Both are called with a live
        // buffer and the object is released on every path.
        unsafe {
            let provider = gtk::ffi::gtk_css_provider_new();
            assert!(!provider.is_null(), "GTK would not make a provider");
            let mut error: *mut glib::ffi::GError = std::ptr::null_mut();
            gtk::ffi::gtk_css_provider_load_from_data(
                provider,
                css.as_ptr(),
                isize::try_from(css.len()).expect("a sheet shorter than isize"),
                &raw mut error,
            );
            let refused = (!error.is_null()).then(|| {
                let message = std::ffi::CStr::from_ptr((*error).message)
                    .to_string_lossy()
                    .into_owned();
                glib::ffi::g_error_free(error);
                message
            });
            glib::gobject_ffi::g_object_unref(provider.cast());
            assert_eq!(refused, None, "GTK refused a sheet for {look:?}");
        }
    }
}

/// Every source file under the crate, so a guard can read the whole tree.
///
/// Walked at run time rather than listed, because a list is the thing that
/// stops matching the tree. A widget module added tomorrow is covered without
/// anybody remembering this file exists.
fn every_source() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("the crate's own source tree is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("a source file is UTF-8");
                out.push((path.display().to_string(), text));
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut out,
    );
    out
}

/// No widget may wear a class the sheet does not paint.
///
/// An unpainted class is silent. The widget is built, packed and shown, and
/// it renders with no padding, no colour and no box, which reads as a layout
/// bug rather than a missing rule. The names are read out of the widget
/// source rather than a hand-kept list, so renaming a rule out from under a
/// live `add_class` turns this red instead of shipping.
///
/// What this does not catch: a class assembled at run time, which is passed
/// as a variable rather than a literal. Those are the status, attention and
/// connection families, and each has its own guard beside the function that
/// builds the name.
#[test]
fn no_widget_wears_a_class_the_sheet_does_not_paint() {
    let painted = classes();
    let mut checked = 0usize;
    // Collected rather than asserted one at a time. Stopping at the first
    // name sends whoever added a surface back for another full run per class.
    let mut unpainted: Vec<String> = Vec::new();
    for (name, src) in every_source() {
        for (at, marker) in src.match_indices("add_class(\"") {
            let rest = &src[at + marker.len()..];
            let class = &rest[..rest.find('"').expect("a terminated string literal")];
            checked += 1;
            if !painted.contains(&class) {
                let found = format!("{name} adds .{class}");
                if !unpainted.contains(&found) {
                    unpainted.push(found);
                }
            }
        }
    }
    assert!(
        unpainted.is_empty(),
        "the sheet has no rule for these, so they render with no box at all:\n{}",
        unpainted.join("\n")
    );
    assert!(
        checked > 100,
        "only {checked} classes were read, so the scan broke rather than the widgets"
    );
}

/// Every themed widget kind this tree builds has the stock background image
/// cleared.
///
/// The stock GTK theme paints a control with a gradient `background-image`
/// rather than a colour. A sheet rule that sets only `background-color` leaves
/// that gradient painting over it, so a control declared transparent comes out
/// as a stock light button: near-white on near-white. A scrim hides it, which
/// is why it survives in a dialog and shows in a bare sidebar.
///
/// The kinds are read out of the widget source at run time, so building a
/// `gtk::ComboBoxText` for the first time turns this red until the reset
/// covers it. Only kinds the stock theme actually skins are checked: a
/// `gtk::Box` and a `gtk::Label` have no themed background to inherit.
#[test]
fn the_sheet_clears_the_stock_background_of_every_widget_kind_this_builds() {
    // Constructor to the CSS element name GTK gives the widget.
    const SKINNED: &[(&str, &str)] = &[
        ("gtk::Button", "button"),
        ("gtk::ToggleButton", "button"),
        ("gtk::Entry", "entry"),
        ("gtk::ComboBoxText", "combobox button"),
        ("gtk::Switch", "switch"),
        ("gtk::FlowBox", "flowboxchild"),
        ("gtk::Popover", "popover"),
        ("gtk::Notebook", "notebook"),
        ("gtk::Frame", "frame"),
        ("gtk::HeaderBar", "headerbar"),
        ("gtk::ListBox", "row"),
    ];
    let source: String = every_source()
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join("\n");
    let mut built = Vec::new();
    for (ctor, element) in SKINNED {
        if source.contains(&format!("{ctor}::")) {
            built.push(*element);
        }
    }
    assert!(
        built.len() >= 4,
        "only {} skinned widget kinds were found, so the scan broke rather \
         than the widget tree",
        built.len()
    );
    let css = stylesheet(&every_look()[0]);
    // The selectors of every block that clears the stock image, whichever
    // block that is, so moving the reset rule does not quietly disarm this.
    let mut cleared = String::new();
    for block in css.split('}') {
        let Some((selectors, body)) = block.split_once('{') else {
            continue;
        };
        if body.contains("background-image: none") {
            cleared.push_str(selectors);
            cleared.push(',');
        }
    }
    let missing: Vec<&str> = built
        .into_iter()
        .filter(|element| !cleared.contains(&format!(".rg-root {element}")))
        .collect();
    assert!(
        missing.is_empty(),
        "these widget kinds are built and keep the stock theme's gradient, so \
         a transparent rule paints as a stock light control: {missing:?}"
    );
}
