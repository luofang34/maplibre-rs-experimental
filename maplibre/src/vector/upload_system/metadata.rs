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
    let translate =
        super::layer_translate_tile_units(style_layer.paint.as_ref(), coords.z, zoom, bearing);
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
        layer_metadata.circle_params = [
            value(&paint.line_offset),
            value(&paint.line_gap_width).max(0.0),
            value(&paint.line_blur).max(0.0),
            f32::from(paint.line_gradient.is_some()),
        ];
    }
    if let Some(LayerPaint::Circle(paint)) = &style_layer.paint {
        let zoom = f64::from(zoom);
        layer_metadata.stroke_color = paint.stroke_color_rgba();
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
        let opacity = paint
            .fill_extrusion_opacity
            .as_ref()
            .and_then(|opacity| opacity.evaluate_at_zoom(zoom))
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
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
    let mut feature_metadata = Vec::with_capacity(feature_indices.iter().sum::<u32>() as usize);
    for (idx, &count) in feature_indices.iter().enumerate() {
        let current_color = feature_colors.get(idx).copied().unwrap_or(fallback_color);
        for _ in 0..count {
            feature_metadata.push(FillShaderFeatureMetadata {
                color: current_color,
            });
        }
    }

    feature_metadata
}
