//! OpenType `GSUB` + `GPOS` complex text shaping powered by `harfrust`.
//!
//! Supports unencoded `GlyphId` substitutions (Indic half-forms/conjuncts, Devanagari/Bengali/
//! Tamil/Thai/Khmer/Myanmar reordering, Latin `fi`/`fl` ligatures, Arabic contextual forms),
//! `GPOS` kerning, cursive attachment, and combining-mark positioning (`mark`/`mkmk`).

#[cfg(feature = "font-shaping")]
use std::ops::Range;

#[cfg(feature = "font-shaping")]
use harfrust::{
    Buffer, BufferFlags, Direction, Feature, Font as HbFont, ShapeOptions, ShaperFont, Tag,
};
use serde::{Deserialize, Serialize};
#[cfg(feature = "font-shaping")]
use unicode_bidi::BidiInfo;

#[cfg(feature = "font-shaping")]
use super::sdf::SDF_EM_SIZE;

/// Atlas lookup key for either a nominal Unicode scalar or an OpenType glyph index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GlyphKey {
    /// Direct Unicode scalar (`char as u32`), used for `.pbf` ranges and unshaped CJK glyphs.
    Codepoint(u32),
    /// Font-specific OpenType glyph index produced by `harfrust` shaping.
    GlyphId {
        /// Identifier of the owning face inside [`super::FontSystem`].
        face_id: u64,
        /// OpenType glyph index (`0` = `.notdef`).
        glyph_id: u32,
    },
}

/// Formats `(face_id, glyph_id)` as a deterministic string key for JSON-compatible atlas maps.
pub fn face_glyph_atlas_key(face_id: u64, glyph_id: u32) -> String {
    format!("{face_id}:{glyph_id}")
}

/// One positioned glyph emitted by OpenType shaping, scaled to MapLibre's `24.0` px em box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapedGlyph {
    /// Identifier of the face in [`super::FontSystem`] that owns `glyph_id`.
    pub face_id: u64,
    /// OpenType glyph index (`harfrust::GlyphInfo::glyph_id`).
    pub glyph_id: u32,
    /// UTF-8 byte offset of the originating cluster within the input string.
    pub cluster: u32,
    /// Horizontal pen advance in 24px-em pixels.
    pub x_advance: f32,
    /// Vertical pen advance in 24px-em pixels.
    pub y_advance: f32,
    /// Horizontal offset from the current pen position in 24px-em pixels (`+x` rightward).
    pub x_offset: f32,
    /// Vertical offset from the baseline in 24px-em pixels (`+y` upward in font design space).
    pub y_offset: f32,
    /// `true` when this glyph belongs to a right-to-left BiDi run.
    #[serde(default)]
    pub rtl: bool,
}

impl ShapedGlyph {
    /// String key used in `SymbolAtlas::face_glyphs`.
    pub fn atlas_key(&self) -> String {
        face_glyph_atlas_key(self.face_id, self.glyph_id)
    }
}

/// Shapes a single unidirectional slice `full_text[byte_range]` with `font` via `harfrust`.
///
/// Cluster offsets in the returned [`ShapedGlyph`]s are UTF-8 byte indices into `full_text` plus
/// `cluster_base`.
#[cfg(feature = "font-shaping")]
pub fn shape_slice_with_font(
    face_id: u64,
    font: &HbFont,
    full_text: &str,
    byte_range: Range<usize>,
    cluster_base: usize,
    rtl: bool,
) -> Vec<ShapedGlyph> {
    let Some(slice) = full_text.get(byte_range.clone()) else {
        return Vec::new();
    };
    if slice.is_empty() {
        return Vec::new();
    }

    let shaper = ShaperFont::new(font);
    let upem = f32::from(font.units_per_em().max(1));
    let scale = SDF_EM_SIZE / upem;

    let mut buffer = Buffer::new();
    buffer.set_flags(BufferFlags::BEGINNING_OF_TEXT | BufferFlags::END_OF_TEXT);
    if let Some(pre) = full_text.get(..byte_range.start) {
        buffer.set_pre_context(pre);
    }
    if let Some(post) = full_text.get(byte_range.end..) {
        buffer.set_post_context(post);
    }
    for (rel_idx, ch) in slice.char_indices() {
        if is_bidi_isolate_or_override(ch) {
            continue;
        }
        let cluster = (cluster_base + byte_range.start + rel_idx) as u32;
        buffer.push(u32::from(ch), cluster);
    }
    if buffer.glyph_infos().is_empty() {
        return Vec::new();
    }

    buffer.set_direction(if rtl {
        Direction::RightToLeft
    } else {
        Direction::LeftToRight
    });
    buffer.guess_segment_properties();

    if harfrust::shape(&shaper, &mut buffer, ShapeOptions::new()).is_err() {
        return Vec::new();
    }

    let infos = buffer.glyph_infos();
    let positions = buffer.glyph_positions();
    infos
        .iter()
        .zip(positions.iter())
        .map(|(info, pos)| ShapedGlyph {
            face_id,
            glyph_id: info.glyph_id,
            cluster: info.cluster,
            x_advance: pos.x_advance as f32 * scale,
            y_advance: pos.y_advance as f32 * scale,
            x_offset: pos.x_offset as f32 * scale,
            y_offset: pos.y_offset as f32 * scale,
            rtl,
        })
        .collect()
}

