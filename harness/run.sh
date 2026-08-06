#!/usr/bin/env bash
# Drive vitrum on a remote measurement host and bring the results back here.
#
# usage: harness/run.sh <command> [args...]
#
#   probe                              what the measurement host is, and what it is missing
#   screenshot <name> [WxH] [scale]    one window, captured, written to harness/out/
#   memory <windows>                   N windows each showing a session, PSS across the tree
#   idle-cpu <seconds> [windows]       CPU burned by an idle client, as a share of one core
#   bench <sessions>                   vitrum against T3 Code, same mocked model, same sessions
#   stress                             load, concurrency and fuzz workloads, daemon profiled
#
# Nothing graphical ever runs on the machine you type this on. This script
# compiles nothing, starts no X server, and launches neither the application
# nor the daemon locally. It copies two already-built binaries to a host whose
# only job is to be measured, runs them there under a private virtual display,
# and copies the results back into harness/out/.
#
# The build stays here on purpose. The measurement host has no Rust toolchain
# and does not need one; keeping the compiler on one side and the display on
# the other is what makes it safe to wipe the remote state between runs.
#
# environment:
#   HARNESS_ENDPOINT     one ssh destination, skipping the fallback search
#   HARNESS_ENDPOINTS    the ordered list to search, space separated
#   HARNESS_BIN_DIR      directory holding vitrum and vitrum-server
#   HARNESS_SCREEN       virtual screen size, default 1920x1080
#   HARNESS_SETTLE       seconds to let the app settle before measuring, default 45
#   HARNESS_STARTUP      seconds between the window mapping and a screenshot, default 8
#   HARNESS_SESSION_CMD  what each session runs, default /bin/bash
#   HARNESS_SESSION_ARGS arguments for it, default -i
#   HARNESS_KEEP_REMOTE  set to 1 to leave the run directory on the remote
#   HARNESS_BENCH_TURNS  agent turns per session in `bench`, default 40
#   HARNESS_BENCH_TOKENS tokens per mocked response, default 200
#   HARNESS_BENCH_TPS    tokens per second the mock streams, default 30
#   HARNESS_BENCH_SEED   the mock's seed, default 1
#   HARNESS_T3           path to the T3 Code binary, if it is not on PATH
#   HARNESS_STRESS_SESSIONS    sessions in the `stress` load workload, default 60
#   HARNESS_STRESS_LINES       lines each of them writes, default 60000
#   HARNESS_STRESS_DRAIN       seconds to wait for the last exit, default 180
#   HARNESS_STRESS_CONNECTIONS connections in the concurrency workload, default 12
#   HARNESS_STRESS_RACE_SESSIONS sessions each of them creates, default 6
#   HARNESS_STRESS_RENAMES     rename rounds per connection, default 8
#   HARNESS_STRESS_SETTLE      convergence budget for the concurrency workload, default 90
#   HARNESS_STRESS_CASES       fuzz cases, default 4000
#   HARNESS_STRESS_SEED        the fuzzer's seed, default 1
#   HARNESS_STRESS_INTERVAL    profiler sampling interval in seconds, default 0.25
set -euo pipefail

HARNESS_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(dirname -- "$HARNESS_DIR")"
OUT_ROOT="$HARNESS_DIR/out"

# perf-host first because it is the bigger and quieter of the two, labhost
# second. Each name appears twice: once as the ~/.ssh/config alias, which goes
# over Tailscale, and once as the LAN address. Tailscale SSH on this tailnet is
# in check mode, so the alias can demand a browser login and then time out;
# the LAN address reaches the ordinary sshd and authenticates with the key.
# Whichever answers first is cached in harness/out/.endpoint.
: "${HARNESS_ENDPOINTS:=perfhost perfhost@192.0.2.10 labhost labhost@192.0.2.11}"

SSH_CTL="/tmp/vitrum-harness-ssh-%C"
SSH_OPTS=(
  -o BatchMode=yes
  -o ConnectTimeout=8
  -o StrictHostKeyChecking=accept-new
  -o ControlMaster=auto
  -o ControlPath="$SSH_CTL"
  -o ControlPersist=120
)

die() {
  echo "harness: $*" >&2
  exit 1
}

usage() {
  sed -n '4,11s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" >&2
  exit 2
}

# --- the remote -------------------------------------------------------------

endpoint_answers() {
  # The Tailscale path can accept the TCP connection and then sit waiting for a
  # browser login, which ConnectTimeout does not cover, so the whole attempt is
  # bounded from outside.
  timeout 12 ssh "${SSH_OPTS[@]}" "$1" true >/dev/null 2>&1
}

