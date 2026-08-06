use super::*;

/// A window with a focused claude session, nothing else raised.
fn focused_claude() -> Facts {
    Facts {
        focused: Some(FocusedSession {
            status: StatusKind::Ready,
            unread: false,
            command: "/usr/bin/claude".to_string(),
        }),
        layer: None,
        sidebar_visible: true,
        workspace_attention: std::collections::BTreeSet::new(),
    }
}

fn binding(steps: Vec<Step>) -> CustomBinding {
    CustomBinding {
        label: "test".to_string(),
        chord: "Ctrl+Shift+G".to_string(),
        steps,
    }
}

/// Facts that make a predicate hold, and facts that make it fail, for every
/// predicate. Both directions, because a predicate stuck at one answer is
/// invisible: the binding still runs, it just always takes one branch.
fn predicate_cases() -> Vec<(Predicate, Facts, Facts)> {
    let no_focus = Facts {
        focused: None,
        ..focused_claude()
    };
    let unread = Facts {
        focused: Some(FocusedSession {
            unread: true,
            ..focused_claude().focused.unwrap()
        }),
        ..focused_claude()
    };
    let working = Facts {
        focused: Some(FocusedSession {
            status: StatusKind::Working,
            ..focused_claude().focused.unwrap()
        }),
        ..focused_claude()
    };
    let in_settings = Facts {
        layer: Some(LayerKind::Settings),
        ..focused_claude()
    };
    let in_search = Facts {
        layer: Some(LayerKind::Search),
        ..focused_claude()
    };
    let collapsed = Facts {
        sidebar_visible: false,
        ..focused_claude()
    };
    let codex = Facts {
        focused: Some(FocusedSession {
            command: "/usr/bin/codex".to_string(),
            ..focused_claude().focused.unwrap()
        }),
        ..focused_claude()
    };
    let failing = Facts {
        workspace_attention: [AttentionKind::Failed].into_iter().collect(),
        ..focused_claude()
    };
    let belling = Facts {
        workspace_attention: [AttentionKind::Bell].into_iter().collect(),
        ..focused_claude()
    };
    vec![
        (Predicate::SessionFocused, focused_claude(), no_focus.clone()),
        (
            Predicate::FocusedStatus {
                status: StatusKind::Working,
            },
            working,
            focused_claude(),
        ),
        (Predicate::FocusedUnread, unread, focused_claude()),
        (
            Predicate::LayerOpen {
                layer: LayerKind::Settings,
            },
            in_settings,
            in_search,
        ),
        (Predicate::SidebarVisible, focused_claude(), collapsed),
        (
            Predicate::FocusedCommandContains {
                text: "claude".to_string(),
            },
            focused_claude(),
            codex,
        ),
        (
            Predicate::WorkspaceHasAttention {
                attention: AttentionKind::Failed,
            },
            failing,
            belling,
        ),
    ]
}

/// Every predicate must answer true on the state it names and false on a
/// state it does not.
#[test]
fn every_predicate_answers_both_ways() {
    for (predicate, yes, no) in predicate_cases() {
        assert_eq!(
            predicate.holds(&yes),
            Some(true),
            "{predicate:?} did not hold on the state it names"
        );
        assert_eq!(
            predicate.holds(&no),
            Some(false),
            "{predicate:?} held on a state it does not name"
        );
    }
}

/// Every predicate about the focused session must be false, never true and
/// never an error, when nothing is focused. A binding that sends literal
/// text on a "status is ready" branch would otherwise fire into no session
/// at all the moment the last tab is closed.
#[test]
fn session_predicates_are_false_with_no_focus() {
    let empty = Facts::default();
    for predicate in [
        Predicate::SessionFocused,
        Predicate::FocusedUnread,
        Predicate::FocusedStatus {
            status: StatusKind::Ready,
        },
        Predicate::FocusedCommandContains {
            text: String::new(),
        },
    ] {
        assert_eq!(
            predicate.holds(&empty),
            Some(false),
            "{predicate:?} answered something other than false with no focus"
        );
    }
}

