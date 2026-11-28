use super::VectorPaintFrame;
use crate::{
    coords::WorldTileCoords,
    render::shaders::{FillShaderFeatureMetadata, ShaderLayerMetadata},
    style::{
        circle::{CirclePitchAlignment, CirclePitchScale},
        layer::{LayerPaint, StyleLayer},
    },
    vector::uniform_paint::{uniform_color, uses_layer_metadata_uniform_color},
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
    if uses_layer_metadata_uniform_color(style_layer) {
        if let Some(color) = uniform_color(style_layer, f64::from(zoom)) {
            layer_metadata.uniform_color = color;
        }
    }
    apply_paint_metadata(style_layer, zoom, translate, light, &mut layer_metadata);
    layer_metadata
}

fn apply_paint_metadata(
    style_layer: &StyleLayer,
    zoom: f32,
    translate: [f32; 2],
    light: crate::style::light::ExtrusionLight,
    layer_metadata: &mut ShaderLayerMetadata,
) {
    let zoom_f64 = f64::from(zoom);
    match &style_layer.paint {
        Some(LayerPaint::Heatmap(paint)) => {
            // Radius and intensity are one value per layer, taken at the view zoom.
            layer_metadata.line_width = if paint.radius_per_feature() {
                1.0
            } else {
                paint.radius_at(zoom_f64)
            };
            layer_metadata.circle_params = [paint.intensity_at(zoom_f64), 1.0, 0.0, 0.0];
        }
        Some(LayerPaint::Line(paint)) => {
            let value = |property: &Option<crate::style::layer::StyleProperty<f32>>| {
                property
                    .as_ref()
                    .and_then(|property| property.evaluate_at_zoom(zoom_f64))
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
        Some(LayerPaint::Circle(paint)) => {
            layer_metadata.stroke_color = paint.stroke_color_at(zoom_f64);
            // Fill opacity is already folded into each feature's colour alpha.
            layer_metadata.circle_params = [
                1.0,
                paint.stroke_opacity_at(zoom_f64),
                paint.blur_at(zoom_f64),
                0.0,
            ];
            layer_metadata.circle_flags = [
                f32::from(paint.circle_pitch_scale == CirclePitchScale::Map),
                f32::from(paint.circle_pitch_alignment == CirclePitchAlignment::Map),
                translate[0],
                translate[1],
            ];
        }
        None if style_layer.type_ == "circle" => {
            layer_metadata.stroke_color = [0.0, 0.0, 0.0, 1.0];
            layer_metadata.circle_params = [1.0, 1.0, 0.0, 0.0];
            layer_metadata.circle_flags = [1.0, 0.0, translate[0], translate[1]];
        }
        Some(LayerPaint::FillExtrusion(paint)) => {
            apply_extrusion_metadata(paint, zoom_f64, light, layer_metadata);
        }
        _ => {}
    }
}

fn apply_extrusion_metadata(
    paint: &crate::style::layer::FillExtrusionPaint,
    zoom: f64,
    light: crate::style::light::ExtrusionLight,
    layer_metadata: &mut ShaderLayerMetadata,
) {
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
    layer_metadata.circle_params = [opacity, f32::from(gradient), light.intensity, height_blend];
    layer_metadata.circle_flags = [
        light.position[0],
        light.position[1],
        light.position[2],
        base_blend,
    ];
}

pub(super) fn feature_metadata(
    style_layer: &StyleLayer,
    feature_indices: &[u32],
    feature_colors: &[[f32; 4]],
    fallback_color: [f32; 4],
) -> Vec<FillShaderFeatureMetadata> {
    let mut out = Vec::new();
    feature_metadata_into(
        &mut out,
        style_layer,
        feature_indices,
        feature_colors,
        fallback_color,
    );
    out
}

pub(super) fn feature_metadata_into(
    out: &mut Vec<FillShaderFeatureMetadata>,
    style_layer: &StyleLayer,
    feature_indices: &[u32],
    feature_colors: &[[f32; 4]],
    fallback_color: [f32; 4],
) {
    out.clear();
    let is_line_gradient = matches!(
        style_layer.paint,
        Some(LayerPaint::Line(ref paint)) if paint.line_gradient.is_some()
    );
    let has_packed_circle = !is_line_gradient
        && style_layer.type_ == "circle"
        && feature_colors.iter().any(|c| c[0] < 0.0);
    let total_vertices = feature_indices.iter().map(|&c| c as usize).sum::<usize>();
    let needed = if has_packed_circle {
        total_vertices * 2
    } else {
        total_vertices
    };
    if out.capacity() < needed {
        out.reserve(needed - out.len());
    }
    if has_packed_circle {
        let default_stroke = match &style_layer.paint {
            Some(LayerPaint::Circle(p)) => p.stroke_color_at(0.0),
            _ => [0.0, 0.0, 0.0, 1.0],
        };
        for (idx, &count) in feature_indices.iter().enumerate() {
            let current_color = feature_colors.get(idx).copied().unwrap_or(fallback_color);
            let pair = if current_color[0] < 0.0 {
                FillShaderFeatureMetadata::unpack_circle_pair(current_color)
            } else {
                [
                    FillShaderFeatureMetadata::from_rgba(current_color),
                    FillShaderFeatureMetadata::from_rgba(default_stroke),
                ]
            };
            for _ in 0..count {
                out.extend_from_slice(&pair);
            }
        }
        return;
    }
    for (idx, &count) in feature_indices.iter().enumerate() {
        let current_color = feature_colors.get(idx).copied().unwrap_or(fallback_color);
        let encoded = if is_line_gradient {
            FillShaderFeatureMetadata::from_line_gradient(current_color[0], current_color[3])
        } else {
            FillShaderFeatureMetadata::from_rgba(current_color)
        };
        out.resize(out.len() + count as usize, encoded);
    }
}

/// Whether `paint`'s per-layer GPU metadata can change when camera bearing changes while zoom and
/// extrusion lighting remain constant.
#[inline]
pub(super) fn paint_depends_on_bearing(paint: Option<&LayerPaint>) -> bool {
    match paint {
        Some(LayerPaint::FillExtrusion(p)) => p.fill_extrusion_translate.is_some(),
        Some(LayerPaint::Fill(p)) => p.fill_translate.is_some(),
        Some(LayerPaint::Line(p)) => p.line_translate.is_some(),
        Some(LayerPaint::Circle(p)) => p.circle_translate.is_some(),
        _ => false,
    }
}

/// Whether `paint`'s [`ShaderLayerMetadata`] fields can change when only view zoom changes.
#[inline]
pub(super) fn paint_metadata_depends_on_zoom(paint: Option<&LayerPaint>) -> bool {
    match paint {
        Some(LayerPaint::Fill(p)) => {
            p.fill_translate.is_some()
                || p.fill_color.as_ref().is_some_and(|v| !v.is_zoom_constant())
                || p.fill_opacity
                    .as_ref()
                    .is_some_and(|v| !v.is_zoom_constant())
        }
        Some(LayerPaint::Line(p)) => {
            p.line_translate.is_some()
                || p.line_color.as_ref().is_some_and(|v| !v.is_zoom_constant())
                || [
                    p.line_opacity.as_ref(),
                    p.line_width.as_ref(),
                    p.line_offset.as_ref(),
                    p.line_gap_width.as_ref(),
                    p.line_blur.as_ref(),
                ]
                .into_iter()
                .flatten()
                .any(|v| !v.is_zoom_constant())
        }
        Some(LayerPaint::Heatmap(_)) => true,
        Some(LayerPaint::Circle(p)) => {
            p.circle_translate.is_some()
                || p.circle_color
                    .as_ref()
                    .is_some_and(|v| !v.is_zoom_constant())
                || p.circle_stroke_color
                    .as_ref()
                    .is_some_and(|v| !v.is_zoom_constant())
                || [
                    p.circle_opacity.as_ref(),
                    p.circle_stroke_opacity.as_ref(),
                    p.circle_blur.as_ref(),
                ]
                .into_iter()
                .flatten()
                .any(|v| !v.is_zoom_constant())
        }
        Some(LayerPaint::FillExtrusion(p)) => {
            p.fill_extrusion_translate.is_some()
                || p.fill_extrusion_color
                    .as_ref()
                    .is_some_and(|v| !v.is_zoom_constant())
                || p.fill_extrusion_opacity
                    .as_ref()
                    .is_some_and(|v| !v.is_zoom_constant())
                || p.fill_extrusion_height
                    .as_ref()
                    .is_some_and(|v| !v.is_zoom_constant())
                || p.fill_extrusion_base
                    .as_ref()
                    .is_some_and(|v| !v.is_zoom_constant())
        }
        _ => false,
    }
}

/// Whether `layer`'s uniform `FillShaderFeatureMetadata` record can change when view zoom changes.
#[inline]
pub(super) fn uniform_color_depends_on_zoom(layer: &StyleLayer) -> bool {
    match layer.paint.as_ref() {
        Some(LayerPaint::FillExtrusion(p)) => {
            p.fill_extrusion_color
                .as_ref()
                .is_some_and(|v| !v.is_zoom_constant())
                || (p.fill_extrusion_translucent == Some(true)
                    && p.fill_extrusion_opacity
                        .as_ref()
                        .is_some_and(|v| !v.is_zoom_constant()))
        }
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

pub(super) fn upload_size(
    style_layer: &StyleLayer,
    bucket: &crate::vector::AvailableVectorLayerBucket,
    is_uniform: bool,
    is_dynamic_circle: bool,
) -> usize {
    let vtx_count = bucket.buffer.vertex_count();
    let fm_bytes = if is_dynamic_circle {
        vtx_count * 2 * std::mem::size_of::<FillShaderFeatureMetadata>()
    } else if is_uniform {
        if uses_layer_metadata_uniform_color(style_layer) {
            0
        } else {
            std::mem::size_of::<FillShaderFeatureMetadata>()
        }
    } else {
        bucket
            .feature_indices
            .iter()
            .map(|count| *count as usize)
            .sum::<usize>()
            * std::mem::size_of::<FillShaderFeatureMetadata>()
    };
    let index_stride = if vtx_count <= 65_536 {
        std::mem::size_of::<u16>()
    } else {
        std::mem::size_of::<u32>()
    };
    vtx_count * std::mem::size_of::<crate::render::shaders::GpuShaderVertex>()
        + bucket.buffer.cpu_index_len() * index_stride
        + fm_bytes
}

pub(super) fn layer_translate_tile_units(
    paint: Option<&LayerPaint>,
    tile_zoom: crate::coords::ZoomLevel,
    view_zoom: f32,
    bearing: f32,
) -> [f32; 2] {
    let translated = crate::style::layer::layer_translate_pixels(
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

/// The index range of each run of consecutive features that share a pattern key.
pub(super) fn pattern_runs(
    feature_indices: &[u32],
    feature_colors: &[[f32; 4]],
    indices: &[u32],
) -> Vec<(u32, std::ops::Range<u32>)> {
    let mut runs: Vec<(u32, std::ops::Range<u32>)> = Vec::new();
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
        let key = feature_colors
            .get(feature)
            .and_then(|color| crate::style::pattern_key::key_of_value(color[0]));
        let Some(key) = key.filter(|_| position > first) else {
            continue;
        };
        match runs.last_mut() {
            Some((last, range)) if *last == key && range.end == first => range.end = position,
            _ => runs.push((key, first..position)),
        }
    }
    runs
}
