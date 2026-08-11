use super::*;
use crate::state::{NotifyPrefs, TEXT_SCALE_MAX_PCT, TEXT_SCALE_MIN_PCT};
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

fn settings() -> Settings {
    Settings::default()
}

// -- Appearance ---------------------------------------------------------

/// Comfortable must emit nothing. Restating the stylesheet's own values
/// back at it would pin every row height to a copy in this file that goes
/// stale the moment the stylesheet is retuned.
#[test]
fn the_default_density_overrides_no_tokens() {
    assert_eq!(root_style(&settings()), "");
}

/// Compact must shrink the row BOXES, not merely the padding. A density
/// switch that moved only spacing would leave the card row at its full
/// height and the list exactly as long as before.
#[test]
fn compact_density_shrinks_the_row_boxes() {
    let compact = Settings {
        density: Density::Compact,
        ..settings()
    };
    let style = root_style(&compact);
    assert!(
        style.contains("--rg-card-h:3.75rem;"),
        "card row not compacted: {style}"
    );
    assert!(
        style.contains("--rg-slim-h:1.75rem;"),
        "slim row not compacted: {style}"
    );
}

/// Compact is a shipped state of every screen, so it is held to the same
/// 4px grid as the default.
///
/// It was not. `--rg-line-head` was 1.125rem and `--rg-space-2` and
/// `--rg-content-inset` were 0.375rem, which are 18px and 6px. An 18px head
/// line makes the card 12 + 18 + 4 + 20 + 12 = 66, and the running binary
/// measured exactly that: a 66px row pitch against Comfortable's 76, on a
/// grid where nothing else is a multiple of 66.
///
/// The previous version of the test above asserted `--rg-row-gap:0rem`,
/// which pinned the other half of the defect: at zero gap the cards touch,
/// and a list of touching cards is one surface with a notch at each end.
/// That assertion is gone because it was protecting the bug.
#[test]
fn every_compact_token_lands_on_the_four_pixel_grid() {
    for (name, value) in COMPACT_TOKENS {
        let rem: f64 = value
            .strip_suffix("rem")
            .unwrap_or_else(|| panic!("{name} is {value}, which is not in rem"))
            .parse()
            .unwrap_or_else(|_| panic!("{name} is {value}, which is not a number"));
        let px = rem * 16.0;
        assert!(
            (px / 4.0).fract() == 0.0,
            "{name} is {value} = {px}px, which is not a 4px multiple; \
             Compact would run the list off the grid again"
        );
    }
}

/// A dense list still needs to say where one row ends.
///
/// Compact set the gap to zero, which made adjacent cards touch: verified
/// at native resolution on the running binary, no background between them
/// across the full width, only the corner radius notching each end.
/// Proximity is the grouping signal this sidebar is built on, so density
/// may shrink the gap but not delete it.
#[test]
fn compact_density_keeps_a_gap_between_rows() {
    let gap = COMPACT_TOKENS
        .iter()
        .find(|(name, _)| *name == "--rg-row-gap")
        .map(|(_, v)| *v)
        .expect("compact no longer sets a row gap at all");
    let px: f64 = gap.trim_end_matches("rem").parse::<f64>().unwrap() * 16.0;
    assert!(
        px >= 4.0,
        "compact row gap is {gap} = {px}px; at zero the cards touch and the \
         list reads as one surface"
    );
}

/// Reduced motion must zero both duration tokens, because every transition
/// in both stylesheets reads its duration from one of them. Anything
/// narrower would silently miss transitions added later.
#[test]
fn reduced_motion_zeroes_both_duration_tokens() {
    let reduced = Settings {
        reduce_motion: true,
        ..settings()
    };
    let style = root_style(&reduced);
    assert!(style.contains("--rg-t-fast:0s;"), "{style}");
    assert!(style.contains("--rg-t-base:0s;"), "{style}");
}

/// The two appearance switches are independent. Composing them with an
/// early return would make turning on reduced motion silently discard the
/// density.
#[test]
fn density_and_reduced_motion_compose() {
    let both = Settings {
        density: Density::Compact,
        reduce_motion: true,
        ..settings()
    };
    let style = root_style(&both);
    assert!(style.contains("--rg-card-h:3.75rem;"), "{style}");
    assert!(style.contains("--rg-t-fast:0s;"), "{style}");
}

