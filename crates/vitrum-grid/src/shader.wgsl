// One instanced quad per grid cell.
//
// Each instance carries its grid coordinate, its glyph's rectangle in the
// coverage atlas, the glyph's offset inside the cell, and the two colours. The
// vertex stage turns that into a screen-space quad; the fragment stage samples
// the atlas with textureLoad (exact texel fetch, no sampler, no filtering) and
// blends foreground over background by coverage.
//
// A wide character's head instance spans two columns and its tail instance
// spans zero, collapsing to a degenerate quad that produces no fragments. That
// is what stops the tail from painting its background over the right half of
// the glyph the head just drew.

struct Globals {
    // Render target size in pixels.
    viewport_px: vec2<f32>,
    // One cell's size in pixels.
    cell_px: vec2<f32>,
    // Underline rule: top offset from the cell's top edge, then thickness.
    underline: vec2<f32>,
    // Padding so the block is 32 bytes.
    reserved: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;

// Low two bits of `flags` hold the column span: 0, 1, or 2.
const SPAN_MASK: u32 = 3u;
// Bit 2 asks for an underline rule across the whole cell.
const FLAG_UNDERLINE: u32 = 4u;

struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) cell: vec2<u32>,
    @location(1) atlas_xy: vec2<u32>,
    @location(2) glyph_wh: vec2<u32>,
    @location(3) glyph_off: vec2<i32>,
    @location(4) fg: vec4<f32>,
    @location(5) bg: vec4<f32>,
    @location(6) flags: u32,
};

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) fg: vec4<f32>,
    @location(1) bg: vec4<f32>,
    @location(2) local_px: vec2<f32>,
    @location(3) @interpolate(flat) atlas_xy: vec2<u32>,
    @location(4) @interpolate(flat) glyph_wh: vec2<u32>,
    @location(5) @interpolate(flat) glyph_off: vec2<i32>,
    @location(6) @interpolate(flat) flags: u32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let span = f32(input.flags & SPAN_MASK);

    // Triangle strip corners: 0 -> (0,0), 1 -> (1,0), 2 -> (0,1), 3 -> (1,1).
    let corner = vec2<f32>(
        f32(input.vertex_index & 1u),
        f32((input.vertex_index >> 1u) & 1u),
    );

    let size = vec2<f32>(globals.cell_px.x * span, globals.cell_px.y * min(span, 1.0));
    let origin = vec2<f32>(f32(input.cell.x), f32(input.cell.y)) * globals.cell_px;
    let offset = corner * size;
    let px = origin + offset;

    var out: VertexOutput;
    out.clip = vec4<f32>(
        px.x / globals.viewport_px.x * 2.0 - 1.0,
        1.0 - px.y / globals.viewport_px.y * 2.0,
        0.0,
        1.0,
    );
    out.fg = input.fg;
    out.bg = input.bg;
    out.local_px = offset;
    out.atlas_xy = input.atlas_xy;
    out.glyph_wh = input.glyph_wh;
    out.glyph_off = input.glyph_off;
    out.flags = input.flags;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Interpolation samples at pixel centres, so flooring gives the integer
    // pixel index inside the cell.
    let local = vec2<i32>(floor(input.local_px));

    var coverage = 0.0;
    if input.glyph_wh.x > 0u && input.glyph_wh.y > 0u {
        let g = local - input.glyph_off;
        if g.x >= 0 && g.y >= 0 && u32(g.x) < input.glyph_wh.x && u32(g.y) < input.glyph_wh.y {
            coverage = textureLoad(atlas, vec2<i32>(input.atlas_xy) + g, 0).r;
        }
    }

    if (input.flags & FLAG_UNDERLINE) != 0u {
        let y = f32(local.y);
        if y >= globals.underline.x && y < globals.underline.x + globals.underline.y {
            coverage = 1.0;
        }
    }

    return mix(input.bg, input.fg, coverage);
}
