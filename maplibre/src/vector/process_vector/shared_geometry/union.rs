//! Union tessellation and feature-span intersection helpers for sibling uniform layers.

use std::{ops::Range, sync::Arc};

use geozero::{
    mvt::{process_geom, tile},
    FeatureProcessor,
};

use crate::{
    render::ShaderVertex,
    style::{
        expression::FeatureProperties,
        layer::{LayerPaint, StyleLayer},
    },
    vector::{
        process_vector::{
            extent_scale, feature_properties,
            mvt_slice::{
                process_feature_properties, retain_readable_indices, shared_props_for_slice,
                source_ids_for_slice,
            },
            processing, VectorTileRequest,
        },
        tessellation::{feature_paint::FeatureStateSyncContext, IndexDataType, ZeroTessellator},
    },
};

#[derive(Clone)]
pub(super) struct FeatureSpan {
    pub feat_idx: usize,
    pub vtx_count: u32,
    pub indices: Range<usize>,
}

pub(super) fn tessellate_union_with_fallback(
    layer: &tile::Layer,
    union_indices: &[usize],
    shared_props: Option<&[Arc<FeatureProperties>]>,
    first: (&StyleLayer, &LayerPaint),
    request: &VectorTileRequest,
    state_ctx: Option<FeatureStateSyncContext>,
) -> Option<(Vec<ShaderVertex>, Vec<IndexDataType>, Vec<FeatureSpan>)> {
    match tessellate_union(
        layer,
        union_indices,
        shared_props,
        first,
        request,
        state_ctx.clone(),
    ) {
        Ok(res) => Some(res),
        Err(error) => {
            let (readable, dropped) = retain_readable_indices(layer, Some(union_indices));
            if dropped == 0 {
                tracing::error!(coords = %request.coords, layer = %layer.name, ?error, "vector tessellation failed");
                return None;
            }
            tracing::warn!(coords = %request.coords, layer = %layer.name, dropped, ?error, "features whose geometry cannot be read are left out");
            tessellate_union(layer, &readable, shared_props, first, request, state_ctx).ok()
        }
    }
}

fn tessellate_union(
    layer: &tile::Layer,
    slice: &[usize],
    shared_props: Option<&[Arc<FeatureProperties>]>,
    (first_style, first_paint): (&StyleLayer, &LayerPaint),
    request: &VectorTileRequest,
    state_ctx: Option<FeatureStateSyncContext>,
) -> geozero::error::Result<(Vec<ShaderVertex>, Vec<IndexDataType>, Vec<FeatureSpan>)> {
    let mut tessellator: ZeroTessellator<IndexDataType> =
        processing::tessellator(first_paint, request);
    tessellator.set_layer_strokes(first_style);
    tessellator.coordinate_scale = extent_scale(layer);
    tessellator.source_ids = source_ids_for_slice(layer, Some(slice));
    tessellator.shared_properties = shared_props_for_slice(shared_props, Some(slice));
    tessellator.feature_state_ctx = state_ctx;
    tessellator.omit_feature_colors = true;

    let skip_props = shared_props.is_some();
    let mut spans = Vec::with_capacity(slice.len());
    tessellator.dataset_begin(Some(&layer.name))?;
    for (out_idx, &feat_idx) in slice.iter().enumerate() {
        let Some(feature) = layer.features.get(feat_idx) else {
            continue;
        };
        let vtx_start = tessellator.buffer.vertices.len();
        let idx_start = tessellator.buffer.indices.len();
        tessellator.feature_begin(out_idx as u64)?;
        if !skip_props {
            process_feature_properties(layer, feature, &mut tessellator)?;
        }
        tessellator.geometry_begin()?;
        process_geom(feature, &mut tessellator)?;
        tessellator.geometry_end()?;
        tessellator.feature_end(out_idx as u64)?;
        let vtx_count = (tessellator.buffer.vertices.len() - vtx_start) as u32;
        let idx_end = tessellator.buffer.indices.len();
        spans.push(FeatureSpan {
            feat_idx,
            vtx_count,
            indices: idx_start..idx_end,
        });
    }
    tessellator.dataset_end()?;
    Ok((
        tessellator.buffer.vertices,
        tessellator.buffer.indices,
        spans,
    ))
}

pub(super) fn intersect_spans<'a>(
    spans: &'a [FeatureSpan],
    sorted_indices: &[usize],
) -> Vec<&'a FeatureSpan> {
    let mut out = Vec::with_capacity(sorted_indices.len().min(spans.len()));
    let (mut i, mut j) = (0, 0);
    while i < spans.len() && j < sorted_indices.len() {
        match spans[i].feat_idx.cmp(&sorted_indices[j]) {
            std::cmp::Ordering::Equal => {
                out.push(&spans[i]);
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    out
}

pub(super) fn eval_sort_key(
    layer: &tile::Layer,
    shared_props: Option<&[Arc<FeatureProperties>]>,
    feat_idx: usize,
    sort_prop: &crate::style::layer::StyleProperty<f32>,
    zoom: f64,
) -> f32 {
    if let Some(props) = shared_props.and_then(|all| all.get(feat_idx)) {
        return sort_prop.evaluate_for(props, zoom).unwrap_or(0.0);
    }
    layer
        .features
        .get(feat_idx)
        .and_then(|feat| sort_prop.evaluate_for(&feature_properties(layer, feat), zoom))
        .unwrap_or(0.0)
}
