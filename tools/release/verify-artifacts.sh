#!/bin/sh
# Build a real release archive for this host and install it through install.sh.
#
#   tools/release/verify-artifacts.sh [scratch-dir]
#
# It runs the two halves of the published pipeline that a tag would run:
# `packaging/build-release-asset.sh` builds the archive exactly as the workflow
# does, and the checksum file is written the way the publish job writes it,
# `sha256sum *.tar.gz > SHA256SUMS` over everything present.
#
# Then it serves that directory over `file://` and runs `install.sh` against
# it. The installer is copied and its download base is repointed at the scratch
# directory; nothing else about it is changed, so the digest comparison, the
# missing-entry refusal and the mismatch refusal are the shipped ones. Two
# negative cases follow the good one, because a verifier that has never
# rejected anything has not been shown to verify.
#
# This publishes nothing and needs no network.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$root"

die() { printf 'verify: %s\n' "$*" >&2; exit 1; }
step() { printf '\n== %s\n' "$*"; }
ok() { printf '  ok  %s\n' "$*"; }

scratch=${1:-$(mktemp -d)}
mkdir -p "$scratch"
scratch=$(CDPATH= cd -- "$scratch" && pwd)
serve="$scratch/release"
mkdir -p "$serve"

version=$(tools/release/versions.sh current)
target=$(rustc -vV | sed -n 's/^host: //p')
archive="vitrum-$version-$target.tar.gz"

step "build $archive"
./packaging/build-release-asset.sh >/dev/null
[ -f "dist/$archive" ] || die "packaging did not produce dist/$archive"
cp "dist/$archive" "$serve/"

# The publish job writes one SHA256SUMS over every archive it collected, not
# the per-platform file the build script appends to. Written the same way here,
# with the same glob, so the file the installer reads is the file the workflow
# produces down to how the names are spelled in it.
# shellcheck disable=SC2035
( cd "$serve" && sha256sum *.tar.gz > SHA256SUMS )

size=$(wc -c < "$serve/$archive" | tr -d ' ')
digest=$(sha256sum "$serve/$archive" | cut -d' ' -f1)
printf '  archive  %s\n' "$archive"
printf '  size     %s bytes\n' "$size"
printf '  sha256   %s\n' "$digest"
printf '  contents %s\n' "$(tar tzf "$serve/$archive" | tr '\n' ' ')"
printf '  SHA256SUMS:\n'
sed 's/^/    /' "$serve/SHA256SUMS"

# `install.sh` names the two binaries it moves; an archive missing either one
# verifies and then fails halfway through installing.
for bin in vitrum vitrum-server; do
    tar tzf "$serve/$archive" | grep -qx "$bin" ||
        die "the archive has no $bin at its root"
done
ok 'the archive carries vitrum and vitrum-server at its root'

# The installer builds the archive name from the version and the host triple.
# If those two rules ever drift apart the entry lookup below fails, which is
# the same failure a user would get, so it is worth asserting here by name.
grep -qF "  $archive" "$serve/SHA256SUMS" ||
    die "SHA256SUMS has no entry for $archive"

step "install through install.sh"
# One substitution: the release base URL becomes this directory. Everything the
# installer does with what it downloads is untouched.
sed "s|^BASE=.*|BASE=\"file://$serve\"|" install.sh > "$scratch/install.sh"
grep -q "^BASE=\"file://$serve\"\$" "$scratch/install.sh" ||
    die 'could not repoint install.sh at the scratch release'
# `sed` must have changed exactly one line and nothing else.
[ "$(diff install.sh "$scratch/install.sh" | grep -c '^[<>]')" = 2 ] ||
    die 'repointing install.sh changed more than the download base'
ok 'install.sh copied with only its download base changed'

run_installer() {
    VITRUM_VERSION="$version" \
    VITRUM_INSTALL_DIR="$scratch/bin" \
    VITRUM_NO_INTEGRATE=1 \
        sh "$scratch/install.sh" 2>&1
}

rm -rf "$scratch/bin"
run_installer | sed 's/^/  /' || die 'install.sh refused a good archive'
# Both binaries, because the client and the daemon speak a versioned protocol
# and an archive that carries a matched pair is the thing being verified.
for bin in vitrum vitrum-server; do
    got=$("$scratch/bin/$bin" --version)
    printf '  %s\n' "$got"
    [ "$got" = "$bin $version" ] ||
        die "$bin reports '$got', expected '$bin $version'"
done
ok "installed and ran both binaries at $version from the verified archive"

step "the digest check refuses a tampered archive"
# One byte, in the compressed payload rather than the gzip header, so the
# refusal is the checksum's and not tar's.
cp "$serve/$archive" "$serve/$archive.good"
printf 'x' | dd of="$serve/$archive" bs=1 seek=64 conv=notrunc status=none
rm -rf "$scratch/bin"
if out=$(run_installer); then
    printf '%s\n' "$out" | sed 's/^/    /' >&2
    die 'install.sh installed an archive whose digest does not match'
fi
printf '%s\n' "$out" | grep -q 'checksum mismatch' ||
    { printf '%s\n' "$out" | sed 's/^/    /' >&2; die 'refused for the wrong reason'; }
[ ! -e "$scratch/bin/vitrum" ] || die 'a tampered archive still installed a binary'
printf '%s\n' "$out" | grep -A3 'checksum mismatch' | sed 's/^/  /'
ok 'refused, and installed nothing'
mv "$serve/$archive.good" "$serve/$archive"

step "the digest check refuses an archive SHA256SUMS does not cover"
# A one-archive scratch release leaves this empty, and an empty SHA256SUMS is
# exactly the shape a partial publish would produce, so `grep` finding nothing
# is the case under test rather than a failure.
grep -vF "$archive" "$serve/SHA256SUMS" > "$serve/SHA256SUMS.tmp" || true
mv "$serve/SHA256SUMS" "$serve/SHA256SUMS.full"
mv "$serve/SHA256SUMS.tmp" "$serve/SHA256SUMS"
rm -rf "$scratch/bin"
if out=$(run_installer); then
    printf '%s\n' "$out" | sed 's/^/    /' >&2
    die 'install.sh installed an archive with no SHA256SUMS entry'
fi
printf '%s\n' "$out" | grep -q "SHA256SUMS has no entry for $archive" ||
    { printf '%s\n' "$out" | sed 's/^/    /' >&2; die 'refused for the wrong reason'; }
[ ! -e "$scratch/bin/vitrum" ] || die 'an uncovered archive still installed a binary'
printf '%s\n' "$out" | grep -A3 'no entry for' | sed 's/^/  /'
ok 'refused, and installed nothing'
mv "$serve/SHA256SUMS.full" "$serve/SHA256SUMS"

printf '\n%s verified: %s bytes, sha256 %s\n' "$archive" "$size" "$digest"
printf 'artifacts under %s\n' "$serve"
