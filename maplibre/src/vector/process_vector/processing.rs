//! Applies one style entry to its source layer, borrowing `tile::Layer` without cloning unless
//! geometry/identity/extrusion tag injection is needed.
use std::{borrow::Cow, sync::Arc};

use super::{
    mvt_slice::{
        layer_header, process_mvt_slice, retain_readable_indices, select_filtered_features,
        shared_props_for_slice, source_ids_for_slice,
    },
    *,
};

pub(super) fn process_layer<T: VectorTransferables, C: Context>(
    source_layer: Option<&tile::Layer>,
    layer_name: &str,
    predecoded_props: Option<&[Arc<FeatureProperties>]>,
    style: &StyleLayer,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    let empty_layer;
    let mut layer: Cow<'_, tile::Layer> = match source_layer {
        Some(existing) => Cow::Borrowed(existing),
        None => {
            empty_layer = tile::Layer {
                version: 2,
                name: layer_name.to_owned(),
                extent: Some(4096),
                ..Default::default()
            };
            Cow::Borrowed(&empty_layer)
        }
    };
    if crate::geojson::reads_geometry(style) && !layer.features.is_empty() {
        super::geometry_tags::add_geometry_tags(layer.to_mut(), request.coords);
    }
    if crate::geojson::reads_identity(style) && !layer.features.is_empty() {
        super::geometry_tags::add_identity_tags(layer.to_mut());
    }
    if matches!(style.paint, Some(LayerPaint::FillExtrusion(_)))
        && layer.keys.iter().any(|key| key == "hide_3d")
    {
        unhide_unreplaced_building_bases(layer.to_mut());
    }
    let shared_props = matches!(layer, Cow::Borrowed(_))
        .then_some(predecoded_props)
        .flatten();
    let selected = match &style.filter {
        Some(raw_filter) => match Filter::parse(raw_filter) {
            Ok(filter) => {
                let state_ctx = context.state_context_for_layer(&layer.name);
                Some(select_filtered_features(
                    layer.as_ref(),
                    &filter,
                    request.style_zoom(),
                    state_ctx.as_ref(),
                    shared_props,
                ))
            }
            Err(error) => {
                // Rendering either all features or none silently would conceal an invalid style.
                tracing::error!(layer = %style.id,%error,"unsupported filter; the layer renders nothing");
                context.layer_missing(&request.coords, &layer.name)?;
                return Ok(());
            }
        },
        None => None,
    };
    match style.paint.as_ref() {
        Some(
            paint @ (LayerPaint::Line(_)
            | LayerPaint::Fill(_)
            | LayerPaint::FillExtrusion(_)
            | LayerPaint::Circle(_)
            | LayerPaint::Heatmap(_)),
        ) => vector_layer(
            layer.as_ref(),
            selected.as_deref(),
            shared_props,
            style,
            paint,
            request,
            context,
        ),
        Some(LayerPaint::Symbol(paint)) => symbol_layer(
            layer.as_ref(),
            selected.as_deref(),
            style,
            paint,
            request,
            context,
            atlas,
        ),
        _ => {
            tracing::warn!(layer = %style.id,"unhandled vector style layer type");
            Ok(())
        }
    }
}

