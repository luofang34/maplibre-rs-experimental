//! Line wrapping and glyph metrics around a symbol anchor.
use std::{collections::HashMap, ops::Range};

use lyon::tessellation::VertexBuffers;

use super::{
    layout::{anchor_fractions, quad, CollectedSymbol},
    line_break::wrap,
    text_offset::{anchored_offset, variable_anchors},
};
use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::assets::{AtlasEntry, SymbolAtlas},
    style::{expression::FeatureProperties, layer::SymbolPaint},
};

/// Index range of glyphs a `format` section colours, with the colour.
pub(super) type TextColorRun = (Range<usize>, [f32; 4]);

/// What one character of a label is drawn with.
#[derive(Clone)]
struct CharStyle<'a> {
    scale: f32,
    color: Option<[f32; 4]>,
    glyphs: &'a HashMap<u32, AtlasEntry>,
}

impl CharStyle<'_> {
    fn advance(&self, c: char) -> f32 {
        self.glyphs
            .get(&(c as u32))
            .map_or(0.0, |glyph| glyph.metrics[2] * self.scale)
    }
}

/// The wrapped lines of a label and where they sit around the anchor, in layout pixels.
struct Block<'a> {
    chars: Vec<char>,
    styles: Vec<CharStyle<'a>>,
    lines: Vec<Range<usize>>,
    /// The largest scale of any character in each line, which sets the line's height.
    line_scales: Vec<f32>,
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
    let chars: Vec<char> = text.chars().collect();
    let styles = char_styles(paint, symbol, zoom, atlas, chars.len())?;
    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    let max_width = paint.number("text-max-width", &symbol.properties, zoom, 10.0) * 24.0;
    // Text along a line runs the whole line: it is never wrapped.
    let lines = if super::is_line_placed(paint) {
        std::iter::once(0..chars.len()).collect()
    } else {
        wrap(&chars, max_width, &|index| {
            styles[index].advance(chars[index]) + spacing
        })
    };
    let line_height = paint.number("text-line-height", &symbol.properties, zoom, 1.2) * 24.0;
    let max_line = lines
        .iter()
        .map(|line| line_width(&chars, &styles, line.clone(), spacing))
        .fold(0.0, f32::max);
    let line_scales: Vec<f32> = lines
        .iter()
        .map(|line| {
            styles[line.clone()]
                .iter()
                .map(|style| style.scale)
                .fold(1.0, f32::max)
        })
        .collect();
    let anchors = variable_anchors(paint);
    let anchor = anchors.first().cloned().unwrap_or_else(|| {
        paint
            .text("text-anchor", &symbol.properties, zoom)
            .unwrap_or_else(|| "center".into())
    });
    let fractions = anchor_fractions(&anchor);
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
        // Every line takes a full line height, with the glyphs centred in it.
        height: line_scales.iter().sum::<f32>() * line_height,
        chars,
        styles,
        lines,
        line_scales,
        spacing,
        line_height,
        max_line,
        fractions,
        justify,
        offset: anchored_offset(paint, &anchors, &anchor, (&symbol.properties, zoom)),
    })
}

/// The glyphs a character is drawn from: its section's font where the atlas holds it, else
/// the layer's.
fn glyphs_for<'a>(
    atlas: &'a SymbolAtlas,
    paint: &SymbolPaint,
    font: Option<&str>,
) -> Option<&'a HashMap<u32, AtlasEntry>> {
    font.and_then(|font| atlas.glyphs.get(font))
        .or_else(|| atlas.glyphs.get(&paint.font_stack()))
        .or_else(|| atlas.glyphs.values().next())
}

