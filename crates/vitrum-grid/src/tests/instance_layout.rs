//! The byte layout of the per-cell GPU instance.
//!
//! The vertex attribute table in `renderer.rs` and the `VertexInput` struct in
//! `shader.wgsl` both hard-code these offsets. Rust will not notice if a field
//! is reordered or resized, so the mismatch surfaces as scrambled colours and
//! glyphs sampled from the wrong atlas rectangle. These assertions turn that
//! into a named test failure.

use core::mem::{align_of, offset_of, size_of};

use crate::renderer::CellInstance;

/// The instance must be exactly 32 bytes with the documented field offsets.
///
/// Each offset below appears verbatim in the pipeline's
/// `wgpu::vertex_attr_array!`. Moving a field without updating that table
/// reinterprets, for example, the foreground bytes as an atlas coordinate: text
/// samples random glyphs and nothing in the type system complains.
#[test]
fn cell_instance_field_offsets_match_the_vertex_attribute_table() {
    assert_eq!(size_of::<CellInstance>(), 32, "the stride must stay 32 bytes");
    assert_eq!(align_of::<CellInstance>(), 4);

    assert_eq!(offset_of!(CellInstance, cell), 0, "location 0, Uint16x2");
    assert_eq!(offset_of!(CellInstance, atlas_xy), 4, "location 1, Uint16x2");
    assert_eq!(offset_of!(CellInstance, glyph_wh), 8, "location 2, Uint16x2");
    assert_eq!(offset_of!(CellInstance, glyph_off), 12, "location 3, Sint16x2");
    assert_eq!(offset_of!(CellInstance, fg), 16, "location 4, Unorm8x4");
    assert_eq!(offset_of!(CellInstance, bg), 20, "location 5, Unorm8x4");
    assert_eq!(offset_of!(CellInstance, flags), 24, "location 6, Uint32");
    assert_eq!(offset_of!(CellInstance, _pad), 28);
}

/// The instance must serialise to an exact, byte-for-byte known image.
///
/// `bytemuck::cast_slice` uploads this memory verbatim, so these bytes are
/// literally what the GPU reads. Pinning them catches endianness assumptions,
/// implicit padding, and a field that silently changed width, none of which a
/// size check alone would find.
#[test]
fn cell_instance_serializes_to_exact_bytes() {
    let instance = CellInstance {
        cell: [0x0102, 0x0304],
        atlas_xy: [0x0506, 0x0708],
        glyph_wh: [0x090a, 0x0b0c],
        glyph_off: [-2, 3],
        fg: [0x11, 0x22, 0x33, 0x44],
        bg: [0x55, 0x66, 0x77, 0x88],
        flags: 0x0000_0006,
        _pad: 0,
    };
    let bytes = bytemuck::bytes_of(&instance);
    assert_eq!(bytes.len(), 32);
    assert_eq!(
        bytes,
        &[
            // cell: two little-endian u16
            0x02, 0x01, 0x04, 0x03, //
            // atlas_xy
            0x06, 0x05, 0x08, 0x07, //
            // glyph_wh
            0x0a, 0x09, 0x0c, 0x0b, //
            // glyph_off: two little-endian i16, -2 and 3
            0xfe, 0xff, 0x03, 0x00, //
            // fg, then bg, in r,g,b,a order
            0x11, 0x22, 0x33, 0x44, //
            0x55, 0x66, 0x77, 0x88, //
            // flags: span 2 plus the underline bit
            0x06, 0x00, 0x00, 0x00, //
            // padding
            0x00, 0x00, 0x00, 0x00,
        ][..]
    );
}

/// A zeroed instance must be a legal, invisible cell.
///
/// The instance buffer is created without initialisation, and wgpu zero-fills
/// it. Any cell the renderer has not uploaded yet is therefore all zeros, and
/// that must draw nothing rather than a black quad at the origin: the span bits
/// are zero, which the vertex shader turns into a degenerate quad.
#[test]
fn a_zeroed_instance_draws_nothing() {
    let zero = CellInstance::default();
    assert_eq!(bytemuck::bytes_of(&zero), &[0u8; 32][..]);
    assert_eq!(
        zero.flags & 0b11,
        0,
        "a zeroed instance must have a zero column span"
    );
    assert_eq!(
        zero.glyph_wh,
        [0, 0],
        "a zeroed instance must sample no glyph"
    );
}
