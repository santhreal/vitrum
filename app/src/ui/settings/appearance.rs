//! Translucency and the backdrop image.
//!
//! Three things are worth pinning here and the rest is CSS. The default
//! profile must emit nothing, because that is what keeps the opaque path free
//! of compositing it never asked for. The control lists must contain the value
//! the model actually defaults to, because a `<select>` whose stored value
//! matches no option silently shows the first one instead. And the clamp has
//! to hold on load, because `ui.json` is a file an operator edits by hand.

use crate::state::{
    AppearancePrefs, BACKDROP_BLUR_MAX_PX, BackdropFit, OPACITY_MAX_PCT, OPACITY_MIN_PCT, Settings,
};
use crate::ui::settings::{
    BLUR_STEPS, DIM_STEPS, OPACITY_STEPS, appearance_tokens, backdrop_url, blur_label, opacity_note,
};

/// A default profile emits no appearance tokens at all.
///
/// WHY: this is the performance contract of the whole feature. Every token
/// here turns a solid fill into an `rgba` composite, and an operator who never
/// opened the Appearance tab must get exactly the document they got before it
/// existed. If this fails, translucency has been switched on for everybody.
#[test]
fn a_default_profile_is_not_translucent() {
    let a = AppearancePrefs::default();
    assert_eq!(appearance_tokens(&a), "");
    assert!(!a.needs_transparent_window());
    assert_eq!(a.opacity_pct, OPACITY_MAX_PCT);
    assert_eq!(a.terminal_opacity_pct, OPACITY_MAX_PCT);
    assert!(a.backdrop.is_empty());
    // The whole Settings default has to agree, not just the sub-struct: it is
    // the one that is actually serialised into a fresh `ui.json`.
    assert_eq!(appearance_tokens(&Settings::default().appearance), "");
}

/// Each opacity is independent, and each emits only its own token.
///
/// WHY: the terminal is the surface people want see-through while the shell
/// stays solid. If one slider moved both, the shell would go translucent the
/// moment anyone touched the terminal, and the sidebar text would be sitting
/// on the wallpaper.
#[test]
fn the_two_opacities_are_independent() {
    let chrome = AppearancePrefs {
        opacity_pct: 80,
        ..Default::default()
    };
    let tokens = appearance_tokens(&chrome);
    assert!(tokens.contains("--rg-opacity:0.8;"), "{tokens}");
    assert!(!tokens.contains("--rg-terminal-opacity"), "{tokens}");

    let grid = AppearancePrefs {
        terminal_opacity_pct: 60,
        ..Default::default()
    };
    let tokens = appearance_tokens(&grid);
    assert!(tokens.contains("--rg-terminal-opacity:0.6;"), "{tokens}");
    assert!(!tokens.contains("--rg-opacity:"), "{tokens}");

    // Either one alone is enough to need a see-through window.
    assert!(chrome.needs_transparent_window());
    assert!(grid.needs_transparent_window());
}

/// The clamp holds the floor, the ceiling and the blur limit.
///
/// WHY: `ui.json` is a plain file. `opacity_pct: 0` in it is a window that is
/// invisible, including the Appearance tab holding the control that would undo
/// it, so the floor is the difference between a bad setting and an install the
/// operator has to repair with a text editor.
#[test]
fn the_clamp_survives_a_hand_edited_file() {
    let mut a = AppearancePrefs {
        opacity_pct: 0,
        terminal_opacity_pct: 200,
        backdrop_blur_px: 255,
        backdrop_dim_pct: 200,
        ..Default::default()
    };
    a.clamp();
    assert_eq!(a.opacity_pct, OPACITY_MIN_PCT);
    assert_eq!(a.terminal_opacity_pct, OPACITY_MAX_PCT);
    assert_eq!(a.backdrop_blur_px, BACKDROP_BLUR_MAX_PX);
    assert_eq!(a.backdrop_dim_pct, 100);

    // Clamping is idempotent: a second pass over clamped values changes none.
    let once = a.clone();
    a.clamp();
    assert_eq!(a, once);

    // A legal profile is left alone.
    let mut legal = AppearancePrefs {
        opacity_pct: 85,
        backdrop_blur_px: 16,
        ..Default::default()
    };
    let before = legal.clone();
    legal.clamp();
    assert_eq!(legal, before);
}

