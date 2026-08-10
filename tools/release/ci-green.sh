#!/bin/sh
# Refuse a commit the pipeline has not passed.
#
#   tools/release/ci-green.sh <sha>
#
# `.github/workflows/cut.yml` runs this before it cuts anything. A release cut
# by hand was cut by somebody who had just run `make gate` and looked at the
# checks; a release cut from a dispatch has nobody in front of it, so the thing
# that person did is done here instead.
#
# It asks the GitHub API for the runs whose head is this commit and requires
# `ci` and `platforms` to have completed successfully. A run that is still
# going is a refusal rather than a wait: the answer is not known yet, and a cut
# that starts anyway publishes before the verdict arrives.
#
# Needs `gh` and a token with `actions: read`.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

die() { printf 'ci-green: %s\n' "$*" >&2; exit 1; }

[ $# -eq 1 ] || die 'usage: tools/release/ci-green.sh <sha>'
sha=$(git rev-parse "$1")

repo=${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}

# The newest run of one workflow against one commit, as `status/conclusion`, or
# `none` when that workflow has never run against it. The API returns runs
# newest first, so the first match is the run that decided the commit.
verdict() {
    gh api "repos/$repo/actions/runs?head_sha=$1&per_page=100" --jq \
        "[.workflow_runs[] | select(.path == \".github/workflows/$2\")]
         | map(.status + \"/\" + (.conclusion // \"running\"))
         | .[0] // \"none\""
}

# A release commit is pushed with the workflow token, and GitHub starts no
# workflow for such a push. So the commit at the head of main after a cut is
# one no run will ever exist for, and a second cut behind it would be refused
# for a state this tooling created. Walking back over release commits, and
# over nothing else, is what makes that state legal: those commits carry the
# version literals and the changelog and nothing a build reads differently.
#
# The walk is bounded. An unbounded one against a history of nothing but
# release commits would page the API until the job's timeout, which reads as a
# hang rather than as a refusal.
commit=$sha
hops=0
while [ "$(verdict "$commit" ci.yml)" = none ]; do
    subject=$(git log -1 --format=%s "$commit")
    case "$subject" in
        "Release v"*) ;;
        *)
            die "no ci run exists for $commit ($subject); push it to main and
     let the pipeline answer before cutting a release from it"
            ;;
    esac
    hops=$((hops + 1))
    [ "$hops" -le 5 ] || die "walked back $hops release commits from $sha and
     found none that ci has run on; something other than this tooling is
     writing release commits"
    commit=$(git rev-parse "$commit^")
done

if [ "$commit" != "$sha" ]; then
    printf 'the %s commits since %s are release commits, which start no run\n' \
        "$hops" "$(git rev-parse --short "$commit")"
fi

for workflow in ci.yml platforms.yml; do
    state=$(verdict "$commit" "$workflow")
    case "$state" in
        completed/success)
            printf 'ok       %-14s %s\n' "${workflow%.yml}" "$(git rev-parse --short "$commit")"
            ;;
        none)
            die "$workflow has never run against $commit. Start it with
     'gh workflow run $workflow --ref main' and cut once it is green."
            ;;
        completed/*)
            die "$workflow is ${state#completed/} on $commit, so this commit is
     not one to publish"
            ;;
        *)
            die "$workflow is still $state on $commit; the verdict is not in yet"
            ;;
    esac
done
