//! What each character of a label is drawn with, and how wide its lines are.
use std::{collections::HashMap, ops::Range};

use super::super::layout::CollectedSymbol;
use crate::{
    sdf::assets::{AtlasEntry, SymbolAtlas},
    style::layer::SymbolPaint,
};

/// What one character of a label is drawn with.
#[derive(Clone)]
pub(super) struct CharStyle<'a> {
    pub(super) scale: f32,
    pub(super) color: Option<[f32; 4]>,
    pub(super) glyphs: &'a HashMap<u32, AtlasEntry>,
    /// The image the character is, with its size in layout pixels.
    pub(super) image: Option<(&'a AtlasEntry, [f32; 2], [f32; 2])>,
}

impl CharStyle<'_> {
    pub(super) fn advance(&self, c: char) -> f32 {
        if let Some((_, size, _)) = self.image {
            return size[0];
        }
        self.glyphs
            .get(&(c as u32))
            .map_or(0.0, |glyph| glyph.metrics[2] * self.scale)
    }

    /// Whether the character takes room in the line.
    pub(super) fn is_drawn(&self, c: char) -> bool {
        self.image.is_some() || self.glyphs.contains_key(&(c as u32))
    }
}

/// The glyphs a character is drawn from: its section's font where the atlas holds it, else
/// the layer's.
pub(super) fn glyphs_for<'a>(
    atlas: &'a SymbolAtlas,
    paint: &SymbolPaint,
    font: Option<&str>,
) -> Option<&'a HashMap<u32, AtlasEntry>> {
    font.and_then(|font| atlas.glyphs.get(font))
        .or_else(|| atlas.glyphs.get(&paint.font_stack()))
        .or_else(|| atlas.glyphs.values().next())
        // A label of images alone needs no glyphs.
        .or(Some(&NO_GLYPHS))
}

static NO_GLYPHS: std::sync::LazyLock<HashMap<u32, AtlasEntry>> =
    std::sync::LazyLock::new(HashMap::new);

/// The style of each of the `count` characters of the label: its section's scale, colour and
/// font, or the layer's own for text that is all one section.
pub(super) fn char_styles<'a>(
    paint: &SymbolPaint,
    symbol: &CollectedSymbol,
    zoom: f64,
    atlas: &'a SymbolAtlas,
    count: usize,
) -> Option<Vec<CharStyle<'a>>> {
    let default = glyphs_for(atlas, paint, None)?;
    let sections = paint.label_sections(&symbol.properties, zoom);
    let text_size = paint
        .text_size
        .as_ref()
        .and_then(|value| value.evaluate_for(&symbol.properties, zoom))
        .unwrap_or(16.0)
        .max(1.0);
    let mut styles = Vec::with_capacity(count);
    for section in &sections {
        let glyphs = glyphs_for(atlas, paint, section.font.as_deref()).unwrap_or(default);
        let scale = section.scale.unwrap_or(1.0).max(0.0);
        // An image keeps its own size whatever the text size, which the layout scales by.
        let image = section
            .image
            .as_ref()
            .and_then(|name| atlas.icons.get(name))
            .map(|entry| {
                let layout_scale = scale * 24.0 / text_size;
                let factor = layout_scale / entry.metrics[3];
                (
                    entry,
                    [entry.rect[2] as f32 * factor, entry.rect[3] as f32 * factor],
                    // An image hangs three layout pixels (scaled) below its line's top, like a glyph's
                    // border, and one in from its pen position.
                    [layout_scale, 3.0 * layout_scale],
                )
            });
        styles.extend(std::iter::repeat_n(
            CharStyle {
                scale,
                color: section.color,
                glyphs,
                image,
            },
            section.length,
        ));
    }
    styles.resize(
        count,
        CharStyle {
            scale: 1.0,
            color: None,
            glyphs: default,
            image: None,
        },
    );
    styles.truncate(count);
    Some(styles)
}

/// Width of a line of glyphs without the spacing after the last one. Tight letter spacing
/// makes it negative, which justifies the line as GL JS does; a line without glyphs has none.
pub(super) fn line_width(
    chars: &[char],
    styles: &[CharStyle<'_>],
    line: Range<usize>,
    spacing: f32,
) -> f32 {
    let advances: Vec<f32> = line
        .filter(|index| styles[*index].is_drawn(chars[*index]))
        .map(|index| styles[index].advance(chars[index]) + spacing)
        .collect();
    if advances.is_empty() {
        0.0
    } else {
        advances.iter().sum::<f32>() - spacing
    }
}
