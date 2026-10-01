//! Text written top to bottom: `text-writing-mode: vertical`.
//!
//! The label is laid out along a horizontal line, as GL JS shapes it, and the whole label then
//! turns a quarter clockwise about its anchor. Characters that stand upright in vertical text
//! take a full em of the line and are turned back, so they read upright on the map; the rest
//! keep their own advance and lie on their side.
use std::ops::Range;

use super::{styles::CharStyle, Emitter};
use crate::sdf::tessellation::layout::anchor_fractions;
use crate::{sdf::assets::AtlasEntry, style::layer::SymbolPaint};

/// Where a vertical glyph's box sits from its line, which GL JS measures from the em box rather
/// than from the baseline.
const UPRIGHT_BASELINE: f32 = 17.0;
/// The offset along the line that centres an upright glyph on its em box.
const UPRIGHT_CENTRE: f32 = 7.0;

/// Whether the first entry of `text-writing-mode` is `vertical`, which a label written both ways
/// takes while it is placeable.
pub(super) fn prefers_vertical(paint: &SymbolPaint) -> bool {
    paint
        .properties
        .get("text-writing-mode")
        .and_then(serde_json::Value::as_array)
        .and_then(|modes| modes.first())
        .and_then(serde_json::Value::as_str)
        == Some("vertical")
}

/// The ways `text-writing-mode` allows, best first, as `[first, second]` with `true` for
/// vertical; `None` unless it names both.
pub(super) fn orientations(paint: &SymbolPaint) -> Option<[bool; 2]> {
    let modes = paint
        .properties
        .get("text-writing-mode")?
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>();
    let vertical_first = modes.iter().position(|mode| *mode == "vertical")?
        < modes.iter().position(|mode| *mode == "horizontal")?;
    Some([vertical_first, !vertical_first])
}

/// Whether the text has a character that stands upright in vertical writing, the only text that
/// is written vertically.
pub(super) fn allows_vertical_writing(chars: &[char]) -> bool {
    chars.iter().any(|c| has_upright_orientation(*c))
}

/// Whether a character of a vertical label stands upright: all but spaces and Arabic script.
pub(super) fn is_upright(c: char) -> bool {
    !(c.is_whitespace() || in_arabic_script(c))
}

fn in_arabic_script(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0600..=0x0604
            | 0x0606..=0x060B
            | 0x060D..=0x061A
            | 0x061C..=0x061E
            | 0x0620..=0x063F
            | 0x0641..=0x064A
            | 0x0656..=0x066F
            | 0x0671..=0x06DC
            | 0x06DE..=0x06FF
            | 0x0750..=0x077F
            | 0x0870..=0x088E
            | 0x0890..=0x0891
            | 0x0898..=0x08E1
            | 0x08E3..=0x08FF
            | 0xFB50..=0xFBC2
            | 0xFBD3..=0xFD3D
            | 0xFD40..=0xFD8F
            | 0xFD92..=0xFDC7
            | 0xFDCF
            | 0xFDF0..=0xFDFF
            | 0xFE70..=0xFE74
            | 0xFE76..=0xFEFC
    )
}

/// Unicode's vertical orientation "upright" ranges, as GL JS tests them.
fn has_upright_orientation(c: char) -> bool {
    matches!(
        u32::from(c),
        0x02EA..=0x02EB
            | 0x1100..=0x11FF
            | 0x1400..=0x167F
            | 0x18B0..=0x18F5
            | 0x2E80..=0x2E99
            | 0x2E9B..=0x2EF3
            | 0x2F00..=0x2FD5
            | 0x2FF0..=0x3007
            | 0x3012..=0x3013
            | 0x3020..=0x302F
            | 0x3031..=0x303F
            | 0x3041..=0x3096
            | 0x309D..=0x30FB
            | 0x30FD..=0x30FF
            | 0x3105..=0x312F
            | 0x3131..=0x318E
            | 0x3190..=0xA48C
            | 0xA490..=0xA4C6
            | 0xA960..=0xA97C
            | 0xAC00..=0xD7A3
            | 0xD7B0..=0xD7C6
            | 0xD7CB..=0xD7FB
            | 0xF900..=0xFA6D
            | 0xFA70..=0xFAD9
            | 0xFE10..=0xFE1F
            | 0xFE30..=0xFE48
            | 0xFE50..=0xFE57
            | 0xFE5F..=0xFE62
            | 0xFE67..=0xFE6F
            | 0xFF00..=0xFF07
            | 0xFF0A..=0xFF0C
            | 0xFF0E..=0xFF19
            | 0xFF1F..=0xFF3A
            | 0xFF3C
            | 0xFF3E
            | 0xFF40..=0xFF5A
            | 0xFFE0..=0xFFE2
            | 0xFFE4..=0xFFE7
            | 0x1F000..=0x1F200
            | 0x1F300..=0x1F7FF
            | 0x1F900..=0x1FBFF
            | 0x20000..=0x323AF
    )
}

/// How far a character advances along a vertical label's line: a full em for an upright one.
pub(super) fn advance(style: &CharStyle<'_>, c: char) -> f32 {
    if let Some((_, size, _)) = style.image {
        // An image takes its height along the line.
        return size[1];
    }
    if is_upright(c) {
        24.0 * style.scale
    } else {
        style.advance(c)
    }
}

