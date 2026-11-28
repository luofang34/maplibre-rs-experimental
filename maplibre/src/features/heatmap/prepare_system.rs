//! Sizes each visible heatmap layer's targets and writes its ramp and opacity for the frame.

use std::collections::HashSet;

use crate::{
    context::MapContext,
    heatmap::resources::HeatmapResources,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        Renderer,
    },
    schedule::system::{SystemError, SystemResult},
    style::layer::LayerPaint,
};

/// Calculates the downscaled (1/4x) density target dimensions for a given viewport size,
/// ensuring both width and height are quarter-resolution and never 0.
#[inline]
pub fn density_target_size(width: f64, height: f64) -> (u32, u32) {
    (
        (width.round() as u32 >> 2).max(1),
        (height.round() as u32 >> 2).max(1),
    )
}

pub fn prepare_system(
    MapContext {
        world,
        style,
        view_state,
        renderer: Renderer { device, queue, .. },
        ..
    }: &mut MapContext,
) -> SystemResult {
    let Some(heatmap_resources) = world
        .resources
        .query_mut::<&mut Eventually<HeatmapResources>>()
    else {
        return Err(SystemError::Dependencies);
    };
    let Initialized(resources) = heatmap_resources else {
        return Ok(());
    };
    let zoom = view_state.zoom().value();
    let size = density_target_size(view_state.width(), view_state.height());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_target_dimensions_are_quarter_resolution_and_never_zero() {
        // Full HD viewport is downscaled to 1/4x (480x270)
        assert_eq!(density_target_size(1920.0, 1080.0), (480, 270));

        // 4K viewport
        assert_eq!(density_target_size(3840.0, 2160.0), (960, 540));

        // Sub-4 pixel dimensions must clamp to at least 1 (never zero)
        assert_eq!(density_target_size(4.0, 4.0), (1, 1));
        assert_eq!(density_target_size(3.0, 3.0), (1, 1));
        assert_eq!(density_target_size(2.0, 2.0), (1, 1));
        assert_eq!(density_target_size(1.0, 1.0), (1, 1));
        assert_eq!(density_target_size(0.0, 0.0), (1, 1));

        // Negative dimensions clamp to at least 1
        assert_eq!(density_target_size(-100.0, -100.0), (1, 1));
    }
}