/// Every control list contains the value the model defaults to.
///
/// WHY: a `<select>` whose `value` matches no `<option>` does not report an
/// error, it displays the first option. A default install would then read as
/// 100% opaque while the stored value said something else, or the reverse.
/// This is the failure mode the text-scale control was already bitten by.
#[test]
fn every_control_offers_the_default() {
    let a = AppearancePrefs::default();
    assert!(OPACITY_STEPS.contains(&a.opacity_pct));
    assert!(OPACITY_STEPS.contains(&a.terminal_opacity_pct));
    assert!(BLUR_STEPS.contains(&a.backdrop_blur_px));
    assert!(DIM_STEPS.contains(&a.backdrop_dim_pct));

    // The opaque value must be FIRST in the opacity list, because that is the
    // one a mismatch silently falls back to. Falling back to opaque is safe;
    // falling back to 20% is an install nobody can read.
    assert_eq!(OPACITY_STEPS[0], OPACITY_MAX_PCT);
}

/// No control offers a value its own clamp would reject.
///
/// WHY: an option the operator can pick and the loader then rewrites is a
/// control that visibly snaps back, which reads as a bug in the settings sheet
/// rather than as a deliberate limit.
#[test]
fn no_control_offers_a_value_the_clamp_rejects() {
    for pct in OPACITY_STEPS {
        let mut a = AppearancePrefs {
            opacity_pct: pct,
            terminal_opacity_pct: pct,
            ..Default::default()
        };
        a.clamp();
        assert_eq!(
            a.opacity_pct, pct,
            "opacity step {pct} does not survive the clamp"
        );
        assert_eq!(a.terminal_opacity_pct, pct);
    }
    for px in BLUR_STEPS {
        let mut a = AppearancePrefs {
            backdrop_blur_px: px,
            ..Default::default()
        };
        a.clamp();
        assert_eq!(
            a.backdrop_blur_px, px,
            "blur step {px} does not survive the clamp"
        );
    }
    for pct in DIM_STEPS {
        let mut a = AppearancePrefs {
            backdrop_dim_pct: pct,
            ..Default::default()
        };
        a.clamp();
        assert_eq!(
            a.backdrop_dim_pct, pct,
            "dim step {pct} does not survive the clamp"
        );
    }
}

/// The steps are ordered and free of duplicates.
///
/// WHY: a duplicated step is two identical rows in the list, and an unordered
/// one makes the control read as a random pile. Cheap to assert, and the kind
/// of thing that rots the moment someone inserts a value by hand.
#[test]
fn the_steps_are_ordered() {
    assert!(
        OPACITY_STEPS.windows(2).all(|w| w[0] > w[1]),
        "opacity runs high to low"
    );
    assert!(
        BLUR_STEPS.windows(2).all(|w| w[0] < w[1]),
        "blur runs low to high"
    );
    assert!(
        DIM_STEPS.windows(2).all(|w| w[0] < w[1]),
        "dim runs low to high"
    );
    assert!(*OPACITY_STEPS.last().expect("non-empty") >= OPACITY_MIN_PCT);
    assert!(*BLUR_STEPS.last().expect("non-empty") <= BACKDROP_BLUR_MAX_PX);
}

/// A backdrop emits a URL, a size, a repeat and nothing that is switched off.
///
/// WHY: the fit is two CSS properties derived from one enum, and getting the
/// pair wrong is how `tile` ends up stretched. Pinning the pair per variant is
/// the only way that stays true when a variant is added.
#[test]
fn a_backdrop_emits_its_url_and_its_fit() {
    let a = AppearancePrefs {
        backdrop: "/home/you/wall.png".to_string(),
        backdrop_fit: BackdropFit::Cover,
        ..Default::default()
    };
    let tokens = appearance_tokens(&a);
    assert!(tokens.contains("--rg-backdrop:url("), "{tokens}");
    assert!(
        tokens.contains("vitrum-backdrop://local/home/you/wall.png"),
        "{tokens}"
    );
    // Blur and dim are zero here, so neither is emitted.
    assert!(!tokens.contains("--rg-backdrop-blur"), "{tokens}");
    assert!(!tokens.contains("--rg-backdrop-dim"), "{tokens}");

    for (fit, size, repeat) in [
        (BackdropFit::Cover, "cover", "no-repeat"),
        (BackdropFit::Contain, "contain", "no-repeat"),
        (BackdropFit::Tile, "auto", "repeat"),
        (BackdropFit::Center, "auto", "no-repeat"),
    ] {
        let a = AppearancePrefs {
            backdrop: "/w.png".to_string(),
            backdrop_fit: fit,
            ..Default::default()
        };
        let tokens = appearance_tokens(&a);
        assert!(
            tokens.contains(&format!("--rg-backdrop-size:{size};")),
            "{fit:?} wanted size {size}, got {tokens}"
        );
        assert!(
            tokens.contains(&format!("--rg-backdrop-repeat:{repeat};")),
            "{fit:?} wanted repeat {repeat}, got {tokens}"
        );
    }
}

