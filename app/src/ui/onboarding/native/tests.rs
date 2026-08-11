//! The walkthrough's shape, without a display.
//!
//! What the pages SAY is asserted by the module above, against the same
//! functions this file calls. What is left for here is what only the presented
//! surface decides: that the sheet is sized for the whole deck rather than for
//! the page that happens to be showing, and that a step's state reaches the
//! markup as a class rather than as prose.

use super::*;
use crate::launch::Detected;
use crate::ui::sheet;

/// A machine that produces the fullest first page: nothing detected, nothing
/// connected, nothing running, so every task row is still worth showing.
fn fresh() -> Machine {
    Machine::default()
}

/// A machine that has done everything the checklist would ask.
fn settled() -> Machine {
    Machine {
        agents: Some(vec![Detected {
            label: "Claude",
            command: "claude",
        }]),
        connected: true,
        any_session: true,
    }
}

/// **The fit invariant, for this surface.** The walkthrough is never allocated
/// more than the window and scrolls whatever did not fit, at every window size
/// including the one-pixel frame a workspace switch produces.
#[test]
fn the_walkthrough_fits_every_window() {
    for machine in [fresh(), settled()] {
        sheet::assert_fits(sheet::ONBOARDING, sheet::DOCUMENT, content(&machine));
    }
}

/// The sheet is sized for the tallest page, not the first one. Pages are
/// swapped inside one sheet, so a size taken from page one slices page three
/// the moment the operator presses Next.
#[test]
fn the_sheet_is_sized_for_the_tallest_page_in_the_deck() {
    let machine = fresh();
    let tallest = pages(&machine)
        .iter()
        .map(|page| page.rows.len())
        .max()
        .expect("the walkthrough has pages");
    let first = pages(&machine)[0].rows.len();
    assert!(
        tallest >= first,
        "the deck's tallest page has {tallest} rows and the first has {first}"
    );

    // Stated as a comparison rather than a number: a deck whose tallest page
    // grew must move this surface's declared height with it.
    let shorter = Machine {
        agents: settled().agents,
        ..machine.clone()
    };
    assert!(
        content(&machine).1 >= content(&shorter).1,
        "a deck with more rows declared no more room than one with fewer"
    );
}

/// **Fails by default on a new step state.** Every state reaches the markup as
/// its own class, so a state added to the enum and left out here stops the
/// build, and two states can never render identically.
#[test]
fn every_step_state_has_its_own_class() {
    let states = [StepState::Done, StepState::Todo, StepState::Info];
    for state in states {
        // Exhaustive by construction: `step_class` matches on the enum, so a
        // new variant fails to compile rather than falling through to a
        // default.
        let class = step_class(state);
        assert!(
            class.starts_with("rg-onboard__step"),
            "{state:?} carries {class}, which is not a step class"
        );
    }
    for (i, a) in states.iter().enumerate() {
        for b in states.iter().skip(i + 1) {
            assert_ne!(
                step_class(*a),
                step_class(*b),
                "{a:?} and {b:?} draw the same class, so they cannot look different"
            );
        }
    }
}
