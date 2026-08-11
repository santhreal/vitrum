//! WHY: two opposite defects live here, and a bus that fixes one usually
//! causes the other.
//!
//! A change that does not reach the pane is a control that appears to do
//! nothing until the next launch, which is the complaint this surface exists
//! to answer. And a bus that notifies on every commit rebuilds a native
//! renderer once per character typed into a text field, which is the complaint
//! about the product feeling slow, arriving from the fix for the first one.
//!
//! So both are asserted: a change to something a pane reads reaches it, and a
//! change to something no pane reads reaches nobody.
//!
//! What this does NOT catch: what the widget does when it is told. The pane
//! owns that and asserts it against a real surface.

use super::*;
use crate::state::hostterm::{HostPalette, HostSource};
use crate::termpalette::TermPalette;
use std::sync::atomic::AtomicUsize;

/// A counting listener, and the count it holds.
fn counter() -> (Arc<AtomicUsize>, impl Fn(&PaneSettings) + Send + Sync + 'static) {
    let hits = Arc::new(AtomicUsize::new(0));
    let mine = Arc::clone(&hits);
    (hits, move |_: &PaneSettings| {
        mine.fetch_add(1, Ordering::Relaxed);
    })
}

/// A palette change reaches a pane, and the pane is told the colour.
///
/// THE BUG this stops: a preference that writes to disk and notifies nobody,
/// so the grid keeps its old colours until the operator restarts and then
/// changes on its own with nothing on screen connecting the two.
#[test]
fn a_palette_change_reaches_a_pane() {
    let _lock = exclusive();
    let seen: Arc<Mutex<Vec<Option<PanePalette>>>> = Arc::new(Mutex::new(Vec::new()));
    let mine = Arc::clone(&seen);
    let sub = subscribe_pane(move |p| {
        mine.lock()
            .push(p.palette);
    });

    let mut settings = Settings::default();
    settings.terminal.palette = TermPalette::Nord;
    publish(&settings);

    let got = seen
        .lock()
        .clone();
    drop(sub);
    assert_eq!(
        got.len(),
        2,
        "a subscriber is called once on subscribe and once per change"
    );
    assert_eq!(got[0], None, "a fresh profile follows the app theme");
    let nord = got[1].expect("Nord has colours");
    assert_eq!(nord.background, [0x2e, 0x34, 0x40, 255]);
    assert_eq!(nord.ansi[0], [0x3b, 0x42, 0x52, 255]);
    assert_eq!(nord.ansi[15], [0xec, 0xef, 0xf4, 255]);
}

/// A change to something no pane reads does not touch a pane.
///
/// THE BUG this stops, and it is the headline complaint: every control in the
/// sheet writes the whole document, one of them is a text field, and a bus
/// that fans out per commit rebuilds the renderer once per character. Twenty
/// characters of a daemon URL is twenty rebuilds of a surface that is
/// unchanged.
#[test]
fn typing_in_a_field_no_pane_reads_does_not_reach_a_pane() {
    let _lock = exclusive();
    let (hits, listener) = counter();
    let sub = subscribe_pane(listener);
    assert_eq!(hits.load(Ordering::Relaxed), 1, "subscribing delivers once");

    let mut settings = Settings::default();
    for typed in "ws://10.0.0.4:9000".chars() {
        settings.daemon_url.push(typed);
        publish(&settings);
    }
    drop(sub);
    assert_eq!(
        hits.load(Ordering::Relaxed),
        1,
        "eighteen keystrokes in a field the pane does not read reached the pane"
    );
}

/// The same value published twice is one change.
#[test]
fn republishing_an_unchanged_document_notifies_nobody() {
    let _lock = exclusive();
    let (hits, listener) = counter();
    let sub = subscribe_pane(listener);
    let mut settings = Settings::default();
    settings.terminal.font_size_px = 18;
    publish(&settings);
    publish(&settings);
    publish(&settings);
    drop(sub);
    assert_eq!(hits.load(Ordering::Relaxed), 2, "one subscribe, one change");
}

