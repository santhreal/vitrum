"""A stand-in for an agent, so a latency run drives the product's real shape.

The sessions vitrum exists to hold are coding agents: full-screen programs that
repaint, that print long wrapped lines, and that sit waiting for a person to
type. This reproduces those three behaviours and nothing else, so a measurement
is taken against the workload the product is for.

It also keeps a shell out of the measurement. A session named for a shell
measures a terminal multiplexer, and the numbers would describe one.

Modes:
  idle    print a banner and wait. Keystrokes are echoed by the line
          discipline, which is the same echo an agent's own readline does.
  emit    print a full-width line every --interval seconds, recording the
          moment before each write to --stamps.
  redraw  repaint the whole screen as fast as the terminal accepts it.
  history print --lines of transcript and then wait, so there is scrollback
          to move through.
"""

import argparse
import sys
import time

COLS = 120
ROWS = 40


def now():
    return time.clock_gettime(time.CLOCK_MONOTONIC)


def line(seed):
    base = 33 + seed % 60
    return "".join(chr(base + (c % 20)) for c in range(COLS))


def banner():
    sys.stdout.write("\x1b[2J\x1b[H")
    sys.stdout.write("assistant ready\r\n")
    sys.stdout.flush()


def mode_idle():
    banner()
    # Reading rather than sleeping: the pty must have a reader or the input
    # queue fills and the line discipline stops echoing, which would end a
    # keystroke measurement partway through with no error.
    for _ in sys.stdin:
        pass
    while True:
        time.sleep(3600)


def mode_emit(interval, stamps):
    banner()
    seed = 0
    with open(stamps, "w", buffering=1) as fh:
        while True:
            time.sleep(interval)
            seed += 1
            started = now()
            sys.stdout.write(line(seed) + "\r\n")
            sys.stdout.flush()
            fh.write(f"{started}\n")


def mode_redraw():
    frame = 0
    while True:
        frame += 1
        out = ["\x1b[H"]
        for row in range(ROWS):
            red = (frame * 7 + row * 3) % 200 + 20
            green = (frame * 11 + row * 5) % 200 + 20
            blue = (frame * 13 + row * 7) % 200 + 20
            out.append(f"\x1b[38;2;{red};{green};{blue}m")
            out.append(line(frame + row))
            if row + 1 < ROWS:
                out.append("\r\n")
        out.append("\x1b[0m")
        sys.stdout.write("".join(out))
        sys.stdout.flush()


def mode_history(lines):
    """A long transcript, then silence.

    Scrolling is measured against a session that has stopped writing. A session
    still printing would repaint the pane on its own, and the probe would time
    the agent's next line instead of the scroll.
    """
    banner()
    for i in range(lines):
        sys.stdout.write(f"{i:05d} {line(i)}\r\n")
    sys.stdout.flush()
    while True:
        time.sleep(3600)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["idle", "emit", "redraw", "history"])
    parser.add_argument("--interval", type=float, default=0.6)
    parser.add_argument("--stamps", default="/tmp/vitrum-stamps")
    parser.add_argument("--lines", type=int, default=4000)
    args = parser.parse_args()
    if args.mode == "idle":
        mode_idle()
    elif args.mode == "emit":
        mode_emit(args.interval, args.stamps)
    elif args.mode == "history":
        mode_history(args.lines)
    else:
        mode_redraw()


if __name__ == "__main__":
    main()
