# The verification harness

Every number in `GOAL.md` was taken by launching vitrum on the author's own
desktop. That has to stop. A measurement run starts an X client, a daemon and
up to twenty windows, and it kills them again afterwards; doing that on the
machine somebody is working at costs them their session sooner or later, and it
also makes the measurement worse, because a live desktop has a compositor, a
window manager and other people's windows in the way.

This directory moves the whole thing to another machine. You build here. The
other machine runs a virtual display, launches the binary you built, drives it
with `xdotool`, photographs it with `import`, reads its memory out of `/proc`,
and sends the results back into `harness/out/`. Nothing graphical ever runs on
the machine you type on.

## Before you quote a number from this harness

Read this first. It is the thing most likely to be got wrong later, and the
damage is a wrong line in `GOAL.md` that nobody notices for a month.

**A number measured here is not a number measured on the desktop, and the two
must never appear side by side as if they were.** `GOAL.md`'s headline figures
were taken on the development desktop, on a Ryzen 9 9950X with an NVIDIA card
on a real WM-managed display:

- **398.0 MB** PSS for twenty windows each showing a session, with all twenty
  sharing **one** `WebKitWebProcess`.
- **0.0292 %** of one core, over 240 seconds, with twenty windows open.

`perfhost` is an i9-13900K with a different core count, a different clock and
a heterogeneous core layout, running headless with no compositor, no window
manager and no display attached to either of its GPUs. It is a different
baseline in every axis those two numbers depend on.

So:

- A remote number is comparable to **another remote number**, taken on the same
  host, on the same kind of run. That is what makes it useful: run the scenario
  before your change and after it, and the difference between the two is real.
- A remote number is **not** comparable to a figure in `GOAL.md`, and must not
  be written into `GOAL.md` next to one, until somebody re-establishes a
  baseline on that host by running the same scenarios on a build already known
  good. Until that exists, quote remote results as "on perfhost, N windows,
  this build against that build", never as "the memory target".
- If you do establish a baseline there, write the host, the CPU, the WebKitGTK
  version and the session workload next to it. A number whose conditions are
  not recorded beside it is not a measurement.

And some things a headless box cannot tell you honestly at all, at any
baseline. There is no GPU path, no compositor and no vsync, so anything about
the WebGL renderer, compositing cost, frame timing or paint latency has to be
measured on hardware with a real display. Geometry from a headless capture is
wrong for a separate reason recorded as SPEC 14.16. And while every family in
the UI stack resolves to DejaVu on that box, no measurement involving text
width, truncation or row height means anything. The section at the end of this
file lists each of these with the specifics.

## The pieces

```
harness/run.sh              what you run, on your machine
harness/remote/rig.sh       what runs on the measurement host
harness/remote/measure.py   PSS and CPU for a process tree, from /proc
harness/remote/sessions.py  a WebSocket client that creates sessions
harness/remote/mockllm.py   a streaming OpenAI/Anthropic server, no model
harness/remote/agentsim.py  an agent TUI that drives one session
harness/out/                reports, captures and remote logs land here
```

`run.sh` compiles nothing and launches nothing. It finds your release build,
copies two files and five scripts to the measurement host, runs `rig.sh` there
over ssh, and copies the results back. If there is no release build it prints
the `cargo build` line and stops, because a harness that quietly rebuilds is a
harness that can report a binary you did not mean to test.

## The measurement host

The default is `perfhost`, with `labhost` as the fallback. Each is tried
first under its `~/.ssh/config` alias, which routes over Tailscale, and then at
its LAN address. That second attempt is not redundancy for its own sake:
Tailscale SSH on this tailnet is in check mode, so the alias can accept the
connection and then wait for a browser login until it times out. The LAN
address reaches the ordinary `sshd` and authenticates with your key. Whichever
answers first is cached in `harness/out/.endpoint`, so you pay the search once.

Set `HARNESS_ENDPOINT` to skip the search entirely, or `HARNESS_ENDPOINTS` to
change the list.

### What is on it, and what it still needs

Run the probe first. It reports and installs nothing:

```
harness/run.sh probe
```

