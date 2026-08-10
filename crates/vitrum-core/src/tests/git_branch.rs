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

// A repository is untrusted input.
//
// The class these close: `git_branch` treating `.git` as data the product
// wrote. It is not. The directory arrives on `createSession` from the client,
// resolution then walks up from it, and the two files it reads are named by
// whatever is on disk there. Reading them with `fs::read_to_string` makes the
// size of the allocation and the duration of the call properties of that
// directory, and this runs on the spawn path, on a Tokio worker.
//
// What they do not catch: a file that is regular and under the cap and still
// hostile. That is the parser's problem, and the cases above cover it.

/// A HEAD larger than the cap must report no branch.
///
/// Truncating it instead would be worse than refusing: the first line of a
/// large file is a perfectly plausible ref name, so a truncated read reports a
/// branch the repository is not on.
#[test]
fn an_oversized_head_reports_none() {
    let dir = TempDir::new("hugehead");
    let mut head = String::from("ref: refs/heads/");
    head.push_str(&"a".repeat(64 * 1024));
    dir.write(".git/HEAD", &head);
    assert_eq!(git_branch(&dir.path), None);
}

/// An oversized `.git` pointer file must report no branch. It is read before
/// HEAD is, so it is the first of the two files a directory chooses the size
/// of.
///
/// This one also passes against an unbounded read, because the padding makes
/// the path bogus and the parse refuses it anyway. It is here to pin the
/// outcome against a later parser that tolerates trailing junk and would then
/// hand a directory-sized string to that parse.
#[test]
fn an_oversized_gitdir_file_reports_none() {
    let dir = TempDir::new("hugegitdir");
    let real = TempDir::new("hugegitdir-real");
    real.write("HEAD", "ref: refs/heads/main\n");
    let mut pointer = format!("gitdir: {}\n", real.path.display());
    pointer.push_str(&"#".repeat(64 * 1024));
    dir.write(".git", &pointer);
    assert_eq!(git_branch(&dir.path), None);
}

/// A HEAD that is a fifo must report no branch, and must do so now.
///
/// Opening a reader-side fifo with no writer blocks until one arrives. Nobody
/// is going to write to this one, so a blocking open parks the calling thread
/// for the life of the process. The failure this asserts against is therefore
/// a hang, and the assertion is that the call returns at all.
#[cfg(unix)]
#[test]
fn a_fifo_head_does_not_park_the_caller() {
    use std::ffi::CString;

    let dir = TempDir::new("fifohead");
    std::fs::create_dir_all(dir.join(".git")).expect("creating .git");
    let path = CString::new(dir.join(".git/HEAD").into_os_string().into_encoded_bytes())
        .expect("a temp path holds no interior nul");
    // SAFETY: a nul-terminated path into a directory this test just created.
    let made = unsafe { libc::mkfifo(path.as_ptr(), 0o644) };
    assert_eq!(made, 0, "mkfifo failed: {}", std::io::Error::last_os_error());

    let (tx, rx) = std::sync::mpsc::channel();
    let probe = dir.path.clone();
    std::thread::spawn(move || {
        let _ = tx.send(git_branch(&probe));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(branch) => assert_eq!(branch, None, "a fifo is not a ref"),
        Err(_) => panic!("git_branch has not returned after 5s: a fifo in .git parks the spawn path"),
    }
}

/// A directory at HEAD must report no branch rather than an io error path that
/// depends on the platform.
#[test]
fn a_directory_at_head_reports_none() {
    let dir = TempDir::new("dirhead");
    std::fs::create_dir_all(dir.join(".git/HEAD")).expect("creating a HEAD directory");
    assert_eq!(git_branch(&dir.path), None);
}
