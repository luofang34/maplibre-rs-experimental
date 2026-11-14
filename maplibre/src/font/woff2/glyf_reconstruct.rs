//! Reconstructs TrueType `glyf`, `loca`, and transformed `hmtx` tables from WOFF2 substreams.

use super::{Cursor, FontError};

/// Output of reconstructing a WOFF2 transformed `glyf` stream.
pub(super) struct ReconstructedGlyf {
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
    Ok((&composite.bytes[start..end], have_instructions))
}

/// Reconstructs TrueType `glyf` and `loca` tables from a WOFF2 transformed `glyf` stream.
pub(super) fn reconstruct_glyf_loca(stream: &[u8]) -> Result<ReconstructedGlyf, FontError> {
    let mut header = Cursor::new(stream);
    let _reserved = header.read_u16_be()?;
    let option_flags = header.read_u16_be()?;
    let num_glyphs = header.read_u16_be()? as usize;
    let mut index_format = header.read_u16_be()?;
    let n_contour_size = header.read_u32_be()? as usize;
    let n_points_size = header.read_u32_be()? as usize;
    let flag_size = header.read_u32_be()? as usize;
    let glyph_size = header.read_u32_be()? as usize;
    let composite_size = header.read_u32_be()? as usize;
    let bbox_size = header.read_u32_be()? as usize;
    let instruction_size = header.read_u32_be()? as usize;

    let mut n_contours = Cursor::new(header.read_slice(n_contour_size)?);
    let mut n_points = Cursor::new(header.read_slice(n_points_size)?);
    let mut flags = Cursor::new(header.read_slice(flag_size)?);
    let mut glyph_data = Cursor::new(header.read_slice(glyph_size)?);
    let mut composites = Cursor::new(header.read_slice(composite_size)?);
    let mut bboxes = Cursor::new(header.read_slice(bbox_size)?);
    let mut instructions = Cursor::new(header.read_slice(instruction_size)?);

    let bitmap_bytes = num_glyphs.div_ceil(32) * 4;
    let bbox_bitmap = bboxes.read_slice(bitmap_bytes)?;
    let overlap_bitmap = if (option_flags & 0x0001) != 0 {
        Some(header.read_slice(num_glyphs.div_ceil(8))?)
    } else {
        None
    };

    let has_bit = |bitmap: &[u8], glyph_idx: usize| -> bool {
        bitmap
            .get(glyph_idx >> 3)
            .is_some_and(|byte| (byte & (0x80 >> (glyph_idx & 7))) != 0)
    };

    let mut glyf = Vec::new();
    let mut offsets = Vec::with_capacity(num_glyphs + 1);
    let mut x_mins = vec![0_i16; num_glyphs];

    for (glyph_idx, x_min_slot) in x_mins.iter_mut().enumerate() {
        offsets.push(glyf.len());
        let contour_count = n_contours.read_i16_be()?;
        let explicit_bbox = has_bit(bbox_bitmap, glyph_idx);

        if contour_count == 0 {
            if explicit_bbox {
                return Err(FontError::InvalidContainer(
                    "WOFF2 empty glyph cannot carry an explicit bbox",
                ));
            }
            continue;
        }

        if contour_count > 0 {
            let contours = contour_count as usize;
            let mut end_pts = Vec::with_capacity(contours);
            let mut total_points = 0usize;
            for _ in 0..contours {
                let pts = usize::from(n_points.read_255_ushort()?);
                total_points = total_points
                    .checked_add(pts)
                    .filter(|&sum| sum > 0 && sum <= u16::MAX as usize + 1)
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
                let raw_flag = flags.read_u8()?;
                let (dx, dy, on_curve) = decode_triplet(raw_flag, &mut glyph_data)?;
                x = x.saturating_add(dx);
                y = y.saturating_add(dy);
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
                decoded_pts.push((dx, dy, on_curve));
            }

            let instr_len = glyph_data.read_255_ushort()?;
            let instr_bytes = instructions.read_slice(usize::from(instr_len))?;

            let (bx_min, by_min, bx_max, by_max) = if explicit_bbox {
                (
                    bboxes.read_i16_be()?,
                    bboxes.read_i16_be()?,
                    bboxes.read_i16_be()?,
                    bboxes.read_i16_be()?,
                )
            } else {
                (
                    i16::try_from(min_x).unwrap_or(i16::MIN),
                    i16::try_from(min_y).unwrap_or(i16::MIN),
                    i16::try_from(max_x).unwrap_or(i16::MAX),
                    i16::try_from(max_y).unwrap_or(i16::MAX),
                )
            };
            *x_min_slot = bx_min;

            glyf.extend_from_slice(&contour_count.to_be_bytes());
            glyf.extend_from_slice(&bx_min.to_be_bytes());
            glyf.extend_from_slice(&by_min.to_be_bytes());
            glyf.extend_from_slice(&bx_max.to_be_bytes());
            glyf.extend_from_slice(&by_max.to_be_bytes());
            for end_pt in end_pts {
                glyf.extend_from_slice(&end_pt.to_be_bytes());
            }
            glyf.extend_from_slice(&instr_len.to_be_bytes());
            glyf.extend_from_slice(instr_bytes);

            let has_overlap = overlap_bitmap.is_some_and(|bm| has_bit(bm, glyph_idx));
            encode_truetype_points(&decoded_pts, has_overlap, &mut glyf)?;
        } else {
            if !explicit_bbox {
                return Err(FontError::InvalidContainer(
                    "WOFF2 composite glyph requires an explicit bbox",
                ));
            }
            let bx_min = bboxes.read_i16_be()?;
            let by_min = bboxes.read_i16_be()?;
            let bx_max = bboxes.read_i16_be()?;
            let by_max = bboxes.read_i16_be()?;
            *x_min_slot = bx_min;

            let (comp_bytes, have_instr) = scan_composite_components(&mut composites)?;
            let instr_bytes = if have_instr {
                let len = glyph_data.read_255_ushort()?;
                Some((len, instructions.read_slice(usize::from(len))?))
            } else {
                None
            };

            glyf.extend_from_slice(&(-1_i16).to_be_bytes());
            glyf.extend_from_slice(&bx_min.to_be_bytes());
            glyf.extend_from_slice(&by_min.to_be_bytes());
            glyf.extend_from_slice(&bx_max.to_be_bytes());
            glyf.extend_from_slice(&by_max.to_be_bytes());
            glyf.extend_from_slice(comp_bytes);
            if let Some((len, bytes)) = instr_bytes {
                glyf.extend_from_slice(&len.to_be_bytes());
                glyf.extend_from_slice(bytes);
            }
        }

        // Pad each glyph to a 4-byte boundary so short `loca` (`offset / 2`) stays exact.
        let padded = (glyf.len() + 3) & !3;
        glyf.resize(padded, 0);
    }
    offsets.push(glyf.len());

    if index_format == 0 && glyf.len() / 2 > u16::MAX as usize {
        index_format = 1;
    }

    let mut loca = Vec::with_capacity(offsets.len() * if index_format == 0 { 2 } else { 4 });
    if index_format == 0 {
        for off in offsets {
            let word = u16::try_from(off / 2)
                .map_err(|_| FontError::InvalidContainer("WOFF2 short loca overflow"))?;
            loca.extend_from_slice(&word.to_be_bytes());
        }
    } else {
        for off in offsets {
            let dword = u32::try_from(off)
                .map_err(|_| FontError::InvalidContainer("WOFF2 long loca overflow"))?;
            loca.extend_from_slice(&dword.to_be_bytes());
        }
    }

    Ok(ReconstructedGlyf {
        glyf,
        loca,
        index_format,
        x_mins,
    })
}

/// Reconstructs a WOFF2 transformed `hmtx` table (transform version 1, W3C WOFF 2.0 Section 5.4).
pub(super) fn reconstruct_hmtx(
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
    let num_h_metrics = usize::from(u16::from_be_bytes([hhea[34], hhea[35]]));
    let num_glyphs = usize::from(u16::from_be_bytes([maxp[4], maxp[5]]));
    if num_h_metrics > num_glyphs {
        return Err(FontError::InvalidContainer(
            "WOFF2 hmtx numOfHMetrics > numGlyphs",
        ));
    }

    let mut cur = Cursor::new(stream);
    let flags = cur.read_u8()?;
    if (flags & 0xFC) != 0 {
        return Err(FontError::InvalidContainer("WOFF2 hmtx reserved flags set"));
    }
    let omit_proportional_lsb = (flags & 0x01) != 0;
    let omit_monospace_lsb = (flags & 0x02) != 0;

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