/// A sequence must produce its effects in the order written. This is the
/// whole point of an ordered step list: `Esc`, then `:w`, then Return is
/// three different outcomes in three different orders.
#[test]
fn a_sequence_runs_in_order() {
    let plan = binding(vec![
        Step::text("\\e"),
        Step::action(KeyAction::ToggleSidebar),
        Step::text(":w\\r"),
    ])
    .plan(&focused_claude())
    .expect("plans");
    assert_eq!(
        plan,
        vec![
            Effect::Text(vec![0x1b]),
            Effect::Action(KeyAction::ToggleSidebar),
            Effect::Text(b":w\r".to_vec()),
        ]
    );
}

/// A conditional must run exactly one branch, and the other branch must
/// contribute nothing at all.
#[test]
fn a_conditional_runs_exactly_one_branch() {
    let b = binding(vec![Step::when(
        Predicate::FocusedUnread,
        vec![Step::action(KeyAction::NextAttention)],
        vec![Step::action(KeyAction::NextTab)],
    )]);
    let read = focused_claude();
    let unread = Facts {
        focused: Some(FocusedSession {
            unread: true,
            ..read.focused.clone().unwrap()
        }),
        ..read.clone()
    };
    assert_eq!(
        b.plan(&unread).expect("plans"),
        vec![Effect::Action(KeyAction::NextAttention)]
    );
    assert_eq!(
        b.plan(&read).expect("plans"),
        vec![Effect::Action(KeyAction::NextTab)]
    );
}

/// Nesting must resolve inside out, and only the reached branch may
/// contribute. A planner that flattened every branch would send the
/// interrupt below on every press.
#[test]
fn nested_conditionals_resolve_to_the_reached_leaf() {
    let b = binding(vec![Step::when(
        Predicate::SessionFocused,
        vec![Step::when(
            Predicate::FocusedCommandContains {
                text: "claude".to_string(),
            },
            vec![Step::when(
                Predicate::SidebarVisible,
                vec![Step::text("ok")],
                vec![Step::action(KeyAction::ToggleSidebar)],
            )],
            vec![Step::text("\\x03")],
        )],
        vec![Step::action(KeyAction::NewSession)],
    )]);
    assert_eq!(
        b.plan(&focused_claude()).expect("plans"),
        vec![Effect::Text(b"ok".to_vec())]
    );
    let collapsed = Facts {
        sidebar_visible: false,
        ..focused_claude()
    };
    assert_eq!(
        b.plan(&collapsed).expect("plans"),
        vec![Effect::Action(KeyAction::ToggleSidebar)]
    );
    assert_eq!(
        b.plan(&Facts::default()).expect("plans"),
        vec![Effect::Action(KeyAction::NewSession)]
    );
}

/// Nesting up to the limit must still work. A limit that also refused the
/// legitimate three-deep binding would be a bug wearing a guard's clothes.
#[test]
fn nesting_up_to_the_limit_is_accepted() {
    let b = binding(vec![nest(MAX_BINDING_DEPTH, Step::text("deep"))]);
    b.validate().expect("a binding at the limit is valid");
    assert_eq!(
        b.plan(&focused_claude()).expect("plans"),
        vec![Effect::Text(b"deep".to_vec())]
    );
}

/// `levels` conditionals wrapped around `leaf`, all on the true branch.
fn nest(levels: usize, leaf: Step) -> Step {
    let mut inner = vec![leaf];
    for _ in 1..levels {
        inner = vec![Step::when(Predicate::SidebarVisible, inner, Vec::new())];
    }
    Step::when(Predicate::SidebarVisible, inner, Vec::new())
}

/// A hand-edited depth bomb must be refused, by both the validator and the
/// planner, without overflowing the stack. The settings file is a text file
/// an operator can edit, and a crash on load is unrecoverable: it takes out
/// every other setting in the file along with the bad binding.
#[test]
fn a_depth_bomb_is_refused_rather_than_crashing() {
    let bomb = binding(vec![nest(4_096, Step::text("boom"))]);
    let too_deep = BindingError::TooDeep {
        limit: MAX_BINDING_DEPTH,
    };
    assert_eq!(bomb.validate(), Err(too_deep.clone()));
    assert_eq!(bomb.plan(&focused_claude()), Err(too_deep));
}

