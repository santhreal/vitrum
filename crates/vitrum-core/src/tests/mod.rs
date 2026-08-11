//! Test suites for vitrum-core, one concern per module.

mod agent_title;
mod attention;
mod close_tree;
mod command_path;
mod git_branch;
mod git_worktree;
mod helpers;
mod hostpath;
#[cfg(not(windows))]
mod hint_session;
mod manager_registry;
mod osc_bound;
mod osc_capture;
mod output_path_cost;
mod output_scan;
mod pty_burst_exit;
mod pty_capacity;
mod pty_detached;
mod pty_escapes;
mod pty_exit;
mod pty_geometry;
mod pty_input;
mod pty_output;
mod pty_resize;
mod rename;
mod scrollback_capacity;
mod scrollback_range;
mod scrollback_seq;
mod terminfo;
mod title;
mod waiting_probe;

/// Every test file in this directory is declared above.
///
/// WHY: `waiting_probe.rs` sat here tracked, compiling nowhere and running
/// nowhere, because nothing declared it. Twenty-six assertions were gone,
/// among them the one that says a platform which cannot classify a waiting
/// child must answer `None` rather than guess. Nothing reported it: an
/// undeclared file is not an error, and the three helpers it was the only
/// caller of degraded into dead-code warnings that read like ordinary lint
/// noise.
///
/// The list of files is read from the directory at run time rather than
/// written down, so adding a suite and forgetting the `mod` line turns this
/// red instead of passing in silence.
///
/// This does NOT catch: the same mistake in another crate's `src/tests`, a
/// module declared behind a `cfg` that no target satisfies, or a test that is
/// compiled and then filtered out of the run.
#[test]
fn every_test_file_in_this_directory_is_declared() {
    let declarations = include_str!("mod.rs");
    let mut orphans: Vec<String> = std::fs::read_dir(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/tests"
    ))
    .expect("the test directory this file lives in must be readable")
    .map(|entry| entry.expect("a directory entry must be readable").path())
    .filter(|path| path.extension().is_some_and(|e| e == "rs"))
    .filter_map(|path| {
        let stem = path.file_stem()?.to_str()?.to_owned();
        (stem != "mod" && !declarations.contains(&format!("mod {stem};"))).then_some(stem)
    })
    .collect();
    orphans.sort();

    assert!(
        orphans.is_empty(),
        "these test files are never compiled or run because nothing declares \
         them: {}. Add `mod <name>;` to crates/vitrum-core/src/tests/mod.rs.",
        orphans.join(", ")
    );
}
