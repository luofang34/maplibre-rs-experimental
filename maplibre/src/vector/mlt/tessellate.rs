//! Direct columnar MLT tile processing into `ZeroTessellator`, `TextTessellator`, and
//! `IndexProcessor` without intermediate `geozero::mvt::Tile` allocation.

use std::sync::Arc;

use mlt_core::{Decoder, Layer, LendingIterator, Parser};
use rustc_hash::FxHashMap;

use super::stream::{
    decode_columnar_layer, geometry_type_of, prop_ref_to_value, props_for_style,
    stream_layer_slice, ColumnarLayer,
};
use crate::{
    io::{
        apc::Context,
        geometry_index::{IndexProcessor, IndexedGeometry},
    },
    sdf::{assets::SymbolAtlas, tessellation::TextTessellator},
    style::{
        expression::{empty_feature_properties, FeatureProperties, Value},
        filter::{FeatureContext, Filter, GeometryType},
        layer::{LayerPaint, StyleLayer},
        source::PromoteId,
    },
    vector::{
        process_vector::{
            build_zero_tessellator, process_layer_from_mvt, ProcessVectorContext,
            ProcessVectorError, VectorTileRequest,
        },
        tessellation::feature_paint::FeatureStateSyncContext,
        transferables::{DefaultSymbolLayerTessellated, LayerIndexed, VectorTransferables},
    },
};

pub(crate) fn process_mlt_tile_with_assets<T: VectorTransferables, C: Context>(
    data: &[u8],
    tile_request: VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: Arc<SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    context.set_overscaled_zoom(tile_request.overscaled_zoom);
    let raw_layers =
        Parser::default()
            .parse_layers(data)
            .map_err(|err| ProcessVectorError::Decoding {
                coords: tile_request.coords,
                source: Box::new(err),
            })?;
    let index_all = context.index_geometry() && T::LayerIndexed::requires_geometry_index();

    let mut ordered_layers: Vec<&StyleLayer> = tile_request.layers.iter().collect();
    ordered_layers.sort_by(|a, b| {
        a.source_layer
            .as_deref()
            .unwrap_or("")
            .cmp(b.source_layer.as_deref().unwrap_or(""))
            .then_with(|| a.index.cmp(&b.index))
    });

    let mut decoder = Decoder::default();
    let mut decoded_layers: FxHashMap<&str, ColumnarLayer<'_>> = FxHashMap::default();
    let mut decode_order: Vec<&str> = Vec::new();
    for layer in raw_layers {
        let Layer::Tag01(layer01) = layer else {
            continue;
        };
        let name = layer01.name();
        let wanted = index_all
            || ordered_layers
                .iter()
                .any(|sl| sl.source_layer.as_deref() == Some(name));
        if !wanted {
            continue;
        }
        let col = decode_columnar_layer(layer01, &mut decoder).map_err(|err| {
            ProcessVectorError::Decoding {
                coords: tile_request.coords,
                source: Box::new(err),
            }
        })?;
        decode_order.push(name);
        decoded_layers.insert(name, col);
    }

    let gpu_atlas = atlas.gpu_upload_handle();
    for style in &ordered_layers {
        let Some(layer_name) = style.source_layer.as_deref() else {
            tracing::error!(layer = %style.id, "vector style layer misses a required attribute");
            continue;
        };
        if style.paint.is_none() {
            tracing::error!(layer = %style.id, "vector style layer misses a required attribute");
            continue;
        }
        let Some(col) = decoded_layers.get(layer_name) else {
            context.layer_missing(&tile_request.coords, layer_name)?;
            continue;
        };

        // Stepped extrusion podium bases with `hide_3d` reuse the exact polygon-part unhider.
        if col.has_hide_3d && matches!(style.paint, Some(LayerPaint::FillExtrusion(_))) {
            let mut mvt_decoder = Decoder::default();
            if let Ok(tile_layer) = col.parsed.clone().into_tile(&mut mvt_decoder) {
                let mvt_layer = super::convert_layer(&tile_layer);
                process_layer_from_mvt(
                    Some(&mvt_layer),
                    layer_name,
                    None,
                    style,
                    &tile_request,
                    context,
                    (&atlas, &gpu_atlas),
                )?;
                continue;
            }
        }

        process_columnar_style_layer(
            col,
            layer_name,
            style,
            &tile_request,
            context,
            (&atlas, &gpu_atlas),
        )?;
    }

    if context.index_geometry() {
        let geometries = if index_all {
            let mut index = IndexProcessor::new();
            for name in decode_order {
                let Some(col) = decoded_layers.get(name) else {
                    continue;
                };
                index.set_coordinate_scale(col.extent_scale);
                let state_ctx = context.state_context_for_layer(name);
                let props =
                    props_for_style(col, tile_request.coords, state_ctx.as_ref(), false, false)
                        .unwrap_or_else(|_| col.base_props.clone());
                index.set_feature_state_context(state_ctx);
                index.begin_layer(name, col.ids.clone());
                index.set_shared_properties(props);
                if let Err(error) = stream_layer_slice(col, None, &mut index) {
                    tracing::warn!(coords = %tile_request.coords, layer = %name, ?error, "skipping MLT query index for layer");
                }
            }
            index.get_geometries()
        } else {
            Vec::new()
        };
        let source = tile_request
            .layers
            .iter()
            .find_map(|layer| layer.source.clone());
        context.layer_indexing_finished(&tile_request.coords, source, geometries)?;
    }
    context.tile_finished(&tile_request.coords)?;
    Ok(())
}

