use crate::legacy::buckets::symbol_bucket::SymbolVertex;
use bytemuck_derive::{Pod, Zeroable};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderSymbolVertex {
    // 4 bytes * 3 = 12 bytes
    pub position: [f32; 3],
    // 4 bytes * 3 = 12 bytes
    pub text_anchor: [f32; 3],
    // 4 bytes * 2 = 8 bytes
    pub tex_coords: [f32; 2],
    // 1 byte * 4 = 4 bytes
    pub color: [u8; 4],
    // 1 byte
    pub is_glyph: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderSymbolVertexNew {
    pub a_pos_offset: [i32; 4],
    pub a_data: [u32; 4],
    pub a_pixeloffset: [i32; 4],
}

const MAX_GLYPH_ICON_SIZE: u32 = 255;
const SIZE_PACK_FACTOR: u32 = 128;
const MAX_PACKED_SIZE: u32 = MAX_GLYPH_ICON_SIZE * SIZE_PACK_FACTOR;

impl ShaderSymbolVertexNew {
    pub fn new(vertex: &SymbolVertex) -> Self {
        let a_size_min =
            (MAX_PACKED_SIZE.min((vertex.size_data.start * SIZE_PACK_FACTOR as f64) as u32) << 1)
                + vertex.is_sdf as u32;
        let a_size_max =
            MAX_PACKED_SIZE.min((vertex.size_data.end * SIZE_PACK_FACTOR as f64) as u32);

        ShaderSymbolVertexNew {
            a_pos_offset: [
                vertex.label_anchor.x as i32,
                vertex.label_anchor.y as i32,
                (vertex.o.x * 32.).round() as i32,
                ((vertex.o.y + vertex.glyph_offset_y) * 32.) as i32,
            ],
            a_data: [vertex.tx as u32, vertex.ty as u32, a_size_min, a_size_max],
            a_pixeloffset: [
                (vertex.pixel_offset.x * 16.) as i32,
                (vertex.pixel_offset.y * 16.) as i32,
                (vertex.min_font_scale.x * 256.) as i32,
                (vertex.min_font_scale.y * 256.) as i32,
            ],
        }
    }
}
