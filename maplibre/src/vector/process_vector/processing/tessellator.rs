use super::*;

pub(crate) fn tessellator(
    paint: &LayerPaint,
    request: &VectorTileRequest,
) -> ZeroTessellator<IndexDataType> {
    let style_zoom = request.style_zoom();
    let mut tessellator = init_tessellator(paint, request, style_zoom)
        .with_feature_opacity(paint.opacity(), style_zoom);
    configure_tessellator_paint(&mut tessellator, paint, request, style_zoom);
    tessellator
}

fn init_tessellator(
    paint: &LayerPaint,
    request: &VectorTileRequest,
    style_zoom: f64,
) -> ZeroTessellator<IndexDataType> {
    let zoom = u8::from(request.coords.z);
    let granularity = match paint {
        LayerPaint::Fill(_) => granularity_for_zoom(128, 2, zoom),
        LayerPaint::Line(_) => granularity_for_zoom(128, 0, zoom),
        _ => 1,
    };
    let last_tile = crate::coords::ZOOM_BOUNDS
        .get(usize::from(zoom))
        .map_or_else(
            || (1_i64 << zoom.min(30)) - 1,
            |&bound| i64::from(bound) - 1,
        );
    match paint {
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
        _ if request.projection.uses_globe_rendering(style_zoom) => ZeroTessellator::default()
            .with_globe_subdivision(
                granularity,
                zoom == 0,
                request.coords.y == 0,
                i64::from(request.coords.y) == last_tile,
            ),
        _ => ZeroTessellator::default(),
    }
}

fn configure_tessellator_paint(
    tessellator: &mut ZeroTessellator<IndexDataType>,
    paint: &LayerPaint,
    request: &VectorTileRequest,
    style_zoom: f64,
) {
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
            tessellator.style_property = paint.fill_extrusion_color.clone();
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
                crate::vector::tessellation::LineFeatureStyle::for_paint(paint, style_zoom);
            tessellator.line_elevation =
                crate::vector::tessellation::LineElevationOptions::for_paint(paint, style_zoom);
        }
        _ => {}
    }
}
