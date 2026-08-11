//! The GPU renderer: one instanced draw call for the whole grid, damage-driven
//! uploads, and a genuinely free no-change frame.
//!
//! # What a frame costs
//!
//! - Nothing changed: [`GridRenderer::render`] returns without creating a
//!   command encoder, without touching a queue, and without submitting. No GPU
//!   work of any kind is recorded. This is the case an idle terminal is in
//!   almost all of the time, and it is why idle CPU stays at zero.
//! - Something changed: each damaged span is written into the persistent
//!   instance buffer (adjacent spans coalesce into one write), then all
//!   `cols * rows` instances are drawn in a single `draw` call.
//!
//! Nothing is allocated per frame. The scratch instance vector is reused, the
//! instance buffer is reused until the grid changes size, and glyph bitmaps are
//! rasterised once and cached in the atlas.

use crate::atlas::{AtlasEntry, AtlasError, DEFAULT_ATLAS_DIM, GlyphAtlas, GlyphKey};
use crate::cell::{Attrs, Cell, Cursor, Rgba};
use crate::font::{CellMetrics, FontConfig, FontError, FontStack, FontStyle};
use crate::grid::CellGrid;

/// Column-span bits in [`CellInstance::flags`].
const SPAN_MASK: u32 = 0b11;
/// Underline bit in [`CellInstance::flags`].
const FLAG_UNDERLINE: u32 = 0b100;
/// How far up [`CellInstance::flags`] the caret shape code sits.
///
/// Three bits, because [`CursorShape`](crate::cell::CursorShape) has four
/// members and zero has to stay free to mean "no caret on this cell".
const CURSOR_SHIFT: u32 = 3;

/// Per-cell GPU instance. 32 bytes, no padding holes, so `bytemuck` can cast a
/// slice of these straight into an upload with no copy.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CellInstance {
    /// Grid coordinate as `[col, row]`.
    pub cell: [u16; 2],
    /// Top-left of the glyph in the atlas, in texels.
    pub atlas_xy: [u16; 2],
    /// Glyph size in texels. `[0, 0]` means there is nothing to sample.
    pub glyph_wh: [u16; 2],
    /// Glyph offset from the cell's top-left corner, in pixels.
    pub glyph_off: [i16; 2],
    /// Foreground after the reverse attribute has been applied.
    pub fg: [u8; 4],
    /// Background after the reverse attribute has been applied.
    pub bg: [u8; 4],
    /// Column span in bits 0-1, underline in bit 2, caret shape in bits 3-5.
    pub flags: u32,
    /// The caret's colour. Read only when the shape bits are non-zero, which
    /// is why this replaces what used to be explicit padding rather than
    /// growing the 32-byte stride.
    pub cursor: [u8; 4],
}

impl CellInstance {
    /// Vertex attribute layout matching the fields above.
    const ATTRIBUTES: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Uint16x2,
        1 => Uint16x2,
        2 => Uint16x2,
        3 => Sint16x2,
        4 => Unorm8x4,
        5 => Unorm8x4,
        6 => Uint32,
        7 => Unorm8x4,
    ];

    const fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: core::mem::size_of::<Self>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    /// Build the instance for one cell, resolving its glyph through the atlas.
    ///
    /// `cursor` is the caret when it sits on this exact cell and `None`
    /// otherwise. It is passed per cell rather than read from the grid so the
    /// upload loop stays a straight walk over damage spans.
    fn build(cell: Cell, col: u16, row: u16, entry: AtlasEntry, cursor: Option<Cursor>) -> Self {
        let (fg, bg) = cell.resolved_colors();
        let mut flags = u32::from(cell.slot.drawn_columns()) & SPAN_MASK;
        if cell.attrs.contains(Attrs::UNDERLINE) {
            flags |= FLAG_UNDERLINE;
        }
        if let Some(c) = cursor {
            flags |= c.shape.code() << CURSOR_SHIFT;
        }
        Self {
            cell: [col, row],
            atlas_xy: [entry.x, entry.y],
            glyph_wh: [entry.w, entry.h],
            glyph_off: [entry.left, entry.top],
            fg: fg.to_bytes(),
            bg: bg.to_bytes(),
            flags,
            cursor: cursor.map_or([0; 4], |c| c.color.to_bytes()),
        }
    }
}

