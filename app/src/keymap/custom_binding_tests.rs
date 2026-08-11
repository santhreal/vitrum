//! The collection half of custom bindings: persistence, lookup, precedence.
//!
//! [`super::binding_tests`] covers one binding's planner. These cover the set:
//! which binding a chord resolves to, what happens to the others when one is
//! broken, and what survives a trip through the settings file.

use super::*;

/// A window with nothing raised and no session focused.
fn bare() -> Facts {
    Facts::default()
}

fn binding(label: &str, chord: &str, steps: Vec<Step>) -> CustomBinding {
    CustomBinding {
        label: label.to_string(),
        chord: chord.to_string(),
        steps,
    }
}

/// The built-in chord for one action, as the default table binds it.
fn builtin(action: KeyAction) -> Chord {
    *CHORDS
        .iter()
        .find(|chord| chord.action == action)
        .expect("every action in this file has a built-in chord")
}

/// A custom binding must take a chord away from the built-in action that owns
/// it. Without this the operator can save a binding on Ctrl+B, see it listed,
/// and watch the sidebar toggle every time they press it: a shortcut the
/// product advertises and never fires.
#[test]
fn a_custom_binding_shadows_the_builtin_chord_it_names() {
    let owners: Vec<KeyAction> = CHORDS
        .iter()
        .filter(|chord| chord.key == "b" && chord.ctrl && !chord.alt)
        .map(|chord| chord.action)
        .collect();
    assert_eq!(
        owners,
        vec![KeyAction::ToggleSidebar],
        "Ctrl+B is not the sidebar toggle's chord, so there is nothing to shadow"
    );
    let toggle = builtin(KeyAction::ToggleSidebar);

    let bindings = CustomBindings::from(vec![binding(
        "Interrupt",
        &toggle.rendered(),
        vec![Step::text("\\x03")],
    )]);

    let found = bindings
        .shadowing(toggle.key, toggle.ctrl, toggle.alt, toggle.shift)
        .expect("the operator's binding must win the chord");
    assert_eq!(found.label, "Interrupt");
    assert_eq!(
        found.plan(&bare()).unwrap(),
        vec![Effect::Text(vec![0x03])],
        "the shadowing binding must perform its own steps, not the built-in"
    );

    // A chord the operator did not bind is left alone, so shadowing cannot
    // swallow the rest of the keyboard.
    let next = builtin(KeyAction::NextTab);
    assert!(
        bindings
            .shadowing(next.key, next.ctrl, next.alt, next.shift)
            .is_none(),
        "an unrelated built-in chord was captured"
    );
}

/// `Shift::Any` entries fire with shift either way, so both spellings of the
/// chord have to shadow one. Matching only the unshifted form left punctuation
/// chords impossible to rebind, with nothing on screen saying why.
#[test]
fn either_spelling_shadows_a_shift_any_chord() {
    let bindings = CustomBindings::from(vec![binding(
        "Shifted",
        "Ctrl+Shift+G",
        vec![Step::text("x")],
    )]);
    assert!(
        bindings.shadowing("g", true, false, Shift::Any).is_some(),
        "a shifted binding did not shadow a Shift::Any chord"
    );
    assert!(
        bindings.shadowing("g", true, false, Shift::Off).is_none(),
        "a shifted binding must not shadow a chord that requires shift up"
    );
}

/// A `when` whose predicate is false and whose `otherwise` is empty must do
/// NOTHING. The tempting bug is to treat "no branch to run" as "run the then
/// branch anyway", which fires the operator's interrupt at an agent that is
/// mid-answer.
#[test]
fn a_false_conditional_with_no_else_performs_nothing() {
    let bind = binding(
        "Interrupt if working",
        "Ctrl+Shift+G",
        vec![Step::when(
            Predicate::FocusedStatus {
                status: StatusKind::Working,
            },
            vec![Step::text("\\x03")],
            Vec::new(),
        )],
    );

    // Nothing focused, so the status predicate is false.
    assert_eq!(bind.plan(&bare()).unwrap(), Vec::new());

    // And the same binding does act once the state matches, or the assertion
    // above would pass for a planner that never runs anything at all.
    let working = Facts {
        focused: Some(FocusedSession {
            status: StatusKind::Working,
            unread: false,
            command: "claude".to_string(),
        }),
        ..bare()
    };
    assert_eq!(bind.plan(&working).unwrap(), vec![Effect::Text(vec![0x03])]);
}

