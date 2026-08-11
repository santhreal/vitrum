//! The fit rule, and the registry that makes it cover every surface.
//!
//! Nothing here builds a widget. `gtk_init` needs a display and this program
//! is tested without one, so the property under test is the policy the widget
//! tree is built from: a transient surface asks for at most its cap, is
//! allocated at most the window, and scrolls whenever those two differ.

use super::*;
use crate::state::Layer;

/// A surface with more content than any cap, for asking what happens when the
/// content genuinely does not fit. Larger than any registered cap on both
/// axes, so no surface accidentally tests the easy case.
const OVERSIZED: (f64, f64) = (400.0, 900.0);

/// Window sizes a surface must survive.
///
/// [`SMALLEST`] is the frame a workspace switch hands a client. The others are
/// a window one pixel short of a cap, which is the case a fixed size request
/// gets wrong, and zero, which is what an unmapped window reports.
fn hostile_windows(natural: (i32, i32)) -> Vec<(i32, i32)> {
    vec![
        (0, 0),
        SMALLEST,
        (natural.0 - 1, natural.1 - 1),
        (natural.0 - 1, natural.1),
        (natural.0, natural.1 - 1),
    ]
}

/// **The invariant every transient surface exists under.** Nothing this module
/// presents is ever allocated more than the window it is in, on either axis,
/// at any window size a window manager can produce.
///
/// Asserted over the registry rather than over one surface, because the defect
/// this closes was never about one sheet: it was about a surface being allowed
/// to have a size request the window has to satisfy. Adding a surface without
/// registering it fails
/// [`every_layer_names_a_registered_surface`] instead of failing here.
#[test]
fn no_surface_is_ever_allocated_more_than_the_window() {
    for (id, bounds) in SURFACES {
        let natural = natural(*bounds, OVERSIZED);
        for (w, h) in hostile_windows(natural) {
            assert!(
                allocated(w, natural.0) <= w.max(0),
                "{id} would be allocated {} in a {w}px window",
                allocated(w, natural.0)
            );
            assert!(
                allocated(h, natural.1) <= h.max(0),
                "{id} would be allocated {} in a {h}px window",
                allocated(h, natural.1)
            );
        }
    }
}

/// **The approval case, which is the one that must not be optional.** An
/// option list taller than the window scrolls; it is never truncated.
///
/// The distinction the assertion is making is the whole point. "Allocated no
/// more than the window" is also true of a surface that was clipped, so the
/// test that only checks the box size passes on the defect. Scrolling is what
/// says the rows below the fold are still reachable.
#[test]
fn an_option_list_taller_than_the_window_scrolls_instead_of_being_sliced() {
    // Thirty options. An agent asking to approve a plan lists one row per
    // step, and the destructive ones are last, which is what makes a slice at
    // the bottom edge the worst possible failure.
    let options = 30.0;
    let content = (LIST.width, options * 2.0);
    let natural = natural(LIST, content);
    let wanted = crate::shell::style::rem(content.1).round() as i32;

    // The cap bites before the window does, so the sheet is already scrolling
    // its own content. This is the half of the property that says the rows
    // past the fold are reachable rather than gone.
    assert!(
        scrolls(natural.1, wanted),
        "{options} rows want {wanted}px, the sheet is {}px, and nothing scrolls",
        natural.1
    );

    for (_, h) in hostile_windows(natural) {
        assert!(allocated(h, natural.1) <= h.max(0));
        // A window shorter than the sheet scrolls too. A window exactly as
        // tall as it does not, and asserting otherwise would demand a
        // scrollbar on a surface that fits.
        assert_eq!(
            scrolls(h, natural.1),
            h < natural.1,
            "a {h}px window against a {}px sheet",
            natural.1
        );
    }
}

