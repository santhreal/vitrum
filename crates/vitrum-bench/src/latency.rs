//! Perceived-latency measurement for the native pane.
//!
//! Every other workload in this crate measures the daemon. This one measures
//! what a person waiting at the window actually experiences: the interval
//! between a cause and the pixels that answer it. Those intervals are the whole
//! argument for a native renderer, so they are measured rather than asserted.
//!
//! # What "painted" means here
//!
//! A frame is painted when the GPU has finished executing it, not when the
//! command buffer was handed to the driver. Every sample therefore ends in
//! `wgpu::Device::poll(wait_indefinitely())` after the submit, which returns
//! once the queue's fence for that submission has signalled. Timing to the
//! submit instead would report the CPU half of a frame and call it a frame.
//!
//! There is one thing after that fence which this harness cannot see: the
//! compositor's scanout. On a fixed-refresh display that adds up to one refresh
//! interval, the same for any renderer, and it is stated in `docs/performance.md`
//! rather than folded into a number here.
//!
//! # Why no display is needed
//!
//! The measured path is PTY -> [`vitrum_vt`] -> [`vitrum_grid::CellGrid`] ->
//! `wgpu`. None of it is a window. A live pane swaps the offscreen texture for
//! a swapchain texture and adds a present call; everything before that,
//! including the whole of the parse, damage and upload cost, is what this runs.
//! So the harness runs on a headless GPU, which is what makes it a gate rather
//! than a thing someone does by hand once.
//!
//! # Signals
//!
//! [`Signal`] is the list, and it is the list the gate is derived from. Adding
//! a variant without recording a bound for it turns the suite red, which is the
//! only way a new perceived-latency signal cannot quietly ship unguarded.

use std::io::{BufRead, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use vitrum_grid::{
    CellGrid, GpuContext, GridRenderer, HeadlessTarget, RendererConfig, Style,
};
use vitrum_model::order::{ActiveOrder, arrange};
use vitrum_model::rollup::rollup_all;
use vitrum_model::view::{Clock, SessionView};
use vitrum_proto::{Attention, ProjectId, SessionId, SessionInfo, SessionStatus};
use vitrum_vt::{ScrollViewport, Vt, VtOptions};

use crate::report::Report;
use crate::stats::Dist;

/// Grid the signals are measured at, in cells. A full-height agent TUI on a
/// laptop-sized window; measuring at 240x80 would flatter the per-cell figures
/// and describe a screen nobody has.
const COLS: u16 = 120;
const ROWS: u16 = 40;

/// Wall-clock ceiling for any single signal. A signal that has not finished its
/// samples by then is a failure with a name, not a harness that hangs. Every
/// measurement loop checks it, so the harness terminates whatever the GPU does.
const SIGNAL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a single sample may take before the run is abandoned. A frame that
/// takes this long is not a slow frame, it is a wedged device.
const SAMPLE_DEADLINE: Duration = Duration::from_secs(5);

/// What a measured number counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    /// Nanoseconds.
    Nanos,
    /// Bytes of resident memory.
    Bytes,
    /// Thousandths of one processor core.
    MilliCores,
}

impl Unit {
    /// Suffix used when a figure is printed.
    pub const fn suffix(self) -> &'static str {
        match self {
            Unit::Nanos => "ns",
            Unit::Bytes => "B",
            Unit::MilliCores => "mcore",
        }
    }
}

/// One perceived-latency signal.
///
/// The variants are the rows of the published table. [`Signal::ALL`] is what
/// the gate iterates, what the report iterates, and what the bound table is
/// checked against, so the three cannot disagree about which signals exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    /// A key is pressed and the glyph it produces is on the screen.
    KeystrokeToGlyph,
    /// A byte an agent wrote is on the screen.
    OutputToGlyph,
    /// A process starts and its first frame is finished.
    FirstFrame,
    /// One frame of a full-screen redraw.
    RedrawFrame,
    /// One frame of scrolling through scrollback.
    ScrollFrame,
    /// One frame of a resize.
    ResizeFrame,
    /// The sidebar model absorbs a fresh snapshot of many sessions.
    SidebarUpdate,
    /// Resident bytes one more pane costs.
    PaneMemory,
    /// Processor cost of holding a full-screen redraw at the refresh rate.
    PaintCpu,
}

impl Signal {
    /// Every signal, in the order the table prints them.
    pub const ALL: &'static [Signal] = &[
        Signal::KeystrokeToGlyph,
        Signal::OutputToGlyph,
        Signal::FirstFrame,
        Signal::RedrawFrame,
        Signal::ScrollFrame,
        Signal::ResizeFrame,
        Signal::SidebarUpdate,
        Signal::PaneMemory,
        Signal::PaintCpu,
    ];

    /// Stable key used in JSON and in the gate's bound table.
    pub const fn key(self) -> &'static str {
        match self {
            Signal::KeystrokeToGlyph => "keystroke_to_glyph",
            Signal::OutputToGlyph => "output_to_glyph",
            Signal::FirstFrame => "first_frame",
            Signal::RedrawFrame => "redraw_frame",
            Signal::ScrollFrame => "scroll_frame",
            Signal::ResizeFrame => "resize_frame",
            Signal::SidebarUpdate => "sidebar_update",
            Signal::PaneMemory => "pane_memory",
            Signal::PaintCpu => "paint_cpu",
        }
    }

    /// The row label.
    pub const fn title(self) -> &'static str {
        match self {
            Signal::KeystrokeToGlyph => "keystroke to painted glyph",
            Signal::OutputToGlyph => "agent output byte to painted glyph",
            Signal::FirstFrame => "process start to first painted frame",
            Signal::RedrawFrame => "frame time, full-screen redraw",
            Signal::ScrollFrame => "frame time, scrollback scroll",
            Signal::ResizeFrame => "frame time, resize",
            Signal::SidebarUpdate => "sidebar model update, 200 sessions",
            Signal::PaneMemory => "resident bytes per extra pane",
            Signal::PaintCpu => "processor cost of a 60 Hz full-screen redraw",
        }
    }

    /// What the number counts.
    pub const fn unit(self) -> Unit {
        match self {
            Signal::PaneMemory => Unit::Bytes,
            Signal::PaintCpu => Unit::MilliCores,
            _ => Unit::Nanos,
        }
    }

    /// How the figure is produced, printed beside it so a number is never
    /// published without its method.
    pub const fn method(self) -> &'static str {
        match self {
            Signal::KeystrokeToGlyph =>
                "one byte written to a pty master; the line discipline echoes it; the echo is read, parsed, synced into the grid, rendered and awaited on the queue fence",
            Signal::OutputToGlyph =>
                "a full-width line written to the pty slave; read from the master, parsed, synced, rendered and awaited on the queue fence",
            Signal::FirstFrame =>
                "a fresh child process is spawned; it acquires a GPU device, builds the renderer and font atlas, paints a grid and awaits the fence, then writes one line; the parent times spawn to that line",
            Signal::RedrawFrame =>
                "a full-screen repaint with a different colour per frame is fed, synced, rendered and awaited; every cell changes every frame",
            Signal::ScrollFrame =>
                "the viewport is moved one row through a populated scrollback, then synced, rendered and awaited",
            Signal::ResizeFrame =>
                "the engine and grid are resized, then the whole grid is re-rendered and awaited",
            Signal::SidebarUpdate =>
                "a daemon snapshot of 200 sessions is decoded from the wire, arranged into sections and rolled up per project",
            Signal::PaneMemory =>
                "resident set of a child process before and after it builds N further panes, each a parser, a grid and a render target; the difference divided by N",
            Signal::PaintCpu =>
                "a full-screen redraw is fed and painted on a 16.67 ms schedule; user plus system processor time of the whole process is read every quarter second and divided by the wall time of that window",
        }
    }
}

