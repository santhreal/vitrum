#!/bin/sh
# Ask a published release the questions an installer asks.
#
#   tools/release/published.sh v1.2.3      a stable release
#   tools/release/published.sh nightly     the moving nightly release
#
# Everything upstream of this proves something about the build. This proves
# something about the release: that it exists, that it is published rather than
# a draft, that it is the one an installer resolves, that it holds exactly one
# archive per published target with a `SHA256SUMS` beside them, and that those
# archives are the bytes that checksum file names.
#
# The digests are recomputed from downloaded assets rather than trusted. A
# checksum file is written by the publish job over the archives it had in hand;
# nothing between that and here had compared it against what a client receives,
# and a checksum nobody verifies is a checksum that can be wrong for a whole
# release cycle.
#
# The empty-release case is the first thing it answers. `v0.1.0` was tagged,
# had a release page, and carried no assets at all, because the matrix leg that
# never started held the publish job forever. Every check below fails on that
# release, and the cut that made it would have failed with it.
#
# Needs `gh` and `curl`.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

die() { printf 'published: %s\n' "$*" >&2; exit 1; }
ok() { printf '  ok  %s\n' "$*"; }

[ $# -eq 1 ] || die 'usage: tools/release/published.sh <tag>'
tag=$1

repo=${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}

case "$tag" in
    nightly) channel=nightly ;;
    v*) channel=stable ;;
    *) die "'$tag' is neither a v* tag nor the nightly tag" ;;
esac

gh release view "$tag" --repo "$repo" >/dev/null 2>&1 ||
    die "$repo has no release for $tag.
     A tag with no release behind it installs for nobody: every installer
     builds its download URL from the tag and gets a 404."

state=$(gh release view "$tag" --repo "$repo" --json isDraft,isPrerelease \
    --jq '(if .isDraft then "draft" else "published" end) + "/" +
          (if .isPrerelease then "prerelease" else "release" end)')
case "$state" in
    draft/*)
        die "$tag is still a draft, which is a release only its maintainers
     can see. Nothing can install it."
        ;;
esac

case "$channel/$state" in
    stable/published/prerelease)
        die "$tag is marked prerelease, so /releases/latest walks past it and
     an installer given no version installs the release before it"
        ;;
    nightly/published/release)
        die "the nightly release is not marked prerelease, so /releases/latest
     resolves to it and every stable install and every 'vitrum update' on
     the stable channel is walked onto a build of main"
        ;;
esac
ok "$tag is $state"

# The endpoint both installers use when no version is given, asked rather than
# inferred from a field. `isLatest` is not a field `gh release view` has, and
# a step that asked for it exited 1 after every leg of the matrix had built.
latest=$(gh api "repos/$repo/releases/latest" --jq .tag_name 2>/dev/null || echo none)
case "$channel" in
    stable)
        [ "$latest" = "$tag" ] ||
            die "$tag is published and /releases/latest resolves to $latest,
     so 'curl | sh' installs $latest and not this release"
        ok "/releases/latest resolves to $tag"
        ;;
    nightly)
        [ "$latest" != "$tag" ] ||
            die "the nightly release became /releases/latest; every stable
     install would take a build of main"
        ok "/releases/latest resolves to $latest, not the nightly"
        ;;
esac

# One archive per published target, a SHA256SUMS, and nothing else. An extra
# asset is an archive under a name no installer asks for, which is the shape a
# half-renamed target takes.
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM

gh release view "$tag" --repo "$repo" --json assets --jq '.assets[].name' |
    LC_ALL=C sort > "$scratch/on-release"

version=""
for target in $(tools/release/targets.sh list); do
    name=$(grep -E "^vitrum-.*-$target\.tar\.gz\$" "$scratch/on-release" | head -1 || true)
    [ -n "$name" ] ||
        die "$tag carries no archive for $target.
     It has: $(tr '\n' ' ' < "$scratch/on-release")
     Every installer on that platform ends in a 404."
    # The version inside the archive names is the release's real version, and
    # on a stable release it has to be the version in the tag. A release whose
    # tag says one version and whose archives say another is a 404 for every
    # client, and it looks correct on the releases page.
    this="${name#vitrum-}"
    this="${this%-$target.tar.gz}"
    if [ -z "$version" ]; then
        version=$this
    elif [ "$this" != "$version" ]; then
        die "$tag holds archives for two versions, $version and $this,
     so which build a machine gets depends on its platform"
    fi
    printf '%s\n' "$name"
done > "$scratch/expected"
printf 'SHA256SUMS\n' >> "$scratch/expected"
LC_ALL=C sort -o "$scratch/expected" "$scratch/expected"

diff "$scratch/expected" "$scratch/on-release" ||
    die "$tag does not hold exactly the assets a release publishes"
ok "$tag holds one archive per published target and a SHA256SUMS"

if [ "$channel" = stable ] && [ "$version" != "${tag#v}" ]; then
    die "$tag holds archives for $version.
     Every installer builds its URL from the tag and asks for
     vitrum-${tag#v}-<target>.tar.gz, which this release does not have."
fi
ok "the archives carry version $version"

# The digests, against the bytes a client receives. `sha256sum -c` inside the
# publish job compares the file it just wrote with the archives it wrote it
# from, which cannot disagree. This compares the published checksum file with
# the published archives, downloaded the way an installer downloads them.
base="https://github.com/$repo/releases/download/$tag"
curl -fsSL --retry 3 -o "$scratch/SHA256SUMS" "$base/SHA256SUMS" ||
    die "$tag publishes a SHA256SUMS that cannot be downloaded from $base"

head -1 "$scratch/SHA256SUMS" | grep -Eq '^[0-9a-fA-F]{64}[ *]' ||
    die "what $base/SHA256SUMS serves is not a checksum file"

while read -r name; do
    [ "$name" != SHA256SUMS ] || continue
    grep -qF "  $name" "$scratch/SHA256SUMS" ||
        die "SHA256SUMS does not cover $name, so every installer for that
     platform refuses the archive this release published for it"
done < "$scratch/on-release"
ok 'SHA256SUMS covers every archive on the release'

( cd "$scratch" && while read -r name; do
    [ "$name" != SHA256SUMS ] || continue
    curl -fsSL --retry 3 -o "$name" "$base/$name" ||
        { printf 'published: %s is listed on the release and 404s at %s\n' "$name" "$base" >&2; exit 1; }
done < on-release )

( cd "$scratch" && sha256sum -c SHA256SUMS ) ||
    die "the archives $tag serves are not the ones its SHA256SUMS names.
     Every installer refuses this release with a checksum mismatch."
ok 'every published archive matches the published SHA256SUMS'

printf '\n%s is installable: version %s, %s archives\n' \
    "$tag" "$version" "$(($(wc -l < "$scratch/on-release") - 1))"
