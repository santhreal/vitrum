# Contributing to vitrum

vitrum is a GUI terminal that runs coding agents in real PTYs: a `vitrum`
client and a `vitrum-server` daemon. The useful contributions are usually
small. A session that renders wrong with a reproduction, a keybinding that
does not fire, a platform where the daemon says the wrong thing when it
cannot start.

Read the [Code of Conduct](CODE_OF_CONDUCT.md) before you open anything.

## The tree

- `app/` - the `vitrum` binary: window chrome, the sidebar, the launcher,
  settings, the updater.
- `crates/vitrum-server/` - the `vitrum-server` daemon. It owns the PTYs and
  listens on loopback.
- `crates/vitrum-proto/` - the wire protocol both ends speak, including
  `PROTOCOL_VERSION` and the hello/welcome handshake.
- `crates/vitrum-core/` - shared session types and the registry the daemon
  and the client agree on.
- `crates/vitrum-grid/` - the terminal grid.
- `crates/vitrum-model/`, `crates/vitrum-fmt/`, `crates/vitrum-os/`,
  `crates/vitrum-search/`, `crates/vitrum-replay/`, `crates/vitrum-bench/` -
  the supporting crates: UI state, formatting, platform calls, scrollback
  search, recorded sessions, benchmarks.
- `vendor/` and `vendor-pty/` - vendored forks. See the fork policy below.
- `harness/` - the measurement scripts `docs/performance.md` reports.
- `packaging/` - the release archive script CI runs on every push.

## Build and test

```sh
cargo build --release --workspace
cargo test  --release --workspace
```

The toolchain is pinned by `rust-toolchain.toml`. `rustup` reads it and
installs the right nightly by itself, so you do not pick a version.

Two system dependencies, not one. The client links the system webview, and
`docs/install.md` lists the package name for each platform.

`vitrum-vt` also needs a **Zig toolchain, exactly 0.15.2**, because its default
`vendored` feature builds libghostty from source and pins the engine commit the
tests ran against. Without `zig` on your `PATH` the two commands above stop at
`libghostty-vt-sys` with `failed to execute zig build: No such file or
directory`, which comes from that crate's build script and does not say what
to install.

The version is fixed by Ghostty's own pin, and a newer Zig fails Ghostty's
build-version check, so installing the latest release is a different failure
rather than a safer one. CI pins 0.15.2 in every workflow that builds the
engine, and a test asserts this paragraph still names the same version they
do.

The `system` feature links a libghostty the platform already provides and
needs no Zig, but only when one is actually discoverable:

```sh
pkg-config --exists ghostty-vt   # must succeed first
cargo test --release -p vitrum-vt --no-default-features --features system
```

If that `pkg-config` check fails, the feature does not save you. The sys
crate falls back to cloning Ghostty and building it, so the run ends at the
same Zig error it would have without the feature. `vitrum-vt/build.rs` exists
to turn that into an error naming the missing piece, but it cannot always win
the race: Cargo may run the sys crate's build script first, and then the
unhelpful panic is the one you see.

Everything outside `vitrum-vt` builds without Zig, which is why
`cargo test -p vitrum` works on a machine that has never had it.

CI runs the same two commands with `RUSTFLAGS: -D warnings`, so a warning is
a failed build. A helper that is only reachable on one platform is dead code
on the other two and will fail there while passing for you. The
`macos compiles` and `windows compiles` jobs in `.github/workflows/ci.yml`
cross-check both targets on every push.

Clippy is advisory. CI runs `cargo clippy --release --workspace` with
`continue-on-error`, and it reports rather than blocks. Fix the findings that
are real bugs, and leave the style-only ones alone.

## House rules

These hold everywhere in this tree, and a change that breaks one gets sent
back.

- **Tests live beside the module they test.** A `#[cfg(test)] mod tests` at
  the bottom of the file, or a `tests` module next to the code inside the
  same crate. Integration tests that need a running daemon live under that
  crate's own test module, as in `crates/vitrum-server/src/tests/`.
- **A test must be proven to fail.** Break the code deliberately, watch the
  test go red, then put the code back. A test that stays green through the
  bug it claims to cover is worse than no test, and it gets deleted rather
  than kept.
- **Every test carries a doc comment saying why it exists.** Which defect it
  would catch, not what it calls.
- **Comments explain why, not what.** The code says what it does. Write down
  the reason it is shaped that way, the thing that went wrong before, or the
  constraint that is not visible from here.
- **No stubs.** No `todo!()`, no `unimplemented!()`, no no-op that returns a
  plausible value, no placeholder that a later change is supposed to fill in.
  Ship the whole path or do not ship it.
- **No em dashes in prose.** Docs, comments, and commit messages use commas,
  colons, or a second sentence.
- **No hype.** A number in the docs was measured on a named host, and
  `harness/` holds the way to measure it again.

## Vendored forks

`vendor/` is a fork of Dioxus desktop and `vendor-pty/` is a fork of
`portable-pty`. Both are upstream's code, carried here under upstream's MIT
license and upstream's copyright, and both keep the license file they arrived
with.

The rule for both is the same: they track upstream and carry only the
deliberate deviations that are written down. `vendor/` declares its
divergence in `vendor/UPSTREAM.toml` and explains it in `vendor/README.md`,
and `tools/upstream/check.sh` fails if the real divergence is not exactly the
declared list. `vendor-pty/README.md` names its single divergence, in
`src/win/psuedocon.rs`, and why it exists. So:

- Do not fix an unrelated bug inside a vendored tree. Send it upstream.
- Do not reformat, restructure, or tidy vendored files. Every diff against
  upstream has to be a deliberate one.
- If you do need a new divergence, write it down where that fork records
  them, with the reason and the condition under which it can be dropped.
- Never change the license or the copyright header in either tree.

## Proposing a change

1. Open an issue first for anything that changes behaviour, the protocol, or
   the shape of the UI. A protocol change means both ends and
   `PROTOCOL_VERSION`, and it is worth agreeing on before it is written.
2. Work on a branch off `main`.
3. Keep the change to one thing. A rename and a fix in one diff cost more to
   review than both separately.
4. Update `CHANGELOG.md` and the page under `docs/` that owns the behaviour in
   the same change. Edit `README.md` only when what the product is, or how it
   is installed, changed.
5. Commit messages say why the change is being made.

Before you open a pull request:

- [ ] `cargo build --release --workspace` is clean, with no warnings.
- [ ] `cargo test --release --workspace` passes.
- [ ] A bug fix comes with a test that fails without the fix.
- [ ] No stubs, no `todo!()`, no placeholder returns.
- [ ] Public items are documented.
- [ ] `CHANGELOG.md` mentions the change if a user would notice it.

## Security

Do not open a public issue for a vulnerability. `SECURITY.md` says how to
report one privately.

## License

vitrum is dual licensed under MIT or Apache-2.0, at your option. See
`LICENSE`, `LICENSE-MIT`, and `LICENSE-APACHE`.

Unless you say otherwise, any contribution you deliberately submit for
inclusion in vitrum is dual licensed on those same terms, with no further
conditions. The vendored trees under `vendor/` and `vendor-pty/` stay under
their own upstream MIT license.