/// The ceiling a signal may not cross.
///
/// Two numbers because a distribution has two ways to fail: the typical frame
/// getting slower, and the worst frame getting slower while the typical one
/// holds. A hitch is what a person notices, so `max` is a bound and not a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    /// Ceiling for the 99th percentile.
    pub p99: u64,
    /// Ceiling for the worst sample.
    pub max: u64,
}

/// The published ceiling for every signal.
///
/// These are not the measured numbers. Each is set well above what this machine
/// measures, because the gate has to survive a slower machine, a busy machine
/// and a software rasteriser without crying wolf, while still catching a change
/// that gives back an order of magnitude. `docs/performance.md` states both the
/// measurement and the bound so the headroom is visible.
///
/// Returning `Option` rather than a total function is deliberate: [`bounds`]
/// proves at test time that no variant returns `None`, so a new signal cannot
/// be added without a decision being recorded here.
pub const fn bound(signal: Signal) -> Option<Bound> {
    Some(match signal {
        // 4 ms is a quarter of a 60 Hz frame. The measured figure is two
        // orders of magnitude under it, and the slack covers a host with a
        // software adapter rather than tuning the gate to the fastest machine
        // that runs it, which is how a gate ends up switched off.
        Signal::KeystrokeToGlyph => Bound {
            p99: 4_000_000,
            max: 20_000_000,
        },
        Signal::OutputToGlyph => Bound {
            p99: 4_000_000,
            max: 20_000_000,
        },
        // Device acquisition and font discovery dominate this one and are
        // driver work, so the bound is loose and guards a regression of the
        // kind that adds a second full initialisation.
        Signal::FirstFrame => Bound {
            p99: 2_000_000_000,
            max: 4_000_000_000,
        },
        // A 120x40 repaint has to fit in a 120 Hz frame with room for the rest
        // of the window, so 4 ms typical and 16 ms worst.
        Signal::RedrawFrame => Bound {
            p99: 4_000_000,
            max: 16_000_000,
        },
        Signal::ScrollFrame => Bound {
            p99: 4_000_000,
            max: 16_000_000,
        },
        // A resize rebuilds every instance and may add glyphs, so it gets a
        // frame of its own but no more.
        Signal::ResizeFrame => Bound {
            p99: 16_000_000,
            max: 60_000_000,
        },
        // Decoding and ordering 200 sessions is pure CPU and must not reach a
        // frame.
        Signal::SidebarUpdate => Bound {
            p99: 8_000_000,
            max: 40_000_000,
        },
        // A pane that costs more than 4 MB of parser, grid and target has
        // stopped being cheap. The webview client this replaces could not be
        // charged per pane at all: its heap moved by more than 100 MB between
        // two reads of the same three sessions.
        Signal::PaneMemory => Bound {
            p99: 4 * 1024 * 1024,
            max: 4 * 1024 * 1024,
        },
        // The webview client spent 1084 mcore holding one pane at 60 Hz. A
        // quarter of a core is the point past which the native renderer has
        // stopped being the cheap option.
        Signal::PaintCpu => Bound {
            p99: 250,
            max: 500,
        },
    })
}

/// Every signal paired with its bound, or the signals that have none.
///
/// # Errors
///
/// Names the signals [`bound`] did not answer for.
pub fn bounds() -> Result<Vec<(Signal, Bound)>, Vec<Signal>> {
    let mut ok = Vec::with_capacity(Signal::ALL.len());
    let mut missing = Vec::new();
    for &signal in Signal::ALL {
        match bound(signal) {
            Some(b) => ok.push((signal, b)),
            None => missing.push(signal),
        }
    }
    if missing.is_empty() { Ok(ok) } else { Err(missing) }
}


/// One signal's result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measured {
    pub signal: Signal,
    pub dist: Dist,
    /// Anything that qualifies the figure: how many frames actually reached the
    /// GPU, what the child reported, how many panes were built.
    pub note: String,
}

/// A signal that crossed its bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breach {
    pub signal: Signal,
    /// `p99` or `max`.
    pub stat: &'static str,
    pub measured: u64,
    pub limit: u64,
}

impl std::fmt::Display for Breach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} is {}{}, over the {}{} bound",
            self.signal.key(),
            self.stat,
            self.measured,
            self.signal.unit().suffix(),
            self.limit,
            self.signal.unit().suffix(),
        )
    }
}

/// Compare measurements against [`bound`].
///
/// A signal with no measurement is not a pass. It is reported as a breach with
/// a `missing` stat, because a gate that goes green when a signal stops being
/// measured is a gate that can be switched off by deleting a measurement.
pub fn check(measured: &[Measured]) -> Vec<Breach> {
    let mut out = Vec::new();
    for &signal in Signal::ALL {
        let Some(limit) = bound(signal) else {
            out.push(Breach {
                signal,
                stat: "bound",
                measured: 0,
                limit: 0,
            });
            continue;
        };
        let Some(m) = measured.iter().find(|m| m.signal == signal) else {
            out.push(Breach {
                signal,
                stat: "missing",
                measured: 0,
                limit: limit.p99,
            });
            continue;
        };
        if m.dist.p99 > limit.p99 {
            out.push(Breach {
                signal,
                stat: "p99",
                measured: m.dist.p99,
                limit: limit.p99,
            });
        }
        if m.dist.max > limit.max {
            out.push(Breach {
                signal,
                stat: "max",
                measured: m.dist.max,
                limit: limit.max,
            });
        }
    }
    out
}

/// What a run was asked to do.
#[derive(Debug, Clone)]
pub struct LatencySpec {
    /// Samples per frame-level signal.
    pub samples: usize,
    /// Child spawns for [`Signal::FirstFrame`].
    pub spawns: usize,
    /// Extra panes built for [`Signal::PaneMemory`].
    pub panes: usize,
    /// Sessions in the snapshot for [`Signal::SidebarUpdate`].
    pub sessions: usize,
    /// Quarter-second windows [`Signal::PaintCpu`] averages over.
    pub cpu_windows: usize,
    /// Fail the process when a bound is crossed.
    pub gate: bool,
    /// Force the software rasteriser, for a machine with no usable GPU.
    pub software: bool,
}

