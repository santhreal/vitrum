#!/usr/bin/env python3
"""Create sessions on a running vitrum-server and print their ids.

usage: sessions.py <count> <cwd> <command> [args...]

The measurement runs need N real sessions before they open N windows, and the
honest way to get them is the wire protocol the client itself speaks, not a
fixture. `crates/vitrum-proto/src/lib.rs` tags `ClientMsg` with `t` and renames
both variants and fields to camelCase, so `CreateSession { project_id, .. }`
goes out as `{"t":"createSession","projectId":...}`.

The WebSocket framing here is RFC 6455 written against the standard library
rather than a dependency, because the measurement host is not a development
box and should not need one installed to answer a question about memory. It
handles exactly what this conversation contains: a text message out, text
messages in, fragmentation, and ping.
"""

import base64
import hashlib
import json
import os
import socket
import struct
import sys

GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
# Must equal `vitrum_proto::PROTOCOL_VERSION`. The daemon refuses any other
# number outright, so a bump that misses this file makes every measurement run
# fail at the handshake. `harness_protocol.rs` asserts the two agree.
PROTOCOL_VERSION = 2


class Ws:
    def __init__(self, host, port, path="/", timeout=15.0):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.buf = b""
        key = base64.b64encode(os.urandom(16)).decode()
        request = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {host}:{port}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            "\r\n"
        )
        self.sock.sendall(request.encode())
        while b"\r\n\r\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("server closed the connection during the handshake")
            self.buf += chunk
        head, self.buf = self.buf.split(b"\r\n\r\n", 1)
        lines = head.decode("latin-1").split("\r\n")
        if "101" not in lines[0]:
            raise RuntimeError(f"upgrade refused: {lines[0]}")
        # The accept hash is checked rather than assumed. A proxy that answers
        # 101 without understanding WebSocket would otherwise look like a
        # working daemon right up until the first frame came back as garbage.
        want = base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()
        got = None
        for line in lines[1:]:
            name, _, value = line.partition(":")
            if name.strip().lower() == "sec-websocket-accept":
                got = value.strip()
        if got != want:
            raise RuntimeError(f"bad Sec-WebSocket-Accept: {got!r}")

    def _read(self, n):
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise RuntimeError("server closed the connection")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def _send_frame(self, opcode, payload):
        mask = os.urandom(4)
        n = len(payload)
        header = bytes([0x80 | opcode])
        if n < 126:
            header += bytes([0x80 | n])
        elif n < 65536:
            header += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            header += bytes([0x80 | 127]) + struct.pack(">Q", n)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(header + mask + masked)

    def send_json(self, obj):
        self._send_frame(0x1, json.dumps(obj).encode())

    def recv(self):
        """One complete message as (opcode, payload).

        Control frames may sit between the fragments of a data message, so a
        ping is answered and skipped without disturbing the reassembly.
        """
        parts = []
        first_opcode = None
        while True:
            b0, b1 = self._read(2)
            fin = b0 & 0x80
            opcode = b0 & 0x0F
            masked = b1 & 0x80
            length = b1 & 0x7F
            if length == 126:
                length = struct.unpack(">H", self._read(2))[0]
            elif length == 127:
                length = struct.unpack(">Q", self._read(8))[0]
            mask = self._read(4) if masked else None
            data = self._read(length) if length else b""
            if mask:
                data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
            if opcode == 0x9:
                self._send_frame(0xA, data)
                continue
            if opcode == 0xA:
                continue
            if opcode == 0x8:
                raise RuntimeError("server closed the websocket")
            if opcode != 0x0:
                first_opcode = opcode
            parts.append(data)
            if fin:
                return first_opcode, b"".join(parts)

    def wait_for(self, kind):
        """The next JSON message tagged `kind`, skipping anything else.

        A `close` from the daemon and a `{"t":"error"}` are both reported
        rather than waited through, because the alternative is a harness that
        hangs for the full ssh timeout when the daemon has already said no.
        """
        while True:
            opcode, payload = self.recv()
            if opcode != 0x1:
                continue
            msg = json.loads(payload)
            if msg.get("t") == kind:
                return msg
            if msg.get("t") == "error":
                raise RuntimeError(f"daemon error: {msg}")

    def close(self):
        try:
            self._send_frame(0x8, b"")
        except OSError:
            pass
        self.sock.close()


def connect():
    port = int(os.environ.get("VITRUM_PORT", "7737"))
    ws = Ws("127.0.0.1", port)
    ws.send_json({"t": "hello", "protocol": PROTOCOL_VERSION})
    welcome = ws.wait_for("welcome")
    if welcome.get("protocol") != PROTOCOL_VERSION:
        raise RuntimeError(f"daemon speaks protocol {welcome.get('protocol')}")
    return ws


def cmd_create(count, cwd, command, args):
    ws = connect()
    for i in range(count):
        ws.send_json(
            {
                "t": "createSession",
                "projectId": 1,
                "cwd": cwd,
                "command": command,
                "args": args,
                "cols": 120,
                "rows": 32,
                "title": f"rig-{i + 1:02d}",
            }
        )
        created = ws.wait_for("sessionCreated")
        print(created["id"], flush=True)
    ws.close()


def cmd_count():
    """How many sessions the daemon holds, one number on stdout.

    This exists because "twenty windows" and "twenty windows each showing its
    OWN session" are different measurements, and GOAL.md records mistaking the
    second for the first: a 1059.2 MB result had fewer sessions than windows,
    so several windows showed the same session and the figure was not the
    workload it claimed. Counting the windows cannot catch that. Counting both
    can.
    """
    ws = connect()
    ws.send_json({"t": "list"})
    sessions = ws.wait_for("sessions")
    print(len(sessions.get("sessions", [])), flush=True)
    ws.close()


def main(argv):
    if len(argv) >= 2 and argv[1] == "count":
        cmd_count()
    elif len(argv) >= 4:
        cmd_create(int(argv[1]), argv[2], argv[3], argv[4:])
    else:
        sys.exit(
            "usage: sessions.py <count> <cwd> <command> [args...]\n"
            "       sessions.py count"
        )


if __name__ == "__main__":
    main(sys.argv)
