use super::synth_font::build_synthetic_sfnt;
use crate::font::{rasterize_char_sdf, rasterize_glyph_sdf, FontSystem, SDF_BUFFER};

#[test]
fn cjk_sdf_rasterization_enforces_buffer_symmetry_and_empty_glyph_safety(
) -> Result<(), Box<dyn std::error::Error>> {
    let sfnt = build_synthetic_sfnt()?;
    let glyph = rasterize_char_sdf(&sfnt, 0, 'A').ok_or("glyph A")?;
    let pw = (glyph.width + SDF_BUFFER * 2) as usize;
    let ph = (glyph.height + SDF_BUFFER * 2) as usize;
    assert_eq!(glyph.bitmap.len(), pw * ph);
    // Border corners are strictly outside the glyph contour (< 192 inside cutoff).
    assert!(glyph.bitmap[0] < 128);
    assert!(glyph.bitmap[pw - 1] < 128);
    assert!(glyph.bitmap[pw * ph - 1] < 128);

    // .notdef (gid 0) or space produces empty/zero bitmap without panic.
    let space = rasterize_char_sdf(&sfnt, 0, ' ').ok_or("space")?;
    assert_eq!((space.width, space.height), (0, 0));
    assert!(space.bitmap.is_empty());

    assert!(rasterize_char_sdf(&sfnt, 0, '\u{FFFD}').is_none());
    assert!(rasterize_glyph_sdf(&[0, 1, 2, 3], 0, 0).is_none());
    Ok(())
}

#[test]
fn degenerate_and_extreme_bounds_sfnt_never_overflow_rasterizer(
) -> Result<(), Box<dyn std::error::Error>> {
    let sfnt = build_synthetic_sfnt()?;
    assert!(rasterize_char_sdf(&sfnt, 0, 'A').is_some());
    if let Some(empty) = rasterize_glyph_sdf(&sfnt, 0, u32::MAX) {
        assert_eq!((empty.width, empty.height), (0, 0));
        assert!(empty.bitmap.is_empty());
    }
    #[cfg(target_os = "macos")]
    {
        let mut sys = FontSystem::new();
        for ch in ['中', '文', 'あ', '한'] {
            if let Some(sdf) = sys.rasterize_local_char(Some("sans-serif"), "Noto Sans CJK", ch) {
                assert_eq!(
                    sdf.bitmap.len(),
                    (sdf.width + SDF_BUFFER * 2) as usize * (sdf.height + SDF_BUFFER * 2) as usize
                );
            }
        }
    }
    Ok(())
}
