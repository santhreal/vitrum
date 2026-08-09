#!/bin/sh
# Cut a release into a throwaway clone, assert the artifacts, and unwind.
#
#   tools/release/dry-run.sh 0.1.1
#
# Every step `tools/release/cut.sh` performs runs here, including the commit
# and the tag, against a clone of this repository in a temporary directory.
# Nothing is created in this working tree, and the proof is not an assertion in
# the script's prose: the footprint a cut could leave is captured before and
# after and the two must match.
#
# That footprint is deliberately narrow. This tree is shared, six other lanes
# write to it while this runs, and a check that aborts because someone else
# saved a file in another crate is a check that gets switched off. So it
# guards exactly what a cut touches — the four release files, the git ref and
# index state, and the temporary files this tooling is the only writer of —
# and merely reports anything else that moved.
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

# The files `cut.sh` edits, the git state it changes, and the temporary names
# only this tooling ever writes. A change to any of these during a dry run is
# this script's fault and nobody else's.
footprint() {
    for f in $(sites) CHANGELOG.md; do
        if [ -e "$f" ]; then sha256sum "$f"; else printf 'absent  %s\n' "$f"; fi
    done
    printf 'HEAD %s\n' "$(git rev-parse HEAD)"
    printf 'branch %s\n' "$(git rev-parse --abbrev-ref HEAD)"
    printf 'tags %s\n' "$(git tag -l | LC_ALL=C sort | tr '\n' ' ')"
    printf 'staged %s\n' "$(git diff --cached --name-only | LC_ALL=C sort | tr '\n' ' ')"
    printf 'strays %s\n' "$(strays | tr '\n' ' ')"
}

# `sites` is asked of the same script the cut asks, so a fourth release file
# joins this guard the moment it joins the cut.
sites() { tools/release/versions.sh sites; }

# Temporary files this tooling writes and always moves into place. One left
# behind in this tree means a script ran outside the clone.
strays() {
    git ls-files -o --exclude-standard |
        grep -E '(\.versions\.tmp|\.roll\.tmp)$|(^|/)NOTES\.md$' || true
}

# Everything else, reported and never fatal: whatever the other lanes are
# doing is theirs.
neighbourhood() {
    git status --porcelain | LC_ALL=C sort
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
before_neighbours=$(mktemp)
footprint > "$before"
neighbourhood > "$before_neighbours"
before_sum=$(sha256sum < "$before" | cut -d' ' -f1)
sed 's/^/    /' "$before"
printf '  footprint: %s\n' "$before_sum"
printf '  %s other paths dirty in this shared tree, not guarded\n' \
    "$(wc -l < "$before_neighbours" | tr -d ' ')"

work=$(mktemp -d)
repo="$work/repo"
trap 'rm -rf "$work" "$before" "$before_neighbours"' EXIT HUP INT TERM

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
# An identity on the clone, not on each command. A cut commits, and `cut.sh`
# runs its own `git commit` in here: a machine with no `user.email` configured
# — every hosted CI runner is one — fails that commit with "empty ident name"
# after the rehearsal has already done its work. The identity is repository
# local, so it describes these throwaway commits and reaches nothing else.
git -C "$repo" config user.name 'release dry run'
git -C "$repo" config user.email 'dry-run@invalid'
git -C "$repo" add -A -- $OVERLAY
git -C "$repo" commit --quiet --allow-empty \
    -m 'dry run: release tooling from the working tree'
[ -z "$(git -C "$repo" status --porcelain)" ] || die 'the clone is not clean'
ok "clone at $(git -C "$repo" rev-parse --short HEAD), tree clean"

# The clone's `origin` is this shared working tree, whose tags change while a
# dry run is going. `cut.sh` asks the remote whether a tag is published, and
# that question needs a deterministic answer, so the clone is pointed at an
# empty scratch remote instead. It is a real remote reached with a real
# `git ls-remote`, so the guard is exercised rather than skipped, and the tag
# can be published to it below to exercise the other side of the guard.
git init --quiet --bare "$work/origin.git"
git -C "$repo" remote set-url origin "$work/origin.git"
git -C "$repo" tag -l | while read -r t; do git -C "$repo" tag -d "$t" >/dev/null; done
ok "origin repointed at a scratch remote with no tags"

# A cut refuses to run against an empty Unreleased section, on purpose: a
# release whose notes nobody wrote must stop. That makes "is there anything
# unreleased right now" a property of the moment rather than of the tooling,
# and the rehearsal is about the tooling — run the day after a release, it
# failed here having proven nothing. So the clone gets a note of its own when
# it has none, which also means the roll below is exercised on known content.
if ! ( cd "$repo" && tools/release/changelog.sh unreleased >/dev/null 2>&1 ); then
    awk '
        !seeded && /^## Unreleased/ {
            print
            print ""
            print "- Rehearsal note, written by the release dry run."
            seeded = 1
            next
        }
        { print }
    ' "$repo/CHANGELOG.md" > "$repo/CHANGELOG.md.seed"
    mv "$repo/CHANGELOG.md.seed" "$repo/CHANGELOG.md"
    git -C "$repo" add -- CHANGELOG.md
    git -C "$repo" commit --quiet -m 'dry run: an unreleased note to cut'
    ok 'seeded an unreleased note: the tree had none to rehearse with'
