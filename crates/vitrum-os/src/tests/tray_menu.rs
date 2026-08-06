//! The tray menu model shared by all three backends.
//!
//! Linux builds ksni items from this, macOS and Windows build muda items from
//! it. Asserting the model once is what keeps the three trays from drifting
//! into three different menus.

use crate::icon::{Rgba, render_tray_icon};
use crate::tray::{
    ID_ATTENTION, ID_NEW_SESSION, ID_QUIT, ID_TOGGLE_WINDOW, TrayCommand, TrayEntry,
    attention_summary, command_for_id, tray_icon_size, tray_menu, tray_tooltip,
};

fn labels(entries: &[TrayEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|e| match e {
            TrayEntry::Separator => "---".to_string(),
            TrayEntry::Item(i) => i.label.clone(),
        })
        .collect()
}

/// With nothing pending the menu is exactly the three actions.
///
/// A status row reading "0 sessions need attention" is noise, and a menu that
/// changes height every time a session finishes is jarring to click through.
#[test]
fn an_idle_menu_has_only_the_actions() {
    let menu = tray_menu(true, 0);
    assert_eq!(labels(&menu), vec!["Hide Window", "New Session", "---", "Quit Vitrum"]);
}

/// With sessions pending the menu gains a disabled summary at the top.
#[test]
fn a_pending_menu_leads_with_a_disabled_summary() {
    let menu = tray_menu(true, 3);
    assert_eq!(
        labels(&menu),
        vec![
            "3 sessions need attention",
            "---",
            "Hide Window",
            "New Session",
            "---",
            "Quit Vitrum",
        ]
    );
    let TrayEntry::Item(summary) = &menu[0] else { panic!("first entry must be an item") };
    assert_eq!(summary.id, ID_ATTENTION);
    assert!(!summary.enabled, "the summary is a status line, not a button");
    assert_eq!(summary.command, None, "clicking the summary must do nothing");
}

/// The toggle row must say what clicking it will do.
///
/// A row that reads "Show Window" while the window is already showing is the
/// classic tray bug, and it happens whenever the tray is not told about
/// visibility changes.
#[test]
fn the_toggle_row_reflects_the_current_visibility() {
    let TrayEntry::Item(shown) = &tray_menu(true, 0)[0] else { panic!("item") };
    assert_eq!(shown.label, "Hide Window");
    let TrayEntry::Item(hidden) = &tray_menu(false, 0)[0] else { panic!("item") };
    assert_eq!(hidden.label, "Show Window");
    // The id is stable across the label flip, because the backends route by id.
    assert_eq!(shown.id, hidden.id);
    assert_eq!(shown.id, ID_TOGGLE_WINDOW);
    assert_eq!(shown.command, Some(TrayCommand::ToggleWindow));
}

/// Quit must be last, enabled, and named after the product.
#[test]
fn quit_is_last_and_enabled() {
    let menu = tray_menu(false, 5);
    let TrayEntry::Item(quit) = menu.last().expect("a menu is never empty") else {
        panic!("the last entry must be an item")
    };
    assert_eq!(quit.id, ID_QUIT);
    assert_eq!(quit.label, "Quit Vitrum");
    assert!(quit.enabled);
    assert_eq!(quit.command, Some(TrayCommand::Quit));
}

/// A separator must never be first or last.
///
/// A leading or trailing separator draws as a stray line at the edge of the
/// menu on every platform.
#[test]
fn separators_are_never_at_the_edges() {
    for (visible, count) in [(true, 0), (false, 0), (true, 1), (false, 99)] {
        let menu = tray_menu(visible, count);
        assert!(
            !matches!(menu.first(), Some(TrayEntry::Separator)),
            "leading separator for ({visible}, {count})"
        );
        assert!(
            !matches!(menu.last(), Some(TrayEntry::Separator)),
            "trailing separator for ({visible}, {count})"
        );
    }
}

/// Ids must route to commands, and unknown ids must route to nothing.
///
/// The macOS and Windows backends receive a bare id string from a global event
/// handler. If an unknown id fell through to a default, a stray menu event from
/// another muda menu in the process would quit the application.
#[test]
fn ids_route_to_commands_and_unknown_ids_do_not() {
    assert_eq!(command_for_id(ID_TOGGLE_WINDOW), Some(TrayCommand::ToggleWindow));
    assert_eq!(command_for_id(ID_QUIT), Some(TrayCommand::Quit));
    assert_eq!(command_for_id(ID_NEW_SESSION), Some(TrayCommand::NewSession));
    assert_eq!(command_for_id(ID_ATTENTION), None);
    assert_eq!(command_for_id(""), None);
    assert_eq!(command_for_id("Quit Vitrum"), None);
    assert_eq!(command_for_id("quit "), None);
}

