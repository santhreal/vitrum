# Agent rules for vitrum

Rules for anyone (human or agent) changing this repository. Keep them short.
If a rule here conflicts with a PR description, this file wins until it is
updated in its own PR.

## Distinct concerns get distinct PRs

When work is distinct, ship it as its own PR. Do not bundle unrelated changes
into one branch because they happened in the same session.

Examples of distinct concerns that must not share a PR:

- adding or editing this file
- a performance change in a crate
- a UI refinement
- a bench / fuzz harness change
- a reopen or salvage of an old branch

One concern per commit when practical. One concern per PR always.

## UI changes require before and after screenshots

Every PR that changes UI — layout, chrome, sheets, dialogs, titlebar, sidebar,
terminal pane framing, settings, What's New, update prompts, empty states,
motion, colour, typography, or any other painted surface — must include:

1. a **before** screenshot of the affected surface on `main` (or the PR base)
2. an **after** screenshot of the same surface with the change applied

Put both in the PR body. Crop to the relevant region when that makes the
diff clearer; otherwise show the full window. Do not open or merge a UI PR
that lacks them.

This applies to small refinements as well as large ones, including a minimal
"update available" affordance and any polish of the post-update changelog
sheet.

## One writer per worktree, and the canonical tree only integrates

Several agents work in this repository at once. The rule that keeps that from
losing work is that a worktree has one writer, and the tree at the repository
root is not one of them: it stages, gates, reviews and lands, and nothing else.
A change is written in a lane, and it arrives here as a merge.

`tools/integrate.py lanes` prints every worktree, what is uncommitted in it,
and what it holds that `main` does not.

What is safe and what is not:

- **A commit is safe.** Refs live in the shared repository, so a worktree can
  be deleted and every branch it made is still here.
- **Uncommitted work is not.** It exists only in that directory.
- **A lane under `/tmp` does not survive a reboot.** Put lanes somewhere
  durable. `lanes` marks the volatile ones.

So: commit before you leave a lane, even if the commit is scratch. If it is not
worth a commit on the branch, `git stash create` makes a commit object without
touching the working tree, and `git branch wip/<lane>-<what> <sha>` gives it a
name that outlives the directory.

Before editing a file another lane is holding, ask that lane. Two agents in one
file is the one case this protocol does not cover.

## Batches are staged, gated once, and never squashed

A wave of pull requests is merged onto a staging branch, gated as a whole, then
landed fast-forward. Two changes that each pass alone and fail together are
invisible to per-pull-request gating and this is the only place they surface.

Each pull request keeps its own `--no-ff` merge commit inside the batch. A
squashed batch is one commit that `git bisect` can only point at whole, which
is the single cost of batching that cannot be undone afterwards.

Landing is `--ff-only`, so `main` is never rewritten and a wave that went stale
is restaged rather than forced.
