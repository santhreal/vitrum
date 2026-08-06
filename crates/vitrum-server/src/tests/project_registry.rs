//! The project registry: naming, idempotence, and ordering.

use std::collections::HashSet;

use vitrum_proto::{ProjectId, ProjectInfo};

use crate::ProjectRegistry;

/// Everything the registry holds, treating every id in `live` as still having
/// a session. The registry has no unfiltered reader by design: a project with
/// no sessions is not reportable, so a naming or ordering test has to say
/// which projects are alive before it can ask what they are called.
fn listed(reg: &ProjectRegistry, live: &[u64]) -> Vec<ProjectInfo> {
    let set: HashSet<ProjectId> = live.iter().copied().map(ProjectId).collect();
    reg.live(&set)
}

/// A project must be named by the last component of its root.
///
/// A sidebar row shows about twenty characters, so an absolute path is unreadable
/// there. The root is kept in full for anything that needs the real location.
#[test]
fn a_project_is_named_by_its_last_path_component() {
    let reg = ProjectRegistry::default();
    reg.ensure(ProjectId(1), "/home/dev/work/vitrum");
    let projects = listed(&reg, &[1]);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "vitrum");
    assert_eq!(projects[0].root, "/home/dev/work/vitrum");
    assert_eq!(projects[0].id, ProjectId(1));
}

/// A trailing separator must not produce a blank name, which would render as an
/// empty sidebar row the user cannot identify.
#[test]
fn a_trailing_separator_does_not_blank_the_name() {
    let reg = ProjectRegistry::default();
    reg.ensure(ProjectId(1), "/home/dev/work/vitrum/");
    assert_eq!(listed(&reg, &[1])[0].name, "vitrum");
}

/// A root with no usable component must keep its raw string rather than becoming
/// blank.
#[test]
fn a_rootless_path_keeps_its_raw_string() {
    let reg = ProjectRegistry::default();
    reg.ensure(ProjectId(1), "/");
    assert_eq!(listed(&reg, &[1])[0].name, "/");

    let empty = ProjectRegistry::default();
    empty.ensure(ProjectId(2), "");
    assert_eq!(listed(&empty, &[2])[0].name, "");
}

/// Registering the same id twice must not duplicate or re-root the project.
///
/// Two agents in one repository is the normal case, and the second may be started
/// from a subdirectory. Re-rooting on it would move the project under the user's
/// feet and split the sidebar group.
#[test]
fn registering_twice_neither_duplicates_nor_re_roots() {
    let reg = ProjectRegistry::default();
    reg.ensure(ProjectId(1), "/work/repo");
    reg.ensure(ProjectId(1), "/work/repo/crates/inner");
    let projects = listed(&reg, &[1]);
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].root, "/work/repo");
    assert_eq!(projects[0].name, "repo");
}

/// Distinct ids must coexist and list in id order, so the sidebar does not
/// reshuffle between refreshes and its rows stay clickable.
#[test]
fn projects_list_in_id_order() {
    let reg = ProjectRegistry::default();
    reg.ensure(ProjectId(3), "/c");
    reg.ensure(ProjectId(1), "/a");
    reg.ensure(ProjectId(2), "/b");
    let ids: Vec<ProjectId> = listed(&reg, &[1, 2, 3]).iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![ProjectId(1), ProjectId(2), ProjectId(3)]);
}

/// An empty registry must list nothing rather than a placeholder, so a client can
/// tell "no projects yet" from "one unnamed project".
#[test]
fn an_empty_registry_lists_nothing() {
    assert!(listed(&ProjectRegistry::default(), &[1, 2, 3]).is_empty());
}

/// The same root under two ids must stay two projects.
///
/// The client owns project identity; two windows onto one directory are two rows
/// if the client says so, and the server must not merge them.
#[test]
fn the_same_root_under_two_ids_stays_two_projects() {
    let reg = ProjectRegistry::default();
    reg.ensure(ProjectId(1), "/work/repo");
    reg.ensure(ProjectId(2), "/work/repo");
    assert_eq!(listed(&reg, &[1, 2]).len(), 2);
}
