use super::*;

/// A slug that changed spelling silently swaps every operator's chosen icon
/// for the default, because the slug is the only thing `launch.json` stores.
#[test]
fn every_icon_has_a_unique_slug_and_a_label() {
    for (i, icon) in ALL.iter().enumerate() {
        assert!(!icon.slug.is_empty(), "icon {i} has no slug");
        assert!(!icon.label.is_empty(), "{} has no label", icon.slug);
        assert!(!icon.stroke.is_empty(), "{} draws nothing", icon.slug);
        assert_eq!(
            from_slug(icon.slug).map(|found| found.slug),
            Some(icon.slug),
            "{} is not reachable through from_slug",
            icon.slug
        );
        assert!(
            ALL.iter().filter(|other| other.slug == icon.slug).count() == 1,
            "{} appears twice",
            icon.slug
        );
    }
}

/// The default icon used to be one shape for everything, which made a list of
/// saved commands a column of identical boxes.
#[test]
fn a_command_with_no_icon_gets_the_shape_it_implies() {
    let cases = [
        ("claude", "spark"),
        ("claude --permission-mode plan", "spark"),
        ("/usr/bin/bash -l", "terminal"),
        ("git status", "branch"),
        ("cargo test -p vitrum-app", "wrench"),
        ("pytest", "flask"),
        ("docker compose up", "container"),
        ("C:\\tools\\codex.exe", "hexagon"),
    ];
    for (line, slug) in cases {
        assert_eq!(default_for(line).slug, slug, "default for {line:?}");
    }
}

/// A prefix match would put the Git icon on `gitk` and the Claude icon on
/// `claudex`, which is a confident wrong answer the operator cannot correct
/// without noticing it first.
#[test]
fn an_unrecognised_command_gets_the_generic_icon_not_a_near_miss() {
    for line in ["gitk", "claudex", "my-claude", "", "   "] {
        assert_eq!(default_for(line).slug, FALLBACK, "default for {line:?}");
    }
}

/// A slug from a newer build, or a hand-edited one, must not blank the row.
#[test]
fn an_unknown_slug_falls_back_to_the_command_default() {
    assert_eq!(from_slug("no-such-icon"), None);
    assert_eq!(resolve(Some("no-such-icon"), "git push").slug, "branch");
    assert_eq!(resolve(None, "git push").slug, "branch");
    assert_eq!(resolve(Some("flask"), "git push").slug, "flask");
}