impl Default for LatencySpec {
    fn default() -> Self {
        LatencySpec {
            samples: 2000,
            spawns: 5,
            panes: 8,
            sessions: 200,
            cpu_windows: 8,
            gate: false,
            software: false,
        }
    }
}

// ---------------------------------------------------------------------------
// The pipeline under measurement
// ---------------------------------------------------------------------------

/// One GPU device, shared by every pane in the process.
///
/// The product acquires a device once and every pane draws through it. A
/// harness that built a device per pane would charge each pane a whole driver
/// context and report a footprint the product never pays.
fn context(software: bool) -> anyhow::Result<GpuContext> {
    if software {
        GpuContext::headless_software()
    } else {
        GpuContext::headless()
    }
    .context("acquiring a headless GPU device")
}

/// One session's own state: a parser, a grid and something to draw into.
///
/// Split from the renderer because the two scale differently. A window holds
/// one renderer and one of these per pane, so what "another pane costs" is the
/// size of this and not of a second font stack.
struct Surface {
    target: HeadlessTarget,
    vt: Vt,
    grid: CellGrid,
    viewport: (u32, u32),
}

/// A renderer and the surface it is currently drawing: one pane, minus its
/// window.
struct Pane<'g> {
    gpu: &'g GpuContext,
    renderer: GridRenderer,
    surface: Surface,
}

impl<'g> Pane<'g> {
    fn new(gpu: &'g GpuContext, scrollback: usize) -> anyhow::Result<Self> {
        let config = RendererConfig {
            format: HeadlessTarget::FORMAT,
            ..RendererConfig::default()
        };
        let renderer =
            GridRenderer::new(gpu.device(), &config).context("building the grid renderer")?;
        let surface = Self::surface(gpu, &renderer, scrollback)?;
        Ok(Pane {
            gpu,
            renderer,
            surface,
        })
    }

    /// Another pane's worth of state, drawn through this renderer.
    fn surface(
        gpu: &GpuContext,
        renderer: &GridRenderer,
        scrollback: usize,
    ) -> anyhow::Result<Surface> {
        let (cw, ch) = renderer.cell_size();
        let viewport = (cw * u32::from(COLS), ch * u32::from(ROWS));
        let target = HeadlessTarget::new(gpu.device(), viewport.0, viewport.1);
        let vt = Vt::new(VtOptions {
            cols: COLS,
            rows: ROWS,
            max_scrollback: scrollback,
        })
        .context("building the terminal engine")?;
        let grid = CellGrid::new(COLS, ROWS, Style::DEFAULT).context("building the cell grid")?;
        Ok(Surface {
            target,
            vt,
            grid,
            viewport,
        })
    }

    /// The engine this pane's own surface is driven by.
    fn vt(&mut self) -> &mut Vt {
        &mut self.surface.vt
    }

    /// Sync the engine into the grid, render, and wait for the GPU to finish.
    ///
    /// Returns whether the frame reached the GPU at all. A frame that did not
    /// is a real outcome of the damage contract, and it is counted rather than
    /// dropped, so a suspiciously fast distribution can be recognised as one
    /// full of no-ops.
    fn paint(&mut self) -> anyhow::Result<bool> {
        let Pane {
            gpu,
            renderer,
            surface,
        } = self;
        surface
            .vt
            .sync(&mut surface.grid)
            .context("syncing the engine")?;
        let stats = renderer
            .render(
                gpu.device(),
                gpu.queue(),
                &mut surface.grid,
                surface.target.view(),
                surface.viewport,
            )
            .context("rendering the frame")?;
        if stats.gpu_work {
            gpu.device()
                .poll(wgpu::PollType::wait_indefinitely())
                .context("waiting for the frame's fence")?;
        }
        Ok(stats.gpu_work)
    }

    /// Draw `surface` through this pane's renderer, as a window does when it
    /// paints its second tab.
    fn paint_other(&mut self, surface: &mut Surface) -> anyhow::Result<()> {
        surface
            .vt
            .sync(&mut surface.grid)
            .context("syncing the engine")?;
        let stats = self
            .renderer
            .render(
                self.gpu.device(),
                self.gpu.queue(),
                &mut surface.grid,
                surface.target.view(),
                surface.viewport,
            )
            .context("rendering the frame")?;
        if stats.gpu_work {
            self.gpu
                .device()
                .poll(wgpu::PollType::wait_indefinitely())
                .context("waiting for the frame's fence")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// A pty with nothing on the far end but the kernel
// ---------------------------------------------------------------------------

/// A pseudoterminal pair, used as the real transport between a keystroke and
/// the parser.
///
/// No child process runs on it. The line discipline is what echoes a keystroke,
/// which is exactly the hop a shell-less measurement wants: the interval being
/// measured is the product's, and a shell's scheduling would be added to every
/// sample without being part of what a renderer can improve.
struct Pty {
    master: libc::c_int,
    slave: libc::c_int,
}

impl Pty {
    fn open() -> anyhow::Result<Self> {
        let mut master = 0;
        let mut slave = 0;
        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        size.ws_col = COLS;
        size.ws_row = ROWS;
        // SAFETY: both out pointers are valid for the call and `size` is a
        // fully initialised winsize. The name and termios arguments are null,
        // which openpty documents as "use the defaults".
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw const size,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error()).context("opening a pseudoterminal");
        }
        let pty = Pty { master, slave };
        // Both ends. A blocking descriptor turns `drain` into a wait for bytes
        // that are never coming, and turns a full input queue into a write that
        // never returns. Every read here is guarded by `poll` instead.
        pty.set_nonblocking(pty.slave)?;
        pty.set_nonblocking(pty.master)?;
        Ok(pty)
    }

    fn set_nonblocking(&self, fd: libc::c_int) -> anyhow::Result<()> {
        // SAFETY: `fd` is one of the two descriptors this type owns.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error()).context("reading pty flags");
        }
        // SAFETY: as above.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error()).context("setting pty flags");
        }
        Ok(())
    }

    /// Write everything, waiting for room rather than spinning on `EAGAIN`.
    ///
    /// Bounded: a pty whose input queue never drains ends the run with a
    /// message instead of burning a core.
    fn write(&self, fd: libc::c_int, bytes: &[u8]) -> anyhow::Result<()> {
        let mut sent = 0;
        while sent < bytes.len() {
            // SAFETY: the slice is live for the call and `sent` is in bounds.
            let n = unsafe { libc::write(fd, bytes[sent..].as_ptr().cast(), bytes.len() - sent) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    self.wait(fd, libc::POLLOUT, 2000)
                        .context("waiting for room in a pty")?;
                    continue;
                }
                return Err(err).context("writing to a pty");
            }
            sent += n as usize;
        }
        Ok(())
    }

    /// Wait for `events` on `fd`, or report that they never came.
    fn wait(&self, fd: libc::c_int, events: libc::c_short, ms: i32) -> anyhow::Result<()> {
        let mut pfd = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: one valid pollfd, count 1.
        let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
        if rc < 0 {
            return Err(std::io::Error::last_os_error()).context("polling a pty");
        }
        if rc == 0 {
            bail!("the pty was not ready within {ms} ms");
        }
        Ok(())
    }

    /// Wait for the master to have bytes, then read once.
    ///
    /// The timeout is what keeps a measurement loop terminating: a pty that
    /// stops echoing ends the run with a message instead of parking the
    /// harness forever.
    fn read_master(&self, buf: &mut [u8], deadline: Duration) -> anyhow::Result<usize> {
        let ms = i32::try_from(deadline.as_millis()).unwrap_or(i32::MAX);
        loop {
            self.wait(self.master, libc::POLLIN, ms)
                .context("the pty produced no echo")?;
            // SAFETY: `buf` is live and its length is passed unchanged.
            let n = unsafe { libc::read(self.master, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    continue;
                }
                return Err(err).context("reading a pty");
            }
            return Ok(n as usize);
        }
    }

    /// Throw away whatever is queued on a descriptor.
    fn drain(&self, fd: libc::c_int) {
        let mut buf = [0u8; 4096];
        loop {
            // SAFETY: `buf` is live and its length is passed unchanged.
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                return;
            }
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // SAFETY: both descriptors are owned by this value and closed once.
        unsafe {
            libc::close(self.slave);
            libc::close(self.master);
        }
    }
}

