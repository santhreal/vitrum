#!/usr/bin/env python3
"""A fake coding-agent TUI, for running as a session's command under a PTY.

The benchmark compares how much memory vitrum and T3 Code spend on the same
terminal workload. A program that only prints lines would measure almost
nothing a terminal actually does, so this behaves the way a real agent TUI
behaves: it takes the alternate screen, addresses the cursor instead of only
emitting newlines, keeps a full-width status line and a spinner redrawn in
place, wraps and reflows streamed text, and reflows again on resize.

It also emits vitrum's OSC 7373 hints, `working` while a response streams and
`approval` when it stops for an answer, so the attention path is exercised
rather than left cold. Terminals that do not know the sequence ignore it, so
the same bytes go to T3 Code and the workload stays identical.

Output is a pure function of `--seed`, `--turns` and the terminal size: no
clock, no counter, no random state that outlives a turn. Two runs with one
seed against one mock produce byte-identical output.

usage: agentsim.py --endpoint http://127.0.0.1:N --turns T
                   [--cols C] [--rows R] [--seed S]
"""

import argparse
import http.client
import json
import os
import random
import signal
import sys
import time
from urllib.parse import urlparse

ESC = "\x1b"
ALT_ON = f"{ESC}[?1049h"
ALT_OFF = f"{ESC}[?1049l"
HIDE_CURSOR = f"{ESC}[?25l"
SHOW_CURSOR = f"{ESC}[?25h"
SPINNER = "|/-\\"

TASKS = [
    "read crates/vitrum-core/src/session.rs",
    "wire the scrollback trim to the resize path",
    "explain why the hint parser keeps partial sequences",
    "add a regression test for split OSC frames",
    "measure the buffer growth under thirty two windows",
]
APPROVALS = [
    "apply the patch to session.rs?",
    "run the full test suite?",
    "overwrite harness/out/report.json?",
]


class Screen:
    """Cursor-addressed drawing over a fixed grid.

    Everything is written through one buffered stream and flushed once per
    frame, because a TUI that flushes per escape sequence measures the write
    path rather than the terminal.
    """

    def __init__(self, stream, cols, rows):
        self.stream = stream
        self.resize(cols, rows)

    def resize(self, cols, rows):
        self.cols = max(20, cols)
        self.rows = max(8, rows)
        self.body_top = 3
        self.body_bottom = self.rows - 2
        self.spinner_row = self.rows - 1
        self.status_row = 1

    @property
    def body_height(self):
        return max(1, self.body_bottom - self.body_top + 1)

    def at(self, row, col=1):
        self.stream.write(f"{ESC}[{row};{col}H")

    def clear_line(self):
        self.stream.write(f"{ESC}[2K")

    def write(self, text):
        self.stream.write(text)

    def flush(self):
        self.stream.flush()

    def clear(self):
        self.stream.write(f"{ESC}[2J")

    def status(self, left, right):
        """A full-width reverse-video bar, padded or truncated to the width."""
        gap = self.cols - len(left) - len(right)
        if gap < 1:
            left = left[: max(0, self.cols - len(right) - 1)]
            gap = max(1, self.cols - len(left) - len(right))
        self.at(self.status_row)
        self.clear_line()
        self.write(f"{ESC}[7m{left}{' ' * gap}{right}{ESC}[0m")

    def spinner(self, frame, label):
        self.at(self.spinner_row)
        self.clear_line()
        self.write(f"{SPINNER[frame % len(SPINNER)]} {label}"[: self.cols])

    def prompt(self, text):
        self.at(self.rows)
        self.clear_line()
        self.write(text[: self.cols])


def hint(stream, state, label=None):
    """Emit OSC 7373 with an ST terminator, never BEL.

    BEL is a legal terminator for the sequence but it is also the bell, and a
    terminal that does not parse the hint would ring on every one of these.
    """
    body = f"7373;{state}" if label is None else f"7373;{state};{label}"
    stream.write(f"{ESC}]{body}{ESC}\\")


class Wrapped:
    """The scrolling body: word wrapping now, full reflow on resize."""

    def __init__(self, screen):
        self.screen = screen
        self.text = ""

    def feed(self, text):
        self.text += text

    def lines(self):
        """Wrap the accumulated text at the current width.

        Wrapping from the whole buffer rather than from a fixed line list is
        what makes a resize reflow instead of just re-clipping: the same text
        lands on different rows once the width changes.
        """
        out = [""]
        for word in self.text.split(" "):
            while len(word) > self.screen.cols:
                # A word wider than the screen has to break, the same way a
                # terminal breaks it.
                if out[-1]:
                    out.append("")
                out[-1] = word[: self.screen.cols]
                word = word[self.screen.cols :]
                out.append("")
            if not out[-1]:
                out[-1] = word
            elif len(out[-1]) + 1 + len(word) <= self.screen.cols:
                out[-1] += " " + word
            else:
                out.append(word)
        return out

    def draw(self):
        lines = self.lines()[-self.screen.body_height :]
        for index in range(self.screen.body_height):
            self.screen.at(self.screen.body_top + index)
            self.screen.clear_line()
            if index < len(lines):
                self.screen.write(lines[index][: self.screen.cols])

    def reset(self):
        self.text = ""


class Stopped(Exception):
    """SIGTERM arrived. Restore the terminal before leaving."""


