//! Reconstructs TrueType `glyf`, `loca`, and transformed `hmtx` tables from WOFF2 substreams.

use super::{super::container::MAX_DECOMPRESSED_FONT_BYTES, Cursor, FontError};

/// Output of reconstructing a WOFF2 transformed `glyf` stream.
pub(in crate::font) struct ReconstructedGlyf {
    pub glyf: Vec<u8>,
    pub loca: Vec<u8>,
    pub index_format: u16,
    pub x_mins: Vec<i16>,
}

/// Decodes one WOFF2 variable-length `(dx, dy, on_curve)` point triplet (W3C WOFF 2.0 Section 5.1).
fn decode_triplet(flag: u8, glyph_stream: &mut Cursor<'_>) -> Result<(i32, i32, bool), FontError> {
    let on_curve = (flag & 0x80) == 0;
    let f = flag & 0x7F;
    let sign = |bit: u8| if bit != 0 { 1_i32 } else { -1_i32 };
    let (dx, dy) = if f < 10 {
        let b0 = i32::from(glyph_stream.read_u8()?);
        let mag = ((i32::from(f & 0x0E)) << 7) + b0;
        (0, mag * sign(f & 1))
    } else if f < 20 {
        let k = f - 10;
        let b0 = i32::from(glyph_stream.read_u8()?);
        let mag = ((i32::from(k & 0x0E)) << 7) + b0;
        (mag * sign(k & 1), 0)
    } else if f < 84 {
        let k = f - 20;
        let b0 = glyph_stream.read_u8()?;
        let mag_x = 1 + i32::from(k & 0x30) + i32::from(b0 >> 4);
        let mag_y = 1 + (i32::from(k & 0x0C) << 2) + i32::from(b0 & 0x0F);
        (mag_x * sign(k & 1), mag_y * sign(k & 2))
    } else if f < 120 {
        let k = f - 84;
        let group = i32::from(k / 4);
        let b0 = i32::from(glyph_stream.read_u8()?);
        let b1 = i32::from(glyph_stream.read_u8()?);
        let mag_x = 1 + 256 * (group / 3) + b0;
        let mag_y = 1 + 256 * (group % 3) + b1;
        (mag_x * sign(k & 1), mag_y * sign(k & 2))
    } else if f < 124 {
        let k = f - 120;
        let b0 = u16::from(glyph_stream.read_u8()?);
        let b1 = u16::from(glyph_stream.read_u8()?);
        let b2 = u16::from(glyph_stream.read_u8()?);
        let mag_x = i32::from((b0 << 4) | (b1 >> 4));
        let mag_y = i32::from(((b1 & 0x0F) << 8) | b2);
        (mag_x * sign(k & 1), mag_y * sign(k & 2))
    } else {
        let k = f - 124;
        let b0 = u16::from(glyph_stream.read_u8()?);
        let b1 = u16::from(glyph_stream.read_u8()?);
        let b2 = u16::from(glyph_stream.read_u8()?);
        let b3 = u16::from(glyph_stream.read_u8()?);
        let mag_x = i32::from((b0 << 8) | b1);
        let mag_y = i32::from((b2 << 8) | b3);
        (mag_x * sign(k & 1), mag_y * sign(k & 2))
    };
    Ok((dx, dy, on_curve))
}