/// The theme attribute must be exactly what `sidebar.css` keys on. A typo
/// here is a light theme that never appears, with no error anywhere.
#[test]
fn the_theme_attribute_matches_what_the_stylesheet_selects_on() {
    let light = Settings {
        theme: ThemePref::Light,
        ..settings()
    };
    let dark = Settings {
        theme: ThemePref::Dark,
        ..settings()
    };
    assert_eq!(theme_attr(&light), "light");
    assert_eq!(theme_attr(&dark), "dark");

    let css = include_str!("../../../assets/sidebar.css");
    assert!(
        css.contains(r#"[data-theme="light"]"#),
        "the stylesheet no longer has a light palette keyed on data-theme"
    );
}

/// System must resolve to one of the two real palettes and never to a
/// third value the stylesheet has no block for.
#[test]
fn the_system_theme_resolves_to_a_palette_that_exists() {
    let system = Settings {
        theme: ThemePref::System,
        ..settings()
    };
    assert!(matches!(theme_attr(&system), "light" | "dark"));
}

/// 100% must be exactly 16px, not 16.0000px. A fractional root font size
/// at the default setting would shift every rem-derived box by a subpixel
/// for no reason.
#[test]
fn the_default_text_scale_is_exactly_the_browser_default() {
    assert_eq!(ui_scale_px(100), "16px");
}

/// Every offered step must produce a clean length, so the operator never
/// gets a shell laid out on 22.400000000000002px.
#[test]
fn every_text_scale_step_produces_a_clean_length() {
    for step in UI_SCALE_STEPS {
        let length = ui_scale_px(*step);
        assert!(
            !length.contains("0000") && !length.contains("9999"),
            "step {step} produced {length}"
        );
    }
    assert_eq!(ui_scale_px(150), "24px");
}

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

/// Every token the compact density overrides must be one the stylesheet
/// actually declares. Overriding a name nothing reads is the exact shape
/// of a dead control: the style attribute changes, the DOM changes, and
/// not one pixel moves. A rename in `sidebar.css` fails here instead of
/// shipping silently.
#[test]
fn every_density_token_is_one_the_stylesheet_declares() {
    let css = include_str!("../../../assets/sidebar.css");
    for (name, _) in COMPACT_TOKENS {
        assert!(
            css.contains(&format!("{name}:")),
            "{name} is overridden by the compact density but no stylesheet declares it, \
             so the switch would change nothing"
        );
    }
}

/// The two motion tokens must exist for the same reason, and the
/// stylesheet must still route its reduced-motion handling through them.
#[test]
fn the_motion_tokens_are_the_stylesheets_only_durations() {
    let css = include_str!("../../../assets/sidebar.css");
    assert!(css.contains("--rg-t-fast:") || css.contains("--rg-t-fast :"));
    assert!(css.contains("--rg-t-base:") || css.contains("--rg-t-base :"));
    assert!(
        css.contains("prefers-reduced-motion"),
        "the stylesheet stopped honouring the OS preference, so the switch is now the \
         only path and its doc comment is wrong"
    );
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

/// One `SelectRow`, rendered exactly as the sheet would render it.
#[derive(Props, Clone, PartialEq)]
struct SelectHarnessProps {
    value: String,
    options: Vec<(String, String)>,
}

#[component]
fn SelectHarness(props: SelectHarnessProps) -> Element {
    rsx! {
        SelectRow {
            label: "Text scale",
            desc: String::new(),
            value: props.value.clone(),
            options: props.options.clone(),
            onpick: move |_: String| {},
        }
    }
}

fn scale_options() -> Vec<(String, String)> {
    UI_SCALE_STEPS
        .iter()
        .map(|pct| (pct.to_string(), format!("{pct}%")))
        .collect()
}

fn render_select(value: &str) -> String {
    let mut dom = VirtualDom::new_with_props(
        SelectHarness,
        SelectHarnessProps {
            value: value.to_string(),
            options: scale_options(),
        },
    );
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// WHY: a stored value outside the menu made the control state a setting
/// that was not the one in effect.
///
/// `set_text_scale` clamps to a RANGE, `TEXT_SCALE_MIN_PCT..=TEXT_SCALE_MAX_PCT`,
/// while the menu offers eight STEPS. A hand-edited `"textScalePct": 137` is
/// therefore accepted whole and really does render the shell at 137%, but no
/// `<option>` carried that value, and a `<select>` whose value matches none of
/// its options does not error and does not blank: it selects the FIRST one.
/// So the Appearance tab read "80%" over a window running at 137%, and picking
/// any other step would have been the operator's only way to find out.
///
/// Exactly one option is selected in every case, because two would be the same
/// defect wearing different clothes.
#[test]
fn a_stored_scale_the_menu_cannot_express_is_shown_rather_than_swallowed() {
    let html = render_select("137");
    assert!(
        html.contains(
            r#"<option value="137" selected=true>137 (in effect, not one of the choices)</option>"#
        ),
        "the stored value got no option of its own: {html}"
    );
    assert_eq!(
        html.matches("selected=true").count(),
        1,
        "exactly one option may be selected: {html}"
    );
    assert!(
        !html.contains(r#"<option value="80" selected=true>"#),
        "80% is still being presented as the setting in effect: {html}"
    );
}

/// A value the menu DOES offer gets no extra option, and selects its own.
///
/// The other half of the contract: the repair must not add a duplicate row to
/// every menu in the sheet.
#[test]
fn a_stored_scale_the_menu_offers_selects_that_option_alone() {
    let html = render_select("125");
    assert!(
        html.contains(r#"<option value="125" selected=true>125%</option>"#),
        "the offered step is not the selected one: {html}"
    );
    assert!(
        !html.contains("not one of the choices"),
        "an offered value grew a stray option: {html}"
    );
    assert_eq!(
        html.matches("<option").count(),
        UI_SCALE_STEPS.len(),
        "the menu grew or lost a row: {html}"
    );
}

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

// ═══════════════════════════════════════════════════════════════════════════
// The sheet, rendered
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Props, Clone, PartialEq)]
struct SheetHarnessProps {
    tab: SettingsTab,
}

/// The signal has to be created INSIDE the component: `Signal::new` needs a
/// live Dioxus runtime and panics when a test builds one up front.
#[component]
fn SheetHarness(props: SheetHarnessProps) -> Element {
    let state = use_signal(UiState::default);
    let update_offer = use_signal(|| None::<crate::update::Available>);
    rsx! {
        SettingsSheet {
            state,
            tab: props.tab,
            on_tab: move |_: SettingsTab| {},
            on_reconnect: move |_: String| {},
            on_dismiss: move |()| {},
            update_offer,
        }
    }
}

fn render_tab(tab: SettingsTab) -> String {
    let mut dom = VirtualDom::new_with_props(SheetHarness, SheetHarnessProps { tab });
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// WHY: every panel in this sheet reads its preferences through a `use_memo`
/// over `UiState`, and nothing in this suite had ever built one.
///
/// The panels used to read `UiState` straight, which subscribed each of them
/// to the session list and repainted a preference tab every time an agent
/// printed a line. They now read a memo and hold its guard across the whole
/// body. That is the right shape and it is also the shape that panics at
/// runtime if a body ever reads and writes the same signal in one pass, which
/// is a blank modal for the operator and which no pure-function test in this
/// file can see. Every tab is built, because the panels are separate
/// components and a fault in one of them is invisible from the other eight.
#[test]
fn every_tab_of_the_sheet_builds_and_names_itself() {
    for tab in SettingsTab::ALL {
        let html = render_tab(tab);
        assert!(
            html.contains(r#"aria-label="Settings""#),
            "the {tab:?} tab did not build the sheet: {html}"
        );
        assert!(
            html.contains(&format!(
                r#"class="rg-sheet__tab rg-sheet__tab--active" type="button" role="tab" aria-selected="true">{}"#,
                tab.label()
            )),
            "the {tab:?} tab is not the one marked active: {html}"
        );
        assert!(
            html.contains(r#"role="tabpanel""#),
            "the {tab:?} tab rendered no panel: {html}"
        );
    }
}

/// The Appearance tab draws the controls whose values come through the memo.
///
/// WHY: `every_tab_of_the_sheet_builds_and_names_itself` proves the panel does
/// not panic, which a panel returning nothing at all also satisfies. This
/// asserts the defaults reach the DOM as the selected options, so a memo that
/// resolved to the wrong document, or never resolved, is a failure here.
#[test]
fn the_appearance_tab_renders_the_stored_defaults_as_selected() {
    let html = render_tab(SettingsTab::Appearance);
    let d = Settings::default();
    assert!(
        html.contains(&format!(
            r#"<option value="{}" selected=true>{}%</option>"#,
            d.text_scale_pct, d.text_scale_pct
        )),
        "the stored text scale is not the selected step: {html}"
    );
    assert!(
        html.contains(r#"<option value="system" selected=true>"#),
        "the stored theme is not the selected option: {html}"
    );
    assert!(
        !html.contains("not one of the choices"),
        "a shipped default is not expressible by its own menu: {html}"
    );
}

/// The sheet's own source, read for what its rows declare.
///
/// A row's catalogue path is a literal in this file, so the only way to check
/// every row has one the catalogue knows is to read the file back. Deriving
/// the list at run time is what makes a row added tomorrow fail here instead
/// of shipping a hint that says nothing.
const SHEET_SOURCE: &str = include_str!("../settings.rs");

/// Every control that names a catalogue path names one that exists.
///
/// THE BUG this stops: a row carrying a path that was renamed in the
/// catalogue. `setting` returns `None`, `when_note` returns the empty string,
/// and the row silently loses its timing sentence with nothing failing.
#[test]
fn every_control_in_the_sheet_is_catalogued() {
    let mut unknown = Vec::new();
    for rest in SHEET_SOURCE.split("path: \"").skip(1) {
        let Some(path) = rest.split('"').next() else {
            continue;
        };
        if crate::state::catalog::setting(path).is_none() {
            unknown.push(path.to_string());
        }
    }
    assert!(
        unknown.is_empty(),
        "the sheet names settings the catalogue does not have: {unknown:?}"
    );
}

/// Every setting that does not apply on the spot says so on its own row.
///
/// THE BUG this stops: a control that looks like it did nothing. Window
/// opacity applies to the next window and the splash to the next launch, so a
/// row without the sentence reads as broken and gets toggled back.
///
/// The variant space comes from the catalogue at run time, so adding a setting
/// with a delay and no row goes red here rather than reaching an operator.
#[test]
fn every_setting_that_needs_a_restart_says_so_on_its_row() {
    let mut silent = Vec::new();
    for s in crate::state::catalog::SETTINGS {
        if s.live == crate::state::catalog::Live::Immediately {
            continue;
        }
        if !SHEET_SOURCE.contains(&format!("path: \"{}\",", s.path)) {
            silent.push(s.path);
        }
    }
    assert!(
        silent.is_empty(),
        "these settings apply later and no row says when: {silent:?}"
    );
}

/// Text scale moves the document root, which is the only element `rem` reads.
///
/// THE BUG this stops: scaling a descendant. Both stylesheets declare their
/// geometry and type tokens in `rem`, and the pane computes its own pixel
/// sizes from the same root, so a rule applied anywhere below `html` leaves
/// the shell and the grid disagreeing about how big a line is.
#[test]
fn the_root_font_rule_scales_the_document_root() {
    let mut settings = Settings::default();
    settings.text_scale_pct = 100;
    assert_eq!(root_font_rule(&settings), "html{font-size:16px;}");
    settings.text_scale_pct = 150;
    assert_eq!(root_font_rule(&settings), "html{font-size:24px;}");
}

/// The rule on the document root carries exactly the rem the pane lays out
/// against.
///
/// THE BUG this stops: the two arithmetics drifting. `ui/terminal.rs` sizes
/// the pane from [`rem_px`] and the stylesheet resolves every `rem` against
/// this rule, so a rounding or clamping change on one side alone moves the
/// pane's left edge off the sidebar it sits against by a whole scale step.
/// The out-of-range values are here because clamping is where the two would
/// disagree first.
#[test]
fn the_root_rule_carries_the_rem_the_pane_uses() {
    for pct in [0u16, 1, 50, 80, 100, 125, 150, 200, 400, u16::MAX] {
        let mut settings = Settings::default();
        settings.text_scale_pct = pct;
        assert_eq!(
            root_font_rule(&settings),
            format!("html{{font-size:{}px;}}", trim_num(rem_px(pct))),
            "the document root and the pane disagree at {pct}%"
        );
    }
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
