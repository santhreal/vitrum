#!/bin/bash
# Bring the whole capture stack up on a headless host, or take it down again.
#
# usage:
#   harness/screenshots/rig.sh all  [<display-num>]
#   harness/screenshots/rig.sh up   <display-num> <WxH> [<ui-scale>]
#   harness/screenshots/rig.sh shots <display-num>
#   harness/screenshots/rig.sh down <display-num>
#
# `all` is the whole job in one command: it brings the stack up, takes the
# three pictures the README shows, writes them into the repository at the
# paths the README names, and takes the stack down again.
#
# `up` starts an Xvfb of its own, a daemon of its own, eight agent sessions
# and one client window, and prints the window id. `shots` drives that window
# and writes the pictures. `down` kills exactly what `up` started and nothing
# else.
#
# A display belongs to whoever created it. `up` refuses a display number that
# already answers rather than clearing it, because the same number on a shared
# host is somebody else's session and `pkill -f "Xvfb :N"` takes it down.
#
# What the run needs on the host:
#
#   /src writable, or already holding the project directories below
#   Xvfb, xdotool, import (ImageMagick), python3 with numpy and Pillow
#   a release build of `vitrum` and `vitrum-server` in $VITRUM_BIN
#
# The project directories are real repositories with real branches, created
# here if they are missing, because the daemon reads the branch off the
# directory the session runs in. They are named `/src/<project>`: nothing
# published from this repository shows a path from the machine that produced
# it, so the capture host grows the synthetic tree rather than the shots
# growing a home directory.
set -u

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$HERE/../.." && pwd)
VITRUM_BIN=${VITRUM_BIN:-$ROOT/target/release}
STAGE=${STAGE:-$HOME/.cache/vitrum-shots}
PORT=${PORT:-7791}
# Session id to open in the window, from `sessions.tsv`. Empty leaves the pane
# on the empty state, which is what a shot of the sidebar alone wants.
OPEN=${OPEN:-}

# project root -> branch. A worktree of the first, so one row carries a branch
# that is not the project's own.
declare -A BRANCHES=(
  [/src/vitrum]=main
  [/src/veyyon]=theme-contrast
  [/src/keyhog]=main
  [/src/worktrees/hint-parser]=hint-parser
)
SUBDIRS="/src/vitrum/crates/vitrum-core /src/veyyon/crates/tui /src/keyhog/fuzz /src/keyhog/docs"

die() { echo "rig.sh: $*" >&2; exit 1; }

make_tree() {
  [ -w /src ] || [ -d /src/vitrum ] || die "/src is not writable and holds no projects"
  for root in "${!BRANCHES[@]}"; do
    branch=${BRANCHES[$root]}
    if [ ! -d "$root/.git" ]; then
      mkdir -p "$root" || die "cannot create $root"
      ( cd "$root" \
        && git init -q -b "$branch" . \
        && git -c user.email=rig@invalid -c user.name=rig commit -q --allow-empty -m "staged" \
      ) || die "cannot make a repository at $root"
    fi
    ( cd "$root" && git rev-parse --verify -q "$branch" >/dev/null \
      || git -c user.email=rig@invalid -c user.name=rig branch "$branch" >/dev/null 2>&1
      git symbolic-ref HEAD "refs/heads/$branch" ) || die "cannot put $root on $branch"
  done
  for dir in $SUBDIRS; do mkdir -p "$dir" || die "cannot create $dir"; done
}

make_bin() {
  # One wrapper per agent this build knows. The command name is what resolves
  # the provider mark on the row, so it has to be exactly the agent's, and the
  # transcript is resolved from the directory rather than from an argument:
  # the launcher lists a running session by its command line, and a flag
  # naming a fixture would be on the front page.
  mkdir -p "$STAGE/bin"
  python3 "$HERE/stage.py" --table > "$STAGE/roles.tsv" || die "cannot write the role table"
  for agent in claude codex gemini opencode veyyon; do
    cat > "$STAGE/bin/$agent" <<EOF
#!/bin/bash
role=\$(awk -F'\t' -v d="\$PWD" '\$1 == d { print \$2 }' "$STAGE/roles.tsv")
exec python3 "$HERE/agent.py" --role "\${role:-working-core}"
EOF
    chmod +x "$STAGE/bin/$agent"
  done
}

