#!/bin/sh
# Wait for a dispatched workflow run, and fail when it did not happen.
#
#   tools/release/await-run.sh <workflow.yml> <head-sha> <since-epoch> [timeout]
#
# `gh workflow run` exits 0 the moment GitHub accepts the request. It exits 0
# for a request that creates no run at all, which is how a cut published a tag
# with no archives: the step said `gh workflow list release.yml`, a command
# that succeeds, prints the workflow, and starts nothing. Nothing downstream
# asked whether a run existed, so the cut reported green.
#
# So the dispatch is not the event this waits for. A run of that workflow, at
# that commit, created after the dispatch was made, is. No run inside the
# appearance window is a failure naming the workflow, which is the exact shape
# that bug had.
#
# Then it waits for that run to finish and requires `success`. A cut that ends
# before the publish concludes is a cut somebody has to come back to.
#
# Needs `gh` and a token with `actions: read`.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

die() { printf 'await-run: %s\n' "$*" >&2; exit 1; }

[ $# -ge 3 ] || die 'usage: tools/release/await-run.sh <workflow.yml> <head-sha> <since-epoch> [timeout]'
workflow=$1
sha=$2
since=$3
timeout=${4:-3600}

# A dispatched run is queued within seconds. Ten minutes is long enough that a
# busy actions backlog is not read as a missing run, and short enough that a
# dispatch which created nothing is reported while the cut is still on screen.
appear=600

repo=${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}
server=${GITHUB_SERVER_URL:-https://github.com}

# The newest run of this workflow at this commit that was created at or after
# the dispatch. The commit is what separates this run from the one the last
# release made: a release commit is unique, and every workflow a cut dispatches
# is dispatched against it.
find_run() {
    gh api "repos/$repo/actions/workflows/$workflow/runs?head_sha=$sha&per_page=50" \
        --jq "[.workflow_runs[]
               | select((.created_at | fromdateiso8601) >= $since)]
              | sort_by(.created_at) | last | .id // empty"
}

printf 'awaiting %s at %s\n' "$workflow" "$(printf '%s' "$sha" | cut -c1-7)"

run=
waited=0
while [ -z "$run" ]; do
    run=$(find_run)
    [ -z "$run" ] || break
    if [ "$waited" -ge "$appear" ]; then
        die "no run of $workflow was created for $sha in ${appear}s.
     The dispatch was accepted and produced nothing, which is what a
     mistyped 'gh workflow run' does: it succeeds and starts no build.
     Check $server/$repo/actions/workflows/$workflow"
    fi
    sleep 10
    waited=$((waited + 10))
done

url="$server/$repo/actions/runs/$run"
printf 'run %s\n' "$url"

waited=0
while :; do
    state=$(gh api "repos/$repo/actions/runs/$run" \
        --jq '.status + "/" + (.conclusion // "running")')
    case "$state" in
        completed/success)
            printf 'ok       %s\n' "$workflow"
            exit 0
            ;;
        completed/*)
            die "$workflow is ${state#completed/}: $url"
            ;;
    esac
    if [ "$waited" -ge "$timeout" ]; then
        die "$workflow is still $state after ${timeout}s: $url"
    fi
    sleep 20
    waited=$((waited + 20))
done
