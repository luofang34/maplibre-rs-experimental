//! Line wrapping and glyph metrics around a symbol anchor.
use std::ops::Range;

use lyon::tessellation::VertexBuffers;

mod styles;

use styles::{char_styles, line_width, CharStyle};

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

/// The wrapped lines of a label and where they sit around the anchor, in layout pixels.
struct Block<'a> {
    chars: Vec<char>,
    styles: Vec<CharStyle<'a>>,
    lines: Vec<Range<usize>>,
    /// The largest scale of any character in each line, which sets the line's height.
    line_scales: Vec<f32>,
    /// How far each line grows past its height to hold an image taller than its em box.
    line_extras: Vec<f32>,
    /// The height of the content of each line: its em box, or its tallest image.
    line_contents: Vec<f32>,
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
    let tallest_image = |line: &Range<usize>| {
        styles[line.clone()]
            .iter()
            .filter_map(|style| style.image.map(|(_, size, _)| size[1]))
            .fold(0.0, f32::max)
    };
    let line_contents: Vec<f32> = lines
        .iter()
        .zip(&line_scales)
        .map(|(line, largest)| (largest * 24.0).max(tallest_image(line)))
        .collect();
    let line_extras: Vec<f32> = lines
        .iter()
        .zip(&line_scales)
        .map(|(line, largest)| (tallest_image(line) - largest * 24.0).max(0.0))
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
        height: line_scales.iter().sum::<f32>() * line_height + line_extras.iter().sum::<f32>(),
        chars,
        styles,
        lines,
        line_scales,
        line_extras,
        line_contents,
        spacing,
        line_height,
        max_line,
        fractions,
        justify,
        offset: anchored_offset(paint, &anchors, &anchor, (&symbol.properties, zoom)),
    })
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

/// Where the quads of one label go and what they have recorded.
struct Emitter<'a> {
    buffer: &'a mut VertexBuffers<ShaderSymbolVertex, u32>,
    anchor: geo_types::Point<f64>,
    elevation: f32,
    angle: f32,
    rotation: f32,
    /// Whether each quad is placed by its centre along a line.
    along_line: bool,
    centres: Vec<f32>,
    colors: Vec<TextColorRun>,
}

impl Emitter<'_> {
    /// Adds the quad of `entry` at `bounds`, placed along a line by the `centre` of its advance.
    fn quad(&mut self, entry: &AtlasEntry, bounds: [f32; 4], centre: f32) {
        quad(
            self.buffer,
            self.anchor,
            bounds,
            entry,
            self.elevation,
            self.angle,
            self.rotation,
        );
        if self.along_line {
            // The vertex carries where the centre lies in the straight layout, in 1/32 pixel.
            let first = self.buffer.vertices.len() - 4;
            for vertex in &mut self.buffer.vertices[first..] {
                vertex.a_pixeloffset[0] = (centre * 32.0).round() as i32;
            }
            self.centres.push(centre);
        }
    }

    /// Notes that the glyphs added since `first_index` have a colour of their own.
    fn colour(&mut self, first_index: usize, color: [f32; 4]) {
        let end = self.buffer.indices.len();
        match self.colors.last_mut() {
            Some((range, last)) if *last == color && range.end == first_index => range.end = end,
            _ => self.colors.push((first_index..end, color)),
        }
    }
}

/// Adds the quads of the characters `line` holds, the first at `pen` on the line's `baseline`
/// with the line's `content` height.
fn place_line(
    emit: &mut Emitter<'_>,
    block: &Block<'_>,
    line: Range<usize>,
    (baseline, content): (f32, f32),
    mut pen: f32,
) {
    for index in line {
        let style = &block.styles[index];
        if let Some((entry, size, hang)) = style.image {
            // An image sits on the bottom of the line's content.
            let (x, y) = (pen + hang[0], baseline + content - size[1] + hang[1]);
            let entry = AtlasEntry {
                kind: 3,
                ..entry.clone()
            };
            emit.quad(
                &entry,
                [x, y, x + size[0], y + size[1]],
                pen + size[0] / 2.0,
            );
            pen += size[0] + block.spacing;
            continue;
        }
        let Some(glyph) = style.glyphs.get(&(block.chars[index] as u32)) else {
            continue;
        };
        let scale = style.scale;
        if glyph.rect[2] > 0 && glyph.rect[3] > 0 {
            // Smaller glyphs of a line sit on its bottom, where the largest one does.
            let x = pen + glyph.metrics[0] * scale;
            let y = baseline - glyph.metrics[1] * scale + content - scale * 24.0;
            let first_index = emit.buffer.indices.len();
            let bounds = [
                x,
                y,
                x + glyph.rect[2] as f32 * scale,
                y + glyph.rect[3] as f32 * scale,
            ];
            emit.quad(glyph, bounds, pen + glyph.metrics[2] * scale / 2.0);
            if let Some(color) = style.color {
                emit.colour(first_index, color);
            }
        }
        pen += glyph.metrics[2] * scale + block.spacing;
    }
}

fn glyph_pass(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    block: &Block<'_>,
    justify: f32,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
) -> (Vec<f32>, Vec<TextColorRun>) {
    let shift = crate::sdf::translation::tile_translation(paint, "text", zoom);
    let mut emit = Emitter {
        buffer,
        anchor: geo_types::Point::new(symbol.anchor.x() + shift[0], symbol.anchor.y() + shift[1]),
        elevation: if paint.uses_shared_height() {
            0.0
        } else {
            paint.height_offset("text", &symbol.properties, zoom)
        },
        angle: symbol.angle,
        rotation: paint
            .number("text-rotate", &symbol.properties, zoom, 0.0)
            .to_radians(),
        along_line: symbol.line.is_some() && crate::sdf::paint::text_follows_line(paint, zoom),
        centres: Vec::new(),
        colors: Vec::new(),
    };
    let half_leading = (block.line_height - 24.0) / 2.0;
    let grown = block.line_extras.iter().any(|extra| *extra > 0.0);
    let mut line_top = 0.0;
    for (index, line) in block.lines.iter().enumerate() {
        let (largest, extra, content) = (
            block.line_scales[index],
            block.line_extras[index],
            block.line_contents[index],
        );
        // A block with a line taller than the line height is aligned by its whole height alone.
        let leading = if grown {
            0.0
        } else {
            half_leading * largest - 5.0
        };
        let baseline = -block.height * block.fractions[1] + leading + line_top + block.offset[1];
        line_top += block.line_height * largest + extra;
        let width = line_width(&block.chars, &block.styles, line.clone(), block.spacing);
        let pen = -block.max_line * block.fractions[0]
            + (block.max_line - width) * justify
            + block.offset[0];
        place_line(&mut emit, block, line.clone(), (baseline, content), pen);
    }
    (emit.centres, emit.colors)
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

#[cfg(test)]
#[path = "text_layout/tests.rs"]
mod tests;
