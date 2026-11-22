//! Deduplicates source-layer geometry tessellation and vertex buffers across sibling style layers.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use geozero::mvt::tile;
use lyon::tessellation::VertexBuffers;
use rustc_hash::FxHashMap;

use super::{
    mvt_slice::{decode_layer_properties_bounded, layer_header, select_filtered_features},
    processing, ProcessVectorContext, ProcessVectorError, VectorTileRequest,
};
use crate::{
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
            OverAlignedVertexBuffer,
        },
        transferables::VectorTransferables,
        upload_system::paint::uniform_color,
    },
};

mod fill_outlined;
mod union;
use union::{eval_sort_key, intersect_spans, tessellate_union_with_fallback, FeatureSpan};

static NEXT_SHARED_GEOMETRY_KEY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GeometryBucketKey {
    Fill,
    FillOutlined,
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
    while let Some(first_style) = ordered_layers.get(start) {
        let group_name = first_style.source_layer.as_deref();
        let mut end = start + 1;
        while ordered_layers
            .get(end)
            .is_some_and(|style| style.source_layer.as_deref() == group_name)
        {
            end += 1;
        }
        let chunk = ordered_layers.get(start..end).unwrap_or(&[]);
        start = end;
        let Some(name) = group_name else {
            for style in chunk {
                tracing::error!(layer = %style.id, "vector style layer misses a required attribute");
            }
            continue;
        };
        process_source_layer_group(
            SourceLayerGroupArgs {
                tile,
                name,
                chunk,
                index_uses_props,
                request,
                atlases,
            },
            context,
            shared_layer_props,
        )?;
    }
    Ok(())
}

struct SourceLayerGroupArgs<'a, 'b> {
    tile: &'a geozero::mvt::Tile,
    name: &'b str,
    chunk: &'b [&'b StyleLayer],
    index_uses_props: bool,
    request: &'b VectorTileRequest,
    atlases: (&'b Arc<SymbolAtlas>, &'b Arc<SymbolAtlas>),
}

fn process_source_layer_group<'a, 'b, T: VectorTransferables, C: crate::io::apc::Context>(
    args: SourceLayerGroupArgs<'a, 'b>,
    context: &mut ProcessVectorContext<T, C>,
    shared_layer_props: &mut FxHashMap<&'a str, Option<Vec<Arc<FeatureProperties>>>>,
) -> Result<(), ProcessVectorError> {
    let source_layer = args
        .tile
        .layers
        .iter()
        .find(|layer| layer.name == args.name);
    let (key_counts, layer_keys) =
        collect_group_bucket_keys(args.chunk, source_layer, args.request);
    let need_props = args.index_uses_props
        || (args.chunk.len() > 1
            && layer_keys
                .iter()
                .any(|k| !matches!(k, Some(GeometryBucketKey::FillOutlined))));
    let shared_props = need_props
        .then(|| {
            source_layer.and_then(|layer| {
                shared_layer_props
                    .entry(layer.name.as_str())
                    .or_insert_with(|| {
                        let state_ctx = context.state_context_for_layer(&layer.name);
                        let max_unique = if args.index_uses_props {
                            usize::MAX
                        } else {
                            512
                        };
                        decode_layer_properties_bounded(layer, state_ctx.as_ref(), max_unique)
                    })
                    .as_deref()
            })
        })
        .flatten();
    dispatch_source_layer_groups(
        &args,
        source_layer,
        shared_props,
        (&key_counts, layer_keys),
        context,
    )?;
    if !args.index_uses_props {
        shared_layer_props.remove(args.name);
    }
    Ok(())
}

fn collect_group_bucket_keys(
    chunk: &[&StyleLayer],
    source_layer: Option<&tile::Layer>,
    request: &VectorTileRequest,
) -> (
    FxHashMap<GeometryBucketKey, usize>,
    Vec<Option<GeometryBucketKey>>,
) {
    let mut key_counts: FxHashMap<GeometryBucketKey, usize> = FxHashMap::default();
    let mut layer_keys: Vec<Option<GeometryBucketKey>> = Vec::with_capacity(chunk.len());
    for &style in chunk {
        let key = style
            .paint
            .is_some()
            .then(|| bucket_key(style, source_layer, request))
            .flatten();
        if let Some(k) = key {
            *key_counts.entry(k).or_insert(0) += 1;
        }
        layer_keys.push(key);
    }
    (key_counts, layer_keys)
}