/// Blur and dim reach CSS in the units CSS expects.
///
/// WHY: a blur emitted as a bare number is an invalid `filter` and silently
/// does nothing, and a dim emitted as a percent where an alpha is wanted is a
/// scrim that is either invisible or opaque. Both fail quietly.
#[test]
fn the_blur_and_dim_carry_their_units() {
    let a = AppearancePrefs {
        backdrop: "/w.png".to_string(),
        backdrop_blur_px: 24,
        backdrop_dim_pct: 40,
        ..Default::default()
    };
    let tokens = appearance_tokens(&a);
    assert!(tokens.contains("--rg-backdrop-blur:24px;"), "{tokens}");
    assert!(tokens.contains("--rg-backdrop-dim:0.4;"), "{tokens}");
}

/// A backdrop path is percent-encoded into the URL.
///
/// WHY: an unencoded space ends the `url()` token and the whole declaration is
/// dropped, so a wallpaper in `~/My Pictures` would silently never appear.
/// Quotes and parentheses are the injection-shaped version of the same bug.
#[test]
fn a_backdrop_url_is_encoded() {
    assert_eq!(backdrop_url("/a/b.png"), "vitrum-backdrop://local/a/b.png");

    let spaced = backdrop_url("/My Pictures/w.png");
    assert!(!spaced.contains(' '), "{spaced}");
    assert!(spaced.contains("%20"), "{spaced}");

    // Nothing that could close the `url()` or the attribute survives raw.
    let hostile = backdrop_url("/a\")/x;background:red;\".png");
    for bad in ['"', '(', ')', ';', '\''] {
        assert!(!hostile.contains(bad), "{bad} survived encoding: {hostile}");
    }
}

/// The opacity note changes with the state it describes and never oversells.
///
/// WHY: the control cannot deliver blur, and a note that implies it can is the
/// difference between a documented limit and a bug report. Both branches must
/// say so, and the opaque branch must additionally warn that the first change
/// needs a new window.
#[test]
fn the_opacity_note_states_the_limit() {
    let opaque = opacity_note(&AppearancePrefs::default());
    let clear = opacity_note(&AppearancePrefs {
        opacity_pct: 80,
        ..Default::default()
    });
    assert_ne!(opaque, clear);
    for note in [opaque, clear] {
        assert!(
            note.contains("unblurred"),
            "the note must not imply we blur: {note}"
        );
        assert!(
            note.contains("compositor"),
            "the note must name who does blur: {note}"
        );
    }
    assert!(opaque.contains("next window"), "{opaque}");
}

/// Blur steps are labelled, and zero reads as words rather than as `0px`.
#[test]
fn the_blur_labels_read_as_english() {
    assert_eq!(blur_label(0), "None");
    assert_eq!(blur_label(16), "16px");
}

/// The stylesheet a default profile actually gets.
fn backdrop_css() -> &'static str {
    include_str!("../../../assets/parts/23-backdrop.css")
}

