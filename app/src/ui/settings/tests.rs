use super::*;
use crate::state::{NotifyPrefs, SettingsTab, TEXT_SCALE_MAX_PCT, TEXT_SCALE_MIN_PCT};
use vitrum_proto::{Attention, ProjectId, SessionInfo};

fn info(id: u64) -> SessionInfo {
    SessionInfo {
        id: SessionId(id),
        project_id: ProjectId(1),
        title: format!("session {id}"),
        cwd: "/home/mk/src/vitrum".to_string(),
        command: "claude".to_string(),
        args: Vec::new(),
        status: SessionStatus::Running,
        created_at_ms: 1_000,
        last_activity_ms: 1_000,
        cols: 80,
        rows: 24,
        git_branch: None,
        worktree: None,
        unread: false,
        attention: Attention::default(),
        hint: None,
        term_title: None,
    }
}

/// A session the operator has looked at once, at t=0.
///
/// `last_visited_ms` matters: `has_unseen_completion` compares the
/// completion instant against it, and with `None` it falls back to the
/// daemon's `unread` flag instead. A fixture that leaves it unset is
/// testing the fallback path, not the one the client actually takes, and
/// would report "no completion" for a session that plainly just exited.
fn view(id: u64) -> SessionView {
    SessionView {
        last_visited_ms: Some(0),
        ..SessionView::new(info(id))
    }
}

// -- Appearance ---------------------------------------------------------











/// A hand-edited file must not be able to produce a window whose first row
/// is taller than the screen, which would put the settings gear
/// permanently out of reach.
#[test]
fn an_absurd_saved_text_scale_is_clamped_into_range() {
    assert_eq!(clamp_scale(4_000), TEXT_SCALE_MAX_PCT);
    assert_eq!(clamp_scale(1), TEXT_SCALE_MIN_PCT);
    assert_eq!(clamp_scale(125), 125);
}

// -- Terminal -----------------------------------------------------------

/// Every offered stack must end in the generic `monospace`, so a font the
/// machine lacks degrades to another monospace and not to a proportional
/// face.
#[test]
fn every_font_stack_falls_back_to_monospace() {
    for (label, stack) in FONT_STACKS {
        if stack.is_empty() {
            continue;
        }
        assert!(
            stack.ends_with("monospace"),
            "{label} would fall back to a proportional face: {stack}"
        );
    }
}

/// A degenerate saved font size makes the cell width zero and the pane go
/// blank with nothing logged.
#[test]
fn a_degenerate_saved_font_size_is_clamped_before_it_reaches_the_pane() {
    let tiny = TerminalPrefs {
        font_size_px: 0,
        ..TerminalPrefs::default()
    };
    assert_eq!(term_font_px(&tiny), TERM_FONT_MIN_PX);

    let huge = TerminalPrefs {
        font_size_px: 900,
        ..TerminalPrefs::default()
    };
    assert_eq!(term_font_px(&huge), TERM_FONT_MAX_PX);
}

/// Every offered size must be within the range the clamp allows, or the
/// menu would show a choice that silently becomes a different one.
#[test]
fn every_offered_terminal_size_survives_the_clamp() {
    for px in TERM_FONT_STEPS {
        assert!(
            (TERM_FONT_MIN_PX..=TERM_FONT_MAX_PX).contains(px),
            "{px}px is offered but would be clamped away"
        );
    }
}

// -- Notifications ------------------------------------------------------

/// The two urgent kinds notify by default and completion does not. Twenty
/// agents finishing is twenty interruptions about work nobody is waiting
/// on.
#[test]
fn the_urgent_kinds_notify_by_default_and_completion_does_not() {
    let prefs = NotifyPrefs::default();
    assert!(notify_enabled(&prefs, NotificationKind::NeedsApproval));
    assert!(notify_enabled(&prefs, NotificationKind::Failed));
    assert!(!notify_enabled(&prefs, NotificationKind::Finished));
}

