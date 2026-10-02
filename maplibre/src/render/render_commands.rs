//! Stencil commands that stop a command tuple when required tile resources are unavailable.

#![deny(missing_docs)]
use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{PhaseItem, RenderCommand, RenderCommandResult, TileMaskItem},
        tile_mesh::{GlobeTileMeshCache, TileMeshUsage},
        tile_view_pattern::WgpuTileViewPattern,
        MaskPipeline,
    },
    tcs::world::World,
};

/// Binds the stencil pipeline and the phase item's view or flat projection uniform.
pub struct SetMaskPipeline;
impl<P: PhaseItem> RenderCommand<P> for SetMaskPipeline {
    fn render<'w>(
        world: &'w World,
        item: &P,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((Initialized(pipeline), Initialized(projection_resources))) =
            world.resources.query::<(
                &Eventually<MaskPipeline>,
                &Eventually<ProjectionGpuResources>,
            )>()
        else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Draws an indexed tile mask using its current metadata range and cached mesh.
pub struct DrawMask;
impl RenderCommand<TileMaskItem> for DrawMask {
    fn render<'w>(
        world: &'w World,
        item: &TileMaskItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        Self::render_with_reference(
            world,
            item,
            pass,
            u32::from(item.source_shape.coords().stencil_reference_value_3d()),
        )
    }
}

impl DrawMask {
    pub(crate) fn render_with_reference<'w>(
        world: &'w World,
        item: &TileMaskItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
        reference: u32,
    ) -> RenderCommandResult {
        let Some((Initialized(tile_view_pattern), tile_mesh_cache)) = world
            .resources
            .query::<(&Eventually<WgpuTileViewPattern>, &GlobeTileMeshCache)>()
        else {
            return RenderCommandResult::Failure;
        };

        let tile_mask = &item.source_shape;
        let Some(mesh) = tile_mesh_cache.get(
            tile_mask.coords(),
            TileMeshUsage::Stencil,
            item.generate_borders,
        ) else {
            return RenderCommandResult::Failure;
        };

        pass.set_stencil_reference(reference);

        let Some(tile_view_pattern_buffer) = tile_mask.buffer_range() else {
            return RenderCommandResult::Failure;
        };
        pass.set_vertex_buffer(0, mesh.vertex_buffer().slice(..));
        pass.set_vertex_buffer(
            1,
            tile_view_pattern.buffer().slice(tile_view_pattern_buffer),
        );
        pass.set_index_buffer(mesh.index_buffer().slice(..), mesh.index_format());
        pass.draw_indexed(0..mesh.index_count(), 0, 0..1);

        RenderCommandResult::Success
    }
}

/// Binds the stencil pipeline, then draws the mask if all required resources are ready.
pub type DrawMasks = (SetMaskPipeline, DrawMask);
