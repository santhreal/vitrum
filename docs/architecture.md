# Architecture

vitrum is one process holding a window, talking to a second process holding the
sessions. The window is a Dioxus shell around a native GPU terminal pane. There
is one escape-sequence parser, and it is in Rust.

## The tree

```
app/               the window: sidebar, terminal panes, dialogs, settings
crates/
  vitrum-proto     the wire protocol, shared by the client and the daemon
  vitrum-core      PTY sessions, scrollback, the process registry
  vitrum-server    the daemon: sessions, search, collision detection
  vitrum-model     sidebar ordering, dispositions, time
  vitrum-fmt       formatting shared between surfaces
  vitrum-os        notifications, paths, single instance, theme, badge
  vitrum-search    scrollback search
  vitrum-grid      terminal cells: the grid model, and a wgpu renderer
  vitrum-vt        the VT engine, backed by libghostty
  vitrum-replay    seekable replay over captured bytes, and the replay binary
  vitrum-bench     measurement harnesses
vendor/            a patched dioxus-desktop; see Cargo.toml [patch.crates-io]
vendor-pty/        a patched portable-pty
vendor-ghostty-vt-sys/
                   a patched libghostty-vt-sys, pinning the instruction set
```

Three binaries ship: `vitrum`, `vitrum-server`, and `vitrum-replay`.

## The two processes

`vitrum-server` owns every PTY. It listens on loopback, speaks protocol
version 3, and requires a per-user token on every connection. Sessions outlive
the window because they are children of the daemon and not of the window. They
do not outlive the daemon.

`vitrum` is the window. It connects to the daemon over a WebSocket, one socket
per session, and it holds no PTY of its own. `docs/remote.md` describes running
the daemon on another machine, which is the same arrangement over a tunnel.

## The pane

A pane is a `GtkDrawingArea` given its own X window with
`gdk_window_ensure_native`. That window's XID carries a wgpu surface, so a
Vulkan swapchain lives inside the same GTK toplevel as the shell, with no
offscreen copy and no compositing pass between the two. Bytes arrive from the
daemon, `vitrum-vt` feeds them to libghostty, libghostty maintains the screen,
and `vitrum-grid` uploads the changed cells and draws them.

The pane is X11. Wayland has no equivalent of an XID handed to a child widget,
and the subsurface arrangement that replaces it is not written yet.

Three things follow from the parser being in the client's own address space:

- OSC 7 reaches the sidebar. The working directory an agent reports is a value
  the same process reads, so a row, its branch and its worktree follow an agent
  that changes directory. A prompt boundary sequence is readable on the same
  terms; nothing consumes one yet.
- A keystroke crosses no bridge. It is encoded from a gdk event and written to
  the socket.
- A frame is scheduled against the swapchain rather than against a document
  layout pass.

## No JavaScript

The pane was an emulator written in JavaScript, running inside the WebKit view.
That arrangement had two escape-sequence parsers for one product, held the
working directory and the prompt boundary inside a renderer addon where nothing
else could read them, and gave the frame budget to a DOM layout pass.

Nothing in this repository is a script now, and nothing writes a script element
into a document. `app/src/tests/no_javascript.rs` enumerates the tracked tree
and fails on either. The shell is still drawn by the system webview, WebKitGTK
on Linux and WebView2 on Windows, from markup and CSS the Rust in `app/`
produces.

## The forks

`vendor/` is why twenty windows share one web process. It exposes WebKit's
`webkit_web_view_new_with_related_view`, which upstream wry has and
dioxus-desktop did not. It also paints the window as soon as it is built rather
than when the event loop starts, and hands WebKit its background colour through
the call that takes 0.0 to 1.0 channels rather than the one that clamps 0-255
to white.

`vendor-ghostty-vt-sys/` is why a release runs on a pre-Haswell CPU. The
upstream build script passes `-Dtarget` to zig only when cross-compiling, so a
native build compiles for the builder's own CPU, and the release path emitted
AVX2: a bare SIGILL on everything before Haswell. The fork pins `-Dtarget` and
`-Dcpu=baseline` on all four targets. `[patch.crates-io]` does not reach a
registry build, so `cargo install vitrum` still compiles against the upstream
script.

`vendor-pty/` is a patched `portable-pty`.

Each fork records what it changed in its own `UPSTREAM.toml`.
`tools/upstream/check.sh` diffs it against the published crate.

## Where the grid is shared

`vitrum-grid` holds the cell model and the renderer. Two things draw from it:
the pane, and `vitrum-replay`, which reconstructs a screen from captured bytes.
Both run the same parser, so a replay and a live session agree on how many
columns a character takes.

Development, testing and the fork policy are in
[CONTRIBUTING.md](../CONTRIBUTING.md).
