//! What the frame's terrain hands the main pass: whether it draws, its ground surface, and
//! which layers the drapes already show.

use std::collections::HashSet;

use crate::{
    render::render_phase::{LayerItem, RenderPhase},
    style::Style,
    tcs::world::World,
    terrain::{drape_targets::is_drapeable, TerrainFrame},
};

/// Records whether the frame draws terrain, and hands the main pass its ground surface.
pub(super) fn set_frame(world: &mut World, frame: TerrainFrame) {
    let ground = frame
        .active
        .then_some(crate::render::ground::GroundSurface {
            draw_after_layer_index: frame.draw_after_layer_index,
            draw: crate::terrain::draw_terrain,
        });
    world
        .resources
        .insert(crate::render::ground::FrameGround(ground));
    world.resources.insert(frame);
}

pub(super) fn hide_draped_layers(world: &mut World, style: &Style) {
    let drapeable: HashSet<&str> = style
        .layers
        .iter()
        .filter(|layer| {
            is_drapeable(&layer.type_) && crate::vector::structures::kind(layer).is_none()
        })
        .map(|layer| layer.id.as_str())
        .collect();
    if let Some(layer_phase) = world.resources.get_mut::<RenderPhase<LayerItem>>() {
        layer_phase.retain(|item| !drapeable.contains(item.style_layer.as_str()));
    }
}