As of this writing, `perfhost` is an Ubuntu 24.04.3 box with an i9-13900K, 64
GB of RAM and an idle load average of about zero. It already has GTK 3, libsoup
3, `libxdo`, Mesa, `Xvfb`, `xdotool`, ImageMagick, `python3`, `rsync`, `flock`
and `setsid`. It is missing exactly two packages, and the probe exits 4 until
they are installed:

```
sudo apt-get update && sudo apt-get install -y libwebkit2gtk-4.1-0 libjavascriptcoregtk-4.1-0
```

Those are the engine the client renders through. Ubuntu 24.04 offers
`2.52.3-0ubuntu0.24.04.1`, which is the same version this project's desktop has,
so the two boxes agree on the one dependency whose version changes both the
rendering and the memory figure.

You do not have to remember to probe first. `memory`, `idle-cpu` and
`screenshot` each run the same check before they start anything, and refuse
with the install command and exit 4 rather than bringing up a display and a
daemon for a binary that cannot load.

There is a second, optional list. Every font in the UI stack currently falls
back to DejaVu on that box, so text metrics there will not match the desktop
until you install the faces the stack actually names:

```
sudo apt-get install -y fonts-ubuntu fonts-cantarell fonts-noto-core fonts-jetbrains-mono
```

You need those before any geometry or screenshot claim from the remote means
anything. You do not need them for memory or idle CPU.

### The fallback, labhost

Probed as well, because which host you pick changes what has to be installed
and it is not the answer you would guess.

`labhost` is Ubuntu 24.04.2 on an i7-11700K, 16 threads, 32 GB, headless.
It has an RTX 3080 Ti whose driver and library versions do not match, so
`nvidia-smi` fails; that is reported and does not matter to anything this
harness measures. It **already has** `libwebkit2gtk-4.1-0` and
`libjavascriptcoregtk-4.1-0` at `2.52.3-0ubuntu0.24.04.1`, the same version as
the desktop. What it lacks is the tooling:

```
sudo apt-get update && sudo apt-get install -y libxdo3 xdotool imagemagick
sudo apt-get install -y fonts-cantarell
```

So the two hosts need opposite things: `perfhost` has the tools and needs the
engine, `labhost` has the engine and needs the tools. Either way it is one
`apt-get` line, and `probe` prints the right one for whichever host answered.

Treat `labhost` as a **third** baseline, not as a spare copy of the second.
It has half the threads and half the memory of `perfhost` and a different CPU
generation again, so a number from one is no more comparable to a number from
the other than either is to the desktop. Pick one host and stay on it for a
before-and-after pair.

## Reproducing the memory number

`GOAL.md` records 398.0 MB for twenty windows, each showing its own session,
as PSS across the whole client process tree. To take the same measurement on
the measurement host:

```
harness/run.sh memory 20
```

What that does, in order, and why each step is there:

1. Copies `vitrum-app` and `vitrum-server` from your release build. It refuses
   to run if your build's glibc is newer than the host's, because the failure
   mode otherwise is an unexplained "version GLIBC_x.y not found" at launch.
2. Takes `/tmp/vitrum-harness.lock`. Only one run at a time, because the client
   only reaches a daemon on the default port. A non-default `--server` forces
   `--standalone`, which breaks the window handoff, so two concurrent runs would
   fight over `127.0.0.1:7737` and each would measure a mixture of the two.
3. Picks a free X display between `:101` and `:199` and starts `Xvfb` on it with
   no window manager. The host already runs somebody else's `Xvfb` on `:99`, and
   measuring another process's window on a shared display is a mistake this
   project has already made once.
4. Points `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`
   and `XDG_RUNTIME_DIR` at a fresh directory under `/tmp`. A persisted sidebar
   width or workspace-bar flag from an earlier run silently falsifies the next
   one. `XDG_RUNTIME_DIR` matters as much as the rest, because the
   single-instance lock and socket live there.
5. Starts `vitrum-server` on port 7737. If anything is already listening it
   stops rather than measure against a daemon it did not start.
6. Creates twenty sessions over the wire protocol, each running `/bin/bash -i`.
7. Launches `vitrum-app` once with `vitrum://session/<first id>`, then launches
   it nineteen more times, each with the next session's URL. Those nineteen do
   not start a second copy of the program: they hand their request to the
   process holding the single-instance lock and exit. That is what puts twenty
   windows in one process, and the whole 398 MB result depends on it, so the
   window count is asserted rather than assumed.
