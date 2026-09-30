//! Line wrapping and glyph metrics around a symbol anchor.
use std::collections::HashMap;

use lyon::tessellation::VertexBuffers;

use super::layout::{anchor_fractions, offset, quad, CollectedSymbol};
use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::assets::{AtlasEntry, SymbolAtlas},
    style::layer::SymbolPaint,
};

/// The wrapped lines of a label and where they sit around the anchor, in layout pixels.
struct Block<'a> {
    glyphs: &'a HashMap<u32, AtlasEntry>,
    lines: Vec<String>,
    spacing: f32,
    line_height: f32,
    max_line: f32,
    height: f32,
    fractions: [f32; 2],
    justify: f32,
    offset: [f32; 2],
}

fn block<'a>(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &'a SymbolAtlas,
) -> Option<Block<'a>> {
    let text = paint.label(&symbol.properties, zoom)?;
    let glyphs = atlas
        .glyphs
        .get(&paint.font_stack())
        .or_else(|| atlas.glyphs.values().next())?;
    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    let max_width = paint.number("text-max-width", &symbol.properties, zoom, 10.0) * 24.0;
    // Text along a line runs the whole line: it is never wrapped.
    let lines = if super::is_line_placed(paint) {
        vec![text]
    } else {
        wrap(&text, max_width, glyphs, spacing)
    };
    let line_height = paint.number("text-line-height", &symbol.properties, zoom, 1.2) * 24.0;
    let max_line = lines
        .iter()
        .map(|line| line_width(line, glyphs, spacing))
        .fold(0.0, f32::max);
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
    Some(Block {
        glyphs,
        // Every line takes a full line height, with the glyphs centred in it.
        height: lines.len() as f32 * line_height,
        lines,
        spacing,
        line_height,
        max_line,
        fractions,
        justify,
        offset: offset(paint, "text-offset", 24.0),
    })
}

/// The label's layout box `[left, top, right, bottom]` around the anchor, in layout pixels.
pub(super) fn extent(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
) -> Option<[f32; 4]> {
    let block = block(symbol, paint, zoom, atlas)?;
    if block.max_line <= 0.0 {
        return None;
    }
    let left = -block.max_line * block.fractions[0] + block.offset[0];
    let top = -block.height * block.fractions[1] + block.offset[1];
    Some([left, top, left + block.max_line, top + block.height])
}

pub(super) fn append(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
) -> Vec<f32> {
    let mut centres = Vec::new();
    let Some(Block {
        glyphs,
        lines,
        spacing,
        line_height,
        max_line,
        height,
        fractions,
        justify,
        offset,
    }) = block(symbol, paint, zoom, atlas)
    else {
        return centres;
    };
    let width = |text: &str| line_width(text, glyphs, spacing);
    let half_leading = (line_height - 24.0) / 2.0;
    let elevation = if paint.uses_shared_height() {
        0.0
    } else {
        paint.height_offset("text", &symbol.properties, zoom)
    };
    let shift = crate::sdf::translation::tile_translation(paint, "text");
    let anchor = geo_types::Point::new(symbol.anchor.x() + shift[0], symbol.anchor.y() + shift[1]);
    let follows_line = crate::sdf::paint::text_follows_line(paint, zoom);
    let rotation = paint
        .number("text-rotate", &symbol.properties, zoom, 0.0)
        .to_radians();
    for (row, line) in lines.iter().enumerate() {
        let baseline =
            -height * fractions[1] + half_leading - 5.0 + row as f32 * line_height + offset[1];
        let mut pen = -max_line * fractions[0] + (max_line - width(line)) * justify + offset[0];
        for c in line.chars() {
            let Some(glyph) = glyphs.get(&(c as u32)) else {
                continue;
            };
            if glyph.rect[2] > 0 && glyph.rect[3] > 0 {
                let (x, y) = (pen + glyph.metrics[0], baseline - glyph.metrics[1]);
                quad(
                    buffer,
                    anchor,
                    [x, y, x + glyph.rect[2] as f32, y + glyph.rect[3] as f32],
                    glyph,
                    elevation,
                    symbol.angle,
                    rotation,
                );
                if symbol.line.is_some() && follows_line {
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
