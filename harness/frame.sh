#!/usr/bin/env bash
# Attribute a frame to its phases, and prove the probe costs nothing when off.
#
# usage: harness/frame.sh [rounds] [frames-per-round]
#
# The in-process probe lives behind the `probe` cargo feature, which is off by
# default, so no single binary can measure both a build that carries the probe
# and one that does not. This builds both, alternates them round by round so a
# machine that drifts charges both arms equally, and pairs the rounds:
#
#   absent   built without the feature. No probe instruction exists.
#   off      built with the feature, switch off.
#   on       built with the feature, switch on, recording per phase.
#
# The difference that has to be nothing is `off` minus `absent`. It is judged
# against the noise band measured between consecutive `absent` rounds, which is
# what the same arm measured against itself is worth on this machine.
#
# Everything runs headless on a GPU device with no display. Nothing graphical
# is started and no display is opened.
#
# Output: one harness/out/frame-<timestamp>/report.json, plus the per-round
# reports it was built from.
set -euo pipefail

ROUNDS="${1:-6}"
FRAMES="${2:-400}"

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(dirname -- "$HARNESS_DIR")"
cd "$REPO"

command -v cargo >/dev/null || { echo "frame.sh: cargo is not installed" >&2; exit 1; }
command -v python3 >/dev/null || { echo "frame.sh: python3 is not installed" >&2; exit 1; }

TARGET="$(cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

OUT="$HARNESS_DIR/out/frame-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$OUT/bin"

# Two binaries out of one source tree. They differ only in whether the
# instrumentation points are compiled, which is the whole question.
cargo build --release -p vitrum-bench
cp "$TARGET/release/vitrum-bench" "$OUT/bin/absent"
cargo build --release -p vitrum-bench --features probe
cp "$TARGET/release/vitrum-bench" "$OUT/bin/probe"

echo "rounds $ROUNDS, $FRAMES frames per arm per round"
for r in $(seq 1 "$ROUNDS"); do
  "$OUT/bin/absent" frame --frames "$FRAMES" --rounds 1 --out "$OUT/absent/$r" >/dev/null
  "$OUT/bin/probe" frame --frames "$FRAMES" --rounds 1 --out "$OUT/probe/$r" >/dev/null
  printf 'round %s of %s\n' "$r" "$ROUNDS"
done

python3 "$HARNESS_DIR/frame_compare.py" "$OUT" >"$OUT/report.json"
python3 "$HARNESS_DIR/frame_compare.py" --text "$OUT"
echo "results $OUT"
