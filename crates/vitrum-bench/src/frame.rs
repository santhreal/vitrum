//! Where one frame's time goes, measured inside the process that spends it.
//!
//! Every other measurement in this crate ends at a process boundary. `latency`
//! times a child from `spawn` to its first painted frame, `pipeline` re-execs
//! this binary as the thing under load, and the wire workloads cross a socket.
//! Each of those pays for the boundary, and the price is a floor under the
//! result and a frame that can only be reported as one number. A frame that got
//! 30% slower is then a fact with nowhere to go: the escape-sequence parser,
//! the projection onto the grid, the damage walk, the buffer writes and the
//! submission all live inside that one number and are fixed in different files.
//!
//! This workload runs the real path in this process and reads
//! [`vitrum_grid::probe`], which the renderer and the VT engine record into.
//! The result is per-phase distributions for the same frames whose total is
//! also reported, so a regression can be localised rather than merely detected.
//!
//! # The arms
//!
//! The probe is not in a default build at all: its call sites are behind the
//! `probe` cargo feature. So one binary can run only the arms it was compiled
//! for, and the zero-cost claim needs two binaries:
//!
//! - [`Arm::Absent`] — built without the feature. No probe instruction exists.
//! - [`Arm::Off`] — built with the feature, switch off. One relaxed load and a
//!   not-taken branch per span.
//! - [`Arm::On`] — built with the feature, switch on. Recording.
//!
//! `harness/frame.sh` is the command that runs all three: it builds both
//! binaries, alternates them round by round so drift cannot land on one arm,
//! and pairs the rounds to get the `off` minus `absent` difference and the
//! `absent` versus `absent` noise band it has to be judged against.
//!
//! # The skipped frame
//!
//! A frame with no damage returns before recording any GPU command, which is
//! why an idle window costs nothing. A probe that had to make that frame run in
//! order to time it would have destroyed the thing it measures, so the idle
//! frame is measured as its own signal in every arm and the run fails if a
//! skipped frame ever reports GPU work or opens a single span.

use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use vitrum_grid::probe::{self, Phase};
use vitrum_grid::{CellGrid, GpuContext, GridRenderer, HeadlessTarget, RendererConfig, Style};
use vitrum_vt::{Vt, VtOptions};

use crate::report::Report;
use crate::rng::Rng;
use crate::stats::Dist;

/// A sample this long is a wedged device, not a slow frame.
const SAMPLE_DEADLINE: Duration = Duration::from_secs(5);

/// Which build, and which switch position, a set of samples came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arm {
    /// Built without the `probe` feature.
    Absent,
    /// Built with the feature, recording off.
    Off,
    /// Built with the feature, recording on.
    On,
}

impl Arm {
    /// The arm's name in the report, and the key the comparator joins on.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Arm::Absent => "absent",
            Arm::Off => "off",
            Arm::On => "on",
        }
    }

    /// The arms this binary can run.
    ///
    /// A build decides its own arms rather than taking them from a flag: a
    /// caller asking for `on` from a binary with no probe in it would get
    /// zeros and no way to tell them from a frame that spent no time.
    #[must_use]
    pub fn compiled() -> &'static [Arm] {
        if cfg!(feature = "probe") {
            &[Arm::Off, Arm::On]
        } else {
            &[Arm::Absent]
        }
    }
}

/// What one run was asked to do.
#[derive(Debug, Clone)]
pub struct FrameSpec {
    /// Frames per arm per round.
    pub frames: usize,
    /// How many times the arms are measured, alternating between them.
    ///
    /// Rounds exist so a comparison is paired. A machine that gets slower
    /// halfway through a run charges the second arm for it when the arms run
    /// once each, and charges both equally when they alternate.
    pub rounds: usize,
    /// Grid the frames are painted at, in cells.
    pub cols: u16,
    pub rows: u16,
    /// Seed for the byte stream every arm is fed.
    pub seed: u64,
    /// Use a software rasteriser rather than the GPU.
    pub software: bool,
}

impl Default for FrameSpec {
    fn default() -> Self {
        Self {
            frames: 500,
            rounds: 5,
            cols: 120,
            rows: 40,
            seed: 1,
            software: false,
        }
    }
}

/// One pane's worth of state, with no fence wait in the measured region.
///
/// [`crate::latency`] has a similar fixture and a different job: it ends every
/// sample on the GPU's fence, because it reports what a person waiting at the
/// window experiences. The phases here are all on the CPU, and a fence wait
/// inside the sample would bury them in scheduling.
struct Bed {
    gpu: GpuContext,
    renderer: GridRenderer,
    target: HeadlessTarget,
    vt: Vt,
    grid: CellGrid,
    viewport: (u32, u32),
}

