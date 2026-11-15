//! Packed CPU records consumed by the vertex layouts of tile rendering pipelines.

use bytemuck_derive::{Pod, Zeroable};

use super::{attribute, Mat4x4f32, Vec2f32, Vec4f32};
use crate::render::resource::VertexBufferLayout;

/// Tile-space geometry shared by fills, lines and circles.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderVertex {
    /// Tile-local position in the renderer's 4096-unit grid.
    pub position: Vec2f32,
    /// Extrusion direction for polygon/line vertices; circle vertices store radius and stroke width in pixels.
    pub normal: Vec2f32,
    /// Distance along a stroked path, in tile units.
    pub distance: f32,
    /// Height above the body datum for spatial lines, and for circles and heatmap points the
    /// ground under them; a float below -1e20 keeps cartographic draping and then carries a
    /// stroke's side, gap width and blur.
    pub elevation: f32,
    /// Distance along the ring of an extruded wall, in tile units; a stroke packs its
    /// per-feature width and offset here instead. Zero elsewhere.
    pub edge_distance: f32,
    /// For a vertex of a subdivided circle or heatmap kernel, one plus the packed position of
    /// its corner on an eight-by-eight grid; zero where the shader decodes the corner from the
    /// vertex index. A circle's fractional part carries a per-feature stroke opacity.
    pub corner_code: f32,
}

impl ShaderVertex {
    /// Creates a vertex with zero path distance and the sentinel for cartographic draping.
    pub fn new(position: Vec2f32, normal: Vec2f32) -> Self {
        Self {
            position,
            normal,
            distance: 0.0,
            elevation: -1e30,
            edge_distance: 0.0,
            corner_code: 0.0,
        }
    }
}

impl Default for ShaderVertex {
    fn default() -> Self {
        ShaderVertex::new([0.0, 0.0], [0.0, 0.0])
    }
}

/// Per-vertex feature color (`Unorm8x4`, 4 bytes) for vector geometry.
#[repr(C)]
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct FillShaderFeatureMetadata {
    /// Quantized RGBA8 color (or 24-bit fixed-point `line-gradient` length + opacity in `a`).
    pub color: [u8; 4],
}

impl FillShaderFeatureMetadata {
    /// Quantizes floating-point `[r, g, b, a]` in `0.0..=1.0` into `Unorm8x4`.
    #[inline]
    pub fn from_rgba(color: [f32; 4]) -> Self {
        Self {
            color: color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
        }
    }

