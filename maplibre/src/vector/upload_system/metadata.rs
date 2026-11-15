use super::VectorPaintFrame;
use crate::{
    coords::WorldTileCoords,
    render::shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
    style::{
        circle::{CirclePitchAlignment, CirclePitchScale},
        layer::{LayerPaint, StyleLayer},
    },
};

pub(super) fn metadata_for_layer(
    style_layer: &StyleLayer,
    coords: WorldTileCoords,
    frame: VectorPaintFrame,
) -> ShaderLayerMetadata {
    let VectorPaintFrame {
        zoom,
        bearing,
        light,
    } = frame;
    // Extract line-width from style paint (default 1.0px)
    let line_width = match &style_layer.paint {
        Some(LayerPaint::Line(paint)) => paint
            .line_width
            .as_ref()
            .and_then(|w| w.evaluate_at_zoom(f64::from(zoom)))
            .unwrap_or(1.0),
        _ => 1.0,
    };
    let translate = layer_translate_tile_units(style_layer.paint.as_ref(), coords.z, zoom, bearing);
    let mut layer_metadata =
        ShaderLayerMetadata::new(style_layer.index as f32, line_width, translate);
    if let Some(LayerPaint::Heatmap(paint)) = &style_layer.paint {
        let zoom = f64::from(zoom);
        // Radius and intensity are one value per layer, taken at the view zoom.
        layer_metadata.line_width = if paint.radius_per_feature() {
            1.0
        } else {
            paint.radius_at(zoom)
        };
        layer_metadata.circle_params = [paint.intensity_at(zoom), 1.0, 0.0, 0.0];
    }
    if let Some(LayerPaint::Line(paint)) = &style_layer.paint {
        let value = |property: &Option<crate::style::layer::StyleProperty<f32>>| {
            property
                .as_ref()
                .and_then(|property| property.evaluate_at_zoom(f64::from(zoom)))
                .unwrap_or(0.0)
        };
        let flags = u8::from(paint.line_gradient.is_some())
            | (u8::from(paint.line_elevation.is_some()) << 1);
        layer_metadata.circle_params = [
            value(&paint.line_offset),
            value(&paint.line_gap_width).max(0.0),
            value(&paint.line_blur).max(0.0),
            f32::from(flags),
        ];
    }
    if let Some(LayerPaint::Circle(paint)) = &style_layer.paint {
        let zoom = f64::from(zoom);
        layer_metadata.stroke_color = paint.stroke_color_at(zoom);
        // Fill opacity is already folded into each feature's colour alpha.
        layer_metadata.circle_params =
            [1.0, paint.stroke_opacity_at(zoom), paint.blur_at(zoom), 0.0];
        layer_metadata.circle_flags = [
            f32::from(paint.circle_pitch_scale == CirclePitchScale::Map),
            f32::from(paint.circle_pitch_alignment == CirclePitchAlignment::Map),
            0.0,
            0.0,
        ];
    }

    if let Some(LayerPaint::FillExtrusion(paint)) = &style_layer.paint {
        let zoom = f64::from(zoom);
        // Translucent volumes fold `fill-extrusion-opacity` into each feature's colour alpha.
        let opacity = if paint.fill_extrusion_translucent == Some(true) {
            1.0
        } else {
            paint
                .fill_extrusion_opacity
                .as_ref()
                .and_then(|opacity| opacity.evaluate_at_zoom(zoom))
                .unwrap_or(1.0)
                .clamp(0.0, 1.0)
        };
        let gradient = paint.fill_extrusion_vertical_gradient.unwrap_or(true);
        let height_blend = paint
            .fill_extrusion_height
            .as_ref()
            .and_then(|height| height.zoom_interpolation_factor(zoom))
            .map_or(0.0, |factor| 1.0 + factor);
        let base_blend = paint
            .fill_extrusion_base
            .as_ref()
            .and_then(|base| base.zoom_interpolation_factor(zoom))
            .map_or(0.0, |factor| 1.0 + factor);
        layer_metadata.stroke_color = [light.color[0], light.color[1], light.color[2], 1.0];
        layer_metadata.circle_params =
            [opacity, f32::from(gradient), light.intensity, height_blend];
        layer_metadata.circle_flags = [
            light.position[0],
            light.position[1],
            light.position[2],
            base_blend,
        ];
    }

    layer_metadata
}

pub(super) fn feature_metadata(
    feature_indices: &[u32],
    feature_colors: &[[f32; 4]],
    fallback_color: [f32; 4],
) -> Vec<FillShaderFeatureMetadata> {
    let mut out = Vec::new();
    feature_metadata_into(&mut out, feature_indices, feature_colors, fallback_color);
    out
}

