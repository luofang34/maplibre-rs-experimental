//! Primitive bit-packing and float16 conversion helpers for vertex metadata formats.

use std::f32::consts::TAU;

#[inline]
pub(super) fn quantize_rgba_u8(color: [f32; 4]) -> [u8; 4] {
    color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

#[inline]
pub(super) fn pack_line_gradient_color(length: f32, opacity: f32) -> [u8; 4] {
    let len_q4 = (length.max(0.0) * 16.0).round().min(16_777_215.0) as u32;
    [
        (len_q4 & 0xFF) as u8,
        ((len_q4 >> 8) & 0xFF) as u8,
        ((len_q4 >> 16) & 0xFF) as u8,
        (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

#[inline]
fn unpack_circle_color_lane(lane: f32) -> [u8; 2] {
    let v = (-lane - 1.0).max(0.0).round() as u32;
    [((v >> 8) & 0xFF) as u8, (v & 0xFF) as u8]
}

#[inline]
pub(super) fn unpack_circle_color_pair(packed: [f32; 4]) -> [[u8; 4]; 2] {
    let [fr, fg] = unpack_circle_color_lane(packed[0]);
    let [fb, fa] = unpack_circle_color_lane(packed[1]);
    let [sr, sg] = unpack_circle_color_lane(packed[2]);
    let [sb, sa] = unpack_circle_color_lane(packed[3]);
    [[fr, fg, fb, fa], [sr, sg, sb, sa]]
}

#[inline]
pub(super) fn quantize_u8_unit(value: f32) -> u32 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u32
}

#[inline]
fn quantize_halo_q4(value: f32) -> u32 {
    (value.max(0.0) * 16.0).round().min(255.0) as u32
}

#[inline]
fn quantize_elev_i20_bits(elevation: f32) -> u32 {
    let elev_i20 = (elevation * 16.0).round().clamp(-524_288.0, 524_287.0) as i32;
    (elev_i20 as u32) & 0x000f_ffff
}

#[inline]
fn quantize_fade_ticks(start_time_ms: f32) -> u32 {
    (start_time_ms.max(0.0) * 0.25).floor().min(524_287.0) as u32
}

#[inline]
pub(super) fn pack_sdf_placement_words(
    target_opacity: f32,
    elevation: f32,
    start_opacity: f32,
    start_time_ms: f32,
    params: [f32; 4],
) -> [u32; 4] {
    let target_u8 = quantize_u8_unit(target_opacity);
    let start_u8 = quantize_u8_unit(start_opacity);
    let style_op_u8 = quantize_u8_unit(params[3]);
    let size_f16 = u32::from(float32_to_float16_bits(params[0].max(0.0)));
    let halo_w_q4 = quantize_halo_q4(params[1]);
    let halo_b_q4 = quantize_halo_q4(params[2]);
    let elev_bits = quantize_elev_i20_bits(elevation);
    let start_ticks = quantize_fade_ticks(start_time_ms);
    [
        elev_bits | (halo_w_q4 << 20) | (((start_ticks >> 16) & 0x7) << 29),
        size_f16,
        (start_ticks & 0xffff) << 16,
        target_u8 | (start_u8 << 8) | (style_op_u8 << 16) | (halo_b_q4 << 24),
    ]
}

#[inline]
fn encode_pose_angle_u16(angle: f32) -> u32 {
    if angle.is_finite() {
        (angle.rem_euclid(TAU) * (65536.0 / TAU)).round() as u32 & 0xffff
    } else {
        0
    }
}

#[inline]
pub(super) fn apply_glyph_pose_words(packed: &mut [u32; 4], angle: f32, rise: f32) {
    let angle_u16 = encode_pose_angle_u16(angle);
    let rise_f16 = u32::from(float32_to_float16_bits(rise));
    packed[0] |= 1_u32 << 28;
    packed[1] = (packed[1] & 0xffff) | (rise_f16 << 16);
    packed[2] = (packed[2] & 0xffff_0000) | angle_u16;
}

#[inline]
pub(super) fn rebase_fade_words_to_zero(packed: &mut [u32; 4]) {
    let target_u8 = packed[3] & 0xff;
    packed[3] = (packed[3] & !0xff00) | (target_u8 << 8);
    packed[0] &= 0x1fff_ffff;
    packed[2] &= 0x0000_ffff;
}

#[inline]
pub(super) fn decode_fade_start_ms(packed: &[u32; 4]) -> f32 {
    let ticks = (packed[2] >> 16) | (((packed[0] >> 29) & 0x7) << 16);
    (ticks as f32) * 4.0
}

#[inline]
pub(super) fn decode_glyph_pose_lane(
    packed: &[u32; 4],
    pose_xy: [f32; 2],
    posed_glyph_offset: f32,
) -> [f32; 4] {
    let rise = f16_bits_to_f32((packed[1] >> 16) as u16);
    let angle = (packed[2] & 0xffff) as f32 * (TAU / 65536.0);
    [pose_xy[0], pose_xy[1], angle, posed_glyph_offset + rise]
}

#[inline]
fn encode_subnormal_f16(sign: u16, abs_bits: u32) -> u16 {
    let exp = (abs_bits >> 23) as i32 - 127;
    let mant = (abs_bits & 0x7f_ffff) | 0x80_0000;
    let shift = (-14 - exp) as u32 + 13;
    let rounded = (mant + (1 << (shift - 1)) - 1 + ((mant >> shift) & 1)) >> shift;
    sign | (rounded as u16)
}

#[inline]
fn encode_normal_f16(sign: u16, abs_bits: u32) -> u16 {
    let exp16 = (((abs_bits >> 23) as i32 - 127 + 15) as u32) << 10;
    let mant = abs_bits & 0x7f_ffff;
    let round_add = 0x0fff + ((mant >> 13) & 1);
    let half = exp16 + ((mant + round_add) >> 13);
    sign | (half.min(0x7bff) as u16)
}

#[inline]
pub(super) fn float32_to_float16_bits(val: f32) -> u16 {
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
        return encode_subnormal_f16(sign, abs_bits);
    }
    encode_normal_f16(sign, abs_bits)
}

#[inline]
fn decode_subnormal_f16(sign: u32, mant: u32) -> u32 {
    if mant == 0 {
        return sign;
    }
    let mut m = mant;
    let mut e = 127_u32 - 14;
    while (m & 0x0400) == 0 {
        m <<= 1;
        e -= 1;
    }
    sign | (e << 23) | ((m & 0x03ff) << 13)
}

#[inline]
pub(super) fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exp = u32::from((bits >> 10) & 0x1f);
    let mant = u32::from(bits & 0x03ff);
    let f32_bits = if exp == 0 {
        decode_subnormal_f16(sign, mant)
    } else if exp == 0x1f {
        sign | 0x7f80_0000 | (mant << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(f32_bits)
}