/// Uniform block shared by every instance.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    viewport_px: [f32; 2],
    cell_px: [f32; 2],
    underline: [f32; 2],
    /// Caret geometry: bar width, then the thickness of the underline and
    /// hollow-block rules. Both in pixels, both derived from the cell size, so
    /// the caret scales with the font instead of being a fixed number of
    /// pixels that vanishes at 24 px and swallows the glyph at 8 px.
    cursor_px: [f32; 2],
}

/// What one call to [`GridRenderer::render`] actually did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct FrameStats {
    /// Cells written into the instance buffer.
    pub cells_uploaded: u32,
    /// `write_buffer` calls made. Adjacent damage spans coalesce, so a full
    /// rebuild is one write however many rows changed.
    pub writes: u32,
    /// Instances in the draw call. Zero when the frame was skipped.
    pub instances_drawn: u32,
    /// Glyphs rasterised and uploaded to the atlas this frame.
    pub glyphs_added: u32,
    /// True when the whole grid had to be rebuilt: first frame, resize,
    /// viewport change, or glyph atlas reset.
    pub full_rebuild: bool,
    /// True when any GPU command was recorded or submitted. False means the
    /// call touched neither the queue nor an encoder.
    pub gpu_work: bool,
}

impl FrameStats {
    /// The stats of a frame that did nothing at all.
    #[must_use]
    pub const fn skipped() -> Self {
        Self {
            cells_uploaded: 0,
            writes: 0,
            instances_drawn: 0,
            glyphs_added: 0,
            full_rebuild: false,
            gpu_work: false,
        }
    }
}

/// Why a frame could not be rendered.
#[derive(Clone, Debug)]
pub enum RenderError {
    /// The glyph atlas could not place a glyph this frame.
    Atlas(AtlasError),
    /// The font stack could not be built.
    Font(FontError),
    /// The viewport has a zero dimension, so there is nothing to draw into.
    ZeroViewport {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
    },
}

impl core::fmt::Display for RenderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Atlas(err) => write!(f, "glyph atlas: {err}"),
            Self::Font(err) => write!(f, "font stack: {err}"),
            Self::ZeroViewport { width, height } => {
                write!(f, "viewport {width}x{height} has a zero dimension")
            }
        }
    }
}

impl core::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Atlas(err) => Some(err),
            Self::Font(err) => Some(err),
            Self::ZeroViewport { .. } => None,
        }
    }
}

impl From<AtlasError> for RenderError {
    fn from(err: AtlasError) -> Self {
        Self::Atlas(err)
    }
}

impl From<FontError> for RenderError {
    fn from(err: FontError) -> Self {
        Self::Font(err)
    }
}

/// How to build a [`GridRenderer`].
#[derive(Clone, Debug)]
pub struct RendererConfig {
    /// Colour format of the render target the renderer will draw into.
    pub format: wgpu::TextureFormat,
    /// Edge length of the glyph atlas, clamped to the device's limits.
    pub atlas_dim: u32,
    /// Font selection and size.
    pub font: FontConfig,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            format: wgpu::TextureFormat::Rgba8Unorm,
            atlas_dim: DEFAULT_ATLAS_DIM,
            font: FontConfig::default(),
        }
    }
}

/// State that invalidates every uploaded instance when it changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FrameKey {
    cols: u16,
    rows: u16,
    viewport: (u32, u32),
    atlas_generation: u64,
}

