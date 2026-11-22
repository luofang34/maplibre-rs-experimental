//! Compact 16-byte GPU vertex encoding (`GpuShaderVertex`) for vector tile pipelines.
//!
//! CPU tessellation and spatial-elevation systems operate on 32-byte `f32` [`ShaderVertex`]
//! records, while [`BufferPool`](crate::vector::BufferPool) packs them into 16-byte
//! [`GpuShaderVertex`] words on upload (`-50%` vertex VRAM and PCIe/WebGPU `writeBuffer` bytes).

use std::f32::consts::TAU;

use bytemuck::Pod as PodTrait;
use bytemuck_derive::{Pod, Zeroable};

use super::{ShaderSymbolVertex, ShaderVertex};

/// Compact 16-byte GPU vertex record stored in [`VectorBufferPool`](crate::vector::VectorBufferPool).
/// Vertex encoding family matching WGSL vertex buffer decoding for vector geometry pipelines.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuVertexKind {
    /// Fill/polygon pipeline vertex encoding.
    Fill,
    /// Line/stroke pipeline vertex encoding.
    Line,
    /// Fill-extrusion pipeline vertex encoding.
    Extrusion,
}

impl GpuVertexKind {
    pub(crate) fn from_layer_type(layer_type: &str) -> Option<Self> {
        match layer_type {
            "line" => Some(Self::Line),
            "fill-extrusion" => Some(Self::Extrusion),
            "circle" | "heatmap" => None,
            _ => Some(Self::Fill),
        }
    }

    #[inline]
    pub(crate) fn encode(self, v: &ShaderVertex) -> GpuShaderVertex {
        match self {
            Self::Fill => encode_fill_vertex(v),
            Self::Line => encode_line_vertex(v),
            Self::Extrusion => encode_extrusion_vertex(v),
        }
    }
}

/// Packed GPU-ready vertex layout (`16` bytes, 4×`u32`) matching [`ShaderVertex`].
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuShaderVertex(pub [u32; 4]);

/// Zero-byte layer metadata marker for pools (such as [`SymbolBufferPool`](crate::sdf::SymbolBufferPool))
/// that carry per-layer uniforms via bind groups rather than a Slot 2 vertex buffer.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct EmptyLayerMetadata(pub [u8; 0]);

/// Maps a CPU vertex type `Self` to its GPU-resident storage representation `Self::Gpu`.
pub trait PoolVertex: PodTrait {
    /// GPU vertex record uploaded to the vertex slab.
    type Gpu: PodTrait + std::fmt::Debug;

    /// Returns a compact tag identifying which GPU bit-layout `layer_type` produces, so shared
    /// source-layer vertex slabs only alias across compatible shader families.
    #[inline]
    fn encoding_family(layer_type: &str) -> u8 {
        let _ = layer_type;
        0
    }

    /// Required alignment of `first_vertex` in elements (`4` for quad-indexed shaders that decode
    /// corners via `@builtin(vertex_index) % 4u`, `1` otherwise).
    #[inline]
    fn vertex_element_alignment(layer_type: &str) -> u64 {
        let _ = layer_type;
        1
    }

    /// Returns `true` when `src` fits losslessly in the compact 8-byte extrusion format.
    #[inline]
    fn can_encode_compact_extrusion(layer_type: &str, src: &[Self]) -> bool {
        let _ = (layer_type, src);
        false
    }

    /// Packs `src` into two 8-byte compact extrusion vertices per `Self::Gpu` word when eligible.
    #[inline]
    fn try_encode_compact_extrusion<'a>(
        layer_type: &str,
        src: &'a [Self],
        scratch: &'a mut Vec<Self::Gpu>,
    ) -> Option<&'a [Self::Gpu]> {
        let _ = (layer_type, src, scratch);
        None
    }

    /// Returns `src` in `Self::Gpu` form, either by zero-copy slice borrow or by packing into `scratch`.
    fn encode_vertices<'a>(
        layer_type: &str,
        src: &'a [Self],
        scratch: &'a mut Vec<Self::Gpu>,
    ) -> &'a [Self::Gpu];
}