pick_endpoint() {
  local cached="$OUT_ROOT/.endpoint" ep

  if [ -n "${HARNESS_ENDPOINT:-}" ]; then
    endpoint_answers "$HARNESS_ENDPOINT" || die "HARNESS_ENDPOINT=$HARNESS_ENDPOINT does not answer"
    printf '%s\n' "$HARNESS_ENDPOINT"
    return 0
  fi
  if [ -s "$cached" ]; then
    ep="$(cat "$cached")"
    if endpoint_answers "$ep"; then
      printf '%s\n' "$ep"
      return 0
    fi
    echo "harness: cached endpoint $ep no longer answers, searching again" >&2
  fi
  for ep in $HARNESS_ENDPOINTS; do
    echo "harness: trying $ep" >&2
    if endpoint_answers "$ep"; then
      mkdir -p "$OUT_ROOT"
      printf '%s\n' "$ep" >"$cached"
      printf '%s\n' "$ep"
      return 0
    fi
  done
  die "no measurement host answered. Tried: $HARNESS_ENDPOINTS"
}

rsh() {
  ssh "${SSH_OPTS[@]}" "$ENDPOINT" "$@"
}

# --- the binaries -----------------------------------------------------------

find_bin_dir() {
  local candidates=() dir
  [ -n "${HARNESS_BIN_DIR:-}" ] && candidates+=("$HARNESS_BIN_DIR")
  [ -n "${CARGO_TARGET_DIR:-}" ] && candidates+=("$CARGO_TARGET_DIR/release")
  candidates+=("$REPO/target/release")
  for dir in "${candidates[@]}"; do
    if [ -x "$dir/vitrum" ] && [ -x "$dir/vitrum-server" ]; then
      printf '%s\n' "$dir"
      return 0
    fi
  done
  cat >&2 <<EOF
harness: no release build found. Looked in:
$(printf '  %s\n' "${candidates[@]}")

Build it here, then run this again:
  cargo build --release -p vitrum -p vitrum-server

This script will not build for you. A measurement run that silently rebuilds
is a measurement run that can report a binary you did not mean to test.
EOF
  exit 2
}

glibc_of() {
  # "ldd (Ubuntu GLIBC 2.39-0ubuntu8.7) 2.39" -> "2.39"
  sed -n '1s/.* //p'
}

check_glibc() {
  local here there
  here="$(ldd --version | glibc_of)" || die "cannot read the local glibc version"
  there="$(rsh 'ldd --version' | glibc_of)" || die "cannot read the glibc version on $ENDPOINT"
  [ -n "$here" ] && [ -n "$there" ] || die "could not parse a glibc version: here '$here', there '$there'"
  echo "glibc here $here, there $there"
  if [ "$(printf '%s\n%s\n' "$here" "$there" | sort -V | tail -1)" != "$there" ]; then
    die "the binary is built against glibc $here and the measurement host has $there; it will not load there. Build on a host no newer than the target."
  fi
}

# --- commands ---------------------------------------------------------------

[ $# -ge 1 ] || usage
COMMAND="$1"
shift

case "$COMMAND" in
  probe | screenshot | memory | idle-cpu | bench | stress) ;;
  *) usage ;;
esac

command -v ssh >/dev/null || die "ssh is not installed"
command -v rsync >/dev/null || die "rsync is not installed"

# Argument shapes are checked here as well as on the remote, so a typo costs a
# second rather than a round trip and a staged binary.
case "$COMMAND" in
  screenshot)
    [ $# -ge 1 ] || die "screenshot needs a name: harness/run.sh screenshot sidebar"
    case "$1" in */*) die "the screenshot name is a file name, not a path: $1" ;; esac
    ;;
  memory)
    [ $# -eq 1 ] || die "memory takes one argument, the window count"
    case "$1" in '' | *[!0-9]*) die "window count must be a whole number, got $1" ;; esac
    [ "$1" -ge 1 ] || die "window count must be at least 1"
    ;;
  idle-cpu)
    [ $# -ge 1 ] && [ $# -le 2 ] || die "idle-cpu takes seconds, and optionally a window count"
    case "$1" in '' | *[!0-9]*) die "seconds must be a whole number, got $1" ;; esac
    [ "$1" -ge 1 ] || die "seconds must be at least 1"
    if [ $# -eq 2 ]; then
      case "$2" in '' | *[!0-9]*) die "window count must be a whole number, got $2" ;; esac
    fi
    ;;
  bench)
    [ $# -eq 1 ] || die "bench takes one argument, the session count"
    case "$1" in '' | *[!0-9]*) die "session count must be a whole number, got $1" ;; esac
    [ "$1" -ge 1 ] || die "session count must be at least 1"
    ;;
esac

ENDPOINT="$(pick_endpoint)"
RUN_ID="$COMMAND-$(date -u +%Y%m%dT%H%M%SZ)"
LOCAL_OUT="$OUT_ROOT/$RUN_ID"
mkdir -p "$LOCAL_OUT"

echo "endpoint $ENDPOINT"
echo "run $RUN_ID"

STAGE=".cache/vitrum-harness"
rsh "mkdir -p $STAGE/bin"
# --delete so a script removed here is removed there, and --exclude=bin/ so it
# does not take the staged binaries with it. Without the exclusion a probe run
# wipes the pair a measurement run just uploaded.
rsync -a --delete --exclude=bin/ --exclude=__pycache__/ -e "ssh ${SSH_OPTS[*]}" \
  "$HARNESS_DIR/remote/" "$ENDPOINT:$STAGE/"

if [ "$COMMAND" != "probe" ]; then
  BIN_DIR="$(find_bin_dir)"
  echo "binaries $BIN_DIR"
  check_glibc
  # --delete is deliberately absent here: the bin directory holds exactly the
  # files this line sends, and a partial send that also wiped the previous set
  # would leave the host with nothing to run.
  STAGE_BINS=("$BIN_DIR/vitrum" "$BIN_DIR/vitrum-server")
  # vitrum-bench is only built when the stress workloads are wanted, so it is
  # sent when it exists rather than being a hard requirement of every run.
  [ -x "$BIN_DIR/vitrum-bench" ] && STAGE_BINS+=("$BIN_DIR/vitrum-bench")
  rsync -a --checksum -e "ssh ${SSH_OPTS[*]}" "${STAGE_BINS[@]}" "$ENDPOINT:$STAGE/bin/"
  rsh "chmod +x $STAGE/bin/*"
  echo "staged ${#STAGE_BINS[@]} binaries: $(cd "$BIN_DIR" && ls -l $(printf '%s ' "${STAGE_BINS[@]##*/}") | awk '{printf "%s %s bytes; ", $9, $5}')"
