# Performance

vitrum paints its terminal panes with a native renderer: libghostty's VT parser
feeds a cell grid, and the grid is drawn by a wgpu pipeline on a drawing area
that owns its window, inside a GTK 3 shell. Release 0.3.1 painted the same
panes with a JavaScript terminal emulator inside a webview, and drew the shell
around them with a second webview. This page is the measured difference.

Every figure below came from a run that was executed, not estimated. Each row
names its method. Where a signal could not be measured on the old build, the
row says so and no ratio is given.

## How to reproduce

```
cargo run --release -p vitrum-bench --no-default-features -- latency --gate
```

That prints the table, writes `report.json` and `report.md` under the output
directory, and exits non-zero if any signal is outside the bounds recorded in
`crates/vitrum-bench/src/latency.rs`. It needs a GPU adapter and no display
server. `--software` forces a software adapter, `--samples` sets the sample
count, and `--panes` sets how many extra panes the memory signal builds.

The old build is not reproducible from this tree. Its figures were taken from
an installed 0.3.1 binary with the probes under `harness/latency/`, which read
pixels out of an X display and time the interval between a cause and the
pixels that answer it. Nothing is instrumented, so the same probe measures a
released binary and a development build and the two figures mean the same
thing.

## Hosts

Two machines, named here by what they are rather than by whose they are.

**Measurement host.** 32 logical cores, NVIDIA RTX 4090, Linux 6.8. Both
builds were measured here, so every ratio in the table below is same-host.
The old build ran under a nested X server with software rasterisation, because
that display has no direct rendering and the webview could not reach the
adapter through it. The new build was measured headless on the adapter itself.
That asymmetry is real and it favours the new build: part of the old build's
cost is software rasterisation it could not avoid on that display.

**Development host.** 32 logical cores, NVIDIA RTX 5090, Linux 6.17. New build
only, listed for scale.

## What "painted" means

A frame is painted when the GPU has finished it: the command buffer is
submitted and the queue fence is awaited before the clock is stopped. Submit
is not painting, and a renderer that stops the clock at submit reports the
driver's queue depth as speed.

The new-build figures end at that fence. They do not include the compositor
picking the frame up and putting it on a scanout. That term belongs to the
display server, is the same for any client, and is measured separately below
as the platform floor.

## The platform floor

Between a key being pressed and a pixel changing there is a cost no
application controls: the display server delivers the event, the client is
scheduled, the drawing request goes back, and the result is composited. A
client that does nothing but fill one rectangle per key pays it.

Measured with `harness/latency/floor.py` on the measurement host's display,
60 presses:

| Statistic | Floor |
|---|---:|
| p50 | 1.995 ms |
| p95 | 2.499 ms |
| p99 | 2.638 ms |

About 1 ms of that is the probe's own sampling interval, which polls the
rectangle roughly every millisecond. The floor is therefore an upper bound on
the display path, and subtracting all of it from an old-build figure is
conservative: it understates the improvement rather than overstating it.

Every old-build figure below is quoted twice. The measured figure is what the
probe saw. The own cost is that figure with the floor subtracted, and it is
the part this renderer replaced.

## Latency

Measurement host. Old build: 0.3.1, nested X server, software raster,
40 to 60 samples per signal. New build: 2000 samples per signal, hardware
adapter, headless.

| Signal | Old, measured | Old, own cost | New, p50 | New, p99 | Ratio |
|---|---:|---:|---:|---:|---:|
| keystroke to painted glyph | 11.58 ms | 9.58 ms | 0.055 ms | 0.104 ms | 174x |
| agent output byte to painted glyph | 30.87 ms | 28.87 ms | 0.310 ms | 0.352 ms | 93x |
| scroll notch to repainted pane | 11.66 ms | 9.66 ms | 0.301 ms | 0.339 ms | 32x |
| frame time, full-screen redraw | not observable | not observable | 0.383 ms | 0.477 ms | none |
| frame time, resize | not measured | not measured | 0.216 ms | 0.308 ms | none |
| sidebar model update, 200 sessions | not measured | not measured | 0.187 ms | 0.200 ms | none |

Ratios are p50 own cost over new p50.

### Method, per row

**keystroke to painted glyph.** Old build: a synthetic key event is injected
into the focused pane and the pane rectangle is polled until it shows a frame
outside the set of frames it showed at rest. Learning the rest set first is
what stops a blinking caret being reported as latency. New build: one byte is
written to a pty master, the line discipline echoes it, the echo is read,
parsed, synced into the grid, rendered, and the queue fence is awaited.

**agent output byte to painted glyph.** Old build: the session writes a
full-width line and records its own pre-write timestamp on the same clock and
the same host as the probe, so this is one clock's difference and not two
machines being compared. New build: a full-width line is written to the pty,
read back, parsed, rendered and awaited.

**scroll notch to repainted pane.** Old build: a wheel button is pressed over
a pane holding 4000 lines of transcript, alternating direction so the view
cannot reach the end of the scrollback and start reporting no-op frames. New
build: the viewport is moved one row over 4000 rows of scrollback and the
whole grid is re-rendered and awaited.

**frame time, full-screen redraw.** The old build could not be observed here.
It repaints at the display refresh rate and no faster: over 20 seconds of a
session repainting as fast as the terminal would accept, it produced 1197
distinct frames, 59.85 per second, with a gap of 16.66 ms at p50 and 29.17 ms
at worst. Its frame time is therefore somewhere at or under 16.66 ms and the
measurement cannot say where, so no ratio is claimed. What can be compared is
what the machine paid for those frames, which is the next section.