    /// Packs a `line-gradient` feature length in `1/16` tile units (`24-bit` unsigned fixed-point
    /// in `rgb`, up to `1,048,575.9375` tile units) and straight opacity in `a`.
    #[inline]
    pub fn from_line_gradient(length: f32, opacity: f32) -> Self {
        let len_q4 = (length.max(0.0) * 16.0).round().min(16_777_215.0) as u32;
        Self {
            color: [
                (len_q4 & 0xFF) as u8,
                ((len_q4 >> 8) & 0xFF) as u8,
                ((len_q4 >> 16) & 0xFF) as u8,
                (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            ],
        }
    }

    /// Unpacks a `pack_circle_colors` negative-lane `[f32; 4]` pair into `[fill, stroke]`.
    #[inline]
    pub fn unpack_circle_pair(packed: [f32; 4]) -> [Self; 2] {
        let pair = |lane: f32| -> [u8; 2] {
            let v = (-lane - 1.0).max(0.0).round() as u32;
            [((v >> 8) & 0xFF) as u8, (v & 0xFF) as u8]
        };
        let [fr, fg] = pair(packed[0]);
        let [fb, fa] = pair(packed[1]);
        let [sr, sg] = pair(packed[2]);
        let [sb, sa] = pair(packed[3]);
        [
            Self {
                color: [fr, fg, fb, fa],
            },
            Self {
                color: [sr, sg, sb, sa],
            },
        ]
    }
}

/// What the fourth lane of [`SDFShaderFeatureMetadata::pose`] holds for a glyph with a pose,
/// before the glyph's rise in metres is added; a vertex without a pose holds 0. At this
/// magnitude an `f32` keeps any rise on Earth to within a centimetre.
pub const POSED_GLYPH: f32 = 65536.0;

/// Per-vertex collision visibility, GPU fade ramp, and sampled terrain height for symbols (`32 B`).
///
/// Field offsets (`16 B`, `8 B`, `4 B`, `4 B`) preserve natural hardware alignment across
/// WebGPU, Metal, and mobile Vulkan drivers while using only 4 vertex attribute locations.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Pod, Zeroable, Default)]
pub struct SDFShaderFeatureMetadata {
    /// Bit-packed symbol metrics, elevation, fade timestamp, and along-line angle/rise (`@location(15)`):
    /// - `packed[0]`: `(elev_q4_i20 & 0x000f_ffff) | (halo_w_q4 << 20) | (has_pose << 28) | (start_hi3 << 29)`
    /// - `packed[1]`: `size_f16 | (rise_f16 << 16)`
    /// - `packed[2]`: `angle_u16 | (start_lo16 << 16)`
    /// - `packed[3]`: `target_u8 | (start_u8 << 8) | (style_u8 << 16) | (halo_b_q4 << 24)`
    pub packed: [u32; 4],
    /// Full-precision `[shift_x, shift_y]` in layout px when `!has_pose`, or `[dx, dy]` in tile
    /// units relative to the label's integer anchor when `has_pose` (`@location(12)`).
    pub pose_xy: [f32; 2],
    /// Fill colour of the symbol's text or icon (`Unorm8x4`, `@location(10)`).
    pub color: [u8; 4],
    /// Halo colour of the text or icon (`Unorm8x4`, `@location(13)`).
    pub halo: [u8; 4],
}

impl SDFShaderFeatureMetadata {
    /// Packs placement visibility, fade ramp, anchor shift, and paint style into 32 bytes.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn from_placement(
        target_opacity: f32,
        elevation: f32,
        start_opacity: f32,
        start_time_ms: f32,
        shift: [f32; 2],
        color: [f32; 4],
        halo: [f32; 4],
        params: [f32; 4],
    ) -> Self {
        let u8_norm = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        let target_u8 = u8_norm(target_opacity);
        let start_u8 = u8_norm(start_opacity);
        let style_op_u8 = u8_norm(params[3]);
        let size_f16 = u32::from(super::f32_to_f16_bits(params[0].max(0.0)));
        let halo_w_q4 = (params[1].max(0.0) * 16.0).round().min(255.0) as u32;
        let halo_b_q4 = (params[2].max(0.0) * 16.0).round().min(255.0) as u32;
        let elev_i20 = (elevation * 16.0).round().clamp(-524_288.0, 524_287.0) as i32;
        let elev_bits = (elev_i20 as u32) & 0x000f_ffff;
        let start_ticks = (start_time_ms.max(0.0) * 0.25).round().min(524_287.0) as u32;
        Self {
            packed: [
                elev_bits | (halo_w_q4 << 20) | (((start_ticks >> 16) & 0x7) << 29),
                size_f16,
                (start_ticks & 0xffff) << 16,
                target_u8 | (start_u8 << 8) | (style_op_u8 << 16) | (halo_b_q4 << 24),
            ],
            pose_xy: shift,
            color: color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
            halo: halo.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
        }
    }

    /// Encodes a glyph's curved-line pose (`dx, dy` in tile units, `angle` in radians, `rise` in metres).
    #[inline]
    pub fn set_glyph_pose(&mut self, dx: f32, dy: f32, angle: f32, rise: f32) {
        let tau = std::f32::consts::TAU;
        let angle_u16 = if angle.is_finite() {
            (angle.rem_euclid(tau) * (65536.0 / tau)).round() as u32 & 0xffff
        } else {
            0
        };
        let rise_f16 = u32::from(super::f32_to_f16_bits(rise));
        self.packed[0] |= 1_u32 << 28;
        self.packed[1] = (self.packed[1] & 0xffff) | (rise_f16 << 16);
        self.packed[2] = (self.packed[2] & 0xffff_0000) | angle_u16;
        self.pose_xy = [dx, dy];
    }

