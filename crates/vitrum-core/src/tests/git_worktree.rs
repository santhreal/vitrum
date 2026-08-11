//! Worktree resolution for the sidebar, read from `.git` directly.
//!
//! A linked worktree is a second checkout of one repository. Two of them are
//! ordinarily on different branches, but nothing stops two being on the same
//! one, and a sidebar that shows only the branch then draws two rows that are
//! word for word identical while the agents in them are working on different
//! trees. The worktree name is what distinguishes them.
//!
//! The name is git's own: the directory component under `.git/worktrees`. It
//! is unique within the repository by construction, and it is a name rather
//! than a path, so it never puts a filesystem location on screen.

use crate::SessionManager;
use crate::session::{GitContext, git_context};
use crate::tests::helpers::{TempDir, shell_spec};

/// Build `<root>/main/.git/worktrees/<name>/HEAD` and a checkout pointing at
/// it, and return the checkout's path.
fn linked_worktree(dir: &TempDir, name: &str, branch: &str) -> std::path::PathBuf {
    dir.write(
        &format!("main/.git/worktrees/{name}/HEAD"),
        &format!("ref: refs/heads/{branch}\n"),
    );
    let target = dir.join(&format!("main/.git/worktrees/{name}"));
    dir.write(
        &format!("{name}-checkout/.git"),
        &format!("gitdir: {}\n", target.to_string_lossy()),
    );
    dir.join(&format!("{name}-checkout"))
}

/// A linked worktree must report the name git gave it.
#[test]
fn a_linked_worktree_reports_its_name() {
    let dir = TempDir::new("wt-name");
    let checkout = linked_worktree(&dir, "attention", "attention");
    let git = git_context(&checkout);
    assert_eq!(git.worktree, Some("attention".to_string()));
    assert_eq!(git.branch, Some("attention".to_string()));
}

/// Two worktrees of one repository must not collide, which is the whole reason
/// the name is read from `.git/worktrees` instead of from the checkout.
///
/// Both are on the same branch here, because that is the case the branch cell
/// cannot tell apart and the case this exists for.
#[test]
fn two_worktrees_on_one_branch_are_still_distinguishable() {
    let dir = TempDir::new("wt-pair");
    let left = linked_worktree(&dir, "attention", "main");
    let right = linked_worktree(&dir, "review", "main");
    let left = git_context(&left);
    let right = git_context(&right);
    assert_eq!(left.branch, right.branch, "the branch cell cannot separate them");
    assert_eq!(left.worktree, Some("attention".to_string()));
    assert_eq!(right.worktree, Some("review".to_string()));
    assert_ne!(left.worktree, right.worktree);
}

/// The worktree must be a name and never a path.
///
/// A path names the machine that produced it and resolves to nothing for
/// anyone reading the sidebar. The pointer file holds an absolute path, so
/// this is the assertion that the path is not what gets published.
#[test]
fn the_worktree_is_a_name_and_not_a_path() {
    let dir = TempDir::new("wt-path");
    let checkout = linked_worktree(&dir, "attention", "main");
    let name = git_context(&checkout).worktree.expect("a linked worktree");
    assert!(
        !name.contains('/') && !name.contains('\\'),
        "the worktree must be a bare name, got {name:?}"
    );
    assert!(
        !name.contains(&*dir.path.to_string_lossy()),
        "the worktree must not carry the checkout's location, got {name:?}"
    );
}

/// An ordinary repository has no worktree, and saying it has one would put a
/// second cell on every row in the product.
#[test]
fn a_plain_repository_reports_no_worktree() {
    let dir = TempDir::new("wt-plain");
    dir.write(".git/HEAD", "ref: refs/heads/main\n");
    let git = git_context(&dir.path);
    assert_eq!(git.branch, Some("main".to_string()));
    assert_eq!(git.worktree, None);
}

/// A submodule stores the same kind of pointer file and is not a worktree.
///
/// Git writes `gitdir: <repo>/.git/modules/<name>` for one. Taking the last
/// component of any pointer would label every submodule as a worktree, which
/// is a wrong answer rather than a missing one.
#[test]
fn a_submodule_is_not_a_worktree() {
    let dir = TempDir::new("wt-submodule");
    dir.write("main/.git/modules/vendor/HEAD", "ref: refs/heads/main\n");
    let target = dir.join("main/.git/modules/vendor");
    dir.write("vendor/.git", &format!("gitdir: {}\n", target.to_string_lossy()));
    let git = git_context(&dir.join("vendor"));
    assert_eq!(git.branch, Some("main".to_string()), "a submodule still has a branch");
    assert_eq!(git.worktree, None);
}