impl Bed {
    fn new(spec: &FrameSpec) -> anyhow::Result<Self> {
        let gpu = if spec.software {
            GpuContext::headless_software()
        } else {
            GpuContext::headless()
        }
        .context("acquiring a headless GPU device")?;
        let config = RendererConfig {
            format: HeadlessTarget::FORMAT,
            ..RendererConfig::default()
        };
        let renderer =
            GridRenderer::new(gpu.device(), &config).context("building the grid renderer")?;
        let (cw, ch) = renderer.cell_size();
        let viewport = (cw * u32::from(spec.cols), ch * u32::from(spec.rows));
        let target = HeadlessTarget::new(gpu.device(), viewport.0, viewport.1);
        let vt = Vt::new(VtOptions {
            cols: spec.cols,
            rows: spec.rows,
            max_scrollback: 1 << 20,
        })
        .context("building the terminal engine")?;
        let grid =
            CellGrid::new(spec.cols, spec.rows, Style::DEFAULT).context("building the cell grid")?;
        Ok(Bed {
            gpu,
            renderer,
            target,
            vt,
            grid,
            viewport,
        })
    }

    /// Feed `bytes`, sync, and render. Returns the elapsed time and whether the
    /// frame reached the GPU.
    ///
    /// The fence is waited on outside the timed region, so the queue stays
    /// bounded without the wait landing in a sample.
    fn frame(&mut self, bytes: &[u8]) -> anyhow::Result<(u64, bool)> {
        let start = Instant::now();
        self.vt.feed(bytes);
        self.vt
            .sync(&mut self.grid)
            .map_err(|e| anyhow::anyhow!("syncing the engine: {e}"))?;
        let stats = self
            .renderer
            .render(
                self.gpu.device(),
                self.gpu.queue(),
                &mut self.grid,
                self.target.view(),
                self.viewport,
            )
            .map_err(|e| anyhow::anyhow!("rendering the frame: {e}"))?;
        let elapsed = start.elapsed();
        if stats.gpu_work {
            self.gpu
                .device()
                .poll(wgpu::PollType::wait_indefinitely())
                .context("waiting for the frame's fence")?;
        }
        if elapsed > SAMPLE_DEADLINE {
            bail!("a single frame took {elapsed:?}; the device is not responding");
        }
        Ok((elapsed.as_nanos().min(u128::from(u64::MAX)) as u64, stats.gpu_work))
    }

    /// Render with nothing changed. This is the frame the damage contract is
    /// supposed to skip entirely.
    fn idle_frame(&mut self) -> anyhow::Result<(u64, bool)> {
        let start = Instant::now();
        let stats = self
            .renderer
            .render(
                self.gpu.device(),
                self.gpu.queue(),
                &mut self.grid,
                self.target.view(),
                self.viewport,
            )
            .map_err(|e| anyhow::anyhow!("rendering the idle frame: {e}"))?;
        let elapsed = start.elapsed();
        Ok((
            elapsed.as_nanos().min(u128::from(u64::MAX)) as u64,
            stats.gpu_work,
        ))
    }
}

/// The byte stream every arm is fed, generated once so the arms do identical
/// work.
///
/// Shaped like an agent's transcript rather than random bytes: coloured lines,
/// cursor moves, an occasional full repaint. Random bytes would spend the whole
/// frame in the parser's error recovery and report a distribution no session
/// produces.
fn stream(spec: &FrameSpec) -> Vec<Vec<u8>> {
    let mut rng = Rng::new(spec.seed);
    let mut out = Vec::with_capacity(spec.frames);
    for i in 0..spec.frames {
        let mut chunk = Vec::with_capacity(2048);
        if i % 64 == 63 {
            // A full repaint: home, then every row rewritten in a new colour,
            // so no cell can match what is already stored.
            chunk.extend_from_slice(b"\x1b[H");
            for row in 0..spec.rows {
                let shade = (i as u16 + row) % 200 + 16;
                chunk.extend_from_slice(format!("\x1b[38;5;{shade}m").as_bytes());
                for col in 0..spec.cols {
                    chunk.push(b'0' + ((col + row) % 10) as u8);
                }
                chunk.extend_from_slice(b"\r\n");
            }
        } else {
            let lines = 1 + rng.below(3);
            for _ in 0..lines {
                let fg = 16 + rng.below(216);
                chunk.extend_from_slice(format!("\x1b[38;5;{fg}m").as_bytes());
                let width = 8 + rng.below(usize::from(spec.cols).saturating_sub(9));
                for _ in 0..width {
                    // Printable ASCII, which is what a transcript is.
                    chunk.push(b'!' + (rng.below(93)) as u8);
                }
                chunk.extend_from_slice(b"\x1b[0m\r\n");
            }
        }
        out.push(chunk);
    }
    out
}