8. Waits `HARNESS_SETTLE` seconds, 45 by default.
9. Sums `Pss` from `/proc/<pid>/smaps_rollup` across the client's whole tree,
   and reports the daemon's tree separately.

PSS and not RSS, because twenty windows share one `WebKitWebProcess` and RSS
would count that engine twenty times over.

The workload is a knob, and it moves the number by more than the margin you
are likely to care about. `GOAL.md` measures 1068.1 MB with the sessions
printing nothing, 1101.0 with ordinary shell startup, and 1153.6 with 400 lines
each, all on the pre-sharing build. Change it explicitly:

```
HARNESS_SESSION_CMD=/bin/sleep HARNESS_SESSION_ARGS=600 harness/run.sh memory 20
```

## Reproducing the idle CPU number

`GOAL.md` records 0.0292% over 240 seconds with twenty windows open. The same
shape of run:

```
harness/run.sh idle-cpu 240 20
```

The setup is identical to `memory`. After the settle the pointer is parked at
`1,1`, off every row, because a cursor resting on a session card leaves it in a
hover state and a hover transition is work. Then nothing touches the display for
the length of the window.

The number is the sum of `utime + stime` deltas across the client tree, divided
by elapsed wall-clock time and by the clock tick, expressed as a percentage of
one core. A process that starts or exits inside the window is named in the
report instead of being folded silently into the total, because "the tree
changed under the measurement" is the commonest way a figure like this comes out
wrong. The same run also prints PSS at both ends of the window, which is how you
check for drift.

Every report also carries a `machine` line: the core count, and the load
average at both ends of the window. That is there because a number which moved
because the box was busy and a number which moved because the code changed are
indistinguishable once written down, and the habit of assuming the first is how
a real regression gets waved through. Ruling it out costs one line, so the line
is always printed. For scale, the development desktop sat at load 13 to 28
while this was being built and `perfhost` sat at 0.04, which is most of the
argument for measuring there rather than here.

## Comparing against T3 Code

```
harness/run.sh bench 20
```

This puts vitrum and T3 Code under one identical workload and reports what each
costs in memory. Twenty sessions, each running a mock agent that talks to a mock
LLM: no network, no API key, no model, and a seed, so the same command produces
the same bytes on every run. A benchmark whose input varies measures nothing.

Two pieces do the pretending:

- `harness/remote/mockllm.py` serves OpenAI and Anthropic streaming endpoints
  and paces tokens against a monotonic deadline, so `--tokens-per-second` means
  what it says under load. `GET /stats` reports what it actually served, which
  is how you check that both halves of the comparison got the same work.
- `harness/remote/agentsim.py` is an agent TUI and nothing more: alternate
  screen, a status line, wrapping, a spinner, cursor addressing, and hint OSC
  sequences. It exercises a terminal the way a coding agent does.

The mock agents are excluded from both products. They are the same twenty
processes whichever app is hosting them, and vitrum's daemon happens to be their
parent, so a plain tree walk charges 190 MB of Python to vitrum and nothing to
T3 Code. Adding a constant to both sides of a ratio also drags it toward 1. They
are measured, printed on their own line, and subtracted from each side.

T3 Code is never installed by this harness and none of its source is in this
tree. If it is not on the box, the run reports vitrum's number alone and says
there is no ratio, rather than inventing one. Point `HARNESS_T3` at the binary
if it lives somewhere the search does not look.

Measured on `axiomserver`, 32 threads, load 0.76, twenty sessions, PSS:

| | processes | PSS |
|---|---|---|
| vitrum client | 3 | 845.8 MB |
| vitrum daemon | 1 | 45.1 MB |
| mock agents (excluded) | 20 | 190.2 MB |

Most of the client figure is one WebKit web process at 679 MB. It is one
process for twenty windows rather than twenty, which is the whole reason for the
vendored dioxus-desktop fork, and it is also the number to attack next.

## Screenshots

```
harness/run.sh screenshot sidebar
harness/run.sh screenshot sidebar-1.5x 1382x800 1.5
```

The capture lands in `harness/out/<run-id>/sidebar.png`. The run creates one
session, opens it, waits `HARNESS_STARTUP` seconds so the first daemon snapshot
has landed, sizes and positions the window, parks the pointer, and captures with
`import -window`.