/// A predicate this build cannot answer runs NEITHER branch. Falling through to
/// `otherwise` would be a guess, and the operator who wrote the binding on a
/// newer build meant one specific side: running the other one is worse than
/// doing nothing.
#[test]
fn an_unknown_predicate_runs_neither_branch() {
    let raw = r#"[{
        "label": "From a newer build",
        "chord": "Ctrl+Shift+G",
        "steps": [{
            "step": "when",
            "predicate": {"kind": "focused-is-haunted"},
            "then": [{"step": "text", "text": "then"}],
            "otherwise": [{"step": "text", "text": "else"}]
        }]
    }]"#;
    let bindings: CustomBindings = serde_json::from_str(raw).expect("unknown names load");
    let bind = &bindings.all()[0];

    assert_eq!(
        bind.steps,
        vec![Step::when(
            Predicate::Unknown,
            vec![Step::text("then")],
            vec![Step::text("else")],
        )],
        "the unknown predicate did not survive as Unknown"
    );
    assert_eq!(
        bind.plan(&bare()).unwrap(),
        Vec::new(),
        "an unanswerable question ran a branch"
    );
    assert_eq!(
        bind.validate(),
        Ok(()),
        "an unknown predicate is not a broken binding, only an inert one"
    );
}

/// One binding with a bad escape is inert on its own and takes nothing else with
/// it. Both halves matter: a half-decoded escape reaching a pty runs whatever
/// the arrived bytes happen to mean, and refusing the whole set would delete
/// every working binding along with the broken one.
#[test]
fn a_malformed_escape_makes_one_binding_inert_and_leaves_the_rest() {
    let bindings = CustomBindings::from(vec![
        binding("Good", "Ctrl+Shift+G", vec![Step::text("ls\\r")]),
        binding(
            "Broken",
            "Ctrl+Shift+H",
            vec![Step::text("safe"), Step::text("oops\\q")],
        ),
        binding("Also good", "Ctrl+Shift+J", vec![Step::text("\\e")]),
    ]);

    assert_eq!(
        bindings.errors(),
        vec![(
            1,
            BindingError::BadEscape {
                at: 4,
                what: "\\q".to_string()
            }
        )],
        "the broken binding must be reported by position, and only it"
    );

    let broken = bindings
        .lookup(&crate::launch::parse_chord("Ctrl+Shift+H").unwrap())
        .expect("a broken binding still owns its chord");
    assert!(
        broken.plan(&bare()).is_err(),
        "a bad escape must refuse the binding rather than send the bytes before it"
    );

    // The good ones are untouched, which is what "one bad binding must not lose
    // the others" means at the point of use rather than at the point of load.
    for (chord, want) in [
        ("Ctrl+Shift+G", Effect::Text(b"ls\r".to_vec())),
        ("Ctrl+Shift+J", Effect::Text(vec![0x1b])),
    ] {
        let found = bindings
            .lookup(&crate::launch::parse_chord(chord).unwrap())
            .expect("a working binding is still bound");
        assert_eq!(found.plan(&bare()).unwrap(), vec![want]);
    }
}

/// Everything an operator can build has to survive the settings file. A field
/// that serialises and does not read back is a binding that works until the app
/// restarts, which is the hardest kind of loss to attribute.
#[test]
fn the_json_form_round_trips_every_kind_of_step() {
    let before = CustomBindings::from(vec![
        binding(
            "Interrupt then rerun",
            "Ctrl+Shift+G",
            vec![
                Step::text("\\x03"),
                Step::action(KeyAction::ToggleSidebar),
                Step::when(
                    Predicate::FocusedCommandContains {
                        text: "claude".to_string(),
                    },
                    vec![Step::when(
                        Predicate::WorkspaceHasAttention {
                            attention: AttentionKind::Failed,
                        },
                        vec![Step::text("retry\\r")],
                        Vec::new(),
                    )],
                    vec![Step::action(KeyAction::NextAttention)],
                ),
            ],
        ),
        binding("Bare", "Alt+K", Vec::new()),
    ]);

    let json = serde_json::to_string(&before).unwrap();
    let after: CustomBindings = serde_json::from_str(&json).unwrap();
    assert_eq!(after, before);

    // An array, not an object with a key: the shape the settings file stores is
    // part of the contract, because a later build reading it has no chance to
    // guess a different one.
    assert!(
        json.starts_with('['),
        "the wire form is not an array: {json}"
    );

    // And the plan is the same on both sides, so the round trip preserved
    // behaviour and not merely bytes.
    let facts = Facts {
        focused: Some(FocusedSession {
            status: StatusKind::Ready,
            unread: false,
            command: "/usr/bin/claude".to_string(),
        }),
        workspace_attention: [AttentionKind::Failed].into_iter().collect(),
        ..bare()
    };
    assert_eq!(
        after.all()[0].plan(&facts).unwrap(),
        vec![
            Effect::Text(vec![0x03]),
            Effect::Action(KeyAction::ToggleSidebar),
            Effect::Text(b"retry\r".to_vec()),
        ]
    );
}

