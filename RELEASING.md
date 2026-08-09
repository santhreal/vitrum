# Cutting a release

A release is a git tag, a GitHub release pointing at it, and one `tar.gz` per
platform with a `SHA256SUMS` beside them. `install.sh`, `install.ps1` and
`vitrum update` all refuse an archive the sums do not cover, so a release
without them installs for nobody.

Two commands do the whole thing.

```sh
make release-dry-run VERSION=0.1.1   # rehearse it; changes nothing
make release VERSION=0.1.1           # do it here, stopping before the push
```

`make release` prints the push that publishes it. Run that, and
`.github/workflows/release.yml` does the rest.

## Before you cut

```sh
make gate            # release build and test of every crate, warnings fatal
make release-check   # every version literal and target triple agrees
```

Write the release into `CHANGELOG.md` under `## Unreleased`, including the
gaps. `make release` refuses an empty Unreleased section, because a release
whose known gaps are discovered by the first user is a release that should
have said them.

Then launch it and use it. Start two sessions, switch between them, save a
preset, bind a key, fire it. The suite does not open a window.

## What `make release` does

In this order, refusing before it edits anything:

1. Refuses a dirty tree, a branch other than `main`, a version that is not
   greater than the current one, a tag that already exists, and an empty
   `## Unreleased`.
2. Bumps every version literal: `Cargo.toml`, `Cargo.lock` and `README.md`.
   `tools/release/versions.sh` owns that list and checks it.
3. Renames `## Unreleased` to `## v<version> - <date>` and opens a fresh empty
   one.
4. Commits exactly those four files as `Release v<version>`.
5. Makes an annotated tag `v<version>` carrying the release notes.
6. Prints the push command, and the two commands that undo it.

Nothing is pushed. That is the only step you take by hand, because it is the
only one that cannot be taken back.

## What `make release-dry-run` does

Everything above, in a clone of this repository in a temporary directory, and
then it deletes the clone. It asserts the commit, the tag, the bumped
literals and the rolled changelog section, breaks each version literal in turn
to confirm the check catches it, and drives every refusal `make release` owes
you. It digests this working tree before and after and fails if they differ,
so a dry run that reports success has proved it changed nothing.

CI runs it on every push.

## Publishing

Pushing the tag starts `.github/workflows/release.yml`. It:

- refuses a tag whose version is not the workspace version, before building;
- builds `vitrum-<version>-<target>.tar.gz` on the runner for each of the four
  published targets, and refuses a runner whose host triple is not the target
  it is building;
- refuses a set of archives that is not exactly those four;
- writes one `SHA256SUMS` over all four;
- uploads everything to a draft, confirms the draft holds every asset, and
  only then publishes it.

Re-running it on the same tag is safe: it puts the release back into draft and
removes its assets before uploading, so a re-run replaces the release instead
of adding a second copy of every file. A run that dies partway leaves an
unpublished draft, which installs for nobody, rather than a release that
installs half of itself.

The source tarball GitHub generates for the tag is separate and automatic; it
is what the README's `curl` line fetches for people building from source.

## Nightly

The same workflow publishes a nightly on a schedule and on every push to main
that changes a crate, the app or the packaging. You do nothing to cut one.

It is one moving tag, `nightly`, repointed at the commit it was built from,
marked prerelease so `/releases/latest` walks past it. Its assets are replaced
in place on every run and its notes name the commit.

A nightly's version is a semver prerelease of the next patch, so `0.1.0` in
`Cargo.toml` publishes `0.1.1-nightly.<date>.<sha>`. It sorts above the last
release and below the next one, the archive name is the workspace version as
always, and `vitrum --version` says which nightly a binary is:

```sh
tools/release/versions.sh nightly
```

To install one, name the tag. To leave the channel, install a stable version;
`docs/install.md` covers pinning.

## Verifying without publishing

```sh
make verify-artifacts
```

Builds the archive for this host with `packaging/build-release-asset.sh`,
writes `SHA256SUMS` the way the publish job writes it, serves both over
`file://`, and installs them with `install.sh` — the shipped script, with only
its download base repointed. Then it corrupts one byte and removes the
`SHA256SUMS` entry and requires the installer to refuse both, so the digest
check is exercised rather than assumed.

## After publishing

Fetch the tarball the README tells people to fetch and build it, in a scratch
directory. If that fails, the release is broken for everybody who has never
built it before, and you will not find out any other way.

```sh
cd "$(mktemp -d)"
curl -L https://github.com/santhreal/vitrum/archive/refs/tags/v0.1.1.tar.gz | tar xz
cd vitrum-0.1.1
cargo build --release --locked
./target/release/vitrum --version
```
