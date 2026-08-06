#!/usr/bin/env bash
# The half of the harness that runs on the measurement host.
#
# usage: rig.sh <run-id> <command> [args...]
#
#   probe
#   screenshot <name> [WxH] [ui-scale|auto]
#   memory <windows>
#   idle-cpu <seconds> [windows]
#
# harness/run.sh is the only intended caller: it stages the binaries, invokes
# this over ssh, and copies the results back. Running it by hand on the
# measurement host works and is sometimes what you want while debugging, but
# nothing here is a developer desktop tool. It starts an X server, an
# application and a daemon, and it must never be pointed at a machine somebody
# is sitting in front of.
#
# Everything this script starts, it starts in its own process group and kills
# by that group id. There is no `pkill` and no matching on process names: this
# host may be running somebody else's Xvfb, somebody else's vitrum, or a second
# copy of this harness, and a name match would take all of them down.
set -euo pipefail

RUN_ID="${1:-}"
COMMAND="${2:-}"
[ -n "$RUN_ID" ] && [ -n "$COMMAND" ] || {
  sed -n '4,9s/^# \{0,1\}//p' "$0" >&2
  exit 2
}
shift 2

STAGE="$HOME/.cache/vitrum-harness"
BIN="$STAGE/bin"
# The per-run tree lives under /tmp and not under the stage directory for one
# concrete reason: the single-instance socket is a filesystem socket, and
# `sun_path` is 108 bytes. `$XDG_RUNTIME_DIR/vitrum/instance.sock` under a home
# directory plus a cache path plus a run id gets uncomfortably close, and bind
# fails with ENAMETOOLONG rather than truncating.
RUN="/tmp/vh-$RUN_ID"
OUT="$RUN/out"
LOG="$RUN/log"

SCREEN="${HARNESS_SCREEN:-1920x1080}"
SETTLE="${HARNESS_SETTLE:-45}"
STARTUP="${HARNESS_STARTUP:-8}"
SESSION_CMD="${HARNESS_SESSION_CMD:-/bin/bash}"
SESSION_ARGS="${HARNESS_SESSION_ARGS:--i}"
PORT=7737
# The size of the decoy X window every vitrum process maps alongside the real
# one. Compared exactly, never as a substring; see `app_windows`.
DECOY_GEOMETRY="10x10"

APP_PID=""
DAEMON_PID=""
XVFB_PID=""
DBUS_PID=""

die() {
  echo "rig: $*" >&2
  exit 1
}

cleanup() {
  local rc=$?
  set +e
  if [ -n "$APP_PID" ]; then kill -TERM -"$APP_PID" 2>/dev/null; fi
  if [ -n "$DAEMON_PID" ]; then kill -TERM -"$DAEMON_PID" 2>/dev/null; fi
  sleep 1
  if [ -n "$APP_PID" ]; then kill -KILL -"$APP_PID" 2>/dev/null; fi
  if [ -n "$DAEMON_PID" ]; then kill -KILL -"$DAEMON_PID" 2>/dev/null; fi
  if [ -n "$DBUS_PID" ]; then kill -TERM "$DBUS_PID" 2>/dev/null; fi
  if [ -n "$XVFB_PID" ]; then kill -TERM "$XVFB_PID" 2>/dev/null; fi
  return $rc
}

# ---------------------------------------------------------------------------
# The pieces every measurement command needs
# ---------------------------------------------------------------------------

# Start a command as its own session leader, so the whole subtree can be killed
# by group id later. The wrapper writes its own pid before exec'ing, and exec
# keeps that pid, so the number in the pid file is both the process and the
# group.
spawn_group() {
  local pidfile="$1" logfile="$2"
  shift 2
  setsid bash -c 'echo $$ >"$1"; exec "${@:3}" >>"$2" 2>&1' _ "$pidfile" "$logfile" "$@" &
  local i=0
  while [ ! -s "$pidfile" ] && [ "$i" -lt 200 ]; do
    sleep 0.05
    i=$((i + 1))
  done
  [ -s "$pidfile" ] || return 1
  cat "$pidfile"
}