/// Encodes decoded `(dx, dy, on_curve)` deltas into standard TrueType simple-glyph flag, X, and Y arrays.
fn encode_truetype_points(
    points: &[(i32, i32, bool)],
    has_overlap: bool,
    out: &mut Vec<u8>,
) -> Result<(), FontError> {
    let mut flags = Vec::with_capacity(points.len());
    let mut x_bytes = Vec::with_capacity(points.len() * 2);
    let mut y_bytes = Vec::with_capacity(points.len() * 2);

    for (idx, &(dx, dy, on_curve)) in points.iter().enumerate() {
        let dx_i16 = i16::try_from(dx)
            .map_err(|_| FontError::InvalidContainer("WOFF2 glyph x delta overflow"))?;
        let dy_i16 = i16::try_from(dy)
            .map_err(|_| FontError::InvalidContainer("WOFF2 glyph y delta overflow"))?;
        let mut f = if on_curve { 0x01_u8 } else { 0x00_u8 };
        if idx == 0 && has_overlap {
            f |= 0x40;
        }
        if dx_i16 == 0 {
            f |= 0x10;
        } else if (-255..=255).contains(&dx_i16) {
            f |= 0x02;
            if dx_i16 > 0 {
                f |= 0x10;
            }
            x_bytes.push(dx_i16.unsigned_abs() as u8);
        } else {
            x_bytes.extend_from_slice(&dx_i16.to_be_bytes());
        }

        if dy_i16 == 0 {
            f |= 0x20;
        } else if (-255..=255).contains(&dy_i16) {
            f |= 0x04;
            if dy_i16 > 0 {
                f |= 0x20;
            }
            y_bytes.push(dy_i16.unsigned_abs() as u8);
        } else {
            y_bytes.extend_from_slice(&dy_i16.to_be_bytes());
        }
        flags.push(f);
    }

    out.extend_from_slice(&flags);
    out.extend_from_slice(&x_bytes);
    out.extend_from_slice(&y_bytes);
    Ok(())
}

/// Scans a WOFF2 composite glyph component stream to find its exact byte length and whether
/// TrueType instructions follow (`WE_HAVE_INSTRUCTIONS = 0x0100`).
fn scan_composite_components<'a>(
    composite: &mut Cursor<'a>,
) -> Result<(&'a [u8], bool), FontError> {
    let start = composite.position();
    let mut have_instructions = false;
    loop {
        let flags = composite.read_u16_be()?;
        let _glyph_index = composite.read_u16_be()?;
        // ARG_1_AND_2_ARE_WORDS (0x0001): 4 bytes vs 2 bytes.
        let arg_bytes = if (flags & 0x0001) != 0 { 4 } else { 2 };
        let _ = composite.read_slice(arg_bytes)?;
        // WE_HAVE_A_SCALE (0x0008), WE_HAVE_AN_X_AND_Y_SCALE (0x0040), WE_HAVE_A_TWO_BY_TWO (0x0080).
        let xform_bytes = if (flags & 0x0008) != 0 {
            2
        } else if (flags & 0x0040) != 0 {
            4
        } else if (flags & 0x0080) != 0 {
            8
        } else {
            0
        };
        let _ = composite.read_slice(xform_bytes)?;
        if (flags & 0x0100) != 0 {
            have_instructions = true;
        }
        if (flags & 0x0020) == 0 {
            break;
        }
    }
    let end = composite.position();
    let slice = composite
        .bytes
        .get(start..end)
        .ok_or(FontError::InvalidContainer(
            "WOFF2 composite glyph truncated",
        ))?;
    Ok((slice, have_instructions))
}

struct GlyfStreams<'a> {
    num_glyphs: usize,
    index_format: u16,
    n_contours: Cursor<'a>,
    n_points: Cursor<'a>,
    flags: Cursor<'a>,
    glyph_data: Cursor<'a>,
    composites: Cursor<'a>,
    bboxes: Cursor<'a>,
    instructions: Cursor<'a>,
    bbox_bitmap: &'a [u8],
    overlap_bitmap: Option<&'a [u8]>,
}

