# Cutting a release

A release is a **git tag**, a **GitHub release** pointing at it, and one
`tar.gz` per platform with a `SHA256SUMS` beside them. `vitrum update` refuses
a release that publishes no sums, so the archives are not optional. Building
from the source tarball GitHub generates for the tag stays the documented way
to install the first time, and the README says so.

Why a tag rather than `main`: `main` carries whatever is in flight. A tag is a
state that was tested as a whole, and `vitrum --version` reports the crate
version it was cut at, so an operator can always say which build they are on.

## Before tagging

Run all of it. Each line has to be clean, not nearly clean.

```sh
cargo build --release --workspace     # zero warnings
cargo test  --release --workspace     # zero failures
cargo clippy --release --workspace    # advisory; fix real bugs, ignore style
```

Then check by hand, because no test can:

1. **`CHANGELOG.md` describes this version**, including its gaps. A release
   whose known gaps are discovered by the first user is a release that should
   have said them.
2. **Launch it and use it.** Start two sessions, switch between them, save a
   preset, bind a key, fire it. The suite does not open a window.
3. **Follow the README's own install block** on a clean machine or a scratch
   `$HOME`. The instructions are the product for anybody who has not run it.

## Bumping the version

The version lives in **one** place, `Cargo.toml` at the workspace root. Every
other mention derives from it or is pinned to it by a test:

- `vitrum --version` reads `CARGO_PKG_VERSION`.
- `the_readme_downloads_the_version_this_crate_is` fails if the README's
  tarball URL, git tag or unpack directory names a different version.
- The macOS bundle's `CFBundleShortVersionString` is in the README's install
  block and is checked by eye.

So: change `Cargo.toml`, run `cargo test`, and fix what it names.

```sh
# 1. bump
$EDITOR Cargo.toml            # version = "0.1.0"
$EDITOR CHANGELOG.md          # rename the unreleased heading, date it
cargo test --release --workspace   # this tells you every place that drifted

# 2. commit and tag, staging only the files you changed
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit --only -m "Release v0.1.0" -- Cargo.toml Cargo.lock CHANGELOG.md
git tag -a v0.1.0 -m "v0.1.0"
git push origin main --tags
```

## Publishing

`vitrum update` installs a prebuilt archive, so a release without one leaves
every existing install unable to update itself. Build the asset on each
platform you publish for, on that platform, because the binaries link against
the system webview and are not cross-compiled.

```sh
sh packaging/build-release-asset.sh
```

That writes `dist/vitrum-<version>-<target>.tar.gz` and appends its digest to
`dist/SHA256SUMS`. Collect both from every platform into one `dist` before the
next step, so a single `SHA256SUMS` lists every archive.

```sh
gh release create v0.1.0 \
  --title "v0.1.0" \
  --notes-file <(sed -n '/^## v0.1.0/,/^## v/p' CHANGELOG.md | sed '$d') \
  dist/vitrum-*.tar.gz dist/SHA256SUMS
```

The source tarball GitHub generates for the tag is separate and automatic; it
is what the README's `curl` line fetches for people building from source.

The updater refuses a release that has no `SHA256SUMS`, so uploading the
archives without it publishes an update nobody can install. That refusal is
deliberate: an unverified archive becomes the program the operator runs.

## After publishing

Fetch the tarball the README tells people to fetch and build it, in a scratch
directory. If that fails, the release is broken for everybody who has never
built it before, and you will not find out any other way.

```sh
cd "$(mktemp -d)"
curl -L https://github.com/santhreal/vitrum/archive/refs/tags/v0.1.0.tar.gz | tar xz
cd vitrum-0.1.0
cargo build --release --locked
./target/release/vitrum --version
```