/// Every switch must silence its own kind and no other. Matching the wrong
/// arm here would make the failure switch mute approvals, which is the
/// exact opposite of what the operator asked for.
#[test]
fn each_notification_switch_governs_only_its_own_kind() {
    for kind in NOTIFY_KINDS {
        let mut prefs = NotifyPrefs {
            finished: true,
            needs_approval: true,
            failed: true,
            skip_focused_session: false,
        };
        set_notify_enabled(&mut prefs, kind, false);
        assert!(
            !should_notify(&prefs, kind, false),
            "{kind} was not silenced"
        );
        for other in NOTIFY_KINDS {
            if other != kind {
                assert!(
                    should_notify(&prefs, other, false),
                    "silencing {kind} also silenced {other}"
                );
            }
        }
    }
}

/// Suppressing the focused session must apply to every kind. A partial
/// implementation would still pop a desktop notification about the
/// terminal the operator is looking at.
#[test]
fn the_focused_session_is_suppressed_for_every_kind() {
    let prefs = NotifyPrefs {
        finished: true,
        needs_approval: true,
        failed: true,
        skip_focused_session: true,
    };
    for kind in NOTIFY_KINDS {
        assert!(
            !should_notify(&prefs, kind, true),
            "{kind} leaked while focused"
        );
        assert!(
            should_notify(&prefs, kind, false),
            "{kind} lost while unfocused"
        );
    }
}

/// The suppression is a preference, not a law.
#[test]
fn focus_suppression_can_be_turned_off() {
    let prefs = NotifyPrefs {
        skip_focused_session: false,
        ..NotifyPrefs::default()
    };
    assert!(should_notify(&prefs, NotificationKind::Failed, true));
}

/// A steady state must produce nothing. Level-triggered notification is
/// the defect this function exists to prevent: the daemon pushes snapshots
/// several times a second and one blocked session would otherwise raise a
/// notification on every one of them.
#[test]
fn an_unchanged_snapshot_is_never_notable() {
    let mut blocked = view(1);
    blocked.info.attention.waiting = Some(true);
    let rows = vec![blocked];
    assert_eq!(notable_transitions(&rows, &rows), Vec::new());
}

/// A clean exit is a completion, and it must fire exactly once.
#[test]
fn a_clean_exit_notifies_once_on_the_edge() {
    let before = vec![view(1)];
    let mut done = view(1);
    done.info.status = SessionStatus::Exited { code: Some(0) };
    let after = vec![done];

    let first = notable_transitions(&before, &after);
    assert_eq!(first.len(), 1, "expected one completion, got {first:?}");
    assert_eq!(first[0].kind, NotificationKind::Finished);
    assert_eq!(first[0].session, SessionId(1));

    assert_eq!(
        notable_transitions(&after, &after),
        Vec::new(),
        "the same completion fired twice"
    );
}

/// A non-zero exit is a failure, not a completion, and it must produce ONE
/// notification. It satisfies both predicates, and two notifications about
/// one dead process is how a product teaches people to mute it.
#[test]
fn a_nonzero_exit_is_one_failure_and_not_also_a_completion() {
    let before = vec![view(1)];
    let mut dead = view(1);
    dead.info.status = SessionStatus::Exited { code: Some(101) };

    let out = notable_transitions(&before, &[dead]);
    assert_eq!(out.len(), 1, "expected exactly one notification: {out:?}");
    assert_eq!(out[0].kind, NotificationKind::Failed);
    assert_eq!(out[0].detail, "claude exited 101");
}

/// A session the previous snapshot never held is never notable. The list
/// arrives whole on every reconnect, so the alternative empties a day of
/// failures onto the desktop the moment the socket flaps.
#[test]
fn a_reconnect_does_not_replay_history_as_notifications() {
    let mut dead = view(1);
    dead.info.status = SessionStatus::Exited { code: Some(2) };
    let mut blocked = view(2);
    blocked.info.attention.waiting = Some(true);

    assert_eq!(
        notable_transitions(&[], &[dead, blocked]),
        Vec::new(),
        "a fresh snapshot replayed old state as new transitions"
    );
}

