//! The mark belongs on the launcher, not in the window.
//!
//! Documented in `assets/logo/README.md`; enforced here, because a rule that
//! lives only in a document is a rule that survives exactly until somebody
//! wants a splash of brand in the titlebar.

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
    ("app.css", include_str!("../app.css")),
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
         nowhere else inside the application. See assets/logo/README.md. \
         Found:\n  {}",
        found.join("\n  ")
    );
}

/// The rule is written down where someone editing the mark will see it.
///
/// A test that fails with a rule nobody can find just gets deleted. This
/// asserts the explanation exists next to the asset, so the failure above
/// has somewhere to send the reader.
#[test]
fn the_rule_is_documented_beside_the_asset() {
    let doc = include_str!("../../../assets/logo/README.md");
    assert!(
        doc.contains("Where it may not appear"),
        "assets/logo/README.md no longer states where the mark is banned"
    );
    assert!(
        doc.contains("the_mark_stays_out_of_the_window"),
        "the doc no longer points at the test that enforces it, so a \
         failing test has nowhere to send the reader"
    );
    for place in ["launcher", "loading screen"] {
        assert!(
            doc.contains(place),
            "the doc no longer names {place} as an allowed place"
        );
    }
}
