//! Repeating-image pattern binding and fused draw commands.

use super::DrawVectorTile;
use crate::{
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ProjectionGpuResources,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
        tile_view_pattern::WgpuTileViewPattern,
    },
    tcs::world::World,
    vector::{pattern::PatternResources, VectorBufferPool},
};

/// Binds the pattern pipeline and the pattern of the item's layer.
pub struct SetPatternTilePipeline;
impl RenderCommand<LayerItem> for SetPatternTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (Some(patterns), Some(Initialized(projection_resources))) = (
            world.resources.get::<PatternResources>(),
            world.resources.get::<Eventually<ProjectionGpuResources>>(),
        ) else {
            return RenderCommandResult::Failure;
        };
        let binding = patterns.binding(item.style_layer);
        if binding.is_none() && !patterns.is_per_feature(item.style_layer) {
            return RenderCommandResult::Failure;
        }
        let Some(pipeline) = patterns.pipeline() else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline.select(super::is_uniform_entry(world, item)));
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        if let Some(binding) = binding {
            pass.set_bind_group(1, binding, &[]);
        }
        RenderCommandResult::Success
    }
}

/// Fused command binding the pattern pipeline/texture and drawing the bucket in one query and
/// entry lookup.
pub struct DrawPatternTiles;
impl RenderCommand<LayerItem> for DrawPatternTiles {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some((
            patterns,
            Initialized(projection_resources),
            Initialized(buffer_pool),
            Initialized(tile_view_pattern),
        )) = world.resources.query::<(
            &PatternResources,
            &Eventually<ProjectionGpuResources>,
            &Eventually<VectorBufferPool>,
            &Eventually<WgpuTileViewPattern>,
        )>()
        else {
            return RenderCommandResult::Failure;
        };
        let binding = patterns.binding(item.style_layer);
        if binding.is_none() && !patterns.is_per_feature(item.style_layer) {
            return RenderCommandResult::Failure;
        }
        let Some(pipeline) = patterns.pipeline() else {
            return RenderCommandResult::Failure;
        };
        let Some(entry) =
            buffer_pool
                .index()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return RenderCommandResult::Failure;
        };
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }
        pass.set_pipeline(pipeline.select(entry.is_uniform_feature_metadata()));
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        if let Some(binding) = binding {
            pass.set_bind_group(1, binding, &[]);
        }
        DrawVectorTile::<true>::draw_prepared(
            world,
            buffer_pool,
            tile_view_pattern,
            entry,
            item,
            pass,
        )
    }
}

/// Binds the image of the item's layer at the second bind group.
pub struct SetPatternBindGroup;
impl RenderCommand<LayerItem> for SetPatternBindGroup {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(patterns) = world.resources.get::<PatternResources>() else {
            return RenderCommandResult::Failure;
        };
        match patterns.binding(item.style_layer) {
            Some(binding) => pass.set_bind_group(1, binding, &[]),
            None if patterns.is_per_feature(item.style_layer) => {}
            None => return RenderCommandResult::Failure,
        }
        RenderCommandResult::Success
    }
}
