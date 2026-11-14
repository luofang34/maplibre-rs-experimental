//! OpenType `GSUB`/`GPOS` shaped-run wrapping, width calculation, and quad placement.

use std::ops::Range;

use super::{
    super::{
        layout::{anchor_fractions, CollectedSymbol},
        text_offset::{anchored_offset, variable_anchors},
    },
    Block, Emitter,
};
use crate::{font::ShapedGlyph, sdf::assets::SymbolAtlas, style::layer::SymbolPaint};

/// Shaped glyph slices referenced by a [`Block`].
pub(in crate::sdf::tessellation) struct ShapedBlock<'a> {
    pub(super) atlas: &'a SymbolAtlas,
    pub(super) glyphs: Vec<ShapedGlyph>,
}

pub(in crate::sdf::tessellation) fn build_shaped_block<'a>(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &'a SymbolAtlas,
) -> Option<Block<'a>> {
    let vertical_requested = !super::super::is_line_placed(paint)
        && symbol
            .vertical
            .unwrap_or_else(|| super::vertical::prefers_vertical(paint));
    if vertical_requested {
        return None;
    }
    let raw = paint.raw_label_among(&symbol.properties, zoom, Some(&atlas.icons))?;
    let shaped = atlas.shaped_runs.get(&paint.font_stack())?.get(&raw)?;
    if shaped.is_empty() {
        return None;
    }

    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    let max_width = paint.number("text-max-width", &symbol.properties, zoom, 10.0) * 24.0;
    let (flat_glyphs, lines) = if super::super::is_line_placed(paint) {
        (shaped.clone(), std::iter::once(0..shaped.len()).collect())
    } else {
        wrap_shaped_paragraphs(&raw, shaped, max_width, spacing)
    };
    if lines.is_empty() {
        return None;
    }

    let line_height = paint.number("text-line-height", &symbol.properties, zoom, 1.2) * 24.0;
    let max_line = lines
        .iter()
        .map(|range| shaped_line_width(&flat_glyphs[range.clone()], spacing))
        .fold(0.0_f32, f32::max);
    let line_count = lines.len();
    let line_scales = vec![1.0_f32; line_count];
    let line_extras = vec![0.0_f32; line_count];
    let line_contents = vec![24.0_f32; line_count];

    let anchors = variable_anchors(paint);
    let anchor = anchors.first().cloned().unwrap_or_else(|| {
        paint
            .text("text-anchor", &symbol.properties, zoom)
            .unwrap_or_else(|| "center".into())
    });
    let fractions = anchor_fractions(&anchor);
    let justify_prop = paint
        .text("text-justify", &symbol.properties, zoom)
        .unwrap_or_else(|| "center".into());
    let justify = match justify_prop.as_str() {
        "left" => 0.0,
        "right" => 1.0,
        "auto" => fractions[0],
        _ => 0.5,
    };
    let height = line_count as f32 * line_height;
    let offset = anchored_offset(paint, &anchors, &anchor, (&symbol.properties, zoom));

    Some(Block {
        chars: Vec::new(),
        styles: Vec::new(),
        lines,
        line_scales,
        line_extras,
        line_contents,
        spacing,
        line_height,
        max_line,
        height,
        fractions,
        justify,
        offset,
        vertical: false,
        shaped: Some(ShapedBlock {
            atlas,
            glyphs: flat_glyphs,
        }),
    })
}

pub(in crate::sdf::tessellation) fn unwrapped_shaped_width(
    paint: &SymbolPaint,
    symbol: &CollectedSymbol,
    zoom: f64,
    atlas: &SymbolAtlas,
) -> Option<f32> {
    let raw = paint.raw_label_among(&symbol.properties, zoom, Some(&atlas.icons))?;
    let shaped = atlas.shaped_runs.get(&paint.font_stack())?.get(&raw)?;
    if shaped.is_empty() {
        return None;
    }
    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    Some(shaped_line_width(shaped, spacing))
}

