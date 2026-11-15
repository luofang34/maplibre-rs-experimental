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
    if matches!(&style_layer.paint, Some(LayerPaint::Fill(_))) {
        if let Some(color) = super::paint::uniform_color(style_layer, f64::from(zoom)) {
            layer_metadata.stroke_color = color;
        }
    }
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
                || [
                    p.line_width.as_ref(),
                    p.line_offset.as_ref(),
                    p.line_gap_width.as_ref(),
                    p.line_blur.as_ref(),
                ]
                .into_iter()
                .flatten()
                .any(|v| !v.is_zoom_constant())
        }
        Some(LayerPaint::Circle(_) | LayerPaint::Heatmap(_)) => true,
        Some(LayerPaint::FillExtrusion(p)) => {
            p.fill_extrusion_translate.is_some()
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
        Some(LayerPaint::Line(p)) => {
            p.line_color.as_ref().is_some_and(|v| !v.is_zoom_constant())
                || p.line_opacity
                    .as_ref()
                    .is_some_and(|v| !v.is_zoom_constant())
        }
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
    bucket: &crate::vector::AvailableVectorLayerBucket,
    is_uniform: bool,
) -> usize {
    let fm_bytes = if is_uniform {
        std::mem::size_of::<FillShaderFeatureMetadata>()
    } else {
        bucket
            .feature_indices
            .iter()
            .map(|count| *count as usize)
            .sum::<usize>()
            * std::mem::size_of::<FillShaderFeatureMetadata>()
    };
    bucket.buffer.buffer.vertices.len() * std::mem::size_of::<crate::render::ShaderVertex>()
        + bucket.buffer.buffer.indices.len() * std::mem::size_of::<u32>()
        + fm_bytes
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

pub(super) fn find_shared_fallback_vertices<'a>(
    bucket: &crate::vector::AvailableVectorLayerBucket,
    layers: &'a crate::vector::VectorLayerBucketComponent,
    pending: &'a crate::vector::content::LayerReplacements,
) -> Option<&'a [crate::render::ShaderVertex]> {
    let shared_key = bucket
        .buffer
        .shared_geometry_key()
        .filter(|_| bucket.buffer.buffer.vertices.is_empty())?;
    pending
        .vector
        .iter()
        .find(|sib| {
            sib.buffer.shared_geometry_key() == Some(shared_key)
                && !sib.buffer.buffer.vertices.is_empty()
        })
        .or_else(|| {
            layers.layers.iter().find_map(|sib| match sib {
                crate::vector::VectorLayerBucket::AvailableLayer(sib)
                    if sib.buffer.shared_geometry_key() == Some(shared_key)
                        && !sib.buffer.buffer.vertices.is_empty() =>
                {
                    Some(sib)
                }
                _ => None,
            })
        })
        .map(|sib| sib.buffer.buffer.vertices.as_slice())
}

#[cfg(target_arch = "wasm32")]
pub(super) fn flush_safe_cpu_releases(
    tiles: &mut crate::tcs::tiles::Tiles,
    buffer_pool: &crate::vector::VectorBufferPool,
    coords: WorldTileCoords,
    releases: &[(String, Option<u64>, bool)],
) {
    for (layer_id, shared_key, release_colors) in releases {
        let sibling_pending = shared_key.is_some_and(|key| {
            tiles
                .query::<(
                    &crate::vector::VectorLayerBucketComponent,
                    &crate::vector::content::LayerReplacements,
                )>(coords)
                .is_some_and(|(layers, pending)| {
                    pending.vector.iter().any(|sib| {
                        sib.buffer.shared_geometry_key() == Some(key)
                            && !sib.buffer.buffer.indices.is_empty()
                    }) || layers.layers.iter().any(|sib| match sib {
                        crate::vector::VectorLayerBucket::AvailableLayer(sib) => {
                            sib.buffer.shared_geometry_key() == Some(key)
                                && !sib.buffer.buffer.indices.is_empty()
                                && !buffer_pool.has_layer(coords, &sib.style_layer_id)
                        }
                        _ => false,
                    })
                })
        });
        if !sibling_pending {
            crate::vector::content::release_committed_cpu_geometry(
                tiles,
                coords,
                layer_id,
                *release_colors,
            );
        }
    }
}
