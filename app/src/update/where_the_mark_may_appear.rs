//! The mark belongs on the launcher, not in the window.
//!
//! It may appear in two places: the launcher, and a loading screen. It may
//! not appear anywhere in the window chrome, and
//! [`the_mark_stays_out_of_the_window`] is what stops it.
//!
//! The rule used to be written down in `assets/logo/README.md`, with a test
//! asserting that document still explained it, on the reasoning that a test
//! failing with a rule nobody can find just gets deleted. That directory is
//! gone now, along with every picture this repository published, and it took
//! the explanation with it. So the rule is stated here instead, next to the
//! only thing that enforces it, where it cannot be separated from the failure
//! it causes.

/// Every user interface source, and the stylesheet they share.
const UI_SOURCES: &[(&str, &str)] = &[
    ("ui/dialog.rs", include_str!("../ui/dialog.rs")),
    ("ui/menu.rs", include_str!("../ui/menu.rs")),
    ("ui/mod.rs", include_str!("../ui/mod.rs")),
    ("ui/search.rs", include_str!("../ui/search.rs")),
    ("ui/settings.rs", include_str!("../ui/settings.rs")),
    ("ui/shortcuts.rs", include_str!("../ui/shortcuts.rs")),
    ("ui/sidebar.rs", include_str!("../ui/sidebar.rs")),
    ("ui/terminal.rs", include_str!("../ui/terminal.rs")),
    ("ui/titlebar.rs", include_str!("../ui/titlebar.rs")),
    ("ui/workspaces.rs", include_str!("../ui/workspaces.rs")),
    ("assets/shell.css", include_str!("../../assets/shell.css")),
];

/// Ways the mark could get into a window.
///
/// Two kinds, because blocking only the filename would be trivially
/// defeated by pasting the geometry inline, which is exactly what someone
/// in a hurry with an `rsx!` block would do.
const SMELLS: &[(&str, &str)] = &[
    ("assets/logo", "a path into the logo directory"),
    ("vitrum.svg", "the mark by filename"),
    ("vitrum-inverted", "the inverted mark by filename"),
    ("vitrum.ico", "the Windows icon"),
    ("21.61,26.00", "the mark's geometry, pasted inline"),
    ("193.82,38.00", "the mark's geometry, pasted inline"),
];

/// No user interface source may reference the mark.
///
/// The window exists to show the operator agents doing work. A logo in
/// there answers a question nobody asked, cannot be acted on, and costs
/// space that belongs to the sessions. It is the plainest failure of the
/// test this product applies to everything on screen: what does the
/// operator do differently because this is here?
///
/// The launcher entry is different in kind. That is drawn by the desktop
/// before the program is running, so it is the one moment the mark is the
/// only thing that can identify it.
#[test]
fn the_mark_stays_out_of_the_window() {
    let mut found = Vec::new();
    for (name, source) in UI_SOURCES {
        for (needle, what) in SMELLS {
            if let Some(at) = source.find(needle) {
                let line = source[..at].lines().count();
                found.push(format!("{name}:{line} has {what} (`{needle}`)"));
            }
        }
    }
    assert!(
        found.is_empty(),
        "the mark may appear on the launcher and on a loading screen, and \
         nowhere else inside the application. See this module's doc comment. \
         Found:\n  {}",
        found.join("\n  ")
    );
}