# The lowest X display number nobody is using.
#
# This host already runs an Xvfb on :99 that belongs to something else. Taking
# a fixed number would either fail or, worse, succeed against a server whose
# contents are not ours: tools/regression/screenshot.sh records ten minutes
# spent measuring another process's window on a shared display.
pick_display() {
  local n
  for n in $(seq 101 199); do
    [ -e "/tmp/.X${n}-lock" ] && continue
    [ -e "/tmp/.X11-unix/X${n}" ] && continue
    printf ':%s\n' "$n"
    return 0
  done
  return 1
}

start_x() {
  DISPLAY="$(pick_display)" || die "no free X display between :101 and :199"
  export DISPLAY
  Xvfb "$DISPLAY" -screen 0 "${SCREEN}x24" -dpi 96 -nolisten tcp -noreset \
    >"$LOG/xvfb.log" 2>&1 &
  XVFB_PID=$!
  local i=0
  while [ "$i" -lt 200 ]; do
    if xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
      echo "display $DISPLAY at ${SCREEN}, no window manager"
      return 0
    fi
    sleep 0.1
    i=$((i + 1))
  done
  cat "$LOG/xvfb.log" >&2
  die "Xvfb $DISPLAY did not come up"
}

start_dbus() {
  local line
  DBUS_SESSION_BUS_ADDRESS=""
  DBUS_PID=""
  while IFS= read -r line; do
    case "$line" in
      unix:*) DBUS_SESSION_BUS_ADDRESS="$line" ;;
      *[!0-9]*) ;;
      ?*) DBUS_PID="$line" ;;
    esac
  done < <(dbus-daemon --session --fork --print-address=1 --print-pid=1)
  [ -n "$DBUS_SESSION_BUS_ADDRESS" ] || die "dbus-daemon gave no session address"
  export DBUS_SESSION_BUS_ADDRESS
}

