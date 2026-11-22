use super::synth_font::build_synthetic_sfnt;
use crate::font::{is_local_cjk_codepoint, FontSystem};

#[test]
fn alm_061c_is_ignored_during_shaping() -> Result<(), Box<dyn std::error::Error>> {
    let sfnt = build_synthetic_sfnt()?;
    let mut sys = FontSystem::new();
    sys.register_font_bytes(Some("TestSans"), sfnt)?;

    let plain = sys.shape_text("TestSans", "AA", false).ok_or("plain")?;
    let with_alm = sys
        .shape_text("TestSans", "A\u{061C}A", false)
        .ok_or("with_alm")?;
    assert_eq!(plain.len(), with_alm.len());
    assert_eq!(plain[0].glyph_id, with_alm[0].glyph_id);
    assert_eq!(plain[1].glyph_id, with_alm[1].glyph_id);
    Ok(())
}

#[test]
fn empty_and_control_only_runs_return_none() -> Result<(), Box<dyn std::error::Error>> {
    let sfnt = build_synthetic_sfnt()?;
    let mut sys = FontSystem::new();
    sys.register_font_bytes(Some("TestSans"), sfnt)?;

    assert!(sys.shape_text("TestSans", "", false).is_none());
    assert!(sys
        .shape_text(
            "TestSans",
            "\u{061C}\u{200E}\u{200F}\u{2067}\u{2069}",
            false
        )
        .is_none());
    assert!(sys.shape_text("TestSans", "", true).is_none());
    assert!(sys
        .shape_text("TestSans", "\u{061C}\u{200F}", true)
        .is_none());
    Ok(())
}

#[test]
fn vertical_combining_mark_stays_in_base_face_run() -> Result<(), Box<dyn std::error::Error>> {
    let sfnt = build_synthetic_sfnt()?;
    let mut sys = FontSystem::new();
    sys.register_font_bytes(Some("TestSans"), sfnt)?;

    // Vertical shaping with combining mark keeps the mark glued to its base face run.
    let shaped = sys
        .shape_text("TestSans", "A\u{0301}", true)
        .ok_or("vertical combining")?;
    assert!(!shaped.is_empty());
    let face_ids: Vec<u64> = shaped.iter().map(|g| g.face_id).collect();
    assert!(face_ids.windows(2).all(|w| w[0] == w[1]));
    Ok(())
}

#[test]
fn mixed_script_horizontal_shaping_resolves_all_faces() -> Result<(), Box<dyn std::error::Error>> {
    let sfnt = build_synthetic_sfnt()?;
    let mut sys = FontSystem::new();
    sys.register_font_bytes(Some("TestSans"), sfnt)?;

    let out = sys
        .shape_text("TestSans", "AV \u{0915}\u{094D}\u{0937}", false)
        .ok_or("mixed script")?;
    assert!(out.len() >= 3);
    assert!(is_local_cjk_codepoint('字'));
    assert!(!is_local_cjk_codepoint('Z'));
    Ok(())
}
