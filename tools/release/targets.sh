#!/bin/sh
# The published target triples, and the check that everything agrees on them.
#
#   tools/release/targets.sh list    print the published triples
#   tools/release/targets.sh check   fail if any file names a different set
#
# Four files have an opinion about which platforms a release carries, and this
# compares all four: the build matrix in `.github/workflows/release.yml`, the
# `uname` mapping in `install.sh`, the architecture gate in `install.ps1`, and
# the table in `docs/install.md`. A triple that exists in the matrix and not in
# an installer is an asset nobody downloads; a triple in an installer and not
# in the matrix is a 404 at the end of a `curl | sh`. Neither shows up until
# someone runs the installer on that platform, which is after the release is
# published. A table that names a build nobody publishes is the same failure
# with a longer delay.
#
# `update.rs` inside the app is not a fifth site. It builds the asset name from
# `VITRUM_TARGET`, which the build script takes from the compiler, so it names
# whatever platform it was compiled for and cannot disagree with this list.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

TARGETS='aarch64-apple-darwin
aarch64-unknown-linux-gnu
x86_64-apple-darwin
x86_64-pc-windows-msvc
x86_64-unknown-linux-gnu'

die() { printf 'targets: %s\n' "$*" >&2; exit 1; }

sorted() { LC_ALL=C sort -u; }

# Every triple the file names, whatever syntax it names it in.
#
# A triple that follows a `/` is a multiarch library directory, not a target:
# `/usr/lib/x86_64-linux-gnu` is where a Debian derivative keeps the GTK
# this installer probes for, and reading it as a published target failed this
# check on a file that named the right set.
found_in() {
    grep -oE '(^|[^/[:alnum:]_.-])(aarch64|x86_64|i686|armv7)-[a-z0-9_]+-[a-z0-9_]+(-[a-z0-9]+)?' "$1" |
        grep -oE '(aarch64|x86_64|i686|armv7)-[a-z0-9_]+-[a-z0-9_]+(-[a-z0-9]+)?' |
        sorted
}

want=$(printf '%s\n' "$TARGETS" | sorted)
# The two installers split the set between them by platform: `install.sh` runs
# on Linux and macOS, `install.ps1` on Windows. Each is checked against its own
# half, and the halves are cut out of the one list above rather than typed
# twice, so a fifth target joins whichever installer it belongs to or fails.
want_windows=$(printf '%s\n' "$want" | grep windows | sorted)
want_posix=$(printf '%s\n' "$want" | grep -v windows | sorted)

compare() {
    file=$1
    expect=$2
    what=$3
    got=$(found_in "$file")
    [ "$got" = "$expect" ] || {
        printf 'targets: %s names a different set:\n' "$file" >&2
        printf '%s\n' "$expect" > "$tmp"
        printf '%s\n' "$got" | diff --label published --label "$file" -u "$tmp" - >&2
        die "$file disagrees with the published target set"
    }
    printf 'targets: %s names %s\n' "$file" "$what"
}

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT HUP INT TERM

case "${1:-check}" in
    list) printf '%s\n' "$TARGETS" ;;
    check)
        compare .github/workflows/release.yml "$want" 'every published target'
        compare install.sh "$want_posix" 'every Linux and macOS target'
        compare install.ps1 "$want_windows" 'every Windows target'
        compare docs/install.md "$want" 'every published target'
        printf 'targets: %s\n' "$(printf '%s' "$want" | tr '\n' ' ')"
        ;;
    *) die "unknown command: $1" ;;
esac
