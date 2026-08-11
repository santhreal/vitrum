#!/bin/sh
# The version literals a release must move, and the check that they agree.
#
#   tools/release/versions.sh current        print the workspace version
#   tools/release/versions.sh check          fail if any site disagrees
#   tools/release/versions.sh bump <version> rewrite every site
#   tools/release/versions.sh sites          list the files a bump touches
#   tools/release/versions.sh selftest       break each site, require a catch
#   tools/release/versions.sh nightly [sha]  the version a nightly publishes as
#
# A version literal lives in more places than the manifest, and the ones that
# must move are not the ones that look like they should. `install.sh` names
# `0.1.0` in its usage text, but it resolves the version from the releases API
# and never from that literal, so moving it would only rewrite an example.
# `docs/performance.md` names the build a measurement was
# taken on, which is a fact about the past. Both are excluded on purpose, and
# the exclusion is written down here rather than rediscovered every release.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

die() { printf 'versions: %s\n' "$*" >&2; exit 1; }

# Both lists below are read out of the manifests at run time rather than typed
# here. A hardcoded list of crates goes stale the first time somebody adds one,
# and it goes stale silently: the new crate's version simply stops being
# checked, which is the same as having no check for it. Adding a member now
# extends the guard by itself.

# Every workspace member whose manifest says `version.workspace = true`, and so
# moves when the workspace version moves. `vendor` and `vendor-pty` are forks
# pinned to the upstream release they fork; they carry their own literal
# version and drop out of this list on that basis rather than by name.
workspace_members() {
    awk '
        /^\[workspace\]/ { in_ws = 1; next }
        /^\[/ { in_ws = 0 }
        in_ws && /^members = \[/ { in_list = 1; next }
        in_list && /^\]/ { in_list = 0; next }
        in_list { gsub(/[ \t",]/, ""); if ($0 != "") print }
    ' Cargo.toml
}

members() {
    workspace_members | while read -r dir; do
        manifest="$dir/Cargo.toml"
        [ -f "$manifest" ] || die "workspace member $dir has no Cargo.toml"
        grep -q '^version.workspace = true' "$manifest" || continue
        name=$(sed -n 's/^name = "\(.*\)"$/\1/p' "$manifest" | head -1)
        [ -n "$name" ] || die "$manifest has no package name"
        printf '%s\n' "$name"
    done
}

# The internal dependencies in `[workspace.dependencies]` that carry a literal
# version beside their path, because `cargo package` refuses a path-only
# dependency and cargo has no `version.workspace = true` for that table.
#
# Only the ones that resolve to a member from `members` above. The vendored
# forks are declared the same way and their literal is the upstream release
# they fork, which must not move with this workspace; being absent from that
# list is what excludes them, rather than their names being written down here.
internal_deps() {
    sed -n 's/^\([a-z0-9-]*\) = { path = "[^"]*", version = "[^"]*".*/\1/p' Cargo.toml |
        while read -r dep; do
            for m in $MEMBERS; do
                if [ "$dep" = "$m" ]; then
                    printf '%s\n' "$dep"
                    break
                fi
            done
        done
}

MEMBERS=$(members)
INTERNAL_DEPS=$(internal_deps)
[ -n "$MEMBERS" ] || die 'no workspace member uses version.workspace = true'
[ -n "$INTERNAL_DEPS" ] || die 'no internal dependency carries a version literal'

current() {
    v=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
    [ -n "$v" ] || die 'Cargo.toml has no [workspace.package] version'
    printf '%s\n' "$v"
}

# The version a nightly is published under.
#
# It is a semver prerelease of the NEXT patch, not of the current one, so it
# sorts strictly between the last stable and the next: 0.1.0 < 0.1.1-nightly.*
# < 0.1.1. The update channel reads this out of the asset filename and has no
# other field to read, so the ordering is the whole contract. Bumping the
# workspace to it before the build also makes `vitrum --version` say which
# nightly it is, rather than repeating the last stable.
nightly() {
    sha=${1:-$(git rev-parse --short=7 HEAD)}
    day=${2:-$(date -u +%Y%m%d)}
    core=$(current)
    core=${core%%-*}
    major=${core%%.*}
    rest=${core#*.}
    minor=${rest%%.*}
    patch=${rest##*.}
    case $major$minor$patch in
        *[!0-9]*) die "cannot derive a nightly from version $(current)" ;;
    esac
    printf '%s.%s.%s-nightly.%s.%s\n' "$major" "$minor" "$((patch + 1))" "$day" "$sha"
}

# Each line is `<site> <literal>`. A site whose literal is missing reports the
# empty string, which never equals the workspace version, so a site that is
# renamed or deleted fails the check instead of silently dropping out of it.
observed() {
    printf 'Cargo.toml:[workspace.package] %s\n' "$(current)"

    for dep in $INTERNAL_DEPS; do
        lit=$(sed -n "s/^$dep = { path = \"[^\"]*\", version = \"\([^\"]*\)\".*/\1/p" \
            Cargo.toml | head -1)
        printf 'Cargo.toml:[workspace.dependencies].%s %s\n' "$dep" "${lit:-<missing>}"
    done

    for m in $MEMBERS; do
        lit=$(awk -v want="$m" '
            /^\[\[package\]\]/ { name=""; next }
            /^name = / { gsub(/^name = "|"$/, ""); name=$0; next }
            /^version = / && name == want {
                gsub(/^version = "|"$/, ""); print; exit
            }
        ' Cargo.lock)
        printf 'Cargo.lock:%s %s\n' "$m" "${lit:-<missing>}"
    done

    # The sentence that opens README.md's Status section. It is spelled out in
    # three places: this locator, the bump rewrite, and the self-test mutation.
    # All three have to agree with the document, and a `<missing>` report here
    # is what an edit to that sentence produces.
    #
    # `[^ ]*` rather than a dotted shape: a nightly version carries three
    # extra dot-separated fields after the patch, and a locator that counts
    # dots would silently capture a prefix of it and report agreement.
    lit=$(sed -n 's/^vitrum is at version \([^ ]*\)\..*/\1/p' README.md | head -1)
    printf 'README.md:status-line %s\n' "${lit:-<missing>}"

    # The release line SECURITY.md says it fixes. It carries a series rather
    # than a version, so what is compared is the series: the workspace version
    # with its own major.minor swapped for the one the policy names. Equal
    # exactly when the policy is talking about the line being released, and
    # unequal for a nightly too, whose suffix is carried through untouched.
    #
    # It is checked because it went stale in silence once already: the policy
    # promised fixes for 0.1.x through the whole of the 0.2 line, which is a
    # published statement that the current release is unsupported.
    series=$(sed -n \
        's/^Fixes go to the current release line, \([0-9][0-9]*\.[0-9][0-9]*\)\.x,.*/\1/p' \
        SECURITY.md | head -1)
    if [ -n "$series" ]; then
        cur=$(current)
        cur_major=${cur%%.*}
        cur_rest=${cur#*.}
        cur_series=$cur_major.${cur_rest%%.*}
        printf 'SECURITY.md:supported-line %s\n' "$series.${cur#"$cur_series".}"
    else
        printf 'SECURITY.md:supported-line <missing>\n'
    fi
}

check() {
    want=$(current)
    observed > "$tmp"
    awk -v want="$want" '$2 != want {
        printf "versions: %s says %s, workspace says %s\n", $1, $2, want > "/dev/stderr"
        bad = 1
    } END { exit bad }' "$tmp" ||
        die 'version literals disagree; run tools/release/versions.sh bump <version>'
    printf 'versions: %s agrees at %s sites\n' "$want" "$(wc -l < "$tmp" | tr -d ' ')"
}

# `sed -i` is a GNU extension and macOS wants an argument, so every rewrite
# goes through a temporary file that is moved into place.
rewrite() {
    file=$1
    script=$2
    sed "$script" "$file" > "$file.versions.tmp"
    mv "$file.versions.tmp" "$file"
}

bump() {
    new=$1
    case "$new" in
        [0-9]*.[0-9]*.[0-9]*) ;;
        *) die "not a version: $new" ;;
    esac
    old=$(current)

    # `1,/re/` rather than `0,/re/`, and the pattern written out rather than
    # reused as `s//`. Both of those are GNU extensions, and this script runs
    # on the macOS runners, where BSD sed accepted the address, resolved the
    # empty pattern to nothing, and substituted nothing. The internal
    # dependency rewrites below use ordinary patterns and did apply, so a
    # nightly bumped every `vitrum-* = { version = ... }` requirement and left
    # the workspace version alone, and cargo then refused to resolve a
    # workspace whose members demanded a version none of them carried.
    rewrite Cargo.toml \
        "1,/^version = \"$old\"$/s/^version = \"$old\"\$/version = \"$new\"/"
    for dep in $INTERNAL_DEPS; do
        rewrite Cargo.toml \
            "s|^\($dep = { path = \"[^\"]*\", version = \)\"$old\"|\1\"$new\"|"
    done
    rewrite README.md "s/^vitrum is at version $old\\./vitrum is at version $new./"
    # A patch release leaves the policy alone; a minor or major changes the
    # line it names, in all three places it names it.
    #
    # The prerelease is stripped before the series is taken. A nightly is
    # `0.2.2-nightly.<date>.<sha>`, whose dots are not version components, and
    # `${new%.*}` read the last of them: the series came out as
    # `0.2.2-nightly.20260810`, SECURITY.md was rewritten to name a line that
    # cannot exist, and every nightly build then failed its own version check
    # with `<missing>`.
    series_of() { s_core=${1%%-*}; printf '%s' "${s_core%.*}"; }
    old_series=$(series_of "$old")
    new_series=$(series_of "$new")
    if [ "$old_series" != "$new_series" ]; then
        rewrite SECURITY.md "s/\\b$old_series\\.x\\b/$new_series.x/g"
    fi

    # Cargo owns the lock file. `--workspace --offline` rewrites the version of
    # every workspace member and touches no dependency, which is exactly the
    # edit a bump makes, and it needs no network and no build.
    # `--offline` covers the normal case, where every dependency is already in
    # the registry cache. A cold machine has no cache to read, and refusing to
    # bump there would be refusing over a detail of the lock file's provenance.
    ${CARGO:-cargo} update --workspace --offline --quiet ||
        ${CARGO:-cargo} update --workspace --quiet

    check
}

sites() { printf '%s\n' Cargo.toml Cargo.lock README.md SECURITY.md; }

# Mutate exactly one site to a version nothing else carries, using the same
# locator `observed` reads it with, so a locator that has gone stale mutates
# nothing and the self-test says so.
mutate() {
    site=$1
    fake=9.9.9
    case $site in
        'Cargo.toml:[workspace.package]')
            rewrite Cargo.toml "0,/^version = \"[^\"]*\"\$/s//version = \"$fake\"/" ;;
        'Cargo.toml:[workspace.dependencies].'*)
            dep=${site##*.}
            rewrite Cargo.toml \
                "s|^\($dep = { path = \"[^\"]*\", version = \)\"[^\"]*\"|\1\"$fake\"|" ;;
        'Cargo.lock:'*)
            pkg=${site#Cargo.lock:}
            awk -v want="$pkg" -v fake="$fake" '
                /^\[\[package\]\]/ { name = ""; print; next }
                /^name = / { line = $0; gsub(/^name = "|"$/, ""); name = $0; print line; next }
                /^version = / && name == want && !done { print "version = \"" fake "\""; done = 1; next }
                { print }
            ' Cargo.lock > Cargo.lock.versions.tmp
            mv Cargo.lock.versions.tmp Cargo.lock ;;
        'README.md:status-line')
            rewrite README.md "s/^vitrum is at version [^ ]*\\./vitrum is at version $fake./" ;;
        'SECURITY.md:supported-line')
            # The series, not the version: the site reports a series widened to
            # a full version, so a fake version here would be read back as a
            # nonsense series. 9.9 is a line nothing is released from.
            rewrite SECURITY.md \
                "s/\\([0-9][0-9]*\\.[0-9][0-9]*\\)\\.x/9.9.x/g" ;;
        *) die "no mutation for site $site" ;;
    esac
}

