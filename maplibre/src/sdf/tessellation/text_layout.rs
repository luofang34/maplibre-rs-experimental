//! Line wrapping and glyph metrics around a symbol anchor.
use std::collections::HashMap;

use lyon::tessellation::VertexBuffers;

use super::layout::{anchor_fractions, offset, quad, CollectedSymbol};
use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::assets::{AtlasEntry, SymbolAtlas},
    style::{expression::FeatureProperties, layer::SymbolPaint},
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
        glyphs,
        // Every line takes a full line height, with the glyphs centred in it.
        height: lines.len() as f32 * line_height,
        lines,
        spacing,
        line_height,
        max_line,
        fractions,
        justify,
        offset: anchored_offset(paint, &anchors, &anchor, (&symbol.properties, zoom)),
    })
}

/// The anchors `text-variable-anchor` lists, best first; empty without the property.
pub(super) fn variable_anchors(paint: &SymbolPaint) -> Vec<String> {
    paint
        .properties
        .get("text-variable-anchor")
        .and_then(|value| value.as_array())
        .map(|anchors| {
            anchors
                .iter()
                .filter_map(|anchor| anchor.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The text offset in layout pixels for an anchor: `text-radial-offset` pushes the text away
/// from the point along the anchor's direction, and `text-offset` applies as written when no
/// radial offset is set or the anchor is fixed.
fn anchored_offset(
    paint: &SymbolPaint,
    variable: &[String],
    anchor: &str,
    (properties, zoom): (&crate::style::expression::FeatureProperties, f64),
) -> [f32; 2] {
    if variable.is_empty() || !paint.properties.contains_key("text-radial-offset") {
        return offset(paint, "text-offset", 24.0, (properties, zoom));
    }
    let radius = paint.number("text-radial-offset", properties, zoom, 0.0) * 24.0;
    let diagonal = radius / std::f32::consts::SQRT_2;
    let x = match anchor {
        "top-right" | "bottom-right" => -diagonal,
        "top-left" | "bottom-left" => diagonal,
        "left" => radius,
        "right" => -radius,
        _ => 0.0,
    };
    let y = match anchor {
        "top-right" | "top-left" => diagonal,
        "bottom-right" | "bottom-left" => -diagonal,
        "top" => radius,
        "bottom" => -radius,
        _ => 0.0,
    };
    [x, y]
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
    };
    let Some(block) = block(symbol, paint, zoom, atlas) else {
        return laid;
    };
    let (values, anchor_sets) = justifications(paint, block.justify, &symbol.properties, zoom);
    for justify in &values {
        let start = buffer.indices.len();
        let centres = glyph_pass(symbol, paint, zoom, &block, *justify, buffer);
        laid.centres.extend(centres);
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
) -> Vec<f32> {
    let Block {
        glyphs,
        lines,
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
    let width = |text: &str| line_width(text, glyphs, spacing);
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

/// A place the text may break, with the least raggedness of any way to reach it.
struct Break {
    index: usize,
    x: f32,
    prior: Option<usize>,
    badness: f32,
}

fn is_whitespace(c: char) -> bool {
    c.is_whitespace()
}

/// Characters after which a line may break.
fn breakable(c: char) -> bool {
    matches!(
        c,
        '\n' | ' '
            | '&'
            | ')'
            | '+'
            | '-'
            | '/'
            | '\u{ad}'
            | '\u{b7}'
            | '\u{200b}'
            | '\u{2010}'
            | '\u{2013}'
            | '\u{2027}'
    )
}

/// Ideographic characters, which may break between any two of them.
fn allows_ideographic_breaking(c: char) -> bool {
    matches!(
        u32::from(c),
        0x2E80..=0x2FDF | 0x3000..=0x30FF | 0x3100..=0x9FFF | 0xA000..=0xA4CF | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x2FA1F
    )
}

fn break_penalty(c: char, next: char, penalizable_ideographic_break: bool) -> f32 {
    let mut penalty = 0.0;
    if c == '\n' {
        penalty -= 10_000.0;
    }
    if penalizable_ideographic_break {
        penalty += 150.0;
    }
    if c == '(' || c == '\u{ff08}' {
        penalty += 50.0;
    }
    if next == ')' || next == '\u{ff09}' {
        penalty += 50.0;
    }
    penalty
}

fn badness(line_width: f32, target: f32, penalty: f32, last: bool) -> f32 {
    let raggedness = (line_width - target).powi(2);
    if last {
        // Final lines shorter than the average are favoured over longer ones.
        if line_width < target {
            raggedness / 2.0
        } else {
            raggedness * 2.0
        }
    } else {
        raggedness + penalty.abs() * penalty
    }
}

fn evaluate_break(
    (index, x): (usize, f32),
    (target, penalty, last): (f32, f32, bool),
    potential: &[Break],
) -> Break {
    let mut prior = None;
    let mut best = badness(x, target, penalty, last);
    for (at, candidate) in potential.iter().enumerate() {
        let total = badness(x - candidate.x, target, penalty, last) + candidate.badness;
        if total <= best {
            prior = Some(at);
            best = total;
        }
    }
    Break {
        index,
        x,
        prior,
        badness: best,
    }
}

/// Breaks the text into lines the way GL JS does: at the places that leave the lines closest
/// to the same width, with the width a line may run to only steering how many lines there are.
fn wrap(
    text: &str,
    max_width: f32,
    glyphs: &HashMap<u32, AtlasEntry>,
    spacing: f32,
) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let advance = |c: char| {
        glyphs
            .get(&(c as u32))
            .map_or(0.0, |glyph| glyph.metrics[2] + spacing)
    };
    let total: f32 = chars.iter().map(|c| advance(*c)).sum();
    let target = total / (total / max_width).ceil().max(1.0);
    let has_zero_width_space = chars.contains(&'\u{200b}');
    let mut potential: Vec<Break> = Vec::new();
    let mut x = 0.0;
    for (index, c) in chars.iter().copied().enumerate() {
        if !is_whitespace(c) {
            x += advance(c);
        }
        let Some(next) = chars.get(index + 1).copied() else {
            continue;
        };
        let ideographic = allows_ideographic_breaking(c);
        if breakable(c) || ideographic || (index + 2 < chars.len() && next == '(') {
            let penalty = break_penalty(c, next, ideographic && has_zero_width_space);
            let found = evaluate_break((index + 1, x), (target, penalty, false), &potential);
            potential.push(found);
        }
    }
    let last = evaluate_break((chars.len(), x), (target, 0.0, true), &potential);
    let mut ends = vec![last.index];
    let mut prior = last.prior;
    while let Some(at) = prior {
        ends.push(potential[at].index);
        prior = potential[at].prior;
    }
    ends.reverse();
    let mut lines = Vec::new();
    let mut start = 0;
    for end in ends {
        lines.push(
            chars[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_owned(),
        );
        start = end;
    }
    if start < chars.len() {
        lines.push(chars[start..].iter().collect::<String>().trim().to_owned());
    }
    lines
}

#[cfg(test)]
#[path = "text_layout/tests.rs"]
mod tests;
