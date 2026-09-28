//! Packed vertex attributes shared by symbol tessellation and the GPU pipeline.
use bytemuck_derive::{Pod, Zeroable};

/// Tile anchor, glyph atlas coordinates and pixel offsets of a symbol quad vertex.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderSymbolVertex {
    /// Tile anchor followed by offsets in units of 1/32 pixel.
    pub a_pos_offset: [i32; 4],
    /// Atlas coordinates, symbol kind, and component radius encoded as f32 bits.
    pub a_data: [u32; 4],
    /// Offsets in 1/16 pixels, then height in metres and rotation in radians as f32 bits.
    pub a_pixeloffset: [i32; 4],
}
