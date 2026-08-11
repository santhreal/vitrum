//! Translucency and the backdrop image.
//!
//! Three things are worth pinning here and the rest is CSS. The default
//! profile must emit nothing, because that is what keeps the opaque path free
//! of compositing it never asked for. The control lists must contain the value
//! the model actually defaults to, because a `<select>` whose stored value
//! matches no option silently shows the first one instead. And the clamp has
//! to hold on load, because `ui.json` is a file an operator edits by hand.

use crate::state::{
    AppearancePrefs, BACKDROP_BLUR_MAX_PX, OPACITY_MAX_PCT, OPACITY_MIN_PCT,
};
use crate::ui::settings::{BLUR_STEPS, DIM_STEPS, OPACITY_STEPS, blur_label, opacity_note};



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




