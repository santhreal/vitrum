# Changelog

Notable changes per release. Versions follow [semver](https://semver.org).
Before 1.0, a minor bump may break compatibility.

## Unreleased

### Changed

- **The terminal pane is native, and the product ships no JavaScript.** Every
  pane was drawn by a vendored emulator written in JavaScript, running inside
  the WebKit view. A pane is now a GTK drawing area with its own X window, a
  wgpu swapchain on that window, and `vitrum-grid` painting the cell grid into
  it, inside the same toplevel as the shell and with no offscreen copy between
  them. `vitrum-vt` drives libghostty, which is the only escape-sequence parser
  left: the product had two, and the second one held the working directory, the
  prompt boundary and the palette inside a renderer addon where nothing else in
  the process could read them. The emulator, its WebGL renderer, the bridge
  script and the serialised tables handed across to them are deleted.
  `app/src/tests/no_javascript.rs` enumerates the tracked tree and fails on a
  script file or a script element, so the deletion is a decision rather than a
  commit. The shell is still Dioxus over the system webview.
- **The window is drawn like an application and not like a page.** Every
  surface is on one baseline grid and one spacing scale, sized against the
  system text size rather than against a fixed pixel count. Rows, headers,
  dialogs and the status bar align on the same optical centre, sheets are
  centred on the window instead of on their own content box, and the motion
  that remains has a reason to be there. `docs/appearance.md` states the rules.

### Added

- **Worktrees are a first-class thing the sidebar knows about.** A checkout and
  every worktree beside it are one project. Each row says which worktree it is
  in and which branch that worktree is on, two agents in two worktrees of one
  repository no longer read as two unrelated projects, and a row in the main
  checkout is distinguished from a row in a worktree rather than both drawing a
  bare branch name.
- **The status bar says what the window is doing.** It carries the session's
  working directory, its worktree and branch, the daemon it is connected to and
  the protocol version it agreed on, and the pane's grid size. It was blank.
- **The pane paints in a palette you already read code in.** The grid took its
  sixteen ANSI slots from whatever the shell theme happened to declare, so an
  operator arrived with colours tuned over years and the terminal was the one
  surface that ignored them. Settings, Appearance now selects the palette:
  Solarized Dark, Solarized Light, Gruvbox Dark, Nord, Dracula, Tokyo Night and
  One Half Light, each at its published definition. The choice is independent
  of light and dark and moves no chrome. The default follows the app theme and
  emits nothing, so an install that never opens the tab is unchanged.
  `docs/appearance.md` lists them.
- **The grid can paint in the colours your own terminal is configured with.**
  The alternative to shipping an opinion is not shipping seven of them. Turn on
  "Follow the host terminal" and the import reads the terminal's own
  configuration and takes the sixteen ANSI slots, the background, the
  foreground, the cursor and the selection out of it. Four formats are parsed:
  the sectioned key/value alacritty and foot write, kitty's flat one, X
  resources, and Windows Terminal's JSON. A terminal that exports a variable
  naming itself is tried first and every other candidate is tried after it. A
  file that declares only some of the twenty colours is refused rather than
  merged, naming what each candidate was missing, because a palette half from
  one terminal and half from another is one nobody has ever looked at. The
  result is stored rather than re-detected, so the grid does not change colour
  because a config file moved, and the settings row names the file it read.
  `docs/appearance.md` states the order and what the scan cannot see.
- **Everything named in this section is adjustable in Settings.** The pane's
  font, size, line height, padding, cursor, palette source and opacity; what a
  row shows and how a project groups; the status bar's fields; motion, density
  and text scale; the frame pacing and the read and flush windows the pane
  runs on. `docs/configuration.md` lists each setting, its default and the file
  it is written to.

### Fixed

- **The sidebar draws.** It came up empty: a window with no projects, no rows
  and no way to reach a running session, next to a daemon that was holding
  them. The rows are rendered from the session list the client already had.
- **A pane no longer paints black while a harness starts.** An agent that
  writes to standard error before it draws anything left the pane a black
  rectangle with the errors in it, and nothing said that was a startup failure
  rather than a hung session. Startup output is shown as the session's first
  screen with the exit status beside it when there is one.
- **A sidebar row says where its agent is working when nothing else on it
  does.** A row at its project's own directory drew no directory, on the
  grounds that the project header above it already said so. The header carries
  a project name, not a path, so that silence was only readable while the
  branch beside it was speaking. An agent started outside a repository hit
  both blanks at once: the client mints a project for the launch directory, so
  the row sits at its root, and a directory that is not a checkout has no
  branch, so the row's whole context line went out empty. It now draws its
  directory, shortened against the home directory, in exactly that case.
- **A row follows an agent that changes directory.** The report an agent emits
  when it moves reached a renderer addon and stopped there, so a harness that
  sent itself somewhere else kept the row, the branch and the worktree it
  started in for the life of the session. The parser is in the client's own
  address space now: an OSC 7 moves the row, its branch, its worktree and the
  status bar within the frame it arrives on.
- **A full-screen agent reaches the bottom of the pane, not two rows past
  it.** The grid was measured from a computed style on the container element.
  Under `box-sizing: border-box`, which that window set on everything, the
  measurement was the border box with the padding inside it, and the padding
  subtracted afterwards belonged to a different element. The pane's own 32px
  per axis was therefore counted as space a cell could occupy, so the child was
  told it had two rows and four columns the window frame covers: the last
  option of an approval prompt was sliced in half by the bottom edge, and a
  centred prompt was centred on a grid wider than the one on screen. The pane
  measures its own drawing area in device pixels now, a row the edge would cut
  in half is never counted, and a measurement taken before the font was
  measured is not recorded as a fit, so a pane no longer sits at 80x24 for the
  life of the window with a dead band underneath it.
- **A Codex row holds "Needs approval" for the whole approval prompt.** Codex
  blinks the marker in its terminal title while a gate is up, alternating
  `[ ! ] Action Required` and `[ . ] Action Required` about twice a second.
  The title rule matched only the first of those, so every second title write
  read as no declaration at all and the row fell back to the observed state,
  `Ready`. The status alternated between the two for as long as the prompt
  went unanswered, and `Ready` is the one answer the sidebar must never give
  while an agent is waiting on an answer. The rule now reads the shape of the
  banner rather than one frame of its animation, so any marker Codex draws
  carries the same claim, and the claim still ends the moment Codex retitles.
- **A page-back the daemon cannot grant is refused once.** "That is the whole
  history the daemon still holds for this session." was raised every time the
  pane arrived at the top of its buffer, and arriving at the top is not a
  click: the strip took a line from the terminal, which refit the grid,
  repainted it and put the viewport back at the top, so the notice caused the
  gesture that raised it. The strip appeared, retired a few seconds later and
  appeared again, with the grid reflowing each way, and Dismiss was undone
  before it could be read. The refusal now records the painted history window
  it was about and stays silent until that window changes, which any new
  scrollback or any other session does. The pane ceiling refusal is counted
  on the same terms. The strip is also an overlay rather than a row taken from
  the grid, so raising one no longer resizes the session.
- **The window stopped flashing.** A resize, a session switch and a settings
  change each cleared the pane and repainted it from nothing, and a frame was
  presented before the grid had been uploaded. The swapchain is reconfigured
  without an intervening clear, and a frame is presented only once the grid it
  is drawn from is complete.
- **The window is stable under the things that used to end it.** A second
  window, a display that goes away, a swapchain that reports lost or outdated,
  a font that cannot be rasterised and a daemon that drops mid-session are each
  recovered from rather than propagated, and the recovery is exercised in the
  suite rather than reasoned about.
- **The installer resolves the latest release when the GitHub API refuses.**
  Its anonymous rate limit is per address and is spent by everything behind
  that address, so on an office, a carrier NAT or a CI runner a working
  network answered `403` and the install stopped before it downloaded
  anything. Both installers now fall back to the redirect on the releases
  page, which resolves the same version and is not the resource that ran out.
  Passing `GITHUB_TOKEN` still skips the limit entirely, and an explicit
  version still asks nothing.

### Performance

The pane's path from a byte on the socket to a lit pixel no longer leaves this
process's address space, is not serialised, and does not wait on a document
layout pass. Bytes are read from the socket, parsed by libghostty, written into
the cell grid and uploaded to the GPU, and the frame is scheduled against the
swapchain. Measurements, the method behind each one and the comparison against
v0.3.1 are in [docs/performance.md](docs/performance.md).

## v0.3.1 - 2026-08-11

### Fixed

- **The terminal engine is built optimized in every profile.** The vendored
  engine took its optimize mode from cargo's profile, so a debug build of the
  workspace linked a debug build of the engine: that one reads uninitialised
  stack memory on a scroll, and the debug test binary faulted with an access
  violation on Windows. Release builds, which already asked for an optimized
  engine, were never affected. The debug suite for the replay crate also drops
  from 220 s to 1.2 s.
- **Two files read at startup are read with a bound.** The daemon token and
  the saved window geometry were each read whole. A file that grew — a log
  written to the wrong path, a disk that filled mid-write — was pulled into
  memory before anything looked at it. Both now stop at their bound and say
  so: an over-large token names its limit rather than reporting a malformed
  one, and an over-large geometry file reads as corrupt and the window opens
  at its default size.

## v0.3.0 - 2026-08-10

### Added

- **A row shows the directory its session is in.** Only the part the project
  heading above it does not already give: a session at the project root shows
  nothing, one below it shows the remainder, and one outside the project
  entirely — a worktree beside it, or an agent that moved itself — shows its
  own path. That last case previously drew a branch with nothing saying the
  files were somewhere else. It is the live directory, not the launch
  directory: a session follows the OSC 7 report a shell already emits from its
  prompt, and `docs/states.md` documents the sequence for an agent that wants
  to report a move itself. Off in Settings under Sidebar; the session still
  moves and the branch still follows it.
- **The Claude Code hook reports where the agent is working.** It writes OSC 7
  on the same pass it writes the status hint, so a session launched in one
  directory and sent to work in another moves to where the work is and picks
  up that directory's branch. No hostname is sent.
- **A build for 64-bit ARM Linux.** Every release now carries
  `aarch64-unknown-linux-gnu` beside the x86-64 archive, built on a runner of
  that architecture rather than cross-compiled, so the same gates that judge a
  native build judge it too.
- **The install is one command, with nothing to install first.** On Linux the
  installer adds the WebKit runtime this machine is missing using the
  distribution's own package manager, printing every command that takes root
  first, and fetches a downloader for itself when the machine has neither curl
  nor wget. On Windows it installs the WebView2 runtime the same way. Refusing
  and naming a second command was the previous behaviour and is still available
  as `--no-deps`.

### Changed

- **The README is a landing page.** What vitrum is, what it looks like, the
  one install command per platform, and links out. The five-state table, the
  key bindings, the installer's steps, the compositor rules and the measured
  footprint moved to the document that owns each of them; nothing was lost.
- **The row tail behaves the same under the keyboard as under the pointer.**
  Moving focus to a row's trailing control reveals it and hides the timestamp,
  the way hovering already did, and the focus ring is drawn inside the card
  instead of being clipped by its edge.
- **A narrow sidebar drops what it cannot draw.** Below the default width the
  working directory, the filter's keycap and the filter's placeholder are not
  emitted, in place of a clipped word and a one-glyph path. The field keeps its
  name for a screen reader.
- **Nothing in the interface calls a session a tab.** The row menu and the exit
  bar say "Stop viewing" and "Stop viewing the others", the wording the
  shortcut help already used, and the What's New sheet closes with the same
  word as every other sheet.

### Fixed

- **A search answer is bounded by bytes, not by rows.** A query against a large
  scrollback could be asked for 200 rows and return a gigabyte, because the cap
  counted rows and a row can be as long as the ring. Lines are priced against
  an 8 MiB budget before they are copied, so a pathological line ends the
  answer instead of the process.
- **A repository pointer file cannot stall a session.** The branch a row shows
  is read from `.git` and `.git/HEAD` in a directory the client names. Either
  could be any size, and either could be a fifo, which parked the thread
  starting the session for as long as nothing wrote to it. Both are opened
  without blocking, refused unless they are regular files, and read to 8 KiB.
- **An unterminated escape sequence no longer slows a session for good.** A
  session that printed an OSC introducer and never closed it moved its whole
  output path to one byte at a time, permanently. The capture gives up after
  4 KiB and the fast path resumes.

## v0.2.1 - 2026-08-10

The first published release of the 0.2 line. `v0.2.0` is a tag and never
became a release: the gate that reads a built binary for the oldest system it
can start on demanded a tool that does not exist on macOS, so both macOS legs
failed after their builds had already succeeded and no archive was ever
uploaded. Everything below `v0.2.0` in this file shipped here.

### Fixed

- **A macOS build no longer fails the release.** `check-abi.sh` resolved
  `readelf` before it knew whether there was an ELF to read. macOS has
  neither, so the check exited before it could report that there was nothing
  to check. It now asks for the tool at the first ELF binary, and runs it once
  for real before believing it: pointing `READELF` at something that does not
  exist previously reported every archive clean, because each invocation
  discards its errors and reads an empty answer as "requires nothing".
- **Closing a session no longer leaks the thread that reads it.** A session
  whose child was never reaped left its output coalescer parked forever
  waiting for an exit status that nothing would deliver, holding the session
  and its threads for the life of the process. The same gap sat in the read
  loop itself: a child that ignores the hangup keeps its terminal open, so
  nothing ends the read and nothing reaps the child, and the loop waited on
  both. Every wait in the output path now observes the close, so a closed
  session ends whether or not the child cooperates.
- **A failed accept no longer takes the daemon down.** Running out of file
  descriptors made the accept loop return, which ends every hosted agent's
  terminal for a condition that clears as soon as one connection closes. The
  loop now pauses and retries, and gives up only after a run of failures with
  no success between them.

### Changed

- **The daemon serves 64 connections at once.** The accept loop spawned a task
  per connection with nothing counting them, so anything opening sockets in a
  loop could exhaust the descriptors of the process that owns every agent's
  terminal. A client past the ceiling waits for a slot rather than being
  refused.

## v0.2.0 - 2026-08-10

### Security

- **The daemon authenticates every connection.** `vitrum-server` listens on
  loopback and accepted anything that reached it. A browser allows a
  cross-origin WebSocket with no preflight, so any page open on the same
  machine could speak the protocol, start a session running a command of its
  choosing and read every transcript; another user on the machine could do the
  same. Two checks now guard it. A handshake carrying an `Origin` header is
  refused with 403, which a native client never sends and a browser always
  sends. Every connection presents a per-user token, 32 random bytes the
  daemon writes at startup with mode 0600 inside a 0700 directory, compared in
  constant time. The client reads it from `VITRUM_TOKEN`, then `--token-file`,
  then the default path. There is no `--token` flag, because argv is readable
  by every other user on the machine.
- **A connection that says nothing no longer costs a task and a descriptor.**
  A peer could open a socket, send no handshake, and hold both until the
  daemon exited. The handshake now has a deadline. An inbound message is also
  capped at 4 MiB, where the default allowed a peer to choose 64 MiB of the
  daemon's heap per connection.

### Changed

- **`PROTOCOL_VERSION` is 3.** This breaks compatibility with a 0.1.x client
  or daemon. The two refuse each other and name which is older. Restarting the
  daemon ends every session it holds, so do it when the agents are idle.

### Added

- **A fresh profile starts with the agents already listed.** The launcher's
  first run showed an empty list and asked for a command. It now seeds a
  preset per known agent, installed ones first, and captions the rest as not
  installed. Deleting a seeded row is a decision, so seeding is keyed on the
  launch store never having existed rather than on it being empty.
- **Stopping the daemon ends the sessions it holds.** `SIGTERM` or `Ctrl-C`
  used to leave the decision to the kernel: closing the last terminal
  descriptor hangs each session up, which ends most children and is not a
  guarantee, because a process may ignore a hangup for as long as it likes.
  One that did was left running with no session to reach it through, to be
  found by process id and killed by hand. Each child is now hung up, given
  three quarters of a second to exit on its own, and killed outright if it is
  still there. That budget is spent once for all sessions, not once each.

### Fixed

- **The Linux build ran on almost no Linux.** Both binaries were built on the
  newest Ubuntu, so they required `GLIBC_2.39` and died with `version
  GLIBC_2.39 not found` on Debian 12, Ubuntu 22.04 and RHEL 9. Exactly two
  symbols asked for it, `pidfd_spawnp` and `pidfd_getpid`, which the standard
  library uses when the machine it is built on offers them; nothing here asked
  for either. The client also linked `libxdo.so.3`, a default feature of two
  menu crates, and Arch ships only `libxdo.so.4`, so it did not start there at
  all. The Linux target is now built against a 2.28 floor, `libxdo` is out of
  the dependency graph along with a second TLS stack that came with it, and
  the floor and the shared-library list are both asserted on the built
  artifact so neither can come back unnoticed.
- **`vitrum` was installed and `command -v vitrum` found nothing.** bash reads
  one login file and stops at the first that exists, so a `~/.bash_profile`,
  which rustup, nvm and bun each create, shadowed the `~/.profile` the
  installer wrote to, while `~/.bashrc` is skipped by a login shell that is
  not interactive. The installer now writes the file bash actually reads. It
  also checks the downloaded binary against the machine before installing it,
  rather than checking a list of distributions.
- **A refused connection retried forever.** The backoff counter was reset when
  the socket opened, and a daemon that rejects a handshake accepts the socket
  first and closes it afterwards, so the delay never grew: 75 attempts in 20
  seconds, each writing a refusal to the log. The reset moved to the accepted
  handshake. The same case now makes 7 attempts, backs off to 8 seconds, stops
  and offers Retry.
- **A failed session also announced that it had finished.** A row drew a red
  Failed pill and a green Done badge beside each other, saying opposite things
  about the same turn.
- **The sidebar's attention chip printed a sliced word.** At the resting width
  the chip read `5 wa...`, because it absorbed the whole width deficit. The
  search field is now the only part that gives up space, and the chip reads
  whole below the previous width.
- **An unread row drew a dot beside a dot.** The unread marker sat eight
  pixels from the status pill's own leading dot, two hues, neither reading as
  a category, and only on one of the two row shapes. Unread is now the title's
  weight and colour, which both shapes get.
- **Settings prose wrapped at four different right edges** under one straight
  column of controls, because each row was its own grid sized by its own
  control. All of it now wraps against one measure.
- **Every CI job named a toolchain it did not use.** The jobs asked for stable
  and then built with the nightly pinned in `rust-toolchain.toml`, which
  outranks what the setup step selects, so the toolchain in the logs was not
  the toolchain in the build. They now install exactly what that file names.
- **An applied update is the build that keeps running.** Applying a staged
  update renames the new binary over the running one, which unlinks the image
  the process is executing. From that moment Linux answers `/proc/self/exe`
  with the path plus ` (deleted)`, and the restart into the new build was
  asked for that path, so it failed with `No such file or directory` on every
  successful update. The binary on disk was correct and the next start was
  fine, so it read as cosmetic; it was not, because for the rest of that run
  the process was the version that had just been replaced. The path is now
  read before the swap, when it still names the file.

## v0.1.2 - 2026-08-09

### Added

- **The Windows executable carries the mark.** Explorer, the taskbar,
  Alt-Tab and the shortcut the installer writes all draw an executable from
  its own icon resource, and none of them runs the program, so the window
  icon set at startup reaches none of them. The `.ico` is generated during
  the build from the same procedural geometry the window icon and the
  installer use, so it cannot drift from the mark, and no binary image is
  committed. A missing resource compiler warns and still produces a working
  binary.

### Fixed

- **The window paints in a tenth of a second instead of seven tenths.**
  Nothing drew the toplevel until the event loop started, which happens
  after the webview is built, so a launch showed a black rectangle for
  690 ms. The window now takes its background and pumps GTK's pending
  events as soon as it is built. First painted pixel moved from 690 ms to
  100 ms.
- **No surface flashes white before the first frame.** Two defects on
  Linux. The GTK window kept the theme's background, which is white under a
  light theme. And wry builds a GDK colour by passing the 0-255 channels
  straight into a type whose channels run 0.0 to 1.0, so `(6, 6, 8, 255)`
  clamped to opaque white. The window now carries an explicit background,
  and the webview is told its colour through the WebKit call in the units
  that call expects. The full-screen white frame at 706 ms is gone.
- **The mark stands on the window until the webview takes it.** The
  interval between the window appearing and the document painting was flat
  dark. The toplevel draws the 96 px mark from `vitrum-os` and retires it
  the moment a mapped WebKit view exists, so the mark holds the screen from
  107 ms to 645 ms and never paints over the running UI.

### Removed

- **The loading screen that could not load.** It lived inside the document,
  so it could only appear once the webview it was covering for already
  existed, and against its own 400 ms delay the widest window it could ever
  have painted in was 143 ms. Deleting it removes `loading.js` and drops the
  JavaScript bill from 432,438 bytes to 429,041.
- **The AV1 encoder.** The renderer fork took `image` with default features,
  which turns on every codec it has, and AVIF encoding reaches it through
  `ravif` to `rav1e`. A terminal multiplexer was linking a video encoder, and
  because `rav1e` ships hand-written AVX-512 the Windows executable failed the
  instruction-set gate with 278 instructions above the AVX2 floor while
  `vitrum-server.exe`, which does not link it, was clean. `image` is reached
  only from the icon helpers, and the bundled fallback icon is already decoded
  RGBA, so the codec set is now PNG, ICO and JPEG. Fifty-nine crates leave the
  build, among them `rav1e`, `ravif`, `rayon`, `pulp` and `raw-cpuid`.

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
  replacing the JavaScript emulator the pane was drawn with then. The argument
  for it is one parser and OSC 7 and OSC 133 semantics in Rust, not frame rate.
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
  any healthy run was announced as evicted history. A false
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
  repository variable that falls back to a hosted runner, and two guards, one
  in the pipeline and one in the test suite, refuse a label the project has not
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
  root for its own module, and three duplicated helpers, a millisecond clock,
  a seeded RNG and a scrollback corpus, collapse to one each.
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
  from a terminal with tabs was discoverable only by accident: that the sidebar
  is an inbox, that a row's colour is its agent's state, that one chord jumps
  to whichever agent wants you, that sessions outlive the window, that
  workspaces and the three bands exist at all. It is now four short
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