up() {
  DNUM=$1; GEOM=$2; SCALE=${3:-1}
  W=${GEOM%x*}; H=${GEOM#*x}
  [ -x "$VITRUM_BIN/vitrum" ] || die "no vitrum at $VITRUM_BIN"
  [ -x "$VITRUM_BIN/vitrum-server" ] || die "no vitrum-server at $VITRUM_BIN"
  xdpyinfo -display ":$DNUM" >/dev/null 2>&1 && die "display :$DNUM already answers; it is not yours"

  rm -rf "$STAGE/cfg" "$STAGE/state" "$STAGE/data" "$STAGE/cache" "$STAGE/run"
  mkdir -p "$STAGE/cfg" "$STAGE/state" "$STAGE/data" "$STAGE/cache" "$STAGE/run" "$STAGE/log"
  chmod 700 "$STAGE/run"
  make_tree
  make_bin

  # A profile that has been used before.
  #
  # The stage is wiped for every run, and a wiped profile is a first run: the
  # onboarding sheet opens over the window, takes the keyboard, and every
  # chord and click the capture sends afterwards goes to it. Three pictures of
  # the same welcome sheet is what that produces.
  #
  # It is also the wrong picture. The README shows a machine with eight agents
  # on it, and nobody with eight agents running is on their first launch. So
  # the two facts the product remembers about the operator are written before
  # the client starts: the sheet has been read, and so have the notes for the
  # version that is about to run.
  VERSION=$("$VITRUM_BIN/vitrum" --version 2>/dev/null | awk '{print $NF}')
  mkdir -p "$STAGE/cfg/vitrum"
  cat > "$STAGE/cfg/vitrum/ui.json" <<JSON
{"version":1,"settings":{"onboarded":true,"seenVersion":"$VERSION"}}
JSON

  export XDG_CONFIG_HOME=$STAGE/cfg XDG_STATE_HOME=$STAGE/state \
         XDG_DATA_HOME=$STAGE/data XDG_CACHE_HOME=$STAGE/cache \
         XDG_RUNTIME_DIR=$STAGE/run
  export PATH=$STAGE/bin:$PATH
  export VITRUM_PORT=$PORT
  # The capture host has no GPU: llvmpipe under Xvfb, and DRI3 unavailable.
  # The chrome is GTK and renders through Cairo on the CPU, which needs
  # nothing from a driver. The pane's own surface is Vulkan through lavapipe,
  # which does work headless and is what `--software` means here.
  export LIBGL_ALWAYS_SOFTWARE=1 GDK_BACKEND=x11

  Xvfb ":$DNUM" -screen 0 "${W}x${H}x24" -dpi 96 > "$STAGE/log/xvfb.log" 2>&1 &
  echo $! > "$STAGE/run/xvfb.pid"
  for _ in $(seq 100); do xdpyinfo -display ":$DNUM" >/dev/null 2>&1 && break; sleep 0.1; done
  xdpyinfo -display ":$DNUM" >/dev/null 2>&1 || die "Xvfb :$DNUM did not start"

  # A window manager on the display THIS RUN created, and only there. Without
  # one there is no input focus and no window ever receives a click, so a run
  # that has to open the launcher or the settings sheet photographs a window
  # it cannot drive.
  #
  # It draws no frame. The window draws its own titlebar, so a second one from
  # the window manager is foreign chrome in the middle of the picture, and its
  # buttons sit exactly where a click aimed at the window lands: a click meant
  # for the window iconified it, and a click on the frame dragged it off the
  # left edge of the screen.
  if command -v openbox >/dev/null; then
    mkdir -p "$STAGE/cfg/openbox"
    cat > "$STAGE/cfg/openbox/rc.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<openbox_config xmlns="http://openbox.org/3.4/rc">
  <applications>
    <application class="*">
      <decor>no</decor>
      <position force="yes"><x>0</x><y>0</y></position>
    </application>
  </applications>
</openbox_config>
XML
    DISPLAY=":$DNUM" openbox --config-file "$STAGE/cfg/openbox/rc.xml" \
      > "$STAGE/log/wm.log" 2>&1 &
    echo $! > "$STAGE/run/wm.pid"
    sleep 1
  fi

  "$VITRUM_BIN/vitrum-server" --port "$PORT" > "$STAGE/log/daemon.log" 2>&1 &
  echo $! > "$STAGE/run/daemon.pid"
  for _ in $(seq 100); do [ -s "$STAGE/run/vitrum/token" ] && break; sleep 0.1; done
  [ -s "$STAGE/run/vitrum/token" ] || die "the daemon never wrote a token"

  python3 "$HERE/stage.py" > "$STAGE/sessions.tsv" || die "the daemon refused the session set"

  # `VITRUM_LOG=debug` for one line only: the pane prints the grid it handed
  # the child on every resize, and that number is the pane's own claim about
  # how many rows it is showing. `shots` reads it back and holds the picture
  # to it, which is the only way a dead band under the last row is
  # distinguishable from a screen that simply has empty rows at the bottom.
  DISPLAY=":$DNUM" VITRUM_LOG=${VITRUM_LOG:-debug} "$VITRUM_BIN/vitrum" --standalone --no-autostart \
    --server "ws://127.0.0.1:$PORT" --ui-scale "$SCALE" \
    ${OPEN:+"vitrum://session/$OPEN"} \
    > "$STAGE/log/app.log" 2>&1 &
  APID=$!
  echo $APID > "$STAGE/run/app.pid"

  # Resolve by pid and reject the 10x10 decoy every vitrum process maps: a
  # search by name matches both windows and the decoy is the one that answers
  # first.
  WIN=""
  for _ in $(seq 600); do
    kill -0 $APID 2>/dev/null || { tail -20 "$STAGE/log/app.log" >&2; die "the client exited"; }
    for id in $(DISPLAY=":$DNUM" xdotool search --pid $APID 2>/dev/null); do
      geo=$(DISPLAY=":$DNUM" xdotool getwindowgeometry "$id" 2>/dev/null | awk '/Geometry/ {print $2}')
      case "$geo" in ""|10x10) continue;; esac
      WIN=$id
    done
    [ -n "$WIN" ] && break
    sleep 0.1
  done
  [ -n "$WIN" ] || die "no window for pid $APID"

  sleep 8
  DISPLAY=":$DNUM" xdotool windowsize "$WIN" "$W" "$H"
  DISPLAY=":$DNUM" xdotool windowmove "$WIN" 0 0
  DISPLAY=":$DNUM" xdotool mousemove 1 1
  sleep 3
  echo "$WIN" > "$STAGE/run/window"
  echo "$W $H" > "$STAGE/run/size"
  echo "window $WIN on :$DNUM"
}

