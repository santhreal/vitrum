# vitrum-vt

The escape-sequence engine. Bytes from a PTY go in, a `vitrum_grid::CellGrid`
comes out. No window, no event loop, no PTY, and no renderer.

The state machine is libghostty's VT, the one shipped in the Ghostty terminal,
linked through `libghostty-vt-sys` and wrapped here in a safe Rust surface.
This crate is the only parser in the product. The daemon, the replay tool, the
benchmark harness and the live pane all drive this implementation, so a
replayed screen and a live one cannot disagree.

```rust
use vitrum_grid::{CellGrid, Style};
use vitrum_vt::{Vt, VtOptions};

let mut vt = Vt::new(VtOptions { cols: 80, rows: 24, ..VtOptions::default() })?;
let mut grid = CellGrid::new(80, 24, Style::DEFAULT)?;

vt.feed(b"\x1b[1;32mready\x1b[0m");
let stats = vt.sync(&mut grid)?;
assert!(!stats.is_noop());

// Nothing was fed since, so the engine reports no dirty row and the sync
// touches no cell.
assert!(vt.sync(&mut grid)?.is_noop());
```

`sync` is the damage contract. It reads only the rows the engine reports dirty
and writes only the cells whose value differs, so an idle terminal returns
`SyncStats::is_noop` and the host presents no frame. A frame is presented
because something changed, never because a clock ticked.

`Vt` is not `Send`. libghostty runs its callbacks on the thread that calls
`feed`, so a session belongs to the thread that created it. One session per
thread is the intended shape.

What a host reads back:

- `events()` for what the program announced: title, working directory,
  clipboard, bell, and the hyperlinks in `linkage`.
- `drain_pty_write()` for the bytes the program asked the terminal to send
  back, such as a device attributes reply.
- `cursor()` for position, shape, blink and visibility.
- `mode()` for how to encode input. Bracketed paste, DECCKM and the six mouse
  protocols each turn the same key, paste or click into different bytes.

`COLORTERM` is a constant here rather than in whichever crate sets the
environment, because this is the crate that either reproduces a 24-bit colour
or does not. A test feeds every channel value through the engine and asserts
the cell comes back exact, so weakening the renderer and weakening the string
fail together.

Building requires Zig, which `libghostty-vt-sys` uses to compile the vendored
sources. Set `LIBGHOSTTY_VT_SYS_OPTIMIZE` to pick the Zig optimisation mode.

Part of [vitrum](https://github.com/santhreal/vitrum). MIT licensed.
