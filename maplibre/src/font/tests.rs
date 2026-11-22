mod cjk_sdf;
mod shaping;
pub(crate) mod synth_font;
mod woff;

use synth_font::{build_synthetic_sfnt, encode_woff1, encode_woff2_transformed};

use super::{
    container::sfnt_checksum, detect_container_format, is_complex_shaping_codepoint,
    is_local_cjk_codepoint, parse_font_stack_entry, rasterize_char_sdf, rasterize_glyph_sdf,
    unpack_font_container, FontContainerFormat, FontFaces, FontStyle, FontSystem, FontWeight,
    UnicodeRange,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn unpacks_ttf_woff1_and_woff2_and_preserves_sfnt_checksum() -> TestResult {
    let sfnt = build_synthetic_sfnt()?;
    assert_eq!(
        detect_container_format(&sfnt),
        Some(FontContainerFormat::Ttf)
    );
    assert_eq!(sfnt_checksum(&sfnt), 0xB1B0_AFBA);

    let woff1 = encode_woff1(&sfnt)?;
    assert_eq!(
        detect_container_format(&woff1),
        Some(FontContainerFormat::Woff)
    );
    let unpacked_w1 = unpack_font_container(&woff1)?;
    assert_eq!(sfnt_checksum(&unpacked_w1), 0xB1B0_AFBA);

    let woff2 = encode_woff2_transformed(&sfnt)?;
    assert_eq!(
        detect_container_format(&woff2),
        Some(FontContainerFormat::Woff2)
    );
    let unpacked_w2 = unpack_font_container(&woff2)?;
    assert_eq!(sfnt_checksum(&unpacked_w2), 0xB1B0_AFBA);

    let glyph_a = rasterize_char_sdf(&unpacked_w2, 0, 'A').ok_or("missing 'A' in WOFF2")?;
    assert!(glyph_a.width > 0 && glyph_a.height > 0);
    assert_eq!(
        glyph_a.bitmap.len(),
        (glyph_a.width as usize + 6) * (glyph_a.height as usize + 6)
    );
    Ok(())
}

#[test]
fn harfrust_shapes_ligatures_indic_conjuncts_and_kerning() -> TestResult {
    let sfnt = build_synthetic_sfnt()?;
    let woff2 = encode_woff2_transformed(&sfnt)?;
    let mut sys = FontSystem::new();
    sys.register_font_bytes(Some("TestSans"), woff2)?;

    let shaped_fi = sys
        .shape_text("TestSans", "fi", false)
        .ok_or("shaping 'fi' failed")?;
    let [fi0] = shaped_fi.as_slice() else {
        return Err(format!("expected 1 ligature glyph, got {}", shaped_fi.len()).into());
    };
    assert_eq!(fi0.glyph_id, 6);
    let lig_sdf = sys
        .rasterize_face_glyph(fi0.face_id, fi0.glyph_id)
        .ok_or("ligature glyph 6 failed to rasterise")?;
    assert!(lig_sdf.width > 0 && !lig_sdf.bitmap.is_empty());

    let shaped_indic = sys
        .shape_text("TestSans", "\u{0915}\u{094D}\u{0937}", false)
        .ok_or("shaping Devanagari conjunct failed")?;
    let [indic0] = shaped_indic.as_slice() else {
        return Err(format!("expected 1 conjunct glyph, got {}", shaped_indic.len()).into());
    };
    assert_eq!(indic0.glyph_id, 10);

    let shaped_aa = sys.shape_text("TestSans", "AA", false).ok_or("AA failed")?;
    let shaped_av = sys.shape_text("TestSans", "AV", false).ok_or("AV failed")?;
    let aa0 = shaped_aa.first().ok_or("missing AA glyph")?;
    let av0 = shaped_av.first().ok_or("missing AV glyph")?;
    assert!(av0.x_advance < aa0.x_advance - 1.5);
    Ok(())
}

#[test]
fn sdf_rasterizer_matches_pbf_baseline_and_interior_threshold() -> TestResult {
    let sfnt = build_synthetic_sfnt()?;
    let space = rasterize_char_sdf(&sfnt, 0, ' ').ok_or("missing space")?;
    assert_eq!((space.width, space.height, space.top), (0, 0, -27));
    assert!(space.advance > 0 && space.bitmap.is_empty());

    let glyph_a = rasterize_char_sdf(&sfnt, 0, 'A').ok_or("missing 'A'")?;
    assert_eq!(glyph_a.top, -10);
    let max_alpha = glyph_a.bitmap.iter().copied().max().unwrap_or(0);
    assert!(
        max_alpha > 192,
        "interior SDF must exceed 0.75 edge threshold (192)"
    );
    assert!(
        glyph_a.bitmap.first().copied().ok_or("empty bitmap")? < 192,
        "outer corner must be outside contour"
    );

    let unencoded = rasterize_glyph_sdf(&sfnt, 0, 6).ok_or("missing gid 6")?;
    assert!(unencoded.width > 0);
    Ok(())
}

#[test]
fn classifies_cjk_sip_and_parses_font_faces_and_unicode_ranges() -> TestResult {
    assert!(is_local_cjk_codepoint('泽'));
    assert!(is_local_cjk_codepoint('あ'));
    assert!(is_local_cjk_codepoint('한'));
    assert!(is_local_cjk_codepoint('𪚥'));
    assert!(is_local_cjk_codepoint('𦨭'));
    assert!(!is_local_cjk_codepoint('A'));
    assert!(is_complex_shaping_codepoint('क'));
    assert!(is_complex_shaping_codepoint('پ'));

    let ranges = UnicodeRange::parse_list("U+0020-007F, U+09??, U+2A6A5");
    let [r0, r1, r2] = ranges.as_slice() else {
        return Err(format!("expected 3 ranges, got {}", ranges.len()).into());
    };
    assert!(r0.contains(u32::from('A')));
    assert!(r1.contains(0x0915) && !r1.contains(0x0A00));
    assert!(r2.contains(u32::from('𪚥')));

    let (fam, weight, style) = parse_font_stack_entry("Open Sans Semibold Italic");
    assert_eq!(fam, "Open Sans");
    assert_eq!(weight, FontWeight(600));
    assert_eq!(style, FontStyle::Italic);

    let parsed: FontFaces = serde_json::from_value(serde_json::json!([
        {
            "font-family": "Custom Sans",
            "src": "url(\"https://example.com/custom.woff2\")",
            "font-weight": "bold",
            "font-style": "italic",
            "unicode-range": "U+0000-00FF"
        }
    ]))?;
    let [face0] = parsed.0.as_slice() else {
        return Err(format!("expected 1 face, got {}", parsed.0.len()).into());
    };
    assert_eq!(face0.family, "Custom Sans");
    assert_eq!(face0.urls, vec!["https://example.com/custom.woff2"]);
    assert_eq!(face0.weight, FontWeight(700));
    assert_eq!(face0.style, FontStyle::Italic);
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn native_system_fallback_rasterizes_cjk_ideographs() -> TestResult {
    let mut sys = FontSystem::new();
    let latin = sys
        .rasterize_local_char(Some("sans-serif"), "Open Sans Regular", 'A')
        .ok_or("expected native system font fallback for Latin 'A'")?;
    assert!(latin.width > 0 && latin.height > 0 && !latin.bitmap.is_empty());

    if let Some(sdf) = sys.rasterize_local_char(Some("sans-serif"), "Open Sans Regular", '泽') {
        assert!(sdf.width > 0 && sdf.height > 0 && !sdf.bitmap.is_empty());
    } else if cfg!(target_os = "macos") {
        return Err("expected macOS native system font fallback for CJK '泽'".into());
    }
    Ok(())
}
