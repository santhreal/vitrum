# Cutting a release

A release is a git tag, a GitHub release pointing at it, and one `tar.gz` per
platform with a `SHA256SUMS` beside them. `install.sh`, `install.ps1` and
`vitrum update` all refuse an archive the sums do not cover, so a release
without them installs for nobody.

Two commands do the whole thing.

```sh
make release-dry-run VERSION=x.y.z   # rehearse it; changes nothing
make release VERSION=x.y.z           # do it here, stopping before the push
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

1. Refuses a dirty tree, a branch other than `main`, an empty `## Unreleased`,
   anything that is not a plain `x.y.z`, and a tag that already exists here or
   on `origin`. The remote is asked with `git ls-remote`; a remote that cannot
   be reached is a refusal, not an assumption.
2. Bumps every version literal: `Cargo.toml`, `Cargo.lock` and `README.md`.
   `tools/release/versions.sh` owns that list, derives it from the manifests,
   and checks it.
3. Renames `## Unreleased` to `## v<version> - <date>` and opens a fresh empty
   one.
4. Commits exactly the files it changed as `Release v<version>`.
5. Makes an annotated tag `v<version>` carrying the release notes.
6. Prints the push command, and the two commands that undo it.

Nothing is pushed. That is the only step you take by hand, because it is the
only one that cannot be taken back.

### The first release of a version

Every version must increase, with one exception: the version the workspace
already carries, while its tag exists neither here nor on `origin`. Requiring
an increase there would mean the tool choosing the number. A tree that says
`0.1.0` in `Cargo.toml` and in `README.md` would have to ship `0.1.1` because
nothing had been tagged yet.

So `make release VERSION=<current>` is legal exactly once. It skips the bump,
because there is nothing to bump, and commits the changelog alone. Once the
tag exists it is refused, and refused on the tag rather than on the version,
so it cannot become a way to re-cut something already published.

If `CHANGELOG.md` already carries a section for that version, written before
the tag existed, the `## Unreleased` body is merged into it, newest first and
redated to the day of the cut, rather than opening a second heading for the
same version. Two headings for one version would be read as one, and every
reader takes the first.

## What `make release-dry-run` does

Everything above, in a clone of this repository in a temporary directory
pointed at a scratch remote, and then it deletes the clone. It asserts the
commit, the tag, the bumped literals and the rolled changelog section, breaks
each version literal in turn to confirm the check catches it, and drives every
refusal `make release` owes you including both halves of the tag guard.

It captures what a cut can touch before and after: the four release files, the
git ref and index state, and the temporary names this tooling is the only
writer of. It fails if any of it moved. It deliberately does not guard the
rest of the tree: this tree is shared, and a check that aborts because another
change saved a file is a check that gets switched off. Anything else that moved
is reported and is not a failure.

CI runs it on every push.

## Publishing

Pushing the tag starts `.github/workflows/release.yml`. It:

- refuses a tag whose version is not the workspace version, before building;
- builds `vitrum-<version>-<target>.tar.gz` on the runner for each of the four
  published targets, and refuses a runner whose host triple is not the target
  it is building;
- refuses a set of archives that is not exactly those four;
- disassembles every binary in every archive and refuses anything above the
  CPU floor, on the runner that built it and again over all four together;
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

## The CPU floor

A published binary must run on every machine its triple claims. That is not
automatic. A compiler asked to build for the machine it is running on emits
instructions that machine has, and every published target here is built on a
runner of its own architecture, so a build that detects rather than obeys makes
the instruction set of a release a property of the runner.

`libghostty-vt-sys` did exactly that: it passed `-Dtarget` to zig only when
cross-compiling. Built on an AVX-512 host, the terminal library carried 5581
AVX-512 instructions; pinned, it carries none.

The damage is not that AVX-512 shipped. Ghostty vendors highway, which
compiles one kernel per instruction set and picks between them at run time
from CPUID. Host detection put AVX-512 instructions *inside the AVX2 kernel*:
the symbol is `ghostty::N_AVX2::CodepointWidth`, and it uses `%k` mask
registers. A machine with AVX2 and no AVX-512, which is every Intel desktop
part since Rocket Lake, passes the CPUID check for that kernel, calls it, and
dies with `SIGILL` on the first character it draws.

The build pins zig to `-Dcpu=baseline` on all four targets. Raising that needs
a measurement showing what it buys and a note of which CPUs it drops.

`tools/release/check-isa.sh` disassembles binaries and fails on anything above
AVX2 on x86-64 or armv8.2-a on aarch64. That floor is above the pin on purpose.
Dispatching libraries, highway and simdutf in the terminal engine and memchr
and its relatives in the Rust tree, carry AVX2 kernels in every build, pinned
or not, and never run them on a machine that cannot. The pinned and unpinned
libraries carry the same 3662 AVX2 instructions and differ only in AVX-512, so
AVX2 is what dispatch looks like and anything above it is what host detection
looks like. A gate at the true baseline would fire on every build ever made
here, and a gate that always fires gets deleted.

It runs on each builder, again over all four archives before the draft is
promoted, and inside `make verify-artifacts`. It gates the artifact rather than
the flag, because a flag can be dropped in a refactor and nothing says so,
while the disassembly is what a reader receives. Run it by hand with
`make check-isa` after `make package`.

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
`file://`, and installs them with the shipped `install.sh`, with only its
download base repointed. Then it corrupts one byte and removes the
`SHA256SUMS` entry and requires the installer to refuse both, so the digest
check is exercised rather than assumed.

## After publishing

Fetch the tarball the README tells people to fetch and build it, in a scratch
directory. If that fails, the release is broken for everybody who has never
built it before, and you will not find out any other way.

```sh
VERSION=x.y.z
cd "$(mktemp -d)"
curl -L "https://github.com/santhreal/vitrum/archive/refs/tags/v$VERSION.tar.gz" | tar xz
cd "vitrum-$VERSION"
cargo build --release --locked
./target/release/vitrum --version
```