# Break each site in turn and require `check` to notice. A consistency check
# nobody has seen fail is a consistency check that may only be reporting that
# it ran, and this list is read out of `observed` rather than typed here, so a
# site added there is covered the moment it is added.
selftest() {
    backup=$(mktemp -d)
    for f in $(sites); do cp "$f" "$backup/$f"; done
    restore() { for f in $(sites); do cp "$backup/$f" "$f"; done; }
    trap 'restore; rm -rf "$backup"; rm -f "$tmp"' EXIT HUP INT TERM

    check >/dev/null || die 'the tree is already inconsistent; cannot self-test'

    observed | cut -d' ' -f1 > "$backup/sites"
    n=0
    while read -r site; do
        mutate "$site"
        if cmp -s "$backup/${site%%:*}" "${site%%:*}"; then
            restore
            die "mutating $site changed nothing; its locator is stale"
        fi
        # `die` exits, so `check` is run in a subshell here; the point of this
        # call is its status, not its opinion about how the run should end.
        if ( check ) >/dev/null 2>&1; then
            restore
            die "check passed with $site set to 9.9.9"
        fi
        restore
        n=$((n + 1))
        printf 'versions: %s broken and caught\n' "$site"
    done < "$backup/sites"

    check >/dev/null || die 'restore left the tree inconsistent'
    printf 'versions: %s sites, each caught when broken alone\n' "$n"
}

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT HUP INT TERM

cmd=${1:-check}
case "$cmd" in
    current) current ;;
    nightly) shift; nightly "$@" ;;
    check) check ;;
    sites) sites ;;
    selftest) selftest ;;
    bump) [ $# -eq 2 ] || die 'bump needs a version'; bump "$2" ;;
    *) die "unknown command: $cmd" ;;
esac
