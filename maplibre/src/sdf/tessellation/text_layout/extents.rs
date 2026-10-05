//! The boxes a label takes: its layout extent, how far its variable anchors move it, and its
//! width without wrapping.
use super::{
    block,
    styles::{char_styles, line_width},
    vertical,
};
use crate::{
    sdf::{
        assets::SymbolAtlas,
        tessellation::{
            layout::{anchor_fractions, CollectedSymbol},
            text_offset::{anchored_offset, variable_anchors},
        },
    },
    style::layer::SymbolPaint,
};

/// How far the label moves, in layout pixels, when it takes each of its variable anchors
/// instead of the first; empty unless the label has several.
pub(in crate::sdf::tessellation) fn variable_shifts(
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
        let offset = anchored_offset(paint, &anchors, anchor, (&symbol.properties, zoom));
        if block.vertical {
            // The turned box is as wide as the label is tall.
            return vertical::anchor_shift(anchor, [block.height, block.max_line], offset);
        }
        let fractions = anchor_fractions(anchor);
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

/// The two ways a point label whose style allows both is written, as `[first, second]` with
/// `true` for vertical; `None` for a text with one.
pub(in crate::sdf::tessellation) fn both_orientations(
    paint: &SymbolPaint,
    text: &str,
) -> Option<[bool; 2]> {
    let chars: Vec<char> = text.chars().collect();
    if crate::sdf::tessellation::is_line_placed(paint) || !vertical::allows_vertical_writing(&chars)
    {
        return None;
    }
    vertical::orientations(paint)
}

/// Whether the label is written top to bottom, which turns the whole symbol a quarter clockwise.
pub(in crate::sdf::tessellation) fn is_vertical(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
) -> bool {
    block(symbol, paint, zoom, atlas).is_some_and(|block| block.vertical)
}

/// The label's layout box `[left, top, right, bottom]` before a vertical label turns, in layout
/// pixels.
pub(in crate::sdf::tessellation) fn layout_extent(
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

/// The label's layout box `[left, top, right, bottom]` around the anchor, in layout pixels.
pub(in crate::sdf::tessellation) fn extent(
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
    let laid = [left, top, left + block.max_line, top + block.height];
    Some(if block.vertical {
        vertical::turned_box(laid)
    } else {
        laid
    })
}

/// Width in layout pixels of the label on one line, without wrapping; zero without glyphs.
pub(in crate::sdf::tessellation) fn unwrapped_width(
    paint: &SymbolPaint,
    symbol: &CollectedSymbol,
    zoom: f64,
    atlas: &SymbolAtlas,
) -> f32 {
    let Some(text) = paint.label_among(&symbol.properties, zoom, Some(&atlas.icons)) else {
        return 0.0;
    };
    let chars: Vec<char> = text.chars().collect();
    let Some(styles) = char_styles(paint, symbol, zoom, atlas, chars.len()) else {
        return 0.0;
    };
    let spacing = paint.number("text-letter-spacing", &symbol.properties, zoom, 0.0) * 24.0;
    line_width(&chars, &styles, 0..chars.len(), spacing)
}
