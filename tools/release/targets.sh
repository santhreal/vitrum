#!/bin/sh
# The published target triples, and the check that everything agrees on them.
#
#   tools/release/targets.sh list    print the published triples
#   tools/release/targets.sh check   fail if any file names a different set
#
# Three files have an opinion about which platforms a release carries, and this
# compares all three: the build matrix in `.github/workflows/release.yml`, the
# `uname` mapping in `install.sh`, and the table in `docs/install.md`. A triple
# that exists in the matrix and not in the installer is an asset nobody
# downloads; a triple in the installer and not in the matrix is a 404 at the
# end of a `curl | sh`. Neither shows up until someone runs the installer on
# that platform, which is after the release is published. A table that names a
# build nobody publishes is the same failure with a longer delay.
#
# The set is Linux. The client presents its terminal pane to an X11 window, so
# a macOS or Windows archive would install a shell whose pane paints nothing.
# Both platforms still build and pass the suite in `platforms.yml`, and the
# day a pane exists there, the triple is added here first.
#
# `update.rs` inside the app is not a fourth site. It builds the asset name
# from `VITRUM_TARGET`, which the build script takes from the compiler, so it
# names whatever platform it was compiled for and cannot disagree with this
# list.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

TARGETS='aarch64-unknown-linux-gnu
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
        compare install.sh "$want" 'every published target'
        compare docs/install.md "$want" 'every published target'
        printf 'targets: %s\n' "$(printf '%s' "$want" | tr '\n' ' ')"
        ;;
    *) die "unknown command: $1" ;;
esac