impl PoolVertex for ShaderSymbolVertex {
    type Gpu = Self;

    #[inline]
    fn encode_vertices<'a>(
        _layer_type: &str,
        src: &'a [Self],
        _scratch: &'a mut Vec<Self::Gpu>,
    ) -> &'a [Self::Gpu] {
        src
    }
}

impl PoolVertex for ShaderVertex {
    type Gpu = GpuShaderVertex;

    #[inline]
    fn encoding_family(layer_type: &str) -> u8 {
        match layer_type {
            "line" => 1,
            "fill-extrusion" => 2,
            "circle" | "heatmap" => 3,
            _ => 0,
        }
    }

    #[inline]
    fn vertex_element_alignment(layer_type: &str) -> u64 {
        match layer_type {
            "circle" | "heatmap" => 4,
            _ => 1,
        }
    }

    fn encode_vertices<'a>(
        layer_type: &str,
        src: &'a [Self],
        scratch: &'a mut Vec<GpuShaderVertex>,
    ) -> &'a [GpuShaderVertex] {
        scratch.clear();
        scratch.reserve(src.len());
        match Self::encoding_family(layer_type) {
            1 => scratch.extend(src.iter().map(encode_line_vertex)),
            2 => scratch.extend(src.iter().map(encode_extrusion_vertex)),
            3 => scratch.extend(src.iter().map(encode_point_vertex)),
            _ => scratch.extend(src.iter().map(encode_fill_vertex)),
        }
        scratch.as_slice()
    }

    #[inline]
    fn can_encode_compact_extrusion(layer_type: &str, src: &[Self]) -> bool {
        layer_type == "fill-extrusion" && can_encode_compact_extrusion(src)
    }

    #[inline]
    fn try_encode_compact_extrusion<'a>(
        layer_type: &str,
        src: &'a [Self],
        scratch: &'a mut Vec<GpuShaderVertex>,
    ) -> Option<&'a [GpuShaderVertex]> {
        if layer_type != "fill-extrusion" || !can_encode_compact_extrusion(src) {
            return None;
        }
        encode_compact_extrusion_vertices(src, scratch);
        Some(scratch.as_slice())
    }
}

#[inline]
fn encode_fill_vertex(v: &ShaderVertex) -> GpuShaderVertex {
    GpuShaderVertex([
        v.position[0].to_bits(),
        v.position[1].to_bits(),
        v.normal[0].to_bits(),
        v.normal[1].to_bits(),
    ])
}

#[inline]
fn pack_tile_pos_q2(pos: [f32; 2]) -> u32 {
    let q = |c: f32| ((c - 2048.0) * 4.0).round().clamp(-32768.0, 32767.0) as i16 as u16 as u32;
    q(pos[0]) | (q(pos[1]) << 16)
}