down() {
  DNUM=$1
  for name in app daemon wm xvfb; do
    pid=$(cat "$STAGE/run/$name.pid" 2>/dev/null || true)
    [ -n "${pid:-}" ] && kill "$pid" 2>/dev/null
  done
  sleep 1
  for name in app daemon wm xvfb; do
    pid=$(cat "$STAGE/run/$name.pid" 2>/dev/null || true)
    [ -n "${pid:-}" ] && kill -9 "$pid" 2>/dev/null
    rm -f "$STAGE/run/$name.pid"
  done
  xdpyinfo -display ":$DNUM" >/dev/null 2>&1 && echo "warning: :$DNUM still answers" >&2
  echo "down"
}

# The three pictures the README shows, in the order it shows them, written to
# the paths it names. Every interaction here is a chord or a coordinate, and
# both are recorded rather than hunted for, so a rerun after the shell changes
# is one command and, at worst, one number.
#
# Captures read the root window, not the client window. `import -window <id>`
# on a window the server never composited returns the area as it is on screen,
# which is white wherever anything overlaps it; the client is sized to the
# whole screen at 0,0, so the root IS the window and nothing can occlude it.
#
# The pointer is parked at 1,1 before every capture. A pointer resting on a
# session row leaves that row hovered, and a hover state in a still picture
# reads as a selection that is not there.
#
# GEAR is the settings button, as `x,y` inside the window. Either number may
# be negative, and then it counts from the right or the bottom edge instead,
# which is what holds when the window is resized. The button sits at the foot
# of the sidebar, beside the collapse arrow, so x is measured from the left
# and y from the bottom.
GEAR=${GEAR:-196,-25}
APPEARANCE_AT=${APPEARANCE_AT:-}
# ImageMagick geometry applied to every capture, e.g. 1600x760+0+0 to drop the
# empty bottom of a tall window. Empty keeps the whole frame.
CROP=${CROP:-}
OUT=${OUT:-$ROOT/assets/screenshots}

