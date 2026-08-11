# libghostty-vt-sys, forked

A fork of [`libghostty-vt-sys`](https://crates.io/crates/libghostty-vt-sys)
0.2.1 carrying two changes. In `build.rs`, the target and CPU passed to zig are
pinned, on every target, instead of being detected from the machine running the
build. In `src/bindings.rs`, the bindings are regenerated with
`generate_comments(false)`, so the C header's prose is not carried into a file
published from this repository. That prose names another terminal on nearly
every item. The items, their types and their values are unchanged.

`UPSTREAM.toml` is the definition of the divergences.
`sh tools/upstream/check.sh --fork vendor-ghostty-vt-sys` downloads the
pristine crate and fails if the real divergence is not the declared one, or if
upstream has released a newer version.

## Why it exists

Upstream passes `-Dtarget` to zig only when `TARGET` differs from `HOST`. A
native build therefore lets zig read the builder's CPU and compile for it.

Every target this project publishes is built natively, on a runner of the same
architecture as the target, so that condition holds on every release build. The
instruction set of a release was whatever CPU the runner happened to have.

Two release builds of this crate on one desktop, same version, produced
different instruction sets: one with 5581 AVX-512 instructions, one with 3662
AVX2. The second is the configuration the release workflow uses. Either one is
an illegal instruction on a CPU without the extension, and the user sees a bare
`SIGILL` with nothing to search for. AVX2 is the wider failure of the two: it is
absent on everything before Haswell and on every Atom, Celeron and Pentium
through Jasper Lake.

The fork passes `-Dtarget` unconditionally and adds `-Dcpu=baseline`, so the
floor is the architecture's lowest model — `x86_64-v1`, `armv8-a` — and is
never read from the build machine.

## What it does not cover

This is applied through `[patch.crates-io]` in the workspace root, which is
ignored by registry builds. Someone running `cargo install vitrum` compiles
against upstream's build script and can still get a host-tuned binary.

The exposure that matters is closed, because what this project publishes is
prebuilt archives built from this repository, and `tools/release/check-isa.sh`
disassembles each of them in CI and fails the release if anything above the
floor appears. Closing the registry path as well means forking `libghostty-vt`
too, so its dependency resolves to a name this project owns. That is a much
larger fork to maintain for a much smaller exposure, and it is not worth it
while the change is small enough for upstream to take.

## Absorbing a newer upstream

1. `sh tools/upstream/check.sh --fork vendor-ghostty-vt-sys --patches /tmp/p`
   writes the divergence as a patch against 0.2.1.
2. Download the new version and replace everything here except `UPSTREAM.toml`,
   `README.md` and `Cargo.toml`.
3. Apply the patch. If it applies clean, update the version in `UPSTREAM.toml`
   and `Cargo.toml` and re-run the check.
4. Copy any `*.workspace = true` field from the **published** `Cargo.toml` in
   the registry cache, not from `Cargo.toml.orig`. The published one has those
   fields already resolved to upstream's values; `Cargo.toml.orig` inherits
   them from a workspace that does not exist here, and replacing them by guess
   is how this fork first broke the build.
5. Rebuild and run `tools/release/check-isa.sh` on the result. The pin is only
   real if the disassembly says so.

If upstream takes the change, delete this directory, the `[patch.crates-io]`
entry and the workspace member line. The gate stays.
