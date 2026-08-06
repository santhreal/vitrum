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
# not attempted: the binaries link against the system webview.
set -eu

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
target=$(rustc -vV | sed -n 's/^host: //p')
out="dist"
name="vitrum-${version}-${target}.tar.gz"

cargo build --release --locked

bin=$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/release

mkdir -p "$out"
tar czf "$out/$name" -C "$bin" vitrum vitrum-server

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
  sha256sum "$name" >> SHA256SUMS
)

echo "built $out/$name"
echo "sums:"
cat "$out/SHA256SUMS"
