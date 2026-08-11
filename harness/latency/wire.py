"""A minimal client for the vitrum daemon's WebSocket protocol.

Enough of RFC 6455 to speak the control plane: a masked text frame out, an
unmasked frame in, ping answered. No dependency, because the host this runs on
is a bare measurement rig and a probe that needs a package index installed is a
probe that does not run there.

Protocol version 3. Messages are the `t`-tagged, camelCase JSON of
`vitrum-proto`.
"""

import base64
import json
import os
import secrets
import socket
import struct

PROTOCOL_VERSION = 3

OP_TEXT = 0x1
OP_BINARY = 0x2
OP_CLOSE = 0x8
OP_PING = 0x9
OP_PONG = 0xA


def token_path():
    """Where the daemon writes the token this account may read."""
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    if not runtime:
        raise SystemExit("XDG_RUNTIME_DIR is unset, so the token has no home")
    return os.path.join(runtime, "vitrum", "token")


class Wire:
    """One connection to the daemon."""

    def __init__(self, host="127.0.0.1", port=7838, token=None, timeout=15.0):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.buf = b""
        key = base64.b64encode(secrets.token_bytes(16)).decode()
        request = (
            f"GET /ws HTTP/1.1\r\n"
            f"Host: {host}:{port}\r\n"
            f"Upgrade: websocket\r\n"
            f"Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            f"Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(request.encode())
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise SystemExit("the daemon closed the connection during the handshake")
            head += chunk
        status = head.split(b"\r\n", 1)[0]
        if b"101" not in status:
            raise SystemExit(f"the daemon refused the upgrade: {status!r}")
        self.buf = head.split(b"\r\n\r\n", 1)[1]

        if token is None:
            with open(token_path()) as fh:
                token = fh.read().strip()
        self.send({"t": "hello", "protocol": PROTOCOL_VERSION, "token": token})
        welcome = self.recv()
        if welcome.get("t") != "welcome":
            raise SystemExit(f"expected a welcome, got {welcome}")
        self.welcome = welcome

    def send(self, message):
        payload = json.dumps(message).encode()
        header = bytearray([0x80 | OP_TEXT])
        mask = secrets.token_bytes(4)
        length = len(payload)
        if length < 126:
            header.append(0x80 | length)
        elif length < (1 << 16):
            header.append(0x80 | 126)
            header += struct.pack(">H", length)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", length)
        header += mask
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(bytes(header) + masked)

    def _read(self, count):
        while len(self.buf) < count:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise SystemExit("the daemon closed the connection")
            self.buf += chunk
        out, self.buf = self.buf[:count], self.buf[count:]
        return out

    def recv(self):
        """The next JSON message, answering pings on the way."""
        while True:
            first, second = self._read(2)
            opcode = first & 0x0F
            length = second & 0x7F
            if length == 126:
                (length,) = struct.unpack(">H", self._read(2))
            elif length == 127:
                (length,) = struct.unpack(">Q", self._read(8))
            if second & 0x80:
                mask = self._read(4)
                body = bytes(b ^ mask[i % 4] for i, b in enumerate(self._read(length)))
            else:
                body = self._read(length)
            if opcode == OP_PING:
                self.sock.sendall(bytes([0x80 | OP_PONG, 0x80, 0, 0, 0, 0]))
                continue
            if opcode == OP_CLOSE:
                raise SystemExit("the daemon closed the connection")
            if opcode in (OP_TEXT, OP_BINARY):
                return json.loads(body)

    def until(self, tag, limit=200):
        """The next message tagged `tag`, skipping the ones before it."""
        for _ in range(limit):
            message = self.recv()
            if message.get("t") == tag:
                return message
        raise SystemExit(f"no {tag} arrived in {limit} messages")

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass
