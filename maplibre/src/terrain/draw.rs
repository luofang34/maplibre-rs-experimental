//! Draws the terrain tiles of the current frame into the main pass.

use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        resource::TrackedRenderPass,
    },
    tcs::world::World,
    terrain::resources::TerrainResources,
};

/// Draws every terrain tile queued for this frame, writing depth.
pub fn draw_terrain<'w>(pass: &mut TrackedRenderPass<'w>, world: &'w World) {
    let Some((Initialized(terrain), Initialized(projection))) = world.resources.query::<(
        &Eventually<TerrainResources>,
        &Eventually<ProjectionGpuResources>,
    )>() else {
        return;
    };
    if terrain.draws().is_empty() {
        tracing::trace!("no terrain draws queued");
        return;
    }
    tracing::trace!(draws = terrain.draws().len(), "drawing terrain");
    pass.set_render_pipeline(terrain.pipeline());
    pass.set_bind_group(0, projection.bind_group(), &[]);
    pass.set_vertex_buffer(0, terrain.vertex_buffer().slice(..));
    pass.set_index_buffer(terrain.index_buffer().slice(..), wgpu::IndexFormat::Uint32);
    for draw in terrain.draws() {
        pass.set_bind_group(1, &draw.bind_group, &[draw.uniform_offset]);
        pass.draw_indexed(0..terrain.index_count(), 0, 0..1);
    }
}
