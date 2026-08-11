//! What a control on this surface reaches, and how fast.
//!
//! The sheet's whole claim is that a preference takes effect on the spot. The
//! claim has three halves, because three consumers read a different derivation
//! of one document: the pane reads [`live::PaneSettings`], the window frame
//! reads [`live::ShellSettings`], and key dispatch reads the keyboard prefs
//! alongside the saved commands. A row whose setter writes the wrong field
//! still saves, still redraws, and still does nothing until a restart, and
//! that is the failure these tests exist to catch.
//!
//! Every test drives the SETTER OFF THE ROW rather than assigning the field by
//! hand. Assigning the field would prove the bus works and prove nothing about
//! the control, which is the half that goes wrong.
//!
//! No widget is built here. GTK needs a display, the rig has none, and none of
//! what is asserted below is drawn.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::state::Settings;
use crate::state::live;
use crate::ui::settings::spec::{Control, Row, all_rows};

/// The row that edits `path`, or a failure naming what the registry holds.
///
/// Looked up rather than indexed, because a row that moves between pages must
/// not silently stop being tested.
fn row(path: &'static str) -> &'static Row {
    all_rows()
        .find(|row| row.path == path)
        .unwrap_or_else(|| panic!("no row edits {path}"))
}

/// Apply a menu row's setter, the way the sheet's own handler does.
fn choose(settings: &mut Settings, path: &'static str, value: &str) {
    let Control::Choice { set, options, .. } = &row(path).control else {
        panic!("{path} is not a menu");
    };
    let offered = options();
    assert!(
        offered.iter().any(|(v, _)| v == value),
        "{path} does not offer {value}; it offers {:?}",
        offered.iter().map(|(v, _)| v).collect::<Vec<_>>()
    );
    set(settings, value);
}

/// Apply a switch row's setter, the way the sheet's own handler does.
fn flip(settings: &mut Settings, path: &'static str, on: bool) {
    let Control::Switch { set, .. } = &row(path).control else {
        panic!("{path} is not a switch");
    };
    set(settings, on);
}

#[test]
fn a_terminal_row_reaches_the_pane_without_a_restart() {
    let _lease = live::exclusive();
    let mut settings = Settings::default();
    live::publish(&settings);
    let seen: Arc<parking_lot::Mutex<Vec<u16>>> = Arc::default();
    let record = Arc::clone(&seen);
    let _sub = live::subscribe_pane(move |pane| record.lock().push(pane.font_size_px));

    let before = live::pane_settings().font_size_px;
    let target = (before + 3).to_string();
    choose(&mut settings, "terminal.fontSizePx", &target);
    live::publish(&settings);

    assert_eq!(
        live::pane_settings().font_size_px,
        before + 3,
        "the row's setter did not reach the pane's derivation"
    );
    assert_eq!(
        seen.lock().last().copied(),
        Some(before + 3),
        "the pane was never told; it would keep the old size until a restart"
    );
}

#[test]
fn a_shell_row_reaches_the_frame_without_a_restart() {
    let _lease = live::exclusive();
    let mut settings = Settings::default();
    live::publish(&settings);

    let seen: Arc<parking_lot::Mutex<Vec<crate::state::Density>>> = Arc::default();
    let record = Arc::clone(&seen);
    let _sub = live::subscribe_shell(move |shell| record.lock().push(shell.density));

    // The density that is not in force, so the assertion cannot pass on a
    // publish that changed nothing.
    let before = live::shell_settings().density;
    let (target, wire) = if before == crate::state::Density::Compact {
        (crate::state::Density::Comfortable, "comfortable")
    } else {
        (crate::state::Density::Compact, "compact")
    };
    choose(&mut settings, "density", wire);
    live::publish(&settings);

    assert_eq!(
        live::shell_settings().density,
        target,
        "the row's setter did not reach the frame's derivation"
    );
    assert_eq!(
        seen.lock().last().copied(),
        Some(target),
        "the frame was never told; the sidebar would keep the old spacing"
    );
}

#[test]
fn a_rebinding_reaches_key_dispatch_without_a_restart() {
    let _lease = live::exclusive();
    let mut settings = Settings::default();
    live::publish(&settings);
    live::publish_presets(&[]);

    let fanouts = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&fanouts);
    let _sub = live::subscribe_keyboard(move |_, _| {
        count.fetch_add(1, Ordering::Relaxed);
    });
    // One arrival for the current value, which is the subscription's own
    // contract. Everything after it is a change.
    let at_subscribe = fanouts.load(Ordering::Relaxed);

    let action = crate::keymap::KeyAction::NextTab;
    let binding = crate::ui::settings::Binding {
        key: "f9".to_string(),
        ctrl: true,
        alt: false,
        shift: true,
    };
    crate::ui::settings::set_override(&mut settings.keyboard, action, &binding);
    live::publish(&settings);

    assert!(
        fanouts.load(Ordering::Relaxed) > at_subscribe,
        "dispatch was never told about the rebinding"
    );
    let live_prefs = live::keyboard_prefs();
    let moved = crate::ui::settings::effective_chords(&live_prefs)
        .into_iter()
        .find(|chord| chord.action == action)
        .expect("the action is in the table");
    assert_eq!(moved.rendered(), "Ctrl+Shift+F9");
    assert!(
        moved.rebound,
        "the chord is on the new keys but is not marked as moved, so the shortcuts overlay would \
         still advertise the default"
    );
}

#[test]
fn a_switch_row_reaches_the_frame_without_a_restart() {
    let _lease = live::exclusive();
    let mut settings = Settings::default();

    // Drive the switch to the value it is NOT at, so a setter that ignores its
    // argument cannot pass.
    let Control::Switch { get, .. } = &row("showBranch").control else {
        panic!("showBranch is not a switch");
    };
    let before = get(&settings);
    flip(&mut settings, "showBranch", !before);
    live::publish(&settings);

    assert_eq!(
        live::shell_settings().show_branch,
        !before,
        "the switch's setter did not reach the frame's derivation"
    );
}
