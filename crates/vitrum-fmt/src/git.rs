//! Git head display: `main`, `feature/…/rename`, `detached @ 1a2b3c4`.
//!
//! # Detached HEAD is a state, not a missing branch
//!
//! A worktree with no current branch is normal (a bisect, a checked-out tag, a
//! CI checkout) and it is not an error. It gets its own label rather than a
//! blank, because a blank branch cell is indistinguishable from "we failed to
//! read the repository", and those need different reactions from the user. A
//! repository with no commits yet is a third distinct state.
//!
//! Long branch names are elided in the middle rather than the end. Team naming
//! conventions front-load the shared part (`feature/`, `renovate/`,
//! `dependabot/npm_and_yarn/`), so cutting the tail would leave every row
//! reading the same.

use crate::text;

/// What a worktree currently has checked out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Head<'a> {
    /// On a branch. May be a full ref name; [`strip_ref_prefix`] is applied.
    Branch(&'a str),
    /// Detached at a commit.
    Detached(&'a str),
    /// A repository with no commits, so HEAD points at an unborn branch.
    Unborn,
}

impl Head<'_> {
    /// Whether there is no current branch.
    #[must_use]
    pub const fn is_detached(self) -> bool {
        matches!(self, Self::Detached(_))
    }
}

/// Drop a `refs/heads/` or `refs/remotes/` prefix from a ref name.
///
/// `git` hands back a fully qualified ref in several situations
/// (`symbolic-ref HEAD`, `for-each-ref`), and `refs/heads/main` in a sidebar
/// cell is eleven wasted columns that say nothing. A remote-tracking ref keeps
/// its remote (`refs/remotes/origin/main` becomes `origin/main`) because the
/// remote name is the informative part.
#[must_use]
pub fn strip_ref_prefix(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/remotes/"))
        .unwrap_or(name)
}

/// The abbreviated form of a commit id: the first 7 characters of a hex id.
///
/// Anything that is not a hex id of at least 7 characters is returned
/// unchanged, because a tag name or a `HEAD~2` is already short and cutting it
/// would corrupt it.
#[must_use]
pub fn short_commit(commit: &str) -> &str {
    let hex = commit.len() >= 7 && commit.bytes().all(|b| b.is_ascii_hexdigit());
    if hex { &commit[..7] } else { commit }
}

/// A branch name shortened to `budget` columns, with any ref prefix removed.
#[must_use]
pub fn branch(name: &str, budget: usize) -> String {
    text::truncate_middle(strip_ref_prefix(name), budget)
}

/// The full head label for a worktree, shortened to `budget` columns.
///
/// `main`, `origin/main`, `detached @ 1a2b3c4`, `no commits`. When the budget
/// is too small for `detached @ 1a2b3c4`, the label degrades to `@1a2b3c4`
/// before it degrades to a truncation, because the commit id is the part worth
/// keeping.
#[must_use]
pub fn head(head: Head<'_>, budget: usize) -> String {
    match head {
        Head::Branch(name) => branch(name, budget),
        Head::Detached(commit) => {
            let short = short_commit(commit);
            let full = format!("detached @ {short}");
            if text::fits(&full, budget) {
                return full;
            }
            let terse = format!("@{short}");
            if text::fits(&terse, budget) {
                return terse;
            }
            text::truncate_end(&terse, budget)
        }
        Head::Unborn => text::truncate_end("no commits", budget),
    }
}
