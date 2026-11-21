//! What the frame's terrain hands the main pass: whether it draws, its ground surface, and
//! which layers the drapes already show.

use rustc_hash::FxHashSet;

use crate::{
    render::render_phase::{LayerItem, RenderPhase},
    style::{Style, StyleLayerId},
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

/// Caches the style's drapeable layer IDs by `style.revision()` so terrain frames do not
/// rebuild the filter set repeatedly.
#[derive(Default)]
struct DrapeableLayersCache {
    style_rev: Option<u64>,
    layers: FxHashSet<StyleLayerId>,
}

impl DrapeableLayersCache {
    fn layers(&mut self, style: &Style) -> &FxHashSet<StyleLayerId> {
        let rev = style.revision();
        if self.style_rev != Some(rev) {
            self.style_rev = Some(rev);
            self.layers.clear();
            self.layers.extend(
                style
                    .layers
                    .iter()
                    .filter(|layer| {
                        is_drapeable(&layer.type_)
                            && crate::vector::structures::kind(layer).is_none()
                    })
                    .map(crate::style::layer::StyleLayer::layer_id),
            );
        }
        &self.layers
    }
}

pub(super) fn hide_draped_layers(world: &mut World, style: &Style) {
    let mut cache = world
        .resources
        .remove::<DrapeableLayersCache>()
        .unwrap_or_default();
    let drapeable = cache.layers(style);
    if let Some(layer_phase) = world.resources.get_mut::<RenderPhase<LayerItem>>() {
        layer_phase.retain(|item| !drapeable.contains(&item.style_layer));
    }
    world.resources.insert(cache);
}