fn tessellator(paint: &LayerPaint, request: &VectorTileRequest) -> ZeroTessellator<IndexDataType> {
    let zoom = u8::from(request.coords.z);
    let style_zoom = request.style_zoom();
    let granularity = match paint {
        LayerPaint::Fill(_) => granularity_for_zoom(128, 2, zoom),
        LayerPaint::Line(_) => granularity_for_zoom(512, 0, zoom),
        _ => 1,
    };
    let mut tessellator = match paint {
        LayerPaint::Circle(circle) => ZeroTessellator::default().with_circles(CircleOptions {
            // A circle lying on the map follows the globe's curve.
            grid: request.projection.uses_globe_rendering(style_zoom)
                && circle.circle_pitch_alignment == crate::style::circle::CirclePitchAlignment::Map,
            ..CircleOptions::for_paint(circle, style_zoom)
        }),
        LayerPaint::Heatmap(heatmap) => ZeroTessellator::default().with_circles(CircleOptions {
            grid: request.projection.uses_globe_rendering(style_zoom),
            ..CircleOptions::for_heatmap(heatmap, style_zoom)
        }),
        LayerPaint::FillExtrusion(extrusion) => {
            let tessellator = ZeroTessellator::default().with_extrusion(
                ExtrusionOptions::for_paint(extrusion, request.overscaling()),
            );
            let min_grid = if extrusion.may_use_vertex_agl() {
                32
            } else {
                2
            };
            if request.projection.uses_globe_rendering(style_zoom) {
                // Roofs and the edges of walls follow the sphere as fills and lines do.
                let last_tile = i64::from(crate::coords::ZOOM_BOUNDS[usize::from(zoom)]) - 1;
                tessellator.with_globe_subdivision(
                    granularity_for_zoom(128, min_grid, zoom),
                    zoom == 0,
                    request.coords.y == 0,
                    i64::from(request.coords.y) == last_tile,
                )
            } else if extrusion.may_use_vertex_agl() {
                tessellator.with_globe_subdivision(32, false, false, false)
            } else {
                tessellator
            }
        }
        _ if request.projection.uses_globe_rendering(style_zoom) => {
            let last_tile = i64::from(crate::coords::ZOOM_BOUNDS[usize::from(zoom)]) - 1;
            ZeroTessellator::default().with_globe_subdivision(
                granularity,
                zoom == 0,
                request.coords.y == 0,
                i64::from(request.coords.y) == last_tile,
            )
        }
        _ => ZeroTessellator::default(),
    }
    .with_feature_opacity(paint.opacity(), style_zoom);
    match paint {
        // An image repeated over a fill takes nothing from the fill's colour but its opacity.
        LayerPaint::Fill(paint) if paint.fill_pattern.is_some() => {
            tessellator.fallback_color = [1.0; 4];
            tessellator.pattern_property =
                crate::vector::pattern::per_feature_pattern(&LayerPaint::Fill(paint.clone()));
        }
        LayerPaint::Fill(paint) => {
            tessellator.style_property = paint.fill_color.clone();
            tessellator.outline_property = paint.outline_color();
        }
        LayerPaint::FillExtrusion(paint) if paint.fill_extrusion_pattern.is_some() => {
            tessellator.fallback_color = [1.0; 4];
            tessellator.pattern_property = crate::vector::pattern::per_feature_pattern(
                &LayerPaint::FillExtrusion(paint.clone()),
            );
        }
        LayerPaint::FillExtrusion(paint) => {
            tessellator.style_property = paint.fill_extrusion_color.clone()
        }
        LayerPaint::Circle(paint) => tessellator.style_property = paint.circle_color.clone(),
        LayerPaint::Line(paint) => {
            tessellator.pattern_property =
                crate::vector::pattern::per_feature_pattern(&LayerPaint::Line(paint.clone()));
            tessellator.style_property = paint.line_color.clone();
            tessellator.is_line_layer = true;
            tessellator.line_gradient = paint.line_gradient.is_some();
            tessellator.sharp_corner_offset =
                crate::vector::tessellation::sharp_corner_offset(request.overscaling());
            tessellator.split_corners = paint.line_gradient.is_some()
                || paint.line_pattern.is_some()
                || paint.line_dasharray.is_some();
            tessellator.line_feature_style =
                super::super::tessellation::LineFeatureStyle::for_paint(paint, style_zoom);
            tessellator.line_elevation =
                super::super::tessellation::LineElevationOptions::for_paint(paint, style_zoom);
        }
        _ => {}
    }
    tessellator
}

