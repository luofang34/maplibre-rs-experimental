//! Deduplicates source-layer geometry tessellation and vertex buffers across sibling style layers.

use std::{
    ops::Range,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use geozero::{
    mvt::{process_geom, tile},
    FeatureProcessor,
};
use lyon::tessellation::VertexBuffers;
use rustc_hash::FxHashMap;

use super::{
    extent_scale, feature_properties,
    mvt_slice::{
        decode_layer_properties, layer_header, process_feature_properties, retain_readable_indices,
        select_filtered_features, shared_props_for_slice, source_ids_for_slice,
    },
    processing, ProcessVectorContext, ProcessVectorError, VectorTileRequest,
};
use crate::{
    render::ShaderVertex,
    sdf::assets::SymbolAtlas,
    style::{
        expression::{layer_reads_geometry, layer_reads_identity, FeatureProperties},
        feature_state::{layer_geometry_reads_feature_state, layer_paint_reads_feature_state},
        filter::Filter,
        layer::{LayerPaint, StyleLayer},
        line_stroke::{LineCap, LineJoin, LineStroke},
    },
    vector::{
        feature_paint::{feature_paint_retention, FeaturePaintRetention},
        pattern::per_feature_pattern,
        structures,
        tessellation::{
            sort_key_of, IndexDataType, LineElevationOptions, LineFeatureStyle,
            OverAlignedVertexBuffer, ZeroTessellator,
        },
        transferables::VectorTransferables,
        upload_system::paint::uniform_color,
    },
};

static NEXT_SHARED_GEOMETRY_KEY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GeometryBucketKey {
    Fill,
    Line {
        cap: LineCap,
        join: LineJoin,
        miter_bits: u32,
        round_bits: u32,
        split_corners: bool,
    },
}

enum SelectedSet {
    All,
    Indices(Vec<usize>),
}

#[derive(Clone)]
struct FeatureSpan {
    feat_idx: usize,
    vtx_count: u32,
    indices: Range<usize>,
}

pub(super) fn process_ordered_layers<'a, T: VectorTransferables, C: crate::io::apc::Context>(
    tile: &'a geozero::mvt::Tile,
    ordered_layers: &[&StyleLayer],
    index_uses_props: bool,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlases: (&Arc<SymbolAtlas>, &Arc<SymbolAtlas>),
    shared_layer_props: &mut FxHashMap<&'a str, Option<Vec<Arc<FeatureProperties>>>>,
) -> Result<(), ProcessVectorError> {
    let mut start = 0;
    while start < ordered_layers.len() {
        let group_name = ordered_layers[start].source_layer.as_deref();
        let mut end = start + 1;
        while end < ordered_layers.len()
            && ordered_layers[end].source_layer.as_deref() == group_name
        {
            end += 1;
        }
        let chunk = &ordered_layers[start..end];
        start = end;

        let Some(name) = group_name else {
            for style in chunk {
                tracing::error!(layer = %style.id, "vector style layer misses a required attribute");
            }
            continue;
        };

        let source_layer = tile.layers.iter().find(|layer| layer.name == name);
        let shared_props = source_layer.and_then(|layer| {
            shared_layer_props
                .entry(layer.name.as_str())
                .or_insert_with(|| {
                    let state_ctx = context.state_context_for_layer(&layer.name);
                    decode_layer_properties(layer, state_ctx.as_ref())
                })
                .as_deref()
        });

        let mut key_counts: FxHashMap<GeometryBucketKey, usize> = FxHashMap::default();
        let mut layer_keys: Vec<Option<GeometryBucketKey>> = Vec::with_capacity(chunk.len());
        for &style in chunk {
            let key = if style.paint.is_some() {
                bucket_key(style, source_layer, request)
            } else {
                None
            };
            if let Some(k) = key {
                *key_counts.entry(k).or_insert(0) += 1;
            }
            layer_keys.push(key);
        }

        let mut shared_groups: FxHashMap<GeometryBucketKey, Vec<&StyleLayer>> =
            FxHashMap::default();
        for (&style, key) in chunk.iter().zip(layer_keys) {
            if style.paint.is_none() {
                tracing::error!(layer = %style.id, "vector style layer misses a required attribute");
                continue;
            }
            match key {
                Some(k) if key_counts.get(&k).copied().unwrap_or(0) >= 2 => {
                    shared_groups.entry(k).or_default().push(style);
                }
                _ => {
                    processing::process_layer(
                        source_layer,
                        name,
                        shared_props,
                        style,
                        request,
                        context,
                        atlases,
                    )?;
                }
            }
        }

        if let Some(layer) = source_layer {
            for (_key, group) in shared_groups {
                process_shared_group(layer, shared_props, &group, request, context, atlases)?;
            }
        }

        if !index_uses_props {
            shared_layer_props.remove(name);
        }
    }
    Ok(())
}

