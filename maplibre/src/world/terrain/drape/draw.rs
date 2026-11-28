//! Draws the terrain tiles of the current frame into the main pass.

use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
    },
    tcs::world::World,
    terrain::resources::TerrainResources,
};

/// Draws every terrain tile queued for this frame, writing depth.
pub fn draw_terrain<'w>(
    pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    world: &'w World,
) {
    let Some((Initialized(terrain), Initialized(projection))) = world.resources.query::<(
        &Eventually<TerrainResources>,
        &Eventually<ProjectionGpuResources>,
    )>() else {
        return;
    };
    terrain.draw(pass, projection);
}
