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

#[derive(Default)]
struct PaintZoom(Option<f64>);

pub(super) fn stabilize_zoom(world: &mut crate::tcs::world::World, requested: f64) -> f64 {
    let state = world.resources.get_or_init_mut::<PaintZoom>();
    let zoom = next_zoom(state.0, requested);
    state.0 = Some(zoom);
    zoom
}

fn next_zoom(previous: Option<f64>, requested: f64) -> f64 {
    previous
        .filter(|previous| (requested - previous).abs() < 0.125)
        .unwrap_or_else(|| super::drape_paint_zoom(requested))
}

pub(crate) fn current_zoom(world: &crate::tcs::world::World, fallback: f64) -> f64 {
    world
        .resources
        .get::<PaintZoom>()
        .and_then(|state| state.0)
        .unwrap_or_else(|| super::drape_paint_zoom(fallback))
}
