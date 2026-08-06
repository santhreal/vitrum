//! Branch resolution for the sidebar, read from `.git` directly.

use crate::session::git_branch;
use crate::tests::helpers::TempDir;

/// A normal repository must report its branch name.
///
/// The sidebar shows it per project row. Resolving it by reading one file is the
/// point: shelling out to git per row is the documented way a sidebar becomes a
/// CPU hog, so this must work without a git binary at all.
#[test]
fn a_symbolic_head_reports_the_branch_name() {
    let dir = TempDir::new("branch");
    dir.write(".git/HEAD", "ref: refs/heads/main\n");
    assert_eq!(git_branch(&dir.path), Some("main".to_string()));
}

/// A branch name containing slashes must report the whole name.
///
/// Reporting only the last component made every `feature/login` and
/// `fix/login` read `login`, so the one cell that exists to tell two agents
/// apart said the same word for both. The row truncates from the end, which
/// keeps the part that differs.
#[test]
fn a_nested_branch_name_keeps_its_namespace() {
    let dir = TempDir::new("nested");
    dir.write(".git/HEAD", "ref: refs/heads/feature/attention\n");
    assert_eq!(git_branch(&dir.path), Some("feature/attention".to_string()));

    let remote = TempDir::new("remote");
    remote.write(".git/HEAD", "ref: refs/remotes/origin/main\n");
    assert_eq!(git_branch(&remote.path), Some("origin/main".to_string()));
}

/// A detached HEAD must report the short object id.
///
/// Returning `None` for a detached checkout would show no branch during a bisect
/// or a tag checkout, which is exactly when knowing the commit matters.
#[test]
fn a_detached_head_reports_the_short_object_id() {
    let dir = TempDir::new("detached");
    dir.write(".git/HEAD", "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b\n");
    assert_eq!(git_branch(&dir.path), Some("1a2b3c4".to_string()));
}

/// Resolution must walk up from a subdirectory, since a session's cwd is usually
/// a package inside the repository rather than its root.
#[test]
fn resolution_walks_up_from_a_subdirectory() {
    let dir = TempDir::new("walkup");
    dir.write(".git/HEAD", "ref: refs/heads/trunk\n");
    std::fs::create_dir_all(dir.join("crates/deep/inner")).expect("mkdir");
    assert_eq!(
        git_branch(&dir.join("crates/deep/inner")),
        Some("trunk".to_string())
    );
}

/// A worktree or submodule stores `.git` as a file pointing elsewhere, and that
/// indirection must be followed or every worktree shows no branch.
#[test]
fn a_gitdir_file_is_followed() {
    let dir = TempDir::new("worktree");
    dir.write("real/HEAD", "ref: refs/heads/wt\n");
    let real = dir.join("real");
    dir.write(
        "checkout/.git",
        &format!("gitdir: {}\n", real.to_string_lossy()),
    );
    assert_eq!(git_branch(&dir.join("checkout")), Some("wt".to_string()));
}

/// A relative `gitdir:` path must resolve against the file's own directory, which
/// is how git itself writes submodule pointers.
#[test]
fn a_relative_gitdir_file_resolves_against_its_parent() {
    let dir = TempDir::new("relgit");
    dir.write("sub/.git", "gitdir: ../store\n");
    dir.write("store/HEAD", "ref: refs/heads/subbranch\n");
    assert_eq!(git_branch(&dir.join("sub")), Some("subbranch".to_string()));
}

/// A directory outside any repository must report no branch rather than walking to
/// the filesystem root and inventing one from an unrelated parent repo's HEAD.
#[test]
fn a_missing_head_reports_none() {
    let dir = TempDir::new("nohead");
    std::fs::create_dir_all(dir.join(".git")).expect("mkdir");
    assert_eq!(git_branch(&dir.path), None);
}

/// A malformed HEAD must report no branch instead of surfacing garbage into the
/// sidebar or panicking on a short slice.
#[test]
fn a_malformed_head_reports_none() {
    let dir = TempDir::new("badhead");
    dir.write(".git/HEAD", "ref:\n");
    assert_eq!(git_branch(&dir.path), None);

    let short = TempDir::new("shorthead");
    short.write(".git/HEAD", "abc\n");
    assert_eq!(git_branch(&short.path), None);
}

/// An empty HEAD must not panic, which is the state git leaves during some
/// interrupted operations.
#[test]
fn an_empty_head_reports_none() {
    let dir = TempDir::new("emptyhead");
    dir.write(".git/HEAD", "");
    assert_eq!(git_branch(&dir.path), None);
}
