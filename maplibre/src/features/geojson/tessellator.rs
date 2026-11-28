//! Builds configured [`ZeroTessellator`] instances for GeoJSON style layers.

use crate::{
    coords::{WorldTileCoords, EXTENT},
    projection::{globe::subdivision::granularity_for_zoom, ProjectionType},
    style::layer::{LayerPaint, StyleLayer},
    vector::tessellation::{
        sharp_corner_offset, sort_key_of, CircleOptions, ExtrusionOptions, IndexDataType,
        LineElevationOptions, LineFeatureStyle, SortKeys, ZeroTessellator,
        ZeroTessellatorPaintConfig, ZeroTessellatorTileInput,
    },
};

fn with_optional_globe_subdivision(
    tessellator: ZeroTessellator<IndexDataType>,
    use_globe_geometry: bool,
    granularity: u32,
    coords: WorldTileCoords,
) -> ZeroTessellator<IndexDataType> {
    if !use_globe_geometry {
        return tessellator;
    }
    let zoom = u8::from(coords.z);
    let bound = crate::coords::ZOOM_BOUNDS
        .get(usize::from(zoom))
        .copied()
        .unwrap_or_else(|| 1_u32 << zoom);
    let last_tile = i64::from(bound) - 1;
    tessellator.with_globe_subdivision(
        granularity,
        zoom == 0,
        coords.y == 0,
        i64::from(coords.y) == last_tile,
    )
}

fn build_base_tessellator(
    paint: &LayerPaint,
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
    match paint {
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
            let clip_buffer = defaults
                .clip_buffer
                .or_else(|| Some(buffer.unwrap_or(128) as f32 * EXTENT as f32 / 512.0));
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
                with_optional_globe_subdivision(
                    tessellator,
                    true,
                    granularity_for_zoom(128, min_grid, zoom),
                    coords,
                )
            } else if extrusion.may_use_vertex_agl() {
                tessellator.with_globe_subdivision(32, false, false, false)
            } else {
                tessellator
            }
        }
        _ => with_optional_globe_subdivision(
            ZeroTessellator::<IndexDataType>::default(),
            use_globe_geometry,
            granularity,
            coords,
        ),
    }
    .with_feature_opacity(paint.opacity(), style_zoom)
}

pub(super) fn build_tessellator(
    paint: &LayerPaint,
    style_layer: &StyleLayer,
    coords: WorldTileCoords,
    projection: &ProjectionType,
    (style_zoom, overscaling, buffer): (f64, f64, Option<u32>),
    source_ids: Vec<Option<u64>>,
) -> ZeroTessellator<IndexDataType> {
    let mut tessellator =
        build_base_tessellator(paint, coords, projection, (style_zoom, overscaling, buffer));
    let mut cfg = ZeroTessellatorPaintConfig::default();
    let mut is_line = false;
    match paint {
        LayerPaint::Fill(p) if p.fill_pattern.is_some() => {
            cfg.fallback_color = [1.0; 4];
            cfg.pattern_property = crate::vector::pattern::per_feature_pattern(paint);
        }
        LayerPaint::Fill(p) => {
            cfg.style_property = p.fill_color.clone();
            cfg.outline_property = p.outline_color();
        }
        LayerPaint::FillExtrusion(p) if p.fill_extrusion_pattern.is_some() => {
            cfg.fallback_color = [1.0; 4];
            cfg.pattern_property = crate::vector::pattern::per_feature_pattern(paint);
        }
        LayerPaint::FillExtrusion(p) => {
            cfg.style_property = p.fill_extrusion_color.clone();
        }
        LayerPaint::Circle(p) => cfg.style_property = p.circle_color.clone(),
        LayerPaint::Line(p) => {
            is_line = true;
            cfg.pattern_property = crate::vector::pattern::per_feature_pattern(paint);
            cfg.style_property = p.line_color.clone();
            cfg.is_line_layer = true;
            cfg.line_gradient = p.line_gradient.is_some();
            cfg.split_corners =
                p.line_gradient.is_some() || p.line_pattern.is_some() || p.line_dasharray.is_some();
            cfg.line_feature_style = LineFeatureStyle::for_paint(p, style_zoom);
            cfg.line_elevation = LineElevationOptions::for_paint(p, style_zoom);
            cfg.sharp_corner_offset = sharp_corner_offset(overscaling);
        }
        LayerPaint::Background(p) => {
            cfg.style_property = p.background_color.clone();
        }
        _ => {}
    }
    tessellator.apply_paint_config(cfg);
    if is_line {
        tessellator.set_layer_strokes(style_layer);
    }
    tessellator.configure_tile_input(ZeroTessellatorTileInput {
        source_ids,
        sort_key: SortKeys::by(sort_key_of(style_layer)),
        feature_paint_retention: crate::vector::feature_paint::feature_paint_retention(style_layer),
        ..Default::default()
    });
    tessellator
}
