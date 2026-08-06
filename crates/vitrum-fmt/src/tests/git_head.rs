//! Branch names, ref prefixes, and detached HEAD.

use crate::git::{Head, branch, head, short_commit, strip_ref_prefix};
use crate::text::display_width;

/// A fully qualified branch ref loses its `refs/heads/` prefix.
///
/// `git symbolic-ref HEAD` and `git for-each-ref` both hand back the long form.
/// Eleven columns of `refs/heads/` on every sidebar row says nothing that the
/// column header does not already say.
#[test]
fn the_refs_heads_prefix_is_stripped() {
    assert_eq!(strip_ref_prefix("refs/heads/main"), "main");
    assert_eq!(strip_ref_prefix("refs/heads/feature/x"), "feature/x");
    assert_eq!(branch("refs/heads/main", 20), "main");
}

/// A remote-tracking ref keeps its remote name.
///
/// `refs/remotes/origin/main` becomes `origin/main`, not `main`: the remote is
/// the informative half, and two rows reading `main` where one is local and one
/// is a remote branch would be indistinguishable.
#[test]
fn a_remote_tracking_ref_keeps_its_remote() {
    assert_eq!(strip_ref_prefix("refs/remotes/origin/main"), "origin/main");
    assert_eq!(strip_ref_prefix("refs/remotes/upstream/dev"), "upstream/dev");
}

/// A branch name that is not a ref path is left exactly alone.
///
/// A branch may legitimately be called `refs` or `headsman`, and a
/// `trim_start_matches`-style strip would eat into it.
#[test]
fn a_plain_branch_name_is_untouched() {
    assert_eq!(strip_ref_prefix("main"), "main");
    assert_eq!(strip_ref_prefix("feature/refs/heads/x"), "feature/refs/heads/x");
    assert_eq!(strip_ref_prefix("headsman"), "headsman");
    assert_eq!(strip_ref_prefix("refs/tags/v1.0"), "refs/tags/v1.0");
}

/// A branch name that fits is shown in full.
#[test]
fn a_branch_that_fits_is_shown_in_full() {
    assert_eq!(branch("main", 20), "main");
    assert_eq!(branch("main", 4), "main", "exactly at budget");
    assert_eq!(head(Head::Branch("main"), 20), "main");
}

/// A long branch name is cut in the middle, not at the end.
///
/// Team conventions front-load the shared part (`feature/`, `renovate/`,
/// `dependabot/npm_and_yarn/`). A tail cut leaves every row reading the same
/// prefix and the distinguishing part is exactly what gets thrown away.
#[test]
fn a_long_branch_is_cut_in_the_middle() {
    let name = "feature/renovate/bump-serde";
    assert_eq!(display_width(name), 27);
    let short = branch(name, 20);
    assert_eq!(short, "feature/re\u{2026}ump-serde");
    assert_eq!(display_width(&short), 20);
    assert!(short.starts_with("feature/"), "the prefix survives");
    assert!(short.ends_with("serde"), "the distinguishing tail survives");
}

/// The ref prefix is stripped before the budget is applied.
///
/// Truncating first would spend the whole budget on `refs/heads/` and elide
/// the actual name.
#[test]
fn the_prefix_is_stripped_before_truncating() {
    assert_eq!(branch("refs/heads/main", 6), "main");
    assert_eq!(
        branch("refs/heads/feature/renovate/bump-serde", 20),
        branch("feature/renovate/bump-serde", 20)
    );
}

/// A detached HEAD is a named state, not a blank.
///
/// A bisect, a checked-out tag, and a CI checkout are all normal and none of
/// them is an error. A blank branch cell is indistinguishable from "we could
/// not read the repository", and those need different reactions.
#[test]
fn a_detached_head_is_labelled() {
    let label = head(Head::Detached("1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b"), 24);
    assert_eq!(label, "detached @ 1a2b3c4");
    assert_eq!(display_width(&label), 18);
    assert!(Head::Detached("1a2b3c4").is_detached());
    assert!(!Head::Branch("main").is_detached());
}