/// Draws a [`CellGrid`] with one instanced call per frame.
pub struct GridRenderer {
    fonts: FontStack,
    atlas: GlyphAtlas,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    globals_buffer: wgpu::Buffer,
    instances: wgpu::Buffer,
    instance_capacity: u32,
    scratch: Vec<CellInstance>,
    format: wgpu::TextureFormat,
    last: Option<FrameKey>,
}

impl core::fmt::Debug for GridRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GridRenderer")
            .field("font", &self.fonts)
            .field("atlas", &self.atlas)
            .field("format", &self.format)
            .field("instance_capacity", &self.instance_capacity)
            .finish()
    }
}

impl GridRenderer {
    /// Build a renderer, discovering a monospace font from the system.
    ///
    /// # Errors
    ///
    /// [`RenderError::Font`] when no usable monospace face can be found or
    /// parsed.
    pub fn new(device: &wgpu::Device, config: &RendererConfig) -> Result<Self, RenderError> {
        let fonts = FontStack::system(&config.font)?;
        Ok(Self::with_fonts(device, config, fonts))
    }

    /// Build a renderer around an already-constructed font stack. Use this to
    /// share one stack between several renderers, or to pin an exact face.
    #[must_use]
    pub fn with_fonts(
        device: &wgpu::Device,
        config: &RendererConfig,
        fonts: FontStack,
    ) -> Self {
        let atlas = GlyphAtlas::new(device, config.atlas_dim);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vitrum-grid.cells"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("vitrum-grid.bind-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new(
                                core::mem::size_of::<Globals>() as u64
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            // `textureLoad` fetches exact texels, so the atlas
                            // never needs a sampler or filtering.
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vitrum-grid.globals"),
            size: core::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vitrum-grid.bind-group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: globals_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(atlas.view()),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vitrum-grid.pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vitrum-grid.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[CellInstance::layout()],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    // Every pixel of the grid is written by a cell quad, so
                    // blending would only cost bandwidth. A translucent
                    // terminal background arrives as a cell background alpha
                    // and is written through to the target.
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let instance_capacity = 0;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vitrum-grid.instances"),
            size: core::mem::size_of::<CellInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            fonts,
            atlas,
            pipeline,
            bind_group,
            globals_buffer,
            instances,
            instance_capacity,
            scratch: Vec::new(),
            format: config.format,
            last: None,
        }
    }

    /// Cell geometry derived from the font.
    #[must_use]
    pub const fn metrics(&self) -> CellMetrics {
        self.fonts.metrics()
    }

    /// Cell size in pixels as `(width, height)`.
    #[must_use]
    pub const fn cell_size(&self) -> (u32, u32) {
        let m = self.fonts.metrics();
        (m.width, m.height)
    }

    /// The largest grid that fits a `width` x `height` pixel area, at least
    /// 1 x 1.
    #[must_use]
    pub const fn grid_size_for(&self, width: u32, height: u32) -> (u16, u16) {
        let m = self.fonts.metrics();
        // A zero metric means the font reported no cell, which divides into
        // nothing; `unwrap_or` is not const, so the fallback is a match.
        let cols = match width.checked_div(m.width) {
            Some(cols) => cols,
            None => 1,
        };
        let rows = match height.checked_div(m.height) {
            Some(rows) => rows,
            None => 1,
        };
        // A viewport smaller than one cell still gets a 1x1 grid rather than a
        // zero-sized one the grid type would refuse.
        (
            if cols == 0 { 1 } else { cols as u16 },
            if rows == 0 { 1 } else { rows as u16 },
        )
    }

    /// Pixel size a `cols` x `rows` grid occupies.
    #[must_use]
    pub const fn pixel_size_for(&self, cols: u16, rows: u16) -> (u32, u32) {
        let m = self.fonts.metrics();
        (m.width * cols as u32, m.height * rows as u32)
    }

    /// The font stack, for callers that want to pre-warm the atlas.
    #[must_use]
    pub const fn fonts(&self) -> &FontStack {
        &self.fonts
    }

