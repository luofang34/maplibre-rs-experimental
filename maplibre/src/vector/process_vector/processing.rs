//! Applies one style entry to an isolated copy of its source layer.
use super::*;

pub(super) fn process_layer<T: VectorTransferables, C: Context>(
    mut layer: tile::Layer,
    style: &StyleLayer,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    if let Some(filter) = &style.filter {
        match Filter::parse(filter) {
            Ok(filter) => {
                apply_filter_to_layer(&mut layer, &filter, f64::from(u8::from(request.coords.z)))
            }
            Err(error) => {
                // Rendering either all features or none silently would conceal an invalid style.
                tracing::error!(layer = %style.id,%error,"unsupported filter; the layer renders nothing");
                context.layer_missing(&request.coords, &layer.name)?;
                return Ok(());
            }
        }
    }
    match style.paint.as_ref() {
        Some(
            paint @ (LayerPaint::Line(_)
            | LayerPaint::Fill(_)
            | LayerPaint::FillExtrusion(_)
            | LayerPaint::Circle(_)
            | LayerPaint::Heatmap(_)),
        ) => vector_layer(layer, style, paint, request, context),
        Some(LayerPaint::Symbol(paint)) => {
            symbol_layer(layer, style, paint, request, context, atlas)
        }
        _ => {
            tracing::warn!(layer = %style.id,"unhandled vector style layer type");
            Ok(())
        }
    }
}

fn tessellator(paint: &LayerPaint, request: &VectorTileRequest) -> ZeroTessellator<IndexDataType> {
    let zoom = u8::from(request.coords.z);
    let granularity = match paint {
        LayerPaint::Fill(_) => granularity_for_zoom(128, 2, zoom),
        LayerPaint::Line(_) => granularity_for_zoom(512, 0, zoom),
        _ => 1,
    };
    let mut tessellator = match paint {
        LayerPaint::Circle(circle) => ZeroTessellator::default()
            .with_circles(CircleOptions::for_paint(circle, f64::from(zoom))),
        LayerPaint::Heatmap(heatmap) => ZeroTessellator::default()
            .with_circles(CircleOptions::for_heatmap(heatmap, f64::from(zoom))),
        LayerPaint::FillExtrusion(extrusion) => {
            ZeroTessellator::default().with_extrusion(ExtrusionOptions::for_paint(extrusion))
        }
        _ if request.projection.uses_globe_rendering(f64::from(zoom)) => {
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
    .with_feature_opacity(paint.opacity(), f64::from(zoom));
    match paint {
        // An image repeated over a fill takes nothing from the fill's colour but its opacity.
        LayerPaint::Fill(paint) if paint.fill_pattern.is_some() => {
            tessellator.fallback_color = [1.0; 4]
        }
        LayerPaint::Fill(paint) => {
            tessellator.style_property = paint.fill_color.clone();
            tessellator.outline_property = paint.outline_color(f64::from(zoom));
        }
        LayerPaint::FillExtrusion(paint) if paint.fill_extrusion_pattern.is_some() => {
            tessellator.fallback_color = [1.0; 4]
        }
        LayerPaint::FillExtrusion(paint) => {
            tessellator.style_property = paint.fill_extrusion_color.clone()
        }
        LayerPaint::Circle(paint) => tessellator.style_property = paint.circle_color.clone(),
        LayerPaint::Line(paint) => {
            tessellator.style_property = paint.line_color.clone();
            tessellator.is_line_layer = true;
            tessellator.line_gradient = paint.line_gradient.is_some();
            tessellator.line_feature_style =
                super::super::tessellation::LineFeatureStyle::for_paint(paint, f64::from(zoom));
        }
        _ => {}
    }
    tessellator
}

fn vector_layer<T: VectorTransferables, C: Context>(
    mut layer: tile::Layer,
    style: &StyleLayer,
    paint: &LayerPaint,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
) -> Result<(), ProcessVectorError> {
    let original = layer.clone();
    let mut tessellator = tessellator(paint, request);
    tessellator.stroke = crate::style::line_stroke::LineStroke::of_layer(style);
    tessellator.coordinate_scale = extent_scale(&layer);
    tessellator.sort_key =
        crate::vector::tessellation::SortKeys::by(crate::vector::tessellation::sort_key_of(style));
    match layer.process(&mut tessellator) {
        Err(error) => {
            context.layer_missing(&request.coords, &layer.name)?;
            tracing::error!(coords = %request.coords, layer = %layer.name, ?error,"vector tessellation failed");
        }
        Ok(()) => {
            tessellator.apply_sort_keys();
            context.layer_tessellation_finished(
                &request.coords,
                tessellator.buffer.into(),
                tessellator.feature_indices,
                tessellator.feature_colors,
                original,
                style.id.clone(),
            )?
        }
    }
    Ok(())
}

fn symbol_layer<T: VectorTransferables, C: Context>(
    mut layer: tile::Layer,
    style: &StyleLayer,
    paint: &crate::style::layer::SymbolPaint,
    request: &VectorTileRequest,
    context: &mut ProcessVectorContext<T, C>,
    atlas: std::sync::Arc<crate::sdf::assets::SymbolAtlas>,
) -> Result<(), ProcessVectorError> {
    let original = layer.clone();
    let zoom = f64::from(u8::from(request.coords.z));
    let mut tessellator = TextTessellator::with_assets(paint.clone(), zoom, atlas.clone());
    tessellator.coordinate_scale = extent_scale(&layer);
    tessellator.source_ids = layer.features.iter().map(|feature| feature.id).collect();
    match layer.process(&mut tessellator) {
        Err(error) => {
            context.layer_missing(&request.coords, &layer.name)?;
            tracing::error!(coords = %request.coords, layer = %layer.name, ?error,"symbol tessellation failed");
        }
        Ok(()) => {
            tessellator.finish();
            context.symbol_layer_tessellation_finished(
                crate::vector::transferables::DefaultSymbolLayerTessellated {
                    coords: request.coords,
                    buffer: tessellator.quad_buffer.into(),
                    features: tessellator.features,
                    atlas: Some(atlas),
                    layer_data: original,
                    style_layer_id: style.id.clone(),
                },
            )?;
        }
    }
    Ok(())
}
