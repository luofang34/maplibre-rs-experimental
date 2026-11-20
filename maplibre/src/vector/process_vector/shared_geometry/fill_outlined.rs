//! Deduplicates tessellation across sibling antialiased or `fill-outline-color` fill layers
//! on the same source layer.

use geozero::mvt::tile;

use crate::{
    io::apc::Context,
    style::layer::{FillPaint, LayerPaint, StyleLayer},
    vector::{
        process_vector::{
            extent_scale,
            mvt_slice::{layer_header, process_mvt_slice, retain_readable_indices},
            processing, ProcessVectorContext, ProcessVectorError, VectorTileRequest,
        },
        tessellation::{sort_key_of, IndexDataType, OverAlignedVertexBuffer, ZeroTessellator},
        transferables::VectorTransferables,
    },
};

/// Returns `true` when an outlined/antialiased fill layer without filters, patterns, sort keys,
/// or per-feature paint can share a single tessellation with sibling layers on the same source.
pub(super) fn qualifies_outlined_fill(
    style: &StyleLayer,
    fill: &FillPaint,
    style_zoom: f64,
) -> bool {
    let Some(paint) = style.paint.as_ref() else {
        return false;
    };
    style.filter.is_none()
        && sort_key_of(style).is_none()
        && fill.fill_pattern.is_none()
        && crate::vector::pattern::per_feature_pattern(paint).is_none()
        && fill
            .fill_color
            .as_ref()
            .is_none_or(|prop| prop.is_feature_constant())
        && fill
            .fill_opacity
            .as_ref()
            .is_none_or(|prop| prop.is_feature_constant())
        && fill.outline_color().is_some_and(|outline| {
            outline.is_feature_constant() && outline.evaluate_at_zoom(style_zoom).is_some()
        })
}

/// Tessellates `layer` once for a group of qualifying outlined fill layers and emits a bucket
/// per sibling layer, sharing pre-packed GPU buffers zero-copy via `Arc`.
pub(super) fn process_shared_fill_outlined<T: VectorTransferables, C: Context>(
    layer: &tile::Layer,
    group: &[&StyleLayer],
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let Some(first_style) = group.first().copied() else {
        return Ok(());
    };
    let Some(first_paint) = first_style.paint.as_ref() else {
        return Ok(());
    };
    let allow_prepack = context.allow_prepack_for_style(request.coords, first_style, false, false);
    let Some(mut tessellator) = tessellate_readable(layer, request, |slice| {
        let mut tessellator: ZeroTessellator<IndexDataType> =
            processing::tessellator(first_paint, request);
        tessellator.coordinate_scale = extent_scale(layer);
        tessellator.omit_feature_colors = true;
        tessellator.record_outline_entries = true;
        if allow_prepack {
            tessellator.maybe_enable_streaming_prepack(first_style.type_.as_str());
        }
        process_mvt_slice(layer, slice, true, &mut tessellator)?;
        Ok(tessellator)
    }) else {
        for _ in group {
            context.layer_missing(&request.coords, &layer.name)?;
        }
        return Ok(());
    };

    let mut feature_indices = std::mem::take(&mut tessellator.feature_indices);
    let outline_entries = std::mem::take(&mut tessellator.outline_entries);
    feature_indices.shrink_to_fit();
    let shared_buffer = tessellator.into_over_aligned_buffer();
    let header = layer_header(layer);
    let style_zoom = request.style_zoom();
    let mut shared_buffer = Some(shared_buffer);
    let mut feature_indices = Some(feature_indices);
    let mut header = Some(header);

    for (idx, &style) in group.iter().enumerate() {
        let is_last = idx + 1 == group.len();
        let buffer = if is_last {
            shared_buffer
                .take()
                .unwrap_or_else(OverAlignedVertexBuffer::empty)
        } else {
            shared_buffer
                .as_ref()
                .cloned()
                .unwrap_or_else(OverAlignedVertexBuffer::empty)
        };
        let indices = if is_last {
            feature_indices.take().unwrap_or_default()
        } else {
            feature_indices.as_ref().cloned().unwrap_or_default()
        };
        let layer_data = if is_last {
            header.take().unwrap_or_else(|| layer_header(layer))
        } else {
            header.clone().unwrap_or_else(|| layer_header(layer))
        };
        let feature_colors = eval_layer_colors(style, style_zoom, &outline_entries);
        context.layer_tessellation_finished(
            &request.coords,
            buffer,
            indices,
            (feature_colors, Vec::new(), Vec::new()),
            layer_data,
            style,
        )?;
    }
    Ok(())
}

fn eval_layer_colors(
    style: &StyleLayer,
    style_zoom: f64,
    outline_entries: &[bool],
) -> Vec<[f32; 4]> {
    if outline_entries.is_empty() {
        return Vec::new();
    }
    #[cfg(any(target_arch = "wasm32", not(test)))]
    if crate::vector::upload_system::paint::uniform_color(style, style_zoom).is_some() {
        return Vec::new();
    }
    let Some(LayerPaint::Fill(fill)) = style.paint.as_ref() else {
        return Vec::new();
    };
    let opacity = fill
        .fill_opacity
        .as_ref()
        .and_then(|prop| prop.evaluate_at_zoom(style_zoom))
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    let fill_base = fill
        .fill_color
        .as_ref()
        .and_then(|prop| prop.evaluate_at_zoom(style_zoom))
        .map_or([0.0, 0.0, 0.0, 1.0], |c| [c.r, c.g, c.b, c.a]);
    let fill_rgba = [
        fill_base[0],
        fill_base[1],
        fill_base[2],
        fill_base[3] * opacity,
    ];
    let out_base = fill
        .outline_color()
        .and_then(|prop| prop.evaluate_at_zoom(style_zoom))
        .map_or([0.0, 0.0, 0.0, 1.0], |c| [c.r, c.g, c.b, c.a]);
    let outline_rgba = [out_base[0], out_base[1], out_base[2], out_base[3] * opacity];
    outline_entries
        .iter()
        .map(|&is_outline| if is_outline { outline_rgba } else { fill_rgba })
        .collect()
}

fn tessellate_readable<T>(
    layer: &tile::Layer,
    request: &VectorTileRequest,
    mut build: impl FnMut(Option<&[usize]>) -> geozero::error::Result<T>,
) -> Option<T> {
    let error = match build(None) {
        Ok(tessellator) => return Some(tessellator),
        Err(error) => error,
    };
    let (readable, dropped) = retain_readable_indices(layer, None);
    if dropped == 0 {
        tracing::error!(coords = %request.coords, layer = %layer.name, ?error, "vector tessellation failed");
        return None;
    }
    tracing::warn!(coords = %request.coords, layer = %layer.name, dropped, ?error, "features whose geometry cannot be read are left out");
    match build(Some(&readable)) {
        Ok(tessellator) => Some(tessellator),
        Err(error) => {
            tracing::error!(coords = %request.coords, layer = %layer.name, ?error, "vector tessellation failed");
            None
        }
    }
}