pub(super) fn place_shaped_line(
    emit: &mut Emitter<'_>,
    block: &Block<'_>,
    shaped: &ShapedBlock<'_>,
    line_index: usize,
    (baseline, content): (f32, f32),
    justify: f32,
) {
    let Some(range) = block.lines.get(line_index) else {
        return;
    };
    let slice = &shaped.glyphs[range.clone()];
    let width = shaped_line_width(slice, block.spacing);
    let mut pen =
        -block.max_line * block.fractions[0] + (block.max_line - width) * justify + block.offset[0];
    for (idx, sg) in slice.iter().enumerate() {
        if let Some(entry) = shaped.atlas.face_glyphs.get(&sg.atlas_key()) {
            if entry.rect[2] > 0 && entry.rect[3] > 0 {
                let x = pen + entry.metrics[0] + sg.x_offset;
                let y = baseline - (entry.metrics[1] + sg.y_offset) + content - 24.0;
                let bounds = [x, y, x + entry.rect[2] as f32, y + entry.rect[3] as f32];
                emit.quad(entry, bounds, pen + sg.x_advance / 2.0);
            }
        }
        let add_spacing = idx + 1 < slice.len()
            && !sg.rtl
            && slice[idx + 1].cluster != sg.cluster
            && sg.x_advance > 0.0;
        pen += sg.x_advance + if add_spacing { block.spacing } else { 0.0 };
    }
}

fn shaped_line_width(slice: &[ShapedGlyph], spacing: f32) -> f32 {
    if slice.is_empty() {
        return 0.0;
    }
    let mut width = 0.0_f32;
    for (idx, sg) in slice.iter().enumerate() {
        let add_spacing = idx + 1 < slice.len()
            && !sg.rtl
            && slice[idx + 1].cluster != sg.cluster
            && sg.x_advance > 0.0;
        width += sg.x_advance + if add_spacing { spacing } else { 0.0 };
    }
    width
}

fn wrap_shaped_paragraphs(
    raw: &str,
    shaped: &[ShapedGlyph],
    max_width: f32,
    spacing: f32,
) -> (Vec<ShapedGlyph>, Vec<Range<usize>>) {
    let mut out_glyphs = Vec::with_capacity(shaped.len());
    let mut out_lines = Vec::new();
    let mut para_start = 0usize;

    for part in raw.split('\n') {
        let para_end = para_start + part.len();
        let para_glyphs: Vec<&ShapedGlyph> = shaped
            .iter()
            .filter(|sg| {
                let c = sg.cluster as usize;
                c >= para_start && c < para_end
            })
            .collect();
        para_start = para_end + 1;
        if para_glyphs.is_empty() {
            continue;
        }
        let any_rtl = para_glyphs.iter().any(|sg| sg.rtl);
        if max_width <= 0.0 || any_rtl {
            let line_start = out_glyphs.len();
            out_glyphs.extend(para_glyphs.into_iter().cloned());
            out_lines.push(line_start..out_glyphs.len());
            continue;
        }
        wrap_ltr_paragraph(
            raw,
            &para_glyphs,
            max_width,
            spacing,
            &mut out_glyphs,
            &mut out_lines,
        );
    }
    (out_glyphs, out_lines)
}

fn wrap_ltr_paragraph(
    raw: &str,
    para_glyphs: &[&ShapedGlyph],
    max_width: f32,
    spacing: f32,
    out_glyphs: &mut Vec<ShapedGlyph>,
    out_lines: &mut Vec<Range<usize>>,
) {
    let is_space = |sg: &ShapedGlyph| -> bool {
        raw.get(sg.cluster as usize..)
            .and_then(|s| s.chars().next())
            .is_some_and(char::is_whitespace)
    };

    let mut tokens: Vec<(bool, Vec<ShapedGlyph>)> = Vec::new();
    for &sg in para_glyphs {
        let ws = is_space(sg);
        if let Some((last_ws, list)) = tokens.last_mut().filter(|(lw, _)| *lw == ws) {
            let _ = last_ws;
            list.push(sg.clone());
        } else {
            tokens.push((ws, vec![sg.clone()]));
        }
    }

    let mut current: Vec<ShapedGlyph> = Vec::new();
    let mut pending_ws: Vec<ShapedGlyph> = Vec::new();
    for (ws, glyphs) in tokens {
        if ws {
            if !current.is_empty() {
                pending_ws.extend(glyphs);
            }
            continue;
        }
        if current.is_empty() {
            current.extend(glyphs);
            continue;
        }
        let mut candidate = current.clone();
        candidate.extend(pending_ws.iter().cloned());
        candidate.extend(glyphs.iter().cloned());
        if shaped_line_width(&candidate, spacing) <= max_width {
            current = candidate;
            pending_ws.clear();
        } else {
            let start = out_glyphs.len();
            out_glyphs.append(&mut current);
            out_lines.push(start..out_glyphs.len());
            pending_ws.clear();
            current.extend(glyphs);
        }
    }
    if !current.is_empty() {
        let start = out_glyphs.len();
        out_glyphs.append(&mut current);
        out_lines.push(start..out_glyphs.len());
    }
}
