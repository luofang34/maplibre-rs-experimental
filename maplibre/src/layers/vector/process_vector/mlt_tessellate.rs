//! Direct columnar MLT tile processing into `ZeroTessellator`, `TextTessellator`, and
//! `IndexProcessor` without intermediate `geozero::mvt::Tile` allocation.

use std::sync::Arc;

use mlt_core::{Decoder, Layer, Parser};
use rustc_hash::FxHashMap;

mod columnar;
use columnar::{tessellate_columnar_symbol, tessellate_columnar_vector, ColumnarLayerContext};

use crate::{
    io::{
        apc::Context,
        geometry_index::{IndexProcessor, LayerIndexConfig},
    },
    sdf::assets::SymbolAtlas,
    style::{
        expression::FeatureProperties,
        filter::{FeatureContext, Filter, GeometryType},
        layer::{LayerPaint, StyleLayer},
    },
    vector::{
        mlt::stream::{decode_columnar_layer, props_for_style, stream_layer_slice, ColumnarLayer},
        process_vector::{
            process_layer_from_mvt, ProcessVectorContext, ProcessVectorError, VectorTileRequest,
        },
        tessellation::feature_paint::FeatureStateSyncContext,
        transferables::{LayerIndexed, VectorTransferables},
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
    let (decoded_layers, decode_order) = decode_wanted_mlt_layers(
        raw_layers,
        &ordered_layers,
        index_all,
        tile_request.coords,
        &mut decoder,
    )?;
    let gpu_atlas = atlas.gpu_upload_handle();
    for style in &ordered_layers {
        process_style_entry(
            &decoded_layers,
            style,
            &tile_request,
            context,
            (&atlas, &gpu_atlas),
        )?;
    }
    if context.index_geometry() {
        emit_mlt_query_index(
            &decoded_layers,
            decode_order,
            index_all,
            &tile_request,
            context,
        )?;
    }
    context.tile_finished(&tile_request.coords)?;
    Ok(())
}

fn decode_wanted_mlt_layers<'a>(
    raw_layers: Vec<Layer<'a>>,
    ordered_layers: &[&StyleLayer],
    index_all: bool,
    coords: crate::coords::WorldTileCoords,
    decoder: &mut Decoder,
) -> Result<(FxHashMap<&'a str, ColumnarLayer<'a>>, Vec<&'a str>), ProcessVectorError> {
    let mut decoded_layers = FxHashMap::default();
    let mut decode_order = Vec::new();
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
        let col = decode_columnar_layer(layer01, decoder).map_err(|err| {
            ProcessVectorError::Decoding {
                coords,
                source: Box::new(err),
            }
        })?;
        decode_order.push(name);
        decoded_layers.insert(name, col);
    }
    Ok((decoded_layers, decode_order))
}

fn process_style_entry<T: VectorTransferables, C: Context>(
    decoded_layers: &FxHashMap<&str, ColumnarLayer<'_>>,
    style: &StyleLayer,
    tile_request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlases: (&Arc<SymbolAtlas>, &Arc<SymbolAtlas>),
) -> Result<(), ProcessVectorError> {
    let Some(layer_name) = style.source_layer.as_deref() else {
        tracing::error!(layer = %style.id, "vector style layer misses a required attribute");
        return Ok(());
    };
    if style.paint.is_none() {
        tracing::error!(layer = %style.id, "vector style layer misses a required attribute");
        return Ok(());
    }
    let Some(col) = decoded_layers.get(layer_name) else {
        return context.layer_missing(&tile_request.coords, layer_name);
    };
    // Stepped extrusion podium bases with `hide_3d` reuse the exact polygon-part unhider.
    if col.has_hide_3d && matches!(style.paint, Some(LayerPaint::FillExtrusion(_))) {
        let mut mvt_decoder = Decoder::default();
        if let Ok(tile_layer) = col.parsed.clone().into_tile(&mut mvt_decoder) {
            let mvt_layer = crate::vector::mlt::convert_layer(&tile_layer);
            return process_layer_from_mvt(
                Some(&mvt_layer),
                layer_name,
                None,
                style,
                tile_request,
                context,
                atlases,
            );
        }
    }
    process_columnar_style_layer(col, layer_name, style, tile_request, context, atlases)
}

