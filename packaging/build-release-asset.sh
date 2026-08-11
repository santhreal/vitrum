#!/bin/sh
# Build the release archive and checksum that `vitrum update` installs.
#
# The archive name must match what the updater looks for, which is
# `vitrum-<version>-<target>.tar.gz` from `update::archive_name`. A test in
# `app/src/update.rs` asserts this script and that function agree, so changing
# the shape in one place fails the build rather than silently publishing an
# asset no client will ever find.
#
# Run once per platform you publish for, on that platform. Cross-compiling is
# not attempted: the binaries link against the platform's own toolkit.
#
# LINUX IS BUILT AGAINST AN OLD GLIBC, ON PURPOSE
#
# A dynamically linked binary records the symbol version it was linked against
# rather than the oldest one that would serve. Built on the runner's own glibc,
# the published 0.1.2 Linux archive required GLIBC_2.39 — the version on
# ubuntu-latest — and refused to start on Debian 12, Ubuntu 22.04 and RHEL 9.
# Two symbols did it, `pidfd_spawnp` and `pidfd_getpid`, which std uses when the
# BUILD host's glibc offers them and which nothing in this source asked for.
#
# So Linux goes through `cargo-zigbuild`, which links zig's stubs for a named
# glibc release instead of the host's. `check-abi.sh` then reads the artifact
# and fails above the floor, because a build flag can be lost in a refactor
# while the binary is what the user receives.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

# The oldest glibc a published Linux binary may require. Reaches RHEL 8,
# Debian 10, Ubuntu 20.04 and everything newer. `tools/release/check-abi.sh`
# carries the same number and is what enforces it.
GLIBC_FLOOR=2.28

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
target=$(rustc -vV | sed -n 's/^host: //p')
out="dist"
name="vitrum-${version}-${target}.tar.gz"

target_dir=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')

case "$target" in
  *-linux-gnu)
    command -v cargo-zigbuild >/dev/null 2>&1 || {
      echo "cargo-zigbuild is required to build the Linux archive at the" >&2
      echo "GLIBC_$GLIBC_FLOOR floor: cargo install cargo-zigbuild --locked" >&2
      exit 2
    }
    command -v zig >/dev/null 2>&1 || {
      echo "zig is required by cargo-zigbuild and by the terminal engine" >&2
      exit 2
    }
    # Two link flags the client cannot be built without, both a consequence of
    # linking against the host's GTK while targeting an older glibc:
    #
    #   -L native=... : zig searches its own sysroot and the cargo output
    #   directories, and nothing else, so `-lgtk-3` and the rest are simply not
    #   found. Passing the multiarch directory is what makes the system
    #   libraries visible; only their sonames end up in the artifact.
    #
    #   --allow-shlib-undefined : the host's libgmodule references
    #   `dlclose@GLIBC_2.34`, which the 2.28 stubs do not define, and lld
    #   refuses a shared library whose own dependencies it cannot resolve.
    #   Those symbols are resolved by the real libc on the target machine, and
    #   an undefined symbol in vitrum's own objects is still an error.
    libdir=$(dirname "$(cc -print-file-name=libc.so.6 2>/dev/null)")
    [ -d "$libdir" ] || libdir=/usr/lib/$(uname -m)-linux-gnu
    RUSTFLAGS="${RUSTFLAGS:-} -L native=$libdir -C link-arg=-Wl,--allow-shlib-undefined" \
      cargo zigbuild --release --locked --target "${target}.${GLIBC_FLOOR}" \
      -p vitrum -p vitrum-server
    # A named --target puts the output under the triple, without the glibc
    # suffix, which is not where a plain release build lands.
    bin="$target_dir/$target/release"
    ;;
  *)
    cargo build --release --locked -p vitrum -p vitrum-server
    bin="$target_dir/release"
    ;;
esac

# Windows names the same two binaries with an extension, and the archive has to
# carry whatever is actually on disk or tar fails with nothing to add.
case "$target" in
  *windows*) files="vitrum.exe vitrum-server.exe" ;;
  *) files="vitrum vitrum-server" ;;
esac

# Before it is packaged, not after it is published. On Linux this is the gate
# that proves the floor above was actually applied; everywhere else it finds no
# ELF and says so.
( cd "$bin" && "$root/tools/release/check-abi.sh" $files )

mkdir -p "$out"
tar czf "$out/$name" -C "$bin" $files

# `sha256sum` is GNU; macOS has `shasum -a 256`, which prints the same two
# fields in the same order and reads the same file back.
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$@"
  else
    shasum -a 256 "$@"
  fi
}

# One SHA256SUMS lists every platform's archive and each platform runs this
# script on its own machine, so this appends rather than overwrites. It first
# drops any line for this archive: rebuilding the same platform used to leave
# two digests under one name, and a verifier that takes the first one would
# reject the archive sitting next to it.
(
  cd "$out"
  if [ -f SHA256SUMS ]; then
    grep -v "  ${name}\$" SHA256SUMS > SHA256SUMS.tmp || true
    mv SHA256SUMS.tmp SHA256SUMS
  fi
  sha256 "$name" >> SHA256SUMS
)

echo "built $out/$name"
echo "sums:"
cat "$out/SHA256SUMS"
