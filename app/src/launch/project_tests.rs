use super::*;
use vitrum_proto::{ProjectId, ProjectInfo};

fn project(id: u64, root: &str) -> ProjectInfo {
    ProjectInfo {
        id: ProjectId(id),
        name: root.rsplit('/').next().unwrap_or(root).to_string(),
        root: root.to_string(),
    }
}

/// A textual prefix that is not a path prefix must not match. `/src/reg`
/// and `/src/vitrum` are different projects, and grouping a session under
/// the wrong header is the kind of wrongness nobody reports as a bug, they
/// just stop trusting the sidebar.
#[test]
fn containment_is_by_component_not_by_string_prefix() {
    assert!(is_within("/src/vitrum", "/src/vitrum"));
    assert!(is_within("/src/vitrum", "/src/vitrum/app/src"));
    assert!(!is_within("/src/reg", "/src/vitrum"));
    assert!(!is_within("/src/vitrum/app", "/src/vitrum"));
    assert!(!is_within("/other", "/src/vitrum"));
}

/// A session started in a subdirectory belongs to the project above it,
/// not to a new project named after the subdirectory. Otherwise running an
/// agent in `repo/crates/foo` grows a second sidebar header for the same
/// checkout.
#[test]
fn a_subdirectory_joins_the_project_that_contains_it() {
    let ps = vec![project(1, "/src/vitrum")];
    assert_eq!(
        resolve_project(&ps, "/src/vitrum/app/src"),
        (ProjectId(1), false)
    );
}

/// With nested projects the deepest one wins. A monorepo registered as one
/// project and a crate inside it registered as another must not have the
/// crate's sessions filed under the monorepo.
#[test]
fn the_deepest_containing_project_wins() {
    let ps = vec![project(1, "/src"), project(2, "/src/vitrum")];
    assert_eq!(
        resolve_project(&ps, "/src/vitrum/app"),
        (ProjectId(2), false)
    );
    assert_eq!(resolve_project(&ps, "/src/other"), (ProjectId(1), false));
}

/// An unknown directory mints a project, and the mint must be stable
/// across calls. An id derived from a counter would change on every
/// restart and the sidebar would grow a duplicate header for a project the
/// user already had.
#[test]
fn an_unknown_directory_mints_a_stable_id() {
    let ps: Vec<ProjectInfo> = Vec::new();
    let (a, new_a) = resolve_project(&ps, "/src/fresh");
    let (b, new_b) = resolve_project(&ps, "/src/fresh");
    assert!(new_a && new_b);
    assert_eq!(a, b);
    let (c, _) = resolve_project(&ps, "/src/other");
    assert_ne!(a, c, "two directories must not share an id");
}

/// A trailing separator must not create a second project for the same
/// directory. Users paste paths with and without one interchangeably.
#[test]
fn a_trailing_separator_keys_the_same_project() {
    let ps: Vec<ProjectInfo> = Vec::new();
    assert_eq!(
        resolve_project(&ps, "/src/fresh"),
        resolve_project(&ps, "/src/fresh/")
    );
}

/// An existing directory keys by its canonical path, so a relative path
/// and an absolute one agree. Without this, launching in `.` and launching
/// in the same directory by absolute path produce two projects.
#[test]
fn an_existing_directory_keys_canonically() {
    let tmp = std::env::temp_dir();
    let canonical = std::fs::canonicalize(&tmp).unwrap();
    assert_eq!(
        project_key(tmp.to_str().unwrap()),
        canonical.to_str().unwrap()
    );
}

/// A path that does not exist keeps the text the user typed, so the dialog
/// can echo it back in the error message rather than showing an empty
/// field or a canonicalisation failure.
#[test]
fn a_nonexistent_path_keeps_what_was_typed() {
    assert_eq!(project_key("  /no/such/dir  "), "/no/such/dir");
}
