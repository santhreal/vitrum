# vitrum-proto

The wire contract between the vitrum session server and its clients, and
nothing else. Message types, the data-plane frame codec, and the display-safety
rules for text that crosses the boundary. No transport, no I/O, no policy.

The protocol runs two planes over one connection:

- **Control plane**: JSON text frames carrying `ClientMsg` and `ServerMsg`,
  versioned by `PROTOCOL_VERSION`.
- **Data plane**: binary frames carrying raw PTY bytes, through `encode_output`
  and `decode_output`.

The split exists because PTY output is arbitrary bytes. JSON strings must be
valid UTF-8, so routing output through the control plane would force base64 on
the hottest path in the product and would corrupt every byte sequence that is
not text.

```rust
use vitrum_proto::{SessionId, decode_output, encode_output};

let frame = encode_output(SessionId(7), 4096, b"\x1b[31mred\x1b[0m");
let (session, seq, payload) = decode_output(&frame).unwrap();
assert_eq!(session, SessionId(7));
assert_eq!(seq, 4096);
assert_eq!(payload, b"\x1b[31mred\x1b[0m");
```

Part of [vitrum](https://github.com/santhreal/vitrum). MIT licensed.
