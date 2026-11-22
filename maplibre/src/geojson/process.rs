//! Per-layer GeoJSON filtering and tessellation dispatch.

use std::borrow::Cow;

use geozero::FeatureProcessor as _;

use super::{
    filter_geojson, geometry_property, index, query_index, tessellator, value_reader,
    GeoJsonTileRequest, ProcessGeoJsonError, ProjectingTessellator,
};
use crate::{
    coords::WorldTileCoords,
    io::apc::Context,
    sdf::tessellation::TextTessellator,
    style::{
        filter::Filter,
        layer::{LayerPaint, StyleLayer},
    },
    vector::transferables::{
        LayerMissing, LayerTessellated, SymbolLayerTessellated, TileTessellated,
        VectorTransferables,
    },
};

/// Processes every matching style layer of `geojson_value` for a tile and emits completion.
pub(super) fn process_geojson_features<T: VectorTransferables, C: Context>(
    geojson_value: &serde_json::Value,
    request: GeoJsonTileRequest,
    context: &C,
) -> Result<(), ProcessGeoJsonError> {
    let coords = request.coords;
    let promoted = request.promote_id.as_deref();
    let with_geometry = request
        .layers
        .iter()
        .any(geometry_property::reads_feature_inputs)
        .then(|| {
            geometry_property::with_geometry_property(geojson_value, promoted, request.generate_id)
        });

    for style_layer in &request.layers {
        process_style_layer::<T, C>(
            geojson_value,
            with_geometry.as_ref(),
            style_layer,
            &request,
            context,
        )?;
    }

    query_index::send::<T, C>(
        geojson_value,
        (
            coords,
            &request.source_name,
            request.buffer,
            promoted,
            request.generate_id,
        ),
        context,
    )?;
    context
        .send_back(T::TileTessellated::build_from(coords))
        .map_err(ProcessGeoJsonError::SendError)?;

    Ok(())
}

fn process_style_layer<T: VectorTransferables, C: Context>(
    geojson_value: &serde_json::Value,
    with_geometry: Option<&serde_json::Value>,
    style_layer: &StyleLayer,
    request: &GeoJsonTileRequest,
    context: &C,
) -> Result<(), ProcessGeoJsonError> {
    let matches_source = style_layer
        .source
        .as_deref()
        .is_some_and(|source| source == request.source_name);
    if !matches_source {
        return Ok(());
    }
    let Some(paint) = &style_layer.paint else {
        log::warn!("GeoJSON style layer {} has no paint", style_layer.id);
        return Ok(());
    };
    let source_value = match with_geometry {
        Some(annotated) if geometry_property::reads_feature_inputs(style_layer) => annotated,
        _ => geojson_value,
    };
    let Some(filtered) =
        filter_layer_geojson::<T, C>(source_value, style_layer, request.coords, context)?
    else {
        return Ok(());
    };

    match paint {
        LayerPaint::Fill(_)
        | LayerPaint::FillExtrusion(_)
        | LayerPaint::Line(_)
        | LayerPaint::Background(_)
        | LayerPaint::Circle(_)
        | LayerPaint::Heatmap(_) => {
            process_vector_layer::<T, C>(&filtered, paint, style_layer, request, context)
        }
        LayerPaint::Symbol(symbol_paint) => {
            process_symbol_layer::<T, C>(&filtered, symbol_paint, style_layer, request, context)
        }
        _ => {
            log::trace!(
                "GeoJSON layer {} has unsupported paint type, skipping",
                style_layer.id
            );
            Ok(())
        }
    }
}

fn filter_layer_geojson<'a, T: VectorTransferables, C: Context>(
    source_value: &'a serde_json::Value,
    style_layer: &StyleLayer,
    coords: WorldTileCoords,
    context: &C,
) -> Result<Option<Cow<'a, serde_json::Value>>, ProcessGeoJsonError> {
    let Some(raw_filter) = &style_layer.filter else {
        return Ok(Some(Cow::Borrowed(source_value)));
    };
    match Filter::parse(raw_filter) {
        Ok(filter) => Ok(Some(Cow::Owned(filter_geojson(
            source_value,
            &filter,
            f64::from(u8::from(coords.z)),
        )))),
        Err(error) => {
            tracing::error!(
                layer = %style_layer.id,
                %error,
                "unsupported filter; the layer renders nothing"
            );
            context
                .send_back(T::LayerMissing::build_from(coords, style_layer.id.clone()))
                .map_err(ProcessGeoJsonError::SendError)?;
            Ok(None)
        }
    }
}