/// Shapes a vertical top-to-bottom run with OpenType `vert` and `vrt2` features enabled.
#[cfg(feature = "font-shaping")]
pub fn shape_slice_with_font_vertical(
    font: &HbFont,
    face_id: u64,
    full_text: &str,
    byte_range: Range<usize>,
    cluster_base: usize,
) -> Vec<ShapedGlyph> {
    let Some(slice) = full_text.get(byte_range.clone()) else {
        return Vec::new();
    };
    if slice.is_empty() {
        return Vec::new();
    }

    let shaper = ShaperFont::new(font);
    let upem = f32::from(font.units_per_em().max(1));
    let scale = SDF_EM_SIZE / upem;

    let mut buffer = Buffer::new();
    buffer.set_flags(BufferFlags::BEGINNING_OF_TEXT | BufferFlags::END_OF_TEXT);
    if let Some(pre) = full_text.get(..byte_range.start) {
        buffer.set_pre_context(pre);
    }
    if let Some(post) = full_text.get(byte_range.end..) {
        buffer.set_post_context(post);
    }
    for (rel_idx, ch) in slice.char_indices() {
        if is_bidi_isolate_or_override(ch) {
            continue;
        }
        let cluster = (cluster_base + byte_range.start + rel_idx) as u32;
        buffer.push(u32::from(ch), cluster);
    }
    if buffer.glyph_infos().is_empty() {
        return Vec::new();
    }

    buffer.set_direction(Direction::TopToBottom);
    buffer.guess_segment_properties();

    let features = [
        Feature::new(Tag::new(b"vert"), 1, ..),
        Feature::new(Tag::new(b"vrt2"), 1, ..),
    ];
    if harfrust::shape(
        &shaper,
        &mut buffer,
        ShapeOptions::new().features(&features),
    )
    .is_err()
    {
        return Vec::new();
    }

    let infos = buffer.glyph_infos();
    let positions = buffer.glyph_positions();
    infos
        .iter()
        .zip(positions.iter())
        .map(|(info, pos)| {
            let y_advance = if pos.y_advance == 0 {
                -SDF_EM_SIZE
            } else {
                pos.y_advance as f32 * scale
            };
            ShapedGlyph {
                face_id,
                glyph_id: info.glyph_id,
                cluster: info.cluster,
                x_advance: pos.x_advance as f32 * scale,
                y_advance,
                x_offset: pos.x_offset as f32 * scale,
                y_offset: pos.y_offset as f32 * scale,
                rtl: false,
            }
        })
        .collect()
}

/// Shapes `text` vertically (`Direction::TopToBottom` with `vert`/`vrt2`) across newline-separated
/// paragraphs and per-character font fallback resolved by `resolve_face`.
#[cfg(feature = "font-shaping")]
pub fn shape_text_runs_vertical<F>(text: &str, mut resolve_face: F) -> Option<Vec<ShapedGlyph>>
where
    F: FnMut(char) -> Option<(u64, HbFont)>,
{
    if text.is_empty() {
        return None;
    }
    let mut output = Vec::new();
    let mut para_start = 0usize;
    for raw_para in text.split('\n') {
        let para_len = raw_para.len();
        if !raw_para.is_empty() {
            let mut face_runs: Vec<(u64, HbFont, Range<usize>)> = Vec::new();
            for (rel_idx, ch) in raw_para.char_indices() {
                if is_bidi_isolate_or_override(ch) {
                    continue;
                }
                let char_end = rel_idx + ch.len_utf8();
                if is_cluster_continuation(ch) {
                    if let Some((_, _, last_range)) = face_runs.last_mut() {
                        if last_range.end == rel_idx {
                            last_range.end = char_end;
                            continue;
                        }
                    }
                }
                let (face_id, font) = resolve_face(ch)?;
                if let Some((last_id, _, last_range)) = face_runs.last_mut() {
                    if *last_id == face_id && last_range.end == rel_idx {
                        last_range.end = char_end;
                        continue;
                    }
                }
                face_runs.push((face_id, font, rel_idx..char_end));
            }
            for (face_id, font, sub_range) in face_runs {
                let shaped =
                    shape_slice_with_font_vertical(&font, face_id, raw_para, sub_range, para_start);
                output.extend(shaped);
            }
        }
        para_start += para_len + 1;
    }
    (!output.is_empty()).then_some(output)
}