/// Length in layout pixels of a vertical label's line, without the spacing after the last glyph.
pub(super) fn line_length(
    chars: &[char],
    styles: &[CharStyle<'_>],
    line: Range<usize>,
    spacing: f32,
) -> f32 {
    let advances: Vec<f32> = line
        .filter(|index| styles[*index].is_drawn(chars[*index]))
        .map(|index| advance(&styles[index], chars[index]) + spacing)
        .collect();
    if advances.is_empty() {
        0.0
    } else {
        advances.iter().sum::<f32>() - spacing
    }
}

/// Turns a layout box `[left, top, right, bottom]` a quarter clockwise about the anchor.
pub(super) fn turned_box([left, top, right, bottom]: [f32; 4]) -> [f32; 4] {
    [-bottom, left, -top, right]
}

/// Where a box of `[width, height]` on the screen sits from its anchor when it takes `anchor`
/// with `offset`, relative to being centred on it.
pub(super) fn anchor_shift(anchor: &str, [width, height]: [f32; 2], offset: [f32; 2]) -> [f32; 2] {
    let [x, y] = anchor_fractions(anchor);
    [
        -width * (x - 0.5) + offset[0],
        -height * (y - 0.5) + offset[1],
    ]
}

/// A line of a vertical label as it is placed.
pub(super) struct VerticalLine {
    /// Where the line's glyph boxes start, from the top of the label before the label turns.
    pub baseline: f32,
    /// The largest scale on the line.
    pub largest: f32,
    /// How far the column grows past an em to hold an image wider than that.
    pub extra: f32,
    /// The width of the column's content: an em of the largest scale, or its widest image.
    pub content: f32,
    /// Where along the line the first glyph starts.
    pub pen: f32,
}

/// Adds the quads of the characters `line` holds.
pub(super) fn place_line(
    emit: &mut Emitter<'_>,
    (chars, styles, spacing): (&[char], &[CharStyle<'_>], f32),
    line: Range<usize>,
    placed: &VerticalLine,
) {
    let mut pen = placed.pen;
    // How far the line shifts its glyphs sideways, as GL JS keeps the column centred.
    let column_offset = ((placed.largest - 1.0) * 24.0).max(placed.extra);
    for index in line {
        let style = &styles[index];
        let c = chars[index];
        if let Some((entry, size, hang)) = style.image {
            let entry = AtlasEntry {
                kind: 3,
                ..entry.clone()
            };
            let half_advance = size[1] / 2.0;
            let left = hang[0] - half_advance;
            let top = hang[1];
            let line_offset = column_offset / 2.0 + (24.0 - size[0]) / 2.0;
            let across = -(placed.baseline + placed.content - size[1] - line_offset
                + UPRIGHT_BASELINE)
                + (12.0 - half_advance);
            let along = pen - UPRIGHT_CENTRE;
            emit.quad_rotated(
                &entry,
                [
                    left + across,
                    top + along,
                    left + size[0] + across,
                    top + size[1] + along,
                ],
                0.0,
            );
            pen += size[1] + spacing;
            continue;
        }
        let Some(glyph) = style.glyphs.get(&(c as u32)) else {
            continue;
        };
        let scale = style.scale;
        let line_offset = column_offset / 2.0 - (scale - 1.0) * 24.0;
        if glyph.rect[2] > 0 && glyph.rect[3] > 0 {
            let first_index = emit.buffer.indices.len();
            let (width, height) = (glyph.rect[2] as f32 * scale, glyph.rect[3] as f32 * scale);
            if is_upright(c) {
                emit_upright(
                    emit,
                    glyph,
                    (pen, scale),
                    (width, height),
                    placed,
                    line_offset,
                );
            } else {
                // Lying on its side: placed along the line, then turned with the label.
                let x = pen + glyph.metrics[0] * scale;
                let y = placed.baseline - glyph.metrics[1] * scale + placed.content
                    - scale * 24.0
                    - line_offset;
                emit.quad_rotated(
                    glyph,
                    [x, y, x + width, y + height],
                    std::f32::consts::FRAC_PI_2,
                );
            }
            if let Some(color) = style.color {
                emit.colour(first_index, color);
            }
        }
        pen += advance(style, c) + spacing;
    }
}

/// An upright glyph: turned back, so its quad is the plain box where the label's turn puts it.
fn emit_upright(
    emit: &mut Emitter<'_>,
    glyph: &AtlasEntry,
    (pen, scale): (f32, f32),
    (width, height): (f32, f32),
    placed: &VerticalLine,
    line_offset: f32,
) {
    let half_advance = glyph.metrics[2] * scale / 2.0;
    let from_baseline = placed.content - scale * 24.0;
    let left = glyph.metrics[0] * scale
        - half_advance
        - (placed.baseline + from_baseline - line_offset + UPRIGHT_BASELINE);
    let top = -glyph.metrics[1] * scale + pen - UPRIGHT_CENTRE;
    emit.quad_rotated(glyph, [left, top, left + width, top + height], 0.0);
}

#[cfg(test)]
mod tests;
