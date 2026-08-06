//! The pure half of the keyboard page: step addressing and menu vocabulary.
//!
//! The rendering is not asserted here. What is asserted is everything an edit
//! depends on: that a path addresses the list the operator pointed at, that the
//! menus round-trip every value they offer, and that a broken row is described
//! rather than swallowed.

use super::*;

fn nested() -> Vec<Step> {
    vec![
        Step::text("top"),
        Step::when(
            Predicate::SessionFocused,
            vec![
                Step::text("then"),
                Step::when(
                    Predicate::SidebarVisible,
                    vec![Step::text("deep")],
                    Vec::new(),
                ),
            ],
            vec![Step::text("otherwise")],
        ),
    ]
}

/// A path must address the branch it names and no other. Getting this wrong
/// rewrites a step the operator never pointed at, which reads as an edit landing
/// in the wrong place with no way to tell what happened.
#[test]
fn a_path_addresses_the_branch_it_names() {
    let mut steps = nested();

    assert_eq!(list_at(&mut steps, &[]).unwrap().len(), 2);

    let then = list_at(
        &mut steps,
        &[Hop {
            at: 1,
            branch: Branch::Then,
        }],
    )
    .expect("the then branch exists");
    assert_eq!(then[0], Step::text("then"));

    let otherwise = list_at(
        &mut steps,
        &[Hop {
            at: 1,
            branch: Branch::Otherwise,
        }],
    )
    .expect("the otherwise branch exists");
    assert_eq!(otherwise, &vec![Step::text("otherwise")]);

    let deep = list_at(
        &mut steps,
        &[
            Hop {
                at: 1,
                branch: Branch::Then,
            },
            Hop {
                at: 1,
                branch: Branch::Then,
            },
        ],
    )
    .expect("the nested then branch exists");
    assert_eq!(deep, &vec![Step::text("deep")]);

    // And the edit lands where it was addressed.
    deep.push(Step::text("added"));
    let Step::When { then, .. } = &steps[1] else {
        panic!("step 1 is a conditional");
    };
    let Step::When { then: inner, .. } = &then[1] else {
        panic!("the nested step is a conditional");
    };
    assert_eq!(inner, &vec![Step::text("deep"), Step::text("added")]);
}

/// A path that no longer fits drops the edit instead of rewriting whatever now
/// sits at that index. Paths are built during a render and applied on a later
/// event, so the list can have changed in between.
#[test]
fn a_stale_path_addresses_nothing() {
    let mut steps = vec![Step::text("only")];

    assert!(
        list_at(
            &mut steps,
            &[Hop {
                at: 9,
                branch: Branch::Then
            }]
        )
        .is_none(),
        "a path past the end resolved to a list"
    );
    assert!(
        list_at(
            &mut steps,
            &[Hop {
                at: 0,
                branch: Branch::Then
            }]
        )
        .is_none(),
        "a path descended into a step that is not a conditional"
    );
}

/// Every predicate the menu offers has to survive being chosen: the name goes
/// out, comes back, and names the same kind. A mismatch leaves a menu entry that
/// silently resets to something else the moment it is picked.
#[test]
fn every_offered_predicate_round_trips_through_the_menu() {
    for (wire, _) in PREDICATE_KINDS {
        let predicate = predicate_of(wire);
        assert_ne!(
            predicate,
            Predicate::Unknown,
            "{wire} is offered in the menu and does not parse"
        );
        assert_eq!(
            predicate_wire(&predicate),
            *wire,
            "{wire} came back under a different name"
        );
        assert!(
            predicate.holds(&crate::keymap::Facts::default()).is_some(),
            "{wire} is offered and this build cannot answer it, so the step would be inert"
        );
    }
}

/// A name the menu does not offer is `Unknown`, and `Unknown` is not offered.
/// Offering it would let somebody choose a question that can never be answered,
/// which is a binding that saves cleanly and then does nothing.
#[test]
fn the_menu_never_offers_the_unanswerable_predicate() {
    assert_eq!(predicate_of("focused-is-haunted"), Predicate::Unknown);
    assert_eq!(predicate_wire(&Predicate::Unknown), "unknown");
    assert!(
        !PREDICATE_KINDS.iter().any(|(wire, _)| *wire == "unknown"),
        "the menu offers a question no build can answer"
    );
}

/// Every value the three payload menus offer must be a value this build can
/// answer about. An `Unknown` in a menu is a step that renders, saves, and never
/// runs either branch.
#[test]
fn no_payload_menu_offers_an_unknown_value() {
    for status in StatusKind::all() {
        assert_ne!(*status, StatusKind::Unknown);
        assert_eq!(StatusKind::from_wire(status.wire()), *status);
        assert_ne!(status_label(*status), status_label(StatusKind::Unknown));
    }
    for layer in LayerKind::all() {
        assert_ne!(*layer, LayerKind::Unknown);
        assert_eq!(LayerKind::from_wire(layer.wire()), *layer);
        assert_ne!(layer_label(*layer), layer_label(LayerKind::Unknown));
    }
    for attention in AttentionKind::all() {
        assert_ne!(*attention, AttentionKind::Unknown);
        assert_eq!(AttentionKind::from_wire(attention.wire()), *attention);
        assert_ne!(
            attention_label(*attention),
            attention_label(AttentionKind::Unknown)
        );
    }
}

/// A broken row is named by its fault, not by a generic refusal. "Invalid" tells
/// the operator to guess; the escape and its offset tell them where to look.
#[test]
fn a_fault_sentence_names_the_thing_to_fix() {
    let said = fault_sentence(&BindingError::BadEscape {
        at: 4,
        what: "\\q".to_string(),
    });
    assert!(said.contains("\\q"), "the sentence hides the bad escape: {said}");
    assert!(said.contains('4'), "the sentence hides the offset: {said}");

    let chord = fault_sentence(&BindingError::BadChord {
        chord: "Ctrl+".to_string()
    });
    assert!(
        chord.contains("Ctrl+"),
        "the sentence hides the bad chord: {chord}"
    );
}

/// A binding with no name is still addressable in a message. "did nothing" with
/// no subject names nothing the operator can go and fix.
#[test]
fn an_unnamed_binding_is_titled_by_its_chord() {
    let mut binding = CustomBinding {
        label: "   ".to_string(),
        chord: "Ctrl+Shift+G".to_string(),
        steps: Vec::new(),
    };
    assert_eq!(binding.title(), "Ctrl+Shift+G");

    binding.label = "  Interrupt  ".to_string();
    assert_eq!(binding.title(), "Interrupt");
}

/// A recorded keystroke on the top digit row means the digit, not the shifted
/// punctuation. `KeyboardEvent.key` for Ctrl+Shift+1 is `!`, so recording it
/// verbatim stores a chord the keystroke never matches again.
#[test]
fn recording_a_shifted_digit_stores_the_digit() {
    let chord = crate::keymap::chord_from_event("!", "Digit1", true, false, true);
    assert_eq!(chord.key, "1");
    assert!(chord.ctrl && chord.shift && !chord.alt);

    // A letter still comes from `key`, because `code` for it is `KeyK`.
    let letter = crate::keymap::chord_from_event("K", "KeyK", true, false, true);
    assert_eq!(letter.key, "k");

    // A `code` that merely starts with Digit is not one physical key, so it
    // must fall back to `key` rather than half-parse into an unreachable chord.
    assert_eq!(crate::keymap::chord_from_event("x", "Digit", true, false, false).key, "x");
    assert_eq!(crate::keymap::chord_from_event("x", "Digit12", true, false, false).key, "x");
}