/// A refused binding must perform nothing, not the prefix it managed to
/// resolve. Half a keystroke sequence is a worse outcome than none.
#[test]
fn a_refused_binding_yields_no_partial_effects() {
    let b = binding(vec![
        Step::action(KeyAction::NextTab),
        Step::text("\\q"),
        Step::action(KeyAction::PrevTab),
    ]);
    assert_eq!(
        b.plan(&focused_claude()),
        Err(BindingError::BadEscape {
            at: 0,
            what: "\\q".to_string()
        })
    );
}

/// The exact bytes for each escape, and refusal for everything else. This
/// is the byte stream that reaches a live agent's pty, so an off-by-one in
/// the table types real characters at real work.
#[test]
fn literal_text_decodes_to_exact_bytes() {
    let cases: &[(&str, &[u8])] = &[
        ("", b""),
        ("hi", b"hi"),
        ("\\n", b"\n"),
        ("\\r", b"\r"),
        ("\\t", b"\t"),
        ("\\e", &[0x1b]),
        ("\\a", &[0x07]),
        ("\\b", &[0x08]),
        ("\\f", &[0x0c]),
        ("\\0", &[0x00]),
        ("\\x03", &[0x03]),
        ("\\xFF", &[0xff]),
        ("\\\\e", b"\\e"),
        ("\\e[A", &[0x1b, b'[', b'A']),
        ("caf\u{e9}", &[b'c', b'a', b'f', 0xc3, 0xa9]),
    ];
    for (text, want) in cases {
        assert_eq!(
            decode_literal(text).as_deref(),
            Ok(*want),
            "{text:?} decoded wrongly"
        );
    }
    for bad in ["\\q", "\\xZZ", "\\x0Z"] {
        assert!(
            matches!(decode_literal(bad), Err(BindingError::BadEscape { .. })),
            "{bad:?} was accepted"
        );
    }
    for cut in ["\\", "ab\\", "\\x", "\\x0"] {
        assert!(
            matches!(
                decode_literal(cut),
                Err(BindingError::UnterminatedEscape { .. })
            ),
            "{cut:?} was accepted"
        );
    }
}

/// Every step and every predicate must survive the settings file unchanged,
/// including the unknown cases. A variant that round-trips into a different
/// variant silently rewrites the operator's binding on the next save.
#[test]
fn every_step_and_predicate_round_trips() {
    let mut predicates = vec![
        Predicate::SessionFocused,
        Predicate::FocusedUnread,
        Predicate::SidebarVisible,
        Predicate::FocusedCommandContains {
            text: "claude".to_string(),
        },
        Predicate::Unknown,
    ];
    predicates.extend(
        StatusKind::all()
            .iter()
            .copied()
            .chain([StatusKind::Unknown])
            .map(|status| Predicate::FocusedStatus { status }),
    );
    predicates.extend(
        LayerKind::all()
            .iter()
            .copied()
            .chain([LayerKind::Unknown])
            .map(|layer| Predicate::LayerOpen { layer }),
    );
    predicates.extend(
        AttentionKind::all()
            .iter()
            .copied()
            .chain([AttentionKind::Unknown])
            .map(|attention| Predicate::WorkspaceHasAttention { attention }),
    );

    let mut steps = vec![
        Step::action(KeyAction::SelectTab(3)),
        Step::action(KeyAction::LaunchPreset(7)),
        Step::text("\\e:w\\r"),
        Step::Unknown,
    ];
    for predicate in &predicates {
        let json = serde_json::to_string(predicate).expect("serialises");
        let back: Predicate = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(&back, predicate, "predicate changed through {json}");
        steps.push(Step::when(
            predicate.clone(),
            vec![Step::text("y")],
            vec![Step::Unknown],
        ));
    }

    for step in &steps {
        let json = serde_json::to_string(step).expect("serialises");
        let back: Step = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(&back, step, "step changed through {json}");
    }

    let b = binding(steps);
    let json = serde_json::to_string(&b).expect("serialises");
    assert_eq!(
        serde_json::from_str::<CustomBinding>(&json).expect("deserialises"),
        b
    );
}

