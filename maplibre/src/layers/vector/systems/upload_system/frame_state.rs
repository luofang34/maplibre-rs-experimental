//! Per-frame quantized paint state for vector uploads and drape shading.

use crate::{
    render::view_state::ViewState,
    style::Style,
    tcs::{paint_zoom::stabilize_zoom, world::World},
};

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct VectorPaintFrame {
    pub(crate) zoom: f32,
    pub(crate) bearing: f32,
    /// The root light as extruded polygons are shaded with it.
    pub(crate) light: crate::style::light::ExtrusionLight,
}

/// Computes the current frame's quantized paint state alongside the previous frame's state.
pub(in crate::layers::vector::systems::upload_system) fn frame_paint(
    world: &mut World,
    style: &Style,
    view_state: &ViewState,
) -> (VectorPaintFrame, Option<VectorPaintFrame>) {
    let zoom = if style.terrain.is_some() {
        stabilize_zoom(world, view_state.style_zoom().value()) as f32
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
    let previous_paint = if let Some(slot) = world.resources.get_mut::<VectorPaintFrame>() {
        let prev = *slot;
        *slot = paint_frame;
        Some(prev)
    } else {
        world.resources.insert(paint_frame);
        None
    };
    (paint_frame, previous_paint)
}