    /// The glyph atlas.
    #[must_use]
    pub const fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    /// Colour format this renderer's pipeline writes.
    #[must_use]
    pub const fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Forget everything known about the last frame, forcing the next
    /// [`GridRenderer::render`] to rebuild and redraw.
    ///
    /// Call this when the render target was replaced or its contents were
    /// clobbered by something outside the renderer, since the skip path assumes
    /// the previous frame is still on screen.
    pub const fn invalidate(&mut self) {
        self.last = None;
    }

    /// Draw `grid` into `target`.
    ///
    /// Returns without recording any GPU command when the grid has no damage
    /// and nothing structural changed. The caller should treat
    /// [`FrameStats::gpu_work`] as the signal for whether a swapchain frame
    /// needs presenting.
    ///
    /// # Errors
    ///
    /// [`RenderError::ZeroViewport`] for a degenerate viewport and
    /// [`RenderError::Atlas`] when the frame needs more glyphs than the atlas
    /// can hold.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        grid: &mut CellGrid,
        target: &wgpu::TextureView,
        viewport: (u32, u32),
    ) -> Result<FrameStats, RenderError> {
        let (vw, vh) = viewport;
        if vw == 0 || vh == 0 {
            return Err(RenderError::ZeroViewport {
                width: vw,
                height: vh,
            });
        }

        self.atlas.begin_frame();

        let mut key = FrameKey {
            cols: grid.cols(),
            rows: grid.rows(),
            viewport,
            atlas_generation: self.atlas.generation(),
        };
        let full_rebuild = self.last != Some(key);
        if full_rebuild {
            self.ensure_capacity(device, grid.len());
            self.write_globals(queue, viewport);
            grid.mark_all_damaged();
        }

        if !grid.is_dirty() {
            self.last = Some(key);
            return Ok(FrameStats::skipped());
        }

        // Uploading can reset the atlas, which invalidates every coordinate
        // already written this frame. One retry is enough: the reset emptied
        // the atlas, and a second reset means the frame genuinely wants more
        // glyphs than fit, which `GlyphAtlas` reports as `Exhausted`.
        let mut stats = self.upload(queue, grid)?;
        if self.atlas.generation() != key.atlas_generation {
            grid.mark_all_damaged();
            let retry = self.upload(queue, grid)?;
            // Both passes really did write to the buffer, so the reported cost
            // is the sum. Hiding the discarded first pass would make an atlas
            // reset look free.
            stats.cells_uploaded += retry.cells_uploaded;
            stats.writes += retry.writes;
            stats.glyphs_added += retry.glyphs_added;
            key.atlas_generation = self.atlas.generation();
        }
        stats.full_rebuild = full_rebuild;

        let instances = grid.len() as u32;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("vitrum-grid.frame"),
        });
        {
            let clear = grid.default_style().bg;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vitrum-grid.cells"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(to_wgpu_color(clear)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            pass.draw(0..4, 0..instances);
        }
        queue.submit([encoder.finish()]);

        grid.clear_damage();
        stats.instances_drawn = instances;
        stats.gpu_work = true;
        self.last = Some(key);
        Ok(stats)
    }

    /// Write every damaged span into the instance buffer, coalescing spans that
    /// are adjacent in the flat cell order into a single upload.
    fn upload(
        &mut self,
        queue: &wgpu::Queue,
        grid: &CellGrid,
    ) -> Result<FrameStats, RenderError> {
        // Move the scratch buffer out so the atlas and font stack can be
        // borrowed mutably alongside it. The Vec keeps its allocation.
        let mut scratch = core::mem::take(&mut self.scratch);
        let result = self.upload_into(queue, grid, &mut scratch);
        scratch.clear();
        self.scratch = scratch;
        result
    }

    fn upload_into(
        &mut self,
        queue: &wgpu::Queue,
        grid: &CellGrid,
        scratch: &mut Vec<CellInstance>,
    ) -> Result<FrameStats, RenderError> {
        let stride = core::mem::size_of::<CellInstance>() as u64;
        let cols = grid.cols() as usize;

        let mut stats = FrameStats::default();
        let resident_before = self.atlas.resident();
        let mut run_start: usize = 0;
        let mut run_end: usize = 0;
        scratch.clear();

        for span in grid.damage() {
            let flat = span.row as usize * cols + span.start as usize;
            if !scratch.is_empty() && flat != run_end {
                queue.write_buffer(
                    &self.instances,
                    run_start as u64 * stride,
                    bytemuck::cast_slice(scratch),
                );
                stats.writes += 1;
                scratch.clear();
            }
            if scratch.is_empty() {
                run_start = flat;
            }

            // Resolved once per span rather than once per cell: the caret is a
            // single cell, so at most one column in this run can carry it.
            let caret = grid.cursor().filter(|c| {
                c.row == span.row && c.col >= span.start && c.col < span.end
            });
            for col in span.columns() {
                let cell = grid.cell(col, span.row).expect("valid cell index");
                let entry = self.entry_for(queue, cell)?;
                let on_cell = caret.filter(|c| c.col == col);
                scratch.push(CellInstance::build(cell, col, span.row, entry, on_cell));
            }
            run_end = flat + span.len();
            stats.cells_uploaded += span.len() as u32;
        }

        if !scratch.is_empty() {
            queue.write_buffer(
                &self.instances,
                run_start as u64 * stride,
                bytemuck::cast_slice(scratch),
            );
            stats.writes += 1;
        }

        stats.glyphs_added = self.atlas.resident().saturating_sub(resident_before) as u32;
        Ok(stats)
    }

    fn entry_for(&mut self, queue: &wgpu::Queue, cell: Cell) -> Result<AtlasEntry, RenderError> {
        if cell.is_glyphless() {
            return Ok(AtlasEntry::BLANK);
        }
        let key = GlyphKey {
            ch: cell.ch,
            style: FontStyle::from_attrs(cell.attrs),
        };
        Ok(self.atlas.get_or_insert(queue, &mut self.fonts, key)?)
    }

    fn ensure_capacity(&mut self, device: &wgpu::Device, cells: usize) {
        let needed = cells as u32;
        if needed <= self.instance_capacity {
            return;
        }
        self.instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vitrum-grid.instances"),
            size: u64::from(needed) * core::mem::size_of::<CellInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = needed;
        // The scratch vector is always empty here, so `reserve` on the whole
        // cell count is the capacity a full rebuild needs and nothing more.
        self.scratch.reserve(cells);
    }

    fn write_globals(&self, queue: &wgpu::Queue, viewport: (u32, u32)) {
        let m = self.fonts.metrics();
        let globals = Globals {
            viewport_px: [viewport.0 as f32, viewport.1 as f32],
            cell_px: [m.width as f32, m.height as f32],
            underline: [m.underline_y as f32, m.underline_thickness as f32],
            cursor_px: [caret_bar_px(m.width), m.underline_thickness as f32],
        };
        queue.write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));
    }
}

/// Width of a bar caret, in pixels, for a cell `width` pixels wide.
///
/// A fraction of the cell rather than a constant, because a caret fixed at two
/// pixels is a hairline at 24 px type and a third of the cell at 6 px. One
/// pixel is the floor: a zero-width bar is a caret nobody can see.
fn caret_bar_px(width: u32) -> f32 {
    (width as f32 / 8.0).max(1.0)
}

fn to_wgpu_color(color: Rgba) -> wgpu::Color {
    wgpu::Color {
        r: f64::from(color.r) / 255.0,
        g: f64::from(color.g) / 255.0,
        b: f64::from(color.b) / 255.0,
        a: f64::from(color.a) / 255.0,
    }
}