fn open_glyf_streams(stream: &[u8]) -> Result<GlyfStreams<'_>, FontError> {
    let mut header = Cursor::new(stream);
    let _reserved = header.read_u16_be()?;
    let option_flags = header.read_u16_be()?;
    let num_glyphs = header.read_u16_be()? as usize;
    let index_format = header.read_u16_be()?;
    if index_format > 1 {
        return Err(FontError::InvalidContainer("WOFF2 invalid indexFormat"));
    }
    let sizes = [
        header.read_u32_be()? as usize,
        header.read_u32_be()? as usize,
        header.read_u32_be()? as usize,
        header.read_u32_be()? as usize,
        header.read_u32_be()? as usize,
        header.read_u32_be()? as usize,
        header.read_u32_be()? as usize,
    ];
    let n_contours = Cursor::new(header.read_slice(sizes[0])?);
    let n_points = Cursor::new(header.read_slice(sizes[1])?);
    let flags = Cursor::new(header.read_slice(sizes[2])?);
    let glyph_data = Cursor::new(header.read_slice(sizes[3])?);
    let composites = Cursor::new(header.read_slice(sizes[4])?);
    let mut bboxes = Cursor::new(header.read_slice(sizes[5])?);
    let instructions = Cursor::new(header.read_slice(sizes[6])?);
    let bbox_bitmap = bboxes.read_slice(num_glyphs.div_ceil(32) * 4)?;
    let overlap_bitmap = ((option_flags & 0x0001) != 0)
        .then(|| header.read_slice(num_glyphs.div_ceil(8)))
        .transpose()?;
    Ok(GlyfStreams {
        num_glyphs,
        index_format,
        n_contours,
        n_points,
        flags,
        glyph_data,
        composites,
        bboxes,
        instructions,
        bbox_bitmap,
        overlap_bitmap,
    })
}

fn has_bitmap_bit(bitmap: &[u8], glyph_idx: usize) -> bool {
    bitmap
        .get(glyph_idx >> 3)
        .is_some_and(|byte| (byte & (0x80 >> (glyph_idx & 7))) != 0)
}

fn reconstruct_simple_glyph(
    s: &mut GlyfStreams<'_>,
    contour_count: i16,
    explicit_bbox: bool,
    has_overlap: bool,
    glyf: &mut Vec<u8>,
) -> Result<i16, FontError> {
    let contours = contour_count as usize;
    let mut end_pts = Vec::with_capacity(contours);
    let mut total_points = 0usize;
    for _ in 0..contours {
        let pts = usize::from(s.n_points.read_255_ushort()?);
        if pts == 0 {
            return Err(FontError::InvalidContainer(
                "WOFF2 zero-point contour in simple glyph",
            ));
        }
        total_points = total_points
            .checked_add(pts)
            .filter(|&sum| sum <= u16::MAX as usize + 1)
            .ok_or(FontError::InvalidContainer(
                "WOFF2 contour point count invalid",
            ))?;
        end_pts.push((total_points - 1) as u16);
    }

    let mut decoded_pts = Vec::with_capacity(total_points);
    let (mut x, mut y) = (0_i32, 0_i32);
    let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
    let (mut max_x, mut max_y) = (i32::MIN, i32::MIN);
    for _ in 0..total_points {
        let raw_flag = s.flags.read_u8()?;
        let (dx, dy, on_curve) = decode_triplet(raw_flag, &mut s.glyph_data)?;
        x = x.saturating_add(dx);
        y = y.saturating_add(dy);
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
        decoded_pts.push((dx, dy, on_curve));
    }

    let instr_len = s.glyph_data.read_255_ushort()?;
    let instr_bytes = s.instructions.read_slice(usize::from(instr_len))?;
    let (bx_min, by_min, bx_max, by_max) = if explicit_bbox {
        (
            s.bboxes.read_i16_be()?,
            s.bboxes.read_i16_be()?,
            s.bboxes.read_i16_be()?,
            s.bboxes.read_i16_be()?,
        )
    } else {
        (
            i16::try_from(min_x).unwrap_or(i16::MIN),
            i16::try_from(min_y).unwrap_or(i16::MIN),
            i16::try_from(max_x).unwrap_or(i16::MAX),
            i16::try_from(max_y).unwrap_or(i16::MAX),
        )
    };
    glyf.extend_from_slice(&contour_count.to_be_bytes());
    for v in [bx_min, by_min, bx_max, by_max] {
        glyf.extend_from_slice(&v.to_be_bytes());
    }
    for end_pt in end_pts {
        glyf.extend_from_slice(&end_pt.to_be_bytes());
    }
    glyf.extend_from_slice(&instr_len.to_be_bytes());
    glyf.extend_from_slice(instr_bytes);
    encode_truetype_points(&decoded_pts, has_overlap, glyf)?;
    Ok(bx_min)
}

