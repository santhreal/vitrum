//! One test per control: it changes something observable, and it survives the
//! file.
//!
//! Both halves matter and neither implies the other. A control that mutates
//! `Settings` but changes no derivation is a switch that does nothing; a
//! control that changes a derivation but is dropped by the serialiser is a
//! switch that does nothing after a restart, which is the same defect
//! arriving late. So every test here asserts a DERIVED value before and after,
//! then pushes the whole document through the exact encode/parse pair
//! `save_prefs` and `load_prefs` use and asserts the derived value again.
//!
//! Controls whose observable effect lives in another module are marked and
//! tested for the round trip only; the rendering half is asserted where the
//! markup is. Those are named in the module docs as well, so the gap is
//! visible without reading the tests.

use super::*;
use crate::state::{
    DaemonState, Grouping, Persisted, SettingsTab, UiStateLoad, WindowState, encode_ui_state,
    parse_ui_state,
};
use crate::termpalette::{TermPalette, css_tokens};
use vitrum_model::{Clock, Disposition, SessionView};
use vitrum_proto::{Attention, ProjectId, SessionId, SessionInfo, SessionStatus};

/// Push a whole document through the file format and read it back.
///
/// Deliberately the encode/parse pair rather than a plain serde round trip:
/// `parse_ui_state` normalises and repairs on the way in, so a value that
/// survives `to_string`/`from_str` can still be rewritten by the real load
/// path. This asserts against what the product actually does on startup.
fn through_the_file(daemon: &DaemonState, window: &WindowState) -> DaemonState {
    let doc = Persisted::capture(daemon, std::iter::once(window));
    let text = encode_ui_state(&doc);
    let back = match parse_ui_state(&text) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("the settings file did not read back: {other:?}"),
    };
    let mut restored = DaemonState::default();
    back.restore_daemon(&mut restored);
    restored
}

/// Apply one settings change and return the state as it comes back off
/// disk, so every test below is one call rather than six lines of
/// ceremony.
fn after_restart(change: impl FnOnce(&mut Settings)) -> Settings {
    let mut st = UiState::default();
    change(&mut st.daemon.settings);
    through_the_file(&st.daemon, &st.window).settings
}