fn process_columnar_style_layer<T: VectorTransferables, C: Context>(
    col: &ColumnarLayer<'_>,
    layer_name: &str,
    style: &StyleLayer,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    (layout_atlas, gpu_atlas): (&Arc<SymbolAtlas>, &Arc<SymbolAtlas>),
) -> Result<(), ProcessVectorError> {
    let state_ctx = context.state_context_for_layer(layer_name);
    let reads_id = crate::style::expression::layer_reads_identity(style);
    let reads_geom = crate::style::expression::layer_reads_geometry(style);
    let props = props_for_style(
        col,
        request.coords,
        state_ctx.as_ref(),
        reads_id,
        reads_geom,
    )
    .map_err(|err| ProcessVectorError::Decoding {
        coords: request.coords,
        source: Box::new(err),
    })?;

    let selected: Option<Vec<usize>> = match &style.filter {
        Some(raw_filter) => match Filter::parse(raw_filter) {
            Ok(filter) => {
                let zoom = request.style_zoom();
                let mut indices = Vec::new();
                for (idx, prop) in props.iter().enumerate() {
                    let gtype = col
                        .geom_types
                        .get(idx)
                        .copied()
                        .unwrap_or(GeometryType::Unknown);
                    let id = col
                        .ids
                        .get(idx)
                        .copied()
                        .flatten()
                        .map(|v| Value::Number(v as f64));
                    if filter.evaluate(&FeatureContext {
                        properties: prop,
                        geometry_type: gtype,
                        id,
                        zoom,
                    }) {
                        indices.push(idx);
                    }
                }
                Some(indices)
            }
            Err(error) => {
                tracing::error!(layer = %style.id, %error, "unsupported filter; the layer renders nothing");
                context.layer_missing(&request.coords, layer_name)?;
                return Ok(());
            }
        },
        None => None,
    };

    let sel_slice = selected.as_deref();
    let (sliced_ids, sliced_props) = slice_ids_and_props(&col.ids, &props, sel_slice);

    match style.paint.as_ref() {
        Some(
            paint @ (LayerPaint::Line(_)
            | LayerPaint::Fill(_)
            | LayerPaint::FillExtrusion(_)
            | LayerPaint::Circle(_)
            | LayerPaint::Heatmap(_)),
        ) => {
            #[cfg(target_arch = "wasm32")]
            let uniform_paint =
                crate::vector::upload_system::paint::uniform_color(style, request.style_zoom())
                    .is_some()
                    && style.type_ != "circle"
                    && !crate::style::feature_state::layer_paint_reads_feature_state(style);

            let mut tessellator = build_zero_tessellator(paint, request);
            tessellator.set_layer_strokes(style);
            tessellator.coordinate_scale = col.extent_scale;
            tessellator.source_ids = sliced_ids;
            tessellator.shared_properties = sliced_props;
            tessellator.feature_state_ctx = state_ctx;
            tessellator.feature_paint_retention =
                crate::vector::feature_paint::feature_paint_retention(style);
            tessellator.sort_key = crate::vector::tessellation::SortKeys::by(
                crate::vector::tessellation::sort_key_of(style),
            );
            #[cfg(target_arch = "wasm32")]
            if uniform_paint
                && tessellator.pattern_property.is_none()
                && tessellator.outline_property.is_none()
                && !tessellator.line_gradient
            {
                tessellator.omit_feature_colors = true;
            }
            if let Err(error) = stream_layer_slice(col, sel_slice, &mut tessellator) {
                tracing::error!(coords = %request.coords, layer = %layer_name, ?error, "MLT vector tessellation failed");
                return context.layer_missing(&request.coords, layer_name);
            }
            tessellator.apply_sort_keys();
            let mut sort_keys = tessellator.sort_key_values();
            #[cfg(target_arch = "wasm32")]
            if uniform_paint && tessellator.pattern_property.is_none() && !tessellator.line_gradient
            {
                tessellator.feature_colors = Vec::new();
            }
            tessellator.buffer.vertices.shrink_to_fit();
            tessellator.buffer.indices.shrink_to_fit();
            tessellator.feature_indices.shrink_to_fit();
            tessellator.feature_colors.shrink_to_fit();
            sort_keys.shrink_to_fit();
            tessellator.feature_paint.shrink_to_fit();
            context.layer_tessellation_finished(
                &request.coords,
                tessellator.buffer.into(),
                tessellator.feature_indices,
                (
                    tessellator.feature_colors,
                    sort_keys,
                    tessellator.feature_paint,
                ),
                col.header.clone(),
                style.id.clone(),
            )
        }
        Some(LayerPaint::Symbol(paint)) => {
            let mut tessellator = TextTessellator::with_assets(
                paint.clone(),
                request.style_zoom(),
                layout_atlas.clone(),
            );
            tessellator.coordinate_scale = col.extent_scale;
            tessellator.overscaling = request.overscaling();
            tessellator.source_ids = sliced_ids;
            tessellator.shared_properties = sliced_props;
            tessellator.feature_state_ctx = state_ctx;
            if let Err(error) = stream_layer_slice(col, sel_slice, &mut tessellator) {
                tracing::error!(coords = %request.coords, layer = %layer_name, ?error, "MLT symbol tessellation failed");
                return context.layer_missing(&request.coords, layer_name);
            }
            tessellator.finish();
            tessellator.quad_buffer.vertices.shrink_to_fit();
            tessellator.quad_buffer.indices.shrink_to_fit();
            tessellator.features.shrink_to_fit();
            let has_geometry = !tessellator.quad_buffer.indices.is_empty();
            context.symbol_layer_tessellation_finished(DefaultSymbolLayerTessellated {
                coords: request.coords,
                buffer: tessellator.quad_buffer.into(),
                features: tessellator.features,
                atlas: has_geometry.then(|| gpu_atlas.clone()),
                layer_data: col.header.clone(),
                style_layer_id: style.id.clone(),
            })
        }
        _ => Ok(()),
    }
}