It then rejects a flat frame. An occluded window comes back pure white from
`import` and exits 0; a window that never painted comes back pure black. Both
are uniform, so the standard deviation is the test, not the mean.

Two traps are handled for you. Every vitrum process maps **two** X windows: a
10x10 decoy whose `WM_NAME` is the binary's file name, and the real window,
whose `WM_NAME` is exactly `vitrum`. The rig resolves by pid and rejects the
10x10, because taking the first name match gives you the decoy and reads as "the
app opened no window". And the display is private to the run, so you cannot
photograph another instance's window.

## What a remote box cannot tell you honestly

The banner at the top of this file states the baseline rule. This is the
itemised version of it: what specifically cannot be answered here, and why. A
regression is a change between two runs on the same host. It is not the gap
between a run here and a line in `GOAL.md`.

- **Idle CPU is a percentage of a different core.** The desktop is a Ryzen 9
  9950X; `perfhost` is an i9-13900K with a different clock, different boost
  behaviour and a heterogeneous core layout, so a timer's cost in ticks is not
  the same quantity. The 0.055% budget is a desktop budget.
- **GPU-dependent behaviour is not represented.** The measurement host has an
  RTX 4090 and an Intel iGPU, but no display server is attached to either and
  `Xvfb` has no GPU path at all. Anything about the WebGL renderer, compositing
  cost, or GPU memory has to be measured on hardware with a real display. The
  DOM renderer's 23 MB advantage over WebGL and the 0.24% idle cost of a
  compositing layer are desktop findings and must stay desktop findings.
- **Compositor and frame timing mean nothing here.** There is no window manager
  and no compositor. The "one `SessionUpdated` in 16 to 20 ms" line is a
  desktop measurement and cannot be reproduced or refuted on `Xvfb`.
- **Geometry from a headless capture is not trustworthy.** SPEC 14.16 records
  why: `Xvfb` does not resize the webview surface when `xdotool` resizes the X
  window, and tao goes on reporting roughly 1018 CSS px whatever size you ask
  for. The same build and the same session read a 225 CSS px sidebar headless
  and 449 on a real display. Use these captures for colour and for band
  boundaries. Do not measure pitch, row height or the sidebar floor from them.
- **UI scale does not derive itself.** The application reads physical
  millimetres out of RandR, and `Xvfb` does not forward what `-dpi` implies, so
  it lands on 1.0 whatever you pass. That is why the screenshot command takes
  the scale as an argument. `1.5` reproduces the document zoom a 4K panel
  derives; it does not prove the derivation.
- **The operator-waiting state may be UNKNOWN there, and a capture cannot tell
  you that from a regression.** vitrum decides whether a session is blocked on
  the operator by reading `/proc/<pid>/syscall` for the PTY's foreground
  process group (`crates/vitrum-core/src/probe.rs:147`). That file is
  ptrace-gated by Yama, which permits the read only for an ANCESTOR of the
  target. The daemon spawns the PTY child, so it qualifies, which is why this
  works at all.

  Measured, not predicted: shipping the vitrum-core test binary to `perfhost`
  and running it fails 13 of 196 tests, all in `hint_session` and
  `waiting_probe`, the first with `left: None, right: Some(true)`. The same
  binary passes 196 of 196 here. `None` is the UNKNOWN value, so the product is
  behaving exactly as designed and refusing to invent an answer; the host is
  the thing that cannot answer.

  The cause is `kernel.yama.ptrace_scope`, and the test for it has to keep the
  reader as the target's PARENT. A forked `cat /proc/<pid>/syscall` is the
  target's SIBLING and is denied at scope 1 as well as at scope 2, so it cannot
  tell the two apart; that mistake is how this was nearly recorded with
  evidence that did not discriminate. Use a shell builtin redirect, which keeps
  the shell itself as the reader:

  ```
  sleep 5 & pid=$!; read -r line < /proc/$pid/syscall && echo OK || echo DENIED
  ```

  Run three ways, that gives: this desktop scope 1, OK. `perfhost` scope 2,
  DENIED. `labhost` scope 1, OK. So the two candidate hosts differ on this
  and `labhost` does not have the problem at all, which is one more reason
  to prefer it. `probe` reports the sysctl and says what it means. Match the
  desktop with `sudo sysctl -w kernel.yama.ptrace_scope=1` before believing any
  status-pill or sidebar-state claim from a remote capture.
