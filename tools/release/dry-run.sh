#!/bin/sh
# Cut a release into a throwaway clone, assert the artifacts, and unwind.
#
#   tools/release/dry-run.sh 0.1.1
#
# Every step `tools/release/cut.sh` performs runs here, including the commit
# and the tag, against a clone of this repository in a temporary directory.
# Nothing is created in this working tree, and the proof is not an assertion in
# the script's prose: the tree is digested before and after and the two digests
# must match, `git status --porcelain` included.
#
# It also drives each refusal `cut.sh` owes you — dirty tree, wrong branch, a
# version that does not increase, an existing tag, an empty Unreleased — and
# fails if any of them is accepted. A guard nobody exercises is a guard that
# has already stopped working.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

die() { printf 'dry-run: %s\n' "$*" >&2; exit 1; }
step() { printf '\n== %s\n' "$*"; }
ok() { printf '  ok  %s\n' "$*"; }

[ $# -eq 1 ] || die 'usage: tools/release/dry-run.sh <version>'
new=${1#v}

# Tracked and untracked-but-not-ignored content, plus the porcelain status.
# Ignored paths are excluded on purpose: `target/` and `dist/` change whenever
# anything is built and are not part of the tree a dry run must preserve.
digest() {
    git status --porcelain
    git ls-files -z | sort -z | xargs -0 sha256sum
    git ls-files -o --exclude-standard -z | sort -z | xargs -0 sha256sum
}

# The clone carries HEAD, which is where a release is cut from. The release
# tooling itself is overlaid from the working tree, so a dry run exercises the
# scripts as they are on disk rather than the last committed copy of them.
OVERLAY='Makefile RELEASING.md tools/release .github/workflows/release.yml'

# `cut.sh` must refuse; run it and require a non-zero exit.
refuses() {
    what=$1
    shift
    if ( cd "$repo" && "$@" ) >"$work/refuse.log" 2>&1; then
        sed 's/^/    /' "$work/refuse.log" >&2
        die "cut.sh accepted $what"
    fi
    ok "refuses $what: $(tail -1 "$work/refuse.log")"
}

step "before"
before=$(mktemp)
digest > "$before"
before_sum=$(sha256sum < "$before" | cut -d' ' -f1)
printf '  status --porcelain:\n'
git status --porcelain | sed 's/^/    /'
printf '  tree digest: %s\n' "$before_sum"

work=$(mktemp -d)
repo="$work/repo"
trap 'rm -rf "$work" "$before"' EXIT HUP INT TERM

step "clone"
git clone --quiet --no-hardlinks . "$repo"
# `-B` rather than `checkout main`: CI clones a detached HEAD at a tag or a
# pull request head, where no local `main` exists to check out, and the branch
# a cut is made from is the one the clone is sitting on either way.
git -C "$repo" checkout --quiet -B main
for path in $OVERLAY; do
    rm -rf "${repo:?}/$path"
    mkdir -p "$(dirname "$repo/$path")"
    cp -R "$path" "$repo/$path"
done
git -C "$repo" -c user.name=dry-run -c user.email=dry-run@invalid \
    add -A -- $OVERLAY
git -C "$repo" -c user.name=dry-run -c user.email=dry-run@invalid \
    commit --quiet --allow-empty -m 'dry run: release tooling from the working tree'
[ -z "$(git -C "$repo" status --porcelain)" ] || die 'the clone is not clean'
ok "clone at $(git -C "$repo" rev-parse --short HEAD), tree clean"

old=$(cd "$repo" && tools/release/versions.sh current)
unreleased=$(cd "$repo" && tools/release/changelog.sh unreleased)

step "version sites"
( cd "$repo" && tools/release/versions.sh selftest ) | sed 's/^/  /'
[ -z "$(git -C "$repo" status --porcelain)" ] ||
    die 'the version self-test left the clone dirty'
ok 'every site restored'

# The nightly channel publishes under a version derived here and reads it back
# out of the asset filename, so its ordering against the surrounding stables is
# the whole contract and is worth asserting rather than reasoning about.
step "nightly version"
nv=$(cd "$repo" && tools/release/versions.sh nightly)
next=${nv%%-*}
printf '  %s < %s < %s\n' "$old" "$nv" "$next"
case $nv in
    [0-9]*.[0-9]*.[0-9]*-nightly.[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9].*) ;;
    *) die "nightly version $nv is not <x.y.z>-nightly.<date>.<sha>" ;;
esac
# Not `sort -V`: GNU version sort orders `0.1.1` before `0.1.1-nightly.1`,
# which is the opposite of semver, where any prerelease precedes the release
# it qualifies. The two facts semver actually gives are asserted instead, and
# `sort -V` is used only where both operands are plain releases and it agrees.
[ "$nv" != "$next" ] && [ "${nv#"$next"-}" != "$nv" ] ||
    die "$nv is not a prerelease of $next, so it does not sort below it"
[ "$next" != "$old" ] &&
    [ "$(printf '%s\n%s\n' "$next" "$old" | sort -V | head -1)" = "$old" ] ||
    die "$next is not above the current $old, so $nv is not either"
ok "prerelease of $next, which is above $old: sorts strictly between them"

# Each runner writes it into the workspace before building, which is what puts
# it into the archive name; a nightly the check rejects is four builds that
# publish nothing.
( cd "$repo" && tools/release/versions.sh bump "$nv" ) | sed 's/^/  ok  /'
ok "archive would be vitrum-$nv-<target>.tar.gz"
git -C "$repo" checkout --quiet -- .
[ -z "$(git -C "$repo" status --porcelain)" ] ||
    die 'the nightly bump left the clone dirty'