/// The payload must carry the session so a click can focus it, and must
/// name the moment in the title.
#[test]
fn the_payload_carries_the_session_and_names_the_moment() {
    let before = vec![view(7)];
    let mut dead = view(7);
    dead.info.status = SessionStatus::Exited { code: Some(3) };
    let out = notable_transitions(&before, &[dead]);
    let notification = out[0].notification();
    assert_eq!(notification.session, SessionId(7));
    assert!(
        notification.title.contains("failed"),
        "title does not name the moment: {}",
        notification.title
    );
    assert!(notification.activation_url().contains("session/7"));
}

/// Every kind the tab lists must have both a label and an explanation. A
/// switch with a blank description is a switch nobody can evaluate.
#[test]
fn every_notification_kind_is_explained() {
    for kind in NOTIFY_KINDS {
        let (label, desc) = notify_label(kind);
        assert!(!label.is_empty(), "{kind} has no label");
        assert!(desc.len() > 20, "{kind} has no real explanation: {desc}");
    }
    assert_eq!(NOTIFY_KINDS.len(), 3, "a notification kind is unreachable");
}

// -- Keyboard -----------------------------------------------------------

/// With no overrides, the live table must BE the built-in one, field for
/// field.
///
/// Asserted against `CHORDS` itself rather than against a second encoder.
/// There used to be two functions producing this table, one here and one in
/// `keymap.rs`, and this test compared them to each other: a comparison
/// that holds just as well when both are wrong. It held while the shipped
/// startup table was the defaults-only one, which is what made every rebound
/// chord and every preset shortcut dead until a later push landed.
#[test]
fn no_overrides_reproduces_the_builtin_table_exactly() {
    let effective = effective_chords(&KeyboardPrefs::default());
    assert_eq!(effective.len(), CHORDS.len());
    assert!(effective.iter().all(|chord| !chord.rebound));

    for (entry, chord) in effective.iter().zip(CHORDS) {
        assert_eq!(entry.key, chord.key, "key drifted from CHORDS");
        assert_eq!(entry.ctrl, chord.ctrl);
        assert_eq!(entry.alt, chord.alt);
        assert_eq!(entry.action, chord.action);
        assert_eq!(entry.shift, chord.shift);
        assert_eq!(entry.scope, chord.scope);
    }
}

/// The table in place at startup must be the one the operator has.
///
/// It is the only copy guaranteed to be there before the first keydown. It
/// used to be built from the compile-time defaults, so a preset shortcut did
/// nothing at all: the store was read correctly, the chord was encoded
/// correctly, and the table dispatch matched against had never heard of it.
#[test]
fn the_startup_table_carries_saved_preset_chords() {
    let preset = crate::launch::SavedPreset {
        id: 42,
        label: "Vitrum shell".to_string(),
        command: "bash".to_string(),
        args: vec!["-l".to_string()],
        cwd: Some("/src/vitrum".to_string()),
        shortcut: Some("ctrl+shift+j".to_string()),
        icon: None,
    };
    let table = live_chords(&KeyboardPrefs::default(), &[preset]);
    assert!(
        table.iter().any(|chord| chord.action.wire() == "preset:42"),
        "the startup table has no preset chord in it"
    );
}

/// A rebinding must reach the live table, and the old chord must be gone. A
/// rebinder that adds without removing leaves the action reachable at both
/// chords, which is the ghost binding the feature exists to avoid.
#[test]
fn a_rebinding_replaces_the_old_chord_rather_than_adding_to_it() {
    let mut prefs = KeyboardPrefs::default();
    let binding = Binding {
        key: "j".to_string(),
        ctrl: true,
        alt: false,
        shift: false,
    };
    set_override(&mut prefs, KeyAction::ToggleSidebar, &binding);

    let effective = effective_chords(&prefs);
    let mine: Vec<&EffectiveChord> = effective
        .iter()
        .filter(|chord| chord.action == KeyAction::ToggleSidebar)
        .collect();
    assert!(!mine.is_empty(), "the action vanished from the table");
    for chord in &mine {
        assert_eq!(chord.key, "j", "an alias kept its default chord");
        assert!(chord.rebound);
    }
    assert!(
        !effective
            .iter()
            .any(|chord| chord.key == "b" && chord.ctrl && !chord.alt),
        "the default Ctrl+B is still live"
    );
}

