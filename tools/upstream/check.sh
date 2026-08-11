#!/bin/sh
# Check a vendored fork against the crate it was forked from.
#
# Answers two questions a fork has to keep answering or it rots:
#
#   1. Is the divergence still exactly what the fork's UPSTREAM.toml claims?
#   2. Has upstream released a version newer than the one we forked?
#
# The first catches a change made to the fork without recording why, and a
# recorded divergence that has quietly become dead. The second is the
# absorption trigger: it is the only thing that will ever tell you to go and
# merge.
#
# Needs network. Run it from anywhere; paths resolve against the repository.
#
#   sh tools/upstream/check.sh --fork DIR           check one fork
#   sh tools/upstream/check.sh --patches D          also write divergences to D/
#
# The forks in this repository are `vendor-pty` (portable-pty) and
# `vendor-ghostty-vt-sys`. Each names what to compare in its own UPSTREAM.toml
# `compare` key, so a fork whose only change is a build script is checked as
# closely as one that changes library code.
#
# `--patches` is step one of an absorption: it extracts what the fork changed,
# as patches against the release it forked, so the changes can be replayed onto
# a newer one. Reproducing the fork from pristine upstream plus those patches
# is exactly what the procedure in its README.md does.
#
# Exit 0 clean, 1 drift or a newer upstream, 2 the check itself could not run.

set -eu

patches=""
fork="vendor-pty"
while [ $# -gt 0 ]; do
  case "$1" in
    --patches)
      shift
      [ $# -gt 0 ] || { echo "--patches needs a directory" >&2; exit 2; }
      patches="$1"
      ;;
    --fork)
      shift
      [ $# -gt 0 ] || { echo "--fork needs a directory" >&2; exit 2; }
      fork="$1"
      ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

root=$(cd "$(dirname "$0")/../.." && pwd)
manifest="$root/$fork/UPSTREAM.toml"

[ -f "$manifest" ] || { echo "no $manifest" >&2; exit 2; }

crate=$(sed -n 's/^crate *= *"\(.*\)"/\1/p' "$manifest" | head -1)
version=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$manifest" | head -1)
[ -n "$crate" ] && [ -n "$version" ] || { echo "$manifest names no crate and version" >&2; exit 2; }

echo "fork tracks $crate $version"

work=$(mktemp -d)
# The download and the extract both write here, so it goes on any exit path
# including the failure ones below.
trap 'rm -rf "$work"' EXIT INT TERM

url="https://static.crates.io/crates/$crate/$crate-$version.crate"
curl -sSL --fail --max-time 120 "$url" -o "$work/c.crate" \
  || { echo "could not download $url" >&2; exit 2; }
tar xzf "$work/c.crate" -C "$work" \
  || { echo "could not extract $crate-$version.crate" >&2; exit 2; }

# `compare` names what is checked, relative to the crate root, space separated.
# A fork that changes a build script rather than library code has to be able to
# say so; hardcoding `src` would leave that change unchecked, which is the one
# thing this script exists to prevent.
compare=$(sed -n 's/^compare *= *"\(.*\)"/\1/p' "$manifest" | head -1)
[ -n "$compare" ] || compare="src"
crate_root="$work/$crate-$version"

: > "$work/actual"
extra=""
for path in $compare; do
  pristine="$crate_root/$path"
  vendored="$root/$fork/$path"
  [ -e "$pristine" ] || { echo "the published crate has no $path" >&2; exit 2; }
  [ -e "$vendored" ] || { echo "no $vendored" >&2; exit 2; }

  if [ -d "$pristine" ]; then
    # Paths are printed relative to the crate root so they read the same way as
    # the `file =` entries in the manifest.
    diff -rq "$pristine" "$vendored" 2>/dev/null \
      | sed -n "s|^Files $pristine/\(.*\) and .* differ\$|$path/\1|p" \
      >> "$work/actual"
    # `Only in` lines are a file added or deleted rather than edited. Either is
    # a divergence the manifest cannot express, so it is always a failure.
    found=$(diff -rq "$pristine" "$vendored" 2>/dev/null | grep '^Only in' || true)
    [ -z "$found" ] || extra=$(printf '%s\n%s' "$extra" "$found")
  elif ! cmp -s "$pristine" "$vendored"; then
    printf '%s\n' "$path" >> "$work/actual"
  fi
done
sort -o "$work/actual" "$work/actual"

sed -n 's/^file *= *"\(.*\)"/\1/p' "$manifest" | sort > "$work/declared"

status=0

if [ -n "$extra" ]; then
  echo
  echo "FAIL: a file was added or removed, which the manifest cannot describe:"
  echo "$extra" | sed 's/^/  /'
  status=1
fi

undeclared=$(comm -23 "$work/actual" "$work/declared")
if [ -n "$undeclared" ]; then
  echo
  echo "FAIL: diverges from upstream but is not in $fork/UPSTREAM.toml:"
  echo "$undeclared" | sed 's/^/  /'
  echo "  Record why, or revert it to upstream."
  status=1
fi

dead=$(comm -13 "$work/actual" "$work/declared")
if [ -n "$dead" ]; then
  echo
  echo "FAIL: declared in $fork/UPSTREAM.toml but no longer differs:"
  echo "$dead" | sed 's/^/  /'
  echo "  Drop the entry. A dead divergence makes the real ones harder to see."
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "divergence is exactly as declared:"
  sed 's/^/  /' "$work/declared"
fi

# Patches come from the files that REALLY differ, not from what the manifest
# declares. Extracting a declared-but-identical file would write an empty patch
# and quietly drop a divergence during the absorption it was needed for.
if [ -n "$patches" ]; then
  mkdir -p "$patches"
  echo
  echo "writing divergence patches to $patches:"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    name=$(echo "$file" | sed 's|^src/||; s|/|-|g').patch
    # diff exits 1 when files differ, which is the expected case here.
    diff -u "$pristine/${file#src/}" "$vendored/${file#src/}" > "$patches/$name" || true
    echo "  $name ($(wc -l < "$patches/$name" | tr -d ' ') lines)"
  done < "$work/actual"
  echo "  Apply with: patch -p0 <target-file> -i $patches/<name>.patch"
fi

# The absorption trigger. Alpha and beta releases are ignored on purpose: the
# fork tracks what a user installing vitrum would otherwise resolve.
latest=$(curl -sSL --fail --max-time 60 \
  -H 'User-Agent: vitrum-upstream-check (github.com/santhreal/vitrum)' \
  "https://crates.io/api/v1/crates/$crate" 2>/dev/null \
  | tr ',' '\n' | sed -n 's/.*"max_stable_version":"\([^"]*\)".*/\1/p' | head -1)

echo
if [ -z "$latest" ]; then
  echo "WARN: could not ask crates.io for the newest release; divergence was still checked"
elif [ "$latest" = "$version" ]; then
  echo "up to date: $latest is the newest stable release"
else
  echo "FAIL: upstream is at $latest and this fork is at $version"
  echo "  Absorb it: see the procedure in $fork/README.md."
  status=1
fi

exit "$status"
