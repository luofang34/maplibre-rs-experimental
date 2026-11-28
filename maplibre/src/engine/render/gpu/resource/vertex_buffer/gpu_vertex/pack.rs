//! Pure scalar bit-packing helpers for 16-byte GPU vector and extrusion vertices.

use std::f32::consts::TAU;

use super::super::formats::f32_to_f16_bits;

#[inline]
pub(super) fn encode_shader_vertex_words(
    family: u8,
    position: [f32; 2],
    normal: [f32; 2],
    distance: f32,
    edge_distance: f32,
    elevation: f32,
    corner_code: f32,
) -> [u32; 4] {
    match family {
        1 => encode_line_words(
            position,
            normal,
            distance,
            edge_distance,
            elevation,
            corner_code,
        ),
        2 => encode_extrusion_words(
            position,
            normal,
            corner_code,
            edge_distance,
            distance,
            elevation,
        ),
        3 => encode_point_words(
            position,
            normal,
            distance,
            edge_distance,
            elevation,
            corner_code,
        ),
        _ => encode_fill_words(position, normal),
    }
}

#[inline]
fn pack_f16_pair(low: f32, high: f32) -> u32 {
    u32::from(f32_to_f16_bits(low)) | (u32::from(f32_to_f16_bits(high)) << 16)
}

#[inline]
fn quantize_pos_coord_q2(coord: f32) -> u32 {
    ((coord - 2048.0) * 4.0).round().clamp(-32768.0, 32767.0) as i16 as u16 as u32
}

#[inline]
fn pack_tile_pos_q2(pos: [f32; 2]) -> u32 {
    quantize_pos_coord_q2(pos[0]) | (quantize_pos_coord_q2(pos[1]) << 16)
}

#[inline]
fn encode_fill_words(position: [f32; 2], normal: [f32; 2]) -> [u32; 4] {
    [
        position[0].to_bits(),
        position[1].to_bits(),
        normal[0].to_bits(),
        normal[1].to_bits(),
    ]
}