port_is_free() {
  ! (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null
}

wait_port() {
  local i=0
  while [ "$i" -lt 300 ]; do
    if (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null; then return 0; fi
    sleep 0.1
    i=$((i + 1))
  done
  return 1
}

start_daemon() {
  port_is_free "$PORT" || die "something is already listening on 127.0.0.1:$PORT; refusing to measure against a daemon this run did not start"
  DAEMON_PID="$(spawn_group "$RUN/daemon.pid" "$LOG/daemon.log" "$BIN/vitrum-server" --port "$PORT")" \
    || die "vitrum-server did not start"
  wait_port "$PORT" || {
    cat "$LOG/daemon.log" >&2
    die "vitrum-server never bound 127.0.0.1:$PORT"
  }
  echo "daemon pid $DAEMON_PID on 127.0.0.1:$PORT"
}

# Every real vitrum window belonging to $1, with the decoy rejected.
#
# Each vitrum process maps TWO X windows: a 10x10 decoy at +10+10 whose WM_NAME
# is the binary's file name, and the real window, whose WM_NAME is exactly
# "vitrum". Resolving by name alone and taking the first hit returns the decoy
# and reads as "the app opened no window".
#
# The decoy test parses the geometry and compares it EXACTLY. Two substring
# forms were tried and both have a collision, which is the whole lesson here:
#
#   *10x10*              matches "810x102". A real 810x102 window is discarded.
#                        This is the form in tools/regression/screenshot.sh.
#   *"Geometry: 10x10"*  fixes that and still matches "Geometry: 10x100" and
#                        "Geometry: 10x1080". A narrow tall window is discarded.
#
# Both were measured rather than reasoned about, and the second was mine until
# a sibling agent found the identical hole in an unrelated substring matcher.
# Neither collision is reachable at the sizes vitrum uses today, which is
# exactly why a substring form survives review. Comparing the parsed value
# removes the class rather than the instance.
app_windows() {
  local pid="$1" w p geom
  for w in $(xdotool search --name '^vitrum$' 2>/dev/null || true); do
    p="$(xdotool getwindowpid "$w" 2>/dev/null || echo 0)"
    [ "$p" = "$pid" ] || continue
    geom="$(xdotool getwindowgeometry "$w" 2>/dev/null | awk '/Geometry:/ { print $2; exit }')"
    # An unreadable geometry keeps the window. `open_windows` asserts the count
    # as a backstop, and silently dropping a real window is the worse failure.
    if [ "$geom" = "$DECOY_GEOMETRY" ]; then
      continue
    fi
    printf '%s\n' "$w"
  done
  # Explicit, and not decoration. This function is called as
  # `n="$(app_windows "$pid" | wc -l)"` under `set -e -o pipefail`, so if it
  # ever ended on a non-zero status the assignment would fail and the run
  # would abort with no message at all. Zero windows is an answer, not an
  # error.
  return 0
}

# Wait for `$1` to have at least `$2` real windows.
#
# Returns 1 on timeout and 2 when the process is already gone. The distinction
# is worth the extra line: a binary that cannot find libwebkit2gtk exits in
# milliseconds, and without the liveness check the harness sits out the full
# timeout and then reports "the window never mapped", which sends you looking
# at the window manager instead of at the linker.
wait_windows() {
  local pid="$1" want="$2" secs="${3:-90}" i=0 n
  while [ "$i" -lt $((secs * 10)) ]; do
    n="$(app_windows "$pid" | wc -l)"
    [ "$n" -ge "$want" ] && return 0
    kill -0 "$pid" 2>/dev/null || return 2
    sleep 0.1
    i=$((i + 1))
  done
  return 1
}

# Create $1 sessions and print their ids, one per line.
create_sessions() {
  VITRUM_PORT="$PORT" python3 "$STAGE/sessions.py" "$1" "$RUN/cwd" "$SESSION_CMD" $SESSION_ARGS
}

# Open $1 windows, each showing its own session, in ONE vitrum process.
#
# The first launch takes the single-instance lock and becomes the process every
# later launch hands its request to; each later launch carries a
# vitrum://session/<id> URL, posts it to the holder, and exits. That is the
# real deep-link path, and it is why no --server flag appears here: a
# non-default --server forces --standalone, the handoff never happens, and you
# get N processes instead of N windows in one process. The whole 398 MB result
# depends on that distinction.
open_windows() {
  local want="$1"
  local ids=() id opened rc

  mapfile -t ids < <(create_sessions "$want")
  [ "${#ids[@]}" -eq "$want" ] || die "asked for $want sessions, the daemon created ${#ids[@]}"
  echo "sessions ${ids[0]}..${ids[-1]} running: $SESSION_CMD $SESSION_ARGS"

  APP_PID="$(spawn_group "$RUN/app.pid" "$LOG/app.log" \
    "$BIN/vitrum-app" --no-autostart "vitrum://session/${ids[0]}")" \
    || die "vitrum-app did not start"
  wait_windows "$APP_PID" 1 90 || {
    rc=$?
    tail -40 "$LOG/app.log" >&2
    [ "$rc" -eq 2 ] && die "vitrum-app exited before mapping its first window"
    die "the first window never mapped within 90s"
  }
  echo "app pid $APP_PID, window 1 of $want mapped"

  opened=1
  for id in "${ids[@]:1}"; do
    "$BIN/vitrum-app" --no-autostart "vitrum://session/$id" >>"$LOG/handoff.log" 2>&1 || true
    opened=$((opened + 1))
    wait_windows "$APP_PID" "$opened" 60 || {
      rc=$?
      tail -40 "$LOG/handoff.log" >&2
      [ "$rc" -eq 2 ] && die "vitrum-app exited while opening window $opened"
      die "window $opened never mapped within 60s"
    }
  done

  # Two counts, not one, and the second is the one with history behind it.
  #
  # The window count catches a handoff that silently did nothing. But it is
  # weaker than it reads: `wait_windows` has already established `have >= want`
  # by the time this runs, so on its own it only really catches an EXTRA
  # window. What it cannot catch at all is the failure GOAL.md already records:
  # a 1059.2 MB result taken with fewer sessions than windows, so several
  # windows showed the same session and the number was not the workload it
  # claimed. "Twenty windows" and "twenty windows each showing its own session"
  # are different measurements and only one of them is the target.
  #
  # Asking the daemon how many sessions it holds costs one message and closes
  # that. It does not prove each window is showing a DIFFERENT one, which needs
  # the client's own state and is named as unverified in harness/README.md.
  local have sessions
  have="$(app_windows "$APP_PID" | wc -l)"
  [ "$have" -eq "$want" ] || die "wanted $want windows, have $have"
  sessions="$(VITRUM_PORT="$PORT" python3 "$STAGE/sessions.py" count)"
  [ "$sessions" -eq "$want" ] \
    || die "$want windows but the daemon holds $sessions sessions; windows would be sharing sessions and the measurement would not be the stated workload"
  echo "windows $have of $want, all in pid $APP_PID, against $sessions sessions"
}

report_tree() {
  echo
  echo "process tree of $1"
  ps -o pid=,ppid=,rss=,comm= --ppid "$1" 2>/dev/null | sed 's/^/  /' || true
}

# The package a shared object comes from on Ubuntu 24.04, for the sixteen
# `readelf -d vitrum-app` names. Only the ones that can plausibly be absent on
# a server install are listed; glibc's own are never missing.
package_for_so() {
  case "$1" in
    libwebkit2gtk-4.1.so.0) echo libwebkit2gtk-4.1-0 ;;
    libjavascriptcoregtk-4.1.so.0) echo libjavascriptcoregtk-4.1-0 ;;
    libgtk-3.so.0 | libgdk-3.so.0) echo libgtk-3-0t64 ;;
    libsoup-3.0.so.0) echo libsoup-3.0-0 ;;
    libcairo.so.2) echo libcairo2 ;;
    libgdk_pixbuf-2.0.so.0) echo libgdk-pixbuf-2.0-0 ;;
    libgio-2.0.so.0 | libgobject-2.0.so.0 | libglib-2.0.so.0) echo libglib2.0-0t64 ;;
    libssl.so.3 | libcrypto.so.3) echo libssl3t64 ;;
    libxdo.so.3) echo libxdo3 ;;
    libgcc_s.so.1) echo libgcc-s1 ;;
    *) echo "" ;;
  esac
}