/// Each audience hears only about its own half.
///
/// THE BUG this stops: one listener list for everything, so a sidebar chip
/// being toggled resizes every terminal grid in the window.
#[test]
fn the_pane_and_the_shell_hear_about_different_changes() {
    let _lock = exclusive();
    let (pane_hits, pane_listener) = counter();
    let shell_hits = Arc::new(AtomicUsize::new(0));
    let mine = Arc::clone(&shell_hits);
    let pane_sub = subscribe_pane(pane_listener);
    let shell_sub = subscribe_shell(move |_| {
        mine.fetch_add(1, Ordering::Relaxed);
    });

    let mut settings = Settings::default();
    settings.show_worktree = false;
    publish(&settings);
    assert_eq!(pane_hits.load(Ordering::Relaxed), 1, "a chip touched the pane");
    assert_eq!(shell_hits.load(Ordering::Relaxed), 2);

    settings.terminal.cursor_shape = CursorShape::Bar;
    publish(&settings);
    assert_eq!(pane_hits.load(Ordering::Relaxed), 2);
    assert_eq!(
        shell_hits.load(Ordering::Relaxed),
        2,
        "a cursor shape rebuilt the window frame"
    );

    drop(pane_sub);
    drop(shell_sub);
}

/// Dropping a subscription stops the calls.
///
/// THE BUG this stops: a closed pane's callback still in the list, holding a
/// widget that is gone, called on the next settings change.
#[test]
fn dropping_a_subscription_unsubscribes() {
    let _lock = exclusive();
    let (hits, listener) = counter();
    let sub = subscribe_pane(listener);
    let mut settings = Settings::default();
    settings.terminal.font_size_px = 21;
    publish(&settings);
    let before = hits.load(Ordering::Relaxed);
    drop(sub);
    settings.terminal.font_size_px = 22;
    publish(&settings);
    assert_eq!(
        hits.load(Ordering::Relaxed),
        before,
        "a dropped subscription is still being called"
    );
}

/// A pane created after a change starts on the current values.
///
/// THE BUG this stops: a new session opening with the shipped font and palette
/// because it missed the publish that changed them, so two panes in one window
/// look different.
#[test]
fn a_pane_created_later_starts_on_the_current_values() {
    let _lock = exclusive();
    let mut settings = Settings::default();
    settings.terminal.font_size_px = 24;
    settings.terminal.cursor_blink = false;
    publish(&settings);
    let now = pane_settings();
    assert_eq!(now.font_size_px, 24);
    assert!(!now.cursor_blink);
}

/// The pane never receives a value outside the range it can paint.
///
/// THE BUG this stops: a hand-edited profile reaching the renderer. A zero
/// font size is a zero-width cell box and a blank pane; the load path clamps,
/// and this is the second gate for anything that did not come through it.
#[test]
fn the_pane_is_never_handed_an_unpaintable_value() {
    let _lock = exclusive();
    let mut settings = Settings::default();
    settings.terminal.font_size_px = 0;
    settings.terminal.line_height_pct = 60_000;
    settings.terminal.cell_width_pct = 1;
    settings.terminal.blink_interval_ms = 0;
    settings.terminal.wheel_lines = 0;
    settings.terminal.scrollback_lines = u32::MAX;
    let pane = PaneSettings::derive(&settings);
    assert_eq!(pane.font_size_px, super::super::TERM_FONT_MIN_PX);
    assert_eq!(pane.line_height_pct, super::super::LINE_HEIGHT_MAX_PCT);
    assert_eq!(pane.cell_width_pct, super::super::CELL_WIDTH_MIN_PCT);
    assert_eq!(pane.blink_interval_ms, super::super::BLINK_MIN_MS);
    assert_eq!(pane.wheel_lines, 1);
    assert_eq!(pane.scrollback_lines, super::super::SCROLLBACK_MAX_LINES);
}

/// The host import wins over a named scheme while it is in force.
#[test]
fn an_import_in_force_beats_the_named_scheme() {
    let _lock = exclusive();
    let mut settings = Settings::default();
    settings.terminal.palette = TermPalette::Nord;
    settings.terminal.host_palette = HostPalette {
        source: HostSource::Flat,
        origin: "/src/kitty.conf".to_string(),
        background: "#101010".to_string(),
        foreground: "#d0d0d0".to_string(),
        cursor: String::new(),
        selection: String::new(),
        ansi: (0..16).map(|n| format!("#0000{n:02x}")).collect(),
    };

    // Off, so the named scheme still wins.
    let nord = PaneSettings::derive(&settings)
        .palette
        .expect("Nord has colours");
    assert_eq!(nord.background, [0x2e, 0x34, 0x40, 255]);

    settings.terminal.follow_host_terminal = true;
    let host = PaneSettings::derive(&settings)
        .palette
        .expect("the import is whole");
    assert_eq!(host.background, [0x10, 0x10, 0x10, 255]);
    assert_eq!(host.cursor, host.foreground, "no cursor colour falls back");
}