    /// Whether target collision opacity is greater than zero.
    #[inline]
    pub const fn has_target_opacity(&self) -> bool {
        (self.packed[3] & 0xff) != 0
    }

    /// Whether starting fade opacity is greater than zero.
    #[inline]
    pub const fn has_start_opacity(&self) -> bool {
        ((self.packed[3] >> 8) & 0xff) != 0
    }

    /// Decodes the quantized fade start timestamp in milliseconds.
    #[inline]
    pub fn start_time_ms(&self) -> f32 {
        let ticks = (self.packed[2] >> 16) | (((self.packed[0] >> 29) & 0x7) << 16);
        (ticks as f32) * 4.0
    }

    /// Sets quantized target collision opacity (`0.0..=1.0`), used by unit tests.
    #[inline]
    pub fn set_target_opacity(&mut self, opacity: f32) {
        let u = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
        self.packed[3] = (self.packed[3] & !0xff) | u;
    }

    /// Whether this vertex carries an along-line glyph pose (`true`) rather than an anchor shift.
    #[inline]
    pub const fn has_pose(&self) -> bool {
        ((self.packed[0] >> 28) & 1) != 0
    }

    /// Decodes `[dx, dy, angle, POSED_GLYPH + rise]` (or `[shift_x, shift_y, 0.0, 0.0]` when unposed).
    #[inline]
    pub fn decoded_pose(&self) -> [f32; 4] {
        if self.has_pose() {
            let rise = super::f16_bits_to_f32((self.packed[1] >> 16) as u16);
            let angle = (self.packed[2] & 0xffff) as f32 * (std::f32::consts::TAU / 65536.0);
            [self.pose_xy[0], self.pose_xy[1], angle, POSED_GLYPH + rise]
        } else {
            [self.pose_xy[0], self.pose_xy[1], 0.0, 0.0]
        }
    }
}

/// Per-layer instance record shared across tile pipelines.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderLayerMetadata {
    /// Style painter-order index carried by the shared instance layout.
    pub z_index: f32,
    /// Evaluated line width in style pixels, before per-tile drape scaling.
    pub line_width: f32,
    /// Layer translation converted to tile units in map axes.
    pub translate: Vec2f32,
    /// Circle stroke colour or extrusion light colour.
    pub stroke_color: Vec4f32,
    /// Circle opacity, stroke opacity and blur ratio; the last slot is padding.
    pub circle_params: Vec4f32,
    /// Circle pitch-scale (x) and pitch-alignment (y), 1.0 for `map`; the rest is padding.
    pub circle_flags: Vec4f32,
    /// Uniform fill/line/extrusion/circle colour (`@location(8)` on `uniform` pipelines).
    pub uniform_color: Vec4f32,
}

