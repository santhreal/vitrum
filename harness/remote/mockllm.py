#!/usr/bin/env python3
"""A deterministic, free, local stand-in for a streaming LLM API.

The memory comparison between vitrum and T3 Code is only meaningful if both
products run the identical workload, and a real provider gives neither: the
token text varies, the pacing varies with the network, and it costs money per
run. This serves the two wire shapes the two products speak, an
OpenAI-compatible `POST /v1/chat/completions` and an Anthropic-compatible
`POST /v1/messages`, so the choice of API cannot change what is measured.

Determinism comes from the seed plus the request body, never from a counter.
Thirty-two sessions stream at once and their arrival order is whatever the
scheduler decides, so a per-request ordinal would make the same run produce
different text every time. Hashing the prompt gives each session a stable
stream no matter when it lands.

usage: mockllm.py --port N [--tokens-per-second R] [--response-tokens T]
                  [--seed S]
"""

import argparse
import hashlib
import json
import random
import signal
import socket
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# A closed vocabulary keeps responses looking like prose while staying inside
# ASCII, so captured bytes stay comparable across hosts and locales.
WORDS = (
    "the parser reads a frame and yields the next token to the terminal "
    "while the session buffer grows until the scrollback trims it back down "
    "again so memory stays flat under load and the window keeps redrawing "
    "without tearing or dropping a single column of output on resize"
).split()

STATS_LOCK = threading.Lock()
STATS = {"requests": 0, "tokens": 0}


def count(tokens):
    with STATS_LOCK:
        STATS["requests"] += 1
        STATS["tokens"] += tokens


def tokens_for(seed, body, response_tokens):
    """Pick this request's tokens from the seed and the prompt it carries.

    A trailing space on every token but the last keeps the joined text a
    normal sentence, which is what exercises the client's line wrapping.
    """
    digest = hashlib.sha256(json.dumps(body, sort_keys=True).encode()).digest()
    rng = random.Random(f"{seed}:{digest.hex()}")
    picked = [rng.choice(WORDS) for _ in range(response_tokens)]
    return [w + " " for w in picked[:-1]] + picked[-1:] if picked else []


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    args = None

    def log_message(self, fmt, *a):
        # Silence per-request logging. The benchmark opens thousands of these
        # and the writes would show up in the measurement as the mock's own
        # cost rather than the product's.
        pass

    def do_GET(self):
        if self.path == "/healthz":
            self.send_bytes(200, b"ok", "text/plain")
        elif self.path == "/stats":
            with STATS_LOCK:
                payload = json.dumps(dict(STATS)).encode()
            self.send_bytes(200, payload, "application/json")
        else:
            self.send_bytes(404, b"not found", "text/plain")

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            body = json.loads(raw or b"{}")
        except ValueError:
            self.send_bytes(400, b'{"error":"bad json"}', "application/json")
            return
        if self.path.rstrip("/") == "/v1/chat/completions":
            frames = self.openai_frames
        elif self.path.rstrip("/") == "/v1/messages":
            frames = self.anthropic_frames
        else:
            self.send_bytes(404, b'{"error":"no such route"}', "application/json")
            return
        if not body.get("stream"):
            self.send_bytes(400, b'{"error":"stream must be true"}', "application/json")
            return

        tokens = tokens_for(self.args.seed, body, self.args.response_tokens)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        self.close_connection = True
        sent = 0
        try:
            for chunk, paced in frames(tokens):
                if paced:
                    self.pace()
                self.wfile.write(chunk)
                self.wfile.flush()
                if paced:
                    sent += 1
        except (BrokenPipeError, ConnectionResetError):
            pass
        count(sent)

    def pace(self):
        """Hold the token rate to a monotonic schedule, not a flat sleep.

        Sleeping a fixed interval per token makes the effective rate depend on
        how long the handler itself took, so a busy host would quietly stream
        slower and the two products would no longer face the same workload.
        Tracking an absolute deadline keeps the rate the number that was asked
        for, and lets a handler that fell behind catch up instead of drifting.
        """
        interval = 1.0 / self.args.tokens_per_second
        now = time.monotonic()
        deadline = getattr(self, "_deadline", None)
        if deadline is None:
            deadline = now
        deadline += interval
        if deadline > now:
            time.sleep(deadline - now)
        elif now - deadline > interval:
            # Fell far enough behind that catching up would burst. Resync so
            # the remaining tokens are paced rather than dumped.
            deadline = now
        self._deadline = deadline

    def openai_frames(self, tokens):
        ident = "chatcmpl-mock"
        head = {
            "id": ident,
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "mockllm",
            "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": None}],
        }
        yield self.sse(head), False
        for token in tokens:
            frame = {
                "id": ident,
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "mockllm",
                "choices": [
                    {"index": 0, "delta": {"content": token}, "finish_reason": None}
                ],
            }
            yield self.sse(frame), True
        tail = {
            "id": ident,
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "mockllm",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        }
        yield self.sse(tail), False
        yield b"data: [DONE]\n\n", False

    def anthropic_frames(self, tokens):
        start = {
            "type": "message_start",
            "message": {
                "id": "msg_mock",
                "type": "message",
                "role": "assistant",
                "model": "mockllm",
                "content": [],
                "stop_reason": None,
                "usage": {"input_tokens": 0, "output_tokens": 0},
            },
        }
        yield self.sse(start, "message_start"), False
        block_start = {
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""},
        }
        yield self.sse(block_start, "content_block_start"), False
        for token in tokens:
            frame = {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": token},
            }
            yield self.sse(frame, "content_block_delta"), True
        yield self.sse({"type": "content_block_stop", "index": 0}, "content_block_stop"), False
        delta = {
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": len(tokens)},
        }
        yield self.sse(delta, "message_delta"), False
        yield self.sse({"type": "message_stop"}, "message_stop"), False

    @staticmethod
    def sse(payload, event=None):
        text = "" if event is None else f"event: {event}\n"
        return (text + "data: " + json.dumps(payload) + "\n\n").encode()

    def send_bytes(self, status, payload, content_type):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    # The benchmark opens thirty-two sessions at once and each holds its
    # connection open for the whole stream, so the default backlog of five
    # would turn a concurrent load into a queue and measure the wrong thing.
    request_queue_size = 128

    def server_bind(self):
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        super().server_bind()


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--tokens-per-second", type=float, default=40.0)
    parser.add_argument("--response-tokens", type=int, default=120)
    parser.add_argument("--seed", type=int, default=0)
    args = parser.parse_args()
    if args.tokens_per_second <= 0:
        parser.error("--tokens-per-second must be positive")
    if args.response_tokens < 0:
        parser.error("--response-tokens must not be negative")

    Handler.args = args
    server = Server(("127.0.0.1", args.port), Handler)
    print(f"mockllm listening {server.server_address[1]}", flush=True)

    def stop(_signum, _frame):
        # Shut down from another thread: serve_forever will not return from
        # inside its own handler. Leaving the port bound would fail the next
        # scenario with an address already in use.
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        server.serve_forever(poll_interval=0.1)
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