/// Shapes `text` across newline-separated paragraphs, BiDi visual runs, and per-character font
/// fallback resolved by `resolve_face`.
///
/// Returns `None` if `text` has no renderable characters or if any non-ignorable character fails
/// to resolve an OpenType [`HbFont`], allowing callers to fall back to `.pbf` glyph layout.
#[cfg(feature = "font-shaping")]
pub fn shape_text_runs<F>(text: &str, mut resolve_face: F) -> Option<Vec<ShapedGlyph>>
where
    F: FnMut(char) -> Option<(u64, HbFont)>,
{
    if text.is_empty() {
        return None;
    }

    let mut all_glyphs = Vec::new();
    let mut byte_cursor = 0usize;
    for line in text.split('\n') {
        if !line.is_empty() {
            let line_glyphs = shape_single_line(line, byte_cursor, &mut resolve_face)?;
            all_glyphs.extend(line_glyphs);
        }
        byte_cursor += line.len() + 1;
    }

    (!all_glyphs.is_empty()).then_some(all_glyphs)
}

#[cfg(feature = "font-shaping")]
fn shape_single_line<F>(
    line: &str,
    cluster_base: usize,
    resolve_face: &mut F,
) -> Option<Vec<ShapedGlyph>>
where
    F: FnMut(char) -> Option<(u64, HbFont)>,
{
    let bidi = BidiInfo::new(line, None);
    let mut line_glyphs = Vec::new();

    for para in &bidi.paragraphs {
        let (levels, visual_runs) = bidi.visual_runs(para, para.range.clone());
        for run in visual_runs {
            let rtl = levels.get(run.start).is_some_and(|lvl| lvl.is_rtl());
            let sub_runs = split_by_face(line, run, resolve_face)?;
            let ordered: Box<dyn Iterator<Item = (Range<usize>, u64, HbFont)>> = if rtl {
                Box::new(sub_runs.into_iter().rev())
            } else {
                Box::new(sub_runs.into_iter())
            };
            for (sub_range, face_id, font) in ordered {
                line_glyphs.extend(shape_slice_with_font(
                    face_id,
                    &font,
                    line,
                    sub_range,
                    cluster_base,
                    rtl,
                ));
            }
        }
    }

    Some(line_glyphs)
}

/// Partitions `line[run]` into contiguous `(byte_range, face_id, HbFont)` spans while keeping
/// combining marks, joiners (`ZWJ`/`ZWNJ`), and variation selectors attached to the preceding
/// base character's font face so grapheme clusters never split mid-syllable.
#[cfg(feature = "font-shaping")]
fn split_by_face<F>(
    line: &str,
    run: Range<usize>,
    resolve_face: &mut F,
) -> Option<Vec<(Range<usize>, u64, HbFont)>>
where
    F: FnMut(char) -> Option<(u64, HbFont)>,
{
    let slice = line.get(run.clone())?;
    let mut spans: Vec<(Range<usize>, u64, HbFont)> = Vec::new();

    for (rel_idx, ch) in slice.char_indices() {
        let start = run.start + rel_idx;
        let end = start + ch.len_utf8();

        if is_cluster_continuation(ch) || is_bidi_isolate_or_override(ch) {
            if let Some((last_range, _, _)) = spans.last_mut() {
                last_range.end = end;
                continue;
            }
            if is_bidi_isolate_or_override(ch) {
                continue;
            }
        }

        let (face_id, font) = resolve_face(ch)?;
        match spans.last_mut() {
            Some((last_range, last_id, _)) if *last_id == face_id && last_range.end == start => {
                last_range.end = end;
            }
            _ => spans.push((start..end, face_id, font)),
        }
    }

    Some(spans)
}