- **Text metrics depend on installed fonts.** Until the optional font packages
  above are installed, every family in the UI stack resolves to DejaVu on that
  box and no measurement involving text width is comparable to the desktop.
- **Memory is the number that travels best, and it still is not free.** PSS is
  a property of the process tree, not of the panel, so it is the most portable
  figure here. It still moves with the glibc and WebKitGTK versions, with the
  session workload, and with the window size at launch. Compare like with like.

One thing that does transfer, and is worth writing down because it looked like
it would not: both machines have
`kernel.apparmor_restrict_unprivileged_userns=1`, and on both of them
`bubblewrap` cannot create a user namespace from a shell. WebKitGTK falls back
to running its web process unsandboxed in both places, so the sandbox posture of
a measurement run matches the desktop rather than diverging from it. The harness
sets no `WEBKIT_DISABLE_SANDBOX` variable, on purpose.

## Traps, so the next person does not rediscover them

Each of these cost time while building this, and none of them announces itself.

**Tailscale SSH in check mode does not time out on its own.** The alias accepts
the TCP connection, prints "Tailscale SSH requires an additional check" with a
login URL, and then sits there. `ConnectTimeout` does not cover it, because the
connection succeeded; it is the session that never starts. Every reachability
probe in `run.sh` is wrapped in `timeout 12` from the outside for that reason.
The LAN address bypasses it entirely, and the usernames are not your local one:
`perfhost@` and `labhost@`, from `~/.ssh/config`.

**`set -e` with `pipefail` turns a broken GPU driver into a truncated report.**
`nvidia-smi` on a host whose driver and library versions disagree prints its
complaint and exits non-zero, which under `pipefail` failed the pipeline and
aborted the probe halfway through, before it printed a verdict. The rule for
this file: if a command's job is to describe the host, its exit status is not
information and must be discarded with `|| true`. If its job is to take the
measurement, its exit status is the whole point and must not be.

**A missing binary is not a package name.** The verdict prints an `apt-get
install` line and somebody will paste it. An early version collected missing
BINARIES into the same list as missing packages and emitted `apt-get install -y
import identify`, which is not a command. Missing tools go through
`package_for_tool` now, and the list is deduplicated, because a host can be
missing both `xdotool` the package and `xdotool` the binary.

**No substring test is a safe test for the decoy window.** Every vitrum process
maps a 10x10 decoy alongside the real window, and the obvious way to reject it
is a substring match on the geometry. Both obvious forms have a collision, at
opposite ends:

- `*10x10*` matches `810x102`, so a real window at that size is discarded.
  This is the form in `tools/regression/screenshot.sh:136`. It is not wrong for
  the sizes that script uses and it is not my file to change.
- `*"Geometry: 10x10"*` closes that and matches `Geometry: 10x100` and
  `Geometry: 10x1080`, so a narrow tall window is discarded. This was mine,
  written as the fix for the first one, and it stood until a sibling agent
  found the identical missing right-hand boundary in an unrelated matcher and
  I went looking for my own.

Neither collision is reachable at the sizes vitrum uses today, which is exactly
why both survive review and why neither was found by reading. `app_windows`
parses the geometry and compares it to `DECOY_GEOMETRY` exactly. The general
form of the lesson: a containment test on a value that has a well-defined
equality is a bug waiting for a size nobody tried.

**The development desktop may already have a daemon on the default port.**
Observed on this one: a `vitrum-server --port 7737` owned by the user,
launched from a GNOME Terminal, up for hours, with its binary since rebuilt
over. That is theirs and nobody should go near it. It is recorded here because
it is a fact about the machine rather than about any run, and because it is the
concrete case the rig's refusal exists for: a measurement taken against a
daemon the run did not start is a measurement of somebody else's sessions.
`start_daemon` reports the port as in use and stops. Nothing here ever kills a
process it did not start, and a process you cannot account for is somebody
else's by default; the cost of being wrong about that is asymmetric.

Two habits follow. Run measurements on the measurement host, where the rig
owns the port and takes a lock. And if you ever point something local at 7737,
check whose daemon answered before believing what it said.

