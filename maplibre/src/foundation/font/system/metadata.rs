//! Font metadata inspection and CSS style-suffix extraction for [`FontSystem`](super::FontSystem).

#[cfg(feature = "font-shaping")]
use read_fonts::{tables::os2::SelectionFlags, FontRef, TableProvider};
#[cfg(feature = "font-shaping")]
use skrifa::MetadataProvider;

use super::{parse_font_stack_entry, FontStyle, FontWeight};

#[cfg(feature = "font-shaping")]
pub(in crate::foundation::font) fn inspect_face_metadata(
    font: &FontRef<'_>,
) -> (Vec<String>, FontWeight, FontStyle) {
    let mut seen = rustc_hash::FxHashSet::default();
    let mut families = Vec::new();
    for name_id in [
        read_fonts::types::NameId::TYPOGRAPHIC_FAMILY_NAME,
        read_fonts::types::NameId::FAMILY_NAME,
        read_fonts::types::NameId::FULL_NAME,
    ] {
        if let Some(localized) = font.localized_strings(name_id).english_or_first() {
            let text: String = localized.chars().collect();
            let norm = text.trim().to_ascii_lowercase();
            if !norm.is_empty() && seen.insert(norm.clone()) {
                families.push(norm);
            }
        }
    }

    let mut weight = FontWeight::default();
    let mut style = FontStyle::Normal;
    if let Ok(os2) = font.os2() {
        weight = FontWeight(os2.us_weight_class().clamp(1, 1000));
        let flags = os2.fs_selection();
        if flags.contains(SelectionFlags::ITALIC) {
            style = FontStyle::Italic;
        } else if flags.contains(SelectionFlags::OBLIQUE) {
            style = FontStyle::Oblique;
        }
    }
    (families, weight, style)
}

pub(super) fn stack_style_suffix(font_stack: &str) -> String {
    const LABELS: &[(u16, &str)] = &[
        (150, " Thin"),
        (250, " ExtraLight"),
        (350, " Light"),
        (450, ""),
        (550, " Medium"),
        (650, " Semibold"),
        (750, " Bold"),
        (850, " ExtraBold"),
    ];
    let first = font_stack.split(',').next().unwrap_or(font_stack);
    let (_, weight, style) = parse_font_stack_entry(first);
    let w = LABELS
        .iter()
        .find_map(|&(m, l)| (weight.0 <= m).then_some(l))
        .unwrap_or(" Black");
    let s = if matches!(style, FontStyle::Italic) {
        " Italic"
    } else if matches!(style, FontStyle::Oblique) {
        " Oblique"
    } else {
        ""
    };
    [w, s].concat()
}

pub(super) fn normalize_family_token(raw: &str) -> String {
    raw.trim().trim_matches(['"', '\'']).to_ascii_lowercase()
}

pub(super) fn is_non_renderable_char(ch: char) -> bool {
    ch.is_control() || matches!(ch, '\u{200B}'..='\u{200D}' | '\u{FEFF}')
}

pub(super) fn build_local_fallback_stack(
    ideograph_family: Option<&str>,
    font_stack: &str,
) -> String {
    let style_suffix = stack_style_suffix(font_stack);
    match ideograph_family {
        Some(family) if !family.is_empty() => {
            let styled: Vec<String> = family
                .split(',')
                .map(|f| format!("{}{}", f.trim(), style_suffix))
                .collect();
            if font_stack.is_empty() {
                styled.join(",")
            } else {
                format!("{},{}", styled.join(","), font_stack)
            }
        }
        _ => font_stack.to_owned(),
    }
}

pub(super) fn face_matches_name(face_families: &[String], norm: &str, raw_norm: &str) -> bool {
    face_families.iter().any(|f| f == norm || f == raw_norm)
}

pub(super) fn score_registered_face(
    face_weight: FontWeight,
    face_style: FontStyle,
    name_match: bool,
    target_weight: FontWeight,
    target_style: FontStyle,
) -> u32 {
    let style_penalty = if face_style == target_style { 0 } else { 500 };
    let generic_penalty = if name_match { 0 } else { 2000 };
    let weight_diff = u32::from(face_weight.0.abs_diff(target_weight.0));
    generic_penalty + style_penalty + weight_diff
}

#[cfg(feature = "font-shaping")]
pub(super) fn resolve_face_families(
    mut families: Vec<String>,
    family_override: Option<&str>,
) -> Vec<String> {
    if let Some(custom) = family_override {
        let clean = normalize_family_token(custom);
        if !clean.is_empty() && !families.contains(&clean) {
            families.insert(0, clean);
        }
    }
    if families.is_empty() {
        families.push("sans-serif".to_owned());
    }
    families
}
