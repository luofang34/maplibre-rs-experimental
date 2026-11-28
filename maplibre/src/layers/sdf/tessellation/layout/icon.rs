//! Icon quad layout and `icon-text-fit` bounds computation.

use geo_types::Point;
use lyon::tessellation::VertexBuffers;

use super::{anchor_fractions, offset, quad, CollectedSymbol};
use crate::{
    render::shaders::ShaderSymbolVertex,
    sdf::assets::{AtlasEntry, SymbolAtlas},
    style::layer::SymbolPaint,
};

/// Appends the symbol's icon quads (if configured and non-zero size), returning any extra
/// clear-border padding applied around stretchable/fitted pieces.
pub(super) fn append_icon(
    symbol: &CollectedSymbol,
    paint: &SymbolPaint,
    zoom: f64,
    atlas: &SymbolAtlas,
    text_block: Option<&super::super::text_layout::Block<'_>>,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
) -> f32 {
    let Some(icon) = paint
        .text_among_images("icon-image", &symbol.properties, zoom, atlas)
        .and_then(|name| atlas.icon(&name))
    else {
        return 0.0;
    };
    let icon_size = paint.number("icon-size", &symbol.properties, zoom, 1.0);
    if icon_size <= 0.0 {
        return 0.0;
    }
    let ratio = icon.metrics[3];
    let width = icon.rect[2] as f32 / ratio;
    let height = icon.rect[3] as f32 / ratio;
    let icon_offset = offset(paint, "icon-offset", 1.0, (&symbol.properties, zoom));
    let fractions = anchor_fractions(
        &paint
            .text("icon-anchor", &symbol.properties, zoom)
            .unwrap_or_else(|| "center".into()),
    );
    // An image whose anchor is not its centre, such as a shield with a banner above its
    // body, is moved so that the anchor stands where the centre would.
    let recentre = [icon.metrics[0] / ratio, icon.metrics[1] / ratio];
    let placed = [
        -width * fractions[0] + icon_offset[0] - recentre[0],
        -height * fractions[1] + icon_offset[1] - recentre[1],
        width * (1.0 - fractions[0]) + icon_offset[0] - recentre[0],
        height * (1.0 - fractions[1]) + icon_offset[1] - recentre[1],
    ];
    let fitted = fit_to_text(
        paint,
        symbol,
        zoom,
        text_block,
        [width * icon_size, height * icon_size],
        icon_offset,
    );
    emit_icon_pieces(
        IconPiecesSpec {
            symbol,
            paint,
            zoom,
            icon,
            placed,
            fitted,
            icon_size,
            text_block,
        },
        buffer,
    )
}

struct IconPiecesSpec<'a> {
    symbol: &'a CollectedSymbol,
    paint: &'a SymbolPaint,
    zoom: f64,
    icon: &'a AtlasEntry,
    placed: [f32; 4],
    fitted: Option<[f32; 4]>,
    icon_size: f32,
    text_block: Option<&'a super::super::text_layout::Block<'a>>,
}

fn emit_icon_pieces(
    spec: IconPiecesSpec<'_>,
    buffer: &mut VertexBuffers<ShaderSymbolVertex, u32>,
) -> f32 {
    let IconPiecesSpec {
        symbol,
        paint,
        zoom,
        icon,
        placed,
        fitted,
        icon_size,
        text_block,
    } = spec;
    let mut icon_padding = 0.0_f32;
    let height_offset = if paint.uses_shared_height() {
        0.0
    } else {
        paint.height_offset("icon", &symbol.properties, zoom)
    };
    let shift = crate::sdf::translation::tile_translation(paint, "icon", zoom);
    let anchor = Point::new(symbol.anchor.x() + shift[0], symbol.anchor.y() + shift[1]);
    let rotation = paint
        .number("icon-rotate", &symbol.properties, zoom, 0.0)
        .to_radians()
        + if text_block.is_some_and(|b| b.vertical) {
            std::f32::consts::FRAC_PI_2
        } else {
            0.0
        };
    // The vertex shader scales icon quads by `icon_size`. Fitted icons and standalone
    // stretchable icons compute their exact screen-pixel bounds in `icon_quads`, so we
    // divide by `icon_size` here so the shader's multiplication recovers those bounds.
    let pre_scaled = fitted.is_some() || icon.stretch.is_some();
    let undo_size = if pre_scaled { 1.0 / icon_size } else { 1.0 };
    let boxed = fitted.unwrap_or_else(|| {
        if icon.stretch.is_some() {
            placed.map(|edge| edge * icon_size)
        } else {
            placed
        }
    });
    for piece in super::super::icon_quads::icon_quads(icon, boxed, fitted.is_some(), icon_size) {
        icon_padding = icon_padding.max(piece.padding * undo_size);
        let part = AtlasEntry {
            rect: piece.rect,
            ..icon.clone()
        };
        quad(
            buffer,
            anchor,
            piece.bounds.map(|edge| edge * undo_size),
            &part,
            height_offset,
            symbol.angle,
            rotation,
        );
    }
    icon_padding
}

/// The icon box `[left, top, right, bottom]` that `icon-text-fit` stretches around the text,
/// in text-size pixels, or `None` when the icon keeps its own size.
fn fit_to_text(
    paint: &SymbolPaint,
    symbol: &CollectedSymbol,
    zoom: f64,
    text_block: Option<&super::super::text_layout::Block<'_>>,
    [icon_width, icon_height]: [f32; 2],
    offset: [f32; 2],
) -> Option<[f32; 4]> {
    let fit = paint.text("icon-text-fit", &symbol.properties, zoom)?;
    if fit == "none" {
        return None;
    }
    let text_size = paint
        .text_size
        .as_ref()
        .and_then(|value| value.evaluate_for(&symbol.properties, zoom))
        .unwrap_or(16.0);
    let scale = text_size / 24.0;
    // A vertical label's icon is fitted before the symbol turns.
    let [left, top, right, bottom] =
        super::super::text_layout::layout_extent_from_block(text_block)?.map(|edge| edge * scale);
    let padding = paint
        .properties
        .get("icon-text-fit-padding")
        .and_then(|value| value.as_array())
        .filter(|values| values.len() == 4)
        .map_or([0.0; 4], |values| {
            let read = |index: usize| {
                values
                    .get(index)
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or(0.0) as f32
            };
            [read(0), read(1), read(2), read(3)]
        });
    let (min_x, max_x) = if matches!(fit.as_str(), "width" | "both") {
        (
            offset[0] + left - padding[3],
            offset[0] + right + padding[1],
        )
    } else {
        let min = offset[0] + (left + right - icon_width) / 2.0;
        (min, min + icon_width)
    };
    let (min_y, max_y) = if matches!(fit.as_str(), "height" | "both") {
        (
            offset[1] + top - padding[0],
            offset[1] + bottom + padding[2],
        )
    } else {
        let min = offset[1] + (top + bottom - icon_height) / 2.0;
        (min, min + icon_height)
    };
    Some([min_x, min_y, max_x, max_y])
}