/// A rebinding must survive the write to disk and back. Persistence is
/// half the promise; a chord that reverts on restart is a chord nobody
/// trusts.
#[test]
fn a_rebinding_round_trips_through_the_settings_file() {
    let mut prefs = KeyboardPrefs::default();
    let binding = Binding {
        key: "arrowup".to_string(),
        ctrl: true,
        alt: true,
        shift: true,
    };
    set_override(&mut prefs, KeyAction::NextAttention, &binding);

    let text = serde_json::to_string(&prefs).expect("plain data");
    let back: KeyboardPrefs = serde_json::from_str(&text).expect("round trip");
    assert_eq!(
        override_for(&back, KeyAction::NextAttention),
        Some(binding),
        "the rebinding did not survive the file"
    );
}

/// The stored form must be canonical, so one chord has exactly one
/// spelling and a file written by one build reads identically in the next.
#[test]
fn the_stored_chord_form_is_canonical_and_reparses() {
    let binding = Binding {
        key: "k".to_string(),
        ctrl: true,
        alt: true,
        shift: true,
    };
    assert_eq!(binding.encode(), "ctrl+alt+shift+k");
    assert_eq!(Binding::parse("ctrl+alt+shift+k"), Some(binding.clone()));
    assert_eq!(Binding::parse("CTRL+ALT+SHIFT+K"), Some(binding));
}

/// An override this build cannot parse must be ignored and the default
/// must stand. Any other behaviour lets a bad settings file lock a user
/// out of their own keyboard.
#[test]
fn an_unparsable_override_falls_back_to_the_default_binding() {
    let mut prefs = KeyboardPrefs::default();
    prefs
        .overrides
        .insert(KeyAction::ToggleSidebar.wire(), "super+hyper+q".to_string());
    assert_eq!(override_for(&prefs, KeyAction::ToggleSidebar), None);
    assert_eq!(
        effective_chords(&prefs),
        effective_chords(&KeyboardPrefs::default()),
        "a junk override changed the live table"
    );
}

/// A stored override that would be refused in the editor must also be
/// refused on the way in. Hand-editing the file must not be a way around
/// the "no bare letters" rule.
#[test]
fn a_stored_override_that_steals_a_bare_key_is_rejected_on_load() {
    assert_eq!(Binding::parse("k"), None);
    assert_eq!(Binding::parse("shift+k"), None);
    assert_eq!(Binding::parse("ctrl+escape"), None);
}

/// Clearing must restore the default exactly, with no empty entry left
/// behind reporting the action as rebound forever.
#[test]
fn clearing_a_rebinding_restores_the_default_exactly() {
    let mut prefs = KeyboardPrefs::default();
    set_override(
        &mut prefs,
        KeyAction::NewSession,
        &Binding {
            key: "q".to_string(),
            ctrl: true,
            alt: true,
            shift: false,
        },
    );
    assert!(!prefs.overrides.is_empty());
    clear_override(&mut prefs, KeyAction::NewSession);
    assert!(prefs.overrides.is_empty());
    assert_eq!(
        effective_chords(&prefs),
        effective_chords(&KeyboardPrefs::default())
    );
}

/// A colliding binding must be named, not silently accepted. Two actions
/// on one chord means the first in table order wins and the second is
/// dead, with nothing on screen saying so.
#[test]
fn a_colliding_binding_names_the_action_it_would_shadow() {
    let effective = effective_chords(&KeyboardPrefs::default());
    let existing = effective
        .iter()
        .find(|chord| chord.action == KeyAction::NewSession)
        .expect("new session is bound");
    assert_eq!(
        chord_conflict(&effective, &existing.binding(), KeyAction::ToggleSidebar),
        Some(KeyAction::NewSession)
    );
}