fi

old=$(cd "$repo" && tools/release/versions.sh current)
unreleased=$(cd "$repo" && tools/release/changelog.sh unreleased)
# A first release may be cutting a version whose section was written before the
# tag existed. That section has to survive the cut, under the newly dated
# heading and below the Unreleased content, and there must be exactly one of
# it afterwards.
pre_section=$(cd "$repo" && tools/release/changelog.sh notes "$new" 2>/dev/null) ||
    pre_section=

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

# Which cut this is. Cutting the version the workspace already carries is the
# first release of that version and is legal exactly while its tag exists
# nowhere; everything else must increase. There is no rewind: the first-release
# path is the one the very next release takes, so faking a predecessor would
# rehearse the wrong thing.
if [ "$new" = "$old" ]; then
    mode=first
    ok "v$new is the current version and untagged: rehearsing a first release"
else
    ahead=$(printf '%s\n%s\n' "$old" "$new" | sort -V | tail -1)
    [ "$ahead" = "$new" ] ||
        die "$new is below the current $old; that is not a release to rehearse"
    mode=forward
    ok "v$new is above the current $old: rehearsing a forward cut"
fi

step "refusals"
refuses 'a version below the current one' tools/release/cut.sh 0.0.1
refuses 'a non-version argument' tools/release/cut.sh not-a-version
refuses 'a prerelease version' tools/release/cut.sh 9.9.9-rc.1
refuses 'no version at all' tools/release/cut.sh

# The tag guard, from both sides. A first release is legal only while the tag
# exists nowhere, so both places it could exist are made to hold it in turn.
( cd "$repo" && git tag "v$new" )
refuses 'a version already tagged here' tools/release/cut.sh "$new"
( cd "$repo" && git push --quiet origin "refs/tags/v$new" && git tag -d "v$new" >/dev/null )
refuses 'a version already published on the remote' tools/release/cut.sh "$new"
( cd "$repo" && git push --quiet --delete origin "refs/tags/v$new" )

echo scratch > "$repo/.dirty"
refuses 'a dirty tree' tools/release/cut.sh "$new"
rm -f "$repo/.dirty"
( cd "$repo" && git checkout --quiet -b not-main )
refuses 'a branch that is not main' tools/release/cut.sh "$new"
( cd "$repo" && git checkout --quiet main && git branch --quiet -D not-main )
[ -z "$(git -C "$repo" status --porcelain)" ] || die 'refusals left the clone dirty'
[ -z "$(git -C "$repo" tag -l)" ] || die 'a refusal left a tag behind'
ok 'every refusal left the clone untouched'

step "cut v$new"
# Piping the cut into `sed` would report sed's exit status, not the cut's, and
# a cut that made the commit and the tag and then died in its closing output
# would rehearse as a pass. Capture, check, then print.
if ( cd "$repo" && tools/release/cut.sh "$new" ) > "$work/cut.log" 2>&1; then
    sed 's/^/  /' "$work/cut.log"
else
    status=$?
    sed 's/^/  /' "$work/cut.log"
    die "the cut exited $status after making the commit and the tag"
fi
ok 'the cut exited 0'

