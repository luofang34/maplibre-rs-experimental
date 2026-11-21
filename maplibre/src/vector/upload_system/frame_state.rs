//! Per-frame quantized paint state for vector uploads and drape shading.

use super::paint;
use crate::{render::view_state::ViewState, style::Style, tcs::world::World};

/// Rounds `zoom` to eighth-zoom steps for drape paint evaluation.
pub(crate) fn drape_paint_zoom(zoom: f64) -> f64 {
    (zoom * 8.0).round() / 8.0
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::vector::upload_system) struct VectorPaintFrame {
    pub(in crate::vector::upload_system) zoom: f32,
    pub(in crate::vector::upload_system) bearing: f32,
    /// The root light as extruded polygons are shaded with it.
    pub(in crate::vector::upload_system) light: crate::style::light::ExtrusionLight,
}

/// Computes the current frame's quantized paint state alongside the previous frame's state.
pub(in crate::vector::upload_system) fn frame_paint(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
) -> (VectorPaintFrame, Option<VectorPaintFrame>) {
    let zoom = if style.terrain.is_some() {
        paint::stabilize_zoom(world, view_state.style_zoom().value()) as f32
    } else {
        (view_state.style_zoom().level() * 256.0).round() / 256.0
    };
    let bearing = ((view_state.camera().get_bearing().0.to_degrees() * 4.0).round() * 0.25)
        .to_radians() as f32;
    let light = style
        .light
        .as_ref()
        .map_or_else(
            || {
                crate::style::light::LightSpecification::default()
                    .extrusion_light(view_state.style_zoom().value(), f64::from(bearing))
            },
            |light| light.extrusion_light(view_state.style_zoom().value(), f64::from(bearing)),
        )
        .unwrap_or_default();
    let paint_frame = VectorPaintFrame {
        zoom,
        bearing,
        light,
    };
    let previous_paint = world.resources.get::<VectorPaintFrame>().copied();
    world.resources.insert(paint_frame);
    (paint_frame, previous_paint)
}