fn emit_mlt_query_index<T: VectorTransferables, C: Context>(
    decoded_layers: &FxHashMap<&str, ColumnarLayer<'_>>,
    decode_order: Vec<&str>,
    index_all: bool,
    tile_request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let geometries = if index_all {
        let mut index = IndexProcessor::new();
        for name in decode_order {
            let Some(col) = decoded_layers.get(name) else {
                continue;
            };
            let state_ctx = context.state_context_for_layer(name);
            let props = props_for_style(col, tile_request.coords, state_ctx.as_ref(), false, false)
                .unwrap_or_else(|_| col.base_props.clone());
            index.begin_layer(name, col.ids.clone());
            index.configure_layer(LayerIndexConfig {
                coordinate_scale: col.extent_scale,
                feature_state_ctx: state_ctx,
                shared_properties: props,
            });
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
    context.layer_indexing_finished(&tile_request.coords, source, geometries)
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
    let reads_id = crate::style::layer::layer_reads_identity(style);
    let reads_geom = crate::style::layer::layer_reads_geometry(style);
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
    let Some(selected) = filter_columnar_features(
        col,
        layer_name,
        style,
        request,
        &props,
        state_ctx.as_ref(),
        context,
    )?
    else {
        return Ok(());
    };
    let sel_slice = selected.as_deref();
    let (sliced_ids, sliced_props) = slice_ids_and_props(&col.ids, &props, sel_slice);
    let ctx = ColumnarLayerContext {
        col,
        layer_name,
        style,
        request,
        sel_slice,
        sliced_ids,
        sliced_props,
        state_ctx,
    };
    match style.paint.as_ref() {
        Some(
            paint @ (LayerPaint::Line(_)
            | LayerPaint::Fill(_)
            | LayerPaint::FillExtrusion(_)
            | LayerPaint::Circle(_)
            | LayerPaint::Heatmap(_)),
        ) => tessellate_columnar_vector(ctx, paint, context),
        Some(LayerPaint::Symbol(paint)) => {
            tessellate_columnar_symbol(ctx, paint, props.len(), (layout_atlas, gpu_atlas), context)
        }
        _ => Ok(()),
    }
}

fn filter_columnar_features<T: VectorTransferables, C: Context>(
    col: &ColumnarLayer<'_>,
    layer_name: &str,
    style: &StyleLayer,
    request: &VectorTileRequest,
    props: &[Arc<FeatureProperties>],
    state_ctx: Option<&FeatureStateSyncContext>,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<Option<Option<Vec<usize>>>, ProcessVectorError> {
    let Some(raw_filter) = &style.filter else {
        return Ok(Some(None));
    };
    let filter = match Filter::parse(raw_filter) {
        Ok(filter) => filter,
        Err(error) => {
            tracing::error!(layer = %style.id, %error, "unsupported filter; the layer renders nothing");
            context.layer_missing(&request.coords, layer_name)?;
            return Ok(None);
        }
    };
    let zoom = request.style_zoom();
    let promote = state_ctx.and_then(|c| c.promote_property.as_deref());
    let mut indices = Vec::new();
    for (idx, prop) in props.iter().enumerate() {
        let gtype = col
            .geom_types
            .get(idx)
            .copied()
            .unwrap_or(GeometryType::Unknown);
        let id = crate::style::feature_state::resolve_effective_expr_id(
            col.ids.get(idx).copied().flatten(),
            prop,
            promote,
        );
        if filter.evaluate(&FeatureContext {
            properties: prop,
            geometry_type: gtype,
            id,
            zoom,
        }) {
            indices.push(idx);
        }
    }
    Ok(Some(Some(indices)))
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
