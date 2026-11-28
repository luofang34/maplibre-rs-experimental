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
        Self {
            a_pos_offset: [
                clamp_sym_i16(anchor[0]),
                clamp_sym_i16(anchor[1]),
                clamp_sym_i16(offset_q5[0]),
                clamp_sym_i16(offset_q5[1]),
            ],
            a_data: [
                clamp_sym_u16(uv[0]),
                clamp_sym_u16(uv[1]),
                (kind & 3) as u16,
                encode_sym_angle_u16(angle),
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
        self.a_data[2] = (self.a_data[2] & 3) | (quantize_sym_radius_q4(radius) << 2);
    }

    /// Stores the straight-layout glyph centre offset along a line in `1/32` layout pixels.
    #[inline]
    pub fn set_line_centre_q5(&mut self, centre_q5: i32) {
        self.a_pixel_and_height[0] =
            pack_line_centre_in_word(self.a_pixel_and_height[0], centre_q5);
    }

    /// Tile anchor `(x, y)` in tile units as `f32`.
    #[inline]
    pub fn anchor_xy(&self) -> [f32; 2] {
        [
            f32::from(self.a_pos_offset[0]),
            f32::from(self.a_pos_offset[1]),
        ]
    }

    /// Quad corner offset `(x, y)` in layout pixels (`1/32`-pixel fixed point decoded).
    #[inline]
    pub fn offset_px(&self) -> [f64; 2] {
        [
            f64::from(self.a_pos_offset[2]) / 32.0,
            f64::from(self.a_pos_offset[3]) / 32.0,
        ]
    }

    /// Height offset in metres above the ground anchor.
    #[inline]
    pub fn height(&self) -> f32 {
        f32::from_bits(self.a_pixel_and_height[1] as u32)
    }

    /// Anchor map tangent angle in radians (`[0, 2π)`).
    #[inline]
    pub fn angle(&self) -> f32 {
        decode_sym_angle_rad(self.a_data[3])
    }
}

#[inline]
fn clamp_sym_i16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

#[inline]
fn clamp_sym_u16(v: u32) -> u16 {
    v.min(u32::from(u16::MAX)) as u16
}

#[inline]
fn encode_sym_angle_u16(angle: f32) -> u16 {
    if angle.is_finite() {
        ((angle.rem_euclid(TAU) * (65536.0 / TAU)).round() as u32 & 0xffff) as u16
    } else {
        0
    }
}

#[inline]
fn quantize_sym_radius_q4(radius: f64) -> u16 {
    (radius * 16.0).round().clamp(0.0, 16383.0) as u16
}

#[inline]
fn pack_line_centre_in_word(current_word: i32, centre_q5: i32) -> i32 {
    let cx = clamp_sym_i16(centre_q5) as u16 as u32;
    let cy = (current_word as u32) & 0xffff_0000;
    (cx | cy) as i32
}

#[inline]
fn decode_sym_angle_rad(angle_u16: u16) -> f32 {
    f32::from(angle_u16) * (TAU / 65536.0)
}
