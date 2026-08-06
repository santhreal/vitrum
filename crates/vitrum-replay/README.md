# vitrum-replay

Seekable replay over a terminal session's output bytes, plus asciicast v2 import
and export.

A session's scrollback is already a timeline. Every chunk read from the PTY is
numbered by `seq`, the cumulative byte offset of that chunk in the whole output
stream, and those numbers never restart. So "what did the screen look like 40
KiB ago" is answerable exactly, and nobody had to record anything to make it
answerable. This crate turns that property into a scrubber.

Nothing here talks to a daemon, a socket, or a UI. The whole input is a byte
stream plus the seq its first byte carries.

```rust
use vitrum_replay::{Replay, ReplayConfig, Stream};

let bytes: &[u8] = b"one\r\ntwo\r\nthree";
let stream = Stream::new(0, std::slice::from_ref(&bytes));
let mut replay = Replay::build(stream, &ReplayConfig::new(10, 3)?)?;

replay.seek(stream.head_seq())?;
assert_eq!(replay.screen().line(2).trim_end(), "three");

// Back to just after "one\r\n". Row 1 has not been written yet.
replay.seek(5)?;
assert_eq!(replay.screen().line(0).trim_end(), "one");
assert_eq!(replay.screen().line(1).trim_end(), "");
```

Seeking is cheap because a keyframe index snapshots the screen every `stride`
bytes during one linear build pass, and a keyframe is only taken where the VT
parser is provably back in its ground state. A seek restores the newest keyframe
at or before the target and feeds at most `stride` bytes from there.

This crate does not emulate a terminal from scratch: the cell grid is
[`vitrum-grid`](https://crates.io/crates/vitrum-grid) and the byte-level state
machine is [`vte`](https://crates.io/crates/vte). What lives here is the layer
between them.

Part of [vitrum](https://github.com/santhreal/vitrum). MIT licensed.
