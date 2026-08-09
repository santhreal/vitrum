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
```

Three binaries ship: `vitrum`, `vitrum-server`, and `vitrum-replay`.

`vendor/` is why twenty windows share one web process. It exposes WebKit's
`webkit_web_view_new_with_related_view`, which upstream wry has and
dioxus-desktop did not surface.

The data plane is Rust. `app/src/socket.rs` opens the session socket and owns
reconnection, sequence continuity, the backlog splice and reassembly of a
character split across two frames. The webview receives decoded pane
operations and renders them, so the wire format has one decoder rather than
one in `vitrum-proto` and a second in JavaScript.

`vitrum-grid` reaches the shipped build through `vitrum-replay`, which uses its
cell grid to reconstruct a screen, and it agrees with libghostty about how many
columns a character takes because the tests take their samples from the engine.
The wgpu renderer in the same crate does not yet paint a session: the window
draws terminals with xterm.js. `app/src/pane` is the replacement — a native GTK
drawing area, its XID, and a wgpu surface on it — behind the `native-pane`
feature until it can host a session, and the reason to make that move is one
parser and OSC 7 and OSC 133 semantics a webview cannot have, not frame rate.

Development, testing and the fork policy are in
[CONTRIBUTING.md](../CONTRIBUTING.md).
