#!/usr/bin/env bash
# A world: several windows on one daemon, sessions streaming, keystrokes timed.
#
# usage: harness/world.sh [windows] [streams] [keystrokes] [ssh-host]
#
# Stands up a population that matches how the product is operated — several
# windows on one daemon, every window attached to every session, some sessions
# streaming, optionally one running through ssh — drives it, and measures what
# the focused window's keystroke round trip costs while all of that is
# happening. The figures are distributions with their tails, the platform floor
# is measured in the same run, and it is subtracted only when the daemon is on
# this machine.
#
# The daemon it starts is private to the run: its token goes in a run-local
# XDG_RUNTIME_DIR and it listens on a free port, so a daemon the operator is
# already using keeps its token, its port and its sessions.
#
# Nothing graphical runs. There is no window and no display in any of this.
#
# Output: harness/out/world-<timestamp>/, holding the daemon log and the
# workload's own report.json and report.md.
set -euo pipefail

WINDOWS="${1:-4}"
STREAMS="${2:-7}"
KEYSTROKES="${3:-400}"
SSH_HOST="${4:-}"

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(dirname -- "$HARNESS_DIR")"
cd "$REPO"

command -v cargo >/dev/null || { echo "world.sh: cargo is not installed" >&2; exit 1; }
command -v python3 >/dev/null || { echo "world.sh: python3 is not installed" >&2; exit 1; }

TARGET="$(cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"

OUT="$HARNESS_DIR/out/world-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$OUT"
RUNTIME="$OUT/run"
mkdir -p "$RUNTIME"
chmod 700 "$RUNTIME"

cargo build --release -p vitrum-server -p vitrum-bench

PORT="$(python3 -c 'import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()')"

XDG_RUNTIME_DIR="$RUNTIME" "$TARGET/release/vitrum-server" --port "$PORT" \
  >"$OUT/daemon.log" 2>&1 &
DAEMON=$!
cleanup() {
  kill "$DAEMON" 2>/dev/null || true
  wait "$DAEMON" 2>/dev/null || true
}
trap cleanup EXIT

# Readiness is the port accepting, not the process existing.
XDG_RUNTIME_DIR="$RUNTIME" python3 -c "
import socket, sys, time
deadline = time.time() + 30
while time.time() < deadline:
    try:
        socket.create_connection(('127.0.0.1', $PORT), timeout=0.5).close()
        sys.exit(0)
    except OSError:
        time.sleep(0.1)
sys.exit('the daemon never accepted a connection on port $PORT')
"

echo "daemon on 127.0.0.1:$PORT, pid $DAEMON"

ARGS=(world
  --server "ws://127.0.0.1:$PORT/ws"
  --windows "$WINDOWS"
  --streams "$STREAMS"
  --keystrokes "$KEYSTROKES"
  --out "$OUT"
  --profile-pid "$DAEMON")
if [ -n "$SSH_HOST" ]; then
  ARGS+=(--ssh-host "$SSH_HOST")
fi

set +e
XDG_RUNTIME_DIR="$RUNTIME" "$TARGET/release/vitrum-bench" "${ARGS[@]}" | tee "$OUT/report.txt"
STATUS="${PIPESTATUS[0]}"
set -e

echo "results $OUT"
exit "$STATUS"
