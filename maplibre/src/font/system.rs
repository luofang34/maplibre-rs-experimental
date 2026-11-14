//! In-memory font registry, `Style::font_faces` rule binding, and lazy native system fallback.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use harfrust::Font as HbFont;
use read_fonts::{tables::os2::SelectionFlags, FileRef, FontRef, TableProvider};
use skrifa::MetadataProvider;

#[cfg(not(target_arch = "wasm32"))]
mod native;

use super::{
    container::unpack_font_container,
    sdf::{rasterize_char_sdf, rasterize_glyph_sdf, RasterizedGlyph},
    shaper::{shape_text_runs, ShapedGlyph},
    spec::{parse_font_stack_entry, FontFaceRule, FontStyle, FontWeight, UnicodeRange},
    FontError,
};

/// One parsed OpenType/TrueType face stored inside [`FontSystem`].
#[derive(Clone)]
pub struct LoadedFace {
    /// Unique identifier of this face within its [`FontSystem`].
    pub id: u64,
    /// Shared decompressed `sfnt` or `ttcf` container bytes.
    pub bytes: Arc<Vec<u8>>,
    /// Collection index (`0` for single-face fonts).
    pub face_index: u32,
    /// Cached `harfrust::Font` handle sharing `bytes`.
    pub hb_font: HbFont,
    /// Normalized lowercase family names matched by `"text-font"` entries.
    pub families: Vec<String>,
    /// Weight class (`100..=900`).
    pub weight: FontWeight,
    /// Posture (`normal`, `italic`, `oblique`).
    pub style: FontStyle,
    /// Optional CSS `unicode-range` filter (`empty` = all codepoints in `cmap`).
    pub unicode_ranges: Vec<UnicodeRange>,
}

impl std::fmt::Debug for LoadedFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedFace")
            .field("id", &self.id)
            .field("face_index", &self.face_index)
            .field("families", &self.families)
            .field("weight", &self.weight)
            .field("style", &self.style)
            .finish()
    }
}

impl LoadedFace {
    /// Returns `true` when `ch` passes `unicode_ranges` and maps to a non-`.notdef` glyph in `cmap`.
    pub fn covers_char(&self, ch: char) -> bool {
        let cp = u32::from(ch);
        let in_range = self.unicode_ranges.is_empty()
            || self.unicode_ranges.iter().any(|range| range.contains(cp));
        in_range && self.hb_font.charmap().map_unicode(ch).is_some()
    }

    /// Rasterises the nominal `cmap` glyph for `ch` into a 24px SDF bitmap.
    pub fn rasterize_char(&self, ch: char) -> Option<RasterizedGlyph> {
        rasterize_char_sdf(&self.bytes, self.face_index, ch)
    }

    /// Rasterises `glyph_id` (including unencoded `GSUB` glyphs) into a 24px SDF bitmap.
    pub fn rasterize_glyph(&self, glyph_id: u32) -> Option<RasterizedGlyph> {
        rasterize_glyph_sdf(&self.bytes, self.face_index, glyph_id)
    }
}

/// Registry of in-memory OpenType/WOFF/WOFF2 fonts, style `font-faces`, and lazy native system
/// fallback fonts.
#[derive(Default)]
pub struct FontSystem {
    pub(super) next_id: u64,
    pub(super) faces: HashMap<u64, Arc<LoadedFace>>,
    registered_order: Vec<u64>,
    rule_keys: HashMap<String, Vec<u64>>,
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) native: Option<native::NativeFontDb>,
}

impl std::fmt::Debug for FontSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontSystem")
            .field("registered_faces", &self.registered_order.len())
            .field("total_loaded_faces", &self.faces.len())
            .finish()
    }
}

impl FontSystem {
    /// Creates an empty [`FontSystem`] (system fonts on native targets load lazily on demand).
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up a loaded face by `face_id`.
    pub fn face(&self, face_id: u64) -> Option<&Arc<LoadedFace>> {
        self.faces.get(&face_id)
    }

    /// Rasterises an OpenType `glyph_id` owned by `face_id` into a 24px SDF glyph.
    pub fn rasterize_face_glyph(&self, face_id: u64, glyph_id: u32) -> Option<RasterizedGlyph> {
        self.faces.get(&face_id)?.rasterize_glyph(glyph_id)
    }

    /// Unpacks a `TTF`, `OTF`, `TTC`, `WOFF`, or `WOFF2` font buffer and registers every face in
    /// memory across all targets (including `wasm32`).
    pub fn register_font_bytes(
        &mut self,
        family_override: Option<&str>,
        data: Vec<u8>,
    ) -> Result<Vec<u64>, FontError> {
        let unpacked = unpack_font_container(&data)?.into_owned();
        self.insert_sfnt_faces(
            Arc::new(unpacked),
            family_override,
            None,
            None,
            Vec::new(),
            true,
        )
    }