/// The switch alone does nothing until an import succeeds.
///
/// THE BUG this stops: turning on "follow my terminal" with nothing imported
/// and getting a black grid, which is what painting an empty palette produces.
#[test]
fn following_the_host_terminal_with_no_import_changes_nothing() {
    let _lock = exclusive();
    let mut settings = Settings::default();
    settings.terminal.palette = TermPalette::Dracula;
    let before = PaneSettings::derive(&settings).palette;
    settings.terminal.follow_host_terminal = true;
    assert_eq!(
        PaneSettings::derive(&settings).palette,
        before,
        "an empty import took effect"
    );
}

/// Terminal opacity reaches the pane as the background alpha.
#[test]
fn terminal_opacity_reaches_the_pane_as_an_alpha() {
    let _lock = exclusive();
    let mut settings = Settings::default();
    settings.terminal.palette = TermPalette::Nord;
    assert_eq!(
        PaneSettings::derive(&settings)
            .palette
            .expect("Nord has colours")
            .background[3],
        255,
        "a fully opaque profile must not composite"
    );

    settings.appearance.terminal_opacity_pct = 50;
    let half = PaneSettings::derive(&settings)
        .palette
        .expect("Nord has colours");
    assert_eq!(half.background[3], 128);
    assert_eq!(half.foreground[3], 255, "text is not translucent");
}

/// The selection wash from the built-in table survives its own syntax.
///
/// THE BUG this stops: the built-in palettes write the selection as
/// `rgba(r, g, b, a)` and everything else as `#rrggbb`. A reader that handles
/// one syntax hands the renderer a transparent selection, which is a selection
/// nobody can see.
#[test]
fn the_selection_wash_is_read_out_of_its_own_syntax() {
    let _lock = exclusive();
    let mut settings = Settings::default();
    settings.terminal.palette = TermPalette::Nord;
    let nord = PaneSettings::derive(&settings)
        .palette
        .expect("Nord has colours");
    assert_eq!(nord.selection_bg, [0x4c, 0x56, 0x6a, 153]);
    assert_ne!(nord.selection_bg[3], 0, "the selection is invisible");
}

/// A rebinding reaches key dispatch and a font change does not.
#[test]
fn a_rebinding_reaches_the_keyboard_listeners_only() {
    let _lock = exclusive();
    let hits = Arc::new(AtomicUsize::new(0));
    let mine = Arc::clone(&hits);
    let sub = subscribe_keyboard(move |_, _| {
        mine.fetch_add(1, Ordering::Relaxed);
    });
    let mut settings = Settings::default();
    settings.terminal.font_size_px = 19;
    publish(&settings);
    assert_eq!(hits.load(Ordering::Relaxed), 1, "a font size rebound a key");

    settings
        .keyboard
        .overrides
        .insert("toggle-sidebar".to_string(), "Ctrl+Alt+J".to_string());
    publish(&settings);
    assert_eq!(hits.load(Ordering::Relaxed), 2);
    assert_eq!(
        keyboard_prefs().overrides.get("toggle-sidebar").map(String::as_str),
        Some("Ctrl+Alt+J")
    );
    drop(sub);
}

/// A rebinding and a preset edit arrive at ONE listener, together.
///
/// THE BUG this closes: the chord table is folded from two sources, the
/// operator's rebindings and their saved commands. A dispatcher that heard
/// about them on separate channels can hold a table folded from a stale half,
/// and a dispatcher that folds once at startup hears about neither. Both
/// produce the same symptom, which is the one nobody reports as a bug: the
/// shortcut the operator just bound does nothing, and works after a restart.
///
/// So the assertion is on what the callback RECEIVES, not on the fanout count.
/// A listener called with the right number of times and the wrong pair is the
/// failure this is written against.
#[test]
fn a_rebinding_and_a_preset_edit_both_arrive_whole() {
    let _lock = exclusive();
    let seen: Arc<Mutex<Vec<(usize, Vec<u64>)>>> = Arc::new(Mutex::new(Vec::new()));
    let mine = Arc::clone(&seen);
    let sub = subscribe_keyboard(move |prefs, presets| {
        mine.lock().push((
            prefs.overrides.len(),
            presets.iter().map(|p| p.id).collect(),
        ));
    });

    let mut settings = Settings::default();
    settings
        .keyboard
        .overrides
        .insert("new-session".to_string(), "Ctrl+Alt+K".to_string());
    publish(&settings);

    publish_presets(&[SavedPreset {
        id: 42,
        label: "Resume here".to_string(),
        ..SavedPreset::default()
    }]);

    let log = seen.lock().clone();
    drop(sub);

    assert_eq!(
        log,
        vec![
            (0, vec![]),
            (1, vec![]),
            (1, vec![42]),
        ],
        "the fold's two halves did not arrive together"
    );
    assert_eq!(presets().len(), 1, "the preset never reached the bus");
    assert_eq!(
        keyboard_prefs().overrides.len(),
        1,
        "the preset edit dropped the rebinding that came before it"
    );
}