fn process_vector_layer<T: VectorTransferables, C: Context>(
    filtered: &serde_json::Value,
    paint: &LayerPaint,
    style_layer: &StyleLayer,
    request: &GeoJsonTileRequest,
    context: &C,
) -> Result<(), ProcessGeoJsonError> {
    let coords = request.coords;
    let promoted = request.promote_id.as_deref();
    let mut tessellator = tessellator::build_tessellator(
        paint,
        style_layer,
        coords,
        &request.projection,
        (request.style_zoom(), request.overscaling(), request.buffer),
    );
    tessellator.source_ids = index::feature_ids_with(filtered, promoted, request.generate_id);
    let mut projecting = ProjectingTessellator::new(coords, tessellator);

    if let Err(e) = value_reader::process_geojson_value(filtered, &mut projecting) {
        log::warn!(
            "GeoJSON tessellation for layer {} failed: {e:?}",
            style_layer.id
        );
        return context
            .send_back(T::LayerMissing::build_from(coords, style_layer.id.clone()))
            .map_err(ProcessGeoJsonError::SendError);
    }

    let mut inner = projecting.into_inner();
    // For bare GeoJSON geometries (Polygon, LineString, etc. — not a FeatureCollection),
    // value_reader never calls `feature_end`, so `feature_indices` stays empty while
    // `buffer.indices` is not. Manually commit the remaining geometry as a single feature.
    if inner.feature_indices.is_empty() && !inner.buffer.indices.is_empty() {
        drop(inner.feature_end(0));
    }
    inner.apply_sort_keys();

    let synthetic_layer = geozero::mvt::tile::Layer {
        version: 2,
        name: style_layer.id.clone(),
        ..Default::default()
    };
    let sort_keys = inner.sort_key_values();
    context
        .send_back(
            T::LayerTessellated::build_from_with_paint(crate::vector::TessellatedLayerPayload {
                coords,
                buffer: inner.buffer.into(),
                feature_indices: inner.feature_indices,
                feature_colors: inner.feature_colors,
                feature_sort_keys: sort_keys,
                layer_data: synthetic_layer,
                style_layer_id: style_layer.id.clone(),
                entries: inner.feature_paint,
            })
            .with_overscaled_zoom(request.overscaled_zoom),
        )
        .map_err(ProcessGeoJsonError::SendError)
}

fn process_symbol_layer<T: VectorTransferables, C: Context>(
    filtered: &serde_json::Value,
    symbol_paint: &crate::style::layer::SymbolPaint,
    style_layer: &StyleLayer,
    request: &GeoJsonTileRequest,
    context: &C,
) -> Result<(), ProcessGeoJsonError> {
    let coords = request.coords;
    let zoom = request.style_zoom();
    let atlas = request
        .atlas
        .clone()
        .unwrap_or_else(crate::sdf::assets::fallback_atlas);
    let mut tessellator = TextTessellator::with_assets(symbol_paint.clone(), zoom, atlas.clone());
    tessellator.overscaling = request.overscaling();
    tessellator.source_ids =
        index::feature_ids_with(filtered, request.promote_id.as_deref(), request.generate_id);
    let mut projecting = ProjectingTessellator::new(coords, tessellator);

    if let Err(e) = value_reader::process_geojson_value(filtered, &mut projecting) {
        log::warn!(
            "GeoJSON text tessellation for layer {} failed: {e:?}",
            style_layer.id
        );
        return context
            .send_back(T::LayerMissing::build_from(coords, style_layer.id.clone()))
            .map_err(ProcessGeoJsonError::SendError);
    }

    let mut inner = projecting.into_inner();
    // A bare geometry never reaches feature_end, so its symbol is committed here.
    if inner.features.is_empty() {
        drop(inner.feature_end(0));
    }
    inner.finish();

    let synthetic_layer = geozero::mvt::tile::Layer {
        version: 2,
        name: style_layer.id.clone(),
        ..Default::default()
    };
    context
        .send_back(T::SymbolLayerTessellated::build_from(
            coords,
            inner.quad_buffer.into(),
            inner.features,
            Some(atlas),
            synthetic_layer,
            style_layer.id.clone(),
        ))
        .map_err(ProcessGeoJsonError::SendError)
}