#[cfg(feature = "font-shaping")]
const CLUSTER_CONTINUATION_RANGES: &[(u32, u32)] = &[
    // Combining Diacritical Marks & Extended/Supplement/Symbols
    (0x0300, 0x036F),
    (0x0483, 0x0489),
    // Hebrew & Arabic harakat / tashkeel / Quranic marks
    (0x0591, 0x05BD),
    (0x05BF, 0x05BF),
    (0x05C1, 0x05C2),
    (0x05C4, 0x05C5),
    (0x05C7, 0x05C7),
    (0x0610, 0x061A),
    (0x064B, 0x065F),
    (0x0670, 0x0670),
    (0x06D6, 0x06ED),
    (0x08D3, 0x08FF),
    // Indic vowel signs, virama/halant, nukta, chandrabindu, visarga, Vedic marks (Devanagari..Sinhala)
    (0x0900, 0x0903),
    (0x093A, 0x094F),
    (0x0951, 0x0957),
    (0x0962, 0x0963),
    (0x0981, 0x0983),
    (0x09BC, 0x09BC),
    (0x09BE, 0x09CD),
    (0x09D7, 0x09D7),
    (0x09E2, 0x09E3),
    (0x0A01, 0x0A03),
    (0x0A3C, 0x0A3C),
    (0x0A3E, 0x0A4D),
    (0x0A51, 0x0A51),
    (0x0A70, 0x0A71),
    (0x0A75, 0x0A75),
    (0x0A81, 0x0A83),
    (0x0ABC, 0x0ABC),
    (0x0ABE, 0x0ACD),
    (0x0AE2, 0x0AE3),
    (0x0B01, 0x0B03),
    (0x0B3C, 0x0B3C),
    (0x0B3E, 0x0B57),
    (0x0B62, 0x0B63),
    (0x0B82, 0x0B82),
    (0x0BBE, 0x0BCD),
    (0x0BD7, 0x0BD7),
    (0x0C00, 0x0C04),
    (0x0C3C, 0x0C56),
    (0x0C62, 0x0C63),
    (0x0C81, 0x0C83),
    (0x0CBC, 0x0CBC),
    (0x0CBE, 0x0CCD),
    (0x0CD5, 0x0CD6),
    (0x0CE2, 0x0CE3),
    (0x0D00, 0x0D03),
    (0x0D3B, 0x0D4D),
    (0x0D57, 0x0D57),
    (0x0D62, 0x0D63),
    (0x0D81, 0x0D83),
    (0x0DCA, 0x0DDF),
    (0x0DF2, 0x0DF3),
    // Thai, Lao, Tibetan, Myanmar, Khmer vowel signs & tone/killer marks
    (0x0E31, 0x0E31),
    (0x0E34, 0x0E3A),
    (0x0E47, 0x0E4E),
    (0x0EB1, 0x0EB1),
    (0x0EB4, 0x0EBC),
    (0x0EC8, 0x0ECD),
    (0x0F18, 0x0F19),
    (0x0F35, 0x0F35),
    (0x0F37, 0x0F37),
    (0x0F39, 0x0F39),
    (0x0F3E, 0x0F3F),
    (0x0F71, 0x0F84),
    (0x0F86, 0x0F87),
    (0x0F8D, 0x0FBC),
    (0x102B, 0x103E),
    (0x1056, 0x1059),
    (0x105E, 0x1060),
    (0x1062, 0x1064),
    (0x1067, 0x106D),
    (0x1071, 0x1074),
    (0x1082, 0x108D),
    (0x108F, 0x108F),
    (0x109A, 0x109D),
    (0x17B4, 0x17D3),
    (0x17DD, 0x17DD),
    (0x1CD0, 0x1DFF),
    // ZWNJ & ZWJ
    (0x200C, 0x200D),
    // Combining Diacritical Marks for Symbols & Variation Selectors
    (0x20D0, 0x20FF),
    (0xFE00, 0xFE0F),
    (0xFE20, 0xFE2F),
    (0xE0100, 0xE01EF),
];

/// Returns `true` for Unicode combining marks, joiners, and variation selectors that must stay in
/// the same shaping run as their preceding base character.
#[cfg(feature = "font-shaping")]
fn is_cluster_continuation(ch: char) -> bool {
    let cp = u32::from(ch);
    CLUSTER_CONTINUATION_RANGES
        .iter()
        .any(|&(lo, hi)| cp >= lo && cp <= hi)
}

#[cfg(feature = "font-shaping")]
fn is_bidi_isolate_or_override(ch: char) -> bool {
    matches!(
        ch,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}
