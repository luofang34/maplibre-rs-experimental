//! Union tessellation and feature-span intersection helpers for sibling uniform layers.

use std::{
    ops::Range,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use geozero::{mvt::tile, FeatureProcessor};
use lyon::tessellation::VertexBuffers;

use crate::{
    io::vector_tile::feature_properties,
    render::ShaderVertex,
    style::{
        expression::FeatureProperties,
        layer::{LayerPaint, StyleLayer},
    },
    vector::{
        process_vector::{
            extent_scale,
            mvt_slice::{
                layer_header, process_feature_properties, retain_readable_indices,
                shared_props_for_slice, source_ids_for_slice,
            },
            processing, ProcessVectorContext, ProcessVectorError, VectorTileRequest,
        },
        tessellation::{
            feature_paint::FeatureStateSyncContext, IndexDataType, OverAlignedVertexBuffer,
            ZeroTessellator,
        },
        transferables::VectorTransferables,
    },
};

static NEXT_SHARED_GEOMETRY_KEY: AtomicU64 = AtomicU64::new(1);

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
    tessellator.configure_tile_input(crate::vector::tessellation::ZeroTessellatorTileInput {
        coordinate_scale: extent_scale(layer),
        source_ids: source_ids_for_slice(layer, Some(slice)),
        shared_properties: shared_props_for_slice(shared_props, Some(slice)),
        feature_state_ctx: state_ctx,
        omit_feature_colors: true,
        ..Default::default()
    });

    let skip_props = shared_props.is_some();
    let mut spans = Vec::with_capacity(slice.len());
    tessellator.dataset_begin(Some(&layer.name))?;
    for (out_idx, &feat_idx) in slice.iter().enumerate() {
        let Some(feature) = layer.features.get(feat_idx) else {
            continue;
        };
        let vtx_start = tessellator.vertex_count();
        let idx_start = tessellator.index_count();
        tessellator.feature_begin(out_idx as u64)?;
        if !skip_props {
            process_feature_properties(layer, feature, &mut tessellator)?;
        }
        tessellator.geometry_begin()?;
        super::super::mvt_slice::process_geom_safe(feature, &mut tessellator)?;
        tessellator.geometry_end()?;
        tessellator.feature_end(out_idx as u64)?;
        let vtx_count = (tessellator.vertex_count() - vtx_start) as u32;
        let idx_end = tessellator.index_count();
        spans.push(FeatureSpan {
            feat_idx,
            vtx_count,
            indices: idx_start..idx_end,
        });
    }
    tessellator.dataset_end()?;
    let (vertices, indices) = tessellator.into_raw_buffers();
    Ok((vertices, indices, spans))
}

pub(super) fn intersect_spans<'a>(
    spans: &'a [FeatureSpan],
    sorted_indices: &[usize],
) -> Vec<&'a FeatureSpan> {
    let mut out = Vec::with_capacity(sorted_indices.len().min(spans.len()));
    let (mut i, mut j) = (0, 0);
    while let (Some(span), Some(&target)) = (spans.get(i), sorted_indices.get(j)) {
        match span.feat_idx.cmp(&target) {
            std::cmp::Ordering::Equal => {
                out.push(span);
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

pub(super) fn emit_shared_group_layers<T: VectorTransferables, C: crate::io::apc::Context>(
    layer: &tile::Layer,
    per_layer: Vec<(&StyleLayer, Vec<IndexDataType>, Vec<u32>)>,
    shared_vertices: &mut Vec<ShaderVertex>,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let non_empty_count = per_layer
        .iter()
        .filter(|(_, idxs, _)| !idxs.is_empty())
        .count();
    let shared_key =
        (non_empty_count >= 2).then(|| NEXT_SHARED_GEOMETRY_KEY.fetch_add(1, Ordering::Relaxed));
    shared_vertices.shrink_to_fit();
    let mut primary_taken = false;
    for (style, indices, feature_indices) in per_layer {
        let buffer = if indices.is_empty() {
            OverAlignedVertexBuffer::empty()
        } else {
            let vertices = if !primary_taken {
                primary_taken = true;
                std::mem::take(shared_vertices)
            } else {
                Vec::new()
            };
            OverAlignedVertexBuffer::from(VertexBuffers { vertices, indices })
                .with_shared_geometry_key(shared_key)
        };
        context.layer_tessellation_finished(
            &request.coords,
            buffer,
            feature_indices,
            (Vec::new(), Vec::new(), Vec::new()),
            layer_header(layer),
            style,
        )?;
    }
    Ok(())
}