fn bucket_key(
    style: &StyleLayer,
    source_layer: Option<&tile::Layer>,
    request: &VectorTileRequest,
) -> Option<GeometryBucketKey> {
    let layer = source_layer.filter(|l| !l.features.is_empty())?;
    let edge_y = (1_i32 << u8::from(request.coords.z).min(30)) - 1;
    if request.coords.y <= 0 || request.coords.y >= edge_y {
        return None;
    }
    let style_zoom = request.style_zoom();
    if layer_reads_geometry(style)
        || layer_reads_identity(style)
        || layer_paint_reads_feature_state(style)
        || layer_geometry_reads_feature_state(style)
        || feature_paint_retention(style) != FeaturePaintRetention::None
        || structures::kind(style).is_some()
        || uniform_color(style, style_zoom).is_none()
    {
        return None;
    }
    let _ = layer;
    let paint = style.paint.as_ref()?;
    match paint {
        LayerPaint::Fill(fill) => {
            if fill.outline_color().is_some() || per_feature_pattern(paint).is_some() {
                return None;
            }
            Some(GeometryBucketKey::Fill)
        }
        LayerPaint::Line(line) => {
            if line.may_use_vertex_agl()
                || line.line_gradient.is_some()
                || per_feature_pattern(paint).is_some()
                || LineFeatureStyle::for_paint(line, style_zoom).is_some()
                || LineElevationOptions::for_paint(line, style_zoom).is_some()
                || LineStroke::join_property(style).is_some()
            {
                return None;
            }
            let stroke = LineStroke::of_layer_at_zoom(style, style_zoom);
            let split_corners = line.line_pattern.is_some() || line.line_dasharray.is_some();
            Some(GeometryBucketKey::Line {
                cap: stroke.cap,
                join: stroke.join,
                miter_bits: stroke.miter_limit.to_bits(),
                round_bits: stroke.round_limit.to_bits(),
                split_corners,
            })
        }
        _ => None,
    }
}