/// WHY: a default profile generated two full-viewport composited layers, one
/// of them carrying a `filter`, for a wallpaper it did not have.
///
/// `a_default_profile_is_not_translucent` above asserted that this function
/// emits no tokens, and that assertion was true and did not mean what the file
/// claimed it meant. `.rg-app::before` and `::after` were `content: ""`
/// unconditionally, so both pseudo-elements were generated on every install
/// whatever the tokens said. `filter: blur(0px)` is not a no-op to the
/// compositor: any `filter` other than `none` makes the element a containing
/// block, a stacking context and its own composited layer.
///
/// The gate is `content`, because `content: none` is the one value that stops
/// a pseudo-element from being generated at all rather than merely painting
/// nothing. So the token has to be absent by default AND the stylesheet has to
/// default it to `none`; either half alone leaves the layer.
#[test]
fn a_default_profile_generates_neither_backdrop_layer() {
    let tokens = appearance_tokens(&AppearancePrefs::default());
    assert!(!tokens.contains("--rg-backdrop-layer"), "{tokens}");
    assert!(!tokens.contains("--rg-scrim-layer"), "{tokens}");

    let css = backdrop_css();
    assert!(css.contains("--rg-backdrop-layer: none;"), "{css}");
    assert!(css.contains("--rg-scrim-layer: none;"), "{css}");
    assert!(
        css.contains("content: var(--rg-backdrop-layer);"),
        "the image layer is generated unconditionally again"
    );
    assert!(
        css.contains("content: var(--rg-scrim-layer);"),
        "the scrim layer is generated unconditionally again"
    );
    assert!(
        !css.contains("content: \"\";"),
        "a pseudo-element is back to an unconditional `content`"
    );
}

/// A backdrop switches its layer on, and the scrim waits for a dim.
///
/// WHY: the two layers are independent. A wallpaper with no scrim must not pay
/// for a second viewport-sized box, and a dim with no wallpaper has nothing to
/// darken, so neither token may be emitted on its own.
#[test]
fn each_backdrop_layer_appears_only_when_it_would_paint() {
    let image = AppearancePrefs {
        backdrop: "/tmp/wall.png".to_string(),
        ..Default::default()
    };
    let tokens = appearance_tokens(&image);
    assert!(tokens.contains("--rg-backdrop-layer:\"\";"), "{tokens}");
    assert!(!tokens.contains("--rg-scrim-layer"), "{tokens}");

    let dimmed = AppearancePrefs {
        backdrop_dim_pct: 40,
        ..image.clone()
    };
    let tokens = appearance_tokens(&dimmed);
    assert!(tokens.contains("--rg-scrim-layer:\"\";"), "{tokens}");
    assert!(tokens.contains("--rg-backdrop-dim:0.4;"), "{tokens}");

    // A dim with no image is not a scrim, it is a grey window.
    let orphan = AppearancePrefs {
        backdrop_dim_pct: 40,
        ..Default::default()
    };
    let tokens = appearance_tokens(&orphan);
    assert!(!tokens.contains("--rg-scrim-layer"), "{tokens}");
    assert!(!tokens.contains("--rg-backdrop-dim"), "{tokens}");
}

/// WHY: every menu and every tooltip in an opaque window ran a
/// `backdrop-filter`.
///
/// The rule's own comment said "only at the point where the desktop actually
/// shows through", and nothing gated it: the `@supports` block applied it to
/// `.rg-sheet`, `.rg-menu` and `.rg-tooltip` on every install. That forces the
/// engine to read back everything behind the element and Gaussian-blur it,
/// which behind a fully opaque surface produces a pixel-identical result at
/// the cost of a composited readback per popup.
#[test]
fn surfaces_blur_their_backdrop_only_below_full_opacity() {
    assert!(
        !appearance_tokens(&AppearancePrefs::default()).contains("--rg-surface-blur"),
        "an opaque profile still asks for a backdrop blur"
    );
    let translucent = AppearancePrefs {
        opacity_pct: 80,
        ..Default::default()
    };
    assert!(
        appearance_tokens(&translucent).contains("--rg-surface-blur:blur("),
        "a see-through window lost the blur that keeps its text legible"
    );
    // The terminal's own opacity is not the window's: the desktop does not
    // show through a solid shell because the grid inside it is see-through.
    let grid_only = AppearancePrefs {
        terminal_opacity_pct: 60,
        ..Default::default()
    };
    assert!(
        !appearance_tokens(&grid_only).contains("--rg-surface-blur"),
        "a translucent grid switched on blurring for every menu in the shell"
    );

    let css = backdrop_css();
    assert!(css.contains("--rg-surface-blur: none;"), "{css}");
    assert!(
        css.contains("backdrop-filter: var(--rg-surface-blur);"),
        "the surface blur is unconditional again"
    );
}