def stream_tokens(endpoint, prompt, seed, turn):
    """Yield response tokens from the mock's OpenAI-compatible route.

    Raises on anything other than a clean 200 stream, because a silent success
    against a dead endpoint would report a memory number for a workload that
    never ran.
    """
    url = urlparse(endpoint)
    conn = http.client.HTTPConnection(url.hostname, url.port or 80, timeout=30)
    body = json.dumps(
        {
            "model": "mockllm",
            "stream": True,
            "seed": seed,
            "messages": [{"role": "user", "content": f"turn {turn}: {prompt}"}],
        }
    )
    try:
        conn.request(
            "POST",
            "/v1/chat/completions",
            body=body,
            headers={"Content-Type": "application/json"},
        )
        response = conn.getresponse()
        if response.status != 200:
            raise RuntimeError(f"endpoint answered {response.status} for a stream")
        for raw in response:
            line = raw.decode("utf-8", "replace").strip()
            if not line.startswith("data:"):
                continue
            payload = line[5:].strip()
            if payload == "[DONE]":
                return
            frame = json.loads(payload)
            for choice in frame.get("choices", []):
                token = choice.get("delta", {}).get("content")
                if token:
                    yield token
    finally:
        conn.close()


def check_endpoint(endpoint):
    url = urlparse(endpoint)
    conn = http.client.HTTPConnection(url.hostname, url.port or 80, timeout=10)
    try:
        conn.request("GET", "/healthz")
        response = conn.getresponse()
        payload = response.read()
        if response.status != 200 or payload.strip() != b"ok":
            raise RuntimeError(f"healthz answered {response.status} {payload!r}")
    finally:
        conn.close()


def terminal_size(cols, rows):
    if cols and rows:
        return cols, rows
    try:
        size = os.get_terminal_size(sys.stdout.fileno())
        detected = (size.columns, size.lines)
    except OSError:
        detected = (80, 24)
    return cols or detected[0], rows or detected[1]


def run(args, out):
    cols, rows = terminal_size(args.cols, args.rows)
    screen = Screen(out, cols, rows)
    body = Wrapped(screen)
    resized = []

    def on_winch(_signum, _frame):
        resized.append(True)

    if hasattr(signal, "SIGWINCH"):
        signal.signal(signal.SIGWINCH, on_winch)

    def maybe_reflow():
        if not resized:
            return
        resized.clear()
        new_cols, new_rows = terminal_size(args.cols, args.rows)
        screen.resize(new_cols, new_rows)
        screen.clear()
        body.draw()

    out.write(ALT_ON + HIDE_CURSOR)
    screen.clear()
    rng = random.Random(args.seed)
    for turn in range(1, args.turns + 1):
        task = TASKS[rng.randrange(len(TASKS))]
        approval = APPROVALS[rng.randrange(len(APPROVALS))]
        body.reset()
        screen.clear()
        screen.status(f" agentsim  turn {turn}/{args.turns} ", f" {task[:30]} ")
        screen.at(screen.body_top - 1)
        screen.clear_line()
        screen.write(f"> {task}"[: screen.cols])
        hint(out, "working", task)
        screen.spinner(0, "thinking")
        screen.flush()

        for index, token in enumerate(stream_tokens(args.endpoint, task, args.seed, turn)):
            maybe_reflow()
            body.feed(token)
            body.draw()
            # The spinner frame follows the token index, not the clock, so the
            # captured bytes do not depend on how fast the host happened to be.
            screen.spinner(index + 1, f"streaming token {index + 1}")
            screen.status(
                f" agentsim  turn {turn}/{args.turns} ", f" {index + 1} tokens "
            )
            screen.flush()

        hint(out, "approval", approval)
        screen.spinner(0, "waiting")
        screen.prompt(f"[approval] {approval} (y/n) ")
        screen.flush()
        # A real TUI blocks here. This one holds long enough for the daemon to
        # observe the attention state, then answers itself, because the
        # benchmark runs unattended and a blocking read would never return.
        time.sleep(0.15)
        screen.prompt("[approval] y")
        screen.flush()

    hint(out, "ready", "done")
    screen.at(screen.rows)
    screen.flush()
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--turns", type=int, required=True)
    parser.add_argument("--cols", type=int, default=0)
    parser.add_argument("--rows", type=int, default=0)
    parser.add_argument("--seed", type=int, default=0)
    args = parser.parse_args()
    if args.turns < 1:
        parser.error("--turns must be at least 1")

    out = sys.stdout

    def stop(_signum, _frame):
        raise Stopped()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)

    try:
        check_endpoint(args.endpoint)
    except Exception as error:
        print(f"agentsim: endpoint {args.endpoint} unreachable: {error}", file=sys.stderr)
        return 2

    try:
        return run(args, out)
    except Stopped:
        return 0
    except Exception as error:
        out.write(ALT_OFF + SHOW_CURSOR)
        out.flush()
        print(f"agentsim: {error}", file=sys.stderr)
        return 1
    finally:
        # Leaving the alternate screen set would poison whatever the harness
        # runs next in this terminal.
        try:
            out.write(ALT_OFF + SHOW_CURSOR)
            out.flush()
        except ValueError:
            pass


if __name__ == "__main__":
    sys.exit(main())
