#!/bin/sh
# The order every crate is published to crates.io in, and the check that it is
# still every crate.
#
#   tools/release/crates.sh list    print the publish order
#   tools/release/crates.sh check   fail when the workspace and the order differ
#
# Order is not cosmetic. Cargo resolves each crate's dependencies from the
# registry at publish time, so a crate whose dependency is not up there yet is
# rejected outright. The list below is a topological sort of the internal
# graph: the two vendored forks first, because `[patch.crates-io]` is ignored
# by anyone installing from the registry, and the client last.
#
# The membership is not hand-maintained. `check` reads the workspace and fails
# when a crate that can be published is missing from the order, or when the
# order names something the workspace no longer has. A new crate that nobody
# adds here would otherwise be a crate that is simply never published, which
# is invisible: the publish job would go green having published twelve of
# thirteen.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

die() { printf 'crates: %s\n' "$*" >&2; exit 1; }

ORDER='vitrum-portable-pty
vitrum-proto
vitrum-fmt
vitrum-search
vitrum-grid
vitrum-vt
vitrum-model
vitrum-os
vitrum-core
vitrum-server
vitrum-replay
vitrum'

# Every workspace member whose manifest does not set `publish = false`.
# Read from cargo rather than from the manifests, because a member can join
# the workspace through a glob and a manifest can inherit from the workspace.
publishable() {
    cargo metadata --no-deps --format-version 1 |
        python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    if p.get("publish") != []:
        print(p["name"])
'
}

case "${1:-check}" in
    list) printf '%s\n' "$ORDER" ;;
    check)
        command -v cargo >/dev/null 2>&1 || die 'check needs cargo'
        command -v python3 >/dev/null 2>&1 || die 'check needs python3'
        tmp=$(mktemp -d)
        trap 'rm -rf "$tmp"' EXIT HUP INT TERM
        publishable | LC_ALL=C sort > "$tmp/workspace"
        printf '%s\n' "$ORDER" | LC_ALL=C sort > "$tmp/order"
        missing=$(comm -23 "$tmp/workspace" "$tmp/order")
        [ -z "$missing" ] || die "the workspace can publish crates this order does
     not name, so a release would publish every crate except these:
$(printf '%s\n' "$missing" | sed 's/^/       /')
     Add each one to ORDER in this file, after the crates it depends on."
        gone=$(comm -13 "$tmp/workspace" "$tmp/order")
        [ -z "$gone" ] || die "this order names crates the workspace cannot
     publish, so 'cargo publish -p' fails on them and takes the release
     with it:
$(printf '%s\n' "$gone" | sed 's/^/       /')"
        printf 'crates: %s in the publish order, and the workspace has no others\n' \
            "$(printf '%s\n' "$ORDER" | wc -l | tr -d ' ')"
        ;;
    *) die "unknown command: ${1:-}" ;;
esac