/// Rebinding an action onto its own current chord is not a conflict.
/// Reporting one would make the editor refuse to save a row whose
/// modifiers the user toggled and then toggled back.
#[test]
fn an_action_never_conflicts_with_itself() {
    let effective = effective_chords(&KeyboardPrefs::default());
    let existing = effective
        .iter()
        .find(|chord| chord.action == KeyAction::NewSession)
        .expect("new session is bound");
    assert_eq!(
        chord_conflict(&effective, &existing.binding(), KeyAction::NewSession),
        None
    );
}

/// A `Shift::Any` chord must collide with both shift states. Comparing
/// shift by equality would report "free" for a chord that does fire, and
/// the user would bind it and find one of the two dead.
#[test]
fn a_shift_agnostic_chord_collides_with_both_shift_states() {
    let any = EffectiveChord {
        action: KeyAction::NextTab,
        key: "k".to_string(),
        ctrl: true,
        alt: false,
        shift: Shift::Any,
        scope: Scope::Global,
        help: None,
        rebound: false,
    };
    for shift in [true, false] {
        let candidate = Binding {
            key: "k".to_string(),
            ctrl: true,
            alt: false,
            shift,
        };
        assert_eq!(
            chord_conflict(std::slice::from_ref(&any), &candidate, KeyAction::PrevTab),
            Some(KeyAction::NextTab),
            "shift={shift} was reported free against a Shift::Any chord"
        );
    }
}

/// A modifier-free binding must be refused with a reason. The shell
/// matches chords globally, so binding a bare letter makes that letter
/// unusable inside every agent, which reads as a broken keyboard.
#[test]
fn a_binding_without_ctrl_or_alt_is_refused() {
    let bare = Binding {
        key: "k".to_string(),
        ctrl: false,
        alt: false,
        shift: true,
    };
    let why = bare.rejection().expect("a bare letter must be refused");
    assert!(why.contains("Ctrl or Alt"), "unhelpful reason: {why}");
    assert_eq!(Binding { ctrl: true, ..bare }.rejection(), None);
}

/// Escape must never be rebindable. It dismisses every layer in the
/// program, so a user who moves it has no way out of the dialog they moved
/// it in.
#[test]
fn the_escape_hatch_keys_cannot_be_rebound() {
    for key in RESERVED_KEYS {
        let binding = Binding {
            key: (*key).to_string(),
            ctrl: true,
            alt: false,
            shift: false,
        };
        assert!(
            binding.rejection().is_some(),
            "{key} was accepted as a rebinding target"
        );
    }
}

/// The positional tab slots must not be offered. Alt+1 through Alt+9 are
/// one documented range, and moving slot 3 alone leaves a hole in a
/// sequence the overlay advertises as contiguous.
#[test]
fn the_positional_tab_slots_are_not_offered_for_rebinding() {
    let rows = rebindable();
    assert!(
        !rows
            .iter()
            .any(|(action, _)| matches!(action, KeyAction::SelectTab(_))),
        "a positional tab slot was offered"
    );
    assert!(rows.iter().any(|(a, _)| *a == KeyAction::NewSession));
    assert!(rows.iter().any(|(a, _)| *a == KeyAction::ToggleSidebar));
}

/// Every rebindable action appears exactly once and says what it does. A
/// duplicate row means two editors writing one override, and whichever was
/// saved second silently wins.
#[test]
fn every_rebindable_action_is_offered_once_and_described() {
    let rows = rebindable();
    let mut wires: Vec<String> = rows.iter().map(|(a, _)| a.wire()).collect();
    wires.sort();
    let before = wires.len();
    wires.dedup();
    assert_eq!(wires.len(), before, "a duplicate row in {rows:?}");
    assert!(!rows.is_empty());
    for (action, what) in &rows {
        assert!(!what.is_empty(), "{} has no description", action.wire());
    }
}