#[inline]
fn encode_line_vertex(v: &ShaderVertex) -> GpuShaderVertex {
    let w0 = pack_tile_pos_q2(v.position);
    let w1 = pack_f16_pair(v.normal[0], v.normal[1]);
    let dist_i24 = (v.distance * 8.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
    let dist_u24 = (dist_i24 as u32) & 0x00ff_ffff;
    let spatial = v.elevation > -1e20;
    let style_bits = v.edge_distance.to_bits() & 0x7f_ffff;
    let per_feature = style_bits != 0;
    let width_q4 = ((style_bits >> 11) & 0xfff).min(0x7ff);
    let offset_q3 = (((style_bits & 0x7ff) as i32 - 1024 + 512).clamp(0, 1023)) as u32;

    if !spatial {
        let stroke_bits = v.elevation.to_bits() & 0x7f_ffff;
        let neg_side = ((stroke_bits >> 22) & 1) != 0;
        let gap_raw = (stroke_bits >> 11) & 0x7ff;
        let blur_raw = stroke_bits & 0x7ff;
        let gap_u8 = if gap_raw == 0x7ff {
            0xff
        } else {
            gap_raw.min(0xfe)
        };
        let blur_u8 = if blur_raw == 0x7ff {
            0xff
        } else {
            blur_raw.min(0xfe)
        };
        let w2 = dist_u24 | (blur_u8 << 24);
        let w3 = gap_u8
            | (offset_q3 << 8)
            | (width_q4 << 18)
            | (u32::from(per_feature) << 29)
            | (u32::from(neg_side) << 30);
        GpuShaderVertex([w0, w1, w2, w3])
    } else {
        let neg_side = (v.corner_code - v.corner_code.floor()) > 0.25;
        let elev_u16 = ((v.elevation + 1000.0).clamp(0.0, 16383.75) * 4.0).round() as u32 & 0xffff;
        let w2 = dist_u24 | (((elev_u16 >> 8) & 0xff) << 24);
        let w3 = (elev_u16 & 0xff)
            | (offset_q3 << 8)
            | (width_q4 << 18)
            | (u32::from(per_feature) << 29)
            | (u32::from(neg_side) << 30)
            | (1_u32 << 31);
        GpuShaderVertex([w0, w1, w2, w3])
    }
}

#[inline]
fn encode_extrusion_vertex(v: &ShaderVertex) -> GpuShaderVertex {
    let w0 = pack_tile_pos_q2(v.position);
    let len = v.normal[0].hypot(v.normal[1]);
    let kind = if len <= 0.1 {
        0_u32
    } else if len <= 0.5 {
        1_u32
    } else if len <= 1.5 {
        2_u32
    } else {
        3_u32
    };
    let angle_u14 = if kind >= 2 {
        (v.normal[1].atan2(v.normal[0]).rem_euclid(TAU) * (16384.0 / TAU)).round() as u32 & 0x3fff
    } else {
        0
    };
    let datum_code = (v.corner_code / 1_048_576.0).floor();
    let zoom_code = v.corner_code - datum_code * 1_048_576.0;
    let overscale_f = (zoom_code / 65536.0).floor();
    let overscale = overscale_f.clamp(0.0, 7.0) as u32;
    let rise = zoom_code - overscale_f * 65536.0 - 32768.0;
    let edge_u13 = (v.edge_distance.clamp(0.0, 16382.0) * 0.5).round() as u32 & 0x1fff;
    let w1 = kind | (angle_u14 << 2) | (overscale << 16) | (edge_u13 << 19);

    let base_i24 = (v.distance * 16.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
    let height_i24 = (v.elevation * 16.0)
        .round()
        .clamp(-8_388_608.0, 8_388_607.0) as i32;
    let rise_f16 = u32::from(f32_to_f16_bits(rise));
    let w2 = ((base_i24 as u32) & 0x00ff_ffff) | ((rise_f16 & 0xff) << 24);
    let w3 = ((height_i24 as u32) & 0x00ff_ffff) | (((rise_f16 >> 8) & 0xff) << 24);
    GpuShaderVertex([w0, w1, w2, w3])
}

#[inline]
fn extrusion_kind_and_angle(normal: [f32; 2]) -> (u32, f32) {
    let len = normal[0].hypot(normal[1]);
    let kind = if len <= 0.1 {
        0_u32
    } else if len <= 0.5 {
        1_u32
    } else if len <= 1.5 {
        2_u32
    } else {
        3_u32
    };
    let angle = if kind >= 2 {
        normal[1].atan2(normal[0]).rem_euclid(TAU)
    } else {
        0.0
    };
    (kind, angle)
}

#[inline]
fn extrusion_zoom_params(corner_code: f32) -> (f32, f32, f32) {
    let datum_code = (corner_code / 1_048_576.0).floor();
    let zoom_code = corner_code - datum_code * 1_048_576.0;
    let overscale_f = (zoom_code / 65536.0).floor();
    let rise = zoom_code - overscale_f * 65536.0 - 32768.0;
    (datum_code, overscale_f, rise)
}

/// Returns `true` when `v` fits in the compact 8-byte `fill-extrusion` vertex format.
#[inline]
pub(crate) fn is_compact_extrusion_vertex(v: &ShaderVertex) -> bool {
    let (datum_code, _overscale_f, rise) = extrusion_zoom_params(v.corner_code);
    v.edge_distance.abs() <= 1e-3
        && datum_code.abs() <= 1e-3
        && rise.abs() <= 1e-3
        && (0.0..=511.5).contains(&v.distance)
        && (0.0..=1023.5).contains(&v.elevation)
}

/// Returns `true` when every vertex in `src` fits in the compact 8-byte `fill-extrusion`
/// vertex format (base in `0..=511.5` m, height in `0..=1023.5` m, zero `edge_distance`,
/// default ground-datum `0`, and zero zoom-interpolated `rise`).
pub fn can_encode_compact_extrusion(src: &[ShaderVertex]) -> bool {
    !src.is_empty() && src.iter().all(is_compact_extrusion_vertex)
}

#[inline]
pub(crate) fn encode_compact_extrusion_words(v: &ShaderVertex) -> [u32; 2] {
    let w0 = pack_tile_pos_q2(v.position);
    let (kind, angle) = extrusion_kind_and_angle(v.normal);
    let angle_u9 = if kind >= 2 {
        (angle * (512.0 / TAU)).round() as u32 & 0x1ff
    } else {
        0
    };
    let base_u10 = (v.distance * 2.0).round().clamp(0.0, 1023.0) as u32;
    let height_u11 = (v.elevation * 2.0).round().clamp(0.0, 2047.0) as u32;
    let w1 = kind | (angle_u9 << 2) | (base_u10 << 11) | (height_u11 << 21);
    [w0, w1]
}

/// Expands a single 8-byte compact extrusion record into a standard 16-byte [`GpuShaderVertex`].
#[inline]
pub(crate) fn unpack_compact_extrusion_word([w0, compact_w1]: [u32; 2]) -> GpuShaderVertex {
    let rise_f16 = u32::from(f32_to_f16_bits(0.0));
    let kind = compact_w1 & 3;
    let angle_u14 = ((compact_w1 >> 2) & 0x1ff) << 5;
    let base_i24 = (((compact_w1 >> 11) & 0x3ff) * 8) as i32;
    let height_i24 = ((compact_w1 >> 21) * 8) as i32;
    let w1 = kind | (angle_u14 << 2);
    let w2 = ((base_i24 as u32) & 0x00ff_ffff) | ((rise_f16 & 0xff) << 24);
    let w3 = ((height_i24 as u32) & 0x00ff_ffff) | (((rise_f16 >> 8) & 0xff) << 24);
    GpuShaderVertex([w0, w1, w2, w3])
}

/// Packs two 8-byte compact extrusion vertices per 16-byte [`GpuShaderVertex`] word.
pub fn encode_compact_extrusion_vertices(src: &[ShaderVertex], out: &mut Vec<GpuShaderVertex>) {
    out.clear();
    out.reserve(src.len().div_ceil(2));
    for chunk in src.chunks(2) {
        let [first, rest @ ..] = chunk else { continue };
        let a = encode_compact_extrusion_words(first);
        let b = rest.first().map_or([0, 0], encode_compact_extrusion_words);
        out.push(GpuShaderVertex([a[0], a[1], b[0], b[1]]));
    }
}

/// Expands `logical_count` compact 8-byte extrusion vertices back into standard 16-byte
/// [`GpuShaderVertex`] records for fallback non-compact pipelines.
pub fn unpack_compact_extrusion_vertices(
    packed: &[GpuShaderVertex],
    logical_count: usize,
    out: &mut Vec<GpuShaderVertex>,
) {
    out.clear();
    out.reserve(logical_count);
    let mut emitted = 0_usize;
    for word in packed {
        for half in [[word.0[0], word.0[1]], [word.0[2], word.0[3]]] {
            if emitted >= logical_count {
                return;
            }
            out.push(unpack_compact_extrusion_word(half));
            emitted += 1;
        }
    }
}

#[inline]
fn encode_point_vertex(v: &ShaderVertex) -> GpuShaderVertex {
    let w0 = pack_tile_pos_q2(v.position);
    let w1 = pack_f16_pair(v.normal[0], v.normal[1]);
    let w2 = pack_f16_pair(v.distance, v.edge_distance);
    let elev_f16 = if v.elevation > -1e20 {
        f32_to_f16_bits(v.elevation.clamp(-30000.0, 65504.0))
    } else {
        f32_to_f16_bits(-65504.0)
    };
    let corner_floor = v.corner_code.floor();
    let cell = corner_floor.clamp(0.0, 64.0) as u32;
    let frac = v.corner_code - corner_floor;
    let has_stroke_op = frac > 0.125;
    let stroke_op_u8 = if has_stroke_op {
        (((frac - 0.25) * 2.0).clamp(0.0, 1.0) * 255.0).round() as u32
    } else {
        0
    };
    let w3 = u32::from(elev_f16)
        | ((cell | (u32::from(has_stroke_op) << 7)) << 16)
        | (stroke_op_u8 << 24);
    GpuShaderVertex([w0, w1, w2, w3])
}

/// Packs two finite `f32` values into IEEE-754 binary16 lanes matching WGSL `unpack2x16float`.
#[inline]
pub fn pack_f16_pair(low: f32, high: f32) -> u32 {
    u32::from(f32_to_f16_bits(low)) | (u32::from(f32_to_f16_bits(high)) << 16)
}

/// Converts a finite `f32` (clamped to `[-65504.0, 65504.0]`) into IEEE-754 `binary16` bits.
#[inline]
pub fn f32_to_f16_bits(val: f32) -> u16 {
    if !val.is_finite() {
        return if val.is_nan() {
            0
        } else if val.is_sign_negative() {
            0xfbff
        } else {
            0x7bff
        };
    }
    let clamped = val.clamp(-65504.0, 65504.0);
    let bits = clamped.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let abs_bits = bits & 0x7fff_ffff;
    if abs_bits < 0x3300_0000 {
        return sign;
    }
    if abs_bits < 0x3880_0000 {
        // Subnormal f16
        let exp = (abs_bits >> 23) as i32 - 127;
        let mant = (abs_bits & 0x7f_ffff) | 0x80_0000;
        let shift = (-14 - exp) as u32 + 13;
        let rounded = (mant + (1 << (shift - 1)) - 1 + ((mant >> shift) & 1)) >> shift;
        return sign | (rounded as u16);
    }
    let exp16 = (((abs_bits >> 23) as i32 - 127 + 15) as u32) << 10;
    let mant = abs_bits & 0x7f_ffff;
    let round_add = 0x0fff + ((mant >> 13) & 1);
    let half = exp16 + ((mant + round_add) >> 13);
    sign | (half.min(0x7bff) as u16)
}

/// Converts IEEE-754 `binary16` bits back to `f32`.
#[inline]
pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exp = u32::from((bits >> 10) & 0x1f);
    let mant = u32::from(bits & 0x03ff);
    let f32_bits = if exp == 0 {
        if mant == 0 {
            sign
        } else {
            let mut m = mant;
            let mut e = 127_u32 - 14;
            while (m & 0x0400) == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x03ff) << 13)
        }
    } else if exp == 0x1f {
        sign | 0x7f80_0000 | (mant << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(f32_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::tessellation::pack_extrusion_code;

    #[test]
    fn compact_extrusion_allows_overscale_and_tall_base_without_rise_or_pattern() {
        let mut v = ShaderVertex::new([100.0, 200.0], [1.0, 0.0]);
        v.distance = 350.0;
        v.elevation = 450.5;
        v.corner_code = pack_extrusion_code(0.0, 2, 0);
        assert!(can_encode_compact_extrusion(&[v]));
        let unpacked = unpack_compact_extrusion_word(encode_compact_extrusion_words(&v));
        assert!((((unpacked.0[2] << 8) as i32 >> 8) as f32 / 16.0 - 350.0).abs() <= 0.5);
        assert!((((unpacked.0[3] << 8) as i32 >> 8) as f32 / 16.0 - 450.5).abs() <= 0.5);
        for bad in [(5.0, 0.0, 0), (0.0, 4.0, 0), (0.0, 0.0, 1)] {
            let mut other = v;
            (other.edge_distance, other.corner_code) =
                (bad.0, pack_extrusion_code(bad.1, 0, bad.2));
            assert!(!can_encode_compact_extrusion(&[other]));
        }
    }
}