**A second copy of a list is how a fixed bug comes back.** The rig's tool
checking lived in three hand-kept places: the loop in `preflight`, the loop in
`cmd_probe`, and a `case` mapping binaries to packages. Any tool added to one
and missed in another fell through a `*)` branch that echoed the BINARY name,
so the verdict could have told somebody to run `apt-get install -y import`.
That is the exact defect this file had already fixed once, reintroduced by
duplication rather than by an edit. It is now one list of `binary:package`
pairs that both loops read, and a name absent from it reports
`UNKNOWN-PACKAGE-FOR-<name>` instead of guessing. One list cannot disagree
with itself.

**`--server` is not a free flag.** Any non-default `--server` sets `standalone`,
which stops a second launch handing its window request to the first. A memory
run that passes it gets N processes instead of N windows in one process, and
the number it reports means nothing. That is why the rig runs the daemon on the
default port and serialises runs with a lock rather than isolating them by port.

**Kill by process group, never by name.** The host runs other people's Xvfb
servers and may run another copy of this harness. Everything the rig starts
goes through `setsid` with a wrapper that writes its own pid before `exec`, so
the pid in the file is also the process group id and `kill -TERM -$pid` takes
the whole subtree. There is no `pkill` in this directory.

**Keep the run directory short.** The single-instance socket is a filesystem
socket and `sun_path` is 108 bytes. `XDG_RUNTIME_DIR` under a home directory,
plus a cache path, plus a run id gets close enough to matter, and `bind` fails
with `ENAMETOOLONG` rather than truncating. The per-run tree lives at
`/tmp/vh-<run-id>` for that reason alone.

**A rig can be greener than the thing it measures, and this one can too.** The
failure is general: any environment built to verify something, which carries a
dependency the real target lacks, passes for a reason the product does not
have. It showed up elsewhere in this project as a scratch crate that declared a
dependency the real manifest had not yet gained, and stayed green through a
four-minute window where the actual build was broken. The version here would be
installing something on the measurement host to make a run succeed, and then
reading that success as "the application works". It does not mean that; it
means the application works on a host with that package. Two habits keep it
honest. Install only what `probe` names, so the host's package set stays a
deliberate list rather than an accretion. And when a run starts passing after
you changed the host rather than the code, write down which one you changed,
because that distinction is the whole value of the measurement.

## Cleaning up

Every process the rig starts is started in its own process group and killed by
group id. There is no `pkill` anywhere in this directory, and no matching on
process names: the host may be running another Xvfb, another vitrum, or a second
harness, and a name match would take all of them down.

The per-run directory under `/tmp` on the host is deleted after the results are
fetched. Set `HARNESS_KEEP_REMOTE=1` to keep it.

## Environment

| variable | default | what it changes |
|---|---|---|
| `HARNESS_ENDPOINT` | unset | one ssh destination, skipping the search |
| `HARNESS_ENDPOINTS` | the four defaults | the ordered list to search |
| `HARNESS_BIN_DIR` | your release build | where `vitrum` and `vitrum-server` come from |
| `HARNESS_SCREEN` | `1920x1080` | the virtual screen size |
| `HARNESS_SETTLE` | `45` | seconds to settle before measuring |
| `HARNESS_STARTUP` | `8` | seconds between the window mapping and a capture |
| `HARNESS_SESSION_CMD` | `/bin/bash` | what each session runs |
| `HARNESS_SESSION_ARGS` | `-i` | its arguments |
| `HARNESS_KEEP_REMOTE` | `0` | leave the run directory on the host |
| `HARNESS_T3` | unset | T3 Code's binary, when the search cannot find it |
| `HARNESS_BENCH_TURNS` | `40` | turns each mock agent takes |
| `HARNESS_BENCH_TOKENS` | `200` | tokens in each mock response |
| `HARNESS_BENCH_TPS` | `30` | tokens per second the mock streams at |
| `HARNESS_BENCH_SEED` | `1` | the seed both halves run under |

## What has been exercised, and what has not

Verified against `perfhost`, and the probe additionally against
`labhost`:

- Endpoint search, the Tailscale timeout, the LAN fallback, and the cache.
- Staging, the glibc comparison, and `--exclude=bin/` on the script sync, so a
  probe no longer deletes the binaries a measurement run uploaded.
