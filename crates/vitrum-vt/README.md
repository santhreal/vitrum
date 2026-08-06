# vitrum-vt

Ghostty's terminal engine, driving a [`vitrum-grid`](../vitrum-grid) cell grid.
No window, no event loop, no renderer: bytes go in, a grid of cells comes out.

```rust
use vitrum_grid::{CellGrid, Style};
use vitrum_vt::{Vt, VtOptions};

let mut vt = Vt::new(VtOptions { cols: 20, rows: 3, max_scrollback: 0 })?;
let mut grid = CellGrid::new(20, 3, Style::DEFAULT)?;

vt.feed(b"\x1b[1;32mgreen\x1b[0m\r\n");
let stats = vt.sync(&mut grid)?;
assert_eq!(grid.row_text(0).unwrap().trim_end(), "green");

// Nothing arrived since, so the next frame changes nothing.
assert!(vt.sync(&mut grid)?.is_noop());
```

## Why Ghostty

The client renders terminals with xterm.js inside a webview, which costs a
JavaScript engine per session. Replacing it needs a VT implementation, and that
is not a weekend of work: DEC modes, scroll regions, reflow on resize, OSC
handling, grapheme clustering, and a decade of terminal quirks. `libghostty-vt`
is that implementation, extracted from Ghostty and shipped as a C library.

It also brings capabilities the webview path never had: OSC 7 working
directory, OSC 133 shell integration, semantic selection by word and by command
output, and scrollback that reflows when the window resizes.

## Replies are not optional

A VT stream contains questions. `Vt::drain_pty_write` returns the bytes the
terminal owes the program, and a host that never drains them hangs anything
that issues a device query, because the program is blocked reading an answer
that never arrives.

```rust
let mut reply = Vec::new();
vt.drain_pty_write(&mut reply);
if !reply.is_empty() {
    session.write(&reply); // back to the PTY
}
```

## How the engine is linked

Two routes, chosen by feature, decided in one place (`build.rs`) and readable
at runtime through `vitrum_vt::linkage`:

| Route | Feature | Needs | Engine |
| --- | --- | --- | --- |
| `vendored` | default | Zig 0.15.2 on `PATH` | the pinned upstream commit |
| `system` | `system` | an installed libghostty found by pkg-config | whatever the platform ships |

`system` is available on Linux and macOS. Windows has no pkg-config convention,
so it builds vendored and says so rather than offering a switch that silently
does something else.

A route that cannot be satisfied is a build error naming the missing piece and
the command that installs it. It never falls back to the other route: a machine
asked to link the platform's Ghostty must not quietly clone and compile a
different one instead.

`VITRUM_VT_LINKAGE=system|vendored` overrides the feature for one build.
`GHOSTTY_SOURCE_DIR` points a vendored build at a local Ghostty checkout.

```
$ cargo run -q --example version   # or wherever the host prints it
libghostty-vt 0.2.1 (vendored, zig, pinned upstream)
```

## Cost model

One engine allocation per session plus its scrollback. `sync` reads only the
rows the terminal reports as changed and writes through to the grid, which
records damage only where a value differs, so an idle terminal reports
`SyncStats::is_noop` and the renderer records no GPU work. Grapheme clusters are
read into a stack buffer: nothing is allocated per frame, per row, or per cell.

## Known limitation

A grid cell is 16 bytes and holds one `char`, so a grapheme cluster is stored as
its base codepoint. That is counted, not hidden: `SyncStats::graphemes_flattened`
reports how many cells in the frame are showing an approximation.

Part of [vitrum](https://github.com/santhreal/vitrum). MIT OR Apache-2.0.
