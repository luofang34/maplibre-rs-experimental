//! Paint shared by every feature must use the view zoom, including overzoomed source tiles.
use crate::style::layer::{LayerPaint, StyleLayer};

pub(super) fn uniform_color(layer: &StyleLayer, zoom: f64) -> Option<[f32; 4]> {
    let (color, opacity) = match layer.paint.as_ref()? {
        LayerPaint::Fill(paint) => (&paint.fill_color, &paint.fill_opacity),
        LayerPaint::Line(paint) => (&paint.line_color, &paint.line_opacity),
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
    Some([
        color.r as f32,
        color.g as f32,
        color.b as f32,
        color.a as f32 * opacity,
    ])
}

#[cfg(test)]
mod tests;