pub(super) fn feature_metadata_into(
    out: &mut Vec<FillShaderFeatureMetadata>,
    feature_indices: &[u32],
    feature_colors: &[[f32; 4]],
    fallback_color: [f32; 4],
) {
    out.clear();
    let needed = feature_indices.iter().map(|&c| c as usize).sum::<usize>();
    if out.capacity() < needed {
        out.reserve(needed - out.len());
    }
    for (idx, &count) in feature_indices.iter().enumerate() {
        let current_color = feature_colors.get(idx).copied().unwrap_or(fallback_color);
        out.resize(
            out.len() + count as usize,
            FillShaderFeatureMetadata {
                color: current_color,
            },
        );
    }
}

/// Whether `paint`'s per-layer GPU metadata can change when only camera bearing or light changes
/// while zoom remains constant.
#[inline]
pub(super) fn paint_depends_on_bearing(paint: Option<&LayerPaint>) -> bool {
    match paint {
        Some(LayerPaint::FillExtrusion(_)) => true,
        Some(LayerPaint::Fill(p)) => p.fill_translate.is_some(),
        Some(LayerPaint::Line(p)) => p.line_translate.is_some(),
        Some(LayerPaint::Circle(p)) => p.circle_translate.is_some(),
        _ => false,
    }
}

/// Compares two linear RGBA colors at 10-bit precision (`1 / 1024`, `< 0.25 / 255` 8-bit channel
/// error) so sub-pixel zoom interpolations do not re-upload unchanged multi-megabyte vertex-color
/// buffers on every frame while remaining mathematically drift-free by transitivity of equality.
#[inline]
pub(super) fn same_quantized_rgba(a: [f32; 4], b: [f32; 4]) -> bool {
    #[inline]
    fn q10(v: f32) -> u16 {
        (v.clamp(0.0, 1.0) * 1024.0).round() as u16
    }
    q10(a[0]) == q10(b[0])
        && q10(a[1]) == q10(b[1])
        && q10(a[2]) == q10(b[2])
        && q10(a[3]) == q10(b[3])
}

pub(super) fn upload_size(bucket: &crate::vector::AvailableVectorLayerBucket) -> usize {
    bucket.buffer.buffer.vertices.len() * std::mem::size_of::<crate::render::ShaderVertex>()
        + bucket.buffer.buffer.indices.len() * std::mem::size_of::<u32>()
        + bucket
            .feature_indices
            .iter()
            .map(|count| *count as usize)
            .sum::<usize>()
            * std::mem::size_of::<FillShaderFeatureMetadata>()
}

pub(super) fn layer_translate_tile_units(
    paint: Option<&LayerPaint>,
    tile_zoom: crate::coords::ZoomLevel,
    view_zoom: f32,
    bearing: f32,
) -> [f32; 2] {
    let translated = crate::style::translation::layer_translate_pixels(
        paint,
        f64::from(view_zoom),
        f64::from(bearing),
    )
    .map(|pixels| pixels as f32);
    let pixels_to_tile_units = 8.0 * 2.0_f32.powf(f32::from(u8::from(tile_zoom)) - view_zoom);
    [
        translated[0] * pixels_to_tile_units,
        translated[1] * pixels_to_tile_units,
    ]
}

/// The index range of each run of consecutive features that share a sort key.
pub(super) fn sort_runs(
    feature_indices: &[u32],
    feature_sort_keys: &[f32],
    indices: &[u32],
) -> Vec<(f32, std::ops::Range<u32>)> {
    let mut runs: Vec<(f32, std::ops::Range<u32>)> = Vec::new();
    let (mut vertex_end, mut position) = (0_u32, 0_u32);
    for (feature, count) in feature_indices.iter().enumerate() {
        vertex_end = vertex_end.wrapping_add(*count);
        let first = position;
        while indices
            .get(position as usize)
            .is_some_and(|index| *index < vertex_end)
        {
            position = position.wrapping_add(1);
        }
        if position == first {
            continue;
        }
        let key = feature_sort_keys.get(feature).copied().unwrap_or(0.0);
        match runs.last_mut() {
            Some((last, range)) if *last == key && range.end == first => range.end = position,
            _ => runs.push((key, first..position)),
        }
    }
    runs
}

#[cfg(target_arch = "wasm32")]
pub(super) fn can_release_cpu_geometry(
    style_layer: &crate::style::layer::StyleLayer,
    bucket: &crate::vector::AvailableVectorLayerBucket,
    has_dem_source: bool,
) -> bool {
    let edge_y = (1_i32 << u8::from(bucket.coords.z).min(30)) - 1;
    let is_polar_row = bucket.coords.y <= 0 || bucket.coords.y >= edge_y;
    !is_polar_row
        && (!has_dem_source || crate::vector::structures::kind(style_layer).is_none())
        && style_layer
            .paint
            .as_ref()
            .and_then(crate::vector::pattern::per_feature_pattern)
            .is_none()
        && !(style_layer.type_ == "circle" && !bucket.feature_sort_keys.is_empty())
}