/// An entry that is not a binding at all is dropped, and the entries around it
/// still load. The bindings share one settings file with every other
/// preference, so failing the array would cost the operator their theme, their
/// workspaces and their daemon URL over one mistyped row.
#[test]
fn a_structurally_broken_entry_is_dropped_without_taking_the_others() {
    let raw = r#"[
        {"label": "First", "chord": "Ctrl+Shift+G", "steps": []},
        {"label": "Wrecked", "chord": "Ctrl+Shift+H", "steps": "not a list"},
        {"label": "Last", "chord": "Ctrl+Shift+J", "steps": [], "fromALaterBuild": 7}
    ]"#;
    let bindings: CustomBindings = serde_json::from_str(raw).expect("the array still loads");
    assert_eq!(
        bindings
            .all()
            .iter()
            .map(|b| b.label.as_str())
            .collect::<Vec<_>>(),
        vec!["First", "Last"],
        "the broken entry took a working one with it, or was kept"
    );
}

/// A chord text that is not a chord never matches a keystroke and is reported
/// instead. Matching it loosely would let a typo in one row capture keys meant
/// for another, which reads as the whole keyboard breaking.
#[test]
fn a_binding_whose_chord_is_nonsense_matches_nothing_and_is_reported() {
    let bindings = CustomBindings::from(vec![
        binding("Typo", "Ctrl+", vec![Step::text("x")]),
        binding("Fine", "Ctrl+Shift+G", vec![Step::text("y")]),
    ]);

    assert_eq!(
        bindings.errors(),
        vec![(
            0,
            BindingError::BadChord {
                chord: "Ctrl+".to_string()
            }
        )]
    );
    assert_eq!(
        bindings
            .lookup(&crate::launch::parse_chord("Ctrl+Shift+G").unwrap())
            .map(|b| b.label.as_str()),
        Some("Fine"),
        "the row after the typo lost its chord"
    );
    assert!(
        bindings.shadowing("b", true, false, Shift::Off).is_none(),
        "an unparseable chord captured an unrelated keystroke"
    );
}

/// Editing operations the settings panel needs. Order is the operator's, so an
/// add appends and a remove closes the gap rather than renumbering by chord.
#[test]
fn the_list_keeps_the_order_the_operator_gave_it() {
    let mut bindings = CustomBindings::default();
    assert!(bindings.is_empty());

    for label in ["one", "two", "three"] {
        bindings.push(binding(label, "Ctrl+Shift+G", Vec::new()));
    }
    assert_eq!(bindings.len(), 3);

    bindings.get_mut(1).expect("the second row exists").label = "second".to_string();
    assert!(bindings.remove(0));
    assert!(
        !bindings.remove(9),
        "removing a row that is not there reports it"
    );

    assert_eq!(
        bindings
            .all()
            .iter()
            .map(|b| b.label.as_str())
            .collect::<Vec<_>>(),
        vec!["second", "three"]
    );
}

/// A custom binding is reachable by the chord it was saved under, and a
/// binding whose chord does not parse is reachable by nothing.
///
/// WHY: a binding that is saved, listed and validated, and then fires
/// nothing, is the worst shape this feature can take, because every surface
/// tells the operator it works. It happened once already: the dispatcher
/// matched a table that custom bindings were never folded into, so a chord no
/// built-in owned was dead. The lookup below is the step that was missing, and
/// `keys::claim_live` is what calls it now.
#[test]
fn a_custom_binding_is_found_by_the_chord_it_was_saved_under() {
    let bindings = CustomBindings::from(vec![
        binding("Interrupt", "Ctrl+Shift+G", vec![Step::text("\\x03")]),
        binding("Typo", "Ctrl+", vec![Step::text("x")]),
    ]);

    let chord = crate::launch::parse_chord("Ctrl+Shift+G").expect("a chord that parses");
    assert_eq!(
        bindings.lookup(&chord).map(|found| found.label.as_str()),
        Some("Interrupt")
    );

    assert!(
        crate::launch::parse_chord("Ctrl+").is_none(),
        "a chord that does not parse must not be reachable, and must not \
         shadow a built-in by matching loosely"
    );
}
