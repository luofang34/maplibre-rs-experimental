//! Repeating-image pattern binding and fused draw commands.

use super::DrawVectorTile;
use crate::{
    render::{
        pass_refs::PassFrameRefs,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
    },
    tcs::world::World,
};

/// Binds the pattern pipeline and the pattern of the item's layer.
pub struct SetPatternTilePipeline;
impl RenderCommand<LayerItem> for SetPatternTilePipeline {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let refs = pass.as_ref().as_ref();
        let (Some(patterns), Some(projection_resources)) = (
            super::resolve_pattern_resources(refs, world),
            PassFrameRefs::resolve_projection(refs, world),
        ) else {
            return RenderCommandResult::Failure;
        };
        let binding = patterns.binding(item.style_layer);
        if binding.is_none() && !patterns.is_per_feature(item.style_layer) {
            return RenderCommandResult::Failure;
        }
        let uniform = super::is_uniform_entry(refs, world, item);
        let Some(pipeline) = patterns.select_pipeline(uniform) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline);
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
        let refs = pass.as_ref().as_ref();
        let (
            Some(patterns),
            Some(projection_resources),
            Some(buffer_pool),
            Some(tile_view_pattern),
        ) = (
            super::resolve_pattern_resources(refs, world),
            PassFrameRefs::resolve_projection(refs, world),
            super::resolve_vector_pool(refs, world),
            PassFrameRefs::resolve_tile_view_pattern(refs, world),
        )
        else {
            return RenderCommandResult::Failure;
        };
        let binding = patterns.binding(item.style_layer);
        if (binding.is_none() && !patterns.is_per_feature(item.style_layer))
            || !patterns.has_pipeline()
        {
            return RenderCommandResult::Failure;
        }
        let Some(entry) =
            buffer_pool
                .as_ref()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return RenderCommandResult::Failure;
        };
        if entry.indices_buffer_range().is_empty() {
            tracing::error!("Tried to draw a vector tile without any vertices");
            return RenderCommandResult::Failure;
        }
        let Some(pipeline) = patterns.select_pipeline(entry.is_uniform_feature_metadata()) else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(pipeline);
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
        let Some(patterns) = super::resolve_pattern_resources(pass.as_ref().as_ref(), world) else {
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