fn reconstruct_composite_glyph(
    s: &mut GlyfStreams<'_>,
    explicit_bbox: bool,
    glyf: &mut Vec<u8>,
) -> Result<i16, FontError> {
    if !explicit_bbox {
        return Err(FontError::InvalidContainer(
            "WOFF2 composite glyph requires an explicit bbox",
        ));
    }
    let bx_min = s.bboxes.read_i16_be()?;
    let by_min = s.bboxes.read_i16_be()?;
    let bx_max = s.bboxes.read_i16_be()?;
    let by_max = s.bboxes.read_i16_be()?;
    let (comp_bytes, have_instr) = scan_composite_components(&mut s.composites)?;
    let instr_bytes = if have_instr {
        let len = s.glyph_data.read_255_ushort()?;
        Some((len, s.instructions.read_slice(usize::from(len))?))
    } else {
        None
    };
    for v in [-1_i16, bx_min, by_min, bx_max, by_max] {
        glyf.extend_from_slice(&v.to_be_bytes());
    }
    glyf.extend_from_slice(comp_bytes);
    if let Some((len, bytes)) = instr_bytes {
        glyf.extend_from_slice(&len.to_be_bytes());
        glyf.extend_from_slice(bytes);
    }
    Ok(bx_min)
}

fn encode_loca_offsets(offsets: &[usize], index_format: u16) -> Result<Vec<u8>, FontError> {
    let mut loca = Vec::with_capacity(offsets.len() * if index_format == 0 { 2 } else { 4 });
    if index_format == 0 {
        for &off in offsets {
            let word = u16::try_from(off / 2)
                .map_err(|_| FontError::InvalidContainer("WOFF2 short loca overflow"))?;
            loca.extend_from_slice(&word.to_be_bytes());
        }
    } else {
        for &off in offsets {
            let dword = u32::try_from(off)
                .map_err(|_| FontError::InvalidContainer("WOFF2 long loca overflow"))?;
            loca.extend_from_slice(&dword.to_be_bytes());
        }
    }
    Ok(loca)
}

/// Reconstructs TrueType `glyf` and `loca` tables from a WOFF2 transformed `glyf` stream.
pub(in crate::font) fn reconstruct_glyf_loca(
    stream: &[u8],
) -> Result<ReconstructedGlyf, FontError> {
    let mut s = open_glyf_streams(stream)?;
    let mut glyf = Vec::new();
    let mut offsets = Vec::with_capacity(s.num_glyphs + 1);
    let mut x_mins = vec![0_i16; s.num_glyphs];

    for (glyph_idx, x_min_slot) in x_mins.iter_mut().enumerate() {
        offsets.push(glyf.len());
        let contour_count = s.n_contours.read_i16_be()?;
        if contour_count < -1 {
            return Err(FontError::InvalidContainer("WOFF2 invalid contour count"));
        }
        let explicit_bbox = has_bitmap_bit(s.bbox_bitmap, glyph_idx);
        if contour_count == 0 {
            if explicit_bbox {
                return Err(FontError::InvalidContainer(
                    "WOFF2 empty glyph cannot carry an explicit bbox",
                ));
            }
            continue;
        }
        *x_min_slot = if contour_count > 0 {
            let has_overlap = s
                .overlap_bitmap
                .is_some_and(|bm| has_bitmap_bit(bm, glyph_idx));
            reconstruct_simple_glyph(&mut s, contour_count, explicit_bbox, has_overlap, &mut glyf)?
        } else {
            reconstruct_composite_glyph(&mut s, explicit_bbox, &mut glyf)?
        };
        // Pad each glyph to a 4-byte boundary so short `loca` (`offset / 2`) stays exact.
        let padded = (glyf.len() + 3) & !3;
        if padded > MAX_DECOMPRESSED_FONT_BYTES {
            return Err(FontError::InvalidContainer(
                "WOFF2 reconstructed glyf exceeds size limit",
            ));
        }
        glyf.resize(padded, 0);
    }
    offsets.push(glyf.len());

    let mut index_format = s.index_format;
    if index_format == 0 && glyf.len() / 2 > u16::MAX as usize {
        index_format = 1;
    }
    let loca = encode_loca_offsets(&offsets, index_format)?;
    Ok(ReconstructedGlyf {
        glyf,
        loca,
        index_format,
        x_mins,
    })
}

