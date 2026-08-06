#!/bin/sh
# Deterministic vitrum screenshot: one window, one named size, one named UI scale.
#
# usage:
#   tools/regression/screenshot.sh <display-num> <WxH> <ui-scale|auto> <out.png> [app args...]
#
#   tools/regression/screenshot.sh 90 1920x1080 1.0 /tmp/a.png --server ws://127.0.0.1:7801
#   tools/regression/screenshot.sh 91 3840x2160 1.5 /tmp/b.png --server ws://127.0.0.1:7801
#
# VITRUM_APP overrides the binary; it defaults to the debug build in this tree.
# The script prints the window id, the resolved UI scale from the app's own log,
# and the captured image's mean pixel value, then exits non-zero if the capture
# is blank.
#
# ---------------------------------------------------------------------------
# Why this exists, and why it is Xvfb rather than the real desktop
# ---------------------------------------------------------------------------
#
# Four traps produced four wrong conclusions in one afternoon of screenshotting
# this application on a live desktop. Every one of them is silent: the tool
# exits 0 and hands back an image, and the image is a lie.
#
#  1. OCCLUSION RETURNS WHITE. `import -window <id>` on a window that is covered
#     by another window returns a pure white image and exits 0. A run of captures
#     taken while a terminal sat over the window produced "the UI is blank",
#     which was read as a rendering bug. Xvfb has nothing on top of anything, and
#     this script additionally fails on a blank frame instead of returning it.
#
#  2. THE APP MAPS TWO WINDOWS. Every vitrum process maps a 10x10 decoy at +10+10
#     whose WM_NAME is the BINARY file name, and the real window, whose WM_NAME is
#     exactly "vitrum". So `xdotool search --name vitrum` matches both, and a
#     search for the binary name matches ONLY the decoy. Resolving by name and
#     taking the first hit gives you a 10x10 window and "the app opens no window".
#     This script resolves by pid and rejects the 10x10.
#
#  3. SEVERAL INSTANCES SHARE ONE DISPLAY. With three agents each running the app
#     on the same X display, `--name '^vitrum$'` is ambiguous and you photograph
#     somebody else's window. I measured another process's window for ten minutes
#     and drew conclusions from it. One display per run removes the class.
#
#  4. MUTTER MAXIMIZES THE WINDOW AND THEN REFUSES TO RESIZE IT. On the live
#     desktop the window comes up maximized to the whole monitor;
#     `xdotool windowsize`, `wmctrl -b remove,maximized_*` and the application's
#     own restore glyph all leave the geometry at the monitor size, so no size
#     other than the monitor can be captured. (An `_NET_WM_STATE` client message
#     sent by hand does work, which is what this script would have to do on a
#     real display. It does not have to, because Xvfb has no window manager.)
#
# A fifth, found while validating this script and the reason it is Xvfb and NOT
# Xephyr: XEPHYR DISTORTS COLOUR. Measured on the same build and the same shot,
# the sidebar background reads #0a0a0a under Xephyr and #131316 under Xvfb, and
# #131316 is what the real HDMI-0 panel reads. Xephyr composites into a window on
# the parent server and the values that come back are not the values the
# application asked for, so any contrast or palette claim made from a Xephyr
# capture is wrong by roughly a factor of two in luminance. Xephyr also died
# every time it was asked for a 3840x2160 screen on this machine's amdgpu.
#
# ---------------------------------------------------------------------------
# Why the UI scale is an argument rather than a dpi
# ---------------------------------------------------------------------------
#
# vitrum derives its document zoom from PHYSICAL MILLIMETRES read out of RandR,
# not from the toolkit scale factor, because on a GNOME session
# gdk_monitor_get_scale_factor() reports 1 for both an 82 dpi panel and a 163 dpi
# one. On the real monitors that gives 1920x1080 at 597x336mm -> 81.7 dpi -> 1.0
# and 3840x2160 at 597x336mm -> 163.3 dpi -> 1.5, both auto-derived.
#
# Xvfb's RandR output does not forward the millimetres that `-dpi` implies: with
# `-dpi 163` the core protocol reports 598x337mm but the app reads 1016x571mm and
# lands on 96 dpi, so it draws at 1.0 whatever you pass. That is why the scale is
# an explicit argument here: passing `1.5` reproduces the same document scale the
# 4K panel auto-derives instead of approximating it. Pass `auto` to let the app
# decide, which on Xvfb means 1.0.
set -e

