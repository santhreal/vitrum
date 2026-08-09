//! The same native path with the window taken away.
//!
//! The windowed prototype has to run under Xvfb, and Xvfb has no NVIDIA WSI, so
//! it presents through a software rasteriser. That is honest about what a
//! headless CI box can do and dishonest about what the pane costs on a real
//! machine. This mode renders the identical `Vt` -> `CellGrid` ->
//! `GridRenderer` path into an offscreen texture on whichever adapter the box
//! actually has, so the engine's ceiling and the X server's ceiling are two
//! separate numbers instead of one confused one.

use std::time::Instant;

use anyhow::{Result, anyhow};
use vitrum_grid::{CellGrid, GpuContext, GridRenderer, HeadlessTarget, RendererConfig, Style};
use vitrum_vt::{Vt, VtOptions};

use crate::pty::{self, Pty};
use crate::stats::Run;

/// Run the offscreen bench.
pub fn run(args: &[String]) -> Result<()> {
    let mut cols = 100u16;
    let mut rows = 30u16;
    let mut seconds = 15u64;
    let mut stats: Option<String> = None;
    let mut argv: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cols" => {
                cols = args[i + 1].parse()?;
                i += 2;
            }
            "--rows" => {
                rows = args[i + 1].parse()?;
                i += 2;
            }
            "--seconds" => {
                seconds = args[i + 1].parse()?;
                i += 2;
            }
            "--stats" => {
                stats = Some(args[i + 1].clone());
                i += 2;
            }
            "--" => {
                argv = args[i + 1..].to_vec();
                i = args.len();
            }
            other => return Err(anyhow!("unknown flag {other}")),
        }
    }
    if argv.is_empty() {
        argv = vec!["/usr/bin/python3".into(), "-q".into()];
    }

    let gpu = GpuContext::headless().map_err(|e| anyhow!("gpu: {e}"))?;
    let config = RendererConfig {
        format: HeadlessTarget::FORMAT,
        ..RendererConfig::default()
    };
    let mut renderer =
        GridRenderer::new(gpu.device(), &config).map_err(|e| anyhow!("renderer: {e}"))?;
    let cell = renderer.cell_size();
    let (pw, ph) = renderer.pixel_size_for(cols, rows);
    let target = HeadlessTarget::new(gpu.device(), pw, ph);

    let mut grid = CellGrid::new(cols, rows, Style::DEFAULT).map_err(|e| anyhow!("grid: {e}"))?;
    let mut vt = Vt::new(VtOptions {
        cols,
        rows,
        max_scrollback: 10_000,
    })
    .map_err(|e| anyhow!("vt: {e}"))?;

    println!("adapter: {}", gpu.describe());

    let mut pty = Pty::spawn(&argv, cols, rows, cell)?;
    let mut run = Run::new("native-offscreen");
    let mut buf = Vec::with_capacity(1 << 20);
    let mut back = Vec::new();
    let deadline = Instant::now() + std::time::Duration::from_secs(seconds);

    while Instant::now() < deadline {
        buf.clear();
        let open = pty::drain(pty.fd, &mut buf)?;
        if buf.is_empty() {
            if !open {
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
            continue;
        }
        let n = buf.len();
        let t0 = Instant::now();
        vt.feed(&buf);
        back.clear();
        vt.drain_pty_write(&mut back);
        let sync = vt.sync(&mut grid).map_err(|e| anyhow!("sync: {e}"))?;
        if !sync.is_noop() {
            renderer
                .render(gpu.device(), gpu.queue(), &mut grid, target.view(), (pw, ph))
                .map_err(|e| anyhow!("render: {e}"))?;
            // A submit that has not completed is not a frame. Waiting here is
            // what makes the number comparable with the windowed path, which
            // waits before it presents.
            gpu.device()
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .ok();
        }
        run.frame(n, t0.elapsed().as_micros() as u64);
        if !back.is_empty() {
            pty.write(&back)?;
        }
        if !open {
            break;
        }
    }
    pty.kill();

    let mut report = run.report();
    report["adapter"] = serde_json::json!(gpu.describe());
    report["cols"] = serde_json::json!(cols);
    report["rows"] = serde_json::json!(rows);
    let text = serde_json::to_string_pretty(&report)?;
    if let Some(path) = &stats {
        std::fs::write(path, &text)?;
    }
    println!("{text}");
    Ok(())
}