/// The kernel's own echo, with nothing of this product in it.
///
/// A byte written to the master comes back from the master because the line
/// discipline echoed it. That turnaround is under every keystroke measurement
/// in this crate, local or through the daemon, and it belongs to the platform.
/// [`crate::world`] reports it as half of the floor it subtracts.
pub(crate) fn pty_echo(samples: usize) -> anyhow::Result<Dist> {
    let pty = Pty::open()?;
    let mut buf = [0u8; 256];
    // One turnaround before timing: the first read of a fresh pty pays for the
    // buffers the kernel allocates on it.
    pty.write(pty.master, b"w")?;
    let _ = pty.read_master(&mut buf, Duration::from_secs(2))?;
    pty.drain(pty.slave);
    pty.drain(pty.master);

    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        let ch = b'a' + (i % 26) as u8;
        let start = Instant::now();
        pty.write(pty.master, &[ch])?;
        let _ = pty.read_master(&mut buf, Duration::from_secs(2))?;
        out.push(start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64);
        pty.drain(pty.slave);
    }
    Dist::of(out)
}

// ---------------------------------------------------------------------------
// Signal measurements
// ---------------------------------------------------------------------------

/// Guards a measurement loop against never finishing.
struct Deadline {
    signal: Signal,
    start: Instant,
}

impl Deadline {
    fn new(signal: Signal) -> Self {
        Deadline {
            signal,
            start: Instant::now(),
        }
    }

    fn check(&self) -> anyhow::Result<()> {
        let spent = self.start.elapsed();
        if spent > SIGNAL_DEADLINE {
            bail!(
                "{} did not finish its samples in {:?}",
                self.signal.key(),
                SIGNAL_DEADLINE
            );
        }
        Ok(())
    }
}

/// Reject a sample that says the device stopped responding.
fn sane(signal: Signal, sample: Duration) -> anyhow::Result<u64> {
    if sample > SAMPLE_DEADLINE {
        bail!(
            "{} took {:?} for one sample, which is a wedged device rather than a slow one",
            signal.key(),
            sample
        );
    }
    Ok(sample.as_nanos() as u64)
}

/// Keystroke to painted glyph, through a real pty.
pub fn keystroke_to_glyph(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::KeystrokeToGlyph;
    let deadline = Deadline::new(signal);
    let pty = Pty::open()?;
    let gpu = context(spec.software)?;
    let mut pane = Pane::new(&gpu, 0)?;
    let mut buf = [0u8; 256];

    // Paint once so the atlas holds the glyphs the loop will use and the first
    // sample is not a font cache miss reported as a keystroke.
    for ch in b'a'..=b'z' {
        pty.write(pty.master, &[ch])?;
        let n = pty.read_master(&mut buf, Duration::from_secs(2))?;
        pane.vt().feed(&buf[..n]);
    }
    pane.paint()?;
    pty.drain(pty.slave);
    pty.drain(pty.master);

    let mut samples = Vec::with_capacity(spec.samples);
    let mut painted = 0usize;
    for i in 0..spec.samples {
        deadline.check()?;
        // A different character each time, so the cell it lands in really
        // changes and the frame is not skipped by the damage contract.
        let ch = b'a' + (i % 26) as u8;
        let start = Instant::now();
        pty.write(pty.master, &[ch])?;
        let n = pty.read_master(&mut buf, Duration::from_secs(2))?;
        pane.vt().feed(&buf[..n]);
        if pane.paint()? {
            painted += 1;
        }
        samples.push(sane(signal, start.elapsed())?);
        pty.drain(pty.slave);
    }

    let dist = Dist::of(samples)?;
    Ok(Measured {
        signal,
        dist,
        note: format!(
            "{painted} of {} samples reached the GPU; the rest landed on a cell that already held that glyph",
            spec.samples
        ),
    })
}

/// A byte an agent wrote to painted glyph.
pub fn output_to_glyph(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::OutputToGlyph;
    let deadline = Deadline::new(signal);
    let pty = Pty::open()?;
    let gpu = context(spec.software)?;
    let mut pane = Pane::new(&gpu, 0)?;
    let mut buf = [0u8; 8192];

    let mut samples = Vec::with_capacity(spec.samples);
    let mut painted = 0usize;
    for i in 0..spec.samples {
        deadline.check()?;
        // A full-width line, which is what an agent printing a wrapped
        // paragraph actually delivers, and it damages a whole row.
        let mut line = Vec::with_capacity(usize::from(COLS) + 2);
        let base = b'!' + (i % 60) as u8;
        for c in 0..COLS {
            line.push(base + (c % 20) as u8);
        }
        line.extend_from_slice(b"\r\n");

        let start = Instant::now();
        pty.write(pty.slave, &line)?;
        let n = pty.read_master(&mut buf, Duration::from_secs(2))?;
        pane.vt().feed(&buf[..n]);
        if pane.paint()? {
            painted += 1;
        }
        samples.push(sane(signal, start.elapsed())?);
    }

    let dist = Dist::of(samples)?;
    Ok(Measured {
        signal,
        dist,
        note: format!("{painted} of {} samples reached the GPU", spec.samples),
    })
}

