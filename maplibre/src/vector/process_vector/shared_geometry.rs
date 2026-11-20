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

        let need_props = index_uses_props
            || (chunk.len() > 1
                && layer_keys
                    .iter()
                    .any(|k| !matches!(k, Some(GeometryBucketKey::FillOutlined))));
        let shared_props = if !need_props {
            None
        } else {
            source_layer.and_then(|layer| {
                shared_layer_props
                    .entry(layer.name.as_str())
                    .or_insert_with(|| {
                        let state_ctx = context.state_context_for_layer(&layer.name);
                        let max_unique = if index_uses_props { usize::MAX } else { 512 };
                        decode_layer_properties_bounded(layer, state_ctx.as_ref(), max_unique)
                    })
                    .as_deref()
            })
        };

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
            for (key, group) in shared_groups {
                if key == GeometryBucketKey::FillOutlined {
                    fill_outlined::process_shared_fill_outlined(layer, &group, request, context)?;
                } else {
                    process_shared_group(layer, shared_props, &group, request, context, atlases)?;
                }
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
    let _ = source_layer.filter(|l| !l.features.is_empty())?;
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
                style,
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
        (first_style, first_paint),
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

    drop(shared_indices);
    drop(spans);

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
            style,
        )?;
    }
    Ok(())
}
