//! Text shaping, formatting section trimming, and font/anchor helpers for symbols.

use std::borrow::Cow;

use serde_json::{Map, Value};

use super::{ZOOM_ONLY_ENUMS, ZOOM_ONLY_PAINT_ENUMS};
use crate::style::{expression::FORMAT_IMAGE, property::TextSection};

pub(super) fn is_zoom_only_enum(name: &str) -> bool {
    ZOOM_ONLY_ENUMS.contains(&name) || ZOOM_ONLY_PAINT_ENUMS.contains(&name)
}

pub(super) fn is_symbol_paint_name(name: &str) -> bool {
    !name.starts_with("symbol-")
        && (name.ends_with("-color")
            || name.ends_with("-opacity")
            || name.ends_with("-halo-width")
            || name.ends_with("-halo-blur")
            || name.contains("-translate")
            || name.ends_with("-height-offset")
            || name.ends_with("-height-anchor"))
}

pub(super) fn height_offset_property_key(has_shared: bool, component: &str) -> Cow<'static, str> {
    if has_shared {
        Cow::Borrowed("symbol-height-offset")
    } else {
        Cow::Owned(format!("{component}-height-offset"))
    }
}

pub(super) fn height_follows_ground_value(
    properties: &Map<String, Value>,
    component: &str,
) -> bool {
    properties
        .get("symbol-height-anchor")
        .or_else(|| properties.get(&format!("{component}-height-anchor")))
        .and_then(|value| value.as_str())
        .unwrap_or("ground")
        == "ground"
}

pub(super) fn avoid_edges_value(properties: &Map<String, Value>) -> bool {
    properties
        .get("symbol-avoid-edges")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn join_font_values(fonts: &[Value]) -> String {
    fonts
        .iter()
        .filter_map(|font| font.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn extract_font_stack(properties: &Map<String, Value>) -> String {
    properties
        .get("text-font")
        .and_then(|value| value.as_array())
        .map(|fonts| join_font_values(fonts))
        .filter(|font| !font.is_empty())
        .unwrap_or_else(|| "Open Sans Regular".to_string())
}

pub(super) fn apply_text_transform(text: String, transform: Option<&str>) -> String {
    match transform {
        Some("uppercase") => text.to_uppercase(),
        Some("lowercase") => text.to_lowercase(),
        _ => text,
    }
}

pub(super) fn trim_raw_label_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains(FORMAT_IMAGE) {
        return None;
    }
    Some(trimmed.to_string())
}

fn shape_arabic_text(text: String) -> String {
    if crate::style::arabic_shaping::needs_shaping(&text) {
        crate::style::arabic_shaping::shape(&text)
    } else {
        text
    }
}

fn sections_match_char_count(text: &str, sections: &[TextSection]) -> bool {
    let total: usize = sections.iter().map(|section| section.length).sum();
    !sections.is_empty() && total == text.chars().count()
}

fn shape_section_run(
    chars: &mut std::str::Chars<'_>,
    section: TextSection,
    shaped: &mut String,
) -> TextSection {
    let run = shape_arabic_text(chars.by_ref().take(section.length).collect());
    let length = run.chars().count();
    shaped.push_str(&run);
    TextSection { length, ..section }
}

pub(super) fn shape_text_sections(
    text: String,
    sections: Vec<TextSection>,
) -> (String, Vec<TextSection>) {
    // A case change that alters the length of the text leaves no way to tell which
    // characters belong to which section.
    if !sections_match_char_count(&text, &sections) {
        return (shape_arabic_text(text), Vec::new());
    }
    let mut chars = text.chars();
    let mut shaped = String::with_capacity(text.len());
    let mut shaped_sections = Vec::with_capacity(sections.len());
    for section in sections {
        shaped_sections.push(shape_section_run(&mut chars, section, &mut shaped));
    }
    (shaped, shaped_sections)
}

fn count_leading_whitespace(text: &str) -> usize {
    text.chars().take_while(|c| c.is_whitespace()).count()
}

fn trim_section_run(section: &mut TextSection, skip: &mut usize, remaining: &mut usize) -> bool {
    let dropped = (*skip).min(section.length);
    *skip -= dropped;
    section.length -= dropped;
    section.length = section.length.min(*remaining);
    *remaining -= section.length;
    section.length > 0
}

pub(super) fn trim_shaped_sections(text: &str, sections: Vec<TextSection>) -> Vec<TextSection> {
    if sections.is_empty() || text.trim().is_empty() {
        return Vec::new();
    }
    let mut skip = count_leading_whitespace(text);
    let mut remaining = text.trim().chars().count();
    let mut trimmed = Vec::new();
    for mut section in sections {
        if trim_section_run(&mut section, &mut skip, &mut remaining) {
            trimmed.push(section);
        }
    }
    trimmed
}
