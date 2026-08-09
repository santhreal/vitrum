#!/bin/sh
# CHANGELOG.md as a release input.
#
#   tools/release/changelog.sh unreleased          print the Unreleased body
#   tools/release/changelog.sh notes <version>     print one release's body
#   tools/release/changelog.sh roll <version> [date]
#
# `roll` renames the `## Unreleased` heading to `## v<version> - <date>` and
# opens a fresh empty `## Unreleased` above it. The empty section is not
# decoration: the next cut refuses to run against it, so a release whose notes
# nobody wrote stops here rather than shipping a heading with nothing under it.
#
# `.github/workflows/release.yml` reads the same file with the same matcher, so
# a heading this writes is a heading the publish step can find.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

FILE=CHANGELOG.md

die() { printf 'changelog: %s\n' "$*" >&2; exit 1; }

# Prints the body under `## <heading>`, stopping at the next `## `. The heading
# is matched literally rather than as a regex: a version is full of dots, and a
# dot matches anything, so `## v0.1.0` as a pattern also accepts `## v0x1y0`
# and prefix-matches `## v0.1.01`.
section() {
    awk -v want="## $1" '
        index($0, want) == 1 {
            rest = substr($0, length(want) + 1)
            if (rest == "" || rest ~ /^[ \t]/) { found = 1; next }
        }
        found && /^## / { exit }
        found { print }
    ' "$FILE"
}

# Body with leading and trailing blank lines stripped, so "is there anything
# here" is a test on content and not on the blank line every section carries.
trimmed() {
    section "$1" | awk '
        { line[NR] = $0 }
        END {
            first = 1; last = NR
            while (first <= last && line[first] ~ /^[ \t]*$/) first++
            while (last >= first && line[last] ~ /^[ \t]*$/) last--
            for (i = first; i <= last; i++) print line[i]
        }
    '
}

cmd=${1:-unreleased}
case "$cmd" in
    unreleased)
        body=$(trimmed Unreleased)
        [ -n "$body" ] || die "$FILE has no unreleased content"
        printf '%s\n' "$body"
        ;;
    notes)
        [ $# -eq 2 ] || die 'notes needs a version'
        v=${2#v}
        body=$(trimmed "v$v")
        [ -n "$body" ] || die "$FILE has no section for v$v"
        printf '%s\n' "$body"
        ;;
    roll)
        [ $# -ge 2 ] || die 'roll needs a version'
        v=${2#v}
        date=${3:-$(date -u +%Y-%m-%d)}
        [ -n "$(trimmed Unreleased)" ] || die "$FILE has no unreleased content"
        grep -q "^## v$v " "$FILE" && die "$FILE already has a section for v$v"
        awk -v heading="## v$v - $date" '
            !done && /^## Unreleased[ \t]*$/ {
                print "## Unreleased"
                print ""
                print heading
                done = 1
                next
            }
            { print }
        ' "$FILE" > "$FILE.roll.tmp"
        mv "$FILE.roll.tmp" "$FILE"
        printf 'changelog: rolled Unreleased into v%s - %s\n' "$v" "$date"
        ;;
    *) die "unknown command: $cmd" ;;
esac
