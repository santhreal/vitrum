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
    // Caret geometry: bar width, then rule thickness. Both in pixels.
    cursor_px: vec2<f32>,
    // Where cell (0, 0) starts inside the viewport: the slack a box of whole
    // cells cannot cover, halved, so it sits on both edges instead of one.
    origin_px: vec2<f32>,
    // The block is laid out in 16-byte units and the content above is 40
    // bytes. Declared so this struct is the size the uniform buffer is.
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var atlas: texture_2d<f32>;

// Low two bits of `flags` hold the column span: 0, 1, or 2.
const SPAN_MASK: u32 = 3u;
// Bit 2 asks for an underline rule across the whole cell.
const FLAG_UNDERLINE: u32 = 4u;
// Bits 3-5 hold the caret shape, 0 meaning no caret on this cell. The codes
// are `vitrum_grid::cell::CursorShape`.
const CURSOR_SHIFT: u32 = 3u;
const CURSOR_MASK: u32 = 7u;
const CURSOR_BLOCK: u32 = 1u;
const CURSOR_HOLLOW: u32 = 2u;
const CURSOR_BAR: u32 = 3u;
const CURSOR_UNDERLINE: u32 = 4u;

struct VertexInput {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) cell: vec2<u32>,
    @location(1) atlas_xy: vec2<u32>,
    @location(2) glyph_wh: vec2<u32>,
    @location(3) glyph_off: vec2<i32>,
    @location(4) fg: vec4<f32>,
    @location(5) bg: vec4<f32>,
    @location(6) flags: u32,
    @location(7) cursor: vec4<f32>,
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
    @location(7) @interpolate(flat) cursor: vec4<f32>,
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
    let origin = globals.origin_px + vec2<f32>(f32(input.cell.x), f32(input.cell.y)) * globals.cell_px;
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
    out.cursor = input.cursor;
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

    let shape = (input.flags >> CURSOR_SHIFT) & CURSOR_MASK;

    // A block caret is the one shape that replaces the cell rather than being
    // drawn over it: the caret colour becomes the field and the cell's own
    // background knocks the glyph out of it, which is what keeps the character
    // under the caret readable.
    if shape == CURSOR_BLOCK {
        return mix(input.cursor, input.bg, coverage);
    }

    let painted = mix(input.bg, input.fg, coverage);
    if shape == 0u {
        return painted;
    }

    // The remaining shapes are rules laid over the cell as it already is.
    // `local_px` spans the whole instance quad, so on a wide character's head
    // the rules run across both of its columns. That is what a terminal does
    // with a caret parked on a CJK glyph: the caret marks the character, not
    // half of it.
    let p = input.local_px;
    let thickness = globals.cursor_px.y;
    if shape == CURSOR_BAR {
        if p.x < globals.cursor_px.x {
            return input.cursor;
        }
    } else if shape == CURSOR_UNDERLINE {
        if p.y >= globals.cell_px.y - thickness {
            return input.cursor;
        }
    } else if shape == CURSOR_HOLLOW {
        let inside_x = p.x >= thickness && p.x < globals.cell_px.x - thickness;
        let inside_y = p.y >= thickness && p.y < globals.cell_px.y - thickness;
        if !(inside_x && inside_y) {
            return input.cursor;
        }
    }

    return painted;
}
