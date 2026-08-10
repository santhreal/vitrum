#!/usr/bin/env python3
"""How many sidebar rows are PROVEN vs INFERRED against a real daemon.

The sidebar marks inferred rows, and the treatment only makes sense if inferred
is the minority. That is a question about real children, not about the model, so
this spawns them and reports what the daemon actually says.

Mirrors vitrum_model::status::resolve_status for a live child with no hint:
  waiting Some(true)  -> Ready,   source Waiting     PROVEN
  waiting Some(false) -> Working, source Foreground  PROVEN
  waiting None        -> bell / idle / output        INFERRED (idle, output)
"""

import asyncio
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time

import websockets

IDLE_ATTENTION_MS = 30_000

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# node is one of the interesting children (it parks on stdin without ever
# reporting itself as waiting), and where it lives differs on every machine.
# A path written down here is a path that resolves on exactly one of them.
NODE = shutil.which("node")


def protocol_version():
    """The number vitrum-proto declares, read from it rather than copied.

    The daemon refuses any other number outright, so a literal here is a tool
    that works until the next bump and then fails at the handshake with a
    message about protocols rather than about this file.
    """
    src = os.path.join(ROOT, "crates", "vitrum-proto", "src", "lib.rs")
    with open(src, "r") as f:
        m = re.search(r"pub const PROTOCOL_VERSION: u32 = (\d+);", f.read())
    if not m:
        raise RuntimeError("vitrum-proto declares no PROTOCOL_VERSION")
    return int(m.group(1))


def read_token():
    """The daemon's shared secret, the way vitrum_proto::token resolves it.

    The daemon spawns commands for anyone who completes the handshake, so it
    refuses a connection that cannot present this.
    """
    value = os.environ.get("VITRUM_TOKEN")
    if value:
        return value.strip()
    run = os.environ.get("XDG_RUNTIME_DIR")
    if run and os.path.isabs(run):
        path = os.path.join(run, "vitrum", "token")
    else:
        cache = os.environ.get("XDG_CACHE_HOME")
        if not (cache and os.path.isabs(cache)):
            cache = os.path.join(os.path.expanduser("~"), ".cache")
        path = os.path.join(cache, "vitrum", "run", "token")
    try:
        with open(path, "r") as f:
            return f.read().strip()
    except OSError as e:
        raise RuntimeError(
            f"cannot read the vitrum token at {path}: {e}; "
            "start vitrum-server as this user, or set VITRUM_TOKEN"
        )

CASES = [
    ("shell at a prompt", "/bin/sh", ["-i"]),
    ("blocked on read", "/bin/sh", ["-c", "read -r x"]),
    ("cat on the tty", "/bin/cat", []),
    ("sleeping", "/bin/sleep", ["300"]),
    ("spinning", "/bin/sh", ["-c", "while :; do :; done"]),
    ("shell in wait4", "/bin/sh", ["-c", "while :; do sleep 5; done"]),
    ("node parked on stdin", NODE, ["-e", "process.stdin.resume()"]),
    ("node event loop w/ timer", NODE, ["-e", "setInterval(()=>{},50); process.stdin.resume()"]),
    ("full-screen editor", "/usr/bin/vi", ["-u", "NONE", "-n"]),
    ("top on a timer", "/usr/bin/top", ["-d", "5"]),
]


def resolve(info):
    """(status, source, inferred) exactly as vitrum-model would."""
    if info["status"]["state"] != "running" and info["status"]["state"] != "starting":
        return ("failed" if info["attention"]["failed"] else "ready", "exit", False)
    if info.get("hint"):
        return (info["hint"]["state"], "hint", False)
    if info["attention"]["failed"]:
        return ("failed", "exit", False)
    waiting = info["attention"]["waiting"]
    if waiting is True:
        return ("ready", "waiting", False)
    if waiting is False:
        return ("working", "foreground", False)
    if info["attention"]["bell"]:
        return ("ready", "bell", False)
    if info["attention"]["idleMs"] >= IDLE_ATTENTION_MS:
        return ("ready", "idle", True)
    return ("working", "output", True)


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
        env=dict(os.environ, VITRUM_LOG="warn"),
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
            protocol = protocol_version()
            await ws.send(
                json.dumps(
                    {"t": "hello", "protocol": protocol, "token": read_token()}
                )
            )
            welcome = json.loads(await ws.recv())
            if welcome.get("t") != "welcome":
                raise RuntimeError(
                    "the daemon refused the handshake: %s" % (welcome,)
                )
            if welcome.get("protocol") != protocol:
                raise RuntimeError(
                    "this tool speaks protocol %d, the daemon speaks %s"
                    % (protocol, welcome.get("protocol"))
                )

            wanted = 0
            for title, command, args in CASES:
                if command is None:
                    print("  SKIP %-26s (not on PATH)" % title)
                    continue
                if not os.path.exists(command):
                    print("  SKIP %-26s (%s missing)" % (title, command))
                    continue
                wanted += 1
                await ws.send(
                    json.dumps(
                        {
                            "t": "createSession",
                            "projectId": 1,
                            "cwd": "/tmp",
                            "command": command,
                            "args": args,
                            "cols": 100,
                            "rows": 30,
                            "title": title,
                        }
                    )
                )

            await asyncio.sleep(3.0)
            await ws.send(json.dumps({"t": "list"}))
            snapshot = None
            deadline = time.monotonic() + 10
            while snapshot is None and time.monotonic() < deadline:
                m = json.loads(await ws.recv())
                if m["t"] == "sessions":
                    snapshot = m["sessions"]
            if snapshot is None:
                raise RuntimeError("the daemon sent no session list within 10s")
            # A ratio computed over fewer children than were asked for is a
            # different measurement than the one this prints, and nothing else
            # here would notice: a session that failed to spawn simply does not
            # appear in the list.
            if len(snapshot) != wanted:
                raise RuntimeError(
                    "asked for %d children, the daemon holds %d"
                    % (wanted, len(snapshot))
                )

            proven = inferred = 0
            print("%-26s %-8s %-11s %s" % ("child", "waiting", "source", "verdict"))
            for info in snapshot:
                status, source, is_inferred = resolve(info)
                proven += not is_inferred
                inferred += is_inferred
                print(
                    "%-26s %-8s %-11s %s (%s)"
                    % (
                        info["title"],
                        str(info["attention"]["waiting"]),
                        source,
                        "INFERRED" if is_inferred else "proven",
                        status,
                    )
                )
            total = proven + inferred
            print(
                "\n%d of %d rows PROVEN, %d INFERRED (%.0f%% inferred)"
                % (proven, total, inferred, 100.0 * inferred / max(total, 1))
            )
            return 0
    finally:
        daemon.terminate()
        try:
            daemon.wait(5)
        except subprocess.TimeoutExpired:
            daemon.kill()


sys.exit(asyncio.run(main(sys.argv[1])))
