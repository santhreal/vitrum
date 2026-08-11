"""Measure perceived latency of an X client from its pixels.

One subcommand per signal. Each prints JSON on stdout and nothing else, so a
caller can pipe it. Every figure is nanoseconds unless the key says otherwise.

The idle set
------------

A terminal is never still: the caret blinks, and a blink is a pixel change that
answers no cause. Waiting for "any change" would therefore report the blink
period and call it latency.

So every wait here is for a frame this rectangle has NOT been in. The idle set
is learned by watching the rectangle with nothing happening, which collects
every state the blink cycles through. A digest outside that set is new content,
and new content is the answer to the cause. The set is relearned after each
sample, because the steady state after a keystroke is a different steady state.

Usage:
  measure.py probe      --display :8
  measure.py keystroke  --display :8 --samples 100
  measure.py output     --display :8 --stamps FILE --samples 100
  measure.py firstframe --display :8 --spawns 5 -- CMD...
  measure.py repaint    --display :8 --seconds 20
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from xpixels import Screen, now  # noqa: E402

# Long enough to cover a caret blink cycle at any rate a terminal uses, so the
# idle set holds both halves of it.
IDLE_LEARN_SECONDS = 1.4

# A cause that has produced nothing on screen after this long has not produced
# anything. Generous: the point is to record a miss, not to cut a slow answer
# short and report the timeout as the latency.
ANSWER_TIMEOUT = 8.0


def dist(samples):
    """Nearest-rank percentiles over nanosecond samples."""
    if not samples:
        return None
    s = sorted(samples)

    def pick(q):
        rank = max(1, int(-(-q * len(s) // 1)))
        return s[min(rank, len(s)) - 1]

    return {
        "count": len(s),
        "min": s[0],
        "p50": pick(0.50),
        "p95": pick(0.95),
        "p99": pick(0.99),
        "max": s[-1],
        "mean": sum(s) // len(s),
    }


def learn_idle(screen, rect, seconds=IDLE_LEARN_SECONDS):
    """Every digest the rectangle shows while nothing is happening."""
    seen = set()
    end = now() + seconds
    while now() < end:
        seen.add(screen.digest(rect))
    return seen


def wait_new(screen, rect, known, timeout=ANSWER_TIMEOUT):
    """When the rectangle first shows a frame outside `known`."""
    deadline = now() + timeout
    while True:
        digest = screen.digest(rect)
        if digest not in known:
            return now()
        if now() > deadline:
            return None


def pane_rect(screen, window, args):
    """The rectangle sampled, defaulting to the middle of the terminal pane.

    The default leaves the sidebar out. A change there answers the same cause
    and would count, which would report whichever surface repainted first
    rather than the one the operator is reading.
    """
    if args.rect:
        return tuple(int(v) for v in args.rect.split(","))
    x, y, w, h = screen.geometry(window)
    left = x + int(w * 0.34)
    return (left, y + int(h * 0.10), min(560, w - (left - x) - 8), 220)


def find_window(screen, tries=200):
    """The window under test, once one is mapped."""
    for _ in range(tries):
        win = screen.biggest_child()
        if win:
            return win
        time.sleep(0.05)
    raise SystemExit("no mapped window appeared on this display")


def cmd_probe(args):
    screen = Screen(args.display)
    win = find_window(screen)
    rect = pane_rect(screen, win, args)
    idle = learn_idle(screen, rect, 2.0)
    print(
        json.dumps(
            {
                "window": win,
                "geometry": screen.geometry(win),
                "rect": rect,
                "idle_states": len(idle),
                "colours_in_rect": screen.colours(rect),
            },
            indent=2,
        )
    )


def cmd_select(args):
    """Put a session on screen, so the pane holds a transcript to measure."""
    screen = Screen(args.display)
    win = find_window(screen)
    screen.focus(win)
    if args.click:
        x, y = (int(v) for v in args.click.split(","))
        screen.click(x, y)
    else:
        screen.chord(["Alt_L"], args.key)
    time.sleep(args.settle)
    rect = pane_rect(screen, win, args)
    print(
        json.dumps(
            {
                "window": win,
                "rect": rect,
                "colours_in_rect": screen.colours(rect),
                "idle_states": len(learn_idle(screen, rect, 2.0)),
            },
            indent=2,
        )
    )


def cmd_keystroke(args):
    """Type a key, wait for the glyph, repeat.

    The pane is clicked first. Keyboard focus in this window belongs to
    whichever surface was last pointed at, so typing without that measures a
    keystroke the terminal never receives and reports every sample as a miss.
    """
    screen = Screen(args.display)
    win = find_window(screen)
    screen.focus(win)
    x, y, w, h = screen.geometry(win)
    screen.click(x + int(w * 0.6), y + int(h * 0.5))
    time.sleep(args.settle)
    rect = pane_rect(screen, win, args)
    codes = [screen.keycode(name) for name in "a b c d e f g h".split()]

    samples, misses = [], 0
    for i in range(args.samples):
        idle = learn_idle(screen, rect)
        started = screen.press(codes[i % len(codes)])
        answered = wait_new(screen, rect, idle)
        if answered is None:
            misses += 1
            continue
        samples.append(int((answered - started) * 1e9))
    print(
        json.dumps(
            {
                "signal": "keystroke_to_glyph",
                "rect": rect,
                "misses": misses,
                "dist": dist(samples),
            },
            indent=2,
        )
    )


def cmd_output(args):
    """Wait for a marker the session printed, and time from when it printed.

    The emitter writes its own pre-write timestamp to `--stamps` on the same
    host and the same clock, so this is one clock's difference and not two
    machines being compared.
    """
    screen = Screen(args.display)
    win = find_window(screen)
    rect = pane_rect(screen, win, args)

    def last_stamp():
        try:
            with open(args.stamps) as fh:
                lines = [line for line in fh.read().split("\n") if line.strip()]
        except FileNotFoundError:
            return None
        return float(lines[-1]) if lines else None

    samples, misses = [], 0
    seen = last_stamp()
    for _ in range(args.samples):
        idle = learn_idle(screen, rect)
        answered = wait_new(screen, rect, idle)
        stamp = last_stamp()
        if answered is None or stamp is None or stamp == seen:
            misses += 1
            continue
        seen = stamp
        samples.append(int((answered - stamp) * 1e9))
    print(
        json.dumps(
            {
                "signal": "output_to_glyph",
                "rect": rect,
                "misses": misses,
                "dist": dist(samples),
            },
            indent=2,
        )
    )


def cmd_scroll(args):
    """Turn the wheel one notch over the pane and time the repaint.

    Scrollback is the one interaction where a terminal has to redraw every row
    at once, so it is the frame a user notices. The wheel is turned rather than
    a key pressed because that is the input a scroll arrives on, and a keyboard
    shortcut would measure a different path.
    """
    screen = Screen(args.display)
    win = find_window(screen)
    x, y, w, h = screen.geometry(win)
    screen.click(x + int(w * 0.6), y + int(h * 0.5))
    time.sleep(args.settle)
    rect = pane_rect(screen, win, args)
    at = (x + int(w * 0.6), y + int(h * 0.5))

    samples, misses = [], 0
    for i in range(args.samples):
        # Alternate direction, so the view cannot reach the end of the
        # scrollback and start reporting no-op frames as misses.
        button = 4 if (i // 8) % 2 == 0 else 5
        idle = learn_idle(screen, rect)
        started = screen.click(at[0], at[1], button=button)
        answered = wait_new(screen, rect, idle)
        if answered is None:
            misses += 1
            continue
        samples.append(int((answered - started) * 1e9))
    print(
        json.dumps(
            {
                "signal": "scroll_frame",
                "rect": rect,
                "misses": misses,
                "dist": dist(samples),
            },
            indent=2,
        )
    )


def cmd_firstframe(args):
    """Spawn the program and time until its window holds readable content.

    Two answers, because they are two different questions and quoting one for
    the other is how a cold start gets understated:

    - `first_pixels`: anything at all is on screen.
    - `readable`: the sampled rectangle holds more than `--colours` distinct
      pixel values, which no blank surface or plain background reaches and
      which text does.
    """
    results_pixels, results_readable = [], []
    for _ in range(args.spawns):
        env = dict(os.environ, DISPLAY=args.display)
        started = now()
        proc = subprocess.Popen(
            args.command,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        screen = Screen(args.display)
        first_pixels = None
        readable = None
        deadline = started + args.timeout
        rect = None
        while now() < deadline:
            win = screen.biggest_child()
            if not win:
                continue
            if first_pixels is None:
                first_pixels = now()
                x, y, w, h = screen.geometry(win)
                rect = (x + int(w * 0.34), y + int(h * 0.10), min(560, w), 220)
            if screen.colours(rect) > args.colours:
                readable = now()
                break
        if first_pixels:
            results_pixels.append(int((first_pixels - started) * 1e9))
        if readable:
            results_readable.append(int((readable - started) * 1e9))
        os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            proc.wait(timeout=10)
        time.sleep(args.settle)
    print(
        json.dumps(
            {
                "signal": "first_frame",
                "colours_threshold": args.colours,
                "first_pixels": dist(results_pixels),
                "readable": dist(results_readable),
            },
            indent=2,
        )
    )


def cmd_repaint(args):
    """How often the pane can actually change while a program repaints it.

    Counts distinct frames the rectangle shows over a window of wall clock.
    Frames per second here is an upper bound on what a full-screen redraw can
    deliver, because two repaints that produce the same pixels count once.
    """
    screen = Screen(args.display)
    win = find_window(screen)
    rect = pane_rect(screen, win, args)

    changes = []
    last = screen.digest(rect)
    started = now()
    end = started + args.seconds
    polls = 0
    while now() < end:
        polls += 1
        digest = screen.digest(rect)
        if digest != last:
            changes.append(now())
            last = digest
    gaps = [
        int((b - a) * 1e9) for a, b in zip(changes, changes[1:]) if b > a
    ]
    elapsed = now() - started
    print(
        json.dumps(
            {
                "signal": "redraw_frame",
                "rect": rect,
                "seconds": elapsed,
                "frames": len(changes),
                "frames_per_second": len(changes) / elapsed if elapsed else 0,
                "polls": polls,
                "poll_interval_ns": int(elapsed / polls * 1e9) if polls else 0,
                "gap": dist(gaps),
            },
            indent=2,
        )
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--display", default=os.environ.get("DISPLAY", ":8"))
    parser.add_argument("--rect", help="x,y,w,h to sample instead of the default")
    sub = parser.add_subparsers(dest="cmd", required=True)

    sub.add_parser("probe")

    p = sub.add_parser("select")
    p.add_argument("--key", default="1")
    p.add_argument("--click", help="x,y to click instead of pressing Alt+key")
    p.add_argument("--settle", type=float, default=2.0)

    p = sub.add_parser("keystroke")
    p.add_argument("--samples", type=int, default=100)
    p.add_argument("--settle", type=float, default=1.0)

    p = sub.add_parser("output")
    p.add_argument("--samples", type=int, default=100)
    p.add_argument("--stamps", required=True)

    p = sub.add_parser("scroll")
    p.add_argument("--samples", type=int, default=60)
    p.add_argument("--settle", type=float, default=1.0)

    p = sub.add_parser("firstframe")
    p.add_argument("--spawns", type=int, default=5)
    p.add_argument("--timeout", type=float, default=60.0)
    p.add_argument("--settle", type=float, default=3.0)
    p.add_argument("--colours", type=int, default=24)
    p.add_argument("command", nargs=argparse.REMAINDER)

    p = sub.add_parser("repaint")
    p.add_argument("--seconds", type=float, default=20.0)

    args = parser.parse_args()
    if args.cmd == "firstframe" and args.command and args.command[0] == "--":
        args.command = args.command[1:]
    {
        "probe": cmd_probe,
        "select": cmd_select,
        "keystroke": cmd_keystroke,
        "output": cmd_output,
        "scroll": cmd_scroll,
        "firstframe": cmd_firstframe,
        "repaint": cmd_repaint,
    }[args.cmd](args)


if __name__ == "__main__":
    main()
