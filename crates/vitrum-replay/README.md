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

A forward seek feeds the bytes between where the replay is and where it is going.
A backward seek builds a fresh terminal and replays the stream from its first byte,
so its cost tracks the target's distance from the start rather than the distance
moved. There is no keyframe index: the terminal is a state machine with no readable
state and no clone, so a checkpoint would have to be a parked engine, and advancing a
parked engine consumes it. Refilling one cascades into every earlier checkpoint and
costs exactly what rebuilding from the start costs.

This crate does not emulate a terminal from scratch: the cell grid is
[`vitrum-grid`](https://crates.io/crates/vitrum-grid) and the terminal is
[`vitrum-vt`](https://crates.io/crates/vitrum-vt), which wraps Ghostty's VT
implementation. What lives here is the layer between them. It is the same terminal
that paints a live session, so a replay and the pane agree on every byte.

Part of [vitrum](https://github.com/santhreal/vitrum). MIT licensed.
