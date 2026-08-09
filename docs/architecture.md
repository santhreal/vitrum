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

`vitrum-grid` reaches the shipped build through `vitrum-replay`, which uses its
cell grid to reconstruct a screen. The wgpu renderer in the same crate is not
reachable from any surface: the window draws terminals with xterm.js. The
renderer exists for a later move to Dioxus Native, which paints through Blitz
and cannot carry JavaScript.

Development, testing and the fork policy are in
[CONTRIBUTING.md](../CONTRIBUTING.md).