/// The style of each of the `count` characters of the label: its section's scale, colour and
/// font, or the layer's own for text that is all one section.
fn char_styles<'a>(
    paint: &SymbolPaint,
    symbol: &CollectedSymbol,
    zoom: f64,
    atlas: &'a SymbolAtlas,
    count: usize,
) -> Option<Vec<CharStyle<'a>>> {
    let default = glyphs_for(atlas, paint, None)?;
    let sections = paint.label_sections(&symbol.properties, zoom);
    let mut styles = Vec::with_capacity(count);
    for section in &sections {
        let glyphs = glyphs_for(atlas, paint, section.font.as_deref()).unwrap_or(default);
        styles.extend(std::iter::repeat_n(
            CharStyle {
                scale: section.scale.unwrap_or(1.0).max(0.0),
                color: section.color,
                glyphs,
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
        },
    );
    styles.truncate(count);
    Some(styles)
}

/// How far the label moves, in layout pixels, when it takes each of its variable anchors
/// instead of the first; empty unless the label has several.
pub(super) fn variable_shifts(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
) -> Vec<[f32; 2]> {
    let anchors = variable_anchors(paint);
    if anchors.len() < 2 {
        return Vec::new();
    }
    let Some(block) = block(symbol, paint, zoom, atlas) else {
        return Vec::new();
    };
    let corner = |anchor: &str| {
        let fractions = anchor_fractions(anchor);
        let offset = anchored_offset(paint, &anchors, anchor, (&symbol.properties, zoom));
        [
            -block.max_line * fractions[0] + offset[0],
            -block.height * fractions[1] + offset[1],
        ]
    };
    let first = corner(&anchors[0]);
    anchors
        .iter()
        .map(|anchor| {
            let corner = corner(anchor);
            [corner[0] - first[0], corner[1] - first[1]]
        })
        .collect()
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

/// The glyph quads of a label, and for text along a line the centre of each glyph; text that
/// justifies differently for each of its variable anchors is laid out once per justification.
pub(super) struct Laid {
    pub centres: Vec<f32>,
    /// Index range of each justification's glyphs; empty when the text has just one.
    pub sets: Vec<std::ops::Range<usize>>,
    /// Which of `sets` each variable anchor shows.
    pub anchor_sets: Vec<u8>,
    /// Index ranges of glyphs whose section gives them a colour of their own.
    pub colors: Vec<TextColorRun>,
}

/// The justifications the label lays out, one per distinct value its anchors call for under
/// `text-justify: auto`, and the one each anchor takes.
fn justifications(
    paint: &SymbolPaint,
    block_justify: f32,
    properties: &FeatureProperties,
    zoom: f64,
) -> (Vec<f32>, Vec<u8>) {
    let anchors = variable_anchors(paint);
    let auto = paint.text("text-justify", properties, zoom).as_deref() == Some("auto");
    if !auto || anchors.len() < 2 {
        return (vec![block_justify], Vec::new());
    }
    let mut values: Vec<f32> = Vec::new();
    let taken = anchors
        .iter()
        .map(|anchor| {
            let value = anchor_fractions(anchor)[0];
            let at = values.iter().position(|v| *v == value).unwrap_or_else(|| {
                values.push(value);
                values.len() - 1
            });
            at as u8
        })
        .collect();
    (values, taken)
}

pub(super) fn append(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
) -> Laid {
    let mut laid = Laid {
        centres: Vec::new(),
        sets: Vec::new(),
        anchor_sets: Vec::new(),
        colors: Vec::new(),
    };
    let Some(block) = block(symbol, paint, zoom, atlas) else {
        return laid;
    };
    let (values, anchor_sets) = justifications(paint, block.justify, &symbol.properties, zoom);
    for justify in &values {
        let start = buffer.indices.len();
        let (centres, colors) = glyph_pass(symbol, paint, zoom, &block, *justify, buffer);
        laid.centres.extend(centres);
        laid.colors.extend(colors);
        laid.sets.push(start..buffer.indices.len());
    }
    if values.len() > 1 {
        laid.anchor_sets = anchor_sets;
    } else {
        laid.sets.clear();
    }
    laid
}

fn glyph_pass(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    block: &Block<'_>,
    justify: f32,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
) -> (Vec<f32>, Vec<TextColorRun>) {
    let Block {
        chars,
        styles,
        lines,
        line_scales,
        spacing,
        line_height,
        max_line,
        height,
        fractions,
        offset,
        ..
    } = block;
    let (spacing, line_height, max_line, height) = (*spacing, *line_height, *max_line, *height);
    let mut centres = Vec::new();
    let mut colors: Vec<TextColorRun> = Vec::new();
    let half_leading = (line_height - 24.0) / 2.0;
    let elevation = if paint.uses_shared_height() {
        0.0
    } else {
        paint.height_offset("text", &symbol.properties, zoom)
    };
    let shift = crate::sdf::translation::tile_translation(paint, "text", zoom);
    let anchor = geo_types::Point::new(symbol.anchor.x() + shift[0], symbol.anchor.y() + shift[1]);
    let follows_line = crate::sdf::paint::text_follows_line(paint, zoom);
    let rotation = paint
        .number("text-rotate", &symbol.properties, zoom, 0.0)
        .to_radians();
    let mut line_top = 0.0;
    for (line, largest) in lines.iter().zip(line_scales) {
        let baseline = -height * fractions[1] + half_leading * largest - 5.0 + line_top + offset[1];
        line_top += line_height * largest;
        let width = line_width(chars, styles, line.clone(), spacing);
        let mut pen = -max_line * fractions[0] + (max_line - width) * justify + offset[0];
        for index in line.clone() {
            let style = &styles[index];
            let Some(glyph) = style.glyphs.get(&(chars[index] as u32)) else {
                continue;
            };
            let scale = style.scale;
            if glyph.rect[2] > 0 && glyph.rect[3] > 0 {
                // Smaller glyphs of a line sit on its bottom, where the largest one does.
                let x = pen + glyph.metrics[0] * scale;
                let y = baseline - glyph.metrics[1] * scale + (largest - scale) * 24.0;
                let first_index = buffer.indices.len();
                quad(
                    buffer,
                    anchor,
                    [
                        x,
                        y,
                        x + glyph.rect[2] as f32 * scale,
                        y + glyph.rect[3] as f32 * scale,
                    ],
                    glyph,
                    elevation,
                    symbol.angle,
                    rotation,
                );
                if let Some(color) = style.color {
                    match colors.last_mut() {
                        Some((range, last)) if *last == color && range.end == first_index => {
                            range.end = buffer.indices.len();
                        }
                        _ => colors.push((first_index..buffer.indices.len(), color)),
                    }
                }
                if symbol.line.is_some() && follows_line {
                    // A glyph along a line is placed by its centre: the vertex carries where
                    // that centre lies in the straight layout, in 1/32 pixel.
                    let centre = pen + glyph.metrics[2] * scale / 2.0;
                    let first = buffer.vertices.len() - 4;
                    for vertex in &mut buffer.vertices[first..] {
                        vertex.a_pixeloffset[0] = (centre * 32.0).round() as i32;
                    }
                    centres.push(centre);
                }
            }
            pen += glyph.metrics[2] * scale + spacing;
        }
    }
    (centres, colors)
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
    let chars: Vec<char> = text.chars().collect();
    let Some(styles) = char_styles(paint, symbol, zoom, atlas, chars.len()) else {
        return 0.0;
    };
    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    line_width(&chars, &styles, 0..chars.len(), spacing)
}

/// Width of a line of glyphs without the spacing after the last one. Tight letter spacing
/// makes it negative, which justifies the line as GL JS does; a line without glyphs has none.
fn line_width(chars: &[char], styles: &[CharStyle<'_>], line: Range<usize>, spacing: f32) -> f32 {
    let advances: Vec<f32> = line
        .filter(|index| styles[*index].glyphs.contains_key(&(chars[*index] as u32)))
        .map(|index| styles[index].advance(chars[index]) + spacing)
        .collect();
    if advances.is_empty() {
        0.0
    } else {
        advances.iter().sum::<f32>() - spacing
    }
}

#[cfg(test)]
#[path = "text_layout/tests.rs"]
mod tests;
