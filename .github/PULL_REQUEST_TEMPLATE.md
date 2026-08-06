<!-- Fill in what applies and delete the rest. A typo fix needs the title and
     one line. -->

## What this changes

<!-- What is different for someone using vitrum, not which files moved. -->

## Why

<!-- The issue it closes, the report it came from, or the reasoning if neither
     exists. For a refactor, say what the alternative looked like. -->

## Gates

CI runs the `test` job on Ubuntu, macOS and Windows with `RUSTFLAGS=-D warnings`,
plus a `release archive` job that builds the asset and verifies its checksum.
These are the same gates `RELEASING.md` names.

- [ ] `cargo build --release --workspace --locked` is clean under
      `RUSTFLAGS=-D warnings`. A warning is a failure here.
- [ ] `cargo test --release --workspace --locked` passes.
- [ ] Behaviour change: it carries a test, and I ran that test against the tree
      without the fix and watched it fail.
- [ ] Observable behaviour moved: the docs moved with it in this same change.
      `README.md`, `CHANGELOG.md`, `SPEC.md`, and the `--help` text, whichever
      of them said the old thing.
- [ ] I did not run `cargo fmt`. Formatting in this tree is by hand, and a
      reformat buries the change it ships with.

`cargo clippy --release --workspace` is advisory. Fix real defects it names and
ignore the style notes.

## Risk

<!-- What could break. Say so if this touches the daemon protocol, the PTY
     path, scrollback, the updater, or release packaging. -->