- A free display is chosen at `:101` while a foreign `Xvfb` holds `:99`.
- `vitrum-server` starts, binds 7737, and the port check refuses a foreign one.
- `sessions.py` completes the WebSocket handshake, validates the accept hash,
  and creates real sessions: three requested, ids 1, 2 and 3 returned, three
  `bash` children visible under the daemon.
- `measure.py` walks that tree and reports 9.7 MB PSS across four processes and
  0.0000% CPU over three seconds, with no drift.
- Teardown by process group leaves nothing behind: no vitrum processes, port
  7737 free, run directory gone.
- The session cross-check, added because counting windows is the same shape of
  hole as counting nothing. `open_windows` asserted the window count, but
  `wait_windows` has already established `have >= want` by the time it runs, so
  alone it only really catches an EXTRA window. What it could not catch is the
  failure GOAL.md already records: a 1059.2 MB result taken with fewer sessions
  than windows, so several windows showed the same session and the number was
  not the workload it claimed. The rig now also asks the daemon how many
  sessions it holds and requires the two to agree. Proved against a live daemon
  on the remote, which needs no WebKitGTK: a fresh daemon reports 0, after five
  creates it reports 5 and a five-window run passes, after two more it reports
  7 and a five-window run dies. It still does not prove each window is showing
  a DIFFERENT session, which needs the client's own state; that remains
  unverified and is named below rather than implied.
- `bench 20` end to end on `axiomserver`: mock served 140 requests and 28000
  tokens, twenty mock agents ran, and the split reported 845.8 MB of client,
  45.1 MB of daemon and 190.2 MB of excluded workload. The exclusion is not
  taken on trust: with a marker that also matched the tree root, the run
  reported the product as 0.1 MB, which is how the root guard in `cmd_footprint`
  came to exist.
- No ratio has ever been produced, because T3 Code is installed on neither
  measurement host and this harness will not install it. Both halves of the
  comparison are exercised; only the second product is missing.
- Preflight's TOOL-missing branch, which every earlier test had left
  unexercised because the only host available was missing libraries, not
  tools. Run against a curated `PATH` with `xdotool` and `import` hidden, the
  branch fired correctly and exposed a fourth hole: it printed a hardcoded
  blanket list of six packages for two missing tools, and named `dbus-daemon`,
  which is not a package. That is the same defect I had already fixed in
  `probe`, left standing in its twin. The line is now derived from what is
  absent and deduplicated: hiding `xdotool`, `import`, `identify` and `flock`
  reports four missing tools and three packages, with `imagemagick` named once
  for the two binaries it provides.
- The stray-process warning, added after turning the instrument question on
  my own tree walk. The walk follows `ppid`, so a descendant that is reparented
  by double-forking or by an intermediary exiting has its `ppid` set to 1 and
  leaves the tree SILENTLY. Demonstrated rather than assumed: a `setsid`
  grandchild whose parent exits stays alive and stops being counted, and
  nothing in the output said so. Applied to a `WebKitWebProcess` that would
  drop roughly 270 MB from the headline figure and report a smaller,
  better-looking, wrong number. `measure.py pss` now names any process wearing
  a `WebKit` or `vitrum` prefix that sits outside the tree it just measured.
  Proved by reparenting a deliberately vitrum-named stray, which it caught. It
  is a warning and not an error, because on a shared box a stray can
  legitimately belong to somebody else, and only a human can say which.
- The blank-capture guard, where escape-hunting found the second real hole of
  the day. Confirming it against the two failures it names was easy and
  useless: an occluded window returns pure white and an unpainted one returns
  its bare background, both perfectly uniform, so any positive threshold on the
  standard deviation rejects them. Hunting for a frame it MISSES found one
  immediately. On 1382x800, a background with a single 1px line reads
  `sd=0.033` and a background with a 2px seam reads `sd=0.0017`, so both clear
  the 0.001 threshold while having painted essentially nothing; a real
  interface reads 0.198. The deviation only ever proved "not perfectly
  uniform". The guard now counts distinct colours, which reads 1 for either
  blank frame, 2 and 3 for those two escapes, 800 for a gradient and over a
  million for a real render, and requires at least 16. Re-proved on all six:
  the two blanks and the two escapes rejected, both real renders accepted. It
  also removes a fragility the old form had, that `identify` returns `-nan` for
  a pure white frame at this size and the guard rested on what `awk` does with
  that string.