DNUM="$1"
GEOM="$2"
SCALE="$3"
OUT="$4"
[ -n "$OUT" ] || { sed -n '2,8p' "$0"; exit 2; }
shift 4

W=${GEOM%x*}
H=${GEOM#*x}
BIN="${VITRUM_APP:-$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)/target/debug/vitrum-app}"
[ -x "$BIN" ] || { echo "no vitrum-app at $BIN; set VITRUM_APP" >&2; exit 2; }

LOG="${TMPDIR:-/tmp}/vitrum-shot-$DNUM"
mkdir -p "$LOG"

# A server left behind by an interrupted run answers on this display number, and
# you then silently measure ITS geometry rather than the one you asked for. Do
# not trust the EXIT trap of a script that may have been killed.
pkill -f "Xvfb :$DNUM " 2>/dev/null || true
sleep 0.3

Xvfb ":$DNUM" -screen 0 "${GEOM}x24" -dpi 96 >"$LOG/xvfb.log" 2>&1 &
XPID=$!
cleanup() {
  [ -n "$APID" ] && kill "$APID" 2>/dev/null
  kill "$XPID" 2>/dev/null
  pkill -f "Xvfb :$DNUM " 2>/dev/null
  return 0
}
trap cleanup EXIT INT TERM

i=0
while [ "$i" -lt 150 ]; do
  DISPLAY=":$DNUM" xdpyinfo >/dev/null 2>&1 && break
  i=$((i + 1))
  sleep 0.1
done
DISPLAY=":$DNUM" xdpyinfo >/dev/null 2>&1 || { echo "Xvfb :$DNUM did not start" >&2; exit 1; }

if [ "$SCALE" = "auto" ]; then
  set -- "$@"
else
  set -- --ui-scale "$SCALE" "$@"
fi

# --no-autostart: the caller decides which daemon this window talks to, and a
# screenshot run must never spawn one and leave it behind.
# --standalone: without it a second launch hands off to a running instance and
# this run photographs that instance's window instead of its own.
DISPLAY=":$DNUM" "$BIN" --standalone --no-autostart "$@" >"$LOG/app.log" 2>&1 &
APID=$!

# Resolve by pid, reject the 10x10 decoy. See trap 2 and trap 3 above.
WIN=""
i=0
while [ "$i" -lt 600 ]; do
  for w in $(DISPLAY=":$DNUM" xdotool search --name '^vitrum$' 2>/dev/null); do
    p=$(DISPLAY=":$DNUM" xdotool getwindowpid "$w" 2>/dev/null || echo 0)
    [ "$p" = "$APID" ] || continue
    case $(DISPLAY=":$DNUM" xdotool getwindowgeometry "$w") in
      *10x10*) ;;
      *) WIN="$w" ;;
    esac
  done
  [ -n "$WIN" ] && break
  i=$((i + 1))
  sleep 0.05
done
[ -n "$WIN" ] || { echo "no window for pid $APID; log:" >&2; tail -20 "$LOG/app.log" >&2; exit 1; }
echo "window $WIN"

# The window is mapped several seconds before the first daemon snapshot lands. A
# shot taken in that gap shows an empty sidebar, which looks exactly like a bug
# and is not one.
sleep 6
DISPLAY=":$DNUM" xdotool windowsize "$WIN" "$W" "$H" || true
DISPLAY=":$DNUM" xdotool windowmove "$WIN" 0 0 || true
# Park the pointer in a corner so no row is left in a hover state, which would
# otherwise differ between two runs of the same command.
DISPLAY=":$DNUM" xdotool mousemove 1 1 || true
sleep 2

# The monitor line is what the app DERIVED from RandR; "requested" is what it was
# told to use. On Xvfb those differ whenever SCALE is not "auto", and the
# requested one is what the document is actually drawn at.
echo "requested ui scale $SCALE"
grep -o 'monitor .*ui scale [0-9.]*' "$LOG/app.log" | tail -1 || true
DISPLAY=":$DNUM" import -window "$WIN" "$OUT"

python3 - "$OUT" <<'PY'
import sys
from PIL import Image
import numpy as np
a = np.asarray(Image.open(sys.argv[1]).convert("RGB"))
mean = a.mean()
print(f"{sys.argv[1]}  {a.shape[1]}x{a.shape[0]}  mean={mean:.1f}")
if mean > 250:
    sys.exit("blank capture: occluded window or an unpainted UI")
if mean < 0.5:
    sys.exit("black capture: the window was never painted")
PY