/// One frame of a full-screen redraw.
pub fn redraw_frame(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::RedrawFrame;
    let deadline = Deadline::new(signal);
    let gpu = context(spec.software)?;
    let mut pane = Pane::new(&gpu, 0)?;

    // Warm the atlas with everything the loop will draw.
    pane.vt().feed(&repaint(0));
    pane.paint()?;

    let frames = spec.samples.min(600).max(60);
    let mut samples = Vec::with_capacity(frames);
    let mut skipped = 0usize;
    for i in 0..frames {
        deadline.check()?;
        let payload = repaint(i + 1);
        let start = Instant::now();
        pane.vt().feed(&payload);
        if !pane.paint()? {
            skipped += 1;
        }
        samples.push(sane(signal, start.elapsed())?);
    }

    let dist = Dist::of(samples)?;
    Ok(Measured {
        signal,
        dist,
        note: format!(
            "{frames} frames of {COLS}x{ROWS}, {} cells each; {skipped} frames found nothing changed",
            usize::from(COLS) * usize::from(ROWS)
        ),
    })
}

/// User plus system processor time this process has spent, in nanoseconds.
fn cpu_time() -> Duration {
    // SAFETY: `usage` is a live, correctly sized out parameter and the call
    // writes nothing else.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let seconds = usage.ru_utime.tv_sec + usage.ru_stime.tv_sec;
    let micros = usage.ru_utime.tv_usec + usage.ru_stime.tv_usec;
    Duration::from_secs(seconds as u64) + Duration::from_micros(micros as u64)
}

/// Processor cost of holding a full-screen redraw at the refresh rate.
///
/// Frame time says how long one frame takes. It does not say what the machine
/// paid for it, and a renderer that meets the frame budget by spinning a core
/// is the thing that makes a window feel slow while every frame arrives on
/// time. This paces a genuine full-screen redraw at 60 Hz and charges the
/// process for the whole window, threads included.
pub fn paint_cpu(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::PaintCpu;
    let deadline = Deadline::new(signal);
    let gpu = context(spec.software)?;
    let mut pane = Pane::new(&gpu, 0)?;
    pane.vt().feed(&repaint(0));
    pane.paint()?;

    let frame = Duration::from_nanos(16_666_667);
    let window = Duration::from_millis(250);
    let windows = spec.cpu_windows.max(4);
    let mut samples = Vec::with_capacity(windows);
    let mut frames = 0usize;
    let mut late = 0usize;

    for _ in 0..windows {
        deadline.check()?;
        let wall_start = Instant::now();
        let cpu_start = cpu_time();
        let mut next = wall_start;
        while wall_start.elapsed() < window {
            let payload = repaint(frames + 1);
            pane.vt().feed(&payload);
            pane.paint()?;
            frames += 1;
            next += frame;
            match next.checked_duration_since(Instant::now()) {
                Some(rest) => std::thread::sleep(rest),
                // The frame overran its slot. Count it and take the next slot
                // from now, so one slow frame cannot make the loop chase a
                // schedule it has already lost.
                None => {
                    late += 1;
                    next = Instant::now();
                }
            }
        }
        let wall = wall_start.elapsed();
        let cpu = cpu_time().saturating_sub(cpu_start);
        // Thousandths of a core, which is a whole number small enough to read
        // and large enough not to round a real cost to zero.
        samples.push((cpu.as_nanos() * 1000 / wall.as_nanos().max(1)) as u64);
    }

    let dist = Dist::of(samples)?;
    Ok(Measured {
        signal,
        dist,
        note: format!(
            "{frames} frames of {COLS}x{ROWS} paced at 60 Hz over {windows} quarter-second windows; {late} frames missed their slot"
        ),
    })
}

