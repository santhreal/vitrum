# Architecture

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

`vendor/` is why twenty windows share one web process. It exposes WebKit's
`webkit_web_view_new_with_related_view`, which upstream wry has and
dioxus-desktop did not. It also paints the window as soon as it is built
rather than when the event loop starts, and hands WebKit its background colour
through the call that takes 0.0 to 1.0 channels rather than the one that
clamps 0-255 to white.

`vendor-ghostty-vt-sys/` is why a release runs on a pre-Haswell CPU. The
upstream build script passes `-Dtarget` to zig only when cross-compiling, so
a native build compiles for the builder's own CPU, and the release path
emitted AVX2: a bare SIGILL on everything before Haswell. The fork pins
`-Dtarget` and `-Dcpu=baseline` on all four targets. `[patch.crates-io]` does
not reach a registry build, so `cargo install vitrum` still compiles against
the upstream script.

The data plane is Rust. `app/src/socket.rs` opens the session socket and owns
reconnection, sequence continuity, the backlog splice and reassembly of a
character split across two frames. The webview receives decoded pane
operations and renders them, so the wire format has one decoder rather than
one in `vitrum-proto` and a second in JavaScript.

`vitrum-grid` reaches the shipped build through `vitrum-replay`, which uses its
cell grid to reconstruct a screen, and it agrees with libghostty about how many
columns a character takes because the tests take their samples from the engine.
The wgpu renderer in the same crate does not yet paint a session: the window
draws terminals with xterm.js. `app/src/pane` is the replacement, a native GTK
drawing area with its XID and a wgpu surface on it, behind the `native-pane`
feature until it can host a session. The native pane gives one parser, and
OSC 7 and OSC 133 semantics a webview cannot have. Frame rate is unchanged.

Development, testing and the fork policy are in
[CONTRIBUTING.md](../CONTRIBUTING.md).
