//! Context menus for session rows and tabs.
//!
//! The menu's contents come from [`UiState::menu_items`], which is pure data,
//! so this file only positions and paints. Positioning is the part with a bug
//! in it: a menu opened near the bottom right of the window draws off screen
//! and the last two entries, which are the destructive ones, become
//! unreachable. [`clamp`] is what stops that, and it is a pure function with
//! its own tests rather than a CSS guess.
//!
//! The backdrop is a real element, not a document-level listener. A listener
//! on `document` would stay attached while the menu is closed and wake the
//! process on every click anywhere in the window; the backdrop exists only
//! while a menu does.

use dioxus::prelude::*;
use vitrum_fmt::TimeFormat;

use crate::inbox;
use crate::state::{MenuAction, MenuItem, MenuState, UiState};

/// Menu width in CSS pixels. Must match `--rg-menu-w` in `app.css`; the
/// clamp needs a number before layout has happened, so the two are kept in
/// step by [`tests::menu_width_matches_the_stylesheet`].
pub const MENU_W: f64 = 232.0;
/// Height of one entry, matching `--rg-menu-item-h`.
pub const MENU_ITEM_H: f64 = 28.0;
/// Height a separator adds: 1px rule plus the margin either side.
pub const MENU_SEP_H: f64 = 9.0;
/// Vertical padding of the menu box, both edges together.
pub const MENU_PAD_H: f64 = 8.0;
/// Height of the title row naming what was right-clicked.
pub const MENU_HEADER_H: f64 = 24.0;
/// Gap kept between the menu and the window edge.
pub const MENU_MARGIN: f64 = 8.0;

/// Height the menu will occupy, from its contents.
pub fn menu_height(items: &[MenuItem]) -> f64 {
    let seps = items.iter().filter(|i| i.sep_before).count() as f64;
    MENU_HEADER_H + MENU_PAD_H + items.len() as f64 * MENU_ITEM_H + seps * MENU_SEP_H
}

/// Keep a menu of `w` by `h` fully inside a `vw` by `vh` window.
///
/// Flips rather than merely shifting: a menu opened 20px from the right edge
/// would otherwise be shifted left so far that it covers the row it belongs
/// to. Anchoring its right edge at the click keeps the pointer on the corner
/// of the menu, which is where every platform puts it.
///
/// A window too small to hold the menu clamps to the top left corner instead
/// of producing a negative offset, so the first entries stay reachable and the
/// rest scroll.
pub fn clamp(x: f64, y: f64, w: f64, h: f64, vw: f64, vh: f64) -> (f64, f64) {
    (clamp_axis(x, w, vw), clamp_axis(y, h, vh))
}

/// One axis of [`clamp`].
fn clamp_axis(at: f64, size: f64, extent: f64) -> f64 {
    // Too small to hold the menu on this axis at all. Pinning to the margin
    // keeps the first entries reachable and lets the box scroll; flipping
    // would put them off the opposite edge instead.
    if size + 2.0 * MENU_MARGIN > extent {
        return MENU_MARGIN;
    }
    if at + size + MENU_MARGIN > extent {
        return (at - size).max(MENU_MARGIN);
    }
    at.max(MENU_MARGIN)
}

#[derive(Props, Clone, PartialEq)]
pub struct ContextMenuProps {
    pub state: Signal<UiState>,
    pub menu: MenuState,
    /// Wall clock at render time. The menu is a function of it: the snooze
    /// presets name real times, and whether a row is parked depends on where
    /// the clock is relative to its wake instant.
    pub clock: TimeFormat,
    pub on_pick: EventHandler<(MenuAction, MenuState)>,
    pub on_dismiss: EventHandler<()>,
}

