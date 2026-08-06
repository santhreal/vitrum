#!/usr/bin/env python3
"""Measure daemon CPU and RSS for N streaming sessions.

Starts the release daemon on an ephemeral port, creates N sessions each
emitting a fixed-rate stream, optionally attaches to them, then samples
/proc/<pid>/stat over a measurement window.
"""

import argparse
import asyncio
import json
import os
import socket
import subprocess
import sys
import time

import websockets

GEN = """
import sys, time
block = ({payload!r} * {reps}).encode()
period = {period}
nxt = time.monotonic()
out = sys.stdout.buffer
while True:
    out.write(block)
    out.flush()
    nxt += period
    d = nxt - time.monotonic()
    if d > 0:
        time.sleep(d)
"""


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def proc_cpu(pid):
    with open("/proc/%d/stat" % pid, "rb") as f:
        fields = f.read().rsplit(b") ", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def proc_switches(pid):
    """(voluntary, nonvoluntary) context switches, summed over every thread.

    The honest idle metric. 0.00% CPU is also what a 20Hz do-nothing timer
    reports, so a percentage cannot distinguish "parked on a channel" from
    "waking twenty times a second to find nothing". A switch count can.
    """
    vol = invol = 0
    task = "/proc/%d/task" % pid
    for tid in os.listdir(task):
        try:
            with open("%s/%s/status" % (task, tid)) as f:
                for line in f:
                    if line.startswith("voluntary_ctxt_switches:"):
                        vol += int(line.split()[1])
                    elif line.startswith("nonvoluntary_ctxt_switches:"):
                        invol += int(line.split()[1])
        except OSError:
            pass
    return vol, invol


def proc_rss_kb(pid):
    with open("/proc/%d/status" % pid) as f:
        for line in f:
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    return 0


async def main(a):
    port = free_port()
    daemon = subprocess.Popen(
        [a.binary, "--port", str(port)],
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
        else:
            raise SystemExit("daemon never listened")

        if a.ansi:
            payload = "".join(
                "\x1b[3%dm%s\x1b[0m" % (i % 8, "x" * 20) for i in range(5)
            )
        else:
            payload = "x" * 100
        payload += "\r\n"
        period = 0.01
        reps = max(1, int(a.kbps * 1024 * period / len(payload)))
        script = GEN.format(payload=payload, reps=reps, period=period)
        per_session_bps = len(payload) * reps / period

        ids = []
        rx_bytes = [0]

        async with websockets.connect(
            "ws://127.0.0.1:%d" % port, max_size=None, max_queue=256
        ) as ws:
            await ws.send(json.dumps({"t": "hello", "protocol": a.protocol}))
            hello = json.loads(await ws.recv())
            assert hello["t"] == "welcome", hello

            if a.child == "idle":
                command, cargs = "/bin/bash", ["--norc", "-i"]
            else:
                command, cargs = sys.executable, ["-c", script]
            for _ in range(a.sessions):
                await ws.send(
                    json.dumps(
                        {
                            "t": "createSession",
                            "projectId": 1,
                            "cwd": "/tmp",
                            "command": command,
                            "args": cargs,
                            "cols": 200,
                            "rows": 50,
                            "title": None,
                        }
                    )
                )

            stop = asyncio.Event()

            async def reader():
                try:
                    async for frame in ws:
                        if isinstance(frame, bytes):
                            rx_bytes[0] += len(frame)
                        else:
                            m = json.loads(frame)
                            if m["t"] == "sessionCreated":
                                ids.append(m["id"])
                            elif m["t"] == "error":
                                print("SERVER ERROR:", m, file=sys.stderr)
                except websockets.ConnectionClosed:
                    pass
                stop.set()

            task = asyncio.create_task(reader())

            t0 = time.monotonic()
            while len(ids) < a.sessions and time.monotonic() - t0 < 20:
                await asyncio.sleep(0.02)
            if len(ids) < a.sessions:
                raise SystemExit("only %d sessions created" % len(ids))

            attach = {"all": ids, "one": ids[:1], "none": []}[a.attach]
            for sid in attach:
                await ws.send(
                    {"t": "attach", "session": sid, "cols": 200, "rows": 50}
                    if False
                    else json.dumps(
                        {"t": "attach", "session": sid, "cols": 200, "rows": 50}
                    )
                )

            # Let the pipeline reach steady state before sampling.
            await asyncio.sleep(a.warmup)

            c0, r0, w0, b0 = (
                proc_cpu(daemon.pid),
                proc_rss_kb(daemon.pid),
                time.monotonic(),
                rx_bytes[0],
            )
            s0 = proc_switches(daemon.pid)
            await asyncio.sleep(a.seconds)
            c1, r1, w1, b1 = (
                proc_cpu(daemon.pid),
                proc_rss_kb(daemon.pid),
                time.monotonic(),
                rx_bytes[0],
            )
            s1 = proc_switches(daemon.pid)

            task.cancel()

        wall = w1 - w0
        print(
            json.dumps(
                {
                    "label": a.label,
                    "child": a.child,
                    "attached": len(attach),
                    "ansi": bool(a.ansi),
                    "offered_MBps_total": (
                        0.0
                        if a.child == "idle"
                        else round(per_session_bps * a.sessions / 1e6, 3)
                    ),
                    "cpu_percent_of_one_core": round(100 * (c1 - c0) / wall, 3),
                    "rss_MB_start": round(r0 / 1024, 1),
                    "rss_MB_end": round(r1 / 1024, 1),
                    "client_MBps": round((b1 - b0) / wall / 1e6, 3),
                    "ctxt_switches_voluntary": s1[0] - s0[0],
                    "ctxt_switches_nonvoluntary": s1[1] - s0[1],
                    "switches_per_second": round((s1[0] - s0[0] + s1[1] - s0[1]) / wall, 2),
                    "window_s": round(wall, 2),
                }
            )
        )
    finally:
        daemon.terminate()
        try:
            daemon.wait(5)
        except subprocess.TimeoutExpired:
            daemon.kill()


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("binary")
    p.add_argument("label")
    p.add_argument("--sessions", type=int, default=20)
    p.add_argument("--kbps", type=int, default=400)
    p.add_argument("--child", choices=["stream", "idle"], default="stream")
    p.add_argument("--attach", choices=["all", "one", "none"], default="one")
    p.add_argument("--seconds", type=float, default=10.0)
    p.add_argument("--warmup", type=float, default=2.0)
    p.add_argument("--ansi", type=int, default=1)
    p.add_argument("--protocol", type=int, default=1)
    asyncio.run(main(p.parse_args()))