# Absolute geometry of the client window, as `WxH+X+Y`. Read fresh for every
# capture: a window manager is free to place and to resize, and a capture that
# assumes the size that was asked for photographs the desktop around it.
geometry() {
  DISPLAY=":$DNUM" xdotool getwindowgeometry "$WIN" | awk '
    /Position/ { split($2, p, ","); x = p[1]; y = p[2] }
    /Geometry/ { g = $2 }
    END { print g "+" x "+" y }'
}

# ImageMagick 6 ships `convert`, 7 ships `magick`, and the capture host has
# whichever it has.
crop_to() {
  if command -v magick >/dev/null; then
    magick "$1" -crop "$2" +repage "$1"
  else
    convert "$1" -crop "$2" +repage "$1"
  fi
}

# Put the window back where the capture expects it. A click that the window
# does not consume reaches the window manager, and a click on a titlebar the
# window manager did not draw is a move: one interaction is enough to leave
# the frame hanging off the left edge, and the picture after it is of the
# desktop. Asserted before every capture rather than once at the start.
#
# The request is asynchronous: the window manager answers a ConfigureRequest
# when it gets to it, and a capture taken on a fixed sleep after the request
# photographs the window at its old rectangle. So the rectangle is read back
# until it is the one that was asked for.
place() {
  want="${TW}x${TH}+0+0"
  for _ in $(seq 40); do
    [ "$(geometry)" = "$want" ] && return 0
    DISPLAY=":$DNUM" xdotool windowsize "$WIN" "$TW" "$TH"
    DISPLAY=":$DNUM" xdotool windowmove "$WIN" 0 0
    sleep 0.25
  done
  die "the window would not sit at $want; it is at $(geometry)"
}

# Xvfb keeps no backing store, so whatever part of a window has been off the
# screen holds nothing when it comes back and captures as black. `xrefresh`
# exposes every window on the display, which is the redraw that fills it in.
repaint() {
  DISPLAY=":$DNUM" xrefresh 2>/dev/null
  sleep 2
}

# Click a point given as `x,y` inside the window, in the window's own
# coordinates. A negative number counts from the right or the bottom edge, so
# a control anchored to a corner keeps its offset when the window is resized.
# The pointer is driven in root coordinates, so the window's origin is added.
click_in() {
  cx=${1%,*}; cy=${1#*,}
  case "$cx" in -*) cx=$(( W + cx ));; esac
  case "$cy" in -*) cy=$(( H + cy ));; esac
  DISPLAY=":$DNUM" xdotool mousemove $(( WX + cx )) $(( WY + cy )) click 1
}

shot() {
  name=$1
  place
  repaint
  DISPLAY=":$DNUM" xdotool mousemove 1 1
  sleep 1
  raw=$STAGE/run/$name.png
  DISPLAY=":$DNUM" import -window root "$raw" || die "capture of $name failed"
  crop_to "$raw" "$(geometry)" || die "crop of $name to the window failed"
  if [ -n "$CROP" ]; then
    crop_to "$raw" "$CROP" || die "crop of $name failed"
  fi
  mkdir -p "$OUT"
  cp "$raw" "$OUT/$name.png"
  echo "$OUT/$name.png"
}