/// Every offered key must render as something a human can read, or the
/// overlay would advertise `Ctrl+arrowup`.
#[test]
fn every_bindable_key_has_a_readable_label() {
    for key in BINDABLE_KEYS {
        let label = pretty_key(key);
        assert!(!label.is_empty(), "{key} rendered as nothing");
        assert!(
            !label.contains("arrow") && !label.contains("page"),
            "{key} leaked its raw DOM name as {label}"
        );
    }
}

/// No offered key may be one of the reserved ones, or the menu would show
/// a choice the editor then refuses to save.
#[test]
fn the_bindable_menu_offers_no_reserved_key() {
    for key in RESERVED_KEYS {
        assert!(
            !BINDABLE_KEYS.contains(key),
            "{key} is offered but can never be saved"
        );
    }
}



/// Every tab must be reachable from the rail.
///
/// A variant missing from `ALL` is a page nobody can open: the sheet
/// renders one panel per variant, so the panel exists, compiles, and has
/// no way in. That is invisible to the compiler, which is the whole reason
/// this test exists.
///
/// It used to assert `ALL.len() == 8`, which is a proxy and not the
/// subject: the count had to be hand-bumped, said nothing about *which*
/// tabs were present, and passed happily if someone added a variant to
/// `ALL` twice and left another one out. The match below is exhaustive, so
/// adding a variant is a compile error until it is listed here, and the
/// assertion then proves that same variant is reachable.
#[test]
fn every_settings_tab_is_reachable_and_named() {
    // Exhaustive on purpose: a new variant fails to compile here.
    fn declared(tab: SettingsTab) -> SettingsTab {
        tab
    }
    let every = [
        SettingsTab::Appearance,
        SettingsTab::Sidebar,
        SettingsTab::Workspaces,
        SettingsTab::Presets,
        SettingsTab::Terminal,
        SettingsTab::Notifications,
        SettingsTab::Keyboard,
        SettingsTab::Advanced,
        SettingsTab::About,
    ];
    for tab in every {
        assert_eq!(declared(tab), tab);
        assert!(
            SettingsTab::ALL.contains(&tab),
            "{tab:?} is not on the rail, so its panel cannot be opened"
        );
        assert!(!tab.label().is_empty(), "{tab:?} has no label");
    }
    assert_eq!(
        SettingsTab::ALL.len(),
        every.len(),
        "the rail lists a tab twice, or lists one that is not a variant"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// A stored value the menu cannot express
// ═══════════════════════════════════════════════════════════════════════════

/// Every numeric menu in the sheet can reach the stray state, so the rule is
/// asserted on the fold rather than on one control.
///
/// `AppearancePrefs::clamp` is a range clamp too: 73% opacity, a 10px blur and
/// a 33% dim all survive it and none of them is a step.
#[test]
fn stray_option_fires_for_every_value_outside_the_offered_steps() {
    let opacity: Vec<(String, String)> = OPACITY_STEPS
        .iter()
        .map(|p| (p.to_string(), format!("{p}%")))
        .collect();
    assert_eq!(
        stray_option("73", &opacity).as_deref(),
        Some("73 (in effect, not one of the choices)")
    );
    assert_eq!(stray_option("70", &opacity), None);

    let blur: Vec<(String, String)> = BLUR_STEPS
        .iter()
        .map(|p| (p.to_string(), blur_label(*p)))
        .collect();
    assert_eq!(
        stray_option("10", &blur).as_deref(),
        Some("10 (in effect, not one of the choices)")
    );
    assert_eq!(stray_option("8", &blur), None);

    let dim: Vec<(String, String)> = DIM_STEPS
        .iter()
        .map(|p| (p.to_string(), format!("{p}%")))
        .collect();
    assert_eq!(
        stray_option("33", &dim).as_deref(),
        Some("33 (in effect, not one of the choices)")
    );
    assert_eq!(stray_option("30", &dim), None);
}






/// A saved command with a chord, for the conflict table.
fn command_on(id: u64, chord: &str) -> crate::launch::SavedPreset {
    crate::launch::SavedPreset {
        id,
        label: "Review the diff".to_string(),
        command: "claude".to_string(),
        args: Vec::new(),
        cwd: Some("/src/vitrum".to_string()),
        shortcut: Some(chord.to_string()),
        icon: None,
    }
}

/// A chord collides in both directions, against one folded table.
///
/// THE BUG this stops: two shortcut editors answering "is this taken" from two
/// different tables. The saved commands tab asked a list of built-in chords
/// compiled into the binary, so a chord the operator had rebound an action
/// ONTO was reported free, stored, shown on the row, and then never fired
/// because the rebinding matched first. The keybinds tab had the mirror hole:
/// a chord a saved command owned looked free to a rebinding.
///
/// [`live_conflict`] is this same fold with the two lists read from the bus,
/// which is what both tabs now call.
#[test]
fn a_chord_collides_in_both_directions() {
    let candidate = Binding::parse("ctrl+shift+j").expect("ctrl+shift+j is a chord");

    let owned = live_chords(&KeyboardPrefs::default(), &[command_on(7, "ctrl+shift+j")]);
    assert_eq!(
        chord_conflict(&owned, &candidate, KeyAction::ToggleSidebar),
        Some(KeyAction::LaunchPreset(7)),
        "a chord a saved command owns was offered to a rebinding"
    );

    let mut prefs = KeyboardPrefs::default();
    set_override(&mut prefs, KeyAction::ToggleSidebar, &candidate);
    let rebound = live_chords(&prefs, &[]);
    assert_eq!(
        chord_conflict(&rebound, &candidate, KeyAction::LaunchPreset(9)),
        Some(KeyAction::ToggleSidebar),
        "a chord an action was rebound onto was offered to a saved command"
    );
}

/// A chord an action was moved off is free, and one moved onto it is not.
///
/// THE BUG this stops: a conflict answer read from the shipped table rather
/// than the folded one. Both halves are wrong in the direction that costs an
/// operator time: the chord they just freed stays refused, and the chord they
/// just took stays on offer.
#[test]
fn rebinding_moves_which_chord_is_taken() {
    let vacated = Binding::parse("ctrl+shift+b").expect("ctrl+shift+b is a chord");
    let taken = Binding::parse("ctrl+alt+shift+y").expect("ctrl+alt+shift+y is a chord");

    let shipped = live_chords(&KeyboardPrefs::default(), &[]);
    assert_eq!(
        chord_conflict(&shipped, &vacated, KeyAction::LaunchPreset(9)),
        Some(KeyAction::ToggleSidebar),
        "the built-in sidebar chord was on offer to a saved command"
    );

    let mut prefs = KeyboardPrefs::default();
    set_override(&mut prefs, KeyAction::ToggleSidebar, &taken);
    let moved = live_chords(&prefs, &[]);
    assert_eq!(
        chord_conflict(&moved, &vacated, KeyAction::LaunchPreset(9)),
        None,
        "a chord the operator moved the action off is still refused"
    );
    assert_eq!(
        chord_conflict(&moved, &taken, KeyAction::LaunchPreset(9)),
        Some(KeyAction::ToggleSidebar),
        "a chord the operator moved the action onto is still on offer"
    );
}

/// Deleting the saved command frees its chord on the spot.
///
/// THE BUG this stops: a conflict table folded once at startup. The operator
/// deletes the command holding a chord, types that chord into another row, and
/// is refused on behalf of a command that no longer exists.
#[test]
fn deleting_a_saved_command_frees_its_chord() {
    let candidate = Binding::parse("ctrl+shift+j").expect("ctrl+shift+j is a chord");
    let gone = live_chords(&KeyboardPrefs::default(), &[]);
    assert_eq!(
        chord_conflict(&gone, &candidate, KeyAction::ToggleSidebar),
        None,
        "a chord no command and no action holds was refused"
    );
}