/// One arm's numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArmResult {
    pub arm: String,
    /// Every measured frame, pooled across rounds.
    pub frame_ns: Dist,
    /// Every skipped frame, pooled across rounds.
    pub idle_ns: Dist,
    /// The median frame of each round, in order. The comparator pairs these
    /// across arms, which is what makes drift cancel instead of accumulate.
    pub round_p50_ns: Vec<u64>,
    /// The median skipped frame of each round.
    pub round_idle_p50_ns: Vec<u64>,
    /// Frames that reached the GPU. A distribution full of no-ops would
    /// otherwise read as a very fast renderer.
    pub gpu_frames: usize,
}

/// What the probe attributed, when it was on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseResult {
    pub phase: String,
    /// Nanoseconds in this phase per frame.
    pub per_frame_ns: Dist,
    /// Spans opened per frame. Parse, store and submit are once each; damage
    /// and upload are once per damaged span run, so their counts say how
    /// fragmented the frame was.
    pub spans_per_frame: Dist,
    /// Share of the arm's median frame, in tenths of a percent.
    pub share_permille: u64,
}

pub fn run(spec: &FrameSpec) -> anyhow::Result<Report> {
    if spec.frames == 0 || spec.rounds == 0 {
        bail!("a frame run needs at least one frame in at least one round");
    }
    let arms = Arm::compiled();
    let mut report = Report::new(
        "frame",
        "in-process",
        json!({
            "frames": spec.frames,
            "rounds": spec.rounds,
            "cols": spec.cols,
            "rows": spec.rows,
            "seed": spec.seed,
            "software": spec.software,
            "probe_compiled": cfg!(feature = "probe"),
            "arms": arms.iter().map(|a| a.name()).collect::<Vec<_>>(),
        }),
    );
    let started = Instant::now();

    let chunks = stream(spec);
    let mut bed = Bed::new(spec)?;
    let adapter = bed.gpu.describe();

    // Per arm: pooled samples and per-round medians.
    let mut frames: Vec<Vec<u64>> = vec![Vec::new(); arms.len()];
    let mut idles: Vec<Vec<u64>> = vec![Vec::new(); arms.len()];
    let mut round_p50: Vec<Vec<u64>> = vec![Vec::new(); arms.len()];
    let mut round_idle_p50: Vec<Vec<u64>> = vec![Vec::new(); arms.len()];
    let mut gpu_frames = vec![0usize; arms.len()];
    // Per-phase, per-frame accumulation for the `on` arm only.
    let mut phase_ns: Vec<Vec<u64>> = vec![Vec::new(); Phase::ALL.len()];
    let mut phase_calls: Vec<Vec<u64>> = vec![Vec::new(); Phase::ALL.len()];
    let mut idle_spans = 0u64;
    let mut idle_gpu_work = 0u64;

    for _round in 0..spec.rounds {
        for (a, arm) in arms.iter().enumerate() {
            probe::set_enabled(*arm == Arm::On);
            probe::reset();
            // Every arm starts from the same screen, so an arm is never handed
            // a grid another arm left dirtier.
            bed.vt.reset();
            bed.grid.mark_all_damaged();
            let _ = bed.frame(b"")?;

            let mut round = Vec::with_capacity(spec.frames);
            let mut round_idle = Vec::with_capacity(spec.frames);
            for chunk in &chunks {
                let (ns, gpu) = bed.frame(chunk)?;
                if gpu {
                    gpu_frames[a] += 1;
                }
                if *arm == Arm::On {
                    let f = probe::take();
                    for (p, phase) in Phase::ALL.iter().enumerate() {
                        phase_ns[p].push(f.nanos(*phase));
                        phase_calls[p].push(u64::from(f.calls(*phase)));
                    }
                }
                round.push(ns);

                // The frame the damage contract skips, measured right after a
                // frame that did paint, which is the only state it occurs in.
                let (idle_ns, idle_gpu) = bed.idle_frame()?;
                if idle_gpu {
                    idle_gpu_work += 1;
                }
                if *arm == Arm::On && !probe::take().is_empty() {
                    idle_spans += 1;
                }
                round_idle.push(idle_ns);
            }
            probe::set_enabled(false);

            frames[a].extend_from_slice(&round);
            idles[a].extend_from_slice(&round_idle);
            round_p50[a].push(Dist::of(round)?.p50);
            round_idle_p50[a].push(Dist::of(round_idle)?.p50);
        }
    }

    let mut results = Vec::with_capacity(arms.len());
    for (a, arm) in arms.iter().enumerate() {
        results.push(ArmResult {
            arm: arm.name().to_string(),
            frame_ns: Dist::of(std::mem::take(&mut frames[a]))?,
            idle_ns: Dist::of(std::mem::take(&mut idles[a]))?,
            round_p50_ns: std::mem::take(&mut round_p50[a]),
            round_idle_p50_ns: std::mem::take(&mut round_idle_p50[a]),
            gpu_frames: gpu_frames[a],
        });
    }

    // Attribution, against the `on` arm's own median frame.
    let mut phases = Vec::new();
    if let Some(on) = results.iter().find(|r| r.arm == Arm::On.name()) {
        let frame_p50 = on.frame_ns.p50.max(1);
        for (p, phase) in Phase::ALL.iter().enumerate() {
            let per_frame = Dist::of(std::mem::take(&mut phase_ns[p]))?;
            let spans = Dist::of(std::mem::take(&mut phase_calls[p]))?;
            if spans.max == 0 {
                report.failures.push(format!(
                    "phase `{}` was never recorded: the probe has no call site for it, \
                     so the report would show it as free",
                    phase.name()
                ));
            }
            phases.push(PhaseResult {
                phase: phase.name().to_string(),
                share_permille: per_frame.p50.saturating_mul(1000) / frame_p50,
                per_frame_ns: per_frame,
                spans_per_frame: spans,
            });
        }
        let attributed: u64 = phases.iter().map(|p| p.per_frame_ns.p50).sum();
        if attributed > on.frame_ns.p50 {
            report.failures.push(format!(
                "the phases sum to {attributed} ns against a {} ns frame: two spans overlap and \
                 the same time is charged twice",
                on.frame_ns.p50
            ));
        } else {
            report.checks_passed.push(format!(
                "the five phases account for {attributed} ns of the {} ns median frame; the rest \
                 is between them",
                on.frame_ns.p50
            ));
        }
    }

    if idle_gpu_work > 0 {
        report.failures.push(format!(
            "{idle_gpu_work} frames with no damage still recorded GPU work: the skip gate is not \
             holding and every idle window is paying for a frame"
        ));
    } else {
        report
            .checks_passed
            .push("no frame without damage reached the GPU".to_string());
    }
    if idle_spans > 0 {
        report.failures.push(format!(
            "{idle_spans} skipped frames opened a probe span: the probe is making the skip path \
             do work in order to time it"
        ));
    } else if arms.contains(&Arm::On) {
        report
            .checks_passed
            .push("a skipped frame opens no span, so the probe cannot perturb the skip gate"
                .to_string());
    }

    report.duration_secs = started.elapsed().as_secs_f64();
    report.extra = json!({
        "adapter": adapter,
        "arms": results,
        "phases": phases,
    });
    Ok(report)
}

