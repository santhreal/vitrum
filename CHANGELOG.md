# Changelog

Notable changes per release. Versions follow [semver](https://semver.org);
before 1.0 a minor bump may break things, and this file says when it does.

## Unreleased

## v0.1.1 - 2026-08-09

### Added

- **The session socket lives in Rust.** The webview used to open the
  WebSocket, parse the 17-byte output header, track sequence numbers, splice
  backlog against buffered live frames and reassemble characters split across
  two frames. Every one of those is a protocol guarantee written twice, once
  in `vitrum-proto` and once in JavaScript, and two decoders for one wire
  format drift. Rust owns them now and the webview renders decoded pane
  operations.
- **Column widths come from the engine that lays out the pane.**
  `vitrum-grid` classified characters with its own copy of the East Asian
  Width tables while libghostty laid out the same characters, and a character
  the engine gave two columns and the grid counted as one shifted every later
  column on the line. The width tests now feed codepoints to libghostty and
  take their samples from what it reports, so the case list cannot go stale
  when the engine's Unicode data moves.
- **Fonts fall back.** A codepoint the primary face lacks resolves through a
  chain built from the font database, monospaced faces first. The chain is a
  pure function, so which face a character resolves through is answerable
  without a device or a rasterised glyph.
- **`make fast`** runs the narrowest gate for one crate.
- **A native terminal pane, behind the `native-pane` feature.** A GTK drawing
  area in the shell's own toplevel, its X11 window handed to wgpu, painted
  from `vitrum-grid`, with a toolkit-free key encoder. Off by default because
  nothing hosts it yet: input method, selection, clipboard, search, scrollback
  paging and Wayland are named in its module doc as the work between it and
  replacing xterm.js. The argument for it is one parser and OSC 7 and OSC 133
  semantics in Rust, not frame rate.
- **The installer answers for what a real machine does to it.** No `curl` and
  no `wget`, a proxy that needs a scheme, a download truncated mid-flight, a
  captive portal page where the archive should be, a `SHA256SUMS` with no line
  for this archive, an install directory it cannot write, a running `vitrum`
  in the way, a shell whose PATH syntax is not `export`, a second install over
  the first, and a missing system webview named with the package that supplies
  it on eight distributions. Uninstall reads a manifest and removes only what
  the installer wrote.
- **Pictures are gated by machine, not by review.** Every image in the tree is
  enumerated at run time and must be explained by a document; the description
  must name an agent and a state; and neither the description nor the prose
  around it may describe this product through a shell. An orphan image is a
  defect on its own, because unreferenced is how a banned one ships.
- **The JavaScript bill is published and capped.** Each remaining script is
  listed with its byte count and what it still does. The file set is read from
  the tree, so a new script is red until it is recorded, and a script that
  grows past its recorded size is red too.

### Fixed

- **Every failure says what to do, and exits with a code that means it.**
  `vitrum --bogus` printed usage to standard output and exited 0, so a
  wrapper could not tell a typo from a launch. Failures now name the fault
  and the correction, and exit through one shared table: `0` fine, `1`
  failed, `2` you typed something wrong, `3` fix the machine and retry, `4`
  the network is down so retry unchanged, `5` what arrived is not what was
  published. Both binaries render their `exit status:` help block from that
  table, and a test derives each command's codes from its own source, so a
  new failure returning an undocumented code turns the suite red.

- **A contiguous run of output no longer reports missing history.** The
  backlog splice measured every buffered frame against the resume offset,
  which is only the right question for the first one, so the second frame of
  any healthy run was announced to the operator as evicted history. A false
  hole is worse than a silent one: it says the transcript has bytes missing
  when it does not, and gives nobody a way to check.
- **A session with no title draws a whole row.** The fallback lives at the one
  owner rather than at the four call sites that each drew the blank: the row,
  its tip, the row menu and the notification.
- **The nightly tag never holds nothing.** The channel moved its tag before
  rebuilding the release, so between those steps the tag an installer resolves
  had no assets. Nightly now builds a complete staging draft, checks every
  expected asset is on it, and swaps in one rename.
- **Continuous integration runs at all.** Six of seven jobs asked for a
  self-hosted runner label nobody ever registered, and a label with no machine
  behind it does not fail: GitHub queues the job until it discards it a day
  later. Six per push accumulated into 233 unservable jobs that starved the
  servable ones, and the v0.1.0 release matrix died the same way on a retired
  macOS image, which is why that tag carries no assets. Labels now come from a
  repository variable that falls back to a hosted runner, and two guards — one
  in the pipeline, one in the test suite — refuse a label the project has not
  agreed on. The suite also parses every workflow, because a workflow that
  does not parse produces a run with zero jobs, no annotation and no log.
- **A tooltip no longer survives the row it belonged to.** A platform tooltip
  is anchored to the pointer rather than the element, so reordering the
  sidebar underneath one left an opaque rectangle lying across the rows in the
  desktop's own colours. Nothing between the sidebar's body and its floor asks
  the platform for a tooltip now.

### Changed

- **The replay engine is the terminal engine.** `vitrum-replay` parsed with
  `vte` behind a hand-written translation onto a cell grid while the daemon
  parsed the same bytes with Ghostty, so the replay of a session was not the
  session. `vte` is gone from the tree. Six behaviours changed and each is
  asserted rather than tolerated, including that a 24-bit colour channel above
  255 truncates to its low eight bits and that the sixteen ANSI colours are
  Ghostty's theme.
- **The flush window and the read chunk carry their arguments.** A lone write
  ends on the idle flush, so a keystroke pays 300 microseconds rather than the
  6 millisecond cap; at 181 MB/s a run reaches the byte cap in 0.35
  milliseconds, so the clock only governs children producing under about 11
  MB/s. The read chunk is argued from the line discipline's 4096-byte bound,
  which is why raising it buys no syscalls.
- **One owner per primitive.** The data plane leaves the `vitrum-proto` crate
  root for its own module, and three duplicated helpers — a millisecond clock,
  a seeded RNG, a scrollback corpus — collapse to one each.
- **The launcher offers agents, not a shell.** A row whose command is a shell
  argues this is a terminal multiplexer, which is a category where tmux and
  Zellij already win and where nothing this product does is visible.
- **The pane prototype is deleted.** It existed to prove a wgpu surface can
  live on a GTK drawing area inside the shell's own window, it proved it, and
  the widget it justified now ships behind a feature. It also depended on GTK
  unconditionally, so a workspace build on Windows or macOS failed on
  `gobject-sys` before it reached this product's own code.

## v0.1.0 - 2026-08-09

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

- **The installer finishes the install.** `install.sh` and `install.ps1` now
  write the launcher entry, put the install directory on `PATH` and define
  `vu` as `vitrum update`, all idempotently, with `--no-integrate` for images
  and headless hosts. Those steps used to be three platform-sized blocks the
  README asked you to paste after running a command that claimed to be the
  whole install.

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
  a mismatch. The README pipes the script into a shell for convenience; what
  makes that safe is the digest check, not the absence of a pipe, and the
  script is a file you can download and read first.
- `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `AUTHORS`, `NOTICE`,
  issue forms, a pull request template, and Dependabot.
- A `cargo-deny` policy and the CI job that enforces it, so a dependency whose
  license conflicts with the dual license fails the build.
- `make`: `gate`, `measure`, `perf-tables`, `perf-tables-check`, `package`,
  `release`, `release-dry-run`, `release-check`, `verify-artifacts`.
- **A release is one command.** `make release VERSION=x.y.z` bumps the version
  at every site, rolls this file, commits and annotates the tag, and pushes
  nothing. `make release-dry-run` performs the whole cut in a throwaway clone
  and proves the working tree came back byte-identical.
- **A nightly channel.** One moving prerelease tag, so the installer's latest
  lookup passes over it, versioned `<next patch>-nightly.<date>.<commit>` so it
  sorts after the last stable and `vitrum --version` does not repeat it.
- **`COLORTERM` is a constant in the crate that honours it.** An agent reads
  the variable to decide whether to emit 24-bit colour, and one test now
  asserts both the published value and that colours off the 256-colour cube
  reach a cell unquantised.

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

### Not in this release

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