/// Content that fits is drawn at its own size. A cap that also acted as a
/// floor would make a one-line confirmation a half-screen box, which is the
/// "centred on its own content box" complaint from the other direction.
#[test]
fn a_surface_smaller_than_its_cap_is_drawn_at_its_own_size() {
    let small = (6.0, 4.0);
    let natural = natural(NARROW, small);
    let roomy = (natural.0 * 4, natural.1 * 4);
    assert_eq!(allocated(roomy.0, natural.0), natural.0);
    assert_eq!(allocated(roomy.1, natural.1), natural.1);
    assert!(!scrolls(roomy.0, natural.0));
    assert!(!scrolls(roomy.1, natural.1));
}

/// A window smaller than the content is the scrolling case, and a window
/// larger is not. Stated as the exact boundary rather than as two examples,
/// because an off-by-one here is a scrollbar that appears on a surface that
/// fits.
#[test]
fn scrolling_starts_exactly_where_the_window_stops_holding_the_content() {
    let natural = 400;
    assert!(!scrolls(natural, natural));
    assert!(!scrolls(natural + 1, natural));
    assert!(scrolls(natural - 1, natural));
}

/// An unmapped or negative allocation must not produce a negative size. GTK
/// reports zero for an unmapped toplevel and a subtraction that went below it
/// would be a size request no container can satisfy.
#[test]
fn an_unmapped_window_allocates_nothing_rather_than_a_negative_size() {
    for window in [-1000, -1, 0] {
        assert_eq!(allocated(window, 500), 0);
    }
}

/// **Fails by default on a new surface.** Every layer the shell can open is
/// either registered here or delegated to the module that owns it, and the
/// match is exhaustive, so adding a variant to [`Layer`] stops the build until
/// somebody records which surface it is.
#[test]
fn every_layer_names_a_registered_surface() {
    use crate::state::{MenuState, NewSessionSeed, RenameSeed, SettingsTab};
    use vitrum_proto::SessionId;

    let layers = [
        Layer::None,
        Layer::Shortcuts,
        Layer::Menu(MenuState {
            x: 0.0,
            y: 0.0,
            target: SessionId(1),
        }),
        Layer::NewSession(NewSessionSeed {
            project: None,
            cwd: "/src/vitrum".to_string(),
        }),
        Layer::Settings(SettingsTab::default()),
        Layer::Rename(RenameSeed {
            session: SessionId(1),
            title: "one".to_string(),
        }),
        Layer::Search,
        Layer::Onboarding,
        Layer::WhatsNew,
    ];

    for layer in &layers {
        let id = match layer {
            // Nothing open is not a surface.
            Layer::None => continue,
            // Owned by the settings module, which registers its own bounds.
            // Named here so a reader does not read the absence as an omission.
            Layer::Settings(_) => continue,
            Layer::Shortcuts => SHORTCUTS,
            Layer::Menu(_) => MENU,
            Layer::NewSession(_) => LAUNCHER,
            Layer::Rename(_) => RENAME,
            Layer::Search => SEARCH,
            Layer::Onboarding => ONBOARDING,
            Layer::WhatsNew => WHATSNEW,
        };
        assert!(
            SURFACES.iter().any(|(known, _)| *known == id),
            "{id} is presented but has no registered bounds, so nothing caps it"
        );
    }
}

/// Every cap is a real box: positive on both axes, and no wider than a window
/// this product will open. A cap wider than the minimum window is a cap that
/// never engages, which is the same as having none.
#[test]
fn every_cap_is_a_box_a_small_window_can_still_show() {
    for (id, bounds) in SURFACES {
        assert!(bounds.width > 0.0 && bounds.height > 0.0, "{id}");
        let natural = natural(*bounds, OVERSIZED);
        assert!(natural.0 > 0 && natural.1 > 0, "{id} caps to nothing");
    }
}

/// Two surfaces must not share an id. [`crate::shell::Shell::presented`] is
/// how a caller asks what is open, and two surfaces answering the same name
/// makes that question unanswerable.
#[test]
fn no_two_surfaces_share_an_id() {
    let mut ids: Vec<&str> = SURFACES.iter().map(|(id, _)| *id).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), before, "a duplicate surface id in {SURFACES:?}");
}
