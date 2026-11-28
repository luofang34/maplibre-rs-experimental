//! Extruded-polygon (`fill-extrusion`) pipeline binding and draw commands.

use super::extrusion_pass;
use crate::{
    render::{
        pass_refs::PassFrameRefs,
        render_phase::{LayerItem, PhaseItem, RenderCommand, RenderCommandResult},
    },
    tcs::world::World,
};

/// Binds one pass of the extrusion pipeline (selecting the 8-byte compact pipeline when the entry
/// uses uniform height/base encoding) and the item's view or flat projection.
pub struct SetExtrusionPipeline<const PASS: u8>;

impl<const PASS: u8> RenderCommand<LayerItem> for SetExtrusionPipeline<PASS> {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let refs = pass.as_ref().as_ref();
        let (Some(pipeline), Some(projection_resources)) = (
            super::resolve_extrusion_pipeline(refs, world),
            PassFrameRefs::resolve_projection(refs, world),
        ) else {
            return RenderCommandResult::Failure;
        };

        let Some(mode) =
            pipeline.for_globe(projection_resources.is_globe_active(item.projection_binding()))
        else {
            return RenderCommandResult::Failure;
        };
        let entry = super::resolve_vector_entry(refs, world, item);
        let compact = PASS != extrusion_pass::PATTERN
            && entry.is_some_and(crate::vector::resource::IndexEntry::is_compact_extrusion);
        let uniform =
            entry.is_some_and(crate::vector::resource::IndexEntry::is_uniform_feature_metadata);
        let selected = if compact {
            match PASS {
                extrusion_pass::DEPTH => mode.compact_depth.as_ref(),
                extrusion_pass::COLOR => mode.compact_color.as_ref(),
                extrusion_pass::TRANSLUCENT_BACK => mode.compact_translucent_back.as_ref(),
                extrusion_pass::TRANSLUCENT_FRONT => mode.compact_translucent_front.as_ref(),
                _ => None,
            }
        } else {
            match PASS {
                extrusion_pass::DEPTH => mode.depth.as_ref(),
                extrusion_pass::COLOR => mode.color.as_ref().map(|p| p.select(uniform)),
                extrusion_pass::PATTERN => mode.pattern.as_ref().map(|p| p.select(uniform)),
                extrusion_pass::TRANSLUCENT_BACK => {
                    mode.translucent_back.as_ref().map(|p| p.select(uniform))
                }
                extrusion_pass::TRANSLUCENT_FRONT => {
                    mode.translucent_front.as_ref().map(|p| p.select(uniform))
                }
                _ => None,
            }
        };
        let Some(selected) = selected else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(selected);
        pass.set_bind_group(
            0,
            projection_resources.bind_group_for(item.projection_binding()),
            &[],
        );
        RenderCommandResult::Success
    }
}

/// Clears bit 7 (`STENCIL_MARK`) across the viewport in a single 3-vertex fullscreen triangle.
pub struct DrawExtrusionStencilClear;
impl RenderCommand<LayerItem> for DrawExtrusionStencilClear {
    fn render<'w>(
        world: &'w World,
        _item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(pipeline) = super::resolve_extrusion_pipeline(pass.as_ref().as_ref(), world)
        else {
            return RenderCommandResult::Failure;
        };
        let Some(clear_mark) = pipeline.clear_mark.as_ref() else {
            return RenderCommandResult::Failure;
        };
        pass.set_pipeline(clear_mark);
        pass.draw(0..3, 0..1);
        RenderCommandResult::Success
    }
}

/// Fused command binding the extrusion pipeline and drawing the extruded bucket in one query and
/// entry lookup.
pub struct DrawExtrusion<const PASS: u8, const RUNS: bool = false>;
impl<const PASS: u8, const RUNS: bool> RenderCommand<LayerItem> for DrawExtrusion<PASS, RUNS> {
    fn render<'w>(
        world: &'w World,
        item: &LayerItem,
        pass: &mut crate::render::tracked_pass::TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        use crate::render::render_phase::LayerDrawKind;
        let kind = match PASS {
            extrusion_pass::DEPTH => LayerDrawKind::ExtrusionDepth,
            extrusion_pass::COLOR => LayerDrawKind::ExtrusionColor,
            extrusion_pass::PATTERN => LayerDrawKind::ExtrusionPatternColor,
            extrusion_pass::TRANSLUCENT_BACK => LayerDrawKind::ExtrusionTranslucentBack,
            extrusion_pass::TRANSLUCENT_FRONT => LayerDrawKind::ExtrusionTranslucentFront,
            _ => return RenderCommandResult::Failure,
        };
        let _ = RUNS;
        if super::try_draw_vector_group_with_kind(world, std::slice::from_ref(item), pass, kind) {
            RenderCommandResult::Success
        } else {
            RenderCommandResult::Failure
        }
    }
}

/// Draws an extruded bucket into depth only.
pub type DrawExtrusionDepth = DrawExtrusion<{ extrusion_pass::DEPTH }>;
/// Draws an extruded bucket where its depth pass left the nearest surface, once per pixel.
pub type DrawExtrusionColor = DrawExtrusion<{ extrusion_pass::COLOR }>;
/// Draws an extruded bucket from an image, once per pixel.
pub type DrawExtrusionPatternColor = DrawExtrusion<{ extrusion_pass::PATTERN }, true>;
/// Draws the back-facing shell of a translucent 3D volume bucket.
pub type DrawExtrusionTranslucentBack = DrawExtrusion<{ extrusion_pass::TRANSLUCENT_BACK }>;
/// Draws the front-facing shell of a translucent 3D volume bucket.
pub type DrawExtrusionTranslucentFront = DrawExtrusion<{ extrusion_pass::TRANSLUCENT_FRONT }>;
