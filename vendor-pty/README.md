# vitrum-portable-pty

A fork of [`portable-pty`](https://crates.io/crates/portable-pty) 0.9.0 with two
divergences.

## The divergences

`src/win/psuedocon.rs` creates the pseudoconsole without
`PSUEDOCONSOLE_INHERIT_CURSOR`.

That flag makes conhost open every session by asking the terminal where the
cursor is, with `ESC [ 6 n`, and withhold every later byte until something
answers. It exists so a console launched from an existing console continues on
the same line. A terminal emulator can answer it, because it owns a grid.

vitrum's sessions are created by a daemon that has no grid, often with no window
attached at all, so there is nobody to answer from. Answering on the daemon's
behalf was tried and does not close the hole: the report has to reach conhost
before the child finishes, and for a child that writes once and exits there is no
margin. The result was a share of Windows sessions, different ones each run,
delivering the host preamble and then nothing, forever, with the child having run
and exited successfully.

Without the flag the query is never asked. The session starts at the origin, which
is what a fresh session should do anyway, and there is no handshake to lose.

`src/lib.rs` deletes one line, `#[cfg(unix)] use libc;`. The crate names `libc`
in paths rather than through that import, so on the edition this workspace
builds it is an unused import, and every build here runs with `-D warnings`.

`UPSTREAM.toml` is the authoritative copy of that list.
`sh tools/upstream/check.sh --fork vendor-pty` downloads the pristine crate and
fails if the real divergence is not the declared one, or if upstream has
released a newer version. It runs weekly in CI.

## Why a fork and not a patch

`[patch.crates-io]` applies to builds of this workspace and is ignored by anyone
who installs vitrum from a registry. A published client would have built against
upstream and shipped the hang, so the fork is a real, named dependency instead.

## When this goes away

When `portable-pty` lets a caller choose the pseudoconsole flags, and builds
without an unused import. Nothing else in the crate is modified, so tracking a
new release means copying `src/` and reapplying two hunks.

## Absorbing a new release

1. `sh tools/upstream/check.sh --fork vendor-pty --patches /tmp/p` writes the
   divergence as a patch against 0.9.0.
2. Download the new version and replace `src/`.
3. Reapply the patch, set the new version in `UPSTREAM.toml` and in
   `Cargo.toml`, and run the check again.