/// The per-arm table, as a run prints it.
#[must_use]
pub fn table(report: &Report) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let arms: Vec<ArmResult> = serde_json::from_value(report.extra["arms"].clone())
        .unwrap_or_default();
    let _ = writeln!(s, "arm      n      p50        p95        p99        idle p50");
    for a in &arms {
        let _ = writeln!(
            s,
            "{:<8} {:<6} {:<10} {:<10} {:<10} {}",
            a.arm,
            a.frame_ns.count,
            ns(a.frame_ns.p50),
            ns(a.frame_ns.p95),
            ns(a.frame_ns.p99),
            ns(a.idle_ns.p50),
        );
    }
    let phases: Vec<PhaseResult> = serde_json::from_value(report.extra["phases"].clone())
        .unwrap_or_default();
    if !phases.is_empty() {
        let _ = writeln!(s, "\nphase    p50        p99        spans/frame  share");
        for p in &phases {
            let _ = writeln!(
                s,
                "{:<8} {:<10} {:<10} {:<12} {:.1}%",
                p.phase,
                ns(p.per_frame_ns.p50),
                ns(p.per_frame_ns.p99),
                p.spans_per_frame.p50,
                p.share_permille as f64 / 10.0,
            );
        }
    }
    s
}

/// Nanoseconds at a scale a person reads.
fn ns(v: u64) -> String {
    if v < 1_000 {
        format!("{v}ns")
    } else if v < 1_000_000 {
        format!("{:.1}us", v as f64 / 1e3)
    } else {
        format!("{:.2}ms", v as f64 / 1e6)
    }
}
