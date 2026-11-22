use super::synth_font::{build_synthetic_sfnt, encode_woff1, encode_woff2_transformed};
use crate::font::{unpack_font_container, FontError};

fn set_be_u32(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset..offset + 4].copy_from_slice(&val.to_be_bytes());
}

fn set_be_u16(buf: &mut [u8], offset: usize, val: u16) {
    buf[offset..offset + 2].copy_from_slice(&val.to_be_bytes());
}

#[test]
fn woff1_rejects_malformed_headers_checksums_and_oversize() -> Result<(), Box<dyn std::error::Error>>
{
    let sfnt = build_synthetic_sfnt()?;
    let woff1 = encode_woff1(&sfnt)?;

    // declared_len < 44 must fail cleanly.
    let mut small_decl = woff1.clone();
    set_be_u32(&mut small_decl, 8, 20);
    assert!(matches!(
        unpack_font_container(&small_decl),
        Err(FontError::InvalidContainer(_))
    ));

    // declared_len exceeding buffer must fail.
    let mut overrun = woff1.clone();
    set_be_u32(&mut overrun, 8, (woff1.len() + 10) as u32);
    assert!(matches!(
        unpack_font_container(&overrun),
        Err(FontError::InvalidContainer(_))
    ));

    // Corrupted table origChecksum must be rejected.
    let mut bad_csum = woff1.clone();
    let old_csum = u32::from_be_bytes(bad_csum[44 + 16..44 + 20].try_into()?);
    set_be_u32(&mut bad_csum, 44 + 16, old_csum ^ 1);
    assert!(matches!(
        unpack_font_container(&bad_csum),
        Err(FontError::InvalidContainer("WOFF1 table checksum mismatch"))
    ));

    // Oversized totalSfntSize (> MAX_DECOMPRESSED_FONT_BYTES) must fail.
    let mut huge_sfnt = woff1;
    set_be_u32(&mut huge_sfnt, 16, 256 * 1024 * 1024);
    assert!(matches!(
        unpack_font_container(&huge_sfnt),
        Err(FontError::InvalidContainer(_))
    ));
    Ok(())
}

#[test]
fn woff2_rejects_invalid_header_transform_versions_and_stream_mismatches(
) -> Result<(), Box<dyn std::error::Error>> {
    let sfnt = build_synthetic_sfnt()?;
    let woff2 = encode_woff2_transformed(&sfnt)?;

    // declared_len < 48 must fail.
    let mut short_len = woff2.clone();
    set_be_u32(&mut short_len, 8, 40);
    assert!(matches!(
        unpack_font_container(&short_len),
        Err(FontError::InvalidContainer("WOFF2 length invalid"))
    ));

    // Oversize totalSfntSize must fail.
    let mut oversize = woff2.clone();
    set_be_u32(&mut oversize, 16, 200 * 1024 * 1024);
    assert!(matches!(
        unpack_font_container(&oversize),
        Err(FontError::InvalidContainer("WOFF2 totalSfntSize too large"))
    ));

    // Invalid transform version bits (bits 6..7 set to 1 on first table) must fail.
    let mut bad_xform = woff2.clone();
    bad_xform[48] |= 0x40;
    assert!(matches!(
        unpack_font_container(&bad_xform),
        Err(FontError::InvalidContainer(_))
    ));

    // Truncated / empty stream or corrupted brotli must fail cleanly without panic.
    let mut truncated = woff2[..48].to_vec();
    set_be_u16(&mut truncated, 12, 1);
    assert!(unpack_font_container(&truncated).is_err());
    Ok(())
}

#[test]
fn woff2_reconstruct_rejects_invalid_index_format_contours_and_hmtx_flags() {
    use crate::font::woff2::glyf_reconstruct::{reconstruct_glyf_loca, reconstruct_hmtx};

    // Minimal 36-byte header for reconstruct_glyf_loca with index_format = 2 (> 1).
    let mut hdr = vec![0_u8; 36];
    set_be_u16(&mut hdr, 6, 2); // indexFormat = 2
    assert!(matches!(
        reconstruct_glyf_loca(&hdr),
        Err(FontError::InvalidContainer("WOFF2 invalid indexFormat"))
    ));

    // Zero-point contour in simple glyph (numGlyphs=1, nContour=1, nPoints=[0]).
    let mut zero_pts = vec![0_u8; 36 + 2 + 1 + 4];
    set_be_u16(&mut zero_pts, 4, 1); // numGlyphs = 1
    set_be_u32(&mut zero_pts, 8, 2); // nContourSize = 2
    set_be_u32(&mut zero_pts, 12, 1); // nPointsSize = 1
    set_be_u32(&mut zero_pts, 28, 4); // bboxSize = 4
    zero_pts[36..38].copy_from_slice(&1_i16.to_be_bytes()); // contourCount = 1
    zero_pts[38] = 0; // pts = 0
    assert!(matches!(
        reconstruct_glyf_loca(&zero_pts),
        Err(FontError::InvalidContainer(
            "WOFF2 zero-point contour in simple glyph"
        ))
    ));

    // contourCount < -1 must fail cleanly.
    let mut neg_contour = zero_pts.clone();
    neg_contour[36..38].copy_from_slice(&(-2_i16).to_be_bytes());
    assert!(matches!(
        reconstruct_glyf_loca(&neg_contour),
        Err(FontError::InvalidContainer("WOFF2 invalid contour count"))
    ));

    // hmtx numOfHMetrics == 0 must fail cleanly.
    let mut hhea = vec![0_u8; 36];
    let mut maxp = vec![0_u8; 6];
    set_be_u16(&mut maxp, 4, 2);
    assert!(matches!(
        reconstruct_hmtx(&[1, 0, 10, 0, 10], Some(&hhea), Some(&maxp), &[0, 0]),
        Err(FontError::InvalidContainer(
            "WOFF2 hmtx numOfHMetrics is zero"
        ))
    ));

    // hmtx flags == 0 must fail cleanly.
    set_be_u16(&mut hhea, 34, 1);
    assert!(matches!(
        reconstruct_hmtx(&[0, 0, 10, 0, 0, 0, 0], Some(&hhea), Some(&maxp), &[0, 0]),
        Err(FontError::InvalidContainer("WOFF2 hmtx invalid flags"))
    ));

    // hmtx omitted LSB with empty x_mins must fail cleanly.
    assert!(matches!(
        reconstruct_hmtx(&[1, 0, 10, 0, 0], Some(&hhea), Some(&maxp), &[]),
        Err(FontError::InvalidContainer(
            "WOFF2 hmtx omitted LSBs require reconstructed glyf x_mins"
        ))
    ));
}
