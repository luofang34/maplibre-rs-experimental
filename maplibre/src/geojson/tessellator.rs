//! Builds configured [`ZeroTessellator`] instances for GeoJSON style layers.

use crate::{
    coords::{WorldTileCoords, EXTENT},
    projection::{globe::subdivision::granularity_for_zoom, ProjectionType},
    style::layer::{LayerPaint, StyleLayer},
    vector::tessellation::{
        sharp_corner_offset, sort_key_of, CircleOptions, ExtrusionOptions, IndexDataType,
        LineElevationOptions, LineFeatureStyle, SortKeys, ZeroTessellator,
    },
};

pub(super) fn build_tessellator(
    paint: &LayerPaint,
    style_layer: &StyleLayer,
    coords: WorldTileCoords,
    projection: &ProjectionType,
    (style_zoom, overscaling, buffer): (f64, f64, Option<u32>),
) -> ZeroTessellator<IndexDataType> {
    let zoom = u8::from(coords.z);
    let granularity = match paint {
        LayerPaint::Fill(_) => granularity_for_zoom(128, 2, zoom),
        LayerPaint::Line(_) => granularity_for_zoom(512, 0, zoom),
        _ => 1,
    };
    let use_globe_geometry = projection.uses_globe_rendering(f64::from(zoom));
    let mut tessellator = match paint {
        LayerPaint::Circle(circle) => {
            ZeroTessellator::<IndexDataType>::default().with_circles(CircleOptions {
                grid: use_globe_geometry
                    && circle.circle_pitch_alignment
                        == crate::style::circle::CirclePitchAlignment::Map,
                ..CircleOptions::for_paint(circle, style_zoom)
            })
        }
        LayerPaint::Heatmap(heatmap) => {
            ZeroTessellator::<IndexDataType>::default().with_circles(CircleOptions {
                grid: use_globe_geometry,
                ..CircleOptions::for_heatmap(heatmap, style_zoom)
            })
        }
        LayerPaint::FillExtrusion(extrusion) => {
            let defaults = ExtrusionOptions::for_paint(extrusion, overscaling);
            let clip_buffer = defaults.clip_buffer.or_else(|| {
                buffer
                    .filter(|_| extrusion.fill_extrusion_pattern.is_some())
                    .map(|buf| buf as f32 * EXTENT as f32 / 512.0)
            });
            let tessellator =
                ZeroTessellator::<IndexDataType>::default().with_extrusion(ExtrusionOptions {
                    clip_buffer,
                    ..defaults
                });
            let min_grid = if extrusion.may_use_vertex_agl() {
                32
            } else {
                2
            };
            if use_globe_geometry {
                let last_tile = i64::from(crate::coords::ZOOM_BOUNDS[usize::from(zoom)]) - 1;
                tessellator.with_globe_subdivision(
                    granularity_for_zoom(128, min_grid, zoom),
                    zoom == 0,
                    coords.y == 0,
                    i64::from(coords.y) == last_tile,
                )
            } else if extrusion.may_use_vertex_agl() {
                tessellator.with_globe_subdivision(32, false, false, false)
            } else {
                tessellator
            }
        }
        _ if use_globe_geometry => {
            let last_tile = i64::from(crate::coords::ZOOM_BOUNDS[usize::from(zoom)]) - 1;
            ZeroTessellator::<IndexDataType>::default().with_globe_subdivision(
                granularity,
                zoom == 0,
                coords.y == 0,
                i64::from(coords.y) == last_tile,
            )
        }
        _ => ZeroTessellator::<IndexDataType>::default(),
    }
    .with_feature_opacity(paint.opacity(), style_zoom);
    tessellator.sort_key = SortKeys::by(sort_key_of(style_layer));
    match paint {
        LayerPaint::Fill(p) if p.fill_pattern.is_some() => {
            tessellator.fallback_color = [1.0; 4];
            tessellator.pattern_property = crate::vector::pattern::per_feature_pattern(paint);
        }
        LayerPaint::Fill(p) => {
            tessellator.style_property = p.fill_color.clone();
            tessellator.outline_property = p.outline_color();
        }
        LayerPaint::FillExtrusion(p) if p.fill_extrusion_pattern.is_some() => {
            tessellator.fallback_color = [1.0; 4];
            tessellator.pattern_property = crate::vector::pattern::per_feature_pattern(paint);
        }
        LayerPaint::FillExtrusion(p) => {
            tessellator.style_property = p.fill_extrusion_color.clone();
        }
        LayerPaint::Circle(p) => tessellator.style_property = p.circle_color.clone(),
        LayerPaint::Line(p) => {
            tessellator.pattern_property = crate::vector::pattern::per_feature_pattern(paint);
            tessellator.style_property = p.line_color.clone();
            tessellator.is_line_layer = true;
            tessellator.line_gradient = p.line_gradient.is_some();
            tessellator.split_corners =
                p.line_gradient.is_some() || p.line_pattern.is_some() || p.line_dasharray.is_some();
            tessellator.line_feature_style = LineFeatureStyle::for_paint(p, style_zoom);
            tessellator.line_elevation = LineElevationOptions::for_paint(p, style_zoom);
            tessellator.sharp_corner_offset = sharp_corner_offset(overscaling);
            tessellator.set_layer_strokes(style_layer);
        }
        LayerPaint::Background(p) => {
            tessellator.style_property = p.background_color.clone();
        }
        _ => {}
    }
    tessellator
}
