//! Packed 24-byte vertex attributes shared by symbol tessellation and the GPU pipeline.

use std::f32::consts::TAU;

use bytemuck_derive::{Pod, Zeroable};

/// Tile anchor, glyph atlas coordinates and pixel offsets of a symbol quad vertex (`24` bytes).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct ShaderSymbolVertex {
    /// Tile anchor `(x, y)` in tile units followed by quad corner offsets in `1/32` layout pixel.
    pub a_pos_offset: [i16; 4],
    /// `[u, v, (kind & 3) | (radius_q4 << 2), angle_u16]`.
    pub a_data: [u16; 4],
    /// `[px_off_x_i16 | (px_off_y_i16 << 16), height_f32_bits]`.
    pub a_pixel_and_height: [i32; 2],
}

impl ShaderSymbolVertex {
    /// Packs a symbol quad vertex into its 24-byte GPU layout.
    #[inline]
    pub fn new(
        anchor: [i32; 2],
        offset_q5: [i32; 2],
        uv: [u32; 2],
        kind: u32,
        height: f32,
        angle: f32,
    ) -> Self {
        let clamp_i16 = |v: i32| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        let clamp_u16 = |v: u32| v.min(u32::from(u16::MAX)) as u16;
        let angle_u16 = if angle.is_finite() {
            ((angle.rem_euclid(TAU) * (65536.0 / TAU)).round() as u32 & 0xffff) as u16
        } else {
            0
        };
        Self {
            a_pos_offset: [
                clamp_i16(anchor[0]),
                clamp_i16(anchor[1]),
                clamp_i16(offset_q5[0]),
                clamp_i16(offset_q5[1]),
            ],
            a_data: [
                clamp_u16(uv[0]),
                clamp_u16(uv[1]),
                (kind & 3) as u16,
                angle_u16,
            ],
            a_pixel_and_height: [0, height.to_bits() as i32],
        }
    }

    /// Decodes the 2-bit symbol component kind (`0` text, `1` icon, `2` SDF icon, `3` inline image).
    #[inline]
    pub const fn kind(&self) -> u32 {
        (self.a_data[2] & 3) as u32
    }

    /// Stores the measured layout-space bounding radius (in `1/16` px up to `1023.9375` px).
    #[inline]
    pub fn set_radius(&mut self, radius: f64) {
        let r_q4 = (radius * 16.0).round().clamp(0.0, 16383.0) as u16;
        self.a_data[2] = (self.a_data[2] & 3) | (r_q4 << 2);
    }

    /// Stores the straight-layout glyph centre offset along a line in `1/32` layout pixels.
    #[inline]
    pub fn set_line_centre_q5(&mut self, centre_q5: i32) {
        let cx = centre_q5.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16 as u16 as u32;
        let cy = (self.a_pixel_and_height[0] as u32) & 0xffff_0000;
        self.a_pixel_and_height[0] = (cx | cy) as i32;
    }

    /// Height offset in metres above the ground anchor.
    #[inline]
    pub fn height(&self) -> f32 {
        f32::from_bits(self.a_pixel_and_height[1] as u32)
    }

    /// Anchor map tangent angle in radians (`[0, 2π)`).
    #[inline]
    pub fn angle(&self) -> f32 {
        f32::from(self.a_data[3]) * (TAU / 65536.0)
    }
}