# Refuse before starting anything if this binary cannot load or the rig cannot
# drive it.
#
# `probe` answers "is this box set up". This answers the narrower and more
# useful question "will THIS binary run here", by asking the dynamic loader
# rather than the package database, so a library present under a name nobody
# predicted still counts as present. `ldd` sets LD_TRACE_LOADED_OBJECTS, which
# makes the loader resolve and print rather than run, so nothing here starts
# the application.
#
# Without this, a run with WebKitGTK missing gets as far as an X server, a
# daemon, twenty spawned shells and a settle timer before anything says why.
preflight() {
  local so pkg missing="" unknown="" tool tools_missing="" tool_packages=""

  while read -r so; do
    pkg="$(package_for_so "$so")"
    if [ -n "$pkg" ]; then
      case " $missing " in *" $pkg "*) ;; *) missing="$missing $pkg" ;; esac
    else
      unknown="$unknown $so"
    fi
  done < <(ldd "$BIN/vitrum-app" 2>/dev/null | awk '/not found/ {print $1}')

  for tool in $(tool_names); do
    if ! command -v "$tool" >/dev/null 2>&1; then
      tools_missing="$tools_missing $tool"
      pkg="$(package_for_tool "$tool")"
      case " $tool_packages " in
        *" $pkg "*) ;;
        *) tool_packages="$tool_packages $pkg" ;;
      esac
    fi
  done

  [ -n "$missing$unknown$tools_missing" ] || return 0

  echo "This host cannot run the measurement yet." >&2
  if [ -n "$missing" ]; then
    echo >&2
    echo "vitrum-app is missing shared libraries. Install them with, and nothing else:" >&2
    echo "  sudo apt-get update && sudo apt-get install -y$missing" >&2
  fi
  if [ -n "$unknown" ]; then
    echo >&2
    echo "These are also unresolved and this script has no package name for them." >&2
    echo "Find it with: apt-file search <name>" >&2
    printf '  %s\n' $unknown >&2
  fi
  if [ -n "$tools_missing" ]; then
    echo >&2
    echo "The harness itself is missing:$tools_missing" >&2
    # Derived from what is actually absent, not a blanket list. An earlier
    # version printed every tool package unconditionally, six for two missing
    # tools, and named `dbus-daemon`, which is not a package. `cmd_probe` had
    # that exact bug fixed and this path did not, which is what comes of
    # fixing a defect in one place without grepping for its twin. Found by
    # running preflight against a host with the TOOLS hidden rather than the
    # libraries, a branch every earlier test had left unexercised.
    echo "  sudo apt-get install -y$tool_packages" >&2
  fi
  echo >&2
  echo "Run 'harness/run.sh probe' for the full report." >&2
  exit 4
}

# ---------------------------------------------------------------------------
# probe
# ---------------------------------------------------------------------------

