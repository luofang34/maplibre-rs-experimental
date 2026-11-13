use crate::{
    context::MapContext,
    render::render_phase::{LayerItem, RenderPhase, TileMaskItem},
    tcs::system::SystemResult,
};

/// This system sorts all [`RenderPhases`](RenderPhase) for the [`PhaseItem`](crate::render::render_phase::PhaseItem) type.
pub fn sort_phase_system(MapContext { world, .. }: &mut MapContext) -> SystemResult {
    if let Some(layers) = world.resources.get_mut::<RenderPhase<LayerItem>>() {
        layers.sort();
    }
    // Coarser masks first, so the masks of finer tiles lie over them.
    if let Some(masks) = world.resources.get_mut::<RenderPhase<TileMaskItem>>() {
        masks.sort();
    }

    Ok(())
}
