//! Context menus for session rows and tabs.
//!
//! The menu's contents come from [`crate::state::UiState::menu_items`], which
//! is pure data, so this file only positions and paints.

/// The GTK surface: a popover the toolkit anchors, flips and clamps itself.
pub mod native;