/// The serialised names are kebab-case and are part of the file format.
/// Renaming one silently drops every binding that used it.
#[test]
fn the_serialised_shape_is_kebab_case() {
    let b = CustomBinding {
        label: "Interrupt".to_string(),
        chord: "Ctrl+Alt+C".to_string(),
        steps: vec![Step::when(
            Predicate::LayerOpen {
                layer: LayerKind::NewSession,
            },
            vec![Step::action(KeyAction::Dismiss)],
            vec![Step::text("\\x03")],
        )],
    };
    assert_eq!(
        serde_json::to_string(&b).expect("serialises"),
        r#"{"label":"Interrupt","chord":"Ctrl+Alt+C","steps":[{"step":"when","predicate":{"kind":"layer-open","layer":"new-session"},"then":[{"step":"action","action":"dismiss"}],"otherwise":[{"step":"text","text":"\\x03"}]}]}"#
    );
}

/// A binding written by a newer build must load, and the parts this build
/// understands must still run. The alternative is a settings file that
/// fails to parse, which loses every unrelated setting in it.
#[test]
fn a_newer_builds_binding_degrades_instead_of_failing_to_parse() {
    let json = r#"{
        "label": "from the future",
        "chord": "Ctrl+Shift+G",
        "steps": [
            {"step": "action", "action": "next"},
            {"step": "teleport", "destination": "mars"},
            {"step": "action", "action": "warpDrive"},
            {"step": "when", "predicate": {"kind": "moon-is-full"},
             "then": [{"step": "text", "text": "yes"}],
             "otherwise": [{"step": "text", "text": "no"}]},
            {"step": "when",
             "predicate": {"kind": "focused-status", "status": "meditating"},
             "then": [{"step": "text", "text": "yes"}],
             "otherwise": [{"step": "text", "text": "no"}]},
            {"step": "action", "action": "prev"}
        ]
    }"#;
    let b: CustomBinding = serde_json::from_str(json).expect("must still parse");
    assert_eq!(
        b.steps[1],
        Step::Unknown,
        "an unknown step must degrade to the inert one"
    );
    assert_eq!(
        b.plan(&focused_claude()).expect("plans"),
        vec![
            Effect::Action(KeyAction::NextTab),
            Effect::Action(KeyAction::PrevTab),
        ],
        "an unknown step, action or predicate must contribute nothing while \
         the known steps still run"
    );
    b.validate().expect("an unknown step is not invalid");
}

/// An unknown kind must not answer, and must not fall through to the else
/// branch. Guessing the other side of a question this build cannot read
/// runs the branch the operator did not mean.
#[test]
fn an_unknown_kind_answers_nothing() {
    for predicate in [
        Predicate::Unknown,
        Predicate::FocusedStatus {
            status: StatusKind::Unknown,
        },
        Predicate::LayerOpen {
            layer: LayerKind::Unknown,
        },
        Predicate::WorkspaceHasAttention {
            attention: AttentionKind::Unknown,
        },
    ] {
        assert_eq!(predicate.holds(&focused_claude()), None, "{predicate:?}");
    }
}

/// The chord half must go through the profile chord parser, so a custom
/// binding is refused for exactly the reasons a preset shortcut is: a bare
/// key would eat that letter every time the operator typed it.
#[test]
fn the_chord_must_be_a_real_chord() {
    for good in ["Ctrl+Shift+G", "Alt+7", "ctrl+alt+k"] {
        let b = CustomBinding {
            chord: good.to_string(),
            ..binding(Vec::new())
        };
        assert!(b.parsed_chord().is_some(), "{good} was refused");
        b.validate().expect("valid");
    }
    for bad in ["", "G", "Shift+G", "Ctrl+Ctrl+G", "Ctrl+A+B"] {
        let b = CustomBinding {
            chord: bad.to_string(),
            ..binding(Vec::new())
        };
        assert_eq!(
            b.validate(),
            Err(BindingError::BadChord {
                chord: bad.to_string()
            }),
            "{bad:?} was accepted as a chord"
        );
    }
}

/// The status kinds must mirror the model's five, so a binding on "failed"
/// means the same thing the sidebar pill means.
#[test]
fn status_kinds_mirror_the_sidebar_statuses() {
    let mapped: Vec<StatusKind> = vitrum_model::ALL_STATUSES
        .iter()
        .copied()
        .map(StatusKind::from)
        .collect();
    assert_eq!(mapped, StatusKind::all().to_vec());
}