REQUIRED_PACKAGES="libwebkit2gtk-4.1-0 libjavascriptcoregtk-4.1-0 libgtk-3-0t64 libsoup-3.0-0 libcairo2 libgdk-pixbuf-2.0-0 libglib2.0-0t64 libssl3t64 libxdo3 libgl1-mesa-dri"
REQUIRED_TOOL_PACKAGES="xvfb x11-utils xdotool imagemagick procps python3 rsync util-linux"
FIDELITY_PACKAGES="fonts-ubuntu fonts-cantarell fonts-noto-core fonts-jetbrains-mono fonts-liberation fonts-dejavu-core"
# Every tool the rig drives, paired with the package that provides it, as ONE
# list. It used to be three: the loop in `preflight`, the loop in `cmd_probe`,
# and a `case` mapping names to packages, each hand-kept and each free to fall
# behind the others. A tool added to one loop and missing from the mapping fell
# through to a `*)` branch that echoed the BINARY name, so the verdict would
# have told somebody to run `apt-get install -y import`. That is the same
# defect this file already fixed once; a second copy of a list is how it came
# back. One list cannot disagree with itself.
RIG_TOOLS="Xvfb:xvfb xdotool:xdotool import:imagemagick identify:imagemagick xdpyinfo:x11-utils python3:python3 flock:util-linux setsid:util-linux rsync:rsync dbus-daemon:dbus"

tool_names() {
  local pair
  for pair in $RIG_TOOLS; do printf '%s\n' "${pair%%:*}"; done
}

package_for_tool() {
  local pair
  for pair in $RIG_TOOLS; do
    [ "${pair%%:*}" = "$1" ] && { printf '%s\n' "${pair#*:}"; return 0; }
  done
  # Not in the list at all. Name it rather than guess a package from it.
  printf '%s\n' "UNKNOWN-PACKAGE-FOR-$1"
}

# Append a package to `missing` unless it is already there. Without this a box
# lacking xdotool the package AND xdotool the binary is told to install it
# twice in one command line.
note_missing() {
  case " $missing " in
    *" $1 "*) ;;
    *) missing="$missing $1" ;;
  esac
}