fn slice_ids_and_props(
    ids: &[Option<u64>],
    props: &[Arc<FeatureProperties>],
    selected: Option<&[usize]>,
) -> (Vec<Option<u64>>, Vec<Arc<FeatureProperties>>) {
    match selected {
        Some(indices) => (
            indices
                .iter()
                .map(|&idx| ids.get(idx).copied().flatten())
                .collect(),
            indices
                .iter()
                .filter_map(|&idx| props.get(idx).cloned())
                .collect(),
        ),
        None => (ids.to_vec(), props.to_vec()),
    }
}

pub(crate) fn index_mlt_bytes(
    data: &[u8],
    promote_id: Option<&PromoteId>,
    feature_state: Option<&Arc<crate::style::feature_state::SourceFeatureState>>,
) -> Vec<IndexedGeometry<f64>> {
    let Ok(raw_layers) = Parser::default().parse_layers(data) else {
        return Vec::new();
    };
    let mut decoder = Decoder::default();
    let mut index = IndexProcessor::new();
    for layer in raw_layers {
        let Layer::Tag01(layer01) = layer else {
            continue;
        };
        let Ok(col) = decode_columnar_layer(layer01, &mut decoder) else {
            continue;
        };
        let name = col.parsed.name();
        let state_ctx =
            (promote_id.is_some() || feature_state.is_some()).then(|| FeatureStateSyncContext {
                promote_property: promote_id
                    .and_then(|p| p.property_for(Some(name)))
                    .map(str::to_owned),
                source_state: feature_state.cloned(),
                source_layer: Some(name.to_owned()),
            });
        let props = if let Some(ctx) = &state_ctx {
            let mut synced = Vec::with_capacity(col.base_props.len());
            for (idx, base) in col.base_props.iter().enumerate() {
                let mut map = (**base).clone();
                crate::style::feature_state::sync_properties_feature_state(
                    &mut map,
                    col.ids.get(idx).copied().flatten(),
                    ctx.promote_property.as_deref(),
                    ctx.source_state.as_deref(),
                    ctx.source_layer.as_deref(),
                );
                synced.push(Arc::new(map));
            }
            synced
        } else {
            col.base_props.clone()
        };
        index.set_coordinate_scale(col.extent_scale);
        index.set_feature_state_context(state_ctx);
        index.begin_layer(name, col.ids.clone());
        index.set_shared_properties(props);
        let _ = stream_layer_slice(&col, None, &mut index);
    }
    index.get_geometries()
}

pub(crate) fn visit_mlt_features(
    data: &[u8],
    keep_layer: impl Fn(&str) -> bool,
    mut visit: impl FnMut(&str, Option<u64>, GeometryType, &FeatureProperties),
) {
    let Ok(raw_layers) = Parser::default().parse_layers(data) else {
        return;
    };
    let mut decoder = Decoder::default();
    let empty = empty_feature_properties();
    let mut scratch = FeatureProperties::new();
    for layer in raw_layers {
        let Layer::Tag01(layer01) = layer else {
            continue;
        };
        let name = layer01.name();
        if !keep_layer(name) {
            continue;
        }
        let Ok(parsed) = layer01.decode_all(&mut decoder) else {
            continue;
        };
        let col_names: Vec<String> = parsed.iterate_prop_names().map(|n| n.to_string()).collect();
        let mut iter = parsed.iter_features();
        while let Some(Ok(feat)) = iter.next() {
            scratch.clear();
            for (col_idx, opt_val) in feat.iter_all_properties().enumerate() {
                if let (Some(col_name), Some(val)) = (col_names.get(col_idx), opt_val) {
                    scratch.insert(col_name.clone(), prop_ref_to_value(val));
                }
            }
            let props_ref: &FeatureProperties = if scratch.is_empty() { empty } else { &scratch };
            visit(
                name,
                feat.id(),
                geometry_type_of(feat.geometry()),
                props_ref,
            );
        }
    }
}