/// A full-screen repaint whose colours differ per frame, so no cell can match
/// what is already on screen and the whole grid is genuinely re-uploaded.
fn repaint(frame: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(usize::from(COLS) * usize::from(ROWS) * 24);
    out.extend_from_slice(b"\x1b[H");
    for row in 0..ROWS {
        let r = ((frame * 7 + usize::from(row) * 3) % 200 + 20) as u8;
        let g = ((frame * 11 + usize::from(row) * 5) % 200 + 20) as u8;
        let b = ((frame * 13 + usize::from(row) * 7) % 200 + 20) as u8;
        out.extend_from_slice(format!("\x1b[38;2;{r};{g};{b}m").as_bytes());
        for col in 0..COLS {
            // Box drawing and text, which is what a TUI redraw is made of.
            let ch = match (usize::from(col) + frame) % 8 {
                0 => '│',
                1 => '─',
                2 => '┼',
                n => char::from(b'a' + n as u8),
            };
            let mut buf = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
        if row + 1 < ROWS {
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b"\x1b[0m");
    out
}

/// One frame of scrolling through scrollback.
pub fn scroll_frame(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::ScrollFrame;
    let deadline = Deadline::new(signal);
    let scrollback_rows = 4000usize;
    let gpu = context(spec.software)?;
    let mut pane = Pane::new(&gpu, scrollback_rows * usize::from(COLS) * 4)?;

    // Fill the scrollback with distinguishable rows so a viewport move really
    // changes every row on screen.
    let mut fill = Vec::with_capacity(scrollback_rows * (usize::from(COLS) + 2));
    for row in 0..scrollback_rows {
        let base = b'!' + (row % 60) as u8;
        for c in 0..COLS {
            fill.push(base + (c % 20) as u8);
        }
        fill.extend_from_slice(b"\r\n");
    }
    pane.vt().feed(&fill);
    pane.paint()?;

    let frames = spec.samples.min(600).max(60);
    let mut samples = Vec::with_capacity(frames);
    let mut skipped = 0usize;
    for i in 0..frames {
        deadline.check()?;
        // Alternate direction so the viewport does not run off the top and
        // start producing no-op frames halfway through the run.
        let delta = if (i / 200) % 2 == 0 { -1 } else { 1 };
        let start = Instant::now();
        pane.vt().scroll(ScrollViewport::Delta(delta));
        if !pane.paint()? {
            skipped += 1;
        }
        samples.push(sane(signal, start.elapsed())?);
    }

    let dist = Dist::of(samples)?;
    Ok(Measured {
        signal,
        dist,
        note: format!(
            "{frames} one-row viewport moves over {scrollback_rows} rows of scrollback; {skipped} found nothing changed"
        ),
    })
}

/// One frame of a resize.
pub fn resize_frame(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::ResizeFrame;
    let deadline = Deadline::new(signal);
    let gpu = context(spec.software)?;
    let mut pane = Pane::new(&gpu, 0)?;
    pane.vt().feed(&repaint(0));
    pane.paint()?;

    let (cw, ch) = pane.renderer.cell_size();
    let steps = spec.samples.min(400).max(40);
    let mut samples = Vec::with_capacity(steps);
    for i in 0..steps {
        deadline.check()?;
        // A drag across a real range of widths and heights rather than one
        // size toggled back and forth, which would let a cache answer.
        let cols = 60 + (i % 61) as u16;
        let rows = 20 + (i % 21) as u16;
        let viewport = (cw * u32::from(cols), ch * u32::from(rows));

        let start = Instant::now();
        pane.vt().resize(cols, rows, (cw, ch))?;
        pane.surface.grid.resize(cols, rows)?;
        pane.surface.target = HeadlessTarget::new(gpu.device(), viewport.0, viewport.1);
        pane.surface.viewport = viewport;
        pane.paint()?;
        samples.push(sane(signal, start.elapsed())?);
    }

    let dist = Dist::of(samples)?;
    Ok(Measured {
        signal,
        dist,
        note: format!(
            "{steps} resizes across 60-120 columns and 20-40 rows, each rebuilding every instance and reallocating the target"
        ),
    })
}

/// The sidebar model absorbing a fresh snapshot.
///
/// This is the client-side model update: the wire payload is decoded, the rows
/// are arranged into their sections and each project is rolled up. The paint of
/// those rows belongs to the shell and is not in this figure, which is stated
/// beside it in `docs/performance.md` rather than left for a reader to assume.
pub fn sidebar_update(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::SidebarUpdate;
    let deadline = Deadline::new(signal);
    let payload = snapshot_json(spec.sessions)?;
    let clock = Clock::utc(1_700_000_000_000);
    let policy = vitrum_model::disposition::DispositionPolicy::default();

    let iterations = spec.samples.min(2000).max(50);
    let mut samples = Vec::with_capacity(iterations);
    let mut rows_seen = 0usize;
    for _ in 0..iterations {
        deadline.check()?;
        let start = Instant::now();
        let infos: Vec<vitrum_proto::SessionInfo> =
            serde_json::from_str(&payload).context("decoding a sessions snapshot")?;
        let mut views: Vec<SessionView> = infos.into_iter().map(SessionView::new).collect();
        arrange(&mut views, clock, policy, ActiveOrder::default());
        let rollups = rollup_all(&views, clock, policy);
        rows_seen = views.len();
        std::hint::black_box(&rollups);
        samples.push(sane(signal, start.elapsed())?);
    }

    let dist = Dist::of(samples)?;
    Ok(Measured {
        signal,
        dist,
        note: format!(
            "{iterations} snapshots of {rows_seen} sessions, decoded from the wire form, arranged and rolled up"
        ),
    })
}

/// A daemon snapshot as it arrives on the wire.
///
/// Built from real [`vitrum_proto::SessionInfo`] values and serialised, not
/// from hand-written JSON. A hand-written payload drifts from the wire the
/// moment a field is added, and a snapshot that no longer parses is a
/// measurement of the error path.
fn snapshot_json(sessions: usize) -> anyhow::Result<String> {
    let commands = ["claude", "codex", "gemini"];
    let list: Vec<SessionInfo> = (0..sessions)
        .map(|i| SessionInfo {
            id: SessionId(i as u64 + 1),
            project_id: ProjectId((i % 12) as u64 + 1),
            title: format!("session {i}"),
            cwd: format!("/src/project-{}", i % 12),
            command: commands[i % commands.len()].to_string(),
            args: vec!["--resume".to_string()],
            status: SessionStatus::Running,
            created_at_ms: 1_699_000_000_000 + i as u64,
            last_activity_ms: 1_699_900_000_000 + i as u64,
            cols: COLS,
            rows: ROWS,
            git_branch: Some(format!("topic-{}", i % 7)),
            unread: i % 3 == 0,
            // A tenth of the rows are asking for the operator, which is the
            // case the ordering has to do work in.
            attention: Attention {
                bell: i % 10 == 0,
                waiting: Some(i % 5 == 0),
                ..Attention::default()
            },
            hint: None,
            term_title: Some(format!("Ready (session {i})")),
            worktree: (i % 4 != 0).then(|| format!("wt-{}", i % 4)),
        })
        .collect();
    serde_json::to_string(&list).context("serialising a sessions snapshot")
}

// ---------------------------------------------------------------------------
// Child-process signals
// ---------------------------------------------------------------------------

/// The line the child writes once its first frame has been executed by the GPU.
const READY_PREFIX: &str = "vitrum-latency-ready ";

/// Process start to first painted frame.
///
/// The parent times from `spawn` to the child's line, so the figure contains
/// process creation, dynamic linking, GPU device acquisition, font discovery,
/// atlas construction, the first upload, the draw and the fence. A window
/// system would add mapping the window on top of that, which no headless
/// measurement can produce and which is stated as an exclusion rather than
/// estimated.
pub fn first_frame(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::FirstFrame;
    let deadline = Deadline::new(signal);
    let exe = std::env::current_exe().context("finding this binary to re-execute")?;

    let spawns = spec.spawns.max(3);
    let mut samples = Vec::with_capacity(spawns);
    let mut inner = Vec::with_capacity(spawns);
    let mut pane_bytes = Vec::with_capacity(spawns);
    let mut renderer_bytes = Vec::with_capacity(spawns);
    let mut total_bytes = Vec::with_capacity(spawns);
    for _ in 0..spawns {
        deadline.check()?;
        let start = Instant::now();
        let mut child = Command::new(&exe)
            .arg("latency-child")
            .arg("--panes")
            .arg(spec.panes.to_string())
            .args(if spec.software {
                vec!["--software".to_string()]
            } else {
                Vec::new()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawning the first-frame child")?;
        let stdout = child
            .stdout
            .take()
            .context("the first-frame child has no stdout")?;
        let mut lines = BufReader::new(stdout).lines();
        let ready = lines
            .next()
            .context("the first-frame child exited before painting")?
            .context("reading the first-frame child")?;
        let elapsed = start.elapsed();
        let Some(rest) = ready.strip_prefix(READY_PREFIX) else {
            bail!("the first-frame child wrote {ready:?} instead of a ready line");
        };
        inner.push(
            rest.trim()
                .parse::<u64>()
                .context("the child's own first-frame nanoseconds")?,
        );
        // The next two lines carry the footprints measured inside the same
        // child, so the memory signal costs no extra process.
        for line in lines.by_ref().take(3) {
            let line = line.context("reading the first-frame child")?;
            if let Some(bytes) = line.strip_prefix(PANE_BYTES_PREFIX) {
                pane_bytes.push(bytes.trim().parse::<u64>().unwrap_or(0));
            } else if let Some(bytes) = line.strip_prefix(RENDERER_BYTES_PREFIX) {
                renderer_bytes.push(bytes.trim().parse::<u64>().unwrap_or(0));
            } else if let Some(bytes) = line.strip_prefix(TOTAL_BYTES_PREFIX) {
                total_bytes.push(bytes.trim().parse::<u64>().unwrap_or(0));
            }
        }
        let status = child.wait().context("waiting for the first-frame child")?;
        if !status.success() {
            bail!("the first-frame child exited with {status}");
        }
        samples.push(sane(signal, elapsed)?);
    }

    let inner_dist = Dist::of(inner)?;
    let dist = Dist::of(samples)?;
    FOOTPRINTS.with(|cell| *cell.borrow_mut() = (pane_bytes, renderer_bytes, total_bytes));
    Ok(Measured {
        signal,
        dist,
        note: format!(
            "{spawns} spawns; the child's own clock from entering main to the fence was p50 {} ns, so process creation and dynamic linking account for the difference",
            inner_dist.p50
        ),
    })
}

/// Prefixes the child writes its footprint lines with.
const PANE_BYTES_PREFIX: &str = "vitrum-latency-pane-bytes ";
const RENDERER_BYTES_PREFIX: &str = "vitrum-latency-renderer-bytes ";
const TOTAL_BYTES_PREFIX: &str = "vitrum-latency-total-bytes ";

thread_local! {
    /// Per-surface and per-renderer footprints the first-frame children
    /// reported, so [`pane_memory`] does not spawn a second round of processes.
    static FOOTPRINTS: std::cell::RefCell<(Vec<u64>, Vec<u64>, Vec<u64>)> =
        const { std::cell::RefCell::new((Vec::new(), Vec::new(), Vec::new())) };
}

/// Resident bytes one more pane costs.
///
/// Measured inside the first-frame children: each reads its own resident set,
/// builds `--panes` further panes through the renderer it already has, reads it
/// again, and divides. Resident set rather than a heap counter because the
/// question is what the operating system charges for a pane, and a grid, an
/// atlas and a driver allocation are not all on the Rust heap.
///
/// The note carries the other half of the answer: what a second *renderer*
/// costs. A renderer owns a font stack and a glyph atlas, and building one per
/// pane rather than one per window is the difference between the two figures.
pub fn pane_memory(spec: &LatencySpec) -> anyhow::Result<Measured> {
    let signal = Signal::PaneMemory;
    let (samples, renderers, totals) = FOOTPRINTS.with(|cell| cell.borrow().clone());
    if samples.is_empty() {
        bail!("no child reported a per-pane footprint; first_frame has to run first");
    }
    let renderer = Dist::of(renderers).map(|d| d.p50).unwrap_or(0);
    let total = Dist::of(totals).map(|d| d.p50).unwrap_or(0);
    let dist = Dist::of(samples)?;
    let count = dist.count;
    Ok(Measured {
        signal,
        dist,
        note: format!(
            "{count} children, each building {} further panes of parser, grid and render target through the renderer already there; the whole process with one pane, a renderer, a font stack and a Vulkan device resident is {}; a second renderer, with its own font stack and glyph atlas, costs {} on top",
            spec.panes.max(1),
            human_bytes(total),
            human_bytes(renderer),
        ),
    })
}

/// The child half of [`first_frame`] and [`pane_memory`].
///
/// Writes three lines and exits. Nothing else may go to stdout: the parent
/// reads the first line as the ready marker and times to it.
pub fn child(panes: usize, software: bool) -> anyhow::Result<()> {
    let entered = Instant::now();
    let gpu = context(software)?;
    let mut pane = Pane::new(&gpu, 0)?;
    pane.vt().feed(&repaint(0));
    pane.paint()?;
    let first = entered.elapsed().as_nanos() as u64;

    let mut out = std::io::stdout().lock();
    writeln!(out, "{READY_PREFIX}{first}")?;
    out.flush()?;

    // What another pane costs a window that already has one: a parser, a grid
    // and a target, drawn through the renderer that is already there.
    let panes = panes.max(1);
    // Read before any extra pane exists, so this is one whole window's worth:
    // a parser, a grid, a renderer with its font stack and atlas, and the
    // Vulkan device. It is the figure a webview client's process tree is
    // comparable against, which a per-pane delta is not.
    let before = resident_bytes()?;
    writeln!(out, "{TOTAL_BYTES_PREFIX}{before}")?;
    let mut extra = Vec::with_capacity(panes);
    for _ in 0..panes {
        let mut surface = Pane::surface(&gpu, &pane.renderer, 0)?;
        surface.vt.feed(&repaint(1));
        pane.paint_other(&mut surface)?;
        extra.push(surface);
    }
    let after = resident_bytes()?;
    let per_surface = after.saturating_sub(before) / panes as u64;
    writeln!(out, "{PANE_BYTES_PREFIX}{per_surface}")?;

    // And what a second renderer costs, which is the number that decides
    // whether a renderer may be per pane at all.
    let before_renderer = resident_bytes()?;
    let second = Pane::new(&gpu, 0)?;
    let after_renderer = resident_bytes()?;
    let per_renderer = after_renderer.saturating_sub(before_renderer);
    writeln!(out, "{RENDERER_BYTES_PREFIX}{per_renderer}")?;
    out.flush()?;

    drop(second);
    drop(extra);
    Ok(())
}

/// This process's resident set in bytes.
fn resident_bytes() -> anyhow::Result<u64> {
    let status = std::fs::read_to_string("/proc/self/status")
        .context("reading this process's resident set")?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest
                .split_whitespace()
                .next()
                .context("VmRSS has no value")?
                .parse()
                .context("VmRSS is not a number")?;
            return Ok(kb * 1024);
        }
    }
    bail!("no VmRSS line in /proc/self/status")
}

// ---------------------------------------------------------------------------
// The run
// ---------------------------------------------------------------------------

/// Measure every signal and, when asked, gate on the bounds.
pub fn run(spec: &LatencySpec) -> anyhow::Result<Report> {
    let started = Instant::now();
    let gpu = context(spec.software)?;
    let adapter = gpu.describe();
    let class = format!("{:?}", gpu.class());
    drop(gpu);

    let mut measured = Vec::new();
    // first_frame runs before pane_memory: the children it spawns report both.
    measured.push(first_frame(spec)?);
    measured.push(pane_memory(spec)?);
    measured.push(keystroke_to_glyph(spec)?);
    measured.push(output_to_glyph(spec)?);
    measured.push(redraw_frame(spec)?);
    measured.push(scroll_frame(spec)?);
    measured.push(resize_frame(spec)?);
    measured.push(sidebar_update(spec)?);
    // Last: it holds a core busy for several seconds and would otherwise
    // contend with the frame-time signals for the same device.
    measured.push(paint_cpu(spec)?);

    let breaches = check(&measured);

    let mut report = Report::new(
        "latency",
        "none: this workload measures the pane, not the daemon",
        json!({
            "samples": spec.samples,
            "spawns": spec.spawns,
            "panes": spec.panes,
            "sessions": spec.sessions,
            "cpu_windows": spec.cpu_windows,
            "cols": COLS,
            "rows": ROWS,
            "adapter": adapter,
            "adapter_class": class,
            "software_forced": spec.software,
        }),
    );
    report.duration_secs = started.elapsed().as_secs_f64();
    report.extra = json!({
        "signals": measured
            .iter()
            .map(|m| json!({
                "key": m.signal.key(),
                "title": m.signal.title(),
                "unit": m.signal.unit().suffix(),
                "method": m.signal.method(),
                "dist": m.dist,
                "note": m.note,
                "bound": bound(m.signal),
            }))
            .collect::<Vec<_>>(),
        "breaches": breaches,
    });
    for m in &measured {
        report
            .checks_passed
            .push(format!("{} measured over {} samples", m.signal.key(), m.dist.count));
    }
    if spec.gate {
        for b in &breaches {
            report.failures.push(b.to_string());
        }
    }
    Ok(report)
}

/// The table, as a run prints it.
pub fn table(report: &Report) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "| signal | p50 | p95 | p99 | worst | bound p99 | bound worst |"
    );
    let _ = writeln!(s, "|---|---:|---:|---:|---:|---:|---:|");
    let empty = Vec::new();
    let signals = report.extra["signals"].as_array().unwrap_or(&empty);
    for entry in signals {
        let key = entry["key"].as_str().unwrap_or("?");
        let unit = entry["unit"].as_str().unwrap_or("");
        let d = &entry["dist"];
        let b = &entry["bound"];
        let fmt = |v: &serde_json::Value| -> String {
            match (v.as_u64(), unit) {
                (Some(n), "ns") => human_ns(n),
                (Some(n), "B") => human_bytes(n),
                (Some(n), "mcore") => human_cores(n),
                (Some(n), _) => n.to_string(),
                (None, _) => "-".to_string(),
            }
        };
        let _ = writeln!(
            s,
            "| {key} | {} | {} | {} | {} | {} | {} |",
            fmt(&d["p50"]),
            fmt(&d["p95"]),
            fmt(&d["p99"]),
            fmt(&d["max"]),
            fmt(&b["p99"]),
            fmt(&b["max"]),
        );
    }
    s
}

