//! Evaluates uniform layer colors for geometry buckets with feature-constant paint.

use crate::{
    style::layer::{LayerPaint, StyleLayer, StyleProperty},
    vector::upload_system::metadata::uses_layer_metadata_uniform_color,
};

/// White with the opacity of a fill that repeats an image, unless the opacity varies by feature.
fn pattern_color(opacity: Option<&StyleProperty<f32>>, zoom: f64) -> Option<[f32; 4]> {
    if opacity.is_some_and(|opacity| !opacity.is_feature_constant()) {
        return None;
    }
    let opacity = opacity
        .and_then(|opacity| opacity.evaluate_at_zoom(zoom))
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    Some([1.0, 1.0, 1.0, opacity])
}

pub(crate) fn uniform_color(layer: &StyleLayer, zoom: f64) -> Option<[f32; 4]> {
    let Some(layer_paint) = layer.paint.as_ref() else {
        return uses_layer_metadata_uniform_color(layer).then_some([0.0, 0.0, 0.0, 1.0]);
    };
    let (color, opacity) = match layer_paint {
        // A pattern takes only the opacity of the fill.
        LayerPaint::Fill(paint) if paint.fill_pattern.is_some() => {
            // A pattern that varies by feature rides in the feature colours.
            if crate::vector::pattern::per_feature_pattern(layer_paint).is_some() {
                return None;
            }
            return pattern_color(paint.fill_opacity.as_ref(), zoom);
        }
        // The outline is a feature of its own, with its own colour.
        LayerPaint::Fill(paint) if paint.fill_outline_color.is_some() => return None,
        LayerPaint::Fill(paint) => (&paint.fill_color, &paint.fill_opacity),
        // A gradient's colour comes from its ramp, so the feature colours carry the length.
        LayerPaint::Line(paint) if paint.line_gradient.is_some() => return None,
        // A pattern that varies by feature rides in the feature colours.
        LayerPaint::Line(paint) if paint.line_pattern.is_some() => {
            if crate::vector::pattern::per_feature_pattern(layer_paint).is_some() {
                return None;
            }
            (&paint.line_color, &paint.line_opacity)
        }
        LayerPaint::Line(paint) => (&paint.line_color, &paint.line_opacity),
        LayerPaint::FillExtrusion(paint) if paint.fill_extrusion_pattern.is_some() => {
            if crate::vector::pattern::per_feature_pattern(layer_paint).is_some() {
                return None;
            }
            return if paint.fill_extrusion_translucent == Some(true) {
                pattern_color(paint.fill_extrusion_opacity.as_ref(), zoom)
            } else {
                Some([1.0, 1.0, 1.0, 1.0])
            };
        }
        LayerPaint::FillExtrusion(paint) => {
            let opacity = if paint.fill_extrusion_translucent == Some(true) {
                &paint.fill_extrusion_opacity
            } else {
                &None
            };
            (&paint.fill_extrusion_color, opacity)
        }
        LayerPaint::Circle(paint) => {
            if paint.stroke_color_per_feature().is_some() {
                return None;
            }
            (&paint.circle_color, &paint.circle_opacity)
        }
        LayerPaint::Heatmap(_) => return Some([0.0, 0.0, 0.0, 1.0]),
        _ => return None,
    };
    if color.as_ref().is_some_and(|v| !v.is_feature_constant())
        || opacity.as_ref().is_some_and(|v| !v.is_feature_constant())
    {
        return None;
    }
    let color = color
        .as_ref()
        .and_then(|v| v.evaluate_at_zoom(zoom))
        .unwrap_or_default();
    let opacity = opacity
        .as_ref()
        .and_then(|v| v.evaluate_at_zoom(zoom))
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    Some([color.r, color.g, color.b, color.a * opacity])
}