# Send a chord to the window.
#
# To the FOCUSED window, not to a window id. `xdotool key --window` forges a
# KeyPress and sends it with XSendEvent, and GTK reads the send_event flag and
# ignores it: a synthetic key is how a screen recorder replays somebody else's
# session. Focus first and let XTEST drive the real keyboard, which is a press
# the toolkit cannot tell from a finger.
chord() {
  DISPLAY=":$DNUM" xdotool windowactivate --sync "$WIN" 2>/dev/null
  DISPLAY=":$DNUM" xdotool key --clearmodifiers "$1"
}

shots() {
  DNUM=$1
  WIN=$(cat "$STAGE/run/window" 2>/dev/null) || die "no window; run 'up' first"
  [ -n "$WIN" ] || die "no window; run 'up' first"
  read -r TW TH < "$STAGE/run/size" || die "no size; run 'up' first"
  DISPLAY=":$DNUM" xdotool windowactivate --sync "$WIN" 2>/dev/null
  place
  g=$(geometry)
  wh=${g%%+*}; rest=${g#*+}
  W=${wh%x*}; WX=${rest%+*}; WY=${rest#*+}

  # 1. The hero. `up` was given the session to open, so the pane already holds
  #    the transcript of the agent that is waiting for an approval and the
  #    sidebar already holds the other seven.
  hero=$(shot hero-sidebar-five-states) || exit 1
  echo "$hero"
  # Read the pane back out of the picture that was just taken, and hold it to
  # the grid the pane says it handed the child. The window is sized by the
  # caller, so a run at an awkward geometry is the case that catches a pane
  # leaving a row unpainted, and the capture is the only place that band is
  # visible at all. The claim comes out of the app's own log rather than out
  # of the picture: reading it off the status bar would need OCR, and a
  # measurement that agrees with itself proves nothing.
  claimed=$(sed -n 's/.*pane resized to \([0-9]*x[0-9]*\).*/\1/p' "$STAGE/log/app.log" | tail -1)
  [ -n "$claimed" ] || die "the app never logged a pane resize; is VITRUM_LOG=debug set?"
  python3 "$HERE/measure.py" "$hero" "$claimed" \
    || die "the pane's geometry is wrong in $hero"

  # 2. The launcher, over that same sidebar.
  chord ctrl+shift+n
  sleep 3
  shot launcher
  chord Escape
  sleep 2

  # 3. Settings, on Appearance, over that same sidebar. The pointer is driven
  #    in root coordinates, so the window's origin is read again, because
  #    `place` has run since the last read, and added to the offset.
  place
  g=$(geometry)
  wh=${g%%+*}; rest=${g#*+}
  W=${wh%x*}; H=${wh#*x}; WX=${rest%+*}; WY=${rest#*+}
  click_in "$GEAR"
  sleep 3
  if [ -n "$APPEARANCE_AT" ]; then
    click_in "$APPEARANCE_AT"
    sleep 2
  fi
  shot settings-appearance
  chord Escape
}

all() {
  DNUM=${1:-41}
  # The session that is stopped on an approval prompt, so the hero's pane
  # holds the transcript the alt text describes. `stage.py --table` names it.
  OPEN=${OPEN:-2}
  up "$DNUM" "${GEOMETRY:-1600x1000}" "${SCALE:-1}" || exit 1
  rc=0
  shots "$DNUM" || rc=1
  down "$DNUM"
  exit $rc
}

case "${1:-}" in
  all) shift; all "$@";;
  up) shift; [ $# -ge 2 ] || die "usage: rig.sh up <display-num> <WxH> [scale]"; up "$@";;
  shots) shift; [ $# -ge 1 ] || die "usage: rig.sh shots <display-num>"; shots "$@";;
  down) shift; [ $# -ge 1 ] || die "usage: rig.sh down <display-num>"; down "$@";;
  *) sed -n '3,17p' "$0"; exit 2;;
esac