/// Every enabled item in the menu must carry a command its id resolves to.
///
/// This is the invariant that ties the model to the routing: an item a user can
/// click that maps to nothing is a dead menu entry.
#[test]
fn every_enabled_item_routes() {
    for entry in tray_menu(true, 4) {
        let TrayEntry::Item(item) = entry else { continue };
        if item.enabled {
            assert_eq!(
                command_for_id(item.id),
                item.command,
                "id {} must route to its declared command",
                item.id
            );
            assert!(item.command.is_some(), "enabled item {} must do something", item.id);
        }
    }
}

/// The summary must be grammatical at one.
///
/// "1 sessions need attention" is the single most visible sign that nobody
/// looked at the product.
#[test]
fn the_summary_is_grammatical_at_one() {
    assert_eq!(attention_summary(1), "1 session needs attention");
    assert_eq!(attention_summary(2), "2 sessions need attention");
    assert_eq!(attention_summary(0), "0 sessions need attention");
}

/// The tooltip must be the bare product name when nothing is pending.
#[test]
fn the_tooltip_is_bare_when_idle() {
    assert_eq!(tray_tooltip(0), "Vitrum");
    assert_eq!(tray_tooltip(1), "Vitrum: 1 session needs attention");
    assert_eq!(tray_tooltip(7), "Vitrum: 7 sessions need attention");
}

/// The icon size must match the platform's status area.
///
/// 16 in the Windows notification area, 22 for freedesktop and the macOS menu
/// bar. A 22-pixel icon on Windows is downscaled and blurry.
#[test]
fn the_icon_size_matches_the_platform() {
    let expected = if cfg!(target_os = "windows") { 16 } else { 22 };
    assert_eq!(tray_icon_size(), expected);
}

/// Starting a session must be reachable from the tray with the window hidden.
///
/// The tray is what you have when the window is not on screen. A menu offering
/// only show and quit makes you raise the window to do the one thing you came
/// for, which is why this row exists at all.
#[test]
fn a_new_session_can_be_started_from_the_tray() {
    for visible in [true, false] {
        let menu = tray_menu(visible, 0);
        let row = menu
            .iter()
            .find_map(|e| match e {
                TrayEntry::Item(i) if i.id == ID_NEW_SESSION => Some(i),
                _ => None,
            })
            .expect("the menu must offer a new session");
        assert_eq!(row.label, "New Session");
        assert_eq!(row.command, Some(TrayCommand::NewSession));
        assert!(row.enabled);
    }
}

/// The attention count must change the icon, the tooltip and the summary row
/// together.
///
/// These three are what the operator actually reads, and they are driven from
/// one number by three separate functions. Letting them disagree gives a red
/// icon with a tooltip saying nothing is pending, which teaches the operator to
/// ignore the icon.
#[test]
fn the_attention_count_drives_icon_tooltip_and_summary() {
    let size = tray_icon_size();
    let centre = size / 2;

    let idle = render_tray_icon(size, 0);
    assert_eq!(idle.pixel(centre, centre), Some(Rgba::IDLE));
    assert_eq!(tray_tooltip(0), "Vitrum");
    assert!(
        !tray_menu(true, 0).iter().any(|e| matches!(e, TrayEntry::Item(i) if i.id == ID_ATTENTION)),
        "an idle tray must not claim anything needs attention"
    );

    for count in [1, 3, 9, 10, 250] {
        let icon = render_tray_icon(size, count);
        assert_ne!(
            icon.pixel(centre, centre),
            Some(Rgba::IDLE),
            "{count} pending must not render the idle icon"
        );
        assert_eq!(tray_tooltip(count), format!("Vitrum: {}", attention_summary(count)));
        let summary = tray_menu(true, count)
            .into_iter()
            .find_map(|e| match e {
                TrayEntry::Item(i) if i.id == ID_ATTENTION => Some(i),
                _ => None,
            })
            .expect("a pending tray must summarise the count");
        assert_eq!(summary.label, attention_summary(count));
        assert!(!summary.enabled);
    }
}

/// The icon must be drawn in the attention colour, not merely be different.
///
/// The count glyph is white on a filled disc. Asserting only "not idle grey"
/// would pass on a transparent icon, which is what a rasteriser that silently
/// failed would produce.
#[test]
fn a_pending_icon_is_filled_with_the_attention_colour() {
    let size = tray_icon_size();
    let icon = render_tray_icon(size, 4);
    let filled = (0..size * size)
        .filter(|i| icon.pixel(i % size, i / size) == Some(Rgba::ATTENTION))
        .count();
    assert!(filled > (size * size / 4) as usize, "only {filled} attention pixels in {size}px");
}