fn vector_layer<T: VectorTransferables, C: Context>(
    layer: &tile::Layer,
    selected: Option<&[usize]>,
    shared_props: Option<&[Arc<FeatureProperties>]>,
    style: &StyleLayer,
    paint: &LayerPaint,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let state_ctx = context.state_context_for_layer(&layer.name);
    let build = |slice: Option<&[usize]>| {
        let mut tessellator = tessellator(paint, request);
        tessellator.set_layer_strokes(style);
        tessellator.coordinate_scale = extent_scale(layer);
        tessellator.source_ids = source_ids_for_slice(layer, slice);
        tessellator.shared_properties = shared_props_for_slice(shared_props, slice);
        tessellator.feature_state_ctx = state_ctx.clone();
        tessellator.feature_paint_retention =
            crate::vector::feature_paint::feature_paint_retention(style);
        tessellator.sort_key = crate::vector::tessellation::SortKeys::by(
            crate::vector::tessellation::sort_key_of(style),
        );
        process_mvt_slice(layer, slice, shared_props.is_some(), &mut tessellator)?;
        Ok(tessellator)
    };
    let Some(mut tessellator) = tessellate_readable(layer, selected, request.coords, build) else {
        return context.layer_missing(&request.coords, &layer.name);
    };
    tessellator.apply_sort_keys();
    let sort_keys = tessellator.sort_key_values();
    context.layer_tessellation_finished(
        &request.coords,
        tessellator.buffer.into(),
        tessellator.feature_indices,
        (
            tessellator.feature_colors,
            sort_keys,
            tessellator.feature_paint,
        ),
        layer_header(layer),
        style.id.clone(),
    )
}

/// Tessellates the layer with `build`. When a feature's geometry cannot be read, the features
/// whose geometry cannot are left out and the rest are tessellated, so one malformed feature
/// does not take the layer with it; `None` when the layer cannot be tessellated even so.
fn tessellate_readable<Tessellator>(
    layer: &tile::Layer,
    selected: Option<&[usize]>,
    coords: WorldTileCoords,
    mut build: impl FnMut(Option<&[usize]>) -> geozero::error::Result<Tessellator>,
) -> Option<Tessellator> {
    let error = match build(selected) {
        Ok(tessellator) => return Some(tessellator),
        Err(error) => error,
    };
    let (readable, dropped) = retain_readable_indices(layer, selected);
    if dropped == 0 {
        tracing::error!(%coords, layer = %layer.name, ?error, "vector tessellation failed");
        return None;
    }
    tracing::warn!(%coords, layer = %layer.name, dropped, ?error, "features whose geometry cannot be read are left out");
    match build(Some(&readable)) {
        Ok(tessellator) => Some(tessellator),
        Err(error) => {
            tracing::error!(%coords, layer = %layer.name, ?error, "vector tessellation failed");
            None
        }
    }
}

fn symbol_layer<T: VectorTransferables, C: Context>(
    layer: &tile::Layer,
    selected: Option<&[usize]>,
    style: &StyleLayer,
    paint: &crate::style::layer::SymbolPaint,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    let zoom = request.style_zoom();
    let state_ctx = context.state_context_for_layer(&layer.name);
    let build = |slice: Option<&[usize]>| {
        let mut tessellator = TextTessellator::with_assets(paint.clone(), zoom, atlas.clone());
        tessellator.coordinate_scale = extent_scale(layer);
        tessellator.overscaling = request.overscaling();
        tessellator.source_ids = source_ids_for_slice(layer, slice);
        tessellator.feature_state_ctx = state_ctx.clone();
        process_mvt_slice(layer, slice, false, &mut tessellator)?;
        Ok(tessellator)
    };
    let Some(mut tessellator) = tessellate_readable(layer, selected, request.coords, build) else {
        return context.layer_missing(&request.coords, &layer.name);
    };
    tessellator.finish();
    let has_geometry = !tessellator.quad_buffer.indices.is_empty();
    context.symbol_layer_tessellation_finished(
        crate::vector::transferables::DefaultSymbolLayerTessellated {
            coords: request.coords,
            buffer: tessellator.quad_buffer.into(),
            features: tessellator.features,
            atlas: has_geometry.then_some(atlas),
            layer_data: layer_header(layer),
            style_layer_id: style.id.clone(),
        },
    )
}

mod building_bases;
use building_bases::unhide_unreplaced_building_bases;