fn info(id: u64, cwd: &str) -> SessionInfo {
    SessionInfo {
        id: SessionId(id),
        project_id: ProjectId(1),
        title: format!("session {id}"),
        cwd: cwd.to_string(),
        command: "claude".to_string(),
        args: Vec::new(),
        status: SessionStatus::Exited { code: Some(0) },
        created_at_ms: 1_000 + id,
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

// -- Appearance ---------------------------------------------------------

/// Theme changes the attribute the stylesheet selects on, and the choice
/// survives a restart.
#[test]
fn theme() {
    let before = Settings::default();
    let light = Settings { theme: ThemePref::Light, ..before.clone() };
    let dark = Settings { theme: ThemePref::Dark, ..before.clone() };
    // Two explicit preferences, not one against the default. The default is
    // System, which resolves through the desktop, so comparing against it
    // asserts what the machine running the test happens to be set to: on a
    // light desktop it reads "light" and the comparison is against itself.
    assert_ne!(theme_attr(&light), theme_attr(&dark));
    assert_eq!(theme_attr(&light), "light");
    assert_eq!(theme_attr(&dark), "dark");

    let restored = after_restart(|s| s.theme = ThemePref::Light);
    assert_eq!(restored.theme, ThemePref::Light);
    assert_eq!(theme_attr(&restored), "light");
}

/// Density changes the row geometry the shell hands down, and survives.
#[test]
fn density() {
    let before = Settings::default();
    let compact = Settings {
        density: Density::Compact,
        ..before.clone()
    };
    assert_eq!(root_style(&before), "");
    assert!(root_style(&compact).contains("--rg-card-h:3.75rem;"));

    let restored = after_restart(|s| s.density = Density::Compact);
    assert_eq!(restored.density, Density::Compact);
    assert!(root_style(&restored).contains("--rg-card-h:3.75rem;"));
}

/// Text scale changes the root font size the shell paints at, and
/// survives.
#[test]
fn text_scale() {
    let before = Settings::default();
    let big = Settings {
        text_scale_pct: 150,
        ..before.clone()
    };
    assert_eq!(ui_scale_px(before.text_scale_pct), "16px");
    assert_eq!(ui_scale_px(big.text_scale_pct), "24px");

    let restored = after_restart(|s| s.set_text_scale(150));
    assert_eq!(restored.text_scale_pct, 150);
    assert_eq!(ui_scale_px(restored.text_scale_pct), "24px");
}

/// Reduced motion zeroes both duration tokens, and survives.
#[test]
fn reduce_motion() {
    assert!(!root_style(&Settings::default()).contains("--rg-t-fast"));
    let restored = after_restart(|s| s.reduce_motion = true);
    assert!(restored.reduce_motion);
    assert!(root_style(&restored).contains("--rg-t-fast:0s;"));
}

// -- Sidebar ------------------------------------------------------------

/// Auto-settle is the one sidebar control whose effect is visible from
/// here rather than in the markup: it is the disposition policy, so it
/// moves a session between bands and therefore moves it between sections,
/// rollups and the attention jump keys.
#[test]
fn auto_settle_window() {
    let clock = Clock::utc(10_000_000);
    // A LIVE session the OS reports as blocked on the terminal, idle for an
    // hour. That combination is the only one auto-settle actually decides:
    // an exited session settles on rule 6 whatever the policy says, and a
    // Working one is exempt so a silent computing agent is never drained
    // out from under a running job. Getting this fixture wrong is how you
    // write a test that passes for a reason unrelated to the setting.
    let mut row = SessionView::new(info(1, "/home/mk/src"));
    row.info.status = SessionStatus::Running;
    row.info.attention.waiting = Some(true);
    row.info.last_activity_ms = clock.now_ms - 60 * 60_000;
    assert_eq!(row.status(), vitrum_model::SidebarStatus::Ready);

    let manual = vitrum_model::DispositionPolicy::manual();
    assert_eq!(
        row.disposition(clock, manual),
        Disposition::Active,
        "with auto-settle off, an hour-idle session must stay in the inbox"
    );

    let restored = after_restart(|s| s.policy.auto_settle_after_ms = Some(15 * 60_000));
    assert_eq!(restored.policy.auto_settle_after_ms, Some(15 * 60_000));
    assert_eq!(
        row.disposition(clock, restored.policy),
        Disposition::Settled,
        "the saved auto-settle window did not take effect after a restart"
    );
}

/// The row-content switches survive the file. Their rendering effect
/// is asserted in `ui/sidebar.rs`, which is the module that reads them;
/// what this file is responsible for is that the value the operator chose
/// is still there next launch.
#[test]
fn row_content_switches() {
    for (name, set, read) in [
        (
            "show_branch",
            (|s: &mut Settings| s.show_branch = false) as fn(&mut Settings),
            (|s: &Settings| s.show_branch) as fn(&Settings) -> bool,
        ),
        (
            "show_place",
            |s: &mut Settings| s.show_place = false,
            |s: &Settings| s.show_place,
        ),
        (
            "show_time",
            |s: &mut Settings| s.show_time = false,
            |s: &Settings| s.show_time,
        ),
        (
            "show_status_word",
            |s: &mut Settings| s.show_status_word = false,
            |s: &Settings| s.show_status_word,
        ),
        (
            "confirm_terminate",
            |s: &mut Settings| s.confirm_terminate = false,
            |s: &Settings| s.confirm_terminate,
        ),
    ] {
        assert!(read(&Settings::default()), "{name} did not default to on");
        assert!(
            !read(&after_restart(set)),
            "{name} reverted after a restart"
        );
    }
}

/// Dense rows forces every row to the slim variant, and survives the file.
///
/// Off by default: it removes the card row, which is where the inbox's
/// second line of context lives, so nobody gets it without asking. The
/// rendering half is asserted in `ui/sidebar.rs`, which owns `row_variant`.
#[test]
fn dense_rows() {
    assert!(
        !Settings::default().always_slim,
        "dense rows must not be the default; it drops a line of context from every inbox row"
    );
    assert!(after_restart(|s| s.always_slim = true).always_slim);
}

/// Every value the model can hold must be expressible in the menu that
/// edits it. A `<select>` whose stored value matches no option displays
/// the FIRST one, so a fresh install showed "Never - I drain the list by
/// hand" while actually settling rows after seven days: a control lying
/// about the state it exists to show, which is the same class of defect as
/// a control that does nothing. Caught by looking at a screenshot.
#[test]
fn the_settle_menu_can_express_the_shipped_default() {
    let shipped = Settings::default().policy.auto_settle_after_ms;
    assert!(
        SETTLE_STEPS.iter().any(|(ms, _)| *ms == shipped),
        "the menu cannot express the default {shipped:?}, so it would misreport it"
    );
    for (ms, label) in SETTLE_STEPS {
        assert!(!label.is_empty(), "{ms:?} has no label");
    }
}

// -- Terminal -----------------------------------------------------------

/// Every terminal control survives the file, and the palette changes the
/// tokens the grid paints with.
#[test]
fn terminal_controls() {
    let base = css_tokens(Settings::default().terminal.palette);

    let palette = after_restart(|s| s.terminal.palette = TermPalette::Nord);
    assert_eq!(palette.terminal.palette, TermPalette::Nord);
    let tokens = css_tokens(palette.terminal.palette);
    assert_ne!(tokens, base);
    assert!(tokens.contains("--rg-terminal-bg:#2e3440;"), "{tokens}");

    let font = after_restart(|s| {
        s.terminal.font_family = "\"Fira Code\", ui-monospace, monospace".to_string();
    });
    assert_eq!(
        font.terminal.font_family,
        "\"Fira Code\", ui-monospace, monospace",
        "the font choice did not survive the file"
    );

    let size = after_restart(|s| s.terminal.font_size_px = 20);
    assert_eq!(size.terminal.font_size_px, 20);

    let scrollback = after_restart(|s| s.terminal.scrollback_lines = 20_000);
    assert_eq!(scrollback.terminal.scrollback_lines, 20_000);
}

// -- Notifications ------------------------------------------------------

/// Every notification switch changes the decision and survives the file.
#[test]
fn notification_switches() {
    for kind in NOTIFY_KINDS {
        let want = !notify_enabled(&Settings::default().notifications, kind);
        let restored = after_restart(|s| {
            s.notifications.skip_focused_session = false;
            set_notify_enabled(&mut s.notifications, kind, want);
        });
        assert_eq!(
            should_notify(&restored.notifications, kind, false),
            want,
            "{kind} did not survive the file"
        );
    }

    let restored = after_restart(|s| s.notifications.skip_focused_session = false);
    assert!(!restored.notifications.skip_focused_session);
    assert!(should_notify(
        &restored.notifications,
        NotificationKind::Failed,
        true
    ));
}

// -- Keyboard -----------------------------------------------------------

/// A rebinding changes the table key dispatch matches on AND the row the
/// overlay prints, and survives the file. Three assertions because a
/// rebinder that gets any one of them wrong produces an undiscoverable
/// binding.
#[test]
fn keyboard_rebinding() {
    let binding = Binding {
        key: "j".to_string(),
        ctrl: true,
        alt: true,
        shift: false,
    };
    let restored = after_restart(|s| {
        set_override(&mut s.keyboard, KeyAction::ToggleSidebar, &binding);
    });

    assert_eq!(
        override_for(&restored.keyboard, KeyAction::ToggleSidebar),
        Some(binding),
        "the rebinding did not survive the file"
    );
    assert_ne!(
        effective_chords(&restored.keyboard),
        effective_chords(&KeyboardPrefs::default()),
        "the default table is still what dispatch would match"
    );
    assert!(
        effective_help_rows(&restored.keyboard)
            .iter()
            .any(|row| row.keys == "Ctrl+Alt+J"),
        "the overlay is not advertising the rebound chord"
    );
}

// -- Advanced -----------------------------------------------------------

/// The daemon URL overrides the command line, and survives. Empty must
/// keep meaning "use the flag", or saving an empty field would silently
/// point the client at nothing.
#[test]
fn daemon_url() {
    let cli = "ws://127.0.0.1:7737";
    assert_eq!(Settings::default().resolved_daemon_url(cli), cli);

    let restored = after_restart(|s| s.daemon_url = "ws://10.0.0.4:9000".to_string());
    assert_eq!(restored.resolved_daemon_url(cli), "ws://10.0.0.4:9000");

    let cleared = after_restart(|s| s.daemon_url = String::new());
    assert_eq!(
        cleared.resolved_daemon_url(cli),
        cli,
        "clearing the override stopped falling back to --server"
    );
}

// -- Workspaces ---------------------------------------------------------

/// Grouping mode changes how the sidebar buckets rows, and survives the
/// file. This is the per-workspace half of the settings, so it round-trips
/// through `WorkspaceSet` rather than through `Settings`.
#[test]
fn workspace_grouping() {
    let clock = Clock::utc(10_000_000);
    let mut st = UiState::default();
    st.daemon.sessions = vec![
        SessionView::new(info(1, "/home/mk/alpha")),
        SessionView::new(info(2, "/home/mk/beta")),
    ];
    st.daemon
        .workspaces
        .adopt(st.daemon.sessions.iter().map(|row| &row.info));

    let here = st.window.workspace;
    st.daemon
        .workspaces
        .get_mut(here)
        .expect("the default workspace exists")
        .grouping = Grouping::Directory;
    let by_directory: Vec<String> = st.tree(clock).iter().map(|g| g.label.clone()).collect();

    st.daemon
        .workspaces
        .get_mut(here)
        .expect("the default workspace exists")
        .grouping = Grouping::Named;
    let by_folder: Vec<String> = st.tree(clock).iter().map(|g| g.label.clone()).collect();
    assert_ne!(
        by_directory, by_folder,
        "switching the grouping mode produced the same tree, so the control does nothing"
    );

    let restored = through_the_file(&st.daemon, &st.window);
    assert_eq!(
        restored
            .workspaces
            .get(here)
            .expect("the workspace survived")
            .grouping,
        Grouping::Named,
        "the grouping mode reverted after a restart"
    );
}

/// Band visibility changes which sections the sidebar draws, and survives.
#[test]
fn workspace_band_visibility() {
    let clock = Clock::utc(10_000_000);
    let mut st = UiState::default();
    // A row in the Settled band by the acknowledged-exit rule: the child is
    // gone and the operator has looked at it since. Chosen over "idle past
    // the auto-settle window" so this test cannot start passing or failing
    // because someone retuned the default policy.
    let mut row = SessionView::new(info(1, "/home/mk/alpha"));
    row.info.last_activity_ms = 1_000;
    row.last_visited_ms = Some(2_000);
    assert_eq!(
        row.disposition(clock, st.daemon.settings.policy),
        Disposition::Settled
    );
    st.daemon.sessions = vec![row];
    st.daemon
        .workspaces
        .adopt(st.daemon.sessions.iter().map(|r| &r.info));

    let here = st.window.workspace;
    let visible_rows = |st: &UiState| -> usize { st.tree(clock).iter().map(|g| g.len()).sum() };

    let with_settled = visible_rows(&st);
    assert_eq!(
        with_settled, 1,
        "the fixture row is not on screen to begin with"
    );

    st.daemon
        .workspaces
        .get_mut(here)
        .expect("the default workspace exists")
        .sections
        .set(Disposition::Settled, false);
    assert_eq!(
        visible_rows(&st),
        0,
        "hiding the Settled band left its rows on screen, so the switch does nothing"
    );

    let restored = through_the_file(&st.daemon, &st.window);
    assert!(
        !restored
            .workspaces
            .get(here)
            .expect("the workspace survived")
            .sections
            .settled,
        "the hidden band came back after a restart"
    );
}

/// A created workspace, its name and its order all survive the file, and
/// the one this window was viewing is still the one it comes back to.
#[test]
fn workspace_creation_and_order() {
    let mut st = UiState::default();
    let review = st.create_workspace("Review").expect("a valid name");
    let scratch = st.create_workspace("Scratch").expect("a valid name");
    st.daemon
        .workspaces
        .move_to(scratch, 0)
        .expect("the index is in range");
    st.set_workspace(review, 1_000).expect("it exists");

    let doc = Persisted::capture(&st.daemon, std::iter::once(&st.window));
    let text = encode_ui_state(&doc);
    let back = match parse_ui_state(&text) {
        UiStateLoad::Loaded(doc) => *doc,
        other => panic!("the settings file did not read back: {other:?}"),
    };

    let mut fresh = UiState::default();
    back.restore_daemon(&mut fresh.daemon);
    assert!(back.restore_window(&mut fresh.window));

    let names: Vec<String> = fresh
        .daemon
        .workspaces
        .iter()
        .map(|w| w.display_name().to_string())
        .collect();
    assert_eq!(
        names,
        vec![
            "Scratch".to_string(),
            "Workspace".to_string(),
            "Review".to_string()
        ],
        "the workspace order did not survive the file"
    );
    assert_eq!(
        fresh.window.workspace, review,
        "the window came back looking at a different workspace"
    );
}

/// The settings layer remembers which page it was on. Reopening on
/// Appearance every time turns a five-change session into five hunts for
/// the same tab.
#[test]
fn the_open_tab_is_remembered() {
    let mut st = UiState::default();
    st.window.layer = crate::state::Layer::Settings(SettingsTab::Terminal);
    st.set_settings_tab(SettingsTab::Keyboard);
    assert_eq!(
        st.window.layer,
        crate::state::Layer::Settings(SettingsTab::Keyboard)
    );

    st.window.layer = crate::state::Layer::None;
    st.set_settings_tab(SettingsTab::Advanced);
    assert_eq!(
        st.window.layer,
        crate::state::Layer::None,
        "setting a tab opened the modal on its own"
    );
}