/// Nanoseconds at a scale a person reads.
pub fn human_ns(ns: u64) -> String {
    if ns < 10_000 {
        format!("{ns} ns")
    } else if ns < 10_000_000 {
        format!("{:.3} ms", ns as f64 / 1_000_000.0)
    } else {
        format!("{:.1} ms", ns as f64 / 1_000_000.0)
    }
}

/// Bytes at a scale a person reads.
pub fn human_bytes(b: u64) -> String {
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", b as f64 / (1024.0 * 1024.0))
    }
}

/// Thousandths of a core as a fraction of one.
pub fn human_cores(mcore: u64) -> String {
    format!("{:.3} core", mcore as f64 / 1000.0)
}

/// The gate's own suite.
///
/// # Why
///
/// These close the class "a latency signal stops being enforced". The three
/// ways that happens are a new signal added with no bound, a signal that stops
/// being measured, and a bound that is crossed without anything failing. Each
/// is checked against [`Signal::ALL`] read at run time, so adding a variant
/// turns the suite red until a bound and a case exist for it.
///
/// What they do not catch: whether a bound is the right number. That is a
/// judgement recorded in the comments beside [`bound`] and in
/// `docs/performance.md`, and no test can hold it.
#[cfg(test)]
mod tests {
    use super::*;

