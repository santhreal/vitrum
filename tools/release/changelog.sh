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

tmp=$(mktemp)
trap 'rm -f "$tmp" "$FILE.roll.tmp"' EXIT HUP INT TERM

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
        # `--merge` is for a first release whose section was written before the
        # tag existed: this file can already carry `## v0.1.0 - <a date>` for a
        # version that was never published. Rolling Unreleased into a second
        # `## v0.1.0` heading would leave two sections for one version, and
        # every reader of this file — the publish job, the in-app dialog —
        # takes the first one, which is the older and wrong one. So the two are
        # merged into one section instead, newest content first, dated the day
        # it is actually cut. Re-dating is not rewriting history: a section for
        # a version that was never tagged has no release date yet.
        merge=no
        [ "${2:-}" = --merge ] && { merge=yes; shift; }
        [ $# -ge 2 ] || die 'roll needs a version'
        v=${2#v}
        date=${3:-$(date -u +%Y-%m-%d)}
        unreleased=$(trimmed Unreleased)
        [ -n "$unreleased" ] || die "$FILE has no unreleased content"

        if grep -q "^## v$v " "$FILE"; then
            [ "$merge" = yes ] ||
                die "$FILE already has a section for v$v"

            # Drop the Unreleased section, then reopen an empty one above the
            # existing release heading and put its body back under the new
            # dated heading, ahead of what was already there.
            awk '
                /^## Unreleased[ \t]*$/ { skip = 1; next }
                skip && /^## / { skip = 0 }
                skip { next }
                { print }
            ' "$FILE" > "$tmp"
            awk -v want="## v$v " -v heading="## v$v - $date" -v body="$unreleased" '
                !done && index($0, want) == 1 {
                    print "## Unreleased"
                    print ""
                    print heading
                    print ""
                    print body
                    # No blank line here: the blank that followed the old
                    # heading is the next input line and separates the merged
                    # body from the one that was already there.
                    done = 1
                    next
                }
                { print }
            ' "$tmp" > "$FILE.roll.tmp"
            mv "$FILE.roll.tmp" "$FILE"
            printf 'changelog: merged Unreleased into the existing v%s section, dated %s\n' \
                "$v" "$date"
        else
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
        fi
        ;;
    *) die "unknown command: $cmd" ;;
esac
