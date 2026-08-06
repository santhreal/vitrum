#!/usr/bin/env python3
"""End-to-end proof, from OUTSIDE the daemon, with real child processes.

Every session runs a real /bin/sh that emits its escape sequence one byte at a
time with sleeps between, so the kernel really does split it across PTY reads.
Nothing here touches the daemon's internals: it speaks the wire protocol and
reads the sessions snapshot, exactly as the GUI would.
"""

import asyncio
import binascii
import json
import os
import socket
import subprocess
import sys
import time

import websockets


def dribble(seq: bytes) -> str:
    """A shell script that prints `seq` one byte at a time, with sleeps."""
    parts = []
    for b in seq:
        parts.append("printf '\\%03o'; sleep 0.02;" % b)
    return " ".join(parts)


CASES = {
    # ESC ] 7373 ; approval ; probe label ESC \
    "st_terminated": b"\x1b]7373;approval;probe label\x1b\\",
    # ESC ] 7373 ; input ; ready? BEL
    "bel_terminated": b"\x1b]7373;input;ready?\x07",
    # A window title, which every shell prompt emits, also BEL terminated.
    "window_title": b"\x1b]0;my window title\x07",
    # A genuine bell: the control that must still work.
    "real_bell": b"\x07",
    # An unknown state token must be refused, not defaulted.
    "unknown_state": b"\x1b]7373;paused\x07",
}


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


async def main(binary):
    port = free_port()
    daemon = subprocess.Popen(
        [binary, "--port", str(port)],
        env=dict(os.environ, REGLET_LOG="warn"),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                socket.create_connection(("127.0.0.1", port), 0.2).close()
                break
            except OSError:
                await asyncio.sleep(0.02)

        async with websockets.connect("ws://127.0.0.1:%d" % port, max_size=None) as ws:
            await ws.send(json.dumps({"t": "hello", "protocol": 1}))
            assert json.loads(await ws.recv())["t"] == "welcome"

            ids = {}
            for name, seq in CASES.items():
                await ws.send(
                    json.dumps(
                        {
                            "t": "createSession",
                            "projectId": 1,
                            "cwd": "/tmp",
                            "command": "/bin/sh",
                            "args": ["-c", dribble(seq) + " read -r x"],
                            "cols": 100,
                            "rows": 30,
                            "title": name,
                            }
                    )
                )
                m = json.loads(await ws.recv())
                while m["t"] != "sessionCreated":
                    m = json.loads(await ws.recv())
                ids[m["title"]] = m["id"]

            # Long enough for the slowest dribble (28 bytes x 20ms) plus the
            # 150ms settle window, with room to spare.
            await asyncio.sleep(3.0)

            await ws.send(json.dumps({"t": "list"}))
            snapshot = None
            deadline = time.monotonic() + 10
            while snapshot is None and time.monotonic() < deadline:
                m = json.loads(await ws.recv())
                if m["t"] == "sessions":
                    snapshot = m["sessions"]

            by_title = {s["title"]: s for s in snapshot}
            failures = []

            def check(title, ok, detail):
                mark = "PASS" if ok else "FAIL"
                print("  %-4s %-16s %s" % (mark, title, detail))
                if not ok:
                    failures.append(title)

            print("wire bytes sent by each child:")
            for name, seq in CASES.items():
                print("    %-16s %s" % (name, binascii.hexlify(seq, " ").decode()))
            print("what the daemon reports:")

            s = by_title["st_terminated"]
            check(
                "st_terminated",
                s["hint"] is not None
                and s["hint"]["state"] == "approval"
                and s["hint"]["label"] == "probe label"
                and s["attention"]["bell"] is False,
                "hint=%s bell=%s waiting=%s"
                % (s["hint"], s["attention"]["bell"], s["attention"]["waiting"]),
            )

            s = by_title["bel_terminated"]
            check(
                "bel_terminated",
                s["hint"] is not None
                and s["hint"]["state"] == "input"
                and s["hint"]["label"] == "ready?"
                and s["attention"]["bell"] is False,
                "hint=%s bell=%s waiting=%s"
                % (s["hint"], s["attention"]["bell"], s["attention"]["waiting"]),
            )

            s = by_title["window_title"]
            check(
                "window_title",
                s["hint"] is None and s["attention"]["bell"] is False,
                "hint=%s bell=%s (a title must not ring)" % (s["hint"], s["attention"]["bell"]),
            )

            s = by_title["real_bell"]
            check(
                "real_bell",
                s["hint"] is None and s["attention"]["bell"] is True,
                "hint=%s bell=%s (a bare BEL must still ring)"
                % (s["hint"], s["attention"]["bell"]),
            )

            s = by_title["unknown_state"]
            check(
                "unknown_state",
                s["hint"] is None and s["attention"]["bell"] is False,
                "hint=%s bell=%s (unknown token refused, not defaulted)"
                % (s["hint"], s["attention"]["bell"]),
            )

            print("waiting probe on every session:")
            for title, s in sorted(by_title.items()):
                print("    %-16s waiting=%s status=%s"
                      % (title, s["attention"]["waiting"], s["status"]))

            if failures:
                print("FAILURES: %s" % ", ".join(failures))
                return 1
            print("ALL CHECKS PASSED")
            return 0
    finally:
        daemon.terminate()
        try:
            daemon.wait(5)
        except subprocess.TimeoutExpired:
            daemon.kill()


sys.exit(asyncio.run(main(sys.argv[1])))
