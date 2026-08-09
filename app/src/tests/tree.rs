//! What "the files in this repository" means to a guard that walks the tree.
//!
//! Several guards here answer a question of the form "is every X in the tree
//! accounted for": every script is in the JavaScript bill, every picture is
//! explained by a document. Each one grew its own directory walk with its own
//! hand-kept list of directory names to skip, and both lists were written from
//! what happened to be on the machine of whoever wrote them.
//!
//! That is a list that goes stale in silence, and it did. A release build runs
//! the Zig toolchain for `vitrum-vt`, which drops a `.zig-cache` in the
//! checkout holding, among tens of thousands of other files, seven `.js` files
//! belonging to glslang, libxev and mesa. On a developer machine with a warm
//! cache elsewhere the walks never saw them; in CI, on a clean checkout that
//! builds before it tests, the JavaScript bill failed with a list of scripts
//! from a package cache and the asset guards failed with pictures from vendored
//! documentation. Nothing was wrong with the product either time.
//!
//! So the tree is defined as what git tracks. That is the same set a reader
//! sees on the forge, it is exactly the set these guards mean by "ships", and
//! it cannot acquire a member that no one committed. A build artefact, a cache,
//! an editor leaving and a scratch file are all excluded by construction rather
//! than by a name someone remembered to add.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

/// Every file git tracks, repository-relative, forward slashes, sorted.
///
/// Computed once: `git ls-files` on this repository costs a few milliseconds
/// and several guards ask for it.
static TRACKED: LazyLock<Vec<String>> = LazyLock::new(read_tracked);

/// The repository root, from the crate these tests are compiled into.
pub(crate) fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the app crate always has a parent directory")
        .to_path_buf()
}

/// Every tracked file, repository-relative with forward slashes.
pub(crate) fn tracked() -> &'static [String] {
    &TRACKED
}

fn read_tracked() -> Vec<String> {
    let root = root();
    let out = Command::new("git")
        .args(["-C", &root.to_string_lossy(), "ls-files", "-z"])
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "cannot ask git what this repository tracks ({e}); these guards \
                 compare the tree against what ships, and a walk that cannot \
                 read the tree must fail rather than pass over an empty list"
            )
        });
    assert!(
        out.status.success(),
        "git ls-files failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let mut files: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| s.replace('\\', "/"))
        .collect();
    assert!(
        !files.is_empty(),
        "git tracks no files under {}, so every guard built on this listing \
         would pass without looking at anything",
        root.display()
    );
    files.sort();
    files
}

/// The listing must be the repository, not a subdirectory of it and not empty.
///
/// WHY: every guard built on this reads as green when the listing is short,
/// because "nothing untracked was found" and "nothing was looked at" produce
/// the same result. Three files that must be in any checkout of this
/// repository pin the root the listing was taken from as well as its size.
#[test]
fn the_listing_is_this_whole_repository() {
    let files = tracked();
    for expected in ["README.md", "Cargo.toml", "app/src/main.rs"] {
        assert!(
            files.iter().any(|f| f == expected),
            "the tracked listing has {} files and none of them is {expected}; \
             it was taken from the wrong directory",
            files.len()
        );
    }
}

/// Nothing untracked reaches a guard, however the build litters the checkout.
///
/// WHY: this is the defect the module exists for. A `.zig-cache` directory
/// full of somebody else's JavaScript and documentation images appears in any
/// checkout that builds `vitrum-vt` before it tests, and the previous walks
/// read it as part of this product. The listing is asserted to exclude the
/// three shapes of build leaving that land in this repository's root, whether
/// or not this particular checkout happens to have them today.
#[test]
fn no_build_leaving_is_part_of_the_tree() {
    for litter in [".zig-cache/", "zig-out/", "target/", "dist/", ".internal/"] {
        let found: Vec<&String> = tracked()
            .iter()
            .filter(|f| f.starts_with(litter) || f.contains(&format!("/{litter}")))
            .collect();
        assert!(
            found.is_empty(),
            "{litter} is a build leaving and git tracks {} file(s) under it, \
             starting with {}; either it was committed by mistake or the \
             ignore rules stopped covering it",
            found.len(),
            found[0]
        );
    }
}