/// Reconstructs a WOFF2 transformed `hmtx` table (transform version 1, W3C WOFF 2.0 Section 5.4).
pub(in crate::font) fn reconstruct_hmtx(
    stream: &[u8],
    hhea: Option<&[u8]>,
    maxp: Option<&[u8]>,
    x_mins: &[i16],
) -> Result<Vec<u8>, FontError> {
    let hhea = hhea
        .filter(|b| b.len() >= 36)
        .ok_or(FontError::InvalidContainer(
            "WOFF2 transformed hmtx requires hhea table",
        ))?;
    let maxp = maxp
        .filter(|b| b.len() >= 6)
        .ok_or(FontError::InvalidContainer(
            "WOFF2 transformed hmtx requires maxp table",
        ))?;
    let hhea_bytes: [u8; 2] = hhea
        .get(34..36)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(FontError::InvalidContainer("WOFF2 hhea truncated"))?;
    let maxp_bytes: [u8; 2] = maxp
        .get(4..6)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(FontError::InvalidContainer("WOFF2 maxp truncated"))?;
    let num_h_metrics = usize::from(u16::from_be_bytes(hhea_bytes));
    let num_glyphs = usize::from(u16::from_be_bytes(maxp_bytes));
    if num_h_metrics == 0 {
        return Err(FontError::InvalidContainer(
            "WOFF2 hmtx numOfHMetrics is zero",
        ));
    }
    if num_h_metrics > num_glyphs {
        return Err(FontError::InvalidContainer(
            "WOFF2 hmtx numOfHMetrics > numGlyphs",
        ));
    }

    let mut cur = Cursor::new(stream);
    let flags = cur.read_u8()?;
    if flags == 0 || (flags & 0xFC) != 0 {
        return Err(FontError::InvalidContainer("WOFF2 hmtx invalid flags"));
    }
    let omit_proportional_lsb = (flags & 0x01) != 0;
    let omit_monospace_lsb = (flags & 0x02) != 0;
    if (omit_proportional_lsb || omit_monospace_lsb) && x_mins.len() != num_glyphs {
        return Err(FontError::InvalidContainer(
            "WOFF2 hmtx omitted LSBs require reconstructed glyf x_mins",
        ));
    }

    let mut advances = Vec::with_capacity(num_h_metrics);
    for _ in 0..num_h_metrics {
        advances.push(cur.read_u16_be()?);
    }

    let mut out = Vec::with_capacity(num_h_metrics * 4 + (num_glyphs - num_h_metrics) * 2);
    for (idx, adv) in advances.into_iter().enumerate() {
        let lsb = if omit_proportional_lsb {
            x_mins.get(idx).copied().unwrap_or(0)
        } else {
            cur.read_i16_be()?
        };
        out.extend_from_slice(&adv.to_be_bytes());
        out.extend_from_slice(&lsb.to_be_bytes());
    }
    for idx in num_h_metrics..num_glyphs {
        let lsb = if omit_monospace_lsb {
            x_mins.get(idx).copied().unwrap_or(0)
        } else {
            cur.read_i16_be()?
        };
        out.extend_from_slice(&lsb.to_be_bytes());
    }
    Ok(out)
}
