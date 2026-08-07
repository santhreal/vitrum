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
## Demos show agents, not shell output

vitrum manages coding agents. It is not a terminal multiplexer, and a demo
that shows it running `ls`, `htop`, `cat`, a build log, or any other bare
shell session argues the opposite: it puts the product in a category where
tmux, Zellij and Wezterm already win, and where nothing vitrum does is
visible.

So every screenshot, tape, GIF and README image shows sessions that are
coding agents, in the states only this product surfaces: an agent working, an
agent blocked on approval, an agent that finished while you were elsewhere, a
snoozed session, several projects grouped in the sidebar. The sidebar is the
subject. A terminal pane is background, and it holds an agent.

That a session can run any command is true and is not the pitch. Do not
demonstrate it.
