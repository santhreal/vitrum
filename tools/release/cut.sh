#!/bin/sh
# Cut a release locally: one command, no prompts, stops before pushing.
#
#   tools/release/cut.sh 0.1.1
#
# It bumps every version literal, rolls CHANGELOG.md's Unreleased section into
# a dated release section, commits exactly the files it touched, annotates the
# tag with the release notes, and prints the push that publishes it. Pushing is
# left to you because that is the step that cannot be undone.
#
# It refuses to start on a dirty tree, off `main`, on a version that is not
# greater than the current one, on a tag that already exists, or when
# CHANGELOG.md has nothing under Unreleased. Every refusal happens before the
# first edit, so a refused cut leaves the tree exactly as it found it.
#
# RELEASE_BRANCH exists for `tools/release/dry-run.sh`, which runs this in a
# throwaway clone. Nothing else should set it.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

BRANCH=${RELEASE_BRANCH:-main}
REMOTE=${RELEASE_REMOTE:-origin}

die() { printf 'release: %s\n' "$*" >&2; exit 1; }
step() { printf '\n== %s\n' "$*"; }

[ $# -eq 1 ] || die 'usage: tools/release/cut.sh <version>'
new=${1#v}

# Plain `x.y.z` only. The ordering check below is `sort -V`, which places
# `0.1.1` before `0.1.1-rc.1` where semver places it after, so a prerelease cut
# by hand would be compared by the wrong rule. Prerelease versions are the
# nightly channel's, and CI derives those with
# `tools/release/versions.sh nightly` rather than through here.
case "$new" in
    *[!0-9.]*) die "not a plain release version: $new (want x.y.z)" ;;
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) die "not a version: $new (want x.y.z)" ;;
esac

step "checks"

branch=$(git rev-parse --abbrev-ref HEAD)
[ "$branch" = "$BRANCH" ] ||
    die "on branch $branch, releases are cut from $BRANCH"

dirty=$(git status --porcelain)
[ -z "$dirty" ] || {
    printf '%s\n' "$dirty" >&2
    die 'the tree is dirty; commit or stash before cutting a release'
}

old=$(tools/release/versions.sh current)
[ "$new" != "$old" ] || die "$new is already the current version"
# `sort -V` orders versions rather than strings, so 0.10.0 sorts above 0.9.0.
greater=$(printf '%s\n%s\n' "$old" "$new" | sort -V | tail -1)
[ "$greater" = "$new" ] || die "$new is not greater than the current $old"

git rev-parse -q --verify "refs/tags/v$new" >/dev/null &&
    die "tag v$new already exists"

tools/release/changelog.sh unreleased >/dev/null
tools/release/versions.sh check >/dev/null

printf 'v%s from v%s on %s, tree clean\n' "$new" "$old" "$branch"

step "bump"
tools/release/versions.sh bump "$new"

step "changelog"
tools/release/changelog.sh roll "$new"

step "commit"
files=$(tools/release/versions.sh sites; echo CHANGELOG.md)
# shellcheck disable=SC2086
git add -- $files
# shellcheck disable=SC2086
git commit --only --quiet -m "Release v$new" -- $files
git --no-pager show --stat --oneline HEAD | sed 's/^/  /'

step "tag"
notes=$(tools/release/changelog.sh notes "$new")
printf 'v%s\n\n%s\n' "$new" "$notes" | git tag -a "v$new" -F -
git --no-pager tag -l "v$new" --format='  %(refname:short)  %(contents:subject)'

step "next"
cat <<EOF
Nothing has been pushed. Publish with:

  git push $REMOTE $BRANCH tag v$new

That tag starts .github/workflows/release.yml, which builds the four
published targets, writes SHA256SUMS over all four, and publishes the
release only once every asset is uploaded.

To undo instead:

  git tag -d v$new && git reset --hard HEAD~1
EOF
