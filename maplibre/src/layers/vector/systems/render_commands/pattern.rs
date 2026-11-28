//! Repeating-image pattern binding and fused draw commands.

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
        if super::try_draw_vector_group_with_kind(
            world,
            std::slice::from_ref(item),
            pass,
            crate::render::render_phase::LayerDrawKind::VectorPattern,
        ) {
            RenderCommandResult::Success
        } else {
            RenderCommandResult::Failure
        }
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