#[component]
pub fn ContextMenu(props: ContextMenuProps) -> Element {
    let st = props.state.read();
    let menu = props.menu;
    let model_clock = inbox::model_clock(props.clock);
    let items = st.menu_items(menu.target, model_clock);
    // A menu on a session that vanished between the right-click and this paint
    // has nothing to act on. Rendering an empty box would be worse than
    // rendering nothing, because the backdrop would still swallow the next
    // click.
    if items.is_empty() {
        return rsx! {};
    }
    let targets = st.menu_targets(menu.target, model_clock);
    // A bulk menu names its count rather than one row's title. A menu headed
    // with one session's name that then closes nineteen is the exact mistake
    // the counted labels exist to prevent.
    let title = if targets.len() > 1 {
        format!("{} sessions selected", targets.len())
    } else {
        st.session(menu.target)
            .map(|s| crate::inbox::row_title(s).into_owned())
            .unwrap_or_default()
    };

    rsx! {
        div {
            class: "rg-layer",
            onclick: move |_| props.on_dismiss.call(()),
            oncontextmenu: move |e| {
                e.prevent_default();
                props.on_dismiss.call(());
            },
            div {
                class: "rg-menu",
                style: "left: {menu.x}px; top: {menu.y}px",
                role: "menu",
                // Without this, picking an entry also lands on the backdrop
                // and dismisses before the handler has run.
                onclick: move |e| e.stop_propagation(),
                div { class: "rg-menu__header", title: "{title}", "{title}" }
                for (index, item) in items.iter().enumerate() {
                    {
                        let action = item.action;
                        let mut class = String::from("rg-menu__item");
                        if item.danger {
                            class.push_str(" rg-menu__item--danger");
                        }
                        if item.sep_before {
                            class.push_str(" rg-menu__item--sep");
                        }
                        if action.is_caption() {
                            class.push_str(" rg-menu__item--caption");
                        }
                        let hint = item.hint.clone();
                        rsx! {
                            button {
                                // Labels repeat across a menu now that presets
                                // carry counts, so the index is the only stable
                                // key.
                                key: "{index}",
                                class: "{class}",
                                r#type: "button",
                                role: "menuitem",
                                disabled: !item.enabled || action.is_caption(),
                                onclick: move |_| props.on_pick.call((action, menu)),
                                span { class: "rg-menu__label", "{item.label}" }
                                if let Some(hint) = hint {
                                    span { class: "rg-menu__hint", "{hint}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MenuAction;

    fn item(action: MenuAction, sep: bool) -> MenuItem {
        MenuItem {
            action,
            label: "x".to_string(),
            hint: None,
            enabled: true,
            danger: false,
            sep_before: sep,
        }
    }

    /// A menu that fits must not move. Nudging every menu by a pixel "to be
    /// safe" makes the pointer land between two entries instead of on the one
    /// under it.
    #[test]
    fn a_menu_that_fits_opens_exactly_where_it_was_asked_to() {
        assert_eq!(
            clamp(100.0, 200.0, 232.0, 300.0, 1280.0, 800.0),
            (100.0, 200.0)
        );
    }

    /// Near the right edge the menu must flip so its right edge sits at the
    /// click. Shifting instead would slide it across the row it was opened
    /// from, so the user cannot see what they right-clicked.
    #[test]
    fn a_menu_near_the_right_edge_flips_left() {
        let (x, _) = clamp(1200.0, 100.0, 232.0, 300.0, 1280.0, 800.0);
        assert_eq!(x, 968.0);
    }

    /// Same rule vertically, which matters more: the destructive entries are
    /// at the bottom, so a menu running off the bottom of the window hides
    /// exactly the entries a user most needs to see before clicking.
    #[test]
    fn a_menu_near_the_bottom_edge_flips_up() {
        let (_, y) = clamp(100.0, 700.0, 232.0, 300.0, 1280.0, 800.0);
        assert_eq!(y, 400.0);
    }

    /// Both at once, in the corner where a tab's own close button lives.
    #[test]
    fn a_menu_in_the_bottom_right_corner_flips_both_ways() {
        assert_eq!(
            clamp(1270.0, 790.0, 232.0, 300.0, 1280.0, 800.0),
            (1038.0, 490.0)
        );
    }

    /// A window smaller than the menu must pin to the margin, never to a
    /// negative offset and never to a flipped position that is just as far off
    /// screen. A negative `left` puts the entries where no pointer can reach
    /// them and no scrollbar appears.
    #[test]
    fn a_window_too_small_for_the_menu_pins_to_the_margin() {
        assert_eq!(clamp(300.0, 300.0, 232.0, 400.0, 200.0, 200.0), (8.0, 8.0));
        assert_eq!(clamp(0.0, 0.0, 232.0, 400.0, 200.0, 200.0), (8.0, 8.0));
        assert_eq!(clamp(150.0, 10.0, 232.0, 100.0, 200.0, 800.0), (8.0, 10.0));
    }

    /// A click at the very top left must not be pushed off by the margin
    /// clamp, and must not end up left of it either.
    #[test]
    fn a_click_in_the_corner_stays_inside_the_margin() {
        assert_eq!(clamp(0.0, 0.0, 232.0, 300.0, 1280.0, 800.0), (8.0, 8.0));
        assert_eq!(clamp(9.0, 9.0, 232.0, 300.0, 1280.0, 800.0), (9.0, 9.0));
    }

    /// Height must grow with entries and with separators, or the flip
    /// calculation above is done against the wrong box and the menu still runs
    /// off the bottom.
    #[test]
    fn height_counts_entries_and_separators() {
        let plain = vec![
            item(MenuAction::Focus, false),
            item(MenuAction::CloseTab, false),
        ];
        // Named constants, not literals. This assertion previously hardcoded
        // 26.0 for the item height, so raising --rg-menu-item-h to 28px to
        // clear the 28px minimum pointer target broke a test that has nothing
        // to do with pointer targets. What it actually guards is the COUNTING:
        // one header, one padding, one item height per item, and exactly one
        // separator height per item that asks for one. Those are the things a
        // miscount would break, and they hold at any token value.
        assert_eq!(
            menu_height(&plain),
            MENU_HEADER_H + MENU_PAD_H + 2.0 * MENU_ITEM_H
        );
        let with_sep = vec![
            item(MenuAction::Focus, false),
            item(MenuAction::Terminate, true),
        ];
        assert_eq!(
            menu_height(&with_sep),
            MENU_HEADER_H + MENU_PAD_H + 2.0 * MENU_ITEM_H + MENU_SEP_H,
            "a single separator must add exactly one MENU_SEP_H, not one per item"
        );
    }

    /// The width used by the clamp must match the width the stylesheet paints,
    /// or every flip is wrong by the difference and the menu still clips.
    #[test]
    fn menu_width_matches_the_stylesheet() {
        let css = include_str!("../app.css");
        assert!(
            css.contains(&format!("--rg-menu-w: {}px;", MENU_W as u32)),
            "app.css must declare --rg-menu-w: {}px",
            MENU_W as u32
        );
        assert!(
            css.contains(&format!("--rg-menu-item-h: {}px;", MENU_ITEM_H as u32)),
            "app.css must declare --rg-menu-item-h: {}px",
            MENU_ITEM_H as u32
        );
    }

}