#[inline]
fn quantize_line_distance_u24(distance: f32) -> u32 {
    let dist_i24 = (distance * 8.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
    (dist_i24 as u32) & 0x00ff_ffff
}

#[inline]
fn unpack_line_style_bits(edge_distance: f32) -> (bool, u32, u32) {
    let style_bits = edge_distance.to_bits() & 0x7f_ffff;
    let per_feature = style_bits != 0;
    let width_q4 = ((style_bits >> 11) & 0xfff).min(0x7ff);
    let offset_q3 = (((style_bits & 0x7ff) as i32 - 1024 + 512).clamp(0, 1023)) as u32;
    (per_feature, width_q4, offset_q3)
}

#[inline]
fn clamp_line_stroke_byte(raw: u32) -> u32 {
    if raw == 0x7ff {
        0xff
    } else {
        raw.min(0xfe)
    }
}

#[inline]
fn encode_line_words(
    position: [f32; 2],
    normal: [f32; 2],
    distance: f32,
    edge_distance: f32,
    elevation: f32,
    corner_code: f32,
) -> [u32; 4] {
    if elevation > -1e20 {
        encode_spatial_line_words(
            position,
            normal,
            distance,
            edge_distance,
            elevation,
            corner_code,
        )
    } else {
        encode_flat_line_words(position, normal, distance, edge_distance, elevation)
    }
}

#[inline]
fn encode_flat_line_words(
    position: [f32; 2],
    normal: [f32; 2],
    distance: f32,
    edge_distance: f32,
    elevation: f32,
) -> [u32; 4] {
    let w0 = pack_tile_pos_q2(position);
    let w1 = pack_f16_pair(normal[0], normal[1]);
    let dist_u24 = quantize_line_distance_u24(distance);
    let (per_feature, width_q4, offset_q3) = unpack_line_style_bits(edge_distance);
    let stroke_bits = elevation.to_bits() & 0x7f_ffff;
    let neg_side = ((stroke_bits >> 22) & 1) != 0;
    let gap_u8 = clamp_line_stroke_byte((stroke_bits >> 11) & 0x7ff);
    let blur_u8 = clamp_line_stroke_byte(stroke_bits & 0x7ff);
    let w2 = dist_u24 | (blur_u8 << 24);
    let w3 = gap_u8
        | (offset_q3 << 8)
        | (width_q4 << 18)
        | (u32::from(per_feature) << 29)
        | (u32::from(neg_side) << 30);
    [w0, w1, w2, w3]
}

#[inline]
fn encode_spatial_line_words(
    position: [f32; 2],
    normal: [f32; 2],
    distance: f32,
    edge_distance: f32,
    elevation: f32,
    corner_code: f32,
) -> [u32; 4] {
    let w0 = pack_tile_pos_q2(position);
    let w1 = pack_f16_pair(normal[0], normal[1]);
    let dist_u24 = quantize_line_distance_u24(distance);
    let (per_feature, width_q4, offset_q3) = unpack_line_style_bits(edge_distance);
    let neg_side = (corner_code - corner_code.floor()) > 0.25;
    let elev_u16 = ((elevation + 1000.0).clamp(0.0, 16383.75) * 4.0).round() as u32 & 0xffff;
    let w2 = dist_u24 | (((elev_u16 >> 8) & 0xff) << 24);
    let w3 = (elev_u16 & 0xff)
        | (offset_q3 << 8)
        | (width_q4 << 18)
        | (u32::from(per_feature) << 29)
        | (u32::from(neg_side) << 30)
        | (1_u32 << 31);
    [w0, w1, w2, w3]
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
fn encode_extrusion_angle_u14(kind: u32, angle: f32) -> u32 {
    if kind >= 2 {
        (angle * (16384.0 / TAU)).round() as u32 & 0x3fff
    } else {
        0
    }
}

#[inline]
fn encode_compact_angle_u9(kind: u32, angle: f32) -> u32 {
    if kind >= 2 {
        (angle * (512.0 / TAU)).round() as u32 & 0x1ff
    } else {
        0
    }
}

#[inline]
fn extrusion_zoom_params(corner_code: f32) -> (f32, f32, f32) {
    let datum_code = (corner_code / 1_048_576.0).floor();
    let zoom_code = corner_code - datum_code * 1_048_576.0;
    let overscale_f = (zoom_code / 65536.0).floor();
    let rise = zoom_code - overscale_f * 65536.0 - 32768.0;
    (datum_code, overscale_f, rise)
}

#[inline]
fn quantize_extrusion_i24(value: f32) -> u32 {
    let scaled = (value * 16.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
    (scaled as u32) & 0x00ff_ffff
}

#[inline]
fn encode_extrusion_words(
    position: [f32; 2],
    normal: [f32; 2],
    corner_code: f32,
    edge_distance: f32,
    distance: f32,
    elevation: f32,
) -> [u32; 4] {
    let w0 = pack_tile_pos_q2(position);
    let (kind, angle) = extrusion_kind_and_angle(normal);
    let angle_u14 = encode_extrusion_angle_u14(kind, angle);
    let (_datum_code, overscale_f, rise) = extrusion_zoom_params(corner_code);
    let overscale = overscale_f.clamp(0.0, 7.0) as u32;
    let edge_u13 = (edge_distance.clamp(0.0, 16382.0) * 0.5).round() as u32 & 0x1fff;
    let w1 = kind | (angle_u14 << 2) | (overscale << 16) | (edge_u13 << 19);
    let rise_f16 = u32::from(f32_to_f16_bits(rise));
    let w2 = quantize_extrusion_i24(distance) | ((rise_f16 & 0xff) << 24);
    let w3 = quantize_extrusion_i24(elevation) | (((rise_f16 >> 8) & 0xff) << 24);
    [w0, w1, w2, w3]
}

#[inline]
pub(super) fn is_compact_extrusion_scalars(
    corner_code: f32,
    edge_distance: f32,
    distance: f32,
    elevation: f32,
) -> bool {
    let (datum_code, _overscale_f, rise) = extrusion_zoom_params(corner_code);
    edge_distance.abs() <= 1e-3
        && datum_code.abs() <= 1e-3
        && rise.abs() <= 1e-3
        && (0.0..=511.5).contains(&distance)
        && (0.0..=1023.5).contains(&elevation)
}

#[inline]
pub(super) fn encode_compact_extrusion_word_pair(
    position: [f32; 2],
    normal: [f32; 2],
    distance: f32,
    elevation: f32,
) -> [u32; 2] {
    let w0 = pack_tile_pos_q2(position);
    let (kind, angle) = extrusion_kind_and_angle(normal);
    let angle_u9 = encode_compact_angle_u9(kind, angle);
    let base_u10 = (distance * 2.0).round().clamp(0.0, 1023.0) as u32;
    let height_u11 = (elevation * 2.0).round().clamp(0.0, 2047.0) as u32;
    let w1 = kind | (angle_u9 << 2) | (base_u10 << 11) | (height_u11 << 21);
    [w0, w1]
}

#[inline]
pub(super) fn unpack_compact_extrusion_words([w0, compact_w1]: [u32; 2]) -> [u32; 4] {
    let rise_f16 = u32::from(f32_to_f16_bits(0.0));
    let kind = compact_w1 & 3;
    let angle_u14 = ((compact_w1 >> 2) & 0x1ff) << 5;
    let base_i24 = (((compact_w1 >> 11) & 0x3ff) * 8) as i32;
    let height_i24 = ((compact_w1 >> 21) * 8) as i32;
    let w1 = kind | (angle_u14 << 2);
    let w2 = ((base_i24 as u32) & 0x00ff_ffff) | ((rise_f16 & 0xff) << 24);
    let w3 = ((height_i24 as u32) & 0x00ff_ffff) | (((rise_f16 >> 8) & 0xff) << 24);
    [w0, w1, w2, w3]
}

#[inline]
fn encode_point_elev_f16(elevation: f32) -> u32 {
    if elevation > -1e20 {
        u32::from(f32_to_f16_bits(elevation.clamp(-30000.0, 65504.0)))
    } else {
        u32::from(f32_to_f16_bits(-65504.0))
    }
}

#[inline]
fn encode_point_stroke_opacity_u8(frac: f32) -> (bool, u32) {
    let has_stroke_op = frac > 0.125;
    let stroke_op_u8 = if has_stroke_op {
        (((frac - 0.25) * 2.0).clamp(0.0, 1.0) * 255.0).round() as u32
    } else {
        0
    };
    (has_stroke_op, stroke_op_u8)
}

#[inline]
fn encode_point_words(
    position: [f32; 2],
    normal: [f32; 2],
    distance: f32,
    edge_distance: f32,
    elevation: f32,
    corner_code: f32,
) -> [u32; 4] {
    let w0 = pack_tile_pos_q2(position);
    let w1 = pack_f16_pair(normal[0], normal[1]);
    let w2 = pack_f16_pair(distance, edge_distance);
    let elev_f16 = encode_point_elev_f16(elevation);
    let corner_floor = corner_code.floor();
    let cell = corner_floor.clamp(0.0, 64.0) as u32;
    let (has_stroke_op, stroke_op_u8) = encode_point_stroke_opacity_u8(corner_code - corner_floor);
    let w3 = elev_f16 | ((cell | (u32::from(has_stroke_op) << 7)) << 16) | (stroke_op_u8 << 24);
    [w0, w1, w2, w3]
}
