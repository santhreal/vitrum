# vitrum-grid

A fixed-pitch terminal cell grid, drawn with `wgpu`. No window, no event loop,
no PTY, and no VT parser: the whole input is a `CellGrid` that something else
fills in.

Because it is plain `wgpu` and depends on neither Blitz nor Dioxus, the same
renderer drops into a Blitz custom widget, GPUI, Iced, or bare `winit`. The
`wgpu` version is pinned to the one Blitz uses so a host and this renderer can
share one `wgpu::Device` instead of linking two incompatible copies of `wgpu`.

```rust
use vitrum_grid::{CellGrid, GpuContext, GridRenderer, HeadlessTarget, RendererConfig, Style};

let gpu = GpuContext::headless()?;
let config = RendererConfig { format: HeadlessTarget::FORMAT, ..RendererConfig::default() };
let mut renderer = GridRenderer::new(gpu.device(), &config)?;

let (cw, ch) = renderer.cell_size();
let target = HeadlessTarget::new(gpu.device(), cw * 20, ch * 3);
let mut grid = CellGrid::new(20, 3, Style::DEFAULT)?;
grid.write_str(0, 0, "hello", Style::DEFAULT)?;

let size = (target.width(), target.height());
let drawn = renderer.render(gpu.device(), gpu.queue(), &mut grid, target.view(), size)?;
assert!(drawn.gpu_work);

// Nothing changed since, so this frame records no GPU command at all.
let idle = renderer.render(gpu.device(), gpu.queue(), &mut grid, target.view(), size)?;
assert!(!idle.gpu_work);
```

Cost model: one allocation for the grid, one for the instance buffer, one
texture for the glyph atlas, and nothing per frame. One instanced draw call per
frame whatever the grid size. Only changed cells are re-uploaded, and adjacent
damage coalesces into a single `write_buffer`. Twenty idle grids cost twenty
no-ops.

Part of [vitrum](https://github.com/santhreal/vitrum). MIT licensed.
