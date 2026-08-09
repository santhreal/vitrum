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

# A tag that exists anywhere is a version that may already be installed
# somewhere, so both the local tags and the remote's are consulted before
# anything is decided. A remote that cannot be reached is a refusal rather
# than an assumption: the whole point of the question is that getting it wrong
# re-cuts a published version.
if git rev-parse -q --verify "refs/tags/v$new" >/dev/null; then
    die "tag v$new already exists here"
fi
if git remote get-url "$REMOTE" >/dev/null 2>&1; then
    status=0
    git ls-remote --tags --exit-code "$REMOTE" "refs/tags/v$new" >/dev/null ||
        status=$?
    case $status in
        # `--exit-code` reports 2 for "no such ref". Anything else is a failed
        # query, and a failed query is not an answer.
        0) die "tag v$new is already published on $REMOTE" ;;
        2) published="not on $REMOTE" ;;
        *) die "could not ask $REMOTE whether v$new exists (git exited $status)" ;;
    esac
else
    published="no $REMOTE remote, so nothing is published from here"
fi

# A cut is four writes — bump, changelog, commit, tag — and it can stop between
# any two of them. Which of the three modes below applies is decided by what is
# already true, not by a flag, so an interrupted cut is resumed by running the
# same command again rather than by knowing which half ran.
#
#   forward  the normal case: the version increases, everything runs.
#   first    the version is the one the workspace already carries and its tag
#            exists nowhere. Nothing to bump; the changelog and the tag remain.
#   resume   the release commit is already HEAD and only the tag is missing.
#
# `first` is also where a cut that died between the changelog write and the
# commit lands once its edit is committed, and where a section written before
# the tag existed lands. That is why the changelog merge below is a recovery
# path rather than a one-off: the state it handles is "the changelog names this
# version and the tag does not exist", which every interruption in that window
# produces.
if [ "$new" = "$old" ] &&
   [ "$(git log -1 --format=%s)" = "Release v$new" ] &&
   git show --name-only --format= HEAD | grep -q '^CHANGELOG\.md$' &&
   grep -q "^## v$new - " CHANGELOG.md; then
    mode=resume
    printf 'the release commit for v%s is already HEAD and the tag is missing\n' "$new"
    printf 'resuming: the tag is all that is left to make\n'
elif [ "$new" = "$old" ]; then
    mode=first
    printf 'v%s is the current version and has never been tagged (%s)\n' \
        "$new" "$published"
    printf 'cutting it as the first release of this version, with no bump\n'
else
    mode=forward
    # `sort -V` orders versions rather than strings, so 0.10.0 sorts above
    # 0.9.0. Both operands are plain releases here, which is the case where it
    # agrees with semver.
    greater=$(printf '%s\n%s\n' "$old" "$new" | sort -V | tail -1)
    [ "$greater" = "$new" ] || die "$new is not greater than the current $old"
fi

# A resume has already consumed its Unreleased section; requiring another one
# would refuse the very state it exists to recover from.
[ "$mode" = resume ] || tools/release/changelog.sh unreleased >/dev/null
tools/release/versions.sh check >/dev/null

printf 'v%s from v%s on %s, tree clean\n' "$new" "$old" "$branch"

if [ "$mode" = forward ]; then
    step "bump"
    tools/release/versions.sh bump "$new"

    step "changelog"
    tools/release/changelog.sh roll "$new"
elif [ "$mode" = first ]; then
    step "bump"
    printf 'nothing to bump; the workspace is already %s\n' "$new"

    step "changelog"
    # Merging is only ever right when the version has never been published,
    # which is exactly the condition that made this a first release. A forward
    # cut refuses a duplicate section instead, because there it means the
    # changelog and the version have drifted apart.
    tools/release/changelog.sh roll --merge "$new"
else
    step "bump"
    printf 'already done by the interrupted cut\n'

    step "changelog"
    printf 'already rolled by the interrupted cut: %s\n' \
        "$(grep -m1 "^## v$new - " CHANGELOG.md)"
fi

step "commit"
if [ "$mode" = resume ]; then
    printf 'already made by the interrupted cut:\n'
    git --no-pager show --stat --oneline HEAD | sed 's/^/  /'
else
    # Without a bump only the changelog moved, and committing the three
    # unchanged version files would make an empty-looking release commit that
    # hides which files a cut actually touches.
    if [ "$mode" = forward ]; then
        files=$(tools/release/versions.sh sites; echo CHANGELOG.md)
    else
        files=CHANGELOG.md
    fi
    # shellcheck disable=SC2086
    git add -- $files
    # shellcheck disable=SC2086
    git commit --only --quiet -m "Release v$new" -- $files
    git --no-pager show --stat --oneline HEAD | sed 's/^/  /'
fi

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

  git tag -d v$new$undo
EOF