/// A pointer whose target no longer exists must report no worktree.
///
/// `git worktree remove` deletes the administrative directory. A checkout left
/// behind by hand still holds the pointer, and naming a worktree that is gone
/// puts a tree on screen that nothing can be done with.
#[test]
fn a_dangling_pointer_reports_no_worktree() {
    let dir = TempDir::new("wt-dangling");
    let target = dir.join("main/.git/worktrees/attention");
    dir.write("checkout/.git", &format!("gitdir: {}\n", target.to_string_lossy()));
    let git = git_context(&dir.join("checkout"));
    assert_eq!(git.worktree, None);
    assert_eq!(git.branch, None, "there is no HEAD to read either");
}

/// A pointer that names `.git/worktrees` itself, with no worktree under it,
/// must not report `worktrees` as the name.
#[test]
fn the_worktrees_directory_is_not_itself_a_worktree() {
    let dir = TempDir::new("wt-bare");
    dir.write("main/.git/worktrees/HEAD", "ref: refs/heads/main\n");
    let target = dir.join("main/.git/worktrees");
    dir.write("checkout/.git", &format!("gitdir: {}\n", target.to_string_lossy()));
    assert_eq!(git_context(&dir.join("checkout")).worktree, None);
}

/// A `.git` file that is not a pointer must not blank the branch.
///
/// A `.git` that is a file and does not parse as `gitdir:` is not a
/// repository, so resolution keeps walking up and the checkout is still
/// inside its parent repository. Stopping there instead would blank the row's
/// whole context line over one unreadable file.
#[test]
fn an_unreadable_pointer_still_leaves_a_branch() {
    let dir = TempDir::new("wt-unreadable");
    dir.write(".git/HEAD", "ref: refs/heads/main\n");
    dir.write("checkout/.git", "\u{0}\u{1}not a pointer\n");
    let git = git_context(&dir.join("checkout"));
    assert_eq!(git.branch, Some("main".to_string()));
    assert_eq!(git.worktree, None);
}

/// Resolution must walk up from a package inside the worktree, which is where
/// a session's directory usually is.
#[test]
fn resolution_walks_up_from_inside_a_worktree() {
    let dir = TempDir::new("wt-walk");
    let checkout = linked_worktree(&dir, "attention", "attention");
    std::fs::create_dir_all(checkout.join("crates/inner")).expect("mkdir");
    let git = git_context(&checkout.join("crates/inner"));
    assert_eq!(git.worktree, Some("attention".to_string()));
    assert_eq!(git.branch, Some("attention".to_string()));
}

/// Nothing anywhere is not a repository and not a worktree.
#[test]
fn a_directory_outside_a_repository_reports_neither() {
    let dir = TempDir::new("wt-none");
    std::fs::create_dir_all(dir.join("plain")).expect("mkdir");
    let git = git_context(&dir.join("plain"));
    assert_eq!(git.branch, None);
    assert_eq!(git.worktree, None);
}

/// Nothing resolved is the default, so a repository that answers neither
/// question cannot arrive holding a stale name.
#[test]
fn the_empty_context_names_nothing() {
    let git = GitContext::default();
    assert_eq!(git.branch, None);
    assert_eq!(git.worktree, None);
}

/// A session started in a linked worktree must publish it.
///
/// The resolution above is worth nothing if the snapshot the client reads
/// carries a constant. This is the only test that crosses from the `.git` read
/// to the field on the wire, and it is what fails if the field is ever wired
/// back to a placeholder.
#[tokio::test]
async fn a_session_in_a_worktree_publishes_it() {
    let dir = TempDir::new("wt-session");
    let checkout = linked_worktree(&dir, "attention", "attention");
    let mut spec = shell_spec("exit 0");
    spec.cwd = checkout;

    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(spec).expect("spawning a session in a worktree");
    let info = mgr.info(id).expect("the session is live");
    assert_eq!(info.worktree, Some("attention".to_string()));
    assert_eq!(info.git_branch, Some("attention".to_string()));
}

/// A session outside a worktree must publish nothing rather than the last
/// worktree some other session was in.
#[tokio::test]
async fn a_session_outside_a_worktree_publishes_nothing() {
    let dir = TempDir::new("wt-session-plain");
    dir.write(".git/HEAD", "ref: refs/heads/main\n");
    let mut spec = shell_spec("exit 0");
    spec.cwd = dir.path.clone();

    let mgr = SessionManager::new(1024);
    let id = mgr.spawn(spec).expect("spawning a session outside a worktree");
    let info = mgr.info(id).expect("the session is live");
    assert_eq!(info.worktree, None);
    assert_eq!(info.git_branch, Some("main".to_string()));
}