cmd_probe() {
  local missing="" missing_fidelity="" p v scope

  echo "host $(hostname)"
  if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    echo "distro $PRETTY_NAME ($ID $VERSION_ID)"
  fi
  echo "kernel $(uname -srm)"
  echo "glibc $(ldd --version | head -1 | sed 's/^ldd //')"
  echo "cpu $(nproc) threads, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2- | sed 's/^ //')"
  echo "memory $(free -m | awk '/^Mem:/ {print $2 " MB total, " $7 " MB available"}')"
  echo "load $(cut -d' ' -f1-3 /proc/loadavg)"

  echo
  echo "graphics"
  if lspci 2>/dev/null | grep -Eiq 'vga|3d controller|display controller'; then
    lspci 2>/dev/null | grep -Ei 'vga|3d controller|display controller' | sed 's/^/  /' || true
  else
    echo "  no VGA, 3D or display controller on the PCI bus"
  fi
  if [ -d /dev/dri ]; then
    echo "  /dev/dri: $(ls /dev/dri | tr '\n' ' ')"
  else
    echo "  /dev/dri absent, so no direct rendering device"
  fi
  # `|| true` is load-bearing. On santhserver nvidia-smi exits non-zero with
  # "Failed to initialize NVML: Driver/library version mismatch", and under
  # `set -e -o pipefail` that aborted the whole probe halfway through the
  # graphics section, before the verdict. A broken GPU driver is something the
  # probe should REPORT, not something that stops it reporting.
  if command -v nvidia-smi >/dev/null 2>&1; then
    nvidia-smi -L 2>&1 | sed 's/^/  /' || true
  fi

  echo
  echo "display"
  echo "  session type ${XDG_SESSION_TYPE:-none}, DISPLAY ${DISPLAY:-unset}"
  echo "  default systemd target $(systemctl get-default 2>/dev/null || echo unknown)"
  echo "  display managers: $(systemctl is-active gdm3 lightdm sddm 2>/dev/null | tr '\n' ' ')"
  if pgrep -a Xvfb >/dev/null 2>&1; then
    pgrep -a Xvfb | sed 's/^/  running Xvfb: /' || true
  else
    echo "  no Xvfb running"
  fi
  if pgrep -a Xorg >/dev/null 2>&1; then
    pgrep -a Xorg | sed 's/^/  running Xorg: /' || true
  else
    echo "  no Xorg running"
  fi

  echo
  echo "runtime the release binary links against"
  for p in $REQUIRED_PACKAGES; do
    v="$(dpkg-query -W -f='${Version}|${Status}' "$p" 2>/dev/null || true)"
    case "$v" in
      *"install ok installed"*) printf '  present  %-30s %s\n' "$p" "${v%%|*}" ;;
      *) printf '  MISSING  %s\n' "$p"; note_missing "$p" ;;
    esac
  done

  echo
  echo "tools the harness drives it with"
  for p in $REQUIRED_TOOL_PACKAGES; do
    v="$(dpkg-query -W -f='${Version}|${Status}' "$p" 2>/dev/null || true)"
    case "$v" in
      *"install ok installed"*) printf '  present  %-30s %s\n' "$p" "${v%%|*}" ;;
      *) printf '  MISSING  %s\n' "$p"; note_missing "$p" ;;
    esac
  done
  for p in $(tool_names); do
    if command -v "$p" >/dev/null 2>&1; then
      printf '  present  %-30s %s\n' "$p" "$(command -v "$p")"
    else
      printf '  MISSING  %s (no binary on PATH, from %s)\n' "$p" "$(package_for_tool "$p")"
      note_missing "$(package_for_tool "$p")"
    fi
  done

  echo
  echo "fonts the two CSS stacks can land on"
  for p in Ubuntu Cantarell "Noto Sans" "DejaVu Sans" "JetBrains Mono" "Liberation Mono" "DejaVu Sans Mono"; do
    printf '  %-18s -> %s\n' "$p" "$(fc-match "$p" 2>/dev/null || echo 'fc-match missing')"
  done
  for p in $FIDELITY_PACKAGES; do
    v="$(dpkg-query -W -f='${Status}' "$p" 2>/dev/null || true)"
    case "$v" in
      *"install ok installed"*) ;;
      *) missing_fidelity="$missing_fidelity $p" ;;
    esac
  done

  echo
  echo "foreground probe"
  # This decides whether the product can answer "is this session blocked on the
  # operator" AT ALL, so it belongs in the probe next to the packages.
  #
  # vitrum reads /proc/<pid>/syscall of the PTY's foreground process group
  # (crates/vitrum-core/src/probe.rs:147). That file is ptrace-gated. Under
  # kernel.yama.ptrace_scope 0 or 1 a process may read it for its own
  # descendants, which every session is. Under 2 only root may, so the read
  # fails, the probe correctly returns None, and every session's waiting state
  # is UNKNOWN rather than wrong.
  #
  # Measured: this is the difference between the development desktop, which is
  # 1, and axiomexec, which is 2. It makes 13 vitrum-core tests fail there that
  # pass here, and it would make a screenshot taken there show no
  # operator-waiting state at all. That is not a regression in the product, it
  # is the host refusing to answer, and the two look identical in a capture.
  scope="$(sysctl -n kernel.yama.ptrace_scope 2>/dev/null || echo unavailable)"
  echo "  kernel.yama.ptrace_scope $scope"
  case "$scope" in
    0 | 1)
      echo "  a session's foreground state is determinable here, as on the development desktop"
      ;;
    unavailable)
      echo "  no Yama LSM; /proc/<pid>/syscall is readable for own descendants"
      ;;
    *)
      echo "  DEGRADED: /proc/<pid>/syscall is unreadable for a child, so every"
      echo "  session's waiting state reports UNKNOWN. The product is behaving"
      echo "  correctly; the host cannot answer. Any screenshot or status claim"
      echo "  taken here is missing the operator-waiting state entirely."
      echo "  Match the development desktop with: sudo sysctl -w kernel.yama.ptrace_scope=1"
      ;;
  esac

  echo
  echo "sandbox"
  if bwrap --ro-bind / / --dev /dev --unshare-user --unshare-pid /bin/true 2>/dev/null; then
    echo "  bubblewrap can create a user namespace here"
  else
    echo "  bubblewrap cannot create a user namespace (apparmor_restrict_unprivileged_userns=$(sysctl -n kernel.apparmor_restrict_unprivileged_userns 2>/dev/null || echo unknown))"
    echo "  WebKitGTK falls back to running its web process unsandboxed, exactly as it does on the development desktop, which has the same setting"
  fi

  echo
  echo "daemon port"
  if port_is_free "$PORT"; then
    echo "  127.0.0.1:$PORT is free"
  else
    echo "  127.0.0.1:$PORT is IN USE; a measurement run would refuse to start"
    ss -ltnp 2>/dev/null | grep ":$PORT" | sed 's/^/  /' || true
  fi

  echo
  if [ -n "$missing" ]; then
    echo "VERDICT: not ready. Missing:$missing"
    echo
    echo "Install them with, and nothing else:"
    echo "  sudo apt-get update && sudo apt-get install -y$missing"
  else
    echo "VERDICT: ready to measure."
  fi
  if [ -n "$missing_fidelity" ]; then
    echo
    echo "Not required, but every font in the UI stack currently resolves to a"
    echo "fallback, so text metrics here will not match the development desktop:"
    echo "  sudo apt-get install -y$missing_fidelity"
  fi
  [ -z "$missing" ] || exit 4
}

