//! Line wrapping and glyph metrics around a symbol anchor.
use std::collections::HashMap;

use lyon::tessellation::VertexBuffers;

use super::layout::{anchor_fractions, offset, quad, CollectedSymbol};
use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::assets::{AtlasEntry, SymbolAtlas},
    style::layer::SymbolPaint,
};

pub(super) fn append(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
) -> Vec<f32> {
    let mut centres = Vec::new();
    let Some(text) = paint.label(&symbol.properties, zoom) else {
        return centres;
    };
    let Some(glyphs) = atlas
        .glyphs
        .get(&paint.font_stack())
        .or_else(|| atlas.glyphs.values().next())
    else {
        return centres;
    };
    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    let width = |text: &str| line_width(text, glyphs, spacing);
    let max_width = paint.number("text-max-width", &symbol.properties, zoom, 10.0) * 24.0;
    // Text along a line runs the whole line: it is never wrapped.
    let lines = if super::is_line_placed(paint) {
        vec![text.clone()]
    } else {
        wrap(&text, max_width, glyphs, spacing)
    };
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
    let elevation = if paint.uses_shared_height() {
        0.0
    } else {
        paint.height_offset("text", &symbol.properties, zoom)
    };
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
                if symbol.line.is_some() {
                    // A glyph along a line is placed by its centre: the vertex carries where
                    // that centre lies in the straight layout, in 1/32 pixel.
                    let centre = pen + glyph.metrics[2] / 2.0;
                    let first = buffer.vertices.len() - 4;
                    for vertex in &mut buffer.vertices[first..] {
                        vertex.a_pixeloffset[0] = (centre * 32.0).round() as i32;
                    }
                    centres.push(centre);
                }
            }
            pen += glyph.metrics[2] + spacing;
        }
    }
    centres
}

/// Width in layout pixels of the label on one line, without wrapping; zero without glyphs.
pub(super) fn unwrapped_width(
    paint: &SymbolPaint,
    symbol: &CollectedSymbol,
    zoom: f64,
    atlas: &SymbolAtlas,
) -> f32 {
    let Some(text) = paint.label(&symbol.properties, zoom) else {
        return 0.0;
    };
    let Some(glyphs) = atlas
        .glyphs
        .get(&paint.font_stack())
        .or_else(|| atlas.glyphs.values().next())
    else {
        return 0.0;
    };
    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    line_width(&text, glyphs, spacing)
}

fn line_width(text: &str, glyphs: &HashMap<u32, AtlasEntry>, spacing: f32) -> f32 {
    (text
        .chars()
        .filter_map(|c| glyphs.get(&(c as u32)))
        .map(|glyph| glyph.metrics[2] + spacing)
        .sum::<f32>()
        - spacing)
        .max(0.0)
}

fn wrap(
    text: &str,
    max_width: f32,
    glyphs: &HashMap<u32, AtlasEntry>,
    spacing: f32,
) -> Vec<String> {
    let measure = |text: &str| line_width(text, glyphs, spacing);
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

#[cfg(test)]
#[path = "text_layout/tests.rs"]
mod tests;
