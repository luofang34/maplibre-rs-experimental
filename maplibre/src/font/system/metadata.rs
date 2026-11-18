//! Font metadata inspection and CSS style-suffix extraction for [`FontSystem`](super::FontSystem).

#[cfg(feature = "font-shaping")]
use read_fonts::{tables::os2::SelectionFlags, FontRef, TableProvider};
#[cfg(feature = "font-shaping")]
use skrifa::MetadataProvider;

#[cfg(feature = "font-shaping")]
use super::FontWeight;
use super::{parse_font_stack_entry, FontStyle};

#[cfg(feature = "font-shaping")]
pub(in crate::font) fn inspect_face_metadata(
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
