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

pub(crate) fn process_layer<T: VectorTransferables, C: Context>(
    source_layer: Option<&tile::Layer>,
    layer_name: &str,
    predecoded_props: Option<&[Arc<FeatureProperties>]>,
    style: &StyleLayer,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlases: (
        &std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
        &std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
    ),
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
    if crate::style::expression::layer_reads_geometry(style) && !layer.features.is_empty() {
        super::geometry_tags::add_geometry_tags(layer.to_mut(), request.coords);
    }
    if crate::style::expression::layer_reads_identity(style) && !layer.features.is_empty() {
        super::geometry_tags::add_identity_tags(layer.to_mut());
    }
    if matches!(style.paint, Some(LayerPaint::FillExtrusion(_)))
        && layer.keys.iter().any(|key| key == "hide_3d")
    {
        unhide_unreplaced_building_bases(&mut layer);
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
            shared_props,
            style,
            paint,
            request,
            context,
            atlases,
        ),
        _ => {
            tracing::warn!(layer = %style.id,"unhandled vector style layer type");
            Ok(())
        }
    }
}

pub(crate) fn tessellator(
    paint: &LayerPaint,
    request: &VectorTileRequest,
) -> ZeroTessellator<IndexDataType> {
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

fn allow_prepack_for_style_inner(
    coords: WorldTileCoords,
    has_dem_source: bool,
    style: &StyleLayer,
    has_sort_keys: bool,
    has_shared_geometry: bool,
) -> bool {
    let per_feature_pattern = style
        .paint
        .as_ref()
        .and_then(crate::vector::pattern::per_feature_pattern)
        .is_some();
    let edge_y = (1_i32 << u8::from(coords.z).min(30)) - 1;
    let is_polar_row = coords.y <= 0 || coords.y >= edge_y;
    let structure_kind = crate::vector::structures::kind(style);
    !(is_polar_row && style.type_ == "fill")
        && !per_feature_pattern
        && !has_sort_keys
        && style.type_ != "circle"
        && if has_dem_source {
            structure_kind.is_none()
        } else {
            structure_kind
                .is_none_or(|k| matches!(k, crate::vector::structures::StructureKind::Extrusion))
        }
        && !has_shared_geometry
}

pub(super) fn allow_prepack_for_style(
    coords: WorldTileCoords,
    has_dem_source: bool,
    style: &StyleLayer,
    has_sort_keys: bool,
    has_shared_geometry: bool,
) -> bool {
    !cfg!(test)
        && allow_prepack_for_style_inner(
            coords,
            has_dem_source,
            style,
            has_sort_keys,
            has_shared_geometry,
        )
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
    #[cfg(any(target_arch = "wasm32", not(test)))]
    let uniform_paint =
        crate::vector::upload_system::paint::uniform_color(style, request.style_zoom()).is_some()
            && style.type_ != "circle"
            && !crate::style::feature_state::layer_paint_reads_feature_state(style);
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
        #[cfg(any(target_arch = "wasm32", not(test)))]
        if uniform_paint && tessellator.pattern_property.is_none() && !tessellator.line_gradient {
            tessellator.omit_feature_colors = true;
        }
        if context.allow_prepack_for_style(
            request.coords,
            style,
            tessellator.sort_key.is_keyed(),
            false,
        ) {
            tessellator.maybe_enable_streaming_prepack(style.type_.as_str());
        }
        process_mvt_slice(layer, slice, shared_props.is_some(), &mut tessellator)?;
        Ok(tessellator)
    };
    let Some(mut tessellator) = tessellate_readable(layer, selected, request.coords, build) else {
        return context.layer_missing(&request.coords, &layer.name);
    };
    tessellator.apply_sort_keys();
    let mut sort_keys = tessellator.sort_key_values();
    #[cfg(any(target_arch = "wasm32", not(test)))]
    if uniform_paint && tessellator.pattern_property.is_none() && !tessellator.line_gradient {
        tessellator.feature_colors = Vec::new();
    }
    let mut feature_indices = std::mem::take(&mut tessellator.feature_indices);
    let mut feature_colors = std::mem::take(&mut tessellator.feature_colors);
    let mut feature_paint = std::mem::take(&mut tessellator.feature_paint);
    feature_indices.shrink_to_fit();
    feature_colors.shrink_to_fit();
    sort_keys.shrink_to_fit();
    feature_paint.shrink_to_fit();
    let buffer = tessellator.into_over_aligned_buffer();
    context.layer_tessellation_finished(
        &request.coords,
        buffer,
        feature_indices,
        (feature_colors, sort_keys, feature_paint),
        layer_header(layer),
        style,
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

#[allow(clippy::too_many_arguments)]
fn symbol_layer<T: VectorTransferables, C: Context>(
    layer: &tile::Layer,
    selected: Option<&[usize]>,
    shared_props: Option<&[Arc<FeatureProperties>]>,
    style: &StyleLayer,
    paint: &crate::style::layer::SymbolPaint,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    (layout_atlas, gpu_atlas): (
        &std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
        &std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
    ),
) -> Result<(), ProcessVectorError> {
    let zoom = request.style_zoom();
    let state_ctx = context.state_context_for_layer(&layer.name);
    let promote_prop = state_ctx
        .as_ref()
        .and_then(|ctx| ctx.promote_property.as_deref());
    let stream_point = !context.index_geometry()
        && !crate::sdf::tessellation::is_line_placed(paint)
        && !paint.properties.contains_key("symbol-sort-key");
    let build = |slice: Option<&[usize]>| {
        let mut tessellator =
            TextTessellator::with_assets(paint.clone(), zoom, layout_atlas.clone());
        tessellator.coordinate_scale = extent_scale(layer);
        tessellator.overscaling = request.overscaling();
        tessellator.source_ids = source_ids_for_slice(layer, slice);
        tessellator.source_indices =
            crate::sdf::tessellation::source_feature_indices(layer.features.len(), slice);
        tessellator.shared_properties = shared_props_for_slice(shared_props, slice);
        tessellator.feature_state_ctx = state_ctx.clone();
        if stream_point {
            tessellator.enable_streaming_point_layout(
                crate::style::expression::symbol_runtime_referenced_keys(style, promote_prop),
            );
        }
        process_mvt_slice(layer, slice, shared_props.is_some(), &mut tessellator)?;
        Ok(tessellator)
    };
    let Some(mut tessellator) = tessellate_readable(layer, selected, request.coords, build) else {
        return context.layer_missing(&request.coords, &layer.name);
    };
    tessellator.finish();
    if !context.index_geometry() && !tessellator.streamed_point_layout() {
        crate::sdf::tessellation::prune_symbol_features(
            &mut tessellator.features,
            style,
            promote_prop,
        );
    }
    tessellator.quad_buffer.vertices.shrink_to_fit();
    tessellator.quad_buffer.indices.shrink_to_fit();
    tessellator.features.shrink_to_fit();
    let has_geometry = !tessellator.quad_buffer.indices.is_empty();
    context.symbol_layer_tessellation_finished(
        crate::vector::transferables::DefaultSymbolLayerTessellated {
            coords: request.coords,
            buffer: tessellator.quad_buffer.into(),
            features: tessellator.features,
            atlas: has_geometry.then(|| gpu_atlas.clone()),
            layer_data: layer_header(layer),
            style_layer_id: style.id.clone(),
        },
    )
}

mod building_bases;
use building_bases::unhide_unreplaced_building_bases;

#[cfg(test)]
mod tests {
    use super::allow_prepack_for_style_inner;
    use crate::{coords::WorldTileCoords, style::layer::StyleLayer};

    fn parse_layer(json: serde_json::Value) -> Option<StyleLayer> {
        serde_json::from_value(json).ok()
    }

    #[test]
    fn terrain_prepacks_draped_layers_but_keeps_structures_unpacked() {
        let mid_tile = WorldTileCoords::from((5, 5, 4_u8.into()));
        let fill = parse_layer(serde_json::json!({
            "id": "water", "type": "fill", "paint": {"fill-color": "#38bdf8"}
        }));
        let line = parse_layer(serde_json::json!({
            "id": "roads", "type": "line", "paint": {"line-color": "#64748b", "line-width": 2}
        }));
        let extrusion = parse_layer(serde_json::json!({
            "id": "bld", "type": "fill-extrusion", "paint": {"fill-extrusion-color": "#cbd5e1"}
        }));
        let circle = parse_layer(serde_json::json!({
            "id": "pts", "type": "circle", "paint": {"circle-radius": 4}
        }));
        let bridge = parse_layer(serde_json::json!({
            "id": "bridge", "type": "line",
            "metadata": {"maplibre-rs:terrain-structure": "bridge"},
            "paint": {"line-color": "#64748b", "line-width": 2}
        }));

        assert!(
            fill.is_some()
                && line.is_some()
                && extrusion.is_some()
                && circle.is_some()
                && bridge.is_some()
        );
        if let (Some(fill), Some(line), Some(extrusion), Some(circle), Some(bridge)) =
            (fill, line, extrusion, circle, bridge)
        {
            assert!(allow_prepack_for_style_inner(
                mid_tile, true, &fill, false, false
            ));
            assert!(allow_prepack_for_style_inner(
                mid_tile, true, &line, false, false
            ));
            assert!(!allow_prepack_for_style_inner(
                mid_tile, true, &extrusion, false, false
            ));
            assert!(allow_prepack_for_style_inner(
                mid_tile, false, &extrusion, false, false
            ));
            assert!(!allow_prepack_for_style_inner(
                mid_tile, true, &circle, false, false
            ));
            assert!(!allow_prepack_for_style_inner(
                mid_tile, true, &bridge, false, false
            ));
        }
    }
}