ok 'clone restored'

step "refusals"
refuses 'a version that does not increase' tools/release/cut.sh "$old"
refuses 'a non-version argument' tools/release/cut.sh not-a-version
refuses 'no version at all' tools/release/cut.sh
( cd "$repo" && git tag v999.0.0 )
refuses 'a tag that already exists' tools/release/cut.sh 999.0.0
( cd "$repo" && git tag -d v999.0.0 >/dev/null )
echo scratch > "$repo/.dirty"
refuses 'a dirty tree' tools/release/cut.sh "$new"
rm -f "$repo/.dirty"
( cd "$repo" && git checkout --quiet -b not-main )
refuses 'a branch that is not main' tools/release/cut.sh "$new"
( cd "$repo" && git checkout --quiet main && git branch --quiet -D not-main )
[ -z "$(git -C "$repo" status --porcelain)" ] || die 'refusals left the clone dirty'
ok 'every refusal left the clone untouched'

# Rehearsing the version the tree already carries is the common case the day a
# release is cut, and `cut.sh` is right to refuse it against a real tree. The
# clone is instead rewound to the state that preceded it: the workspace drops
# to 0.0.0 and the section already published under this version is renamed to
# match, so the cut runs forward into it exactly as it did the first time.
ahead=$(printf '%s\n%s\n' "$old" "$new" | sort -V | tail -1)
if [ "$ahead" != "$new" ] || [ "$old" = "$new" ]; then
    step "rewind"
    ( cd "$repo" && tools/release/versions.sh bump 0.0.0 >/dev/null )
    sed "s/^## v$new - /## v0.0.0 - /" "$repo/CHANGELOG.md" > "$work/cl"
    mv "$work/cl" "$repo/CHANGELOG.md"
    git -C "$repo" -c user.name=dry-run -c user.email=dry-run@invalid \
        commit --quiet -a -m "dry run: rewind to before v$new"
    ok "clone rewound to 0.0.0 so v$new is a forward cut"
fi

step "cut v$new"
( cd "$repo" && tools/release/cut.sh "$new" ) | sed 's/^/  /'

step "artifacts"


subject=$(git -C "$repo" log -1 --format=%s)
[ "$subject" = "Release v$new" ] || die "commit subject is '$subject'"
ok "commit subject: $subject"

touched=$(git -C "$repo" show --name-only --format= HEAD | LC_ALL=C sort | tr '\n' ' ')
expected='CHANGELOG.md Cargo.lock Cargo.toml README.md '
[ "$touched" = "$expected" ] ||
    die "commit touched [$touched], expected [$expected]"
ok "commit touched exactly: $touched"

[ "$(git -C "$repo" cat-file -t "v$new")" = tag ] ||
    die "v$new is not an annotated tag"
tagline=$(git -C "$repo" tag -l "v$new" --format='%(contents:subject)')
[ "$tagline" = "v$new" ] || die "tag subject is '$tagline'"
[ "$(git -C "$repo" rev-parse "v$new^{commit}")" = "$(git -C "$repo" rev-parse HEAD)" ] ||
    die "v$new does not point at the release commit"
ok "annotated tag v$new on the release commit, notes in its message"

got=$(cd "$repo" && tools/release/versions.sh current)
[ "$got" = "$new" ] || die "workspace version is $got"
( cd "$repo" && tools/release/versions.sh check ) | sed 's/^/  ok  /'

# The date is asserted by shape rather than against `date` here: the cut reads
# the clock, this reads it again, and a run that straddles UTC midnight must
# not fail for that.
heading=$(grep -E "^## v$new - [0-9]{4}-[0-9]{2}-[0-9]{2}\$" "$repo/CHANGELOG.md") ||
    die "CHANGELOG.md has no dated '## v$new' heading"
ok "CHANGELOG.md heading: $heading"

rolled=$(cd "$repo" && tools/release/changelog.sh notes "$new")
[ "$rolled" = "$unreleased" ] ||
    die 'the released section is not the Unreleased body it came from'
ok "released notes are the $(printf '%s' "$unreleased" | wc -l | tr -d ' ')-line Unreleased body, unchanged"

refuses 'a second cut of the same version' tools/release/cut.sh "$new"
refuses 'an empty Unreleased section' tools/release/changelog.sh unreleased

# The publish step reads the file with the same matcher; if it cannot find the
# section here it will not find it on the runner either.
notes_via_workflow=$(awk -v v="## v$new" '
    index($0, v) == 1 { found = 1; next }
    found && /^## / { exit }
    found { print }
' "$repo/CHANGELOG.md" | sed -e '/./,$!d')
[ -n "$notes_via_workflow" ] || die 'the workflow matcher finds no notes for this tag'
ok 'the workflow release-notes matcher finds the section'

step "unwind"
rm -rf "$work"
trap 'rm -f "$before"' EXIT HUP INT TERM
ok 'scratch clone removed'

step "after"
after=$(mktemp)
digest > "$after"
after_sum=$(sha256sum < "$after" | cut -d' ' -f1)
printf '  status --porcelain:\n'
git status --porcelain | sed 's/^/    /'
printf '  tree digest: %s\n' "$after_sum"

if [ "$before_sum" != "$after_sum" ]; then
    diff "$before" "$after" >&2 || true
    rm -f "$after"
    die 'the working tree changed'
fi
rm -f "$after"

printf '\ndry run of v%s passed; the working tree is byte-identical (%s)\n' \
    "$new" "$before_sum"