# The closing block is the only part of a cut that runs after the commit and
# the tag, so a fault there is the most expensive one: the release exists and
# the command still reports failure. It is also a heredoc, which means an
# unset or misspelled variable in it is invisible until it runs. Assert it
# renders, in full, with the exact commands an operator is about to paste.
push_line="  git push origin main tag v$new"
grep -qxF "$push_line" "$work/cut.log" ||
    die "the cut never printed '$push_line'"
ok "it printed the push:$push_line"

# A first or forward cut made the commit, so undoing it removes the tag and the
# commit. A resume made only the tag. Printing the wrong one of these two is a
# data-loss instruction, not a typo.
expected_undo="  git tag -d v$new && git reset --hard HEAD~1"
last=$(sed -e '/./!d' -e '$!d' "$work/cut.log")
[ "$last" = "$expected_undo" ] ||
    die "the cut's last line is [$last], expected [$expected_undo]"
ok "it printed the undo:$last"

# An unexpanded '$' in that output is a variable that did not resolve. Under
# `set -u` an unset one aborts the cut outright; this catches the rest.
if grep -n '\$' "$work/cut.log"; then
    die 'the cut printed an unexpanded variable'
fi
ok 'no unexpanded variables in the cut output'

step "artifacts"
subject=$(git -C "$repo" log -1 --format=%s)
[ "$subject" = "Release v$new" ] || die "commit subject is '$subject'"
ok "commit subject: $subject"

# A first release moves no version literal, because the workspace already
# carries the version being released, so its commit is the changelog alone.
# Asserting the same four files for both modes would let a silent bump on the
# first-release path through.
touched=$(git -C "$repo" show --name-only --format= HEAD | LC_ALL=C sort | tr '\n' ' ')
if [ "$mode" = first ]; then
    expected='CHANGELOG.md '
else
    expected='CHANGELOG.md Cargo.lock Cargo.toml README.md '
fi
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

# Exactly one. Two headings for one version is the failure the merge exists to
# prevent, and every reader of this file takes the first one it finds.
headings=$(grep -c "^## v$new " "$repo/CHANGELOG.md")
[ "$headings" = 1 ] || die "CHANGELOG.md has $headings sections for v$new"
ok "exactly one section for v$new"

rolled=$(cd "$repo" && tools/release/changelog.sh notes "$new")
if [ -n "$pre_section" ]; then
    expected=$(printf '%s\n\n%s' "$unreleased" "$pre_section")
    [ "$rolled" = "$expected" ] ||
        die 'the merged section is not the Unreleased body above the one that was there'
    ok "released notes are the $(printf '%s' "$unreleased" | wc -l | tr -d ' ')-line Unreleased body above the $(printf '%s' "$pre_section" | wc -l | tr -d ' ')-line section that predated the tag"
else
    [ "$rolled" = "$unreleased" ] ||
        die 'the released section is not the Unreleased body it came from'
    ok "released notes are the $(printf '%s' "$unreleased" | wc -l | tr -d ' ')-line Unreleased body, unchanged"
fi

# Once the tag exists a first release is no longer available, which is the
# guard that keeps `VERSION=<current>` from becoming a way to re-cut a
# published version. It must refuse on the tag, not on the version ordering.
refuses 'a second cut of the same version' tools/release/cut.sh "$new"
grep -q "tag v$new already exists here" "$work/refuse.log" ||
    die 'the second cut was refused for the wrong reason'
ok "and it refused on the tag, not on the version"
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

step "resume an interrupted cut"
# A cut writes the commit and then the tag. Killed between the two it leaves a
# clean tree, a bumped workspace, a rolled changelog and no tag — and an empty
# Unreleased section, which is why re-running it must not demand one. This
# rehearses that window by removing the tag from the finished cut above, which
# reproduces the state exactly.
head_before=$(git -C "$repo" rev-parse HEAD)
log_before=$(git -C "$repo" rev-list --count HEAD)
changelog_before=$(sha256sum < "$repo/CHANGELOG.md" | cut -d' ' -f1)
git -C "$repo" tag -d "v$new" >/dev/null

( cd "$repo" && tools/release/cut.sh "$new" ) > "$work/resume.log" 2>&1 ||
    { sed 's/^/  /' "$work/resume.log"; die 'the resume was refused'; }