    /// Returns `true` when `rule` at `url` has already been unpacked and registered.
    pub fn has_registered_rule(&self, rule: &FontFaceRule, url: &str) -> bool {
        let dedup_key = format!(
            "{}|{}|{}|{:?}|{:?}",
            url,
            rule.family.to_ascii_lowercase(),
            rule.weight.0,
            rule.style,
            rule.unicode_ranges
        );
        self.rule_keys.contains_key(&dedup_key)
    }

    /// Unpacks and registers a font fetched for a [`FontFaceRule`], deduplicating by URL and
    /// descriptor metadata so repeated tile loads reuse the same face IDs.
    pub fn register_font_face_rule(
        &mut self,
        rule: &FontFaceRule,
        url: &str,
        data: Vec<u8>,
    ) -> Result<Vec<u64>, FontError> {
        let dedup_key = format!(
            "{}|{}|{}|{:?}|{:?}",
            url,
            rule.family.to_ascii_lowercase(),
            rule.weight.0,
            rule.style,
            rule.unicode_ranges
        );
        if let Some(existing) = self.rule_keys.get(&dedup_key) {
            return Ok(existing.clone());
        }
        let unpacked = unpack_font_container(&data)?.into_owned();
        let ids = self.insert_sfnt_faces(
            Arc::new(unpacked),
            Some(&rule.family),
            Some(rule.weight),
            Some(rule.style),
            rule.unicode_ranges.clone(),
            true,
        )?;
        self.rule_keys.insert(dedup_key, ids.clone());
        Ok(ids)
    }

    /// Returns `true` when at least one in-memory / `font-faces` face matches `font_stack`.
    pub fn has_registered_stack(&self, font_stack: &str) -> bool {
        for raw_entry in font_stack.split(',') {
            let (family, _, _) = parse_font_stack_entry(raw_entry);
            let norm = family.to_ascii_lowercase();
            let raw_norm = raw_entry
                .trim()
                .trim_matches(['"', '\''])
                .to_ascii_lowercase();
            if self.registered_order.iter().any(|id| {
                self.faces
                    .get(id)
                    .is_some_and(|face| face.families.iter().any(|f| f == &norm || f == &raw_norm))
            }) {
                return true;
            }
        }
        false
    }

    /// Resolves the best [`LoadedFace`] for `ch` given a comma-separated MapLibre `font_stack`.
    pub fn resolve_face_for_char(
        &mut self,
        font_stack: &str,
        ch: char,
        allow_system_fallback: bool,
    ) -> Option<(u64, Arc<LoadedFace>)> {
        for raw_entry in font_stack.split(',') {
            let (family, weight, style) = parse_font_stack_entry(raw_entry);
            let raw_norm = raw_entry
                .trim()
                .trim_matches(['"', '\''])
                .to_ascii_lowercase();
            if let Some(hit) = self.best_registered_face(&family, &raw_norm, weight, style, ch) {
                return Some(hit);
            }
        }

        if allow_system_fallback {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(hit) = self.resolve_native_face(font_stack, ch) {
                return Some(hit);
            }

            for &id in &self.registered_order {
                if let Some(face) = self.faces.get(&id) {
                    if face.covers_char(ch) {
                        return Some((id, Arc::clone(face)));
                    }
                }
            }
        }

        None
    }

    /// Rasterises `ch` locally for `localIdeographFontFamily` or missing-glyph fallback.
    pub fn rasterize_local_char(
        &mut self,
        ideograph_family: Option<&str>,
        font_stack: &str,
        ch: char,
    ) -> Option<RasterizedGlyph> {
        if let Some((_, face)) = self.resolve_face_for_char(font_stack, ch, false) {
            if let Some(glyph) = face.rasterize_char(ch) {
                return Some(glyph);
            }
        }
        let style_suffix = stack_style_suffix(font_stack);
        let combined = match ideograph_family {
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
        };
        let (_, face) = self.resolve_face_for_char(&combined, ch, true)?;
        face.rasterize_char(ch)
    }

    /// Shapes `text` with `harfrust` using OpenType faces resolved from `font_stack`.
    pub fn shape_text(
        &mut self,
        font_stack: &str,
        text: &str,
        allow_system_fallback: bool,
    ) -> Option<Vec<ShapedGlyph>> {
        shape_text_runs(text, |ch| {
            if ch.is_whitespace() {
                if let Some((id, face)) =
                    self.resolve_face_for_char(font_stack, ' ', allow_system_fallback)
                {
                    if ch == ' ' || face.covers_char(ch) {
                        return Some((id, face.hb_font.clone()));
                    }
                }
            }
            self.resolve_face_for_char(font_stack, ch, allow_system_fallback)
                .map(|(id, face)| (id, face.hb_font.clone()))
        })
    }

