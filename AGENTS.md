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

vitrum manages coding agents. A demo that shows a bare shell session argues it
is a terminal multiplexer, which drops it into a category where tmux, Zellij
and WezTerm already win and where nothing this product does is visible.

This rule has been broken before by assets that shipped on the front page, so
it is a gate, not a preference.

### Banned outright

No screenshot, GIF, video, tape, README image, docs image, review artifact,
PR body image, release note or social post may contain any of:

- a session named for a shell or a build tool: `bash`, `zsh`, `fish`, `sh`,
  `/bin/bash`, `git`, `cargo`, `make`, `npm`, `docker`
- the output of a build, test run, linter or version control command:
  `cargo test`, `cargo build`, `git log`, `git status`, `npm run`, a compiler
  diagnostic, a test summary line
- a bare shell prompt, or `ls`, `cat`, `htop`, `top`, `df`, `tree` or any
  other system utility
- a launcher, recents list or preset list whose entries are shell commands
- a `+ bash` style control offered as the way to start work
- prose or alt text that describes the product through a shell task

The list is illustrative, not exhaustive. The test is the category: if the
same picture could have been taken in tmux, it does not ship.

### Required instead

Every demo asset shows coding agents in the states only this product
surfaces:

- an agent working, with its provider mark on the row
- an agent blocked on approval, waiting for an answer
- an agent that finished while you were looking at something else
- a snoozed session
- several projects in the sidebar, each with more than one agent

The sidebar is the subject. A terminal pane is background, and what is in it
is an agent's transcript.

### The gate

A PR that adds or changes any image answers this in its body: which agents
are on screen, and in which states? An answer naming a shell command, or no
answer, means the PR does not open.

Assets already in the tree that break this rule are defects, not precedent.
Replace them rather than matching them.

That a session can run any command is true, and it is not the pitch. Never
demonstrate it.

## Never show a real machine's paths

Nothing published from this repository shows a path from the machine that
produced it. Not in a screenshot, a GIF, a tape, a terminal pane, a launcher
field, a title bar, a log line, a test fixture, a doc example or a commit
message.

Banned in any committed artifact:

- a home directory: `/home/<name>`, `/Users/<name>`, `C:\Users\<name>`
- a mount, volume or archive path from the build machine
- a scratch path: `/tmp/...`, staging directories, cache directories
- a checkout location, worktree path or target directory

Use these instead, and only these:

- `~/src/<project>` for a checkout
- `/src/<project>` where the fixture wants an absolute path
- `/home/mk` as the synthetic home already used across the fmt and os tests

A path leaks who produced the artifact and where they keep their work, and it
resolves to nothing for a reader. Before committing an image, read every
visible string in it: the title bar, the launcher's directory field, the
session subtitle and the pane. One of those is where a path gets out.