fi

# Environment does not survive an ssh command line by itself, so the settings
# that change what is measured are passed explicitly and echoed into the
# report. A number whose conditions are not written down next to it is not a
# measurement.
REMOTE_ENV=(
  "HARNESS_SCREEN=${HARNESS_SCREEN:-1920x1080}"
  "HARNESS_SETTLE=${HARNESS_SETTLE:-45}"
  "HARNESS_STARTUP=${HARNESS_STARTUP:-8}"
  "HARNESS_SESSION_CMD=${HARNESS_SESSION_CMD:-/bin/bash}"
  "HARNESS_SESSION_ARGS=${HARNESS_SESSION_ARGS:--i}"
  "HARNESS_BENCH_TURNS=${HARNESS_BENCH_TURNS:-40}"
  "HARNESS_BENCH_TOKENS=${HARNESS_BENCH_TOKENS:-200}"
  "HARNESS_BENCH_TPS=${HARNESS_BENCH_TPS:-30}"
  "HARNESS_BENCH_SEED=${HARNESS_BENCH_SEED:-1}"
  "HARNESS_T3=${HARNESS_T3:-}"
  "HARNESS_STRESS_SESSIONS=${HARNESS_STRESS_SESSIONS:-60}"
  "HARNESS_STRESS_LINES=${HARNESS_STRESS_LINES:-60000}"
  "HARNESS_STRESS_DRAIN=${HARNESS_STRESS_DRAIN:-180}"
  "HARNESS_STRESS_CONNECTIONS=${HARNESS_STRESS_CONNECTIONS:-12}"
  "HARNESS_STRESS_RACE_SESSIONS=${HARNESS_STRESS_RACE_SESSIONS:-6}"
  "HARNESS_STRESS_RENAMES=${HARNESS_STRESS_RENAMES:-8}"
  "HARNESS_STRESS_SETTLE=${HARNESS_STRESS_SETTLE:-90}"
  "HARNESS_STRESS_CASES=${HARNESS_STRESS_CASES:-4000}"
  "HARNESS_STRESS_SEED=${HARNESS_STRESS_SEED:-1}"
  "HARNESS_STRESS_INTERVAL=${HARNESS_STRESS_INTERVAL:-0.25}"
)

REMOTE_CMD="env $(printf '%q ' "${REMOTE_ENV[@]}")bash .cache/vitrum-harness/rig.sh $(printf '%q ' "$RUN_ID" "$COMMAND" "$@")"

set +e
rsh "$REMOTE_CMD" 2>&1 | tee "$LOCAL_OUT/report.txt"
STATUS="${PIPESTATUS[0]}"
set -e

# Artifacts and logs come back whatever the exit code was. A failed run is the
# one whose logs you actually want.
rsync -a -e "ssh ${SSH_OPTS[*]}" "$ENDPOINT:/tmp/vh-$RUN_ID/out/" "$LOCAL_OUT/" 2>/dev/null || true
rsync -a -e "ssh ${SSH_OPTS[*]}" "$ENDPOINT:/tmp/vh-$RUN_ID/log/" "$LOCAL_OUT/log/" 2>/dev/null || true

if [ "${HARNESS_KEEP_REMOTE:-0}" = "1" ]; then
  echo "left /tmp/vh-$RUN_ID on $ENDPOINT"
else
  rsh "rm -rf /tmp/vh-$RUN_ID" || true
fi

echo "results $LOCAL_OUT"
exit "$STATUS"