/// A tight budget drops the word before it drops the commit id.
///
/// The commit is the only part that identifies anything. Truncating
/// `detached @ 1a2b3c4` in the middle would produce `detach…2b3c4`, which loses
/// the front of the id and keeps a fragment of a word nobody needed.
#[test]
fn a_tight_budget_keeps_the_commit_and_drops_the_word() {
    let commit = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";
    assert_eq!(head(Head::Detached(commit), 18), "detached @ 1a2b3c4");
    assert_eq!(head(Head::Detached(commit), 17), "@1a2b3c4");
    assert_eq!(head(Head::Detached(commit), 8), "@1a2b3c4");
    assert_eq!(head(Head::Detached(commit), 4), "@1a…");
    assert_eq!(head(Head::Detached(commit), 0), "");
}

/// A commit id is abbreviated to seven hex characters.
///
/// Seven is what `git log --oneline` shows and what every commit message
/// references. Forty characters would consume a whole sidebar row.
#[test]
fn a_commit_id_is_abbreviated_to_seven_characters() {
    assert_eq!(
        short_commit("1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b"),
        "1a2b3c4"
    );
    assert_eq!(short_commit("1234567"), "1234567", "already seven");
    assert_eq!(short_commit("abcdef"), "abcdef", "six is left alone");
}

/// Something that is not a hex id is not abbreviated.
///
/// A caller may pass a tag or a `HEAD~2`. Slicing the first seven characters
/// off `v1.2.3-rc1` produces `v1.2.3-`, which resolves to nothing.
#[test]
fn a_non_hex_reference_is_not_abbreviated() {
    assert_eq!(short_commit("v1.2.3-rc1"), "v1.2.3-rc1");
    assert_eq!(short_commit("HEAD~2"), "HEAD~2");
    assert_eq!(short_commit("release-2026"), "release-2026");
    assert_eq!(short_commit(""), "");
}

/// A repository with no commits is its own state.
///
/// `git init` then nothing: HEAD points at an unborn branch. It is neither a
/// branch to show nor a detached commit, and showing either would be a lie.
#[test]
fn an_unborn_head_is_its_own_state() {
    assert_eq!(head(Head::Unborn, 20), "no commits");
    assert_eq!(head(Head::Unborn, 10), "no commits", "exactly at budget");
    assert_eq!(head(Head::Unborn, 5), "no c\u{2026}");
    assert_eq!(head(Head::Unborn, 0), "");
    assert!(!Head::Unborn.is_detached());
}

/// A branch name with wide characters is measured in columns.
///
/// Branch names carry CJK in plenty of teams. Measuring them by character
/// would let a ten-character Japanese branch overflow a twenty-column cell.
#[test]
fn a_wide_character_branch_respects_its_column_budget() {
    let name = "機能/セッション一覧";
    assert_eq!(name.chars().count(), 10);
    assert_eq!(display_width(name), 19);
    let short = branch(name, 12);
    assert_eq!(short, "機能/セ\u{2026}一覧");
    assert_eq!(display_width(&short), 12);
}

/// No head label ever exceeds its budget, at any budget.
///
/// Sweeps every state and every width, because the detached form has three
/// separate degradation paths and only one of them is exercised by a typical
/// sidebar width.
#[test]
fn no_head_label_ever_exceeds_its_budget() {
    let commit = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";
    let heads = [
        Head::Branch("main"),
        Head::Branch("refs/heads/feature/renovate/bump-serde-to-1-0-200"),
        Head::Branch("機能/セッション一覧"),
        Head::Detached(commit),
        Head::Detached("v1.2.3-rc1"),
        Head::Unborn,
    ];
    for candidate in heads {
        for budget in 0..=30 {
            let label = head(candidate, budget);
            assert!(
                display_width(&label) <= budget,
                "head({candidate:?}, {budget}) = {label:?} is too wide"
            );
        }
    }
}

/// An empty branch name yields an empty label rather than a stray ellipsis.
///
/// `refs/heads/` with nothing after it should not happen, but a malformed ref
/// from a corrupt repository must not put a lone `…` in the sidebar as if a
/// name had been elided.
#[test]
fn an_empty_branch_name_yields_an_empty_label() {
    assert_eq!(branch("refs/heads/", 20), "");
    assert_eq!(branch("", 20), "");
    assert_eq!(head(Head::Branch(""), 20), "");
}