fn process_shared_group<T: VectorTransferables, C: crate::io::apc::Context>(
    layer: &tile::Layer,
    shared_props: Option<&[Arc<FeatureProperties>]>,
    group: &[&StyleLayer],
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlases: (&Arc<SymbolAtlas>, &Arc<SymbolAtlas>),
) -> Result<(), ProcessVectorError> {
    let style_zoom = request.style_zoom();
    let state_ctx = context.state_context_for_layer(&layer.name);
    let mut active: Vec<(&StyleLayer, SelectedSet)> = Vec::with_capacity(group.len());
    for &style in group {
        let selected = match &style.filter {
            None => SelectedSet::All,
            Some(raw_filter) => match Filter::parse(raw_filter) {
                Ok(filter) => SelectedSet::Indices(select_filtered_features(
                    layer,
                    &filter,
                    style_zoom,
                    state_ctx.as_ref(),
                    shared_props,
                )),
                Err(error) => {
                    tracing::error!(layer = %style.id, %error, "unsupported filter; the layer renders nothing");
                    context.layer_missing(&request.coords, &layer.name)?;
                    continue;
                }
            },
        };
        active.push((style, selected));
    }

    if active.len() < 2 {
        if let Some((style, _)) = active.first() {
            processing::process_layer(
                Some(layer),
                &layer.name,
                shared_props,
                style,
                request,
                context,
                atlases,
            )?;
        }
        return Ok(());
    }

    let union_indices: Vec<usize> = if active.iter().any(|(_, s)| matches!(s, SelectedSet::All)) {
        (0..layer.features.len()).collect()
    } else {
        let mut merged = Vec::new();
        for (_, sel) in &active {
            if let SelectedSet::Indices(list) = sel {
                merged.extend_from_slice(list);
            }
        }
        merged.sort_unstable();
        merged.dedup();
        merged
    };

    if union_indices.is_empty() {
        for (style, _) in active {
            context.layer_tessellation_finished(
                &request.coords,
                OverAlignedVertexBuffer::empty(),
                Vec::new(),
                (Vec::new(), Vec::new(), Vec::new()),
                layer_header(layer),
                style.id.clone(),
            )?;
        }
        return Ok(());
    }

    let first_style = active[0].0;
    let Some(first_paint) = first_style.paint.as_ref() else {
        return Ok(());
    };
    let Some((mut shared_vertices, shared_indices, spans)) = tessellate_union_with_fallback(
        layer,
        &union_indices,
        shared_props,
        first_style,
        first_paint,
        request,
        state_ctx,
    ) else {
        for _ in active {
            context.layer_missing(&request.coords, &layer.name)?;
        }
        return Ok(());
    };

    let mut per_layer: Vec<(&StyleLayer, Vec<IndexDataType>, Vec<u32>)> =
        Vec::with_capacity(active.len());
    for (style, sel) in &active {
        let mut matched: Vec<&FeatureSpan> = match sel {
            SelectedSet::All => spans.iter().collect(),
            SelectedSet::Indices(list) => intersect_spans(&spans, list),
        };
        if let Some(sort_prop) = sort_key_of(style) {
            if matched.len() > 1 {
                let mut keyed: Vec<(f32, &FeatureSpan)> = matched
                    .into_iter()
                    .map(|span| {
                        let key = eval_sort_key(
                            layer,
                            shared_props,
                            span.feat_idx,
                            &sort_prop,
                            style_zoom,
                        );
                        (key, span)
                    })
                    .collect();
                if !keyed.windows(2).all(|pair| pair[0].0 <= pair[1].0) {
                    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
                }
                matched = keyed.into_iter().map(|(_, span)| span).collect();
            }
        }
        let total_idx: usize = matched.iter().map(|s| s.indices.len()).sum();
        let mut indices = Vec::with_capacity(total_idx);
        let mut feature_indices = Vec::with_capacity(matched.len());
        for span in matched {
            indices.extend_from_slice(&shared_indices[span.indices.clone()]);
            feature_indices.push(span.vtx_count);
        }
        per_layer.push((*style, indices, feature_indices));
    }

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
                std::mem::take(&mut shared_vertices)
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
            style.id.clone(),
        )?;
    }
    Ok(())
}

fn tessellate_union_with_fallback(
    layer: &tile::Layer,
    union_indices: &[usize],
    shared_props: Option<&[Arc<FeatureProperties>]>,
    first_style: &StyleLayer,
    first_paint: &LayerPaint,
    request: &VectorTileRequest,
    state_ctx: Option<crate::vector::tessellation::feature_paint::FeatureStateSyncContext>,
) -> Option<(Vec<ShaderVertex>, Vec<IndexDataType>, Vec<FeatureSpan>)> {
    match tessellate_union(
        layer,
        union_indices,
        shared_props,
        first_style,
        first_paint,
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
            tessellate_union(
                layer,
                &readable,
                shared_props,
                first_style,
                first_paint,
                request,
                state_ctx,
            )
            .ok()
        }
    }
}

fn tessellate_union(
    layer: &tile::Layer,
    slice: &[usize],
    shared_props: Option<&[Arc<FeatureProperties>]>,
    first_style: &StyleLayer,
    first_paint: &LayerPaint,
    request: &VectorTileRequest,
    state_ctx: Option<crate::vector::tessellation::feature_paint::FeatureStateSyncContext>,
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

fn intersect_spans<'a>(spans: &'a [FeatureSpan], sorted_indices: &[usize]) -> Vec<&'a FeatureSpan> {
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

fn eval_sort_key(
    layer: &tile::Layer,
    shared_props: Option<&[Arc<FeatureProperties>]>,
    feat_idx: usize,
    sort_prop: &crate::style::layer::StyleProperty<f32>,
    zoom: f64,
) -> f32 {
    if let Some(props) = shared_props.and_then(|all| all.get(feat_idx)) {
        return sort_prop.evaluate_for(props, zoom).unwrap_or(0.0);
    }
    let Some(feature) = layer.features.get(feat_idx) else {
        return 0.0;
    };
    let props = feature_properties(layer, feature);
    sort_prop.evaluate_for(&props, zoom).unwrap_or(0.0)
}
