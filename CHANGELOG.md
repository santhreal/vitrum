# Changelog

Notable changes per release. Versions follow [semver](https://semver.org);
before 1.0 a minor bump may break things, and this file says when it does.

## Unreleased

### Added

- **Claude Code can now declare Approval, so the sidebar shows it.**
  `integrations/claude-code` ships a hook, the `settings.json` that calls it
  and the event mapping. Approval and Input cannot be observed from a pty, so
  they only appear when an agent declares them, and a hook could not declare
  anything: Claude Code owns the hook's stdout and runs it with no controlling
  terminal, so there was nowhere to write the sequence. The hook finds the pty
  by walking its own ancestors. Linux only, because it reads `/proc`.

### Fixed

- **Terminal and Keyboard settings now take effect in every open window.**
  Text scale, terminal font and renderer, terminal opacity and the key
  bindings are pushed into the webview as a script, and the push ran in the
  document of whichever window the sheet was open in. Every other window kept
  its old font, scrollback, renderer and chords until it was next opened,
  which made four controls quietly window-local while the rest of the sheet
  was global. Each window now subscribes to the change and applies it in its
  own document.
- **Escape on What's New (and onboarding) now records the sheet as seen.** Closing with the button or the backdrop already did; Escape only cleared the layer, so the notes could return on the next launch.
- **A second window no longer kills the process.** Opening window two panicked
  with `DuplicateCustomProtocol("vitrum-backdrop")`: every webview is built
  from one shared `WebContext`, a custom scheme belongs to that context rather
  than to the webview, and the scheme was being registered again for each
  window. It is registered once per process now.
- **The measurement harness connects again.** It still asked for wire protocol
  1 after the daemon moved to 2, so every run failed at the handshake and
  created no sessions. A test now asserts the two agree.

### Changed

- **First launch now walks through the product, not just the machine.**
  Onboarding was one screen of three derived rows: is the daemon up, what is
  on your PATH, how to start a session. Everything that makes this different
  from a terminal with tabs — that the sidebar is an inbox, that a row's
  colour is its agent's state, that one chord jumps to whichever agent wants
  you, that sessions outlive the window, that workspaces and the three bands
  exist at all — was discoverable only by accident. It is now four short
  pages: what this machine has, then the inbox, then workspaces, then the
  keyboard and search. Every keystroke it teaches is looked up in the live
  keymap at render time, so a rebind cannot leave it teaching a dead key, and
  a guard rejects any chord-shaped text on any page that the keymap does not
  claim. It still animates nothing, holds no timer, is skippable from every
  page, and does not come back.
- **A quiet sidebar now costs nothing as time passes.** The clock was floored
  to a whole second, which stopped rows rebuilding within a second and left
  every row rebuilding on every second boundary, forever. A row reading
  `5h ago` repeats that answer 3600 times before one character changes, so at
  twenty sessions that was twenty row rebuilds a second for nothing. Each row
  now gets a clock floored to the coarsest instant it cannot tell apart from
  now, taken from whichever of its label or its state changes soonest. Sixty
  second-boundaries over twenty settled rows rebuild nothing at all, measured.
  Rows with a live timer, a countdown to a wake, or a pending auto-settle keep
  a per-second clock and update exactly as before.
- **First launch opens the walkthrough while the daemon is still starting.**
  Agent detection used to finish its PATH walk before the sheet appeared and
  before the connect began, so the two costs added. The sheet opens
  immediately, the walk runs beside the connect, and a still-running walk
  says it is looking rather than claiming nothing matched.
- **What's New matches the onboarding sheet's shape:** an intro line under
  the title and a header Dismiss control, so the notes are framed and
  closable the same way as the first-run walkthrough.
- **Dual licensed MIT OR Apache-2.0**, from MIT alone. The Apache half carries
  an explicit patent grant. The vendored forks under `vendor/` and
  `vendor-pty/` keep the MIT license and copyright they arrived with.
- **The README's performance figures are generated**, from a checked-in
  snapshot of real harness runs, and CI fails when a table drifts from it. The
  `~325 MB` and `0.22%` recorded under v0.1.0 below are superseded: they could
  not have described twenty windows on that build, which crashed on the second
  one. Measured on the current tree, twenty windows are 460.1 MB PSS in three
  client processes, 11.2 MB per extra window, and 0.1% of one core at idle over
  a minute with no memory drift.

### Added

- **A quiet titlebar chip when a newer release is available.** It appears after
  a background check, opens Settings → About with Install already seeded, and
  can be dismissed for that exact version so it does not nag on every launch.
- `install.sh` and `install.ps1`: the release install as a file you can read,
  with the archive verified against the release `SHA256SUMS` and no install on
  a mismatch. Nothing is piped into a shell.
- `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `AUTHORS`, `NOTICE`,
  issue forms, a pull request template, and Dependabot.
- A `cargo-deny` policy and the CI job that enforces it, so a dependency whose
  license conflicts with the dual license fails the build.
- `make`: `gate`, `measure`, `readme-perf`, `readme-perf-check`, `package`.

## v0.1.0 - 2026-08-05

First public release. Pre-1.0: it runs, it is used daily on Linux, and the gaps
below are stated rather than discovered.

### The product

- **A terminal for many coding agents.** Any TUI agent in a real PTY, with no
  per-agent integration: Claude Code, Codex, Gemini CLI, opencode, veyyon, or a
  plain shell.
- **A sidebar that says who is doing what.** Each session draws its agent as a
  distinct mark, its status, and how long since it last spoke. Grouped by
  filesystem directory or by folders you name, per workspace.
- **Same-file collision detection.** Two live sessions writing the same file
  are both flagged. Not the same repository and not the same directory: only
  the file, because a warning that fires on shared checkouts is one you mute.
- **Sessions outlive the window.** The daemon owns the PTYs. Close everything
  and your agents keep running, scrollback intact.
- **One web process for every window.** Twenty windows measured at ~325 MB
  total and 0.22% of one core at idle, headless with software rendering.
- **Saved commands with your own shortcuts.** Save the invocation you actually
  use with the directory it belongs in; bind a key that fires from anywhere.
- **Cross-session scrollback search.** One daemon-side sweep over every
  session's retained output, which no client can do for itself.
- **Workspaces** that are genuinely separate: a new one opens onto nothing.
- **A tray icon carrying the attention count**, with show/hide, a new session
  and quit. The taskbar badge shows the same number.
- **Keybinds you can rewrite.** Rebind any action, send literal text, or run an
  ordered sequence that branches on what the focused session is doing, what
  layer is open, or whether the workspace wants you.
- **`vitrum hint`**, one command a wrapper or a shell prompt calls to declare
  what a session is doing. Approval and Input cannot be observed from a PTY,
  because an agent asking to force-push and a shell at a prompt block in the
  same read; this is how those two states reach the sidebar.
- **A walkthrough on a fresh profile** built from what is on the machine, and
  the entries from this file after an update.
- **Recent commands and a chosen icon per saved command.** The launcher offers
  what you ran and where you ran it, which ranked history cannot express
  because it holds one directory per command.
- **Translucency and backdrops.** Independent window and terminal opacity, and
  a backdrop image inside the window with fit, blur and dim. Both opacities
  default to fully opaque and emit no CSS at all, so an install that never
  opens Appearance composites nothing. The seven named terminal palettes were
  already there; this is the surface behind them.

### Known gaps

- **Collision detection is Linux only.** On macOS and Windows it reports that
  this build has no watcher for the platform rather than reporting that nothing
  is wrong.
- **Attribution needs a file held open longer than an instant.** A write that
  opens, appends and closes within microseconds is counted as unattributed
  rather than guessed at. The count is published; it is never folded into a
  confident "nothing is colliding".
- **Only Linux is exercised end to end.** macOS and Windows compile and the
  platform code exists; neither is tested.
- **Blur is your compositor's job.** vitrum makes the window see-through;
  Hyprland, KWin and picom frost it, and README carries the rule for each. No
  application can blur what is behind its own window, and Wayland has no
  protocol to ask. Native frosting that needs no configuration, Mica and
  Acrylic on Windows and `NSVisualEffectView` on macOS, is not in this release.
- **No GPU terminal renderer.** Cells are drawn as DOM. `vitrum-grid` carries a
  wgpu renderer, but nothing in the window can reach it until Dioxus Native
  lands; today the crate reaches you only through `vitrum-replay`.

### Performance

- **Terminal history no longer crosses the wire as a JSON integer array.**
  `ScrollbackChunk` carries arbitrary PTY bytes, and serde's default for those
  is an array of decimal integers, measured at 3.5 bytes of JSON per payload
  byte on real output. It paid that twice, once from the daemon and again
  across the bridge into the webview. The size was the smaller half:
  `JSON.parse` had to build a JavaScript array before anything could copy it
  into the grid, and JavaScriptCore boxes every element, so a 2 MiB backfill
  allocated 46 MiB of resident memory for that intermediate alone in the
  process every window shares. History is base64 now: 1.33 bytes per payload
  byte, decoded about ten times faster, with nothing allocated beyond the
  buffer the grid receives.
- **The control-plane protocol version is 2.** A client and server that
  disagree already refuse each other with a message naming both versions. If
  an older daemon is still running after an upgrade, stop it and let the new
  client start its own.
- **Settings opens immediately on a Linux desktop with no working portal.**
  Reading the system theme goes to `org.freedesktop.portal.Settings`. If that
  name is registered on the session bus but nothing can start it, D-Bus does
  not answer: it waits out `service_start_timeout`, 120 seconds by default, and
  a read makes two calls. That ran on the thread drawing the sheet, so opening
  Settings froze for four minutes. The read is now bounded at five seconds and
  a portal that does not answer is reported as missing, which is what it is.

### Hardening

Found by running a daemon and feeding it hostile input, not by reading it.

- **Errors are bounded and cannot forge a line.** A 100,000 character command
  produced a 200,991 character error, and a directory or command name carrying
  a newline or a bidi override wrote its own line into the banner. Error text
  is now sanitised and capped, cut in the middle so both what failed and why
  survive. The wire variant is sealed so it cannot be built around.
- **A missing command says what to type instead.** The old message recited
  every entry of `PATH`, over a kilobyte, and answered nothing.
- **A repository cannot forge a sidebar row.** `.git/HEAD` is read directly, so
  a crafted or corrupt one used to reach the tooltip intact, and a multibyte
  one crashed the session on spawn.

### Updating

`vitrum update` installs the newest published release, verified against the
SHA-256 published beside it. The same code runs behind Settings, About. A
release that publishes no checksums is refused rather than trusted.

Two things it is honest about, because both cost you something:

- **The daemon is a separate process that outlives every window.** Updating
  replaces its file; the running process keeps serving the old version until it
  is restarted, and restarting it ends every session it is holding. About shows
  which version the live daemon is actually on.
- **A copy installed by something else is refused**, before the download rather
  than after it.

### No pictures

This release ships no images: no screenshots, no logo, no icons. There was a
mark, and a generator that built the SVG, the PNGs and the `.ico` from three
numbers, and a hero screenshot at the top of the README. All of it is gone.

The screenshots were the reason. Every one this project published showed a
shell, a build tool or the test fixture, which is an argument that vitrum is a
terminal multiplexer, and that is a category where tmux already wins and where
nothing this product does is visible. The mark went with them rather than
leaving a page whose only picture is a logo.

The rule about where a mark may appear survives, because it was always about
the window rather than about the file: the launcher and a loading screen, and
nowhere else inside the application. It is stated and enforced in
`app/src/update/where_the_mark_may_appear.rs`.

### Notes

`vendor/` carries a patched `dioxus-desktop`. It exposes WebKit's
`webkit_web_view_new_with_related_view`, which upstream wry has and
dioxus-desktop did not surface, and it is the reason every window shares one
web process instead of spending one each.