**frame time, resize** and **sidebar model update** have no old-build figure.
The resize path in 0.3.1 was driven by the webview's own layout and could not
be isolated from it. The sidebar figure is the client model only: a snapshot
of 200 sessions decoded from the wire, arranged into sections and rolled up
per project. The paint of those rows belongs to the shell, is not in this
figure, and is not claimed as an improvement.

## Processor cost

A renderer that meets the frame budget by spinning a core is what makes a
window feel slow while every frame arrives on time, so frame time alone does
not answer the question.

Old build: user plus system time of the whole client process tree over 20
seconds while one pane repainted at 60 Hz. New build: user plus system time of
the whole process, threads included, read every quarter second while a
full-screen redraw is paced at 60 Hz.

| Build | Cost of a 60 Hz full-screen redraw |
|---|---:|
| 0.3.1, client tree | 1.084 core |
| 0.3.1, web engine process alone | 0.886 core |
| native renderer, measurement host | 0.032 core |
| native renderer, development host | 0.027 core |

34x on the same host. The native renderer completes a frame in 0.383 ms, so a
16.67 ms slot holds 43 of them. That headroom is what a second pane, a
sidebar animation and a higher refresh rate are spent on.

## Memory

| Build | Figure | Method |
|---|---:|---|
| 0.3.1 client, one window, three sessions | 534.65 MB | PSS across three processes |
| native pane process, one pane | 264.73 MiB | RSS of one process, adapter and font stack resident |
| native pane, each additional pane | 474.0 KiB | RSS before and after building 8 more panes, divided by 8 |
| native pane, each additional renderer | 36.08 MiB | RSS before and after building a second renderer |

The old client could not be charged per pane. Its heap moved by more than
160 MB between two reads taken minutes apart against the same three sessions,
which is larger than any per-pane figure would have been, and adding eight
sessions did not move it out of that band.

The last row is a design constraint, not a defect: a renderer owns a font
stack and a glyph atlas, so panes share one renderer per window and pay
474 KiB each rather than 36 MiB each.

## Startup

Both builds were traced the same way: five runs on the measurement host, a
throwaway Xvfb at 1600x1000x24, a private `HOME` per run so every run reads
the same empty profile, and `vitrum --fixture`. Each mark is milliseconds
from the wall clock stamped immediately before `exec`.

| Mark | 0.3.1, webview shell | GTK 3 shell |
| --- | --- | --- |
| `window.created` | 71.5 | 51.0 |
| `frame.realized` | not marked | 73.7 |
| `shell.mounted` | 223.4, and 711.9 to 713.4 | 337.3 |
| `pane.first-paint` | never arrived | 353.2 |

The old figures are two modes, not one number. Time to a mounted shell was
bimodal from the same binary on the same display with the same profile: three
of five runs landed at 216.5, 223.4 and 228.1 ms, and two at 711.9 and
713.4 ms. A median over five runs of that distribution is whichever mode won
three times, so both are printed above.

The GTK figures are one mode. The five runs land at 333.8, 335.1, 337.3, 339.8
and 348.0 ms, and the 348.0 is the first run of the batch, before the loader
has the shared libraries in page cache. The spread is 14 ms, so the median is
a figure rather than a coin toss.

That is 20 ms off the window and 375 ms off the worst case, against 114 ms
added to the best case. The window is created earlier because nothing has to
start a web engine before it, and the shell mounts later because the whole
widget tree is built and styled before the frame is shown rather than after.
A run that used to be over seven tenths of a second no longer exists.

`pane.first-paint` is new here, and not because the mark was added: the old
build never reached it under Xvfb at all, because its terminal was a
JavaScript emulator inside a compositor that stalls on a display with no
direct rendering. The pane now paints its first frame 16 ms after the shell
mounts on a host with no GPU available to the process.

A different number from the same subject: the pane's own first frame is
192.5 ms at the median on the measurement host, measured as process creation
to an awaited fence with a grid full of content, off screen and with no
shell around it. The process's own clock from entering main to that fence is
205 ms on the development host, so process creation and dynamic linking
account for the rest. This is the cost of standing a pane up, not the cost of
starting the application.

## Regression bounds

Each signal has a bound recorded beside it in
`crates/vitrum-bench/src/latency.rs`. `latency --gate` exits non-zero when a
p99 or a worst case crosses one.

| Signal | Bound, p99 | Bound, worst |
|---|---:|---:|
| keystroke to painted glyph | 4 ms | 20 ms |
| agent output byte to painted glyph | 4 ms | 20 ms |
| first frame | 2000 ms | 4000 ms |
| frame time, full-screen redraw | 4 ms | 16 ms |
| frame time, scroll | 4 ms | 16 ms |
| frame time, resize | 16 ms | 60 ms |
| sidebar model update | 8 ms | 40 ms |
| resident bytes per extra pane | 4 MiB | 4 MiB |
| processor cost of a 60 Hz redraw | 0.250 core | 0.500 core |

The bounds sit well above the measured figures on purpose. A gate tuned to the
fastest host fails on a slower one and gets switched off. These catch an
order-of-magnitude regression, which is the failure worth a red build.

A signal with no measurement is a breach, not a pass, so the gate cannot be
switched off by deleting a measurement. A signal with no bound is a breach
too, so adding one turns the suite red until a bound is recorded for it.

## What is not measured

- End-to-end input to scanout on the new build. The figures stop at the queue
  fence. The display path is bounded separately as the platform floor.
- The shell around the pane. The sidebar, the status bar and the settings
  sheets are GTK widgets and are not covered by any figure here.
- Old-build resize and sidebar paint. There is no comparable measurement, so
  there is no ratio.
- Multi-monitor, fractional scaling and refresh rates above 60 Hz.