# ---------------------------------------------------------------------------
# screenshot
# ---------------------------------------------------------------------------

cmd_screenshot() {
  local name="${1:-}" geom="${2:-1382x800}" scale="${3:-auto}"
  [ -n "$name" ] || die "screenshot needs a name"
  local w="${geom%x*}" h="${geom#*x}" rc
  local png="$OUT/$name.png"
  local -a scale_args=()
  [ "$scale" = "auto" ] || scale_args=(--ui-scale "$scale")

  start_x
  start_dbus
  start_daemon

  local id
  id="$(create_sessions 1)"
  echo "session $id running: $SESSION_CMD $SESSION_ARGS"

  # --standalone because this run photographs ITS OWN window. Without it a
  # second launch would hand off to any vitrum already holding the lock for
  # this profile and this run would capture that instance instead.
  APP_PID="$(spawn_group "$RUN/app.pid" "$LOG/app.log" \
    "$BIN/vitrum-app" --standalone --no-autostart "${scale_args[@]}" "vitrum://session/$id")" \
    || die "vitrum-app did not start"

  wait_windows "$APP_PID" 1 90 || {
    rc=$?
    tail -40 "$LOG/app.log" >&2
    [ "$rc" -eq 2 ] && die "vitrum-app exited before mapping a window"
    die "no window for pid $APP_PID within 90s"
  }
  local win
  win="$(app_windows "$APP_PID" | head -1)"
  echo "app pid $APP_PID, window $win"

  # The window maps several seconds before the first daemon snapshot lands. A
  # frame taken in that gap shows an empty sidebar, which looks exactly like a
  # bug and is not one.
  sleep "$STARTUP"
  xdotool windowsize "$win" "$w" "$h" || true
  xdotool windowmove "$win" 0 0 || true
  # Park the pointer so no row is left hovered, which would otherwise differ
  # between two runs of the same command.
  xdotool mousemove 1 1 || true
  sleep 2

  echo "requested ui scale $scale"
  grep -o 'monitor .*ui scale [0-9.]*' "$LOG/app.log" | tail -1 || true

  import -window "$win" "$png" || die "import could not capture window $win"

  local mean sd colours
  mean="$(identify -format '%[fx:mean]' "$png")" || die "identify could not read $png"
  sd="$(identify -format '%[fx:standard_deviation]' "$png")" || die "identify could not read $png"
  colours="$(identify -format '%k' "$png")" || die "identify could not read $png"
  echo "capture $png"
  echo "  size $(identify -format '%wx%h' "$png")"
  echo "  mean $mean, standard deviation $sd, unique colours $colours"
  # The test is the number of distinct colours, not the deviation, and the
  # reason is an escape hunt rather than taste.
  #
  # An occluded window comes back pure white from `import` and exits 0; a
  # window that never painted comes back its bare background. Both are uniform,
  # so a deviation test catches BOTH of them and looks sufficient. It is not.
  # Measured on 1382x800 frames: a background with one 1px line across it reads
  # sd 0.033, and a background with a 2px seam reads sd 0.0017, so both clear a
  # 0.001 threshold by wide margins while having painted essentially nothing.
  # A real interface reads sd 0.198. The deviation only ever proved "not
  # perfectly uniform", which is a much weaker claim than the comment made.
  #
  # Distinct colours separate them cleanly: 1 for either blank frame, 2 and 3
  # for those two near-blank escapes, 800 for a gradient, and any real render
  # of text, borders and pills is in the thousands because of antialiasing.
  # Sixteen is far above every near-blank case measured and far below anything
  # the application can actually draw.
  #
  # It also sidesteps a fragility the deviation had: `identify` returns `-nan`
  # for a pure white frame at this size, and what `awk` does with that string
  # is not something a guard should rest on. A colour count is an integer.
  [ "$colours" -ge 16 ] \
    || die "the capture has only $colours distinct colours; the window painted nothing meaningful"
}