fn dispatch_source_layer_groups<T: VectorTransferables, C: crate::io::apc::Context>(
    args: &SourceLayerGroupArgs<'_, '_>,
    source_layer: Option<&tile::Layer>,
    shared_props: Option<&[Arc<FeatureProperties>]>,
    (key_counts, layer_keys): (
        &FxHashMap<GeometryBucketKey, usize>,
        Vec<Option<GeometryBucketKey>>,
    ),
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let mut shared_groups: FxHashMap<GeometryBucketKey, Vec<&StyleLayer>> = FxHashMap::default();
    for (&style, key) in args.chunk.iter().zip(layer_keys) {
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
                    args.name,
                    shared_props,
                    style,
                    args.request,
                    context,
                    args.atlases,
                )?;
            }
        }
    }
    if let Some(layer) = source_layer {
        for (key, group) in shared_groups {
            if key == GeometryBucketKey::FillOutlined {
                fill_outlined::process_shared_fill_outlined(layer, &group, args.request, context)?;
            } else {
                process_shared_group(
                    layer,
                    shared_props,
                    &group,
                    args.request,
                    context,
                    args.atlases,
                )?;
            }
        }
    }
    Ok(())
}

fn bucket_key(
    style: &StyleLayer,
    source_layer: Option<&tile::Layer>,
    request: &VectorTileRequest,
) -> Option<GeometryBucketKey> {
    source_layer.filter(|l| !l.features.is_empty())?;
    let edge_y = (1_i32 << u8::from(request.coords.z).min(30)) - 1;
    let style_zoom = request.style_zoom();
    if request.coords.y <= 0
        || request.coords.y >= edge_y
        || layer_reads_geometry(style)
        || layer_reads_identity(style)
        || layer_paint_reads_feature_state(style)
        || layer_geometry_reads_feature_state(style)
        || feature_paint_retention(style) != FeaturePaintRetention::None
        || structures::kind(style).is_some()
    {
        return None;
    }
    let paint = style.paint.as_ref()?;
    match paint {
        LayerPaint::Fill(fill) if fill.outline_color().is_none() => {
            (per_feature_pattern(paint).is_none() && uniform_color(style, style_zoom).is_some())
                .then_some(GeometryBucketKey::Fill)
        }
        LayerPaint::Fill(fill) => fill_outlined::qualifies_outlined_fill(style, fill, style_zoom)
            .then_some(GeometryBucketKey::FillOutlined),
        LayerPaint::Line(line)
            if uniform_color(style, style_zoom).is_some()
                && !line.may_use_vertex_agl()
                && line.line_gradient.is_none()
                && per_feature_pattern(paint).is_none()
                && LineFeatureStyle::for_paint(line, style_zoom).is_none()
                && LineElevationOptions::for_paint(line, style_zoom).is_none()
                && LineStroke::join_property(style).is_none() =>
        {
            let stroke = LineStroke::of_layer_at_zoom(style, style_zoom);
            Some(GeometryBucketKey::Line {
                cap: stroke.cap,
                join: stroke.join,
                miter_bits: stroke.miter_limit.to_bits(),
                round_bits: stroke.round_limit.to_bits(),
                split_corners: line.line_pattern.is_some() || line.line_dasharray.is_some(),
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
    let active = collect_active_selections(
        layer,
        shared_props,
        group,
        style_zoom,
        state_ctx.as_ref(),
        request.coords,
        context,
    )?;
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
    let union_indices = union_selected_indices(layer.features.len(), &active);
    if union_indices.is_empty() {
        for (style, _) in active {
            context.layer_tessellation_finished(
                &request.coords,
                OverAlignedVertexBuffer::empty(),
                Vec::new(),
                (Vec::new(), Vec::new(), Vec::new()),
                layer_header(layer),
                style,
            )?;
        }
        return Ok(());
    }
    let Some((first_style, _)) = active.first() else {
        return Ok(());
    };
    let Some(first_paint) = first_style.paint.as_ref() else {
        return Ok(());
    };
    let Some((mut shared_vertices, shared_indices, spans)) = tessellate_union_with_fallback(
        layer,
        &union_indices,
        shared_props,
        (first_style, first_paint),
        request,
        state_ctx,
    ) else {
        for _ in active {
            context.layer_missing(&request.coords, &layer.name)?;
        }
        return Ok(());
    };
    let per_layer = build_shared_layer_indices(
        layer,
        shared_props,
        &active,
        &spans,
        &shared_indices,
        style_zoom,
    );
    drop(shared_indices);
    drop(spans);
    emit_shared_group_layers(layer, per_layer, &mut shared_vertices, request, context)
}

fn collect_active_selections<'a, T: VectorTransferables, C: crate::io::apc::Context>(
    layer: &tile::Layer,
    shared_props: Option<&[Arc<FeatureProperties>]>,
    group: &[&'a StyleLayer],
    style_zoom: f64,
    state_ctx: Option<&super::super::tessellation::feature_paint::FeatureStateSyncContext>,
    coords: crate::coords::WorldTileCoords,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<Vec<(&'a StyleLayer, SelectedSet)>, ProcessVectorError> {
    let mut active = Vec::with_capacity(group.len());
    for &style in group {
        let selected = match &style.filter {
            None => SelectedSet::All,
            Some(raw_filter) => match Filter::parse(raw_filter) {
                Ok(filter) => SelectedSet::Indices(select_filtered_features(
                    layer,
                    &filter,
                    style_zoom,
                    state_ctx,
                    shared_props,
                )),
                Err(error) => {
                    tracing::error!(layer = %style.id, %error, "unsupported filter; the layer renders nothing");
                    context.layer_missing(&coords, &layer.name)?;
                    continue;
                }
            },
        };
        active.push((style, selected));
    }
    Ok(active)
}

fn union_selected_indices(
    feature_count: usize,
    active: &[(&StyleLayer, SelectedSet)],
) -> Vec<usize> {
    if active.iter().any(|(_, s)| matches!(s, SelectedSet::All)) {
        return (0..feature_count).collect();
    }
    let mut merged = Vec::new();
    for (_, sel) in active {
        if let SelectedSet::Indices(list) = sel {
            merged.extend_from_slice(list);
        }
    }
    merged.sort_unstable();
    merged.dedup();
    merged
}

fn build_shared_layer_indices<'a>(
    layer: &tile::Layer,
    shared_props: Option<&[Arc<FeatureProperties>]>,
    active: &[(&'a StyleLayer, SelectedSet)],
    spans: &[FeatureSpan],
    shared_indices: &[IndexDataType],
    style_zoom: f64,
) -> Vec<(&'a StyleLayer, Vec<IndexDataType>, Vec<u32>)> {
    let mut per_layer = Vec::with_capacity(active.len());
    for (style, sel) in active {
        let mut matched: Vec<&FeatureSpan> = match sel {
            SelectedSet::All => spans.iter().collect(),
            SelectedSet::Indices(list) => intersect_spans(spans, list),
        };
        if let Some(sort_prop) = sort_key_of(style) {
            if matched.len() > 1 {
                let mut keyed: Vec<(f32, &FeatureSpan)> = matched
                    .into_iter()
                    .map(|span| {
                        (
                            eval_sort_key(
                                layer,
                                shared_props,
                                span.feat_idx,
                                &sort_prop,
                                style_zoom,
                            ),
                            span,
                        )
                    })
                    .collect();
                if !keyed
                    .windows(2)
                    .all(|pair| matches!(pair, [a, b] if a.0 <= b.0))
                {
                    keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
                }
                matched = keyed.into_iter().map(|(_, span)| span).collect();
            }
        }
        let total_idx: usize = matched.iter().map(|s| s.indices.len()).sum();
        let mut indices = Vec::with_capacity(total_idx);
        let mut feature_indices = Vec::with_capacity(matched.len());
        for span in matched {
            if let Some(slice) = shared_indices.get(span.indices.clone()) {
                indices.extend_from_slice(slice);
            }
            feature_indices.push(span.vtx_count);
        }
        per_layer.push((*style, indices, feature_indices));
    }
    per_layer
}

fn emit_shared_group_layers<T: VectorTransferables, C: crate::io::apc::Context>(
    layer: &tile::Layer,
    per_layer: Vec<(&StyleLayer, Vec<IndexDataType>, Vec<u32>)>,
    shared_vertices: &mut Vec<crate::render::ShaderVertex>,
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
