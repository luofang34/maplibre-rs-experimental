//! Line wrapping and glyph metrics around a symbol anchor.
use super::layout::{anchor_fractions, offset, quad, CollectedSymbol};
use crate::{
    render::shaders::ShaderSymbolVertexNew,
    sdf::assets::{AtlasEntry, SymbolAtlas},
    style::layer::SymbolPaint,
};
use lyon::tessellation::VertexBuffers;
use std::collections::HashMap;

pub(super) fn append(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
    buffer: &mut VertexBuffers<ShaderSymbolVertexNew, u32>,
) {
    let Some(text) = paint.label(&symbol.properties, zoom) else {
        return;
    };
    let Some(glyphs) = atlas
        .glyphs
        .get(&paint.font_stack())
        .or_else(|| atlas.glyphs.values().next())
    else {
        return;
    };
    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    let width = |text: &str| {
        text.chars()
            .filter_map(|c| glyphs.get(&(c as u32)))
            .map(|glyph| glyph.metrics[2] + spacing)
            .sum::<f32>()
    };
    let max_width = paint.number("text-max-width", &symbol.properties, zoom, 10.0) * 24.0;
    let lines = wrap(&text, max_width, glyphs, spacing);
    let line_height = paint.number("text-line-height", &symbol.properties, zoom, 1.2) * 24.0;
    let max_line = lines.iter().map(|line| width(line)).fold(0.0, f32::max);
    let height = 24.0 + lines.len().saturating_sub(1) as f32 * line_height;
    let fractions = anchor_fractions(
        &paint
            .text("text-anchor", &symbol.properties, zoom)
            .unwrap_or_else(|| "center".into()),
    );
    let justify = paint
        .text("text-justify", &symbol.properties, zoom)
        .unwrap_or_else(|| "center".into());
    let justify = match justify.as_str() {
        "left" => 0.0,
        "right" => 1.0,
        "auto" => fractions[0],
        _ => 0.5,
    };
    let offset = offset(paint, "text-offset", 24.0);
    let elevation = paint.number("text-height-offset", &symbol.properties, zoom, 0.0);
    for (row, line) in lines.iter().enumerate() {
        let baseline = -height * fractions[1] - 5.0 + row as f32 * line_height + offset[1];
        let mut pen = -max_line * fractions[0] + (max_line - width(line)) * justify + offset[0];
        for c in line.chars() {
            let Some(glyph) = glyphs.get(&(c as u32)) else {
                continue;
            };
            if glyph.rect[2] > 0 && glyph.rect[3] > 0 {
                let (x, y) = (pen + glyph.metrics[0], baseline - glyph.metrics[1]);
                quad(
                    buffer,
                    symbol.anchor,
                    [x, y, x + glyph.rect[2] as f32, y + glyph.rect[3] as f32],
                    glyph,
                    elevation,
                    symbol.angle,
                );
            }
            pen += glyph.metrics[2] + spacing;
        }
    }
}

fn wrap(
    text: &str,
    max_width: f32,
    glyphs: &HashMap<u32, AtlasEntry>,
    spacing: f32,
) -> Vec<String> {
    let measure = |text: &str| {
        text.chars()
            .filter_map(|c| glyphs.get(&(c as u32)))
            .map(|g| g.metrics[2] + spacing)
            .sum::<f32>()
    };
    let mut result = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if max_width > 0.0 && !line.is_empty() && measure(&candidate) > max_width {
                result.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        result.push(line);
    }
    result
}