- The foreign-daemon refusal, proven at the primitive. With nothing listening
  the port reads free; with a socket bound to `127.0.0.1:7737` it reads in use,
  so `start_daemon` stops rather than measure against a daemon the run did not
  start; once the listener exits it reads free again. This proves the detection
  and not the whole refusal path, because preflight fires before it while the
  engine is missing.
- The decoy rejection, and this one found a real hole rather than confirming an
  assumption. Proving it against live `xdotool` output on a spare `Xvfb`, with
  a 10x10 window mapped at `+10+10` in the shape vitrum's decoy takes, showed
  the format is `Geometry: 10x10` on its own indented line. But the matcher I
  had, `*"Geometry: 10x10"*`, also rejected a real `10x100` window, because a
  substring test has no right-hand boundary. It was written to close the
  `810x102` collision in the unanchored form and it silently kept a collision
  of its own at the other end. Neither is reachable at the sizes vitrum uses,
  which is exactly why both survived. `app_windows` now parses the geometry and
  compares it exactly. Re-proved against the same case on real output: `10x10`
  rejected, `10x100` kept, where the old matcher rejected both. What remains
  unproven is only the pid-matching half, which needs a real vitrum process.
- The refusal path. `memory`, `idle-cpu` and `screenshot` all stop before
  starting an X server, a daemon or a single session, and print the exact
  `apt-get install` line with exit 4. Preflight asks the dynamic loader which
  of the binary's sixteen `NEEDED` entries do not resolve, and maps each to its
  package, so it answers "will THIS binary run here" rather than "does the
  package database look right". It also checks the nine tools the rig drives
  with. Verified for all three commands, each refusing in under a second.
- The crash path, for a dependency that could slip past preflight. Observed
  before preflight existed, with `libwebkit2gtk-4.1.so.0` genuinely absent: the
  run reports `vitrum-app exited before mapping its first window` in under
  three seconds and exits non-zero, instead of waiting out the ninety-second
  window timeout.
- Probing the second host, which is what caught two defects in this harness:
  `nvidia-smi` failing under `pipefail` and truncating the report, and the
  verdict emitting binary names as if they were apt packages. Both fixed and
  re-verified on both hosts.

A guard can be right and never run. Proving the two separately, because a
sibling agent spent today finding that everything they had proved the DECISION
and nothing proved anything CALLED it:

| guard | correct | reached |
|---|---|---|
| preflight refusal | proved | proved, fires on every real run |
| stray-process warning | proved | proved, caught a real one first use |
| machine and load line | proved | proved, in every report |
| session count cross-check | proved against a live daemon | call site read, never run |
| decoy rejection | proved on live `xdotool` output | call site read, never run |
| blank-capture guard | proved on six frames | call site read, never run |
| foreign-daemon refusal | primitive proved | never run: preflight fires first |

Everything in the right-hand column that says "call site read" needs a mapped
window, which is the same blocker as everything else below. They are listed
rather than folded in with the proved ones, because "the check is correct" and
"the check runs" are different claims and only one of them is testable here.

Not verified, and for one reason: neither host has the full set yet, and
installing packages on them was explicitly out of scope for this change.
`perfhost` lacks the engine, `labhost` lacks the tooling. So the parts of
`screenshot`, `memory` and `idle-cpu` that begin at "a window maps" have not
been run anywhere. That is the window resolution and the pid-matching half of
the decoy rejection, the deep-link handoff for windows two onward, the capture
itself, and every number the three commands print.

One gap is worth naming separately because no amount of installing fixes it.
Nothing here proves that each of N windows is showing a DIFFERENT session. The
rig creates N sessions, opens N windows each carrying a distinct
`vitrum://session/<id>`, asserts both counts and requires them to agree, but
"window 7 is displaying session 7" is the client's own state and is not visible
from outside it. GOAL.md records that exact mistake costing a measurement once
already. If you need the 398 MB figure to mean what it says, check the windows
by eye on the first run, or have the client expose what it is showing.

Install the packages the probe names for whichever host you pick, then run
`harness/run.sh memory 2`. That is the smallest run that exercises all of it,
because two windows is the first case that needs the handoff.