sed 's/^/  /' "$work/resume.log"

grep -q 'resuming: the tag is all that is left to make' "$work/resume.log" ||
    die 'the cut did not recognise the interrupted state'
ok 'recognised the release commit and resumed'

# The three assertions that separate a resume from a second cut: no new commit,
# no second changelog section, and the tag back on the commit that was already
# there. A resume that quietly re-ran the bump would still end with a tag.
[ "$(git -C "$repo" rev-list --count HEAD)" = "$log_before" ] ||
    die 'the resume added a commit'
[ "$(git -C "$repo" rev-parse HEAD)" = "$head_before" ] ||
    die 'the resume moved HEAD'
ok 'no new commit, HEAD unmoved'

[ "$(sha256sum < "$repo/CHANGELOG.md" | cut -d' ' -f1)" = "$changelog_before" ] ||
    die 'the resume rewrote CHANGELOG.md'
[ "$(grep -c "^## v$new " "$repo/CHANGELOG.md")" = 1 ] ||
    die 'the resume opened a second section'
ok 'CHANGELOG.md byte-identical, still one section'

[ -z "$(git -C "$repo" status --porcelain)" ] || die 'the resume left the clone dirty'
[ "$(git -C "$repo" cat-file -t "v$new")" = tag ] ||
    die 'the resume did not make an annotated tag'
[ "$(git -C "$repo" rev-parse "v$new^{commit}")" = "$head_before" ] ||
    die 'the resumed tag is not on the release commit'
resumed_notes=$(git -C "$repo" tag -l "v$new" --format='%(contents:body)')
[ -n "$resumed_notes" ] || die 'the resumed tag carries no notes'
ok 'annotated tag rebuilt on the same commit, notes intact, tree clean'

# Undoing a resume must not offer to throw away a commit the resume did not
# make. That instruction, followed, would delete the release. Assert the whole
# line rather than the absence of `reset --hard`, so the tail is proven to
# render on this path too and not merely to omit one phrase.
resume_last=$(sed -e '/./!d' -e '$!d' "$work/resume.log")
[ "$resume_last" = "  git tag -d v$new" ] ||
    die "the resume's last line is [$resume_last], expected [  git tag -d v$new]"
ok "the undo it prints deletes the tag only:$resume_last"

if grep -n '\$' "$work/resume.log"; then
    die 'the resume printed an unexpanded variable'
fi
ok 'no unexpanded variables in the resume output'

step "unwind"
rm -rf "$work"
trap 'rm -f "$before" "$before_neighbours"' EXIT HUP INT TERM
ok 'scratch clone removed'

step "after"
after=$(mktemp)
after_neighbours=$(mktemp)
footprint > "$after"
neighbourhood > "$after_neighbours"
after_sum=$(sha256sum < "$after" | cut -d' ' -f1)
sed 's/^/    /' "$after"
printf '  footprint: %s\n' "$after_sum"

if [ "$before_sum" != "$after_sum" ]; then
    diff --label before --label after -u "$before" "$after" >&2 || true
    rm -f "$after" "$after_neighbours"
    printf '\nOne of the four files a cut edits, or the git state, moved while\n' >&2
    printf 'this ran. Either a release script wrote outside its clone, or\n' >&2
    printf 'another lane touched a release file — Cargo.lock in particular\n' >&2
    printf 'moves whenever someone adds a dependency or a workspace member.\n' >&2
    printf 'The diff above says which. Re-run to tell the two apart: a script\n' >&2
    printf 'bug reproduces, a neighbour does not.\n' >&2
    die 'a file a cut edits changed while the dry run was going'
fi

# Reported, never fatal. Six lanes write to this tree; their edits are not a
# dry run failure, and treating them as one is how a check gets switched off.
if ! cmp -s "$before_neighbours" "$after_neighbours"; then
    printf '\n  other lanes changed the tree while this ran, outside the\n'
    printf '  release footprint and not a failure:\n'
    diff --label before --label after -u \
        "$before_neighbours" "$after_neighbours" | sed -n '4,$p' | sed 's/^/    /'
fi
rm -f "$after" "$after_neighbours"

printf '\ndry run of v%s passed; the release footprint is byte-identical (%s)\n' \
    "$new" "$before_sum"