    fn best_registered_face(
        &self,
        family: &str,
        raw_norm: &str,
        target_weight: FontWeight,
        target_style: FontStyle,
        ch: char,
    ) -> Option<(u64, Arc<LoadedFace>)> {
        let norm = family.to_ascii_lowercase();
        let generic_any = matches!(
            norm.as_str(),
            "sans-serif" | "serif" | "monospace" | "system-ui"
        );
        let mut best: Option<(u32, u64, Arc<LoadedFace>)> = None;
        for &id in &self.registered_order {
            let Some(face) = self.faces.get(&id) else {
                continue;
            };
            let name_match = face.families.iter().any(|f| f == &norm || f == raw_norm);
            if !name_match && !generic_any {
                continue;
            }
            if !face.covers_char(ch) {
                continue;
            }
            let style_penalty = if face.style == target_style { 0 } else { 500 };
            let generic_penalty = if name_match { 0 } else { 2000 };
            let weight_diff = u32::from(face.weight.0.abs_diff(target_weight.0));
            let score = generic_penalty + style_penalty + weight_diff;
            if best
                .as_ref()
                .is_none_or(|(best_score, _, _)| score < *best_score)
            {
                best = Some((score, id, Arc::clone(face)));
            }
        }
        best.map(|(_, id, face)| (id, face))
    }

    fn insert_sfnt_faces(
        &mut self,
        bytes: Arc<Vec<u8>>,
        family_override: Option<&str>,
        weight_override: Option<FontWeight>,
        style_override: Option<FontStyle>,
        unicode_ranges: Vec<UnicodeRange>,
        record_registered: bool,
    ) -> Result<Vec<u64>, FontError> {
        let file = FileRef::new(bytes.as_slice())
            .map_err(|_| FontError::InvalidContainer("unreadable OpenType sfnt/ttcf header"))?;
        let face_count = match &file {
            FileRef::Font(_) => 1_u32,
            FileRef::Collection(col) => col.len(),
        };

        let blob: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::clone(&bytes) as _;
        let mut added = Vec::new();
        for index in 0..face_count {
            let Ok(font_ref) = FontRef::from_index(bytes.as_slice(), index) else {
                continue;
            };
            let Some(hb_font) = HbFont::new(Arc::clone(&blob), index) else {
                continue;
            };
            let (meta_families, meta_weight, meta_style) = inspect_face_metadata(&font_ref);
            let mut families = meta_families;
            if let Some(custom) = family_override {
                let clean = custom.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
                if !clean.is_empty() && !families.contains(&clean) {
                    families.insert(0, clean);
                }
            }
            if families.is_empty() {
                families.push("sans-serif".to_owned());
            }

            self.next_id += 1;
            let id = self.next_id;
            let loaded = Arc::new(LoadedFace {
                id,
                bytes: Arc::clone(&bytes),
                face_index: index,
                hb_font,
                families,
                weight: weight_override.unwrap_or(meta_weight),
                style: style_override.unwrap_or(meta_style),
                unicode_ranges: unicode_ranges.clone(),
            });
            self.faces.insert(id, loaded);
            if record_registered {
                self.registered_order.push(id);
            }
            added.push(id);
        }

        if added.is_empty() {
            return Err(FontError::InvalidContainer(
                "no valid OpenType faces in container",
            ));
        }
        Ok(added)
    }
}

pub(super) fn inspect_face_metadata(font: &FontRef<'_>) -> (Vec<String>, FontWeight, FontStyle) {
    let mut seen = HashSet::new();
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

fn stack_style_suffix(font_stack: &str) -> String {
    let first = font_stack.split(',').next().unwrap_or(font_stack);
    let (_, weight, style) = parse_font_stack_entry(first);
    let mut out = String::new();
    match weight.0 {
        0..=150 => out.push_str(" Thin"),
        151..=250 => out.push_str(" ExtraLight"),
        251..=350 => out.push_str(" Light"),
        351..=450 => {}
        451..=550 => out.push_str(" Medium"),
        551..=650 => out.push_str(" Semibold"),
        651..=750 => out.push_str(" Bold"),
        751..=850 => out.push_str(" ExtraBold"),
        _ => out.push_str(" Black"),
    }
    match style {
        FontStyle::Normal => {}
        FontStyle::Italic => out.push_str(" Italic"),
        FontStyle::Oblique => out.push_str(" Oblique"),
    }
    out
}