impl ShaderLayerMetadata {
    /// Metadata for a layer without circle paint or uniform fill colour override.
    pub fn new(z_index: f32, line_width: f32, translate: Vec2f32) -> Self {
        Self {
            z_index,
            line_width,
            translate,
            stroke_color: [-1.0, 0.0, 0.0, 1.0],
            circle_params: [1.0, 1.0, 0.0, 0.0],
            circle_flags: [0.0; 4],
            uniform_color: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// Per-tile instance record with projection, viewport and line scaling inputs.
///
/// Every `Vec4f32` field sits on a 16-byte boundary, `Float32x2` scalar pairs sit on 8-byte
/// boundaries, and explicit tail padding rounds the record to 128 bytes so per-tile buffer slices
/// preserve 16-byte vector attribute alignment on mobile Vulkan drivers.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct ShaderTileMetadata {
    /// Column-major tile-to-clip transform used by the flat projection path.
    pub transform: Mat4x4f32,
    /// Mercator world origin in XY and per-tile-unit scales in ZW.
    pub tile_mercator_coords: Vec4f32,
    /// The part of the tile the shape draws, as minimum and maximum tile coordinates: a tile
    /// drawn over a finer view tile covers only that tile's square of it. A line on the
    /// terrain clips itself to it, where a flat shape takes it from the stencil mask.
    pub clip_region: Vec4f32,
    /// Render viewport width in pixels.
    pub viewport_width: f32,
    /// Render viewport height in pixels.
    pub viewport_height: f32,
    /// Converts style pixels to pixels in an offscreen drape texture.
    pub line_width_scale: f32,
    /// Tile units per texture or viewport pixel for line dash lengths.
    pub line_units_per_pixel: f32,
    /// Scale from view zoom to tile zoom, normally `2^(tile_zoom - view_zoom)`.
    pub zoom_factor: f32,
    /// One clips geometry outside the root tile at the antimeridian; zero disables clipping.
    pub clip_antimeridian: u32,
    /// Explicit tail padding keeping `size_of::<ShaderTileMetadata>() == 128`.
    #[doc(hidden)]
    pub _pad: [u32; 2],
}

/// The [`ShaderTileMetadata::clip_region`] of a shape that draws its whole tile.
pub const WHOLE_TILE: Vec4f32 = [0.0, 0.0, 4096.0, 4096.0];

impl ShaderTileMetadata {
    /// Creates root-tile metadata for a 512-pixel viewport without antimeridian clipping.
    /// Callers drawing another tile or viewport must replace those fields before upload.
    pub fn new(transform: Mat4x4f32, zoom_factor: f32) -> Self {
        Self {
            transform,
            tile_mercator_coords: [0.0, 0.0, 1.0 / 4096.0, 1.0 / 4096.0],
            clip_region: WHOLE_TILE,
            viewport_width: 512.0,
            viewport_height: 512.0,
            line_width_scale: 1.0,
            line_units_per_pixel: 8.0 * zoom_factor,
            zoom_factor,
            clip_antimeridian: 0,
            _pad: [0; 2],
        }
    }

    /// Builds an instance-stepped [`VertexBufferLayout`] over `ShaderTileMetadata` with `attributes`.
    pub fn instance_layout(attributes: Vec<wgpu::VertexAttribute>) -> VertexBufferLayout {
        VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes,
        }
    }

    /// Column attributes (`@location(4..=7)`) for [`Self::transform`].
    pub fn transform_attributes() -> [wgpu::VertexAttribute; 4] {
        let base = std::mem::offset_of!(Self, transform) as u64;
        let col = wgpu::VertexFormat::Float32x4.size();
        [
            attribute(base, wgpu::VertexFormat::Float32x4, 4),
            attribute(base + col, wgpu::VertexFormat::Float32x4, 5),
            attribute(base + 2 * col, wgpu::VertexFormat::Float32x4, 6),
            attribute(base + 3 * col, wgpu::VertexFormat::Float32x4, 7),
        ]
    }

    /// Shared instance layout binding `transform` (`4..=7`), `tile_mercator_coords` (`2`), and
    /// `zoom_factor` (`9`), used by fill, fill-extrusion, heatmap, and tile-mask shaders.
    pub fn basic_layout() -> VertexBufferLayout {
        let mut attrs = Self::transform_attributes().to_vec();
        attrs.push(attribute(
            std::mem::offset_of!(Self, zoom_factor) as u64,
            wgpu::VertexFormat::Float32,
            9,
        ));
        attrs.push(attribute(
            std::mem::offset_of!(Self, tile_mercator_coords) as u64,
            wgpu::VertexFormat::Float32x4,
            2,
        ));
        Self::instance_layout(attrs)
    }
}