    /// A measurement that sits exactly on a signal's bound.
    fn at_bound(signal: Signal) -> Measured {
        let limit = bound(signal).expect("every signal has a bound");
        Measured {
            signal,
            dist: Dist {
                count: 100,
                min: 0,
                p50: 0,
                p95: 0,
                p99: limit.p99,
                max: limit.max,
                mean: 0,
            },
            note: String::new(),
        }
    }

    fn full_set() -> Vec<Measured> {
        Signal::ALL.iter().copied().map(at_bound).collect()
    }

    #[test]
    fn every_signal_has_a_bound() {
        match bounds() {
            Ok(pairs) => assert_eq!(pairs.len(), Signal::ALL.len()),
            Err(missing) => panic!(
                "no bound recorded for {:?}; add one to `bound` before publishing the signal",
                missing.iter().map(|s| s.key()).collect::<Vec<_>>()
            ),
        }
    }

    #[test]
    fn keys_and_titles_are_unique() {
        let mut keys: Vec<&str> = Signal::ALL.iter().map(|s| s.key()).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two signals share a key");

        let mut titles: Vec<&str> = Signal::ALL.iter().map(|s| s.title()).collect();
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), count, "two signals share a row label");
    }

    #[test]
    fn every_signal_states_its_method() {
        for &signal in Signal::ALL {
            let method = signal.method();
            assert!(
                method.len() > 40,
                "{} has no usable method sentence; a figure without one cannot be published",
                signal.key()
            );
        }
    }

    #[test]
    fn a_full_set_at_the_bound_passes() {
        assert_eq!(check(&full_set()), Vec::new());
    }

    #[test]
    fn one_over_the_p99_bound_is_a_breach() {
        for &signal in Signal::ALL {
            let mut set = full_set();
            let entry = set.iter_mut().find(|m| m.signal == signal).unwrap();
            entry.dist.p99 += 1;
            entry.dist.max = entry.dist.max.max(entry.dist.p99);
            let breaches = check(&set);
            assert!(
                breaches.iter().any(|b| b.signal == signal && b.stat == "p99"),
                "{} went one over its p99 bound and the gate stayed green",
                signal.key()
            );
        }
    }

    #[test]
    fn one_over_the_worst_bound_is_a_breach() {
        for &signal in Signal::ALL {
            let mut set = full_set();
            let entry = set.iter_mut().find(|m| m.signal == signal).unwrap();
            entry.dist.max += 1;
            let breaches = check(&set);
            assert!(
                breaches.iter().any(|b| b.signal == signal && b.stat == "max"),
                "{} went one over its worst-case bound and the gate stayed green",
                signal.key()
            );
        }
    }

    #[test]
    fn a_signal_that_stopped_being_measured_is_a_breach() {
        for &signal in Signal::ALL {
            let set: Vec<Measured> = full_set().into_iter().filter(|m| m.signal != signal).collect();
            let breaches = check(&set);
            assert!(
                breaches.iter().any(|b| b.signal == signal && b.stat == "missing"),
                "{} was not measured at all and the gate stayed green",
                signal.key()
            );
        }
    }

    #[test]
    fn a_breach_names_the_signal_the_stat_and_both_numbers() {
        let mut set = full_set();
        let entry = set
            .iter_mut()
            .find(|m| m.signal == Signal::KeystrokeToGlyph)
            .unwrap();
        entry.dist.p99 = bound(Signal::KeystrokeToGlyph).unwrap().p99 * 3;
        entry.dist.max = entry.dist.p99;
        let text = check(&set)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("keystroke_to_glyph"), "{text}");
        assert!(text.contains("p99"), "{text}");
        assert!(text.contains("ns"), "{text}");
    }

    #[test]
    fn units_read_at_their_own_scale() {
        assert_eq!(human_ns(900), "900 ns");
        assert_eq!(human_ns(54_000), "0.054 ms");
        assert_eq!(human_bytes(483_840), "472.5 KiB");
        assert_eq!(human_cores(1084), "1.084 core");
    }

    #[test]
    fn percentiles_are_samples_that_were_taken() {
        let dist = Dist::of((1..=100).collect()).unwrap();
        assert_eq!(dist.min, 1);
        assert_eq!(dist.p50, 50);
        assert_eq!(dist.p95, 95);
        assert_eq!(dist.p99, 99);
        assert_eq!(dist.max, 100);
        assert_eq!(dist.mean, 50);
    }

    #[test]
    fn an_empty_set_has_no_distribution() {
        assert!(Dist::of(Vec::new()).is_err());
    }
}
