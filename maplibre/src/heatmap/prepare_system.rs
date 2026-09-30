//! Sizes each visible heatmap layer's targets and writes its ramp and opacity for the frame.

use std::collections::HashSet;

use crate::{
    context::MapContext,
    heatmap::resources::HeatmapResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        Renderer,
    },
    style::layer::LayerPaint,
    tcs::system::{SystemError, SystemResult},
};

pub fn prepare_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(Initialized(resources)) = world
        .resources
        .query_mut::<&mut Eventually<HeatmapResources>>()
    else {
        return Err(SystemError::Dependencies);
    };
    let zoom = view_state.zoom().value();
    let size = (
        view_state.width().round() as u32,
        view_state.height().round() as u32,
    );
    let mut written = HashSet::new();
    for layer in &style.layers {
        let Some(LayerPaint::Heatmap(paint)) = &layer.paint else {
            continue;
        };
        if !layer.is_visible_at(view_state.style_zoom().value()) {
            continue;
        }
        resources.write_layer(
            device,
            queue,
            &layer.id,
            size,
            &paint.ramp(),
            paint.opacity_at(zoom),
        );
        written.insert(layer.id.as_str());
    }
    resources.retain_layers(&written);
    Ok(())
}