# ---------------------------------------------------------------------------
# memory
# ---------------------------------------------------------------------------

cmd_memory() {
  local windows="${1:-}"
  case "$windows" in
    '' | *[!0-9]*) die "memory needs a window count" ;;
  esac
  [ "$windows" -ge 1 ] || die "memory needs at least one window"

  start_x
  start_dbus
  start_daemon
  open_windows "$windows"

  echo "settling for ${SETTLE}s"
  sleep "$SETTLE"

  echo
  echo "client tree, pss"
  python3 "$STAGE/measure.py" pss "$APP_PID"
  echo
  echo "daemon tree, pss"
  python3 "$STAGE/measure.py" pss "$DAEMON_PID"
  report_tree "$APP_PID"
}

# ---------------------------------------------------------------------------
# idle-cpu
# ---------------------------------------------------------------------------

cmd_idle_cpu() {
  local seconds="${1:-}" windows="${2:-1}"
  case "$seconds" in
    '' | *[!0-9]*) die "idle-cpu needs a whole number of seconds" ;;
  esac
  case "$windows" in
    *[!0-9]*) die "the window count must be a whole number" ;;
  esac
  [ "$seconds" -ge 1 ] || die "idle-cpu needs at least one second"

  start_x
  start_dbus
  start_daemon
  open_windows "$windows"

  # Nothing touches the display from here on. The pointer is parked once, off
  # every row, because a cursor resting on a session card leaves it in a hover
  # state and a hover transition is work.
  xdotool mousemove 1 1 || true
  echo "settling for ${SETTLE}s before the window opens"
  sleep "$SETTLE"

  echo
  echo "client tree, cpu over ${seconds}s with $windows window(s) idle"
  python3 "$STAGE/measure.py" cpu "$APP_PID" "$seconds"
  echo
  echo "daemon tree, cpu over the same kind of window"
  python3 "$STAGE/measure.py" cpu "$DAEMON_PID" 5
}

# ---------------------------------------------------------------------------

mkdir -p "$OUT" "$LOG" "$RUN/cwd"

# A clean profile, per run, and never the invoking user's.
#
# Every measurement this project has had to throw away shared one cause:
# persisted state from an earlier run. A saved sidebar width means no measured
# width is the layout's; a saved workspace-bar flag renders a full-width band
# over a one-entry switcher. XDG_RUNTIME_DIR matters just as much, because the
# single-instance lock and socket live there and two runs sharing them would
# hand each other windows.
export XDG_CONFIG_HOME="$RUN/config"
export XDG_STATE_HOME="$RUN/state"
export XDG_DATA_HOME="$RUN/data"
export XDG_CACHE_HOME="$RUN/cache"
export XDG_RUNTIME_DIR="$RUN/run"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

if [ "$COMMAND" != "probe" ]; then
  [ -x "$BIN/vitrum-app" ] || die "no vitrum-app at $BIN; run.sh stages it"
  [ -x "$BIN/vitrum-server" ] || die "no vitrum-server at $BIN; run.sh stages it"
  preflight
  # One measurement at a time. The client only reaches a daemon on the default
  # port, because a non-default --server forces --standalone and breaks the
  # window handoff, so two concurrent runs would fight over 127.0.0.1:7737 and
  # each would measure a mixture of the two.
  exec 9>/tmp/vitrum-harness.lock
  flock -n 9 || die "another harness run holds /tmp/vitrum-harness.lock"
  trap cleanup EXIT INT TERM
  echo "run $RUN_ID in $RUN"
fi

case "$COMMAND" in
  probe) cmd_probe ;;
  screenshot) cmd_screenshot "$@" ;;
  memory) cmd_memory "$@" ;;
  idle-cpu) cmd_idle_cpu "$@" ;;
  *) die "unknown command $COMMAND" ;;
esac