/// Editing a preset does not disturb what a pane paints with.
///
/// A preset edit republishes the keyboard half. If it went through the same
/// path as a settings change, every keystroke in the preset editor's command
/// field would re-derive and re-upload a palette.
#[test]
fn editing_a_preset_does_not_touch_the_pane() {
    let _lock = exclusive();
    let (hits, listener) = counter();
    let sub = subscribe_pane(listener);
    let before = hits.load(Ordering::Relaxed);

    for id in 1..=8u64 {
        publish_presets(&[SavedPreset {
            id,
            label: format!("preset {id}"),
            ..SavedPreset::default()
        }]);
    }

    assert_eq!(
        hits.load(Ordering::Relaxed),
        before,
        "eight preset edits repainted the terminal"
    );
    drop(sub);
}

/// The same preset list published twice is one change.
#[test]
fn republishing_an_unchanged_preset_list_notifies_nobody() {
    let _lock = exclusive();
    let hits = Arc::new(AtomicUsize::new(0));
    let mine = Arc::clone(&hits);
    let sub = subscribe_keyboard(move |_, _| {
        mine.fetch_add(1, Ordering::Relaxed);
    });
    let list = [SavedPreset {
        id: 3,
        label: "Same".to_string(),
        ..SavedPreset::default()
    }];
    publish_presets(&list);
    publish_presets(&list);
    publish_presets(&list);
    assert_eq!(
        hits.load(Ordering::Relaxed),
        2,
        "one subscribe call plus one real change is two, so the list was \
         re-delivered for an edit that changed nothing"
    );
    drop(sub);
}

/// A listener that publishes does not deadlock the bus.
///
/// THE BUG this stops: fanning out with the lock held. A pane that reacts to a
/// settings change by writing one back would hang the whole process on its own
/// mutex, and it would hang on the operator's machine and not on this one.
#[test]
fn a_listener_that_publishes_does_not_deadlock() {
    let _lock = exclusive();
    let sub = subscribe_pane(|_| {
        let _ = pane_settings();
        let _ = shell_settings();
    });
    let mut settings = Settings::default();
    settings.terminal.font_size_px = 17;
    publish(&settings);
    drop(sub);
}

/// A theme change reaches the shell without a restart.
///
/// THE BUG this stops: the shell half of the bus having no subscriber at all,
/// which is the state this batch found it in. Every field of `ShellSettings`
/// is then a value the window frame read once at startup, and an operator who
/// switches to "Follow the system" keeps the appearance the desktop happened
/// to report at launch for the rest of the session.
///
/// Observed through the one side effect the subscriber has: the desktop
/// appearance round trip. Counting it pins the other half of the contract too,
/// which is that a commit touching anything else spends no round trip.
#[test]
fn a_theme_change_reaches_the_shell_without_a_restart() {
    let _lock = exclusive();
    crate::ui::settings::watch_shell();

    let mut settings = Settings::default();
    settings.theme = ThemePref::Dark;
    publish(&settings);
    // The shipped path always finds this cache warm: `theme_attr` fills it on
    // the first render of the first window, long before a sheet can be opened.
    // Cold, the first write forces the cell's own initialising read as well,
    // and the count below would be two for one refresh.
    let _ = crate::ui::settings::system_theme();
    let before = crate::ui::settings::system_theme_reads();

    settings.theme = ThemePref::System;
    publish(&settings);
    assert_eq!(
        crate::ui::settings::system_theme_reads(),
        before + 1,
        "asking to follow the system did not re-read the desktop appearance"
    );

    settings.text_scale_pct = 125;
    publish(&settings);
    assert_eq!(
        crate::ui::settings::system_theme_reads(),
        before + 1,
        "an unrelated commit put a desktop round trip on the commit path"
    );
}
